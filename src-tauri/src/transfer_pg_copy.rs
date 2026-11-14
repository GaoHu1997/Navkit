//! 显式选择的 PostgreSQL text COPY；能力检查与写入共享一个事务。
use super::{ImportMapper, RowFeeder, TaskControl, TransferLimits};
use anyhow::{anyhow, bail, Context, Result};
use sqlx::{Connection, PgConnection, PgPool, Row};

const VALUE_BYTES: usize = 1024 * 1024;
const ROW_BYTES: usize = 4 * 1024 * 1024;
const CHUNK_BYTES: usize = 4 * 1024 * 1024;

fn supported(ty: &str, typmod: i32) -> bool {
    typmod == -1 && matches!(ty, "text" | "varchar" | "int2" | "int4" | "int8" | "numeric" | "bool" | "bytea")
}

fn field(value: Option<&str>, ty: &str) -> Result<String> {
    if !supported(ty, -1) { bail!("COPY 不支持类型 {ty}"); }
    let Some(value) = value else { return Ok("\\N".into()); };
    if value.len() > VALUE_BYTES { bail!("COPY 单值超过字节上限"); }
    let baseline = crate::sync::value_sql(Some(value), &crate::models::DbKind::of("postgresql"), ty, ty)?;
    let converted;
    let value = match ty {
        "bool" => baseline.as_str(),
        "bytea" => { converted = format!("\\x{}", &value[2..]); converted.as_str() },
        "int2" => { value.trim().parse::<i16>().context("COPY int2 无效")?; value },
        "int4" => { value.trim().parse::<i32>().context("COPY int4 无效")?; value },
        "int8" => { value.trim().parse::<i64>().context("COPY int8 无效")?; value },
        "numeric" => { value.trim().parse::<sqlx::types::BigDecimal>().context("COPY numeric 无效")?; value },
        _ => value,
    };
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"), '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"), '\r' => out.push_str("\\r"),
            '\0' => bail!("COPY 不支持空字节"), _ => out.push(ch),
        }
    }
    Ok(out)
}

fn encode_row(mapper: &ImportMapper, types: &[String], row: &[Option<String>]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for (n, ((index, _), ty)) in mapper.pairs.iter().zip(types).enumerate() {
        let value = row.get(*index).ok_or_else(|| anyhow!("COPY 源行缺少映射列"))?;
        let encoded = field(value.as_deref(), ty)?;
        let added = encoded.len() + 1;
        if out.len().checked_add(added).map_or(true, |size| size > ROW_BYTES) { bail!("COPY 编码行超过字节上限"); }
        if n != 0 { out.push(b'\t'); }
        out.extend_from_slice(encoded.as_bytes());
    }
    out.push(b'\n');
    Ok(out)
}

fn protocol_error(error: sqlx::Error, phase: &str) -> anyhow::Error {
    // SQLx begin/finish drain ReadyForQuery before returning an ordinary server
    // error. Do not label a received constraint/permission error as an unknown COMMIT.
    let acknowledged = matches!(&error, sqlx::Error::Database(_));
    let message = if acknowledged { format!("COPY {phase} 收到服务端错误回执") }
        else { format!("TRANSFER_EFFECTS_UNKNOWN: COPY {phase} 连接或协议未确认；禁止重放") };
    anyhow::Error::from(error).context(message)
}

async fn capability(conn: &mut PgConnection, mapper: &ImportMapper) -> Result<(String, Vec<String>)> {
    // COPY/DML 所需 ROW EXCLUSIVE 锁已能与 ALTER/DROP、CREATE TRIGGER/INDEX
    // 的目录写锁冲突；不要使用 ACCESS EXCLUSIVE 阻塞普通业务 SELECT。锁先于目录核验。
    crate::sql_history::query(&format!("LOCK TABLE {} IN ROW EXCLUSIVE MODE", mapper.full)).execute(&mut *conn).await?;
    let settings: (String, String, String) = sqlx::query_as("SELECT version(), current_setting('standard_conforming_strings'), current_setting('client_encoding')").fetch_one(&mut *conn).await?;
    if !settings.0.starts_with("PostgreSQL ") || settings.1 != "on" || settings.2 != "UTF8" { bail!("COPY 要求原生 PostgreSQL、标准字符串与 UTF8 会话"); }
    let target = sqlx::query("SELECT c.oid::bigint AS oid, n.nspname, c.relname, c.relkind::text AS kind, c.relrowsecurity, c.relforcerowsecurity, c.relispartition, EXISTS(SELECT 1 FROM pg_catalog.pg_am am WHERE am.oid=c.relam AND am.amname='heap' AND am.amtype='t') AS heap FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE c.oid=pg_catalog.to_regclass($1)")
        .bind(&mapper.full).fetch_one(&mut *conn).await?;
    let oid: i64 = target.try_get("oid")?;
    if target.try_get::<String,_>("kind")? != "r" || target.try_get::<bool,_>("relrowsecurity")? || target.try_get::<bool,_>("relforcerowsecurity")? || target.try_get::<bool,_>("relispartition")? || !target.try_get::<bool,_>("heap")? { bail!("COPY 仅支持无 RLS 的普通独立 heap 表"); }
    let unsafe_objects: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_trigger WHERE tgrelid=$1::bigint::oid) OR EXISTS(SELECT 1 FROM pg_catalog.pg_rewrite WHERE ev_class=$1::bigint::oid) OR EXISTS(SELECT 1 FROM pg_catalog.pg_constraint WHERE (conrelid=$1::bigint::oid OR confrelid=$1::bigint::oid) AND contype NOT IN ('n')) OR EXISTS(SELECT 1 FROM pg_catalog.pg_index WHERE indrelid=$1::bigint::oid) OR EXISTS(SELECT 1 FROM pg_catalog.pg_inherits WHERE inhrelid=$1::bigint::oid OR inhparent=$1::bigint::oid)")
        .bind(oid).fetch_one(&mut *conn).await?;
    if unsafe_objects { bail!("COPY 不支持触发器、规则、索引、FK/CHECK/排除约束或继承"); }
    let columns = sqlx::query("SELECT a.attname, t.typname, n.nspname, t.typtype::text AS typtype, a.atttypmod, a.attgenerated::text AS generated, a.attidentity::text AS identity, a.atthasdef FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_type t ON t.oid=a.atttypid JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum")
        .bind(oid).fetch_all(&mut *conn).await?;
    if columns.len() != mapper.cols.len() { bail!("COPY 必须显式映射全部目标列"); }
    let mut types = Vec::new();
    for name in &mapper.cols {
        let column = columns.iter().find(|c| c.get::<String,_>("attname") == *name).ok_or_else(||anyhow!("COPY 目标映射发生变化"))?;
        let ty: String = column.try_get("typname")?;
        if column.try_get::<String,_>("nspname")? != "pg_catalog" || column.try_get::<String,_>("typtype")? != "b" || !supported(&ty, column.try_get("atttypmod")?) || !column.try_get::<String,_>("generated")?.is_empty() || !column.try_get::<String,_>("identity")?.is_empty() || column.try_get::<bool,_>("atthasdef")? { bail!("COPY 列 {name} 的类型、修饰或默认值未证明等价"); }
        types.push(ty);
    }
    let full = crate::db::qualified(&mapper.kind, &target.try_get::<String,_>("nspname")?, &target.try_get::<String,_>("relname")?);
    Ok((full, types))
}

pub(super) async fn execute(pool: &PgPool, prefix: &[(String, bool)], feeder: &mut RowFeeder, mapper: &ImportMapper, limits: &TransferLimits, task: Option<&TaskControl>) -> Result<u64> {
    let mut connection = pool.acquire().await?;
    let mut tx = connection.begin().await?;
    let result: Result<u64> = async {
        if let Some(t) = task { t.check()?; }
        let (full, types) = capability(&mut tx, mapper).await?;
        for (sql, _) in prefix { crate::sql_history::query(sql).execute(&mut *tx).await?; }
        let statement = format!("COPY {full} ({}) FROM STDIN WITH (FORMAT text)", mapper.quoted);
        let mut copy = tx.copy_in_raw(&statement).await.map_err(|error|protocol_error(error,"启动"))?;
        let stream: Result<()> = async {
            if !copy.is_textual() || copy.num_columns() != types.len() { bail!("COPY 协议列格式不匹配"); }
            let mut chunk = Vec::new();
            loop {
                if let Some(t) = task { t.check()?; }
                let Some(rows) = feeder.next_batch(limits)? else { break };
                for row in rows {
                    if let Some(t) = task { t.check()?; }
                    let encoded = encode_row(mapper, &types, &row)?;
                    if chunk.len() + encoded.len() > CHUNK_BYTES {
                        copy.send(chunk.as_slice()).await.context("TRANSFER_EFFECTS_UNKNOWN: COPY send 回执未确认")?;
                        chunk.clear();
                    }
                    chunk.extend_from_slice(&encoded);
                }
                if !chunk.is_empty() {
                    copy.send(chunk.as_slice()).await.context("TRANSFER_EFFECTS_UNKNOWN: COPY send 回执未确认")?;
                    chunk.clear();
                }
                tokio::task::yield_now().await;
            }
            if let Some(t) = task { t.check()?; }
            Ok(())
        }.await;
        if let Err(error) = stream {
            return match copy.abort("NavKit COPY stopped").await {
                Ok(()) => Err(error),
                Err(abort) => Err(error.context(format!("TRANSFER_EFFECTS_UNKNOWN: COPY abort/排空未确认：{abort}"))),
            };
        }
        let count = copy.finish().await.map_err(|error|protocol_error(error,"finish/排空"))?;
        if let Some(t) = task { t.check()?; t.update("write", count, 0, "COPY 完成，等待单事务提交"); }
        Ok(count)
    }.await;
    let receipt = match result {
        Ok(count) => tx.commit().await.context("TRANSFER_EFFECTS_UNKNOWN: COPY COMMIT 回执未确认；禁止重放").map(|()|count),
        Err(error) => match tx.rollback().await {
            Ok(()) => Err(error.context("COPY 导入事务回滚已确认")),
            Err(rollback) => Err(error.context(format!("TRANSFER_EFFECTS_UNKNOWN: COPY ROLLBACK 未确认：{rollback}"))),
        },
    };
    // 未确认排空的物理连接不能返回连接池供后续写入复用。
    if receipt.as_ref().err().is_some_and(|e|format!("{e:#}").contains("TRANSFER_EFFECTS_UNKNOWN")) {
        connection.close_on_drop();
        if let Err(close) = connection.close().await {
            return Err(receipt.unwrap_err().context(format!("TRANSFER_EFFECTS_UNKNOWN: COPY 专用连接关闭回执未确认：{close}")));
        }
    }
    receipt
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mapper(schema: &str, table: &str, cols: &[&str]) -> ImportMapper {
        let kind = crate::models::DbKind::of("postgresql");
        ImportMapper { pairs: cols.iter().enumerate().map(|(i,c)|(i,c.to_string())).collect(),
            full: crate::db::qualified(&kind,schema,table), quoted: cols.iter().map(|c|crate::db::quote_ident(&kind,c)).collect::<Vec<_>>().join(", "),
            kind, strategy: "error".into(), conflict_cols: vec![], cols: cols.iter().map(|c|c.to_string()).collect(),
            types: vec![], schema: schema.into(), table: table.into() }
    }
    #[test]
    fn codec_exact_values() {
        assert_eq!(field(None,"text").unwrap(), "\\N");
        assert_eq!(field(Some(""),"text").unwrap(), "");
        assert_eq!(field(Some("\\N\t\n\r\\中文"),"text").unwrap(), "\\\\N\\t\\n\\r\\\\中文");
        assert_eq!(field(Some("9223372036854775807"),"int8").unwrap(), "9223372036854775807");
        assert_eq!(field(Some("12345678901234567890.000000001"),"numeric").unwrap(), "12345678901234567890.000000001");
        assert_eq!(field(Some("1"),"bool").unwrap(), "TRUE");
        assert_eq!(field(Some("0x00FF"),"bytea").unwrap(), "\\\\x00FF");
    }
    #[test]
    fn codec_rejects_unsafe_values() {
        for (value,ty) in [("32768","int2"),("2147483648","int4"),("9223372036854775808","int8"),("wat","bool"),("0x0","bytea"),("NaN","numeric"),("\0","text")] { assert!(field(Some(value),ty).is_err()); }
        assert!(field(Some(&"x".repeat(VALUE_BYTES+1)),"text").is_err());
    }
    #[test]
    fn protocol_disconnect_is_unknown_and_not_a_replay_candidate() {
        let error=protocol_error(sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionReset,"test disconnect")),"finish");
        assert!(format!("{error:#}").contains("TRANSFER_EFFECTS_UNKNOWN"));
    }
    #[test]
    fn capability_type_whitelist() {
        for ty in ["text","varchar","int2","int4","int8","numeric","bool","bytea"] { assert!(supported(ty,-1)); assert!(!supported(ty,10)); }
        for ty in ["jsonb","timestamp","float8","my_domain","_text","bpchar"] { assert!(!supported(ty,-1)); }
    }
    #[test]
    fn encoded_row_budget_and_missing_column() {
        let m = mapper("public","t", &["a","b","c"]);
        let types = vec!["text".into();3];
        let row = vec![Some("\\".repeat(VALUE_BYTES));3];
        assert!(encode_row(&m,&types,&row).is_err());
        assert!(encode_row(&m,&types,&[]).is_err());
        let m = mapper("public","t", &["a"]);
        assert_eq!(encode_row(&m,&["text".into()],&[Some("\\".repeat(VALUE_BYTES))]).unwrap().len(),VALUE_BYTES*2+1);
    }
    #[tokio::test]
    async fn unsupported_request_rejects_before_file_or_target_writes() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query("CREATE TABLE t(v TEXT); INSERT INTO t VALUES('preserved')").execute(&pool).await.unwrap();
        let options: crate::models::ImportOptions = serde_json::from_value(serde_json::json!({"connection_id":"test","schema":"main","table":"t","format":"csv","file":"does-not-exist","truncate":true})).unwrap();
        let task: super::super::ImportTaskOptions = serde_json::from_value(serde_json::json!({"connection_id":"test","schema":"main","table":"t","format":"csv","file":"does-not-exist","task_id":"copy-default"})).unwrap();
        assert_eq!(task.write_strategy,super::super::ImportWriteStrategy::Row);
        let error = super::super::run_import_data_with_strategy(&crate::db::Db::Sqlite(pool.clone()),&crate::models::DbKind::of("sqlite"),&options,&[],None,true,super::super::ImportWriteStrategy::PgCopy).await.unwrap_err();
        assert!(error.to_string().contains("COPY"));
        assert_eq!(sqlx::query_scalar::<_,String>("SELECT v FROM t").fetch_one(&pool).await.unwrap(),"preserved");
    }
    #[tokio::test]
    #[ignore = "仅在明确提供 NAVKIT_PERF_PG_URL 时运行；创建 UUID 隔离模式"]
    async fn live_copy_row_ab_roundtrip_capability_and_rollback() -> Result<()> {
        let url = std::env::var("NAVKIT_PERF_PG_URL").context("必须明确提供 NAVKIT_PERF_PG_URL")?;
        let pool = PgPool::connect(&url).await?;
        let schema = format!("navkit_copy_{}",uuid::Uuid::new_v4().simple());
        let root = std::env::temp_dir().join(format!("navkit-copy-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&root)?;
        let qschema = crate::db::quote_ident(&crate::models::DbKind::of("postgresql"),&schema);
        sqlx::query(&format!("CREATE SCHEMA {qschema}")).execute(&pool).await?;
        let result: Result<()> = async {
            for table in ["copied","legacy"] {
                sqlx::query(&format!("CREATE TABLE {qschema}.{table}(v text, i int8, n numeric, b bool, raw bytea)")).execute(&pool).await?;
            }
            let values = [Some("\\N\t\n\r中文\\".to_string()),Some("9223372036854775807".into()),Some("12345678901234567890.000000001".into()),Some("true".into()),Some("0x00FF".into())];
            let rows = vec![values.to_vec();1000];
            let file = root.join("input.json");
            let objects = rows.iter().map(|row|serde_json::json!({"v":row[0],"i":row[1],"n":row[2],"b":row[3],"raw":row[4]})).collect::<Vec<_>>();
            std::fs::write(&file,serde_json::to_vec(&objects)?)?;
            let cols = ["v","i","n","b","raw"];
            let mut m = mapper(&schema,"copied",&cols);
            m.types = ["text","bigint","numeric","boolean","bytea"].iter().map(|s|s.to_string()).collect();
            let db = crate::db::Db::Postgres(pool.clone());
            let mut options: crate::models::ImportOptions = serde_json::from_value(serde_json::json!({"connection_id":"isolated-test","schema":schema,"table":"copied","file":file.to_str().unwrap(),"format":"json","encoding":"utf8","on_conflict":"error"}))?;
            let make_feeder = || -> Result<RowFeeder> {
                let mut stream = crate::transfer_stream::JsonStream::open(file.to_str().unwrap(),"utf8",16*1024*1024)?;
                stream.names = cols.iter().map(|s|s.to_string()).collect(); Ok(RowFeeder::Json(stream))
            };
            let start = std::time::Instant::now();
            assert_eq!(super::super::run_import_data_with_strategy(&db,&m.kind,&options,&[],None,true,super::super::ImportWriteStrategy::PgCopy).await?,1000);
            let copy_elapsed = start.elapsed();
            options.table = "legacy".into();
            let start = std::time::Instant::now();
            assert_eq!(super::super::run_import_data_with_strategy(&db,&m.kind,&options,&[],None,true,super::super::ImportWriteStrategy::Row).await?,1000);
            eprintln!("隔离 1000 行：COPY={copy_elapsed:?}, row={:?}；单次观测不构成性能承诺",start.elapsed());
            let diff: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM ((SELECT * FROM {qschema}.copied EXCEPT ALL SELECT * FROM {qschema}.legacy) UNION ALL (SELECT * FROM {qschema}.legacy EXCEPT ALL SELECT * FROM {qschema}.copied)) d")).fetch_one(&pool).await?;
            assert_eq!(diff,0);
            // 检查失败必须发生在 DELETE 前；每个变体在隔离模式内。
            for definition in ["v text CHECK(v <> '')","v text DEFAULT 'x'","v varchar(12)","v jsonb","v int PRIMARY KEY","v text[]","v int GENERATED ALWAYS AS IDENTITY"] {
                sqlx::query(&format!("CREATE TABLE {qschema}.blocked({definition})")).execute(&pool).await?;
                let blocked = mapper(&schema,"blocked",&["v"]);
                let mut tx = pool.begin().await?;
                assert!(capability(&mut tx,&blocked).await.is_err()); tx.rollback().await?;
                sqlx::query(&format!("DROP TABLE {qschema}.blocked")).execute(&pool).await?;
            }
            for setup in [
                format!("CREATE VIEW {qschema}.blocked AS SELECT v FROM {qschema}.copied"),
                format!("CREATE TABLE {qschema}.blocked(v text) PARTITION BY LIST(v)"),
                format!("CREATE TABLE {qschema}.blocked(v text); ALTER TABLE {qschema}.blocked ENABLE ROW LEVEL SECURITY"),
                format!("CREATE TABLE {qschema}.blocked(v text); CREATE RULE blocked_rule AS ON INSERT TO {qschema}.blocked DO ALSO NOTIFY navkit_copy_test"),
            ] {
                sqlx::raw_sql(&setup).execute(&pool).await?;
                let blocked = mapper(&schema,"blocked",&["v"]);
                let mut tx = pool.begin().await?;
                assert!(capability(&mut tx,&blocked).await.is_err()); tx.rollback().await?;
                sqlx::query(&format!("DROP {} {qschema}.blocked",if setup.starts_with("CREATE VIEW") {"VIEW"} else {"TABLE"})).execute(&pool).await?;
            }
            // 编码错误发生在 DELETE 后，必须 abort + 整个事务回滚。
            let copied = mapper(&schema,"copied",&cols);
            std::fs::write(&file,b"[{\"v\":\"x\",\"i\":\"overflow\",\"n\":\"1\",\"b\":\"true\",\"raw\":\"0x00\"}]")?;
            assert!(execute(&pool,&[(format!("DELETE FROM {qschema}.copied"),false)],&mut make_feeder()?,&copied,&TransferLimits::default(),None).await.is_err());
            assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT count(*) FROM {qschema}.copied")).fetch_one(&pool).await?,1000);
            sqlx::query(&format!("ALTER TABLE {qschema}.copied ALTER COLUMN i SET NOT NULL")).execute(&pool).await?;
            std::fs::write(&file,b"[{\"v\":\"\",\"i\":null,\"n\":\"1\",\"b\":\"false\",\"raw\":\"0x\"}]")?;
            let error = execute(&pool,&[(format!("DELETE FROM {qschema}.copied"),false)],&mut make_feeder()?,&copied,&TransferLimits::default(),None).await.unwrap_err();
            assert!(format!("{error:#}").contains("事务回滚已确认"));
            assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT count(*) FROM {qschema}.copied")).fetch_one(&pool).await?,1000);
            let task = TaskControl::begin(&format!("copy-cancel-{}",uuid::Uuid::new_v4()))?;
            crate::transfer::cancel_task(&task.id)?;
            let cancelled = execute(&pool,&[(format!("DELETE FROM {qschema}.copied"),false)],&mut make_feeder()?,&copied,&TransferLimits::default(),Some(&task)).await;
            assert!(cancelled.is_err()); task.finish(&cancelled);
            assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT count(*) FROM {qschema}.copied")).fetch_one(&pool).await?,1000);
            Ok(())
        }.await;
        let cleanup = sqlx::query(&format!("DROP SCHEMA {qschema} CASCADE")).execute(&pool).await;
        let local_cleanup = std::fs::remove_dir_all(&root);
        pool.close().await;
        cleanup?; local_cleanup?; result
    }
}
