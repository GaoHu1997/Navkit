use super::*;
use crate::{models::{ColumnInfo,IndexInfo,ForeignKeyInfo,TableSchema}, design::TableOptionsDesign, table_objects::{ConstraintInfo,ConstraintSet,TriggerInfo}, table_workflows::{TableDesignSnapshot,TableCapabilities,FeatureCapability}};
fn ident(k:&DbKind,s:&str)->String{db::quote_ident(k,s)}
fn columns(rows:&[Record])->Result<Vec<ColumnInfo>>{rows.iter().map(|r|Ok(ColumnInfo{name:get(r,"name")?.into(),data_type:get(r,"type")?.into(),nullable:get(r,"nullable")?=="1",default:opt(r,"default")?,primary_key:false,auto_increment:get(r,"auto")?=="1",comment:opt(r,"comment")?,length:None,precision:None,scale:None})).collect()}
fn json_names(r:&Record,k:&str)->Result<Vec<String>>{Ok(serde_json::from_str(get(r,k)?)?)}
/// “关系不存在”是能力探测的唯一可继续信号：权限/语法错误必须原样上抛。
fn missing_relation(e:&anyhow::Error)->bool{
    e.downcast_ref::<sqlx::Error>().and_then(|e|e.as_database_error()).is_some_and(|d|matches!(d.code().as_deref(),Some("42S02")|Some("42P01")))
}
/// “列不存在”(42703) 表示该方言没有这个 catalog 列；与权限/语法错误区分开。
fn undefined_column(e:&anyhow::Error)->bool{
    e.downcast_ref::<sqlx::Error>().and_then(|e|e.as_database_error()).is_some_and(|d|d.code().as_deref()==Some("42703"))
}
/// MySQL 协议族方言能力。MySQL 8 有 `@@server_uuid` 与 `information_schema.INNODB_TABLES`；
/// MariaDB 用 `INNODB_SYS_TABLES`；TiDB 用 `TABLES.TIDB_TABLE_ID` 且没有触发器。
/// `server_uuid` 是兼容变量，不能据此把 TiDB 当成原生 MySQL。
struct MysqlEvidence{tidb:bool,server_uuid:bool}
impl MysqlEvidence{
    async fn probe(s:&mut Session)->Result<Self>{
        let server=s.scalar("SELECT CONCAT(COALESCE(@@version,''),' ',COALESCE(@@version_comment,''))").await?;
        Ok(Self{tidb:db::mysql_read_only_unsupported(&server,""),server_uuid:!s.rows("SHOW VARIABLES LIKE 'server_uuid'").await?.is_empty()})
    }
    fn requires_innodb_identity(&self)->bool{!self.tidb&&self.server_uuid}
}
/// TiDB 的 INNODB_* 只是兼容目录，不能用其空行或占位 ID 证明表身份。
/// 识别 TiDB 后只查官方 TABLES.TIDB_TABLE_ID；其余方言按真实 InnoDB 目录逐个尝试。
fn mysql_table_id_sqls(e:&MysqlEvidence,ns:&str,tn:&str,raw:&str)->Vec<String>{
    if e.tidb{return vec![format!("SELECT CAST(TIDB_TABLE_ID AS CHAR) AS id FROM information_schema.TABLES WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={tn}")];}
    vec![
        format!("SELECT CAST(TABLE_ID AS CHAR) AS id FROM information_schema.INNODB_TABLES WHERE NAME={}",db::quote_literal(raw)),
        format!("SELECT CAST(ID AS CHAR) AS id FROM information_schema.INNODB_SYS_TABLES WHERE NAME={}",db::quote_literal(raw)),
    ]
}
fn mysql_table_id(rows:&[Record])->Result<Option<String>>{
    if rows.is_empty(){return Ok(None);}
    ensure!(rows.len()==1,"TABLE_MYSQL_PHYSICAL_IDENTITY_AMBIGUOUS");
    // 目录可能给出 NULL（例如 TiDB 的视图/TiFlash 副本），必须显式拒绝而不是回落到伪造身份。
    let id=opt(&rows[0],"id")?.filter(|v|!v.trim().is_empty()).ok_or_else(||anyhow!("TABLE_MYSQL_PHYSICAL_IDENTITY_INVALID: 目录返回空表 ID"))?;
    ensure!(id.parse::<u64>().ok().is_some_and(|n|n>0),"TABLE_MYSQL_PHYSICAL_IDENTITY_INVALID");
    Ok(Some(id))
}
/// openGauss/GaussDB 才有 `pg_database.datcompatibility`；普通 PostgreSQL 查询会报 42703，按 None 处理。
const PG_COMPATIBILITY_SQL:&str="SELECT datcompatibility::text AS mode FROM pg_catalog.pg_database WHERE datname=current_database()";
/// PostgreSQL 协议族方言能力。PG 12+ 有 `attgenerated`，10+ 有 `attidentity`/`pg_sequence`/`relispartition`，
/// 11+ 有 `indnkeyatts`，9.5+ 有 `relrowsecurity`；openGauss/GaussDB/KWDB 等按实际列降级，
/// 不按版本文本猜能力，避免同一内核的不同发行版分支误判。
#[derive(Clone,Copy)]
pub(crate) struct PgEvidence{attgenerated:bool,attidentity:bool,attcompression:bool,indnkeyatts:bool,sequences:bool,relrowsecurity:bool,relforcerowsecurity:bool,relreplident:bool,relispartition:bool,mysql_compat:bool}
impl PgEvidence{
    pub(crate) async fn probe(s:&mut Session)->Result<Self>{
        let columns=s.rows("SELECT c.relname AS relation,a.attname AS column FROM pg_catalog.pg_attribute a JOIN pg_catalog.pg_class c ON c.oid=a.attrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='pg_catalog' AND c.relname IN ('pg_class','pg_attribute','pg_index') AND a.attnum>0 AND NOT a.attisdropped").await?;
        let has=|relation:&str,column:&str|columns.iter().any(|r|get(r,"relation").ok()==Some(relation)&&get(r,"column").ok()==Some(column));
        let sequences=match s.rows("SELECT 1 AS probe FROM pg_catalog.pg_sequence LIMIT 0").await{Ok(_)=>true,Err(e) if missing_relation(&e)=>false,Err(e)=>return Err(e)};
        let compat=match s.rows(PG_COMPATIBILITY_SQL).await{Ok(rows)=>rows.first().and_then(|r|opt(r,"mode").ok().flatten()).unwrap_or_default(),Err(e) if undefined_column(&e)=>String::new(),Err(e)=>return Err(e)};
        Ok(Self{attgenerated:has("pg_attribute","attgenerated"),attidentity:has("pg_attribute","attidentity"),attcompression:has("pg_attribute","attcompression"),indnkeyatts:has("pg_index","indnkeyatts"),sequences,relrowsecurity:has("pg_class","relrowsecurity"),relforcerowsecurity:has("pg_class","relforcerowsecurity"),relreplident:has("pg_class","relreplident"),relispartition:has("pg_class","relispartition"),mysql_compat:db::pg_mysql_compat_mode(&compat)})
    }
}
/// 索引元数据：现代层（unnest/ORDINALITY + indnkeyatts 可用时按 key 列截断）。
fn pg_index_sql(e:&PgEvidence,oid:&str)->String{
    let index_keys=if e.indnkeyatts{"WHERE u.n<=i.indnkeyatts"}else{""};
    let index_complex=if e.indnkeyatts{"(i.indexprs IS NOT NULL OR i.indpred IS NOT NULL OR i.indnatts<>i.indnkeyatts)::int::text"}else{"(i.indexprs IS NOT NULL OR i.indpred IS NOT NULL)::int::text"};
    format!("SELECT ci.relname AS name,i.indisprimary::int::text AS primary,i.indisunique::int::text AS unique,i.indisvalid::int::text AS valid,i.indisready::int::text AS ready,am.amname AS method,pg_get_indexdef(i.indexrelid) AS definition,obj_description(i.indexrelid,'pg_class') AS comment,(SELECT json_agg(a.attname ORDER BY u.n)::text FROM unnest(i.indkey) WITH ORDINALITY u(id,n) JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=u.id {index_keys}) AS columns,{index_complex} AS complex FROM pg_index i JOIN pg_class ci ON ci.oid=i.indexrelid JOIN pg_am am ON am.oid=ci.relam WHERE i.indrelid={oid}::oid ORDER BY ci.relname")
}
/// 索引元数据兼容层：不依赖 `unnest ... WITH ORDINALITY` 与 `indnkeyatts`，只用 string_to_array + generate_series
/// （PG 9.x 系、openGauss/GaussDB 老内核都能跑）；负值系统属性（如 tableoid）按 >=0 过滤。
fn pg_index_compat_sql(oid:&str)->String{
    format!("SELECT ci.relname AS name,i.indisprimary::int::text AS primary,i.indisunique::int::text AS unique,i.indisvalid::int::text AS valid,i.indisready::int::text AS ready,am.amname AS method,pg_get_indexdef(i.indexrelid) AS definition,obj_description(i.indexrelid,'pg_class') AS comment,(SELECT json_agg(a.attname ORDER BY pos.n)::text FROM generate_series(1,array_length(string_to_array(i.indkey::text,' '),1)) AS pos(n) LEFT JOIN pg_attribute a ON a.attrelid=i.indrelid AND a.attnum=(string_to_array(i.indkey::text,' '))[pos.n]::int2 AND a.attnum>0 WHERE (string_to_array(i.indkey::text,' '))[pos.n]::int2>=0) AS columns,(i.indexprs IS NOT NULL OR i.indpred IS NOT NULL)::int::text AS complex FROM pg_index i JOIN pg_class ci ON ci.oid=i.indexrelid JOIN pg_am am ON am.oid=ci.relam WHERE i.indrelid={oid}::oid ORDER BY ci.relname")
}
/// 列元数据最后兜底层：标准 information_schema，任何兼容 PG 的内核都应提供。
fn pg_columns_information_schema(ns:&str,tn:&str)->String{
    format!("SELECT c.column_name AS name,CASE WHEN c.data_type='USER-DEFINED' THEN c.udt_name ELSE c.data_type END AS type,CASE WHEN c.is_nullable='YES' THEN '1' ELSE '0' END AS nullable,c.column_default AS \"default\",'0' AS auto,NULL::text AS comment,''::text AS identity,''::text AS generated,NULL::text AS expression,NULL::text AS collation,NULL::text AS compression,c.ordinal_position::text AS ordinal,'b'::text AS typekind,c.udt_name AS typename,NULL::text AS enum_labels FROM information_schema.columns c WHERE c.table_schema={ns} AND c.table_name={tn} ORDER BY c.ordinal_position")
}
pub(super) async fn snapshot(s:&mut Session,b:&Binding,table:&str,readonly:bool)->Result<Stored>{
    let k=s.kind();let ns=db::quote_literal(&b.namespace);let tn=db::quote_literal(table);let full=db::qualified(&k,&b.namespace,table);
    let mut limitations:Vec<String>=Vec::new();
    // 方言层面的写能力缺口（目前只有 TiDB 的 noop 服务器锁）；读取不受影响，但保存必须在发送 DDL 前拒绝。
    let mut dialect_write_reason:Option<String>=None;
    let (meta,ddl,mut cols,mut indexes,foreign_keys,objects,options)=if k.is_mysql_family(){
        let evidence=MysqlEvidence::probe(s).await?;
        if evidence.tidb{dialect_write_reason=Some("TiDB 的 GET_LOCK/RELEASE_LOCK/LOCK TABLES 只是 noop 兼容功能，无法证明并发安全；表结构保存明确拒绝，读取与复制预览仍可用".into());}
        // MySQL 8 用 @@server_uuid；MariaDB/TiDB/OceanBase 缺少该变量时退到 @@server_id，仍缺则留空，
        // 身份由 host/port/db/sql_mode 等其余列共同构成，不能因方言缺一个兼容变量就拒绝读取。
        let server=if evidence.server_uuid{"@@server_uuid"}else if !s.rows("SHOW VARIABLES LIKE 'server_id'").await?.is_empty(){"CAST(@@server_id AS CHAR)"}else{"''"};
        let identity=s.rows(&format!("SELECT {server} AS server, @@hostname AS host, @@port AS port, DATABASE() AS db, @@sql_mode AS mode, @@lower_case_table_names AS names, @@session.character_set_client AS charset_client, @@session.collation_connection AS collation_connection")).await?;
        ensure!(identity.len()==1&&get(&identity[0],"db")?==b.namespace,"TABLE_NAMESPACE_CHANGED");
        // SHOW GRANTS 本身不会把无 TRIGGER 权限误报为空触发器目录；TiDB 没有触发器，
        // 其 TRIGGERS 目录恒为空是方言事实，不要求 TRIGGER 权限才能发布完整定义。
        let grants=s.rows("SHOW GRANTS").await?;
        let schema_scope=format!("{}.*",ident(&k,&b.namespace));let table_scope=full.clone();
        let visible=grants.iter().flat_map(|r|r.values().flatten()).any(|g|{let upper=g.to_ascii_uppercase();let privilege=upper.split(" ON ").next().unwrap_or("");(privilege.contains("ALL PRIVILEGES")||privilege.split([',',' ']).any(|p|p=="TRIGGER"))&&(g.contains(" ON *.* TO ")||g.contains(&format!(" ON {schema_scope} TO "))||g.contains(&format!(" ON {table_scope} TO ")))});
        if evidence.tidb{limitations.push("TiDB 不支持触发器；触发器目录按方言事实为空".into());}else{ensure!(visible,"TABLE_TRIGGER_VISIBILITY_UNPROVEN: 需要可证明的直接 TRIGGER/ALL 权限；角色授权目录未认证，不能发布空触发器");}
        let table_rows=s.rows(&format!("SELECT TABLE_TYPE AS kind, ENGINE AS engine, TABLE_COLLATION AS collation, ROW_FORMAT AS row_format, CREATE_OPTIONS AS extra, TABLE_COMMENT AS comment, CAST(CREATE_TIME AS CHAR) AS created FROM information_schema.TABLES WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={tn}")).await?;
        ensure!(table_rows.len()==1&&get(&table_rows[0],"kind")?=="BASE TABLE","TABLE_BASE_TABLE_REQUIRED");let mut tr=table_rows[0].clone();tr.insert("session_sql_mode".into(),Some(get(&identity[0],"mode")?.into()));
        mysql_session_environment(&mut tr,&identity[0])?;
        let create=s.rows(&format!("SHOW CREATE TABLE {full}")).await?;ensure!(create.len()==1,"TABLE_DEFINITION_UNKNOWN");let ddl=get(&create[0],"Create Table")?.to_string();
        let cr=s.rows(&format!("SELECT COLUMN_NAME AS name, COLUMN_TYPE AS type, IF(IS_NULLABLE='YES',1,0) AS nullable, COLUMN_DEFAULT AS `default`, IF(EXTRA LIKE '%auto_increment%',1,0) AS auto, COLUMN_COMMENT AS comment, EXTRA AS extra, GENERATION_EXPRESSION AS `generated`, COLLATION_NAME AS collation, ORDINAL_POSITION AS ordinal FROM information_schema.COLUMNS WHERE TABLE_SCHEMA={ns} AND TABLE_NAME={tn} ORDER BY ORDINAL_POSITION")).await?;
        ensure!(!cr.is_empty(),"TABLE_COLUMNS_UNKNOWN");let mut cols=columns(&cr)?;
        // COLUMN_DEFAULT 是值而非 SQL；从 SHOW CREATE 的 DEFAULT token 提取真实表达式。
        let (_,parts,_)=crate::table_copy::table_parts(&ddl)?;
        for c in &mut cols{let part=parts.iter().find(|p|crate::table_objects::tokens(p).ok().and_then(|ts|ts.first().and_then(|t|crate::table_objects::identifier(t).ok())).as_deref()==Some(c.name.as_str())).ok_or_else(||anyhow!("TABLE_COLUMN_DEFINITION_UNKNOWN"))?;c.default=default_expression(part)?;}
        let mut ix=ordered_mysql_indexes(s.rows(&format!("SHOW INDEX FROM {full}")).await?)?;for row in &mut ix{row.remove("Cardinality");}let mut indexes=BTreeMap::<String,IndexInfo>::new();
        for r in &ix{let name=get(r,"Key_name")?.to_string();let i=indexes.entry(name.clone()).or_insert(IndexInfo{name:name.clone(),columns:vec![],unique:get(r,"Non_unique")?=="0",primary:name=="PRIMARY",method:Some(get(r,"Index_type")?.into())});if let Some(col)=opt(r,"Column_name")?{i.columns.push(col);}else{ensure!(r.get("Expression").is_some_and(|v|v.is_some()),"TABLE_INDEX_EXPRESSION_UNKNOWN");}}
        let fkrows=s.rows(&format!("SELECT k.CONSTRAINT_NAME AS name,k.COLUMN_NAME AS col,k.REFERENCED_TABLE_NAME AS ref_table,k.REFERENCED_TABLE_SCHEMA AS ref_schema,k.REFERENCED_COLUMN_NAME AS ref_col,r.UPDATE_RULE AS on_update,r.DELETE_RULE AS on_delete FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA=k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME=k.CONSTRAINT_NAME AND r.TABLE_NAME=k.TABLE_NAME WHERE k.TABLE_SCHEMA={ns} AND k.TABLE_NAME={tn} AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.CONSTRAINT_NAME,k.ORDINAL_POSITION")).await?;
        let mut fks=BTreeMap::<String,ForeignKeyInfo>::new();for r in &fkrows{let name=get(r,"name")?.into();let f=fks.entry(name).or_insert(ForeignKeyInfo{name:get(r,"name")?.into(),columns:vec![],ref_table:get(r,"ref_table")?.into(),ref_schema:Some(get(r,"ref_schema")?.into()),ref_columns:vec![],on_update:Some(get(r,"on_update")?.into()),on_delete:Some(get(r,"on_delete")?.into())});f.columns.push(get(r,"col")?.into());f.ref_columns.push(get(r,"ref_col")?.into());}
        let checks=crate::table_objects::extract_checks(&ddl)?;
        let triggerrows=s.rows(&format!("SELECT TRIGGER_NAME AS name,ACTION_TIMING AS timing,EVENT_MANIPULATION AS event,ACTION_STATEMENT AS body,ACTION_ORIENTATION AS orientation,ACTION_ORDER AS ordering,SQL_MODE AS mode,DEFINER AS definer,CHARACTER_SET_CLIENT AS charset,COLLATION_CONNECTION AS collation FROM information_schema.TRIGGERS WHERE EVENT_OBJECT_SCHEMA={ns} AND EVENT_OBJECT_TABLE={tn} ORDER BY ACTION_TIMING,EVENT_MANIPULATION,ACTION_ORDER,TRIGGER_NAME")).await?;
        let mut raw_triggers=Vec::new();let mut triggers=Vec::new();for r in triggerrows{let mut r=r;let name=get(&r,"name")?.to_string();let raw=s.rows(&format!("SHOW CREATE TRIGGER {}",db::qualified(&k,&b.namespace,&name))).await?;ensure!(raw.len()==1,"TABLE_TRIGGER_DEFINITION_UNKNOWN");let statement=get(&raw[0],"SQL Original Statement")?.to_string();r.insert("definition".into(),Some(statement.clone()));triggers.push(TriggerInfo{name,timing:Some(get(&r,"timing")?.into()),event:Some(get(&r,"event")?.into()),statement});raw_triggers.push(r);}
        let options=TableOptionsDesign{engine:Some(get(&tr,"engine")?.into()),charset:Some(get(&tr,"collation")?.split('_').next().ok_or_else(||anyhow!("TABLE_CHARSET_UNKNOWN"))?.into()),collation:Some(get(&tr,"collation")?.into()),row_format:Some(get(&tr,"row_format")?.into()),..Default::default()};
        let objects=ConstraintSet{checks,triggers,completeness:"complete".into(),limitations:vec![],table_definition:Some(ddl.clone())};
        // 物理表 ID 是“表未被 DROP+CREATE 重建”的证据；方言没有该证据时降级为目录身份并保留限制。
        let physical=crate::object_catalog::revision(&identity);
        let mut table_id=None;
        for sql in mysql_table_id_sqls(&evidence,&ns,&tn,&format!("{}/{}",b.namespace,table)){
            match s.rows(&sql).await{
                Ok(rows)=>match mysql_table_id(&rows)?{Some(id)=>{table_id=Some(id);break;},None=>continue},
                Err(e) if missing_relation(&e)||undefined_column(&e)=>continue,
                Err(e)=>return Err(e),
            }
        }
        let id=if let Some(table_id)=table_id{crate::object_catalog::revision(&(&physical,table_id))}else{
            // TiDB 一定提供 TABLES.TIDB_TABLE_ID；读不到说明目录/权限异常，不能伪装成已认证身份。
            ensure!(!evidence.tidb,"TABLE_MYSQL_PHYSICAL_IDENTITY_UNPROVEN: TiDB 未返回 TABLES.TIDB_TABLE_ID，无法证明物理表身份");
            // 原生 MySQL 有 @@server_uuid 时仍要求真实 InnoDB TABLE_ID：非 InnoDB/分区表不能靠降级蒙混。
            ensure!(!evidence.requires_innodb_identity(),"TABLE_MYSQL_PHYSICAL_IDENTITY_UNPROVEN: 需要真实 InnoDB TABLE_ID；分区/特殊文件名/非 InnoDB 未认证");
            limitations.push("当前 MySQL 方言未提供可证明的物理表 ID；DROP+CREATE 重建只能靠定义与目录版本检测".into());crate::object_catalog::revision(&(&physical,b.namespace.clone(),table.to_string()))};
        (NativeMetadata{identity:id,physical,columns:cr,indexes:ix,constraints:fkrows,triggers:raw_triggers,table:tr,sequences:vec![]},ddl,cols,indexes.into_values().collect(),fks.into_values().collect(),objects,options)
    }else{
        // 同一物理会话内复用探测结果（保存表设计会对每步 DDL 反复 snapshot）。
        let evidence=match s.pg_evidence(){Some(e)=>e,None=>{let e=PgEvidence::probe(s).await?;s.set_pg_evidence(e);e}};
        // openGauss/GaussDB 的 B(MySQL) 兼容模式：标识符用反引号、`||` 可能被当逻辑 OR，
        // 按 PG 语义拼出的 SQL 在这里是错的；表设计读写明确拒绝，避免生成错误或危险 SQL。
        ensure!(!evidence.mysql_compat,"TABLE_PG_MYSQL_COMPAT_MODE_UNSUPPORTED: 该数据库是 openGauss/GaussDB 的 B(MySQL) 兼容模式，标识符引号与拼接语义与 PostgreSQL 不同，表设计未认证；请改用 A/PG 兼容库，或使用查询/导出功能");
        // 缺失的列用常量占位，保证同一份读取在 PG 9.x/openGauss 系仍能完成；能力缺口记录到 limitations。
        let (rls,force_rls,partition,replica)=(
            if evidence.relrowsecurity{"c.relrowsecurity::text"}else{"'false'::text"},
            if evidence.relforcerowsecurity{"c.relforcerowsecurity::text"}else{"'false'::text"},
            if evidence.relispartition{"c.relispartition::text"}else{"'false'::text"},
            if evidence.relreplident{"c.relreplident::text"}else{"''::text"},
        );
        if !(evidence.relrowsecurity&&evidence.relforcerowsecurity&&evidence.relispartition&&evidence.relreplident){limitations.push("当前 PG 方言缺少部分 pg_class 列（RLS/分区/replica identity），清空与复制的相应保护按未知处理".into());}
        let identity=s.rows("SELECT current_database() AS db,(SELECT oid::text FROM pg_database WHERE datname=current_database()) AS dboid,inet_server_addr()::text AS addr,inet_server_port()::text AS port,pg_postmaster_start_time()::text AS started").await?;ensure!(identity.len()==1,"TABLE_PHYSICAL_IDENTITY_UNKNOWN");
        let tbl=s.rows(&format!("SELECT c.oid::text AS oid,c.relfilenode::text AS filenode,c.relkind::text AS kind,c.relpersistence::text AS persistence,{rls} AS rls,{force_rls} AS force_rls,{partition} AS partition,{replica} AS replica,(SELECT amname FROM pg_am WHERE oid=c.relam) AS access_method,c.reloptions::text AS options,c.reltablespace::text AS tablespace,(SELECT count(*)::text FROM pg_inherits WHERE inhrelid=c.oid OR inhparent=c.oid) AS inherits,(SELECT count(*)::text FROM pg_rewrite WHERE ev_class=c.oid AND rulename<>'_RETURN') AS rules,obj_description(c.oid,'pg_class') AS comment,pg_get_userbyid(c.relowner) AS owner FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname={ns} AND c.relname={tn}")).await?;
        ensure!(tbl.len()==1&&["r","p"].contains(&get(&tbl[0],"kind")?),"TABLE_BASE_TABLE_REQUIRED");let tr=tbl[0].clone();let oid=get(&tr,"oid")?;
        let default_expr=if evidence.attgenerated{"CASE WHEN a.attgenerated='' THEN pg_get_expr(ad.adbin,ad.adrelid) END"}else{"pg_get_expr(ad.adbin,ad.adrelid)"};
        let auto=if evidence.attidentity{"(a.attidentity<>'' OR COALESCE(pg_get_expr(ad.adbin,ad.adrelid) LIKE 'nextval(%',false))::int::text"}else{"(COALESCE(pg_get_expr(ad.adbin,ad.adrelid) LIKE 'nextval(%',false))::int::text"};
        let identity_column=if evidence.attidentity{"a.attidentity::text"}else{"''::text"};
        let generated_column=if evidence.attgenerated{"a.attgenerated::text"}else{"''::text"};
        let expression=if evidence.attgenerated{"CASE WHEN a.attgenerated<>'' THEN pg_get_expr(ad.adbin,ad.adrelid) END"}else{"NULL::text"};
        // 不用 to_jsonb(a)->>'attcompression'：openGauss 等 9.x 内核没有 to_jsonb，且列本身也可能不存在。
        let compression=if evidence.attcompression{"a.attcompression::text"}else{"NULL::text"};
        let columns_sql=format!("SELECT a.attname AS name,format_type(a.atttypid,a.atttypmod) AS type,(NOT a.attnotnull)::int::text AS nullable,{default_expr} AS \"default\",{auto} AS auto,col_description(a.attrelid,a.attnum) AS comment,{identity_column} AS identity,{generated_column} AS generated,{expression} AS expression,a.attcollation::text AS collation,a.attstorage::text AS storage,{compression} AS compression,a.attnum::text AS ordinal,t.typtype::text AS typekind,t.typname AS typename,(SELECT json_agg(e.enumlabel ORDER BY e.enumsortorder)::text FROM pg_enum e WHERE e.enumtypid=t.oid) AS enum_labels FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid LEFT JOIN pg_attrdef ad ON ad.adrelid=a.attrelid AND ad.adnum=a.attnum WHERE a.attrelid={oid}::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum");
        // 最后一层兜底：pg_catalog 被意外拒绝/未预料内核时退到标准 information_schema（只丢 identity/generated 细节）。
        let cr=match s.rows(&columns_sql).await{
            Ok(rows)=>rows,
            Err(primary)=>match s.rows(&pg_columns_information_schema(&ns,&tn)).await{
                Ok(rows)=>{limitations.push(format!("列元数据主查询失败，已降级 information_schema（identity/generated 细节不可用）：{primary}"));rows}
                Err(_)=>return Err(primary),
            },
        };
        ensure!(!cr.is_empty(),"TABLE_COLUMNS_UNKNOWN");let cols=columns(&cr)?;
        let constraints=s.rows(&format!("SELECT conname AS name,contype::text AS kind,pg_get_constraintdef(oid) AS definition,condeferrable::text AS deferred,condeferred::text AS initially,convalidated::text AS validated,connoinherit::text AS noinherit,(SELECT json_agg(a.attname ORDER BY u.n)::text FROM unnest(conkey) WITH ORDINALITY u(id,n) JOIN pg_attribute a ON a.attrelid=conrelid AND a.attnum=u.id) AS columns,(SELECT relname FROM pg_class WHERE oid=confrelid) AS ref_table,(SELECT n.nspname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE c.oid=confrelid) AS ref_schema,(SELECT json_agg(a.attname ORDER BY u.n)::text FROM unnest(confkey) WITH ORDINALITY u(id,n) JOIN pg_attribute a ON a.attrelid=confrelid AND a.attnum=u.id) AS ref_columns,confupdtype::text AS on_update,confdeltype::text AS on_delete FROM pg_constraint WHERE conrelid={oid}::oid ORDER BY conname")).await?;
        // 现代层用 unnest(...) WITH ORDINALITY + indnkeyatts；老内核（PG 9.x 系、部分 openGauss/GaussDB）
        // 缺 ORDINALITY 或 indnkeyatts 时降级为 string_to_array + generate_series 的兼容查询。
        let ix=match s.rows(&pg_index_sql(&evidence,oid)).await{
            Ok(rows)=>rows,
            Err(primary)=>match s.rows(&pg_index_compat_sql(oid)).await{
                Ok(rows)=>{limitations.push(format!("索引元数据主查询失败，已降级兼容查询：{primary}"));rows}
                Err(_)=>return Err(primary),
            },
        };
        let mut indexes=Vec::new();for r in &ix{indexes.push(IndexInfo{name:get(r,"name")?.into(),columns:opt(r,"columns")?.map(|v|serde_json::from_str(&v)).transpose()?.unwrap_or_default(),unique:get(r,"unique")?=="1",primary:get(r,"primary")?=="1",method:Some(get(r,"method")?.into())});}
        let mut checks=Vec::new();let mut fks=Vec::new();for r in &constraints{match get(r,"kind")?{"c"=>checks.push(ConstraintInfo{name:get(r,"name")?.into(),definition:get(r,"definition")?.into(),generated_name:false}),"f"=>fks.push(ForeignKeyInfo{name:get(r,"name")?.into(),columns:json_names(r,"columns")?,ref_table:get(r,"ref_table")?.into(),ref_schema:Some(get(r,"ref_schema")?.into()),ref_columns:json_names(r,"ref_columns")?,on_update:Some(action(get(r,"on_update")?)?.into()),on_delete:Some(action(get(r,"on_delete")?)?.into())}),_=>{}}}
        let triggerrows=s.rows(&format!("SELECT t.tgname AS name,pg_get_triggerdef(t.oid) AS definition,t.tgenabled::text AS enabled,t.tgfoid::text AS function,t.tgconstraint::text AS constraint,pg_get_functiondef(t.tgfoid) AS function_definition,obj_description(t.oid,'pg_trigger') AS comment FROM pg_trigger t WHERE t.tgrelid={oid}::oid AND NOT t.tgisinternal ORDER BY t.tgname")).await?;
        let triggers=triggerrows.iter().map(|r|Ok(TriggerInfo{name:get(r,"name")?.into(),timing:None,event:None,statement:get(r,"definition")?.into()})).collect::<Result<Vec<_>>>()?;
        let seq=if evidence.sequences{s.rows(&format!("SELECT a.attname AS column,n.nspname AS schema,c.relname AS name,format_type(q.seqtypid,NULL) AS type,q.seqstart::text AS start,q.seqincrement::text AS increment,q.seqmax::text AS max,q.seqmin::text AS min,q.seqcache::text AS cache,q.seqcycle::text AS cycle,{identity_column} AS identity FROM pg_depend d JOIN pg_class c ON c.oid=d.objid AND c.relkind='S' JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_sequence q ON q.seqrelid=c.oid JOIN pg_attribute a ON a.attrelid=d.refobjid AND a.attnum=d.refobjsubid WHERE d.refobjid={oid}::oid AND d.deptype IN ('a','i') ORDER BY a.attnum")).await?}else{limitations.push("当前 PG 方言没有 pg_sequence，序列元数据未读取；复制表不会重建序列".into());Vec::new()};
        let options_text=opt(&tr,"options")?.unwrap_or_default();let fillfactor=options_text.trim_matches(['{','}']).split(',').find_map(|v|v.strip_prefix("fillfactor=")).map(str::parse).transpose()?;
        let options=TableOptionsDesign{fillfactor,..Default::default()};let ddl=crate::object_catalog::revision(&(&cr,&constraints,&ix,&tr,&triggerrows,&seq));
        let objects=ConstraintSet{checks,triggers,completeness:"complete".into(),limitations:vec![],table_definition:Some(serde_json::to_string(&(&cr,&constraints,&ix,&tr,&triggerrows,&seq))?)};
        let physical=crate::object_catalog::revision(&identity);let id=crate::object_catalog::revision(&(&physical,oid,get(&tr,"filenode")?));
        (NativeMetadata{identity:id,physical,columns:cr,indexes:ix,constraints,triggers:triggerrows,table:tr,sequences:seq},ddl,cols,indexes,fks,objects,options)
    };
    indexes.sort_by(|a,b|a.name.cmp(&b.name));for c in &mut cols{c.primary_key=indexes.iter().filter(|i|i.primary).any(|i|i.columns.contains(&c.name));}
    let schema=TableSchema{table:table.into(),database:b.namespace.clone(),columns:cols,indexes,foreign_keys,comment:opt(&meta.table,"comment")?,engine:options.engine.clone(),collation:options.collation.clone(),row_count:None};
    let mut modeled=objects.clone();for c in &mut modeled.checks{let ts=crate::table_objects::tokens(&c.definition)?;if ts.first().is_some_and(|t|t.word("CHECK")){let mut depth=0;let mut end=None;for t in ts.iter().skip(1){if t.symbol("("){depth+=1;}if t.symbol(")"){depth-=1;if depth==0{end=Some(t.end);break;}}}c.definition=c.definition[..end.ok_or_else(||anyhow!("TABLE_CHECK_DEFINITION_UNKNOWN"))?].into();}}
    let mut design=workflow::snapshot_design(&schema,&modeled,&options)?;
    // PG 触发器为调用函数的完整 DDL；MySQL body 使用真实 ACTION_STATEMENT。
    for t in design.triggers.as_mut().unwrap(){if let Some(r)=meta.triggers.iter().find(|r|get(r,"name").ok()==Some(t.name.as_str())){if k.is_mysql_family(){t.statement=get(r,"body")?.into();}else{t.statement=get(r,"definition")?.into();}}}
    let base=crate::object_catalog::revision(&(&ddl,&meta.columns,&meta.indexes,&meta.constraints,&meta.triggers,&meta.table,&meta.sequences));
    let revision=crate::object_catalog::revision(&(&base,&meta.identity,&meta.physical,b));design.base_revision=Some(revision.clone());design.source_fingerprint=Some(b.fingerprint.clone());
    let cap=match (&dialect_write_reason,readonly){
        (Some(reason),_)=>FeatureCapability{supported:false,reason:Some(reason.clone())},
        (None,true)=>FeatureCapability{supported:false,reason:Some("保存连接为只读".into())},
        _=>FeatureCapability{supported:true,reason:None},
    };
    let clear=clear_capability(&k,&meta.table,readonly);
    let clear=if let Some(reason)=&dialect_write_reason{FeatureCapability{supported:false,reason:Some(reason.clone())}}else{clear};
    let capabilities=TableCapabilities{checks:cap.clone(),triggers:cap.clone(),options:cap.clone(),rebuild:cap.clone(),clear,copy_structure:cap.clone(),copy_data:cap,supported_options:if k.is_mysql_family(){vec!["charset","collation","row_format"]}else{vec!["fillfactor"]}.into_iter().map(str::to_string).collect(),limitations:{let mut v=if k.is_mysql_family(){vec!["DDL 隐式提交；失败可能部分保存，保留草稿并报告未验证".to_string(),"直接 TRIGGER 权限必须可证明；角色授权需单独认证".to_string(),"仅认证 InnoDB/TiDB 真实 TABLE_ID；非 InnoDB 及更换引擎在发送 DDL 前拒绝".to_string()]}else{vec!["清空拒绝 RLS/FORCE RLS 或缺失策略元数据，不能用同角色可见行数证明物理清空".to_string(),"复制对 RLS/分区/继承/规则/排除约束及自定义类型逐项拒绝；普通表完整支持".to_string()]};v.extend(limitations);v}};
    let hidden=meta.columns.iter().map(|c|Ok((get(c,"name")?.into(),if opt(c,"generated")?.is_some_and(|v|!v.is_empty()){3}else{0}))).collect::<Result<_>>()?;
    Ok(Stored{public:TableDesignSnapshot{schema,objects,options,design,revision,source_fingerprint:b.fingerprint.clone(),owner_id:b.owner.clone(),completeness:"complete".into(),capabilities},ddl,indexes:vec![],columns:hidden,rowid:None,sequence:None,base,kind:k,native:Some(meta)})
}
fn mysql_session_environment(table:&mut Record,session:&Record)->Result<()>{
    for (source,target) in [("charset_client","session_charset_client"),("collation_connection","session_collation_connection")]{
        let value=get(session,source)?;ensure!(!value.is_empty(),"TABLE_MYSQL_SESSION_ENVIRONMENT_EMPTY: {source}");
        table.insert(target.into(),Some(value.into()));
    }
    Ok(())
}
pub(super) fn require_clear_visibility(kind:&DbKind,table:&Record)->Result<()>{
    if kind.is_pg_family(){
        // 同角色可见行数不能证明物理清空；不发送任何关闭/绕过 RLS 的命令。
        ensure!(get(table,"rls")?=="false"&&get(table,"force_rls")?=="false","TABLE_CLEAR_PG_RLS_UNCERTIFIED: RLS/FORCE RLS 不能通过当前角色的行数证明物理清空");
    }
    Ok(())
}
fn clear_capability(kind:&DbKind,table:&Record,readonly:bool)->FeatureCapability{
    let reason=if readonly{Some("保存连接为只读".into())}else{require_clear_visibility(kind,table).err().map(|e|format!("清空表不可用：{e}"))};
    FeatureCapability{supported:reason.is_none(),reason}
}
fn action(s:&str)->Result<&'static str>{Ok(match s{"a"=>"NO ACTION","r"=>"RESTRICT","c"=>"CASCADE","n"=>"SET NULL","d"=>"SET DEFAULT",_=>return Err(anyhow!("TABLE_FOREIGN_KEY_ACTION_UNKNOWN"))})}
fn ordered_mysql_indexes(rows:Vec<Record>)->Result<Vec<Record>>{let mut indexed=rows.into_iter().map(|r|Ok((get(&r,"Key_name")?.to_string(),get(&r,"Seq_in_index")?.parse::<usize>()?,r))).collect::<Result<Vec<_>>>()?;indexed.sort_by(|a,b|(&a.0,a.1).cmp(&(&b.0,b.1)));let mut previous=None;let mut expected=0;for(name,sequence,_)in &indexed{if previous!=Some(name){previous=Some(name);expected=1;}ensure!(*sequence==expected,"TABLE_INDEX_ORDINAL_PARTIAL: 索引列顺序目录缺失或重复");expected+=1;}Ok(indexed.into_iter().map(|(_,_,r)|r).collect())}
fn default_expression(part:&str)->Result<Option<String>>{
    let ts=crate::table_objects::tokens(part)?;let Some(i)=ts.iter().position(|t|t.word("DEFAULT"))else{return Ok(None)};let first=ts.get(i+1).ok_or_else(||anyhow!("TABLE_DEFAULT_UNKNOWN"))?;let mut end=first.end;
    if first.kind==crate::table_objects::TokenKind::Word&&(first.text.starts_with('_')||first.text.eq_ignore_ascii_case("b")||first.text.eq_ignore_ascii_case("x")){if let Some(literal)=ts.get(i+2).filter(|t|t.kind==crate::table_objects::TokenKind::Literal){end=literal.end;}}
    if first.symbol("(")||ts.get(i+2).is_some_and(|t|t.symbol("(")){let begin=if first.symbol("("){i+1}else{i+2};let mut depth=0;for t in ts.iter().skip(begin){if t.symbol("("){depth+=1;}if t.symbol(")"){depth-=1;if depth==0{end=t.end;break;}}}ensure!(depth==0,"TABLE_DEFAULT_UNBALANCED");}
    Ok(Some(part[first.start..end].into()))
}

#[cfg(test)]mod tests{use super::*;
    #[test]fn mysql_session_metadata_requires_real_charset_and_collation(){
        let source:Record=serde_json::from_value(serde_json::json!({"charset_client":"utf8mb4","collation_connection":"utf8mb4_0900_ai_ci"})).unwrap();
        let mut table=Record::new();mysql_session_environment(&mut table,&source).unwrap();assert_eq!(get(&table,"session_charset_client").unwrap(),"utf8mb4");assert_eq!(get(&table,"session_collation_connection").unwrap(),"utf8mb4_0900_ai_ci");
        for field in ["charset_client","collation_connection"]{
            let mut invalid=source.clone();invalid.remove(field);assert!(mysql_session_environment(&mut Record::new(),&invalid).is_err(),"missing {field}");
            invalid.insert(field.into(),None);assert!(mysql_session_environment(&mut Record::new(),&invalid).is_err(),"null {field}");
            invalid.insert(field.into(),Some(String::new()));assert!(mysql_session_environment(&mut Record::new(),&invalid).is_err(),"empty {field}");
        }
    }
    #[test]fn pg_clear_capability_refuses_rls_force_rls_and_unknown_metadata(){
        let pg=DbKind::of("postgresql");let baseline:Record=serde_json::from_value(serde_json::json!({"rls":"false","force_rls":"false"})).unwrap();assert!(clear_capability(&pg,&baseline,false).supported);
        assert!(!clear_capability(&pg,&baseline,true).supported);
        for field in ["rls","force_rls"]{
            for value in [Some("true"),Some(""),None]{let mut table=baseline.clone();table.insert(field.into(),value.map(str::to_string));let capability=clear_capability(&pg,&table,false);assert!(!capability.supported);assert!(capability.reason.as_deref().unwrap().contains("清空表不可用"));}
            let mut table=baseline.clone();table.remove(field);let capability=clear_capability(&pg,&table,false);assert!(!capability.supported);assert!(capability.reason.as_deref().unwrap().contains(field));
        }
        let mut rls=baseline.clone();rls.insert("rls".into(),Some("true".into()));assert!(clear_capability(&pg,&rls,false).reason.unwrap().contains("RLS/FORCE RLS"));
    }
    #[test]fn mysql_primary_key_metadata_uses_real_ordinal_and_rejects_partial_rows(){let row=|name:&str,seq:&str|{let mut r=Record::new();r.insert("Key_name".into(),Some("PRIMARY".into()));r.insert("Seq_in_index".into(),Some(seq.into()));r.insert("Column_name".into(),Some(name.into()));r};let sorted=ordered_mysql_indexes(vec![row("b","2"),row("a","1")]).unwrap();assert_eq!(get(&sorted[0],"Column_name").unwrap(),"a");assert_eq!(get(&sorted[1],"Column_name").unwrap(),"b");assert!(ordered_mysql_indexes(vec![row("b","2")]).is_err());}
    #[test]fn mysql_default_metadata_keeps_bit_charset_literals_and_function_precision(){
        for (column,expected) in [("flag bit(4) DEFAULT b'1010'","b'1010'"),("label varchar(30) DEFAULT _utf8mb4'中文'","_utf8mb4'中文'"),("created timestamp(6) DEFAULT CURRENT_TIMESTAMP(6)","CURRENT_TIMESTAMP(6)"),("amount int DEFAULT -12","-12"),("label text DEFAULT 'src literal' COMMENT 'kept'","'src literal'")]{assert_eq!(default_expression(column).unwrap().as_deref(),Some(expected));}
    }
    #[test]fn mysql_table_id_sqls_use_official_catalog(){
        // TiDB 的物理表 ID 在 information_schema.TABLES.TIDB_TABLE_ID，
        // 不是兼容的 INNODB_* 或旧 TIDB 目录；混用会让 TiDB 表设计读不出身份。
        let tidb=mysql_table_id_sqls(&MysqlEvidence{tidb:true,server_uuid:true},"`db`","`t`","db/t");
        assert_eq!(tidb.len(),1);
        assert!(tidb[0].contains("information_schema.TABLES")&&tidb[0].contains("TIDB_TABLE_ID"));
        assert!(tidb[0].contains("TABLE_SCHEMA=`db`")&&tidb[0].contains("TABLE_NAME=`t`"));
        assert!(!tidb[0].contains("INNODB")&&!tidb[0].contains(concat!("TIDB","_TABLES")));
        let innodb=mysql_table_id_sqls(&MysqlEvidence{tidb:false,server_uuid:true},"`db`","`t`","db/t");
        assert_eq!(innodb.len(),2);
        assert!(innodb[0].contains("information_schema.INNODB_TABLES WHERE NAME='db/t'"));
        assert!(innodb[1].contains("information_schema.INNODB_SYS_TABLES WHERE NAME='db/t'"));
        assert!(!innodb.iter().any(|s|s.contains("TIDB_TABLE_ID")));
    }
    #[test]fn tidb_is_not_treated_as_native_mysql_even_with_server_uuid(){
        // TiDB 也暴露兼容的 @@server_uuid；不能因此要求 InnoDB TABLE_ID 或走原生强制校验。
        assert!(!MysqlEvidence{tidb:true,server_uuid:true}.requires_innodb_identity());
        assert!(MysqlEvidence{tidb:false,server_uuid:true}.requires_innodb_identity());
        assert!(!MysqlEvidence{tidb:false,server_uuid:false}.requires_innodb_identity());
    }
    #[test]fn mysql_table_id_rejects_empty_ambiguous_and_invalid(){
        let row=|id:&str|{let mut r=Record::new();r.insert("id".into(),Some(id.into()));r};
        assert_eq!(mysql_table_id(&[]).unwrap(),None);
        assert_eq!(mysql_table_id(&[row("128")]).unwrap().as_deref(),Some("128"));
        assert!(mysql_table_id(&[row("1"),row("2")]).is_err());
        assert!(mysql_table_id(&[row("0")]).is_err());
        assert!(mysql_table_id(&[row("")]).is_err());
        assert!(mysql_table_id(&[row("  ")]).is_err());
        assert!(mysql_table_id(&[row("not-a-number")]).is_err());
        let mut null=Record::new();null.insert("id".into(),None);
        assert!(mysql_table_id(&[null]).is_err());
    }
    fn pg_full_evidence()->PgEvidence{PgEvidence{attgenerated:true,attidentity:true,attcompression:true,indnkeyatts:true,sequences:true,relrowsecurity:true,relforcerowsecurity:true,relreplident:true,relispartition:true,mysql_compat:false}}
    #[test]fn pg_index_layers_keep_modern_and_compat_separate(){
        let modern=pg_index_sql(&pg_full_evidence(),"42");
        assert!(modern.contains("ORDINALITY")&&modern.contains("indnkeyatts"));
        let compat=pg_index_compat_sql("42");
        // 兼容层必须完全避开老内核不支持的语法。
        assert!(!compat.contains("ORDINALITY")&&!compat.contains("indnkeyatts")&&!compat.contains("to_jsonb"));
        assert!(compat.contains("string_to_array")&&compat.contains("generate_series"));
    }
    #[test]fn pg_columns_fallback_uses_information_schema_only(){
        let sql=pg_columns_information_schema("'db'","'t'");
        assert!(sql.contains("information_schema.columns"));
        assert!(!sql.contains("attgenerated")&&!sql.contains("attidentity")&&!sql.contains("pg_sequence")&&!sql.contains("to_jsonb"));
        // 别名必须与 columns() 读取的字段一致。
        for alias in ["AS name","AS type","AS nullable","AS \"default\"","AS auto","AS comment","AS generated"]{assert!(sql.contains(alias),"{alias}");}
    }
    #[test]fn mysql_compat_mode_refusal_is_wired_into_snapshot(){
        let source=include_str!("table_workflows_native_metadata.rs");
        // B 模式必须由探测得到并直接拒绝，而不是继续按 PG 语义读或写。
        assert!(source.contains("mysql_compat:db::pg_mysql_compat_mode(&compat)"));
        assert!(source.contains("ensure!(!evidence.mysql_compat"));
        assert!(source.contains("TABLE_PG_MYSQL_COMPAT_MODE_UNSUPPORTED"));
    }
}
