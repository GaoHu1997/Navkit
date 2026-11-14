//! Catalogue-wide ODBC product-route simulation, NEVER vendor/server acceptance.
//! Uses a test-only strict dialect proxy + the complete real DuckDB ODBC driver.
//! Build the independent core + 92-export facade first:
//! pwsh -File tools/acceptance/simulation/odbc/build-proxy.ps1
//! NAVKIT_SIM_ODBC_DRIVER, NAVKIT_SIM_ODBC_BACKEND, NAVKIT_SIM_ODBC_REPORT are mandatory.
use navkit_lib::{catalog::CATALOG, db, models::*, schema};
use serde_json::{json,Value};
use std::{collections::{HashMap,HashSet},path::PathBuf,time::{Duration,Instant}};

fn request(kind:&str,file:&std::path::Path,driver:&std::path::Path)->ConnectionRequest {
 let q=navkit_lib::escape_odbc_connection_value;
 ConnectionRequest {kind:DbKind::of(kind),host:"127.0.0.1".into(),port:0,user:"navkit_sim".into(),password:"contract;secret}中文".into(),database:"main".into(),file:file.display().to_string(),
 params:format!("Driver={};Database={};NavkitSimKind={};UID=navkit_sim;PWD={};",q(&driver.display().to_string()),q(&file.display().to_string()),kind,q("contract;secret}中文")),timeout_secs:5}
}
fn profile(id:&str)->(&'static str,&'static str) {
 match id {
 "oracle"|"dameng"=>("ORACLE_12C_ANSI_SUBSET","SELECT * FROM main.navkit_missing LIMIT 1"),
 "db2"=>("DB2_STANDARD_OFFSET_FETCH_SUBSET","SELECT * FROM main.navkit_missing LIMIT 1"),
 "firebird"=>("FIREBIRD_FIRST_SKIP_SUBSET","SELECT * FROM main.navkit_missing LIMIT 1"),
 "informix"=>("INFORMIX_SKIP_FIRST_SUBSET","SELECT * FROM main.navkit_missing LIMIT 1"),
 "access"|"iris"|"cache"=>("TOP_CLIENT_SKIP_SUBSET","SELECT * FROM main.navkit_missing LIMIT 1"),
 "hive"|"inceptor"|"argodb"|"spark"|"impala"|"kyuubi"=>("HIVE_COMPATIBLE_NO_OFFSET_SUBSET","SELECT * FROM main.navkit_missing LIMIT 1 OFFSET 1"),
 _=>("ODBC_ANSI_LIMIT_OFFSET_SUBSET","SELECT TOP 1 * FROM main.navkit_missing"),
 }
}
fn assert_rejected_dialect_message(error:&anyhow::Error)->anyhow::Result<()> {
 let msg=format!("{error:#}");anyhow::ensure!(msg.contains("42000")&&(msg.contains("dialect profile")||msg.contains("profile")),"must reject dialect before DuckDB parser/table lookup: {msg}"); Ok(())
}
fn page_request(id:&str,schema:&str,table:&str,offset:u64,limit:u64)->PageRequest {
 PageRequest {connection_id:id.into(),schema:schema.into(),table:table.into(),offset,limit,include_total:true,order_by:Some("id".into()),ascending:true,filters:vec![],filter_logic:FilterLogic::And,order_rules:None}
}
const STEP_NAMES: &[&str] = &[
 "open_product_odbc", "verify_odbc_route", "fresh_schema_directory", "create_schema", "set_schema", "create_table",
 "insert_0", "insert_1", "insert_2_exact_sentinel", "insert_3", "insert_4", "schema_directory_after_create",
 "table_directory_after_create", "live_table_schema", "ordered_page_unicode_null", "execute_exact_unicode_null",
 "update_payload", "delete_row", "page_after_mutation", "empty_page_metadata", "invalid_sql_error",
 "unterminated_identifier_error", "dialect_sqlstate_42000", "authentication_sqlstate_28000", "missing_driver_error",
 "default_credential_escaping", "drop_table", "table_directory_after_drop", "reset_schema", "drop_schema",
 "schema_directory_after_drop", "release_handles", "fixture_file_cleanup",
];

struct StepLog { output:PathBuf, kind:String, label:String, dialect:String, steps:Vec<Value>, active:Option<(usize,Instant)> }
impl StepLog {
 fn new(output:&std::path::Path,kind:&str,label:&str,dialect:&str)->Self {
  let log=Self {output:output.into(),kind:kind.into(),label:label.into(),dialect:dialect.into(),
   steps:STEP_NAMES.iter().enumerate().map(|(i,name)|json!({"sequence":i+1,"name":name,"status":"NOT_RUN"})).collect(),active:None};
  log.persist();log
 }
 fn persist(&self) {
  let checkpoint=json!({"checkpoint":true,"counts":{"catalogue_odbc_entries":1,"simulated_pass":0,"fail":1,"vendor_real_pass":0},
   "rows":[{"kind":self.kind,"label":self.label,"protocol":"odbc","dialect_profile":self.dialect,"status":"INCOMPLETE",
    "non_vendor_driver":true,"vendor_server_status":"UNVERIFIED","default_vendor_driver_route":"UNVERIFIED","steps":self.steps}]});
  std::fs::write(&self.output,serde_json::to_vec_pretty(&checkpoint).unwrap()).unwrap();
 }
 fn begin(&mut self,name:&str) {
  assert!(self.active.is_none());let i=STEP_NAMES.iter().position(|n|*n==name).expect("known product step");
  assert_eq!(self.steps[i]["status"],"NOT_RUN");self.steps[i]["status"]=json!("RUNNING");self.active=Some((i,Instant::now()));self.persist();
 }
 fn evidence(&mut self,value:Value) {let i=self.active.as_ref().unwrap().0;self.steps[i]["evidence"]=value;}
 fn finish(&mut self,error:Option<&str>) {
  let (i,started)=self.active.take().unwrap();self.steps[i]["elapsed_ms"]=json!(started.elapsed().as_millis());
  self.steps[i]["status"]=json!(if error.is_some(){"FAIL"}else{"PASS"});
  if let Some(error)=error {self.steps[i]["reason"]=json!(error);}self.persist();
 }
}
macro_rules! step {
 ($log:ident,$name:expr,$body:block) => {{
  $log.begin($name);let result:anyhow::Result<_>=async { $body }.await;
  match result {Ok(value)=>{$log.finish(None);value},Err(error)=>{$log.finish(Some(&format!("{error:#}")));return Err(error);}}
 }};
}

#[tokio::test]
#[ignore="explicit real ODBC backend and test-only strict simulation driver required"]
async fn every_odbc_catalogue_entry_uses_stateful_real_odbc_simulation() {
 let driver=PathBuf::from(std::env::var("NAVKIT_SIM_ODBC_DRIVER").expect("absolute test proxy DLL"));
 let backend=PathBuf::from(std::env::var("NAVKIT_SIM_ODBC_BACKEND").expect("absolute complete DuckDB DLL"));
 let output=PathBuf::from(std::env::var("NAVKIT_SIM_ODBC_REPORT").expect("evidence path"));
 if std::env::var("NAVKIT_SIM_ODBC_LIST_ONLY").as_deref()==Ok("1") {
  let kinds:Vec<Value>=CATALOG.iter().filter(|m|m.protocol==Protocol::Odbc).map(|m|json!({"kind":m.id,"label":m.label,"protocol":"odbc"})).collect();
  std::fs::write(&output,serde_json::to_vec_pretty(&json!({"catalogue_only":true,"kinds":kinds})).unwrap()).unwrap();return;
 }
 assert!(driver.is_absolute()&&backend.is_absolute()&&driver.is_file()&&backend.is_file());
 let root=output.parent().unwrap().join(format!("databases-{}",uuid::Uuid::new_v4().simple()));std::fs::create_dir_all(&root).unwrap();
 let mut rows=Vec::<Value>::new();let mut failed=Vec::new();let mut covered=HashSet::new();
 let selected=std::env::var("NAVKIT_SIM_ODBC_KIND").ok();
 if let Some(ref id)=selected {assert!(CATALOG.iter().any(|m|m.protocol==Protocol::Odbc&&m.id==id),"unknown ODBC simulation kind {id}");}
 for meta in CATALOG.iter().filter(|m|m.protocol==Protocol::Odbc&&selected.as_deref().map_or(true,|id|id==m.id)) {
 assert!(covered.insert(meta.id));let started=Instant::now();let file=root.join(format!("{}-{}.duckdb",meta.id,uuid::Uuid::new_v4().simple()));
 let req=request(meta.id,&file,&driver);let reg:db::DbRegistry=parking_lot::RwLock::new(HashMap::new());let id=format!("odbc-contract-{}",meta.id);
 let schema_name=format!("NS_{}",uuid::Uuid::new_v4().simple());let table="PAYLOAD_TABLE";
 let sentinel=format!("中文;O'Reilly\\尾:{}",uuid::Uuid::new_v4().simple());let full=db::qualified(&req.kind,&schema_name,table);
 let (dialect,illegal)=profile(meta.id);
 let mut steps=StepLog::new(&output,meta.id,meta.label,dialect);
 let result=tokio::time::timeout(Duration::from_secs(60),async {
   step!(steps,"open_product_odbc",{db::open_and_register(&reg,&id,&req).await?;Ok(())});
   let conn=step!(steps,"verify_odbc_route",{let conn=db::get_db(&reg,&id)?;anyhow::ensure!(matches!(conn.as_ref(),db::Db::Odbc(_)),"custom test ODBC route must not escape to JDBC/native");Ok(conn)});
   step!(steps,"fresh_schema_directory",{let before=schema::list_databases(conn.as_ref(),req.kind.clone()).await?;anyhow::ensure!(!before.iter().any(|(n,_)|n==&schema_name),"fresh UUID schema cannot exist");Ok(())});
   step!(steps,"create_schema",{db::execute(&reg,&id,&format!("CREATE SCHEMA {}",db::quote_ident(&req.kind,&schema_name)),req.kind.clone()).await?;Ok(())});
   // The fixture selects the newly created namespace for CURRENT_SCHEMA-aware real queries.
   step!(steps,"set_schema",{db::execute(&reg,&id,&format!("SET schema = {}",db::quote_literal(&schema_name)),req.kind.clone()).await?;Ok(())});
   step!(steps,"create_table",{db::execute(&reg,&id,&format!("CREATE TABLE {full} (id INTEGER PRIMARY KEY,payload VARCHAR(4096),optional_value VARCHAR(30))"),req.kind.clone()).await?;Ok(())});
   for n in 0..5 {let payload=if n==2{sentinel.clone()}else{format!("row-{n}-中文")};let name=["insert_0","insert_1","insert_2_exact_sentinel","insert_3","insert_4"][n];
    step!(steps,name,{db::execute(&reg,&id,&format!("INSERT INTO {full} VALUES ({n},{},NULL)",db::quote_literal(&payload)),req.kind.clone()).await?;Ok(())});}
   step!(steps,"schema_directory_after_create",{let containers=schema::list_databases(conn.as_ref(),req.kind.clone()).await?;anyhow::ensure!(containers.iter().any(|(n,_)|n==&schema_name),"new schema must appear in real directory query");Ok(())});
   step!(steps,"table_directory_after_create",{let objects=schema::list_tables(conn.as_ref(),req.kind.clone(),&schema_name).await?;anyhow::ensure!(objects.iter().any(|(n,_)|n==table),"DDL-created table must appear");Ok(())});
   step!(steps,"live_table_schema",{let info=schema::table_schema(conn.as_ref(),req.kind.clone(),&schema_name,table).await?;anyhow::ensure!(info.columns.iter().any(|c|c.name.eq_ignore_ascii_case("payload")),"real table column metadata");steps.evidence(json!({"columns":info.columns.iter().map(|c|&c.name).collect::<Vec<_>>()}));Ok(())});
   step!(steps,"ordered_page_unicode_null",{let p=schema::page_table(conn.as_ref(),req.kind.clone(),&page_request(&id,&schema_name,table,1,2)).await?;
    anyhow::ensure!(p.total==5&&p.rows.len()==2,"paging total={} rows={}",p.total,p.rows.len());let ix=p.columns.iter().position(|c|c.name.eq_ignore_ascii_case("id")).ok_or_else(||anyhow::anyhow!("id metadata"))?;
    anyhow::ensure!(p.rows[0][ix].as_deref()==Some("1")&&p.rows[1][ix].as_deref()==Some("2"),"offset/order not silently ignored");
    anyhow::ensure!(p.rows.iter().flatten().any(|v|v.as_deref()==Some(&sentinel)),"Unicode/semicolon/apostrophe/backslash must be exact");anyhow::ensure!(p.rows.iter().all(|r|r.last()==Some(&None)),"SQL NULL must remain null");
    steps.evidence(json!({"total":p.total,"rows":p.rows,"sentinel":sentinel}));Ok(())});
   step!(steps,"execute_exact_unicode_null",{let q=db::execute(&reg,&id,&format!("SELECT payload,optional_value FROM {full} WHERE id=2"),req.kind.clone()).await?;anyhow::ensure!(q.sets.iter().any(|s|s.rows==vec![vec![Some(sentinel.clone()),None]]),"query exact sentinel and NULL");Ok(())});
   step!(steps,"update_payload",{db::execute(&reg,&id,&format!("UPDATE {full} SET payload='mutated' WHERE id=0"),req.kind.clone()).await?;Ok(())});
   step!(steps,"delete_row",{db::execute(&reg,&id,&format!("DELETE FROM {full} WHERE id=4"),req.kind.clone()).await?;Ok(())});
   step!(steps,"page_after_mutation",{let changed=schema::page_table(conn.as_ref(),req.kind.clone(),&page_request(&id,&schema_name,table,0,10)).await?;anyhow::ensure!(changed.total==4&&changed.rows.len()==4&&changed.rows.iter().flatten().any(|v|v.as_deref()==Some("mutated")),"result state must change after UPDATE/DELETE");steps.evidence(json!({"total":changed.total,"rows":changed.rows}));Ok(())});
   step!(steps,"empty_page_metadata",{let empty=schema::page_table(conn.as_ref(),req.kind.clone(),&page_request(&id,&schema_name,table,99,2)).await?;anyhow::ensure!(empty.rows.is_empty()&&!empty.columns.is_empty(),"empty page retains metadata");steps.evidence(json!({"row_count":empty.rows.len(),"column_count":empty.columns.len()}));Ok(())});
   step!(steps,"invalid_sql_error",{let err=db::execute(&reg,&id,"THIS IS NOT SQL",req.kind.clone()).await.err().ok_or_else(||anyhow::anyhow!("invalid SQL cannot return fake success"))?;steps.evidence(json!({"error":format!("{err:#}")}));Ok(())});
   step!(steps,"unterminated_identifier_error",{let err=db::execute(&reg,&id,"SELECT \"unterminated_identifier FROM main.navkit_missing",req.kind.clone()).await.err().ok_or_else(||anyhow::anyhow!("unclosed identifier must not become a valid escaped query"))?;steps.evidence(json!({"error":format!("{err:#}")}));Ok(())});
   step!(steps,"dialect_sqlstate_42000",{let err=db::execute(&reg,&id,illegal,req.kind.clone()).await.err().ok_or_else(||anyhow::anyhow!("vendor-profile violation must fail before backend accepts SQL"))?;assert_rejected_dialect_message(&err)?;steps.evidence(json!({"error":format!("{err:#}")}));Ok(())});
   step!(steps,"authentication_sqlstate_28000",{let mut bad=req.clone();bad.password="incorrect".into();bad.params=bad.params.replace(&navkit_lib::escape_odbc_connection_value(&req.password),"incorrect");let err=db::open(&bad).await.err().ok_or_else(||anyhow::anyhow!("authentication should fail"))?;anyhow::ensure!(format!("{err:#}").contains("28000"),"actual connection auth diagnostics");steps.evidence(json!({"error":format!("{err:#}")}));Ok(())});
   step!(steps,"missing_driver_error",{let mut missing=req.clone();missing.params=format!("Driver={};Database={};",navkit_lib::escape_odbc_connection_value(&root.join("definitely_missing_driver.dll").display().to_string()),navkit_lib::escape_odbc_connection_value(&file.display().to_string()));let err=db::open(&missing).await.err().ok_or_else(||anyhow::anyhow!("missing native driver must not silently fall back"))?;steps.evidence(json!({"error":format!("{err:#}")}));Ok(())});
   step!(steps,"default_credential_escaping",{let template=navkit_lib::default_odbc_connection_string(&req.kind,&req);anyhow::ensure!(!template.contains(";secret}中文"),"default credentials must be brace escaped");Ok(())});
   step!(steps,"drop_table",{db::execute(&reg,&id,&format!("DROP TABLE {full}"),req.kind.clone()).await?;Ok(())});
   step!(steps,"table_directory_after_drop",{let after=schema::list_tables(conn.as_ref(),req.kind.clone(),&schema_name).await?;anyhow::ensure!(!after.iter().any(|(n,_)|n==table),"DROP must update object directory; no fixed catalogue rows");Ok(())});
   step!(steps,"reset_schema",{db::execute(&reg,&id,"SET schema = 'main'",req.kind.clone()).await?;Ok(())});
   step!(steps,"drop_schema",{db::execute(&reg,&id,&format!("DROP SCHEMA {}",db::quote_ident(&req.kind,&schema_name)),req.kind.clone()).await?;Ok(())});
   step!(steps,"schema_directory_after_drop",{anyhow::ensure!(!schema::list_databases(conn.as_ref(),req.kind.clone()).await?.iter().any(|(n,_)|n==&schema_name),"DROP schema state must update");Ok(())});
   Ok::<Value,anyhow::Error>(json!({"dialect_profile":dialect,"route":"db::open_and_register -> Db::Odbc -> real Windows ODBC manager -> strict proxy -> real DuckDB",
    "ddl_dml_query_exact_unicode_null":true,"directory_state_changes":true,"paging_offset_order_total_empty":true,"invalid_sql_rejected":true,"dialect_violation_rejected":true,"authentication_rejected":true,"missing_driver_rejected":true,"cleanup_sql_complete":true}))
 }).await;
 if steps.active.is_some() {steps.finish(Some("ODBC contract timed out"));}
 steps.begin("release_handles");drop(reg);steps.finish(None);
 let mut entry=json!({"kind":meta.id,"label":meta.label,"protocol":"odbc","dialect_profile":dialect,"non_vendor_driver":true,"vendor_server_status":"UNVERIFIED","default_vendor_driver_route":"UNVERIFIED","elapsed_ms":started.elapsed().as_millis(),"limits":"Test-only documented SQL subset and metadata mapping. Does not validate proprietary vendor packages, default DSNs, network/TLS/auth mechanisms, all SQL syntax, cloud credentials or JDBC managed default route."});
 match result {Ok(Ok(v))=>{entry["status"]=json!("SIMULATED_ODBC_CONTRACT_PASS");entry["details"]=v;},Ok(Err(e))=>{entry["status"]=json!("FAIL");entry["reason"]=json!(format!("{e:#}"));failed.push(meta.id);},Err(_)=>{entry["status"]=json!("FAIL");entry["reason"]=json!("ODBC contract timed out");failed.push(meta.id);}}
 // Only remove the uniquely created fixture file after every handle was released.
 steps.begin("fixture_file_cleanup");let removed=std::fs::remove_file(&file);let cleanup_ok=removed.is_ok()||!file.exists();entry["fixture_file_removed"]=json!(cleanup_ok);
 if cleanup_ok {steps.finish(None);}else{steps.finish(Some(&format!("unique fixture file cleanup failed: {removed:?}")));}
 entry["steps"]=json!(steps.steps);
  if !cleanup_ok && entry["status"]=="SIMULATED_ODBC_CONTRACT_PASS" {entry["status"]=json!("FAIL");entry["reason"]=json!(format!("unique fixture file cleanup failed: {removed:?}"));failed.push(meta.id);}
 rows.push(entry);
 }
 let counts=json!({"catalogue_odbc_entries":covered.len(),"simulated_pass":rows.iter().filter(|r|r["status"]=="SIMULATED_ODBC_CONTRACT_PASS").count(),"fail":failed.len(),"vendor_real_pass":0});
 std::fs::write(&output,serde_json::to_vec_pretty(&json!({"counts":counts,"driver":driver,"backend":backend,"rows":rows})).unwrap()).unwrap();
 println!("ODBC simulation: {counts}; report={}",output.display());
 assert!(failed.is_empty(),"ODBC contracts failed: {failed:?}");
}
