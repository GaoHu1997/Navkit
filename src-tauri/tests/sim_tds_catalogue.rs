//! TDS packet simulation, NOT a SQL Server deployment. The actual tiberius client is exercised.
use anyhow::{anyhow, ensure, Context, Result};
use navkit_lib::{catalog::CATALOG,db::{self,DbRegistry},models::{ConnectionRequest,DbKind,PageRequest,Protocol},schema};
use serde_json::{json,Value};
use std::{path::{Path,PathBuf},process::{Child,Command,Stdio},time::Duration};
struct Server { child:Child,dir:PathBuf,port:u16,user:String,password:String,database:String }
impl Server {
 async fn start()->Result<Self>{
  let token=uuid::Uuid::new_v4().simple().to_string();let dir=std::env::temp_dir().join(format!("navkit-sim-tds-{token}"));std::fs::create_dir_all(&dir)?;
  let user=format!("user_{}",&token[..12]);let password=format!("pwd-{token}");let database=format!("db_{}",&token[..12]);
  let script=Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/simulation/native/tds_server.py");
  let stderr=std::fs::File::create(dir.join("stderr.log"))?;
  let mut cmd=Command::new(std::env::var("NAVKIT_PYTHON").unwrap_or_else(|_|"python".into()));
  cmd.arg(script).arg(&dir).env("NAVKIT_TDS_USER",&user).env("NAVKIT_TDS_PASSWORD",&password).env("NAVKIT_TDS_DATABASE",&database).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(stderr));
  #[cfg(windows)] { use std::os::windows::process::CommandExt;cmd.creation_flags(0x08000000); }
  let child=cmd.spawn().context("start owned TDS loopback simulator")?;
  let mut server=Self{child,dir,port:0,user,password,database};
  for _ in 0..100{
   if let Ok(bytes)=std::fs::read(server.dir.join("ready.json")){if let Ok(v)=serde_json::from_slice::<Value>(&bytes){if let Some(port)=v["port"].as_u64(){server.port=u16::try_from(port)?;return Ok(server);}}}
   if let Some(status)=server.child.try_wait()?{return Err(anyhow!("TDS simulator exited {status}: {}",std::fs::read_to_string(server.dir.join("stderr.log")).unwrap_or_default()));}
   tokio::time::sleep(Duration::from_millis(50)).await;
  }
  Err(anyhow!("TDS simulator readiness deadline"))
 }
 fn req(&self,kind:&str)->ConnectionRequest{ConnectionRequest{kind:DbKind::of(kind),host:"127.0.0.1".into(),port:self.port,user:self.user.clone(),password:self.password.clone(),database:self.database.clone(),file:String::new(),params:"encrypt=false".into(),timeout_secs:5}}
}
impl Drop for Server {fn drop(&mut self){if matches!(self.child.try_wait(),Ok(None)){let _=self.child.kill();}let _=self.child.wait();if self.dir.is_absolute()&&self.dir.file_name().and_then(|s|s.to_str()).is_some_and(|s|s.starts_with("navkit-sim-tds-")){let _=std::fs::remove_dir_all(&self.dir);}}}
fn lit(s:&str)->String{format!("N'{}'",s.replace('\'',"''"))}
fn page(id:&str,table:&str,offset:u64,limit:u64)->PageRequest{PageRequest{connection_id:id.into(),schema:"dbo".into(),table:table.into(),offset,limit,order_by:Some("id".into()),ascending:true,filters:vec![],filter_logic:Default::default(),order_rules:None}}
async fn run(server:&Server,kind:&str,steps:&mut Vec<Value>)->Result<Value>{
 let req=server.req(kind);let registry=DbRegistry::default();let id=format!("tds-{}",uuid::Uuid::new_v4().simple());let table=format!("rows_{}",uuid::Uuid::new_v4().simple());let full=format!("[dbo].[{table}]");
 let original=format!("{}:中文;O'Reilly\\尾🙂",uuid::Uuid::new_v4());let updated=format!("changed:{}",original);
 db::open_and_register(&registry,&id,&req).await?;steps.push(json!({"step":"actual_native_tiberius_login","pass":true}));
 let result:Result<Value>=async{
  db::execute(&registry,&id,&format!("CREATE TABLE {full} ([id] INT NOT NULL PRIMARY KEY,[payload] NVARCHAR(2000),[nullable_value] NVARCHAR(100))"),req.kind.clone()).await?;
  db::execute(&registry,&id,&format!("INSERT INTO {full} VALUES(1,{},NULL),(2,N'second',N'present')",lit(&original)),req.kind.clone()).await?;
  let conn=registry.read().get(&id).cloned().ok_or_else(||anyhow!("missing native registration"))?;
  ensure!(schema::list_databases(conn.as_ref(),req.kind.clone()).await?.iter().any(|(n,_)|n==&server.database),"real native database metadata absent");
  ensure!(schema::list_tables(conn.as_ref(),req.kind.clone(),"dbo").await?.iter().any(|(n,_)|n==&table),"real native tables metadata absent");
  let structure=schema::table_schema(conn.as_ref(),req.kind.clone(),"dbo",&table).await?;ensure!(structure.columns.len()==3,"native column metadata absent");
  let initial=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&table,0,100)).await?;
  let payload=initial.columns.iter().position(|c|c.name=="payload").ok_or_else(||anyhow!("missing payload column"))?;let nullable=initial.columns.iter().position(|c|c.name=="nullable_value").ok_or_else(||anyhow!("missing nullable column"))?;
  ensure!(initial.rows.len()==2&&initial.rows[0][payload].as_deref()==Some(original.as_str())&&initial.rows[0][nullable].is_none(),"native TDS lost exact Unicode/NULL");
  let second=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&table,1,1)).await?;ensure!(second.rows==initial.rows[1..2],"TDS offset/limit ignored");
  steps.push(json!({"step":"actual_schema_columns_rows_unicode_null_paging","pass":true}));
  db::execute(&registry,&id,&format!("UPDATE {full} SET [payload]={} WHERE [id]=1",lit(&updated)),req.kind.clone()).await?;
  let query=db::execute(&registry,&id,&format!("SELECT * FROM {full} ORDER BY [id]"),req.kind.clone()).await?;ensure!(query.sets.iter().any(|s|s.rows.iter().flatten().any(|v|v.as_deref()==Some(updated.as_str()))),"UPDATE did not change actual TDS readback");
  db::execute(&registry,&id,&format!("DELETE FROM {full} WHERE [id]=2"),req.kind.clone()).await?;
  let remaining=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&table,0,100)).await?;ensure!(remaining.rows.len()==1,"DELETE did not change state");
  let empty=db::execute(&registry,&id,&format!("SELECT * FROM {full} WHERE [id]=9999"),req.kind.clone()).await?;ensure!(empty.sets.iter().any(|s|s.rows.is_empty()&&s.columns.len()==3),"empty native query lost column metadata");
  let zero=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&table,0,0)).await?;ensure!(zero.rows.is_empty(),"limit=0 did not produce empty browse");
  ensure!(db::execute(&registry,&id,"THIS IS NOT VALID SQL",req.kind.clone()).await.is_err(),"TDS invalid SQL became success");
  ensure!(db::execute(&registry,&id,&format!("SELECT * FROM {full} LIMIT 1"),req.kind.clone()).await.is_err(),"SQL Server dialect LIMIT accepted by simulator");
  let mut wrong=req.clone();wrong.password.push_str("-wrong");let denied=db::open(&wrong).await.err().ok_or_else(||anyhow!("TDS invalid LOGIN7 password accepted"))?;ensure!(format!("{denied:#}").contains("18456"),"authentication failure was not a real TDS ERROR token: {denied:#}");
  let mut malformed=req.clone();malformed.params="sslmode=not-a-valid-mode".into();ensure!(db::open(&malformed).await.is_err(),"unknown TLS mode silently downgraded to plaintext");
  let mut encrypted=req.clone();encrypted.params="sslmode=require".into();ensure!(db::open(&encrypted).await.is_err(),"TLS-required client silently accepted ENCRYPT_NOT_SUP");
  steps.push(json!({"step":"actual_mutation_empty_columns_errors_auth_tls","pass":true}));
  Ok(json!({"sentinel_exact_unicode_preserved":true,"state_changed_and_read":true,"schema_paging":true,"null_preserved":true,"vendor_server_status":"NOT_TESTED"}))
 }.await;
 let cleanup=db::execute(&registry,&id,&format!("DROP TABLE IF EXISTS {full}"),req.kind.clone()).await;db::close_with_derived(&registry,&id);
 let details=result?;cleanup.context("cleanup owned native table")?;Ok(details)
}
#[tokio::test]
#[ignore="Starts bounded loopback TDS simulator and invokes actual native product APIs"]
async fn mssql_tds_stateful_catalogue_contract(){
 let kind=CATALOG.iter().find(|k|k.protocol==Protocol::Mssql).expect("Mssql catalogue entry").id;let mut steps=vec![];let mut server=None;
 let outcome=tokio::time::timeout(Duration::from_secs(90),async{server=Some(Server::start().await?);run(server.as_ref().unwrap(),kind,&mut steps).await}).await;
 let (passed,error,details)=match outcome{Ok(Ok(details))=>(true,None,details),Ok(Err(e))=>(false,Some(format!("{e:#}")),Value::Null),Err(_)=>(false,Some("90 second bounded native TDS contract deadline".into()),Value::Null)};
 let wire=server.as_ref().map(|s|std::fs::read_to_string(s.dir.join("wire.jsonl")).unwrap_or_default()).unwrap_or_default();let stderr=server.as_ref().map(|s|std::fs::read_to_string(s.dir.join("stderr.log")).unwrap_or_default()).unwrap_or_default();drop(server);
 let path=std::env::var("NAVKIT_SIM_TDS_REPORT").map(PathBuf::from).unwrap_or_else(|_|Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/acceptance/simulation/native/tds-report.json"));if let Some(parent)=path.parent(){std::fs::create_dir_all(parent).unwrap();}
 let doc=json!({"schema_version":1,"counts":{"covered":1,"pass":usize::from(passed),"fail":usize::from(!passed)},"rows":[{"kind":kind,"status":if passed{"SIMULATED_PROTOCOL_PASS"}else{"SIMULATED_PROTOCOL_FAIL"},"evidence_type":"STATEFUL_LOOPBACK_TDS7_4_SIMULATION","limitations":["Not Microsoft SQL Server or vendor certification","Only tested SQL subset translated to actual mutable SQLite state","No TLS server emulation; TLS-required rejection is tested"],"steps":steps,"details":details,"error":error,"wire":wire,"server_stderr":stderr,"cleanup":{"owned_python_process":"killed/reaped via RAII; unique fixture directory removed"}}]});std::fs::write(&path,serde_json::to_vec_pretty(&doc).unwrap()).unwrap();assert!(passed,"{}",doc["rows"][0]["error"]);
}
