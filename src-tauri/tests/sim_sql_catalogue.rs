//! Full per-kind public-API contracts for SQL protocol families.
//! Compatible products use a stateful reference server, NOT a claim of vendor certification.
//! Run with MySQL/PostgreSQL/Redis local test endpoints and NAVKIT_MYSQL_PASSWORD.
use anyhow::{anyhow, ensure, Context, Result};
use navkit_lib::{catalog::{KindMeta, CATALOG}, db::{self, Db, DbRegistry}, models::{ConnectionRequest, DbKind, MultiResult, PageRequest, Protocol}, schema};
use serde_json::{json, Value};
use std::{path::{Path, PathBuf}, time::{Duration, Instant}};

fn port(name: &str, default: u16) -> Result<u16> { Ok(std::env::var(name).ok().map(|s| s.parse()).transpose()?.unwrap_or(default)) }
fn request(meta: &KindMeta) -> Result<ConnectionRequest> {
    let mut r = ConnectionRequest { kind: DbKind::of(meta.id), host: "127.0.0.1".into(), port: 0, user: String::new(), password: String::new(), database: String::new(), file: String::new(), params: String::new(), timeout_secs: 10 };
    match meta.protocol {
        Protocol::Mysql => { r.port=port("NAVKIT_MYSQL_PORT",3306)?; r.user=std::env::var("NAVKIT_MYSQL_USER").unwrap_or_else(|_|"root".into()); r.password=std::env::var("NAVKIT_MYSQL_PASSWORD").context("NAVKIT_MYSQL_PASSWORD must explicitly configure the reference server")?; }
        Protocol::Postgres => { r.port=port("NAVKIT_PG_PORT",5433)?; r.user=std::env::var("NAVKIT_PG_USER").unwrap_or_else(|_|"postgres".into()); r.password=std::env::var("NAVKIT_PG_PASSWORD").unwrap_or_default(); r.database=std::env::var("NAVKIT_PG_DATABASE").unwrap_or_else(|_|"postgres".into()); }
        Protocol::Redis => { r.port=port("NAVKIT_REDIS_PORT",6379)?; r.password=std::env::var("NAVKIT_REDIS_PASSWORD").unwrap_or_default(); }
        _ => {}
    }
    Ok(r)
}
fn quoted(s: &str, mysql: bool) -> String { let q=if mysql {'`'} else {'"'}; format!("{q}{}{q}",s.replace(q,&format!("{q}{q}"))) }
fn literal(s: &str) -> String { format!("'{}'",s.replace('\'',"''")) }
fn contains(result: &MultiResult, expected: &str) -> bool { result.sets.iter().flat_map(|s|&s.rows).flat_map(|r|r.iter()).any(|v|v.as_deref()==Some(expected)) }
fn has_null(result: &MultiResult) -> bool { result.sets.iter().flat_map(|s|&s.rows).any(|r|r.iter().any(Option::is_none)) }
fn page_request(id: &str, namespace: &str, table: &str, offset: u64, limit: u64) -> PageRequest {
    PageRequest { connection_id:id.into(),schema:namespace.into(),table:table.into(),offset,limit,filters:vec![],filter_logic:Default::default(),order_by:Some("id".into()),ascending:true,order_rules:None }
}
fn owned_dir(token: &str) -> Result<PathBuf> {
    let dir=std::env::temp_dir().join(format!("navkit-sim-sql-{token}"));
    ensure!(dir.is_absolute() && dir.file_name().and_then(|n|n.to_str()).is_some_and(|n|n.starts_with("navkit-sim-sql-")),"invalid isolated directory");
    std::fs::create_dir(&dir)?; Ok(dir)
}
async fn close(reg: &DbRegistry, id: &str) {
    if let Ok(connection)=db::get_db(reg,id) {
        match connection.as_ref() { Db::Sqlite(p)=>p.close().await,Db::Mysql(p)=>p.close().await,Db::Postgres(p)=>p.close().await,_=>{} }
    }
    db::close_with_derived(reg,id);
}
fn remove_owned(dir: &Path) -> Result<()> {
    // Verify the exact absolute target immediately before deletion; never delete a shared runtime.
    let actual=dir.canonicalize()?;
    ensure!(actual.file_name().and_then(|n|n.to_str()).is_some_and(|n|n.starts_with("navkit-sim-sql-")),"refusing non-owned cleanup");
    // sqlx closes the SQLite pool before its worker thread releases every OS handle.
    // Windows can briefly return sharing violation after Pool::close has completed.
    // Retry only those transient ownership/locking failures, with a hard deadline.
    let deadline=Instant::now()+Duration::from_secs(3);
    loop {
        match std::fs::remove_dir_all(&actual) {
            Ok(())=>return Ok(()),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>return Ok(()),
            Err(e) if (e.kind()==std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(),Some(32|33))) && Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(25)),
            Err(e)=>return Err(e).with_context(||format!("remove exact owned fixture {} after all product pools closed",actual.display())),
        }
    }
}
async fn sql_case(meta: &KindMeta, dir: &Path, token: &str) -> Result<Value> {
    let reg=DbRegistry::default(); let id=format!("sim-{}-{token}",meta.id); let mut r=request(meta)?;
    let mysql=meta.protocol==Protocol::Mysql;
    let namespace=if meta.protocol==Protocol::Sqlite {"main".into()} else {format!("sim_{}_{}",meta.id.chars().take(18).collect::<String>(),&token[..20])};
    let table="sentinel_rows";
    let qn=quoted(&namespace,mysql); let qt=quoted(table,mysql); let qualified=format!("{qn}.{qt}");
    if meta.protocol==Protocol::Sqlite { r.file=dir.join("database.sqlite3").to_string_lossy().into(); }
    if meta.id=="h2" { r.file=dir.join("h2-file").to_string_lossy().replace('\\',"/");r.user="sa".into();r.password=format!("fixture-{token}"); }
    let mut namespace_created=false;
    let run: Result<Value>=async {
        db::open_and_register(&reg,&id,&r).await?;
        if meta.protocol!=Protocol::Sqlite {
            let command=if mysql {format!("CREATE DATABASE {qn}")} else {format!("CREATE SCHEMA {qn}")};
            db::execute(&reg,&id,&command,r.kind.clone()).await?; namespace_created=true;
        }
        let payload=format!("{token}:{}:中文;O'Reilly\\尾🙂",meta.id);
        let second=format!("second:{token}:{}",meta.id); let updated=format!("updated:{token}:{}:中文\\尾",meta.id);
        let qi=quoted("id",mysql);let qp=quoted("payload",mysql);let qv=quoted("nullable_value",mysql);
        db::execute(&reg,&id,&format!("CREATE TABLE {qualified} ({qi} INTEGER PRIMARY KEY, {qp} VARCHAR(1000), {qv} VARCHAR(30))"),r.kind.clone()).await?;
        db::execute(&reg,&id,&format!("INSERT INTO {qualified} VALUES (1,{},NULL),(2,{},'present')",literal(&payload),literal(&second)),r.kind.clone()).await?;
        let connection=db::get_db(&reg,&id)?;
        let containers=schema::list_databases(connection.as_ref(),r.kind.clone()).await?;
        ensure!(containers.iter().any(|(n,_)|n==&namespace),"provisioned namespace not discoverable");
        let objects=schema::list_tables(connection.as_ref(),r.kind.clone(),&namespace).await?;
        ensure!(objects.iter().any(|(n,_)|n==table),"provisioned table not discoverable");
        let columns=schema::table_schema(connection.as_ref(),r.kind.clone(),&namespace,table).await?;
        ensure!(columns.columns.iter().any(|c|c.name=="payload") && columns.columns.iter().any(|c|c.name=="nullable_value"),"column metadata missing");
        let first=schema::page_table(connection.as_ref(),r.kind.clone(),&page_request(&id,&namespace,table,0,1)).await?;
        ensure!(first.rows.len()==1 && first.rows[0].iter().any(|v|v.as_deref()==Some(&payload)) && first.rows[0].iter().any(Option::is_none),"first page lost Unicode or NULL");
        let next=schema::page_table(connection.as_ref(),r.kind.clone(),&page_request(&id,&namespace,table,1,1)).await?;
        ensure!(next.rows.len()==1 && next.rows[0].iter().any(|v|v.as_deref()==Some(&second)),"offset/limit ignored");
        let exhausted=schema::page_table(connection.as_ref(),r.kind.clone(),&page_request(&id,&namespace,table,2,1)).await?;
        ensure!(exhausted.rows.is_empty(),"exhausted page must be empty");
        let result=db::execute(&reg,&id,&format!("SELECT * FROM {qualified} ORDER BY {qi}"),r.kind.clone()).await?;
        ensure!(contains(&result,&payload) && contains(&result,&second) && has_null(&result),"query result lost data");
        db::execute(&reg,&id,&format!("UPDATE {qualified} SET {qp}={} WHERE {qi}=1",literal(&updated)),r.kind.clone()).await?;
        let changed=db::execute(&reg,&id,&format!("SELECT * FROM {qualified} WHERE {qi}=1"),r.kind.clone()).await?;
        ensure!(contains(&changed,&updated) && !contains(&changed,&payload),"state update was not observable");
        ensure!(db::execute(&reg,&id,&format!("SELECT * FROM {qn}.{}",quoted(&format!("missing_{token}"),mysql)),r.kind.clone()).await.is_err(),"nonexistent table falsely succeeded");
        ensure!(db::execute(&reg,&id,"THIS IS NOT VALID SQL",r.kind.clone()).await.is_err(),"malformed SQL falsely succeeded");
        if meta.protocol==Protocol::Sqlite {
            let mut invalid=r.clone();invalid.file.clear();ensure!(db::open(&invalid).await.is_err(),"missing SQLite file was accepted");
        } else if meta.id=="h2" {
            let oversized=db::execute(&reg,&id,"SELECT X FROM SYSTEM_RANGE(1,10001)",r.kind.clone()).await.err().ok_or_else(||anyhow!("H2 oversized query was silently truncated to successful partial rows"))?;
            ensure!(format!("{oversized:#}").contains("54000"),"oversized H2 result must report the safety limit");
        } else {
            let mut invalid=r.clone();invalid.user=format!("missing_user_{}",&token[..16]);invalid.timeout_secs=3;
            ensure!(db::open(&invalid).await.is_err(),"unknown credential identity was accepted");
            let mut ssl=r.clone();ssl.params="ssl-mode=not-a-valid-mode".into();
            ensure!(db::open(&ssl).await.is_err(),"unknown TLS mode was silently ignored");
        }
        db::execute(&reg,&id,&format!("DELETE FROM {qualified} WHERE {qi}=2"),r.kind.clone()).await?;
        let deleted=db::execute(&reg,&id,&format!("SELECT * FROM {qualified}"),r.kind.clone()).await?;
        ensure!(contains(&deleted,&updated) && !contains(&deleted,&second),"delete was not observable");
        Ok(json!({"connection":true,"namespace":true,"objects":true,"columns":true,"unicode":true,"null":true,"query":true,"paging_offset_limit":true,"exhausted_page":true,"state_update":true,"state_delete":true,"invalid_sql_rejected":true,"missing_object_rejected":true,"bad_connection_rejected":meta.id!="h2","unknown_tls_rejected":!matches!(meta.protocol,Protocol::Sqlite)&&meta.id!="h2","sql_operations":["CREATE","INSERT","SELECT","UPDATE","DELETE"]}))
    }.await;
    let mut cleanup_errors=Vec::new();
    if namespace_created && meta.id!="h2" {
        let command=if mysql {format!("DROP DATABASE IF EXISTS {qn}")} else {format!("DROP SCHEMA IF EXISTS {qn} CASCADE")};
        if let Err(e)=db::execute(&reg,&id,&command,r.kind.clone()).await {cleanup_errors.push(format!("{e:#}"));}
    }
    close(&reg,&id).await;
    ensure!(cleanup_errors.is_empty(),"cleanup failed: {}",cleanup_errors.join("; "));
    let mut checks=run?;
    if meta.id=="h2" {
        // The ordinary public close path must persist committed data, without a manual SHUTDOWN workaround.
        db::open_and_register(&reg,&id,&r).await?;
        let persisted=db::execute(&reg,&id,&format!("SELECT * FROM {qualified} WHERE {}=1",quoted("id",false)),r.kind.clone()).await;
        let clean=db::execute(&reg,&id,&format!("DROP SCHEMA IF EXISTS {qn} CASCADE"),r.kind.clone()).await;
        close(&reg,&id).await;
        let persisted=persisted?;clean?;
        ensure!(contains(&persisted,&format!("updated:{token}:{}:中文\\尾",meta.id)),"H2 public close lost committed file data");
        let mut wrong=r.clone();wrong.password=format!("wrong-{token}");wrong.timeout_secs=3;
        let denied=db::open(&wrong).await.err().ok_or_else(||anyhow!("H2 wrong file-database password accepted"))?;
        ensure!(format!("{denied:#}").contains("28000"),"H2 rejection must be real authentication, not a file lock or missing runtime: {denied:#}");
        checks["file_data_persisted_across_public_close"]=json!(true);
        checks["bad_connection_rejected"]=json!(true);
        checks["authentication_mode"]=json!("wrong-password-after-file-session-close");
    } else if meta.protocol==Protocol::Postgres {
        checks["authentication_mode"]=json!("unknown-role-rejected; local reference may use trust auth, password auth not claimed");
    }
    checks["cleanup"]=json!({"success":true,"scope":"only unique provisioned namespace/file"}); Ok(checks)
}
async fn redis_case(meta: &KindMeta, token: &str) -> Result<Value> {
    let reg=DbRegistry::default();let id=format!("sim-redis-{token}");let r=request(meta)?;let key=format!("navkit_sim_redis_{token}");
    let value=format!("{token}:中文;O'Reilly\\尾🙂");let update=format!("changed:{token}:中文\\尾");
    let run:Result<Value>=async {
        db::open_and_register(&reg,&id,&r).await?;
        let quote=|s:&str|serde_json::to_string(s).expect("string serialization");
        db::execute(&reg,&id,&format!("SET {key} {}",quote(&value)),r.kind.clone()).await?;
        let connection=db::get_db(&reg,&id)?;
        ensure!(schema::list_databases(connection.as_ref(),r.kind.clone()).await?.iter().any(|(n,_)|n=="db0"),"Redis db0 not discoverable");
        ensure!(schema::list_tables(connection.as_ref(),r.kind.clone(),"db0").await?.iter().any(|(n,_)|n==&key),"Redis key not discoverable");
        let page=schema::page_table(connection.as_ref(),r.kind.clone(),&PageRequest{order_by:None,..page_request(&id,"db0",&key,0,100)}).await?;
        ensure!(page.rows.iter().flat_map(|r|r.iter()).any(|v|v.as_deref()==Some(&value)),"Redis preview lost Unicode");
        ensure!(contains(&db::execute(&reg,&id,&format!("GET {key}"),r.kind.clone()).await?,&value),"Redis GET lost Unicode");
        db::execute(&reg,&id,&format!("SET {key} {}",quote(&update)),r.kind.clone()).await?;
        let changed=db::execute(&reg,&id,&format!("GET {key}"),r.kind.clone()).await?;ensure!(contains(&changed,&update)&&!contains(&changed,&value),"Redis SET did not change state");
        ensure!(db::execute(&reg,&id,"NAVKIT_INVALID_REDIS_COMMAND",r.kind.clone()).await.is_err(),"Redis unknown command accepted");
        ensure!(db::execute(&reg,&id,"SET 'unterminated",r.kind.clone()).await.is_err(),"Redis invalid quoting accepted");
        let missing=db::execute(&reg,&id,&format!("GET missing_{token}"),r.kind.clone()).await?;ensure!(has_null(&missing),"Redis missing key did not preserve NIL");
        let mut bad=r.clone();bad.params="ssl-mode=not-a-valid-mode".into();ensure!(db::open(&bad).await.is_err(),"Redis unknown TLS mode ignored");
        Ok(json!({"connection":true,"namespace":true,"objects":true,"unicode":true,"null":true,"query":true,"state_update":true,"invalid_command_rejected":true,"invalid_quoting_rejected":true,"unknown_tls_rejected":true,"scope":"string key; all-type and binary separate existing real Redis test"}))
    }.await;
    let cleanup=if db::get_db(&reg,&id).is_ok(){db::execute(&reg,&id,&format!("DEL {key}"),r.kind.clone()).await.map(|_|())}else{Ok(())};
    db::close_with_derived(&reg,&id);cleanup?; let mut checks=run?;checks["cleanup"]=json!({"success":true,"scope":"only UUID key"});Ok(checks)
}
#[tokio::test]
#[ignore = "stateful local reference servers; use explicit MySQL password and local endpoints"]
async fn every_sql_family_catalogue_entry_has_stateful_public_api_evidence() {
    let entries:Vec<_>=CATALOG.iter().filter(|k|matches!(k.protocol,Protocol::Mysql|Protocol::Postgres|Protocol::Sqlite|Protocol::Redis)||k.id=="h2").collect();
    assert!(!entries.is_empty());let mut rows=Vec::new();
    for meta in entries {
        let token=uuid::Uuid::new_v4().simple().to_string();let started=Instant::now();
        let dir=owned_dir(&token).expect("create isolated test directory");
        let outcome=tokio::time::timeout(Duration::from_secs(60),async {if meta.protocol==Protocol::Redis{redis_case(meta,&token).await}else{sql_case(meta,&dir,&token).await}}).await;
        let cleanup=remove_owned(&dir);
        let result=match outcome {Ok(r)=>r,Err(_)=>Err(anyhow!("per-kind contract timed out"))}.and_then(|v|cleanup.map(|_|v));
        let backend=match meta.protocol {Protocol::Mysql=>"mysql-reference",Protocol::Postgres=>"postgresql-reference",Protocol::Sqlite=>"sqlite-real-file",Protocol::Redis=>"redis-reference",_=>"h2-real-jdbc-file"};
        let status=if result.is_ok(){if matches!(meta.id,"mysql"|"postgresql"|"sqlite"|"redis"|"h2"){"LIVE_REFERENCE_PASS"}else{"SIMULATED_COMPATIBILITY_PASS"}}else{"FAIL"};
        let (checks,error)=match result {Ok(v)=>(v,Value::Null),Err(e)=>(Value::Null,json!(format!("{e:#}")))};
        println!("{} {status}",meta.id);
        rows.push(json!({"kind":meta.id,"label":meta.label,"status":status,"backend":backend,"elapsed_ms":started.elapsed().as_millis(),"checks":checks,"error":error,"limitations":["Compatible product contracts exercise the actual product client against a stateful reference protocol server, not that vendor's own server.","No vendor-specific extensions, cluster management, distributed transactions, or server-brand certification is claimed."]}));
    }
    let fail=rows.iter().filter(|r|r["status"]=="FAIL").count();
    let report=json!({"suite":"sql-family-per-kind-stateful-contracts","total":rows.len(),"pass":rows.len()-fail,"fail":fail,"coverage":"Every current SQL/Redis catalogue entry plus H2; no missing entries counted pass","rows":rows});
    let output=std::env::var("NAVKIT_SIM_SQL_REPORT").unwrap_or_else(|_|Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/simulation/sql-family-report.json").to_string_lossy().into_owned());
    let output=Path::new(&output);if let Some(parent)=output.parent(){std::fs::create_dir_all(parent).expect("create report directory");}
    std::fs::write(output,serde_json::to_vec_pretty(&report).unwrap()).expect("persist all per-kind failures before assertion");
    assert_eq!(fail,0,"see {} for actual failed cases",output.display());
}

#[test]
fn emit_current_catalogue_simulation_manifest() {
    let mut seen=std::collections::HashSet::new();
    let entries=CATALOG.iter().map(|meta|{
        assert!(seen.insert(meta.id),"duplicate catalogue id {}",meta.id);
        assert!(!meta.planned,"planned route has no implementation: {}",meta.id);
        let suite=if matches!(meta.protocol,Protocol::Mysql|Protocol::Postgres|Protocol::Sqlite|Protocol::Redis)||meta.id=="h2" {"sql"}
        else if matches!(meta.protocol,Protocol::Http|Protocol::Clickhouse)||matches!(meta.id,"druid"|"dynamodb"|"etcd"|"rqlite") {"http"}
        else if meta.protocol==Protocol::Odbc {"odbc"} else {"native"};
        json!({"kind":meta.id,"label":meta.label,"protocol":format!("{:?}",meta.protocol),"suite":suite})
    }).collect::<Vec<_>>();
    let path=std::env::var("NAVKIT_SIM_MANIFEST").map(PathBuf::from).unwrap_or_else(|_|Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/simulation/catalogue-manifest.json"));
    if let Some(parent)=path.parent(){std::fs::create_dir_all(parent).expect("manifest directory");}
    std::fs::write(path,serde_json::to_vec_pretty(&json!({"source":"Rust CATALOG, not stale generated TypeScript","total":entries.len(),"entries":entries})).unwrap()).expect("persist authoritative manifest");
}
