//! Explicit task-wide native logical backup sessions. Never a protocol-family fallback.
//! The existing DDL/value/sink path writes a private stage; only verified session cleanup publishes it.
use super::*;
use sqlx::Executor;
use std::sync::atomic::AtomicUsize;

const PG_BEGIN: &str = "BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY";
const MYSQL_UNLOCK: &str = "UNLOCK TABLES";
const MAX_OBJECTS: usize = 4096;
const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Mode { Compat, PgSnapshot, MysqlLocked }

pub(super) fn mode(kind: &DbKind, policy: Option<&str>) -> Result<Mode> {
    match policy {
        None | Some("compat") => Ok(Mode::Compat),
        Some("pg_snapshot") if kind.id() == "postgresql" => Ok(Mode::PgSnapshot),
        Some("mysql_locked") if kind.id() == "mysql" => Ok(Mode::MysqlLocked),
        Some("pg_snapshot" | "mysql_locked") => bail!("BACKUP_CONSISTENCY_UNSUPPORTED: 显式一致性模式与实际原生数据源不匹配，不能按兼容协议降级"),
        Some(_) => bail!("BACKUP_CONSISTENCY_INVALID: 未知备份一致性模式"),
    }
}

pub(super) struct Scope {
    mode: Mode,
    nonce: String,
    schema: String,
    tables: Vec<String>,
    readers: AtomicUsize,
}
impl Scope {
    fn new(mode: Mode, schema: &str, tables: Vec<String>) -> Self {
        Self { mode, nonce: uuid::Uuid::new_v4().to_string(), schema: schema.into(), tables, readers: AtomicUsize::new(0) }
    }
    fn check_reader(&self, schema: &str, table: &str) -> Result<()> {
        if self.mode != Mode::MysqlLocked || self.schema != schema || !self.tables.iter().any(|name| name == table) {
            bail!("BACKUP_SCOPE_LOST: MySQL reader 不属于已锁定的备份对象，未发布文件");
        }
        self.readers.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn verify_readers(&self, include_data: bool) -> Result<()> {
        let expected = if include_data { self.tables.len() } else { 0 };
        if self.readers.load(Ordering::SeqCst) != expected {
            bail!("BACKUP_SCOPE_LOST: MySQL 共享 reader 数量与已锁定表不一致，不能发布混合会话备份");
        }
        Ok(())
    }
}

tokio::task_local! { static BACKUP_SCOPE: Arc<Scope>; }

pub(super) fn consistency_label() -> Option<&'static str> {
    BACKUP_SCOPE.try_with(|scope| match scope.mode {
        Mode::PgSnapshot => "PostgreSQL 同一物理只读 REPEATABLE READ 事务；所选普通表的受支持 DDL/数据，不含序列、视图、例程、权限及物理备份/PITR",
        Mode::MysqlLocked => "原生 MySQL 同一物理会话一次 LOCK TABLES READ；所选 InnoDB 普通表 DDL/数据；MySQL 可能隐式锁关联表并升级 WRITE（阻塞业务读写），已知 trigger/FK 依赖拒绝；不含视图、例程、触发器、权限及物理备份/PITR",
        Mode::Compat => "兼容逻辑转储（不承诺跨表同一快照）",
    }).ok()
}

fn startup_plan(mode: Mode, lock: &str, nonce: &str) -> Result<Vec<String>> {
    // Only a freshly generated UUID can enter session SQL, never user input.
    let nonce = uuid::Uuid::parse_str(nonce)?;
    match mode {
        Mode::PgSnapshot => Ok(vec![PG_BEGIN.into(),format!("SET LOCAL navkit.backup_token = '{nonce}'"),lock.into()]),
        Mode::MysqlLocked => Ok(vec![lock.into(),format!("SET @navkit_backup_token = '{nonce}'")]),
        Mode::Compat => bail!("兼容模式不能执行显式原生会话计划"),
    }
}

fn verify_nonce(actual: Option<&str>, expected: &str) -> Result<()> {
    if actual != Some(expected) { bail!("BACKUP_SESSION_LOST: 会话 nonce 缺失或改变，事务结束/连接替换，备份未发布"); }
    Ok(())
}

/// Called by the existing MySQL reader BEFORE any per-table transaction command.
/// A lost task-local scope cannot go unnoticed: the final reader count and nonce are mandatory.
pub(super) async fn mysql_shared_reader(connection: &mut sqlx::MySqlConnection, schema: &str, table: &str) -> Result<bool> {
    let scope = match BACKUP_SCOPE.try_with(Arc::clone) { Ok(scope) => scope, Err(_) => return Ok(false) };
    if scope.mode != Mode::MysqlLocked { bail!("BACKUP_SCOPE_LOST: 错误备份协议中的 MySQL reader"); }
    let nonce: Option<String> = crate::sql_history::query_scalar("SELECT CAST(@navkit_backup_token AS CHAR)").fetch_one(&mut *connection).await?;
    verify_nonce(nonce.as_deref(), &scope.nonce)?;
    scope.check_reader(schema, table)?;
    Ok(true)
}

fn pg_pool(source: &sqlx::PgPool) -> sqlx::PgPool {
    // Preserve source after_connect security/session verification (TLS, SSH owner, read-only defaults).
    source.options().clone().max_connections(1).min_connections(0)
        .max_lifetime(None).idle_timeout(None).test_before_acquire(false)
        .connect_lazy_with((*source.connect_options()).clone())
}
fn mysql_pool(source: &sqlx::MySqlPool) -> sqlx::MySqlPool {
    // Preserve TLS/SSH checks and NO_BACKSLASH_ESCAPES; do not silently change the existing typed SQL dialect.
    source.options().clone().max_connections(1).min_connections(0)
        .max_lifetime(None).idle_timeout(None).test_before_acquire(false)
        .connect_lazy_with((*source.connect_options()).clone())
}

#[derive(Debug, PartialEq, Eq)]
struct Object { name: String, identity: String, ty: String, engine: String }

fn selected(objects: Vec<Object>, names: &[String]) -> Result<Vec<Object>> {
    for name in names {
        if !objects.iter().any(|object| &object.name == name) { bail!("BACKUP_OBJECT_MISSING: 所选对象不存在或不可见：{name}"); }
    }
    let objects: Vec<_> = objects.into_iter().filter(|object| names.is_empty() || names.contains(&object.name)).collect();
    if objects.is_empty() { bail!("BACKUP_OBJECTS_EMPTY: 显式模式需要至少一张普通表"); }
    Ok(objects)
}
fn catalog_budget(objects: &[Object]) -> Result<()> {
    let bytes: usize = objects.iter().map(|o| o.name.len() + o.identity.len() + o.ty.len() + o.engine.len()).sum();
    if objects.len() > MAX_OBJECTS || bytes > MAX_CATALOG_BYTES { bail!("BACKUP_CATALOG_LIMIT: 目录超过4096对象/16MiB预算"); }
    Ok(())
}

async fn pg_catalog(pool: &sqlx::PgPool, schema: &str, names: &[String]) -> Result<Vec<Object>> {
    let mut objects = Vec::new();
    let mut stream = crate::sql_history::query("SELECT c.relname, c.oid::text, c.relkind::text, c.relpersistence::text FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p','v','m','f','S') ORDER BY c.relname")
        .bind(schema).fetch(pool);
    while let Some(row) = stream.try_next().await? {
        objects.push(Object { name: row.try_get(0)?, identity: row.try_get(1)?, ty: row.try_get(2)?, engine: row.try_get(3)? });
        catalog_budget(&objects)?;
    }
    drop(stream);
    // Sequences are outside the table-selection contract, not silently backed up.
    selected(objects.into_iter().filter(|o| o.ty != "S" || names.contains(&o.name)).collect(), names)
}

fn pg_lock_sql(kind: &DbKind, schema: &str, objects: &[Object]) -> Result<String> {
    if objects.is_empty() { bail!("BACKUP_OBJECTS_EMPTY: 不能锁定空表集合"); }
    for object in objects {
        if object.ty != "r" || object.engine != "p" { bail!("BACKUP_OBJECT_UNSUPPORTED: pg_snapshot 只支持持久普通表，不支持视图/分区/外部表：{}",object.name); }
    }
    // Acquire ALL structural locks before the first snapshot-bearing SELECT. Otherwise an
    // ALTER between an RR catalog read and LOCK can retain the same OID but change the live row layout.
    Ok(format!("LOCK TABLE {} IN ACCESS SHARE MODE NOWAIT",objects.iter().map(|o|format!("ONLY {}",qualified(kind,schema,&o.name))).collect::<Vec<_>>().join(", ")))
}

async fn pg_lock_and_prove(pool: &sqlx::PgPool, kind: &DbKind, schema: &str, objects: &[Object], structure: bool) -> Result<()> {
    if structure {
        let version: i32 = crate::sql_history::query_scalar("SELECT current_setting('server_version_num')::int").fetch_one(pool).await?;
        if version < 140000 { bail!("BACKUP_DDL_UNSUPPORTED: 显式PG结构模式需要14+可核验列存储/压缩目录；旧版本请用仅数据或兼容模式"); }
    }
    for object in objects {
        if object.ty != "r" || object.engine != "p" { bail!("BACKUP_OBJECT_UNSUPPORTED: pg_snapshot 只支持持久普通表；视图/分区/外部表依赖尚不能证明：{}", object.name); }
        // READ ONLY permits ACCESS SHARE; it blocks DROP/ALTER/TRUNCATE without blocking ordinary DML.
        // Verify the ACTUAL granted lock/OID, not just another repeatable snapshot catalog read.
        let locked: bool = crate::sql_history::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='relation' AND relation=$1::oid AND mode='AccessShareLock' AND granted) AND to_regclass($2)::oid=$1::oid")
            .bind(sqlx::postgres::types::Oid(object.identity.parse::<u32>()?)).bind(qualified(kind, schema, &object.name)).fetch_one(pool).await?;
        if !locked { bail!("BACKUP_DDL_CHANGED: 实际锁定对象OID与快照目录不一致，未发布"); }
        let safe: bool = crate::sql_history::query_scalar("SELECT NOT c.relrowsecurity AND NOT c.relforcerowsecurity AND NOT c.relispartition AND NOT EXISTS (SELECT 1 FROM pg_inherits WHERE inhrelid=c.oid OR inhparent=c.oid) AND NOT EXISTS (SELECT 1 FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid JOIN pg_namespace n ON n.oid=t.typnamespace WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped AND (a.attgenerated<>'' OR a.attidentity<>'' OR n.nspname<>'pg_catalog' OR t.typtype<>'b' OR t.typelem<>0)) FROM pg_class c WHERE c.oid=$1::oid")
            .bind(sqlx::postgres::types::Oid(object.identity.parse::<u32>()?)).fetch_one(pool).await?;
        if !safe { bail!("BACKUP_DEPENDENCY_UNSUPPORTED: RLS/继承/自定义类型/数组或生成/identity列不能通过现有写出完整还原：{}；请使用兼容模式", object.name); }
        if structure {
            // Existing PG DDL reconstructs only columns and PK. Do not promise complete constraints by erasing them.
            // Fail closed for objects it cannot reproduce exactly; data-only mode can still snapshot constrained ordinary tables.
            let reproducible: bool = crate::sql_history::query_scalar("SELECT c.reloptions IS NULL AND c.reltablespace=0 AND c.relam=(SELECT oid FROM pg_am WHERE amname='heap' AND amtype='t') AND obj_description(c.oid) IS NULL AND NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conrelid=c.oid) AND NOT EXISTS (SELECT 1 FROM pg_index WHERE indrelid=c.oid) AND NOT EXISTS (SELECT 1 FROM pg_trigger WHERE tgrelid=c.oid AND NOT tgisinternal) AND NOT EXISTS (SELECT 1 FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid WHERE a.attrelid=c.oid AND a.attnum>0 AND (a.attisdropped OR a.attidentity<>'' OR a.attgenerated<>'' OR a.atthasdef OR a.attcollation<>t.typcollation OR a.attoptions IS NOT NULL OR a.attfdwoptions IS NOT NULL OR a.attstorage<>t.typstorage OR a.attcompression<>'' OR col_description(c.oid,a.attnum) IS NOT NULL)) FROM pg_class c WHERE c.oid=$1::oid")
                .bind(sqlx::postgres::types::Oid(object.identity.parse::<u32>()?)).fetch_one(pool).await?;
            if !reproducible { bail!("BACKUP_DDL_UNSUPPORTED: 现有PG结构写出不能完整保留该表的约束/索引/生成列/default/注释/列存储压缩等；显式模式拒绝丢失结构：{}（可选仅数据或兼容模式）", object.name); }
        }
    }
    Ok(())
}

fn mysql_version(version: &str, comment: &str) -> Result<()> {
    let parts: Vec<_> = version.split('.').collect();
    let supported = parts.len() >= 3 && parts[0] == "8" && matches!(parts[1], "0" | "4")
        && parts[2].parse::<u32>().is_ok()
        && (comment.starts_with("MySQL Community Server") || comment.starts_with("MySQL Enterprise Server"));
    if !supported { bail!("BACKUP_SERVER_UNSUPPORTED: mysql_locked 仅支持已识别的原生MySQL 8.0/8.4；MariaDB/TiDB/未知发行版不按协议能力等同"); }
    Ok(())
}

async fn mysql_catalog(pool: &sqlx::MySqlPool, schema: &str, names: &[String]) -> Result<Vec<Object>> {
    let mut objects = Vec::new();
    let mut stream = crate::sql_history::query("SELECT TABLE_NAME, TABLE_TYPE, ENGINE FROM information_schema.tables WHERE table_schema=? ORDER BY TABLE_NAME").bind(schema).fetch(pool);
    while let Some(row) = stream.try_next().await? {
        objects.push(Object { name: crate::schema::mysql_get(&row,"TABLE_NAME").ok_or_else(||anyhow!("表目录名字不可读取"))?, identity: String::new(),
            ty: crate::schema::mysql_get(&row,"TABLE_TYPE").ok_or_else(||anyhow!("表目录类型不可读取"))?, engine: crate::schema::mysql_get(&row,"ENGINE").unwrap_or_default() });
        catalog_budget(&objects)?;
    }
    drop(stream);
    selected(objects, names)
}

async fn mysql_dependency_gate(pool: &sqlx::MySqlPool, schema: &str, objects: &[Object]) -> Result<()> {
    // Preflight reduces implicit trigger/FK lock expansion; repeat after LOCK
    // because concurrent DDL can change the dependencies between discovery and lock.
    // Catalog visibility is not proof of zero hidden dependencies: UI explicitly
    // warns that MySQL can lock related tables, including WRITE upgrades.
    for object in objects {
        let count: i64 = crate::sql_history::query_scalar(
            "SELECT (SELECT COUNT(*) FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA=? AND EVENT_OBJECT_TABLE=?) + (SELECT COUNT(*) FROM information_schema.KEY_COLUMN_USAGE WHERE REFERENCED_TABLE_NAME IS NOT NULL AND ((TABLE_SCHEMA=? AND TABLE_NAME=?) OR (REFERENCED_TABLE_SCHEMA=? AND REFERENCED_TABLE_NAME=?)))")
            .bind(schema).bind(&object.name).bind(schema).bind(&object.name).bind(schema).bind(&object.name).fetch_one(pool).await?;
        if count != 0 { bail!("BACKUP_LOCK_DEPENDENCY_UNSUPPORTED: 表 {} 含 trigger/FK 关联，LOCK TABLES 可能扩大锁范围或升级 WRITE；该显式模式拒绝，请选择兼容模式",object.name); }
    }
    Ok(())
}

fn mysql_lock_sql(kind: &DbKind, schema: &str, objects: &[Object]) -> Result<String> {
    if objects.is_empty() { bail!("BACKUP_OBJECTS_EMPTY: 不能锁定空表集合"); }
    for object in objects {
        if object.ty != "BASE TABLE" || !object.engine.eq_ignore_ascii_case("InnoDB") {
            bail!("BACKUP_OBJECT_UNSUPPORTED: mysql_locked 只支持普通InnoDB表；视图/其他引擎依赖不能证明：{}", object.name);
        }
    }
    Ok(format!("LOCK TABLES {}", objects.iter().map(|o| format!("{} READ", qualified(kind,schema,&o.name))).collect::<Vec<_>>().join(", ")))
}

async fn mysql_definitions(db: &Db, kind: &DbKind, schema: &str, objects: &[Object]) -> Result<Vec<String>> {
    let mut definitions = Vec::new(); let mut bytes = 0usize;
    for object in objects {
        let ddl = super::table_ddl(db,kind,schema,&object.name,false).await?.filter(|ddl| !ddl.trim().is_empty()).ok_or_else(||anyhow!("BACKUP_DDL_UNREADABLE: 不能读取真实DDL"))?;
        bytes = bytes.checked_add(ddl.len()).ok_or_else(||anyhow!("备份DDL预算溢出"))?;
        if bytes > MAX_CATALOG_BYTES { bail!("BACKUP_CATALOG_LIMIT: DDL超过16MiB预算"); }
        definitions.push(ddl);
    }
    Ok(definitions)
}

/// Only this gate can publish an explicit backup. Cleanup errors remain fatal even with a good dump.
fn finish_stage(stage: StagedOutput, result: Result<DumpDatabaseReport>, cleanup: Result<()>) -> Result<DumpDatabaseReport> {
    match (result, cleanup) {
        (Ok(report), Ok(())) => { stage.publish(||Ok(()))?; Ok(report) },
        (Err(error), Ok(())) => Err(error),
        (result, Err(cleanup)) => Err(result.err().unwrap_or_else(||anyhow!("备份源清理失败"))
            .context(format!("TRANSFER_EFFECTS_UNKNOWN: 源清理未确认，备份未发布：{cleanup:#}"))),
    }
}

pub(super) async fn run(db: &Db, kind: &DbKind, schema: &str, tables: &[String], routines: &[String],
    structure: bool, data: bool, path: &str, comment: &str, skip: bool, mode: Mode) -> Result<DumpDatabaseReport> {
    if schema.is_empty() || !routines.is_empty() { bail!("BACKUP_DEPENDENCY_UNSUPPORTED: 显式模式需要明确schema且不包含例程；依赖尚不能证明，使用兼容模式"); }
    if !structure && !data { bail!("备份至少包含结构或数据"); }
    ensure_writable(path)?;
    let stage = StagedOutput::new(path)?;
    match (db, mode) {
        (Db::Postgres(source), Mode::PgSnapshot) => {
            let pool = pg_pool(source);
            let session = Db::Postgres(pool.clone());
            let mut expected_nonce = None;
            let result: Result<DumpDatabaseReport> = async {
                let version: String = crate::sql_history::query_scalar("SELECT version()").fetch_one(&pool).await?;
                if !version.starts_with("PostgreSQL ") { bail!("BACKUP_SERVER_UNSUPPORTED: 未确认原生PostgreSQL能力"); }
                // Discovery is only a name/OID lock plan, not the metadata used to generate the dump.
                let discovered = pg_catalog(&pool,schema,tables).await?;
                let lock = pg_lock_sql(kind,schema,&discovered)?;
                let scope = Arc::new(Scope::new(mode,schema,Vec::new()));
                expected_nonce = Some(scope.nonce.clone());
                {
                    let mut connection = pool.acquire().await?;
                    for statement in startup_plan(mode,&lock,&scope.nonce)? {
                        connection.execute(statement.as_str()).await?;
                    }
                }
                // This is the first snapshot-bearing SELECT, after every selected table is locked.
                let objects = pg_catalog(&pool,schema,tables).await?;
                if objects != discovered { bail!("BACKUP_DDL_CHANGED: 发现目录与实际快照对象OID/清单不一致，备份未发布"); }
                pg_lock_and_prove(&pool,kind,schema,&objects,structure).await?;
                BACKUP_SCOPE.scope(scope.clone(), async {
                    let report = super::dump_database_to_stage(&session,kind,schema,tables,routines,structure,data,comment,skip,&stage).await?;
                    let nonce: Option<String> = crate::sql_history::query_scalar("SELECT current_setting('navkit.backup_token', true)").fetch_one(&pool).await?;
                    verify_nonce(nonce.as_deref(),&scope.nonce)?;
                    if pg_catalog(&pool,schema,tables).await? != objects { bail!("BACKUP_DDL_CHANGED: 目录清单改变，备份未发布"); }
                    pg_lock_and_prove(&pool,kind,schema,&objects,structure).await?;
                    Ok(report)
                }).await
            }.await;
            let mut final_validation: Result<()> = Ok(());
            let cleanup: Result<()> = async {
                let mut connection = pool.acquire().await?;
                // Pool return performs an async ping; it can retire a socket even after a good final query.
                // Recheck on the very socket we will roll back AND close, before permitting publication.
                final_validation = async {
                    if let Some(expected) = expected_nonce.as_deref() {
                        let actual: Option<String> = crate::sql_history::query_scalar("SELECT current_setting('navkit.backup_token', true)").fetch_one(&mut *connection).await?;
                        verify_nonce(actual.as_deref(),expected)?;
                    }
                    Ok::<_,anyhow::Error>(())
                }.await;
                let rollback = if expected_nonce.is_some() { connection.execute("ROLLBACK").await.map(|_|()) } else { Ok(()) };
                let close = connection.close().await;
                rollback?; close?; Ok(())
            }.await;
            pool.close().await;
            let result = result.and_then(|report|final_validation.map(|_|report));
            finish_stage(stage,result,cleanup)
        },
        (Db::Mysql(source), Mode::MysqlLocked) => {
            let pool = mysql_pool(source);
            let session = Db::Mysql(pool.clone());
            let mut expected_nonce = None;
            let result: Result<DumpDatabaseReport> = async {
                let row = crate::sql_history::query("SELECT @@version AS version, @@version_comment AS comment").fetch_one(&pool).await?;
                mysql_version(&row.try_get::<String,_>("version")?,&row.try_get::<String,_>("comment")?)?;
                let objects = mysql_catalog(&pool,schema,tables).await?;
                mysql_dependency_gate(&pool,schema,&objects).await?;
                let lock = mysql_lock_sql(kind,schema,&objects)?;
                let scope = Arc::new(Scope::new(mode,schema,objects.iter().map(|o|o.name.clone()).collect()));
                expected_nonce = Some(scope.nonce.clone());
                // One lock statement, not one transaction/snapshot per table, and never global FLUSH.
                {
                    // Pin the socket across LOCK and nonce setup: a replacement in between must not gain a valid token without locks.
                    let mut connection = pool.acquire().await?;
                    for statement in startup_plan(mode,&lock,&scope.nonce)? {
                        connection.execute(statement.as_str()).await?;
                    }
                }
                if mysql_catalog(&pool,schema,tables).await? != objects { bail!("BACKUP_DDL_CHANGED: 加锁前后目录清单改变"); }
                mysql_dependency_gate(&pool,schema,&objects).await?;
                if data {
                    for object in &objects {
                        let generated: i64 = crate::sql_history::query_scalar("SELECT COUNT(*) FROM information_schema.columns WHERE table_schema=? AND table_name=? AND generation_expression<>''")
                            .bind(schema).bind(&object.name).fetch_one(&pool).await?;
                        if generated != 0 { bail!("BACKUP_DDL_UNSUPPORTED: 现有SQL数据写出不能向MySQL生成列还原显式值：{}；可选仅结构或兼容模式",object.name); }
                    }
                }
                let definitions = mysql_definitions(&session,kind,schema,&objects).await?;
                BACKUP_SCOPE.scope(scope.clone(), async {
                    let report = super::dump_database_to_stage(&session,kind,schema,tables,routines,structure,data,comment,skip,&stage).await?;
                    let nonce: Option<String> = crate::sql_history::query_scalar("SELECT CAST(@navkit_backup_token AS CHAR)").fetch_one(&pool).await?;
                    verify_nonce(nonce.as_deref(),&scope.nonce)?;
                    scope.verify_readers(data)?;
                    if mysql_catalog(&pool,schema,tables).await? != objects || mysql_definitions(&session,kind,schema,&objects).await? != definitions {
                        bail!("BACKUP_DDL_CHANGED: 锁定会话的目录/真实DDL改变，备份未发布");
                    }
                    Ok(report)
                }).await
            }.await;
            let mut final_validation: Result<()> = Ok(());
            let cleanup: Result<()> = async {
                let mut connection = pool.acquire().await?;
                final_validation = async {
                    if let Some(expected) = expected_nonce.as_deref() {
                        let actual: Option<String> = crate::sql_history::query_scalar("SELECT CAST(@navkit_backup_token AS CHAR)").fetch_one(&mut *connection).await?;
                        verify_nonce(actual.as_deref(),expected)?;
                    }
                    Ok::<_,anyhow::Error>(())
                }.await;
                let unlock = if expected_nonce.is_some() { connection.execute(MYSQL_UNLOCK).await.map(|_|()) } else { Ok(()) };
                let close = connection.close().await;
                unlock?; close?; Ok(())
            }.await;
            pool.close().await;
            let result = result.and_then(|report|final_validation.map(|_|report));
            finish_stage(stage,result,cleanup)
        },
        _ => bail!("BACKUP_CONSISTENCY_UNSUPPORTED: 当前实际通道不是指定的原生驱动；没有发布备份"),
    }
}

#[cfg(test)]
#[path = "transfer_backup_native_tests.rs"]
mod tests;
