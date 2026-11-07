//! Full current HTTP catalogue through actual product APIs and stateful loopback wire simulators.
//! These are protocol simulations, NOT real vendor deployments or brand certifications.
#[path="support/sim_http/mod.rs"] mod sim_http;
use navkit_lib::{catalog::CATALOG,db,models::*,schema};
use anyhow::{ensure,Result};
use serde_json::{json,Value};
use std::{collections::HashMap,time::{Duration,Instant}};

fn request(s:&sim_http::Server)->ConnectionRequest{
    let d=s.snapshot();let mut params=format!("url=http://127.0.0.1:{}",s.port);
    if d.kind=="d1"{params.push_str("/client/v4;account_id=sim_account;database_id=sim_database");}
    if d.kind=="pulsar"{params.push_str(";tenant=tenant");}
    ConnectionRequest{kind:DbKind::of(&d.kind),host:"127.0.0.1".into(),port:s.port,user:"sim_user".into(),password:"sim_password".into(),database:d.container,file:String::new(),params,timeout_secs:10}
}
fn page(id:&str,d:&sim_http::Data,offset:u64,limit:u64)->PageRequest{PageRequest{connection_id:id.into(),schema:d.container.clone(),table:d.object.clone(),offset,limit,order_by:None,ascending:true,filters:vec![],filter_logic:FilterLogic::And,order_rules:None}}
fn contains(rows:&[Vec<Option<String>>],text:&str)->bool {
    fn value(v:&Value,text:&str)->bool { match v {
        Value::String(s)=>s==text || (0..4).any(|i|s==&format!("{text}-row{i}")) || serde_json::from_str::<Value>(s).is_ok_and(|v| !v.is_string() && value(&v,text)),
        Value::Array(a)=>a.iter().any(|v|value(v,text)),Value::Object(o)=>o.values().any(|v|value(v,text)),_=>false
    }}
    rows.iter().flatten().filter_map(|v|v.as_ref()).any(|s| value(&Value::String(s.clone()),text))
}
async fn verify(server:&sim_http::Server,steps:&mut Vec<Value>)->Result<Value>{
    let start=server.snapshot();let req=request(server);let reg:db::DbRegistry=parking_lot::RwLock::new(HashMap::new());let id=format!("simulation-http-{}",start.kind);
    db::open_and_register(&reg,&id,&req).await?;steps.push(json!({"step":"actual_db_open_and_register","pass":true}));
    let conn=db::get_db(&reg,&id)?;
    let databases=schema::list_databases(conn.as_ref(),req.kind.clone()).await?;ensure!(databases.iter().any(|(n,_)|n==&start.container),"simulated container absent: {databases:?}");
    let tables=schema::list_tables(conn.as_ref(),req.kind.clone(),&start.container).await?;ensure!(tables.iter().any(|(n,_)|n==&start.object),"simulated object absent: {tables:?}");
    steps.push(json!({"step":"actual_schema_tree","pass":true,"containers":databases.len(),"objects":tables.len()}));
    let full=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&start,0,50)).await?;
    ensure!(!full.columns.is_empty()&&!full.rows.is_empty(),"browse lacks real simulated columns/rows");
    ensure!(contains(&full.rows,&start.sentinel),"browse lost complex dynamically provisioned Unicode sentinel: {:?}",full.rows);
    ensure!(full.rows.iter().all(|row|row.len()==full.columns.len()),"row/column width mismatch");
    let paged=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&start,1,1)).await?;
    let expected=full.rows.iter().skip(1).take(1).cloned().collect::<Vec<_>>();
    ensure!(paged.rows==expected,"offset=1/limit=1 ignored or cursor incorrectly interpreted: actual={:?} expected={expected:?}",paged.rows);
    let empty=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&start,0,0)).await?;ensure!(empty.rows.is_empty(),"limit=0 was ignored");
    steps.push(json!({"step":"actual_browse_columns_rows_offset_limit","pass":true,"rows":full.rows.len(),"columns":full.columns.iter().map(|c|c.name.clone()).collect::<Vec<_>>(),"null_cells":full.rows.iter().flatten().filter(|c|c.is_none()).count()}));
    let sql=sim_http::query(&start);let first=db::execute(&reg,&id,&sql,req.kind.clone()).await?;
    ensure!(first.sets.iter().any(|s|contains(&s.rows,&start.sentinel)),"query lost current complex state: {:?}",first.sets.iter().map(|s|&s.rows).collect::<Vec<_>>());
    // Typed endpoint state is changed, not a fixed canned response. Read-only APIs stay read-only.
    server.change();let changed=server.snapshot();
    let second=db::execute(&reg,&id,&sql,req.kind.clone()).await?;
    ensure!(second.sets.iter().any(|s|contains(&s.rows,&changed.sentinel)),"query did not observe changed typed simulator state");
    let changed_page=schema::page_table(conn.as_ref(),req.kind.clone(),&page(&id,&changed,0,50)).await?;
    ensure!(contains(&changed_page.rows,&changed.sentinel),"browse did not observe changed endpoint state");
    steps.push(json!({"step":"query_and_read_after_typed_mutation","pass":true,"revision_before":start.revision,"revision_after":changed.revision}));
    if start.kind=="dynamodb"||start.kind=="etcd"{
        let payload=format!("写入中文;O'Reilly\\尾-{}",uuid::Uuid::new_v4());
        let command=if start.kind=="dynamodb"{json!({"operation":"PutItem","payload":{"TableName":start.object,"Item":{"id":{"N":"1"},"payload":{"S":payload},"nullable":{"NULL":true}}}})}else{json!({"operation":"put","key":start.object,"value":payload})};
        db::execute(&reg,&id,&command.to_string(),req.kind.clone()).await?;
        let read=db::execute(&reg,&id,&sql,req.kind.clone()).await?;ensure!(read.sets.iter().any(|s|contains(&s.rows,&payload)),"product write failed to change endpoint/read result");
        steps.push(json!({"step":"actual_product_write_readback","pass":true}));
    }
    let mut wrong=req.clone();wrong.password="wrong_sim_password".into();ensure!(db::open(&wrong).await.is_err(),"invalid password accepted");
    ensure!(server.snapshot().actions.iter().any(|a|a["action"]=="rejected_auth"),"no actual 401 rejection occurred");
    steps.push(json!({"step":"invalid_auth_actual_401","pass":true}));
    let illegal=if matches!(start.kind.as_str(),"meilisearch"|"weaviate"|"qdrant"|"chromadb"){"{"}else if matches!(start.kind.as_str(),"dynamodb"|"etcd"){"{\"operation\":\"INVALID\",\"key\":\"/\"}"}else{"INVALID_COMMAND"};
    ensure!(db::execute(&reg,&id,illegal,req.kind.clone()).await.is_err(),"illegal command silently converted to normal browse success");
    server.fault("business");ensure!(db::execute(&reg,&id,&sql,req.kind.clone()).await.is_err(),"HTTP200 business error accepted as success");
    server.fault("malformed");ensure!(db::execute(&reg,&id,&sql,req.kind.clone()).await.is_err(),"HTTP200 malformed response accepted as empty successful rows");
    steps.push(json!({"step":"illegal_command_business_error_malformed_response","pass":true}));
    ensure!(server.snapshot().violations.is_empty(),"wire contract violations: {:?}",server.snapshot().violations);
    drop(conn);drop(reg);
    Ok(json!({"sentinel_exact_unicode_preserved":true,"state_changed_and_read":true,"vendor_server_status":"NOT_TESTED","schema_paging":true}))
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn simulated_http_catalogue_uses_actual_product_routes(){
    let kinds=CATALOG.iter().filter(|m|m.protocol==Protocol::Http||m.protocol==Protocol::Clickhouse||matches!(m.id,"druid"|"dynamodb"|"etcd"|"rqlite")).collect::<Vec<_>>();
    let selected=std::env::var("NAVKIT_SIM_HTTP_KIND").ok();
    let mut rows=vec![];let mut failures=vec![];
    for meta in kinds.iter().filter(|m|selected.as_deref().is_none_or(|s|s==m.id)){
        let server=sim_http::Server::start(meta.id);let mut steps=vec![];let begin=Instant::now();
        let outcome=tokio::time::timeout(Duration::from_secs(90),verify(&server,&mut steps)).await;
        let mut entry=json!({"kind":meta.id,"protocol":format!("{:?}",meta.protocol),"evidence_type":"STATEFUL_LOOPBACK_PROTOCOL_SIMULATION","limitations":["Not a real vendor deployment; no brand/version certification.","Only implemented HTTP APIs and strict fixture dialects are exercised.","MQ management APIs validate management metadata, not broker message delivery."],"steps":steps});
        match outcome{Ok(Ok(details))=>{entry["status"]=json!("SIMULATED_PROTOCOL_PASS");entry["details"]=details;},Ok(Err(e))=>{entry["status"]=json!("SIMULATED_PROTOCOL_FAIL");entry["error"]=json!(format!("{e:#}"));failures.push(meta.id);},Err(_)=>{entry["status"]=json!("SIMULATED_PROTOCOL_FAIL");entry["error"]=json!("90 second serial protocol verification deadline");failures.push(meta.id);}}
        let data=server.snapshot();entry["requests"]=json!(data.requests);entry["actions"]=json!(data.actions);entry["wire_violations"]=json!(data.violations);entry["elapsed_ms"]=json!(begin.elapsed().as_millis());entry["cleanup"]=json!({"loopback_listener":"RAII joined worker; no child process or external service","owned_port":server.port});
        println!("{}: {} {}",meta.id,entry["status"],entry["error"]);rows.push(entry);drop(server);
    }
    ensure_report(&rows,&failures);
    assert!(failures.is_empty(),"actual product routes failed stateful HTTP protocol simulations: {failures:?}");
}
fn ensure_report(rows:&[Value],failures:&[&str]){
    let path=std::env::var("NAVKIT_SIM_HTTP_REPORT").unwrap_or_else(|_|"../tools/acceptance/simulation/http/report.json".into());
    let path=std::path::PathBuf::from(path);if let Some(p)=path.parent(){std::fs::create_dir_all(p).unwrap();}
    let doc=json!({"schema_version":1,"evidence_type":"STATEFUL_PROTOCOL_SIMULATION_NOT_VENDOR_CERTIFICATION","counts":{"covered":rows.len(),"pass":rows.len()-failures.len(),"fail":failures.len()},"rows":rows});
    std::fs::write(path,serde_json::to_vec_pretty(&doc).unwrap()).unwrap();
}
