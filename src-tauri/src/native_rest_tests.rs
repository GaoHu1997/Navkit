//! Stateful wire-contract verification, NOT Salesforce/HBase vendor deployments.
//! Actual product db::open/metadata/page/execute; no adapter stub and no canned success.
use crate::{db,models::*,schema};
use base64::{engine::general_purpose::{STANDARD,URL_SAFE},Engine};
use serde_json::{json,Value};
use std::{collections::{BTreeMap,HashMap},io::{Read,Write},net::{TcpListener,TcpStream},sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}},thread,time::Duration};
#[derive(Clone)]
struct State {
    kind:String,token:String,sentinel:String,records:BTreeMap<String,Value>,cells:BTreeMap<Vec<u8>,BTreeMap<Vec<u8>,Vec<u8>>>,
    cursors:HashMap<String,Vec<Value>>,scanners:HashMap<String,Vec<Value>>,fault:String,actions:Vec<Value>,serial:u64,violations:Vec<String>,
}
struct Server{port:u16,state:Arc<Mutex<State>>,stop:Arc<AtomicBool>,worker:Option<thread::JoinHandle<()>>}
struct Wire{method:String,path:String,headers:HashMap<String,String>,body:Value}
struct Reply{code:u16,body:Vec<u8>,headers:Vec<(String,String)>}
fn reply(code:u16,value:Value)->Reply{Reply{code,body:serde_json::to_vec(&value).unwrap(),headers:vec![]}}
fn empty(code:u16)->Reply{Reply{code,body:Vec::new(),headers:vec![]}}
impl Server {
    fn start(kind:&str)->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();let port=listener.local_addr().unwrap().port();
        let sentinel=format!("中文;O'Reilly\\尾🙂-{}",uuid::Uuid::new_v4());let token=format!("synthetic-{}",uuid::Uuid::new_v4());
        let records=(0..3).map(|n|(format!("00100000000000{n}"),json!({"Id":format!("00100000000000{n}"),"Name":format!("{sentinel}-{n}"),"Nullable":null}))).collect();
        let cells=(0..3).map(|n|(format!("row-{n}").into_bytes(),BTreeMap::from([(b"cf:payload".to_vec(),format!("{sentinel}-{n}").into_bytes()),(b"cf:binary".to_vec(),vec![0,255,n])]))).collect();
        let state=Arc::new(Mutex::new(State{kind:kind.into(),token,sentinel,records,cells,cursors:HashMap::new(),scanners:HashMap::new(),fault:String::new(),actions:Vec::new(),serial:0,violations:Vec::new()}));
        let stop=Arc::new(AtomicBool::new(false));let shared=state.clone();let done=stop.clone();
        let worker=thread::spawn(move||while !done.load(Ordering::SeqCst){match listener.accept(){Ok((mut stream,_))=>{stream.set_nonblocking(false).unwrap();stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();let request=read_wire(&mut stream);let response=match request{Ok(w)=>handle(&mut shared.lock().unwrap(),w),Err(e)=>{eprintln!("REST fixture wire read error: {e}");shared.lock().unwrap().violations.push(e);reply(400,json!({"error":"wire violation"}))}};let mut head=format!("HTTP/1.1 {} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",response.code,response.body.len());for(k,v)in response.headers{head.push_str(&format!("{k}: {v}\r\n"));}head.push_str("\r\n");let _=stream.write_all(head.as_bytes());let _=stream.write_all(&response.body);},Err(e)if e.kind()==std::io::ErrorKind::WouldBlock=>thread::park_timeout(Duration::from_millis(2)),Err(e)=>{shared.lock().unwrap().violations.push(e.to_string());break;}}});
        Self{port,state,stop,worker:Some(worker)}
    }
    fn req(&self)->ConnectionRequest{let state=self.state.lock().unwrap();ConnectionRequest{kind:DbKind::of(&state.kind),host:"127.0.0.1".into(),port:self.port,user:if state.kind=="salesforce"{"token".into()}else{"basic-user".into()},password:state.token.clone(),database:if state.kind=="salesforce"{"v59.0".into()}else{"default".into()},file:String::new(),params:"ssl=false".into(),timeout_secs:3}}
    fn fault(&self,fault:&str){self.state.lock().unwrap().fault=fault.into();}
    fn state(&self)->State{self.state.lock().unwrap().clone()}
    fn evidence(&self,steps:Vec<&str>){let state=self.state();assert!(state.violations.is_empty(),"{:?}",state.violations);assert!(state.scanners.is_empty(),"server scanners leaked");if let Ok(dir)=std::env::var("NAVKIT_NATIVE_REST_EVIDENCE"){std::fs::create_dir_all(&dir).unwrap();let path=std::path::Path::new(&dir).join(format!("{}.json",state.kind));let file=std::fs::OpenOptions::new().create_new(true).write(true).open(path).unwrap();serde_json::to_writer_pretty(file,&json!({"kind":state.kind,"status":"SIMULATED_PROTOCOL_PASS","evidence_type":"STATEFUL_WIRE_SIMULATION_ACTUAL_PRODUCT_ADAPTER","vendor_server_status":"UNVERIFIED","steps":steps,"actions":state.actions,"wire_violations":state.violations,"open_scanners":state.scanners.len(),"cleanup":"RAII closes owned listener and joins thread; no child processes","sentinel":state.sentinel})).unwrap();}}
}
impl Drop for Server{fn drop(&mut self){self.stop.store(true,Ordering::SeqCst);if let Some(t)=self.worker.take(){t.thread().unpark();t.join().unwrap();}}}
fn read_wire(stream:&mut TcpStream)->Result<Wire,String>{let mut data=Vec::new();let end=loop{let mut block=[0;4096];let n=stream.read(&mut block).map_err(|e|e.to_string())?;if n==0{return Err("unexpected request EOF".into());}data.extend_from_slice(&block[..n]);if data.len()>1024*1024{return Err("request too large".into());}if let Some(i)=data.windows(4).position(|w|w==b"\r\n\r\n"){break i+4;}};let header=std::str::from_utf8(&data[..end]).map_err(|e|e.to_string())?;let mut lines=header.lines();let mut first=lines.next().unwrap().split_whitespace();let method=first.next().ok_or("method")?.to_string();let path=first.next().ok_or("path")?.to_string();let headers:HashMap<String,String>=lines.filter_map(|line|line.split_once(':')).map(|(k,v)|(k.to_ascii_lowercase(),v.trim().into())).collect();let len:usize=headers.get("content-length").map(|s|s.parse().unwrap()).unwrap_or(0);while data.len()<end+len{let mut block=[0;4096];let n=stream.read(&mut block).map_err(|e|e.to_string())?;if n==0{return Err("body EOF".into());}data.extend_from_slice(&block[..n]);}let body=if len==0{Value::Null}else{serde_json::from_slice(&data[end..end+len]).map_err(|e|e.to_string())?};Ok(Wire{method,path,headers,body})}
fn handle(s:&mut State,w:Wire)->Reply{
    s.actions.push(json!({"method":w.method,"path":w.path}));let expected=if s.kind=="salesforce"{format!("Bearer {}",s.token)}else{format!("Basic {}",STANDARD.encode(format!("basic-user:{}",s.token)))};
    if w.headers.get("authorization")!=Some(&expected){s.actions.push(json!({"rejected_auth":true}));return reply(401,json!({"error":"invalid credentials"}));}
    let url=reqwest::Url::parse(&format!("http://127.0.0.1{}",w.path)).unwrap();let parts:Vec<_>=url.path_segments().unwrap().collect();
    if s.kind=="salesforce"{salesforce(s,&w,&url,&parts)}else{hbase(s,&w,&url,&parts)}
}
fn fault(s:&mut State)->Option<Reply>{let f=std::mem::take(&mut s.fault);match f.as_str(){"malformed"=>Some(reply(200,json!({}))),"invalid-json"=>Some(Reply{code:200,body:b"<html>not JSON</html>".to_vec(),headers:vec![]}),"business"=>Some(reply(400,json!({"error":"server rejected dynamic request"}))),"redirect"=>Some(Reply{code:302,body:vec![],headers:vec![("Location".into(),"http://127.0.0.1:1/stolen".into())]}),""=>None,_=>{s.fault=f;None}}}
fn sf_page(s:&mut State,items:Vec<Value>,total:usize)->Reply{if items.len()<=1{return reply(200,json!({"totalSize":total,"done":true,"records":items}));}s.serial+=1;let id=format!("cursor-{}",s.serial);let first=items[..1].to_vec();s.cursors.insert(id.clone(),items[1..].to_vec());let next=if s.fault=="cross-origin"{s.fault.clear();"http://127.0.0.1:1/stolen".into()}else{format!("/services/data/v59.0/query/{id}")};reply(200,json!({"totalSize":total,"done":false,"records":first,"nextRecordsUrl":next}))}
fn salesforce(s:&mut State,w:&Wire,url:&reqwest::Url,p:&[&str])->Reply{
    if p.len()<4||p[..3]!=["services","data","v59.0"]{return reply(404,json!({"error":"unknown API"}));}
    if let Some(r)=fault(s){return r;}
    match(p[3],w.method.as_str()){
        ("sobjects","GET")if p.len()==4=>reply(200,json!({"sobjects":[{"name":"Account","queryable":true}]})),
        ("sobjects","GET")if p==["services","data","v59.0","sobjects","Account","describe"]=>reply(200,json!({"name":"Account","fields":[{"name":"Id","type":"id","nillable":false},{"name":"Name","type":"string","nillable":false},{"name":"Nullable","type":"string","nillable":true}]})),
        ("query","GET")=>{
            if p.len()==5{if let Some(items)=s.cursors.remove(p[4]){let total=items.len()+1;return sf_page(s,items,total);}return reply(404,json!({"error":"unknown cursor"}));}
            let query=url.query_pairs().find(|(k,_)|k=="q").map(|(_,v)|v.into_owned()).unwrap_or_default();
            if !query.starts_with("SELECT ")||!query.contains(" FROM Account"){return reply(400,json!({"errorCode":"MALFORMED_QUERY"}));}
            let number=|key:&str,default:usize|query.split_once(key).and_then(|(_,tail)|tail.split_whitespace().next()).and_then(|n|n.parse().ok()).unwrap_or(default);
            let items:Vec<_>=s.records.values().skip(number(" OFFSET ",0)).take(number(" LIMIT ",10000)).cloned().collect();let total=items.len();sf_page(s,items,total)
        },
        ("sobjects","POST")if p==["services","data","v59.0","sobjects","Account"]=>{s.serial+=1;let id=format!("001000000{:06}",s.serial+100);let mut value=w.body.clone();value["Id"]=json!(id);s.records.insert(id.clone(),value);reply(201,json!({"id":id,"success":true,"errors":[]}))},
        ("sobjects","PATCH"|"DELETE")if p.len()==6&&p[4]=="Account"=>{if w.method=="DELETE"{if s.records.remove(p[5]).is_some(){empty(204)}else{reply(404,json!({"error":"missing record"}))}}else if let Some(row)=s.records.get_mut(p[5]){for(k,v)in w.body.as_object().unwrap(){row[k]=v.clone();}empty(204)}else{reply(404,json!({"error":"missing record"}))}},
        _=>reply(404,json!({"error":"unknown object or method"}))
    }
}
fn cell_row(key:&[u8],cells:&BTreeMap<Vec<u8>,Vec<u8>>)->Value{json!({"key":STANDARD.encode(key),"Cell":cells.iter().map(|(c,v)|json!({"column":STANDARD.encode(c),"$":STANDARD.encode(v),"timestamp":1234})).collect::<Vec<_>>()})}
fn hbase(s:&mut State,w:&Wire,url:&reqwest::Url,p:&[&str])->Reply{
    // Read faults do not affect DELETE: scanner cleanup must still reach the server.
    if w.method!="DELETE"&&!(p.len()==2&&p[1]=="schema")&&!(p.len()==2&&p[1]=="scanner"){if let Some(r)=fault(s){return r;}}
    if w.method=="GET"&&p==["version","cluster"]{return reply(200,json!({"Version":"2.6.3-fixture"}));}
    if w.method=="GET"&&p==["namespaces"]{return reply(200,json!({"Namespace":["default","other"]}));}
    if w.method=="GET"&&p==["namespaces","default","tables"]{return reply(200,json!({"table":[{"name":"sentinel"}]}));}
    if p.first()!=Some(&"sentinel"){return reply(404,json!({"error":"missing table"}));}
    if w.method=="GET"&&p==["sentinel","schema"]{return reply(200,json!({"name":"sentinel","ColumnSchema":[{"name":"cf"}]}));}
    if w.method=="PUT"&&p==["sentinel","scanner"]{
        if w.body["maxVersions"]!=1||w.body["batch"]!=1000{s.violations.push("incorrect scanner model".into());}
        s.serial+=1;let id=format!("scanner-{}",s.serial);let mut pages=Vec::new();for(key,cells)in &s.cells{ // Deliberately split every row across multiple cell batches.
            for(column,value)in cells{pages.push(cell_row(key,&BTreeMap::from([(column.clone(),value.clone())])));}
        }s.scanners.insert(id.clone(),pages);return Reply{code:201,body:vec![],headers:vec![("Location".into(),format!("/sentinel/scanner/{id}"))]};
    }
    if p.len()==3&&p[1]=="scanner"{
        if w.method=="DELETE"{s.scanners.remove(p[2]);s.actions.push(json!({"scanner_deleted":p[2]}));return empty(200);}
        if w.method=="GET"{if s.fault=="bad-base64"{s.fault.clear();return reply(200,json!({"Row":[{"key":"!bad","Cell":[]}]}));}if s.fault=="expired"{s.fault.clear();return reply(410,json!({"error":"expired scanner"}));}
            if let Some(pages)=s.scanners.get_mut(p[2]){if pages.is_empty(){return empty(204);}let page=pages.remove(0);return reply(200,json!({"Row":[page]}));}return reply(404,json!({"error":"missing scanner"}));}
    }
    if p.len()==2&&url.query_pairs().any(|(k,v)|k=="keyencoding"&&v=="b64"){
        let encoded=p[1].replace("%3D","=").replace("%3d","=");let key=match URL_SAFE.decode(encoded){Ok(k)=>k,Err(_)=>return reply(400,json!({"error":"bad row key"}))};
        return match w.method.as_str(){
            "GET"=>if let Some(cells)=s.cells.get(&key){reply(200,json!({"Row":[cell_row(&key,cells)]}))}else{reply(404,json!({"error":"missing row"}))},
            "DELETE"=>{s.cells.remove(&key);empty(200)},
            "PUT"=>{let row=&w.body["Row"][0];if row["key"]!=STANDARD.encode(&key){s.violations.push("body/path row mismatch".into());return reply(400,json!({"error":"row mismatch"}));}let target=s.cells.entry(key).or_default();for cell in row["Cell"].as_array().unwrap(){target.insert(STANDARD.decode(cell["column"].as_str().unwrap()).unwrap(),STANDARD.decode(cell["$"].as_str().unwrap()).unwrap());}empty(200)},
            _=>reply(400,json!({"error":"unsupported method"}))
        };
    }
    reply(404,json!({"error":"unknown route"}))
}
fn page(kind:&str,offset:u64,limit:u64)->PageRequest{PageRequest{connection_id:"rest".into(),schema:if kind=="salesforce"{"v59.0".into()}else{"default".into()},table:if kind=="salesforce"{"Account".into()}else{"sentinel".into()},offset,limit,order_by:None,ascending:true,filters:vec![],filter_logic:FilterLogic::And,order_rules:None}}
async fn execute(reg:&db::DbRegistry,req:&ConnectionRequest,value:Value)->MultiResult{db::execute(reg,"rest",&value.to_string(),req.kind.clone()).await.unwrap()}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn salesforce_actual_product_stateful_rest_contract(){
    let server=Server::start("salesforce");let req=server.req();let reg=db::DbRegistry::default();db::open_and_register(&reg,"rest",&req).await.unwrap();let conn=db::get_db(&reg,"rest").unwrap();
    assert_eq!(schema::list_databases(&conn,req.kind.clone()).await.unwrap()[0].0,"v59.0");assert_eq!(schema::list_tables(&conn,req.kind.clone(),"v59.0").await.unwrap()[0].0,"Account");let meta=schema::table_schema(&conn,req.kind.clone(),"v59.0","Account").await.unwrap();assert_eq!(meta.columns.len(),3);
    let full=schema::page_table(&conn,req.kind.clone(),&page("salesforce",0,20)).await.unwrap();assert_eq!(full.rows.len(),3);assert!(full.rows[0].iter().flatten().any(|v|v.contains(&server.state().sentinel)));assert!(full.rows.iter().all(|r|r[2].is_none()));
    let one=schema::page_table(&conn,req.kind.clone(),&page("salesforce",1,1)).await.unwrap();assert_eq!(one.rows,full.rows[1..2]);let zero=schema::page_table(&conn,req.kind.clone(),&page("salesforce",0,0)).await.unwrap();assert!(zero.rows.is_empty());assert_eq!(zero.columns.len(),3);
    let text=format!("新值;中文'\\🙂{}",uuid::Uuid::new_v4());let created=execute(&reg,&req,json!({"op":"create","object":"Account","values":{"Name":text,"Nullable":null}})).await;let id=created.sets[0].rows[0][0].clone().unwrap();assert_eq!(id.len(),15);
    let updated=format!("更新-{text}");execute(&reg,&req,json!({"op":"update","object":"Account","id":id,"values":{"Name":updated}})).await;let rows=db::execute(&reg,"rest","SELECT Id,Name,Nullable FROM Account",req.kind.clone()).await.unwrap();assert!(rows.sets[0].rows.iter().flatten().flatten().any(|v|v==&updated));
    execute(&reg,&req,json!({"op":"delete","object":"Account","id":id})).await;assert_eq!(server.state().records.len(),3);
    let mut wrong=req.clone();wrong.password="wrong".into();assert!(db::open(&wrong).await.is_err());assert!(db::execute(&reg,"rest","INVALID SOQL",req.kind.clone()).await.is_err());assert!(db::execute(&reg,"rest",r#"{"op":"create","object":"Account","values":{},"ignored":true}"#,req.kind.clone()).await.is_err());
    for fault in ["business","malformed","invalid-json","redirect","cross-origin"]{server.fault(fault);assert!(db::execute(&reg,"rest","SELECT Id,Name FROM Account",req.kind.clone()).await.is_err(),"{fault} must not be successful empty rows");}
    let mut bad=page("salesforce",2001,1);assert!(schema::page_table(&conn,req.kind.clone(),&bad).await.is_err());bad.schema="v60.0".into();assert!(schema::page_table(&conn,req.kind.clone(),&bad).await.is_err());
    assert!(server.state().actions.iter().any(|a|a["path"].as_str().unwrap_or("").contains("query/cursor-")));
    server.evidence(vec!["actual_db_open_bearer_auth","schema_tree_describe","multi_cursor_pages_unicode_null","offset_limit_zero","create_update_delete_readback","bad_auth_malformed_business_redirect_cross_origin_rejected","legacy_bindings_preserved"]);
}
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn hbase_actual_product_stateful_stargate_contract(){
    let server=Server::start("hbase");let req=server.req();let reg=db::DbRegistry::default();db::open_and_register(&reg,"rest",&req).await.unwrap();let conn=db::get_db(&reg,"rest").unwrap();assert_eq!(schema::list_databases(&conn,req.kind.clone()).await.unwrap().len(),2);assert_eq!(schema::list_tables(&conn,req.kind.clone(),"default").await.unwrap()[0].0,"sentinel");let meta=schema::table_schema(&conn,req.kind.clone(),"default","sentinel").await.unwrap();assert_eq!(meta.columns.len(),2);
    let full=schema::page_table(&conn,req.kind.clone(),&page("hbase",0,20)).await.unwrap();assert_eq!(full.rows.len(),3);assert_eq!(full.columns.len(),3);assert!(full.rows[0].iter().flatten().any(|v|v.contains(&server.state().sentinel)));assert!(full.rows[0].iter().flatten().any(|v|v.starts_with("base64:")));
    let one=schema::page_table(&conn,req.kind.clone(),&page("hbase",1,1)).await.unwrap();assert_eq!(one.rows,full.rows[1..2]);assert!(server.state().scanners.is_empty());let zero=schema::page_table(&conn,req.kind.clone(),&page("hbase",0,0)).await.unwrap();assert!(zero.rows.is_empty());assert_eq!(zero.columns[0].name,"row");
    let key=vec![255,0,b'/',b'*',b',',b'?'];let text=format!("二进制写入;O'Reilly\\🙂{}",uuid::Uuid::new_v4());execute(&reg,&req,json!({"op":"put","table":"sentinel","row_base64":STANDARD.encode(&key),"cells":[{"column":"cf:payload","value":text},{"column":"cf:binary","value_base64":"AP8="}]})).await;
    let get=execute(&reg,&req,json!({"op":"get","table":"sentinel","row_base64":STANDARD.encode(&key)})).await;assert!(get.sets[0].rows[0].iter().flatten().any(|v|v==&text));assert!(get.sets[0].rows[0].iter().flatten().any(|v|v=="base64:AP8="));
    let scanned=schema::page_table(&conn,req.kind.clone(),&page("hbase",0,20)).await.unwrap();assert_eq!(scanned.rows.len(),4);assert!(scanned.rows.iter().any(|row|row[0].as_deref()==Some(&format!("base64:{}",STANDARD.encode(&key)))));
    execute(&reg,&req,json!({"op":"delete","table":"sentinel","row_base64":STANDARD.encode(&key)})).await;let missing=execute(&reg,&req,json!({"op":"get","table":"sentinel","row_base64":STANDARD.encode(&key)})).await;assert!(missing.sets[0].rows.is_empty());
    // A genuine absent qualifier must remain NULL, not empty string or a shifted column.
    execute(&reg,&req,json!({"op":"put","table":"sentinel","row":"zzz-null","cells":[{"column":"cf:payload","value":"only-one"}]})).await;let nulls=schema::page_table(&conn,req.kind.clone(),&page("hbase",0,20)).await.unwrap();assert!(nulls.rows.iter().any(|r|r[0].as_deref()==Some("zzz-null")&&r.iter().any(Option::is_none)));
    let mut wrong=req.clone();wrong.password="wrong".into();assert!(db::open(&wrong).await.is_err());assert!(db::execute(&reg,"rest","SELECT * FROM sentinel",req.kind.clone()).await.is_err());assert!(db::execute(&reg,"rest",r#"{"op":"INVALID","table":"sentinel"}"#,req.kind.clone()).await.is_err());
    for f in ["business","malformed","invalid-json","bad-base64","expired"]{server.fault(f);assert!(schema::page_table(&conn,req.kind.clone(),&page("hbase",0,10)).await.is_err(),"{f} must propagate");assert!(server.state().scanners.is_empty(),"{f} leaked scanner");}
    assert!(server.state().actions.iter().filter(|a|a.get("scanner_deleted").is_some()).count()>=9);
    server.evidence(vec!["actual_db_open_basic_auth","version_namespace_table_column_families","real_scanner_put_get_delete","split_cell_batches_merged","offset_limit_zero_binary_null","put_get_delete_readback","malformed_expired_scanner_cleanup","bad_auth_unknown_op_rejected"]);
}
#[tokio::test]
async fn strict_rest_identity_tls_parameters_and_legacy_routes(){
    for kind in ["salesforce","hbase"]{let server=Server::start(kind);let base=server.req();for params in ["ssl=false;tls=true","ssl_verify=false","ssl=false;unknown=true","{\"ssl\":false", "[1]","ssl=false;ssl=false","url=http://u:p@127.0.0.1:1","url=http://127.0.0.1:1/path","url=http://example.com;ssl=false"] {let mut r=base.clone();r.params=params.into();assert!(db::open(&r).await.is_err(),"must reject {kind} {params}");}assert!(server.state().actions.is_empty());}
    for value in ["@navkit-driver:binding","@navkit-driver-version:binding","DRIVER={vendor};Server=127.0.0.1","dsn=test"]{assert!(db::explicit_legacy_binding(value));}for value in ["url=https://host","{\"url\":\"https://host\"}","ssl=false"]{assert!(!db::explicit_legacy_binding(value));}
    let server=Server::start("salesforce");let mut req=server.req();req.params="@navkit-driver:malformed".into();assert!(db::open(&req).await.is_err());assert!(server.state().actions.is_empty(),"explicit binding must not silently hit REST");
}
