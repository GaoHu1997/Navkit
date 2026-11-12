//! 协议级fixture不是实服务PASS；实服务验收必须显式运行ignored测试。
use super::*;
use crate::models::DbKind;
use serde_json::{json,Value};
use tokio::{net::TcpListener,io::{AsyncReadExt,AsyncWriteExt}};
fn req(kind:&str)->ConnectionRequest{ConnectionRequest{kind:DbKind::of(kind),host:"127.0.0.1".into(),port:0,user:String::new(),password:String::new(),database:String::new(),file:String::new(),params:String::new(),timeout_secs:2}}
async fn mock<F>(n:usize,handler:F)->(String,tokio::task::JoinHandle<()>)where F:Fn(usize,&str,Value)->(u16,Value)+Send+'static{
 let l=TcpListener::bind("127.0.0.1:0").await.unwrap();let url=format!("http://{}",l.local_addr().unwrap());let t=tokio::spawn(async move{for i in 0..n{let(mut s,_)=l.accept().await.unwrap();let mut b=vec![];let end=loop{let mut chunk=[0;4096];let got=s.read(&mut chunk).await.unwrap();assert!(got>0);b.extend_from_slice(&chunk[..got]);if let Some(p)=b.windows(4).position(|x|x==b"\r\n\r\n"){break p+4;}};let headers=String::from_utf8(b[..end].to_vec()).unwrap();let length=headers.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("content-length:").and_then(|s|s.trim().parse::<usize>().ok())).unwrap_or(0);while b.len()<end+length{let mut chunk=[0;4096];let got=s.read(&mut chunk).await.unwrap();assert!(got>0);b.extend_from_slice(&chunk[..got]);}let body:Value=serde_json::from_slice(&b[end..end+length]).unwrap();let(code,out)=handler(i,&headers,body);let text=out.to_string();s.write_all(format!("HTTP/1.1 {code} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",text.len()).as_bytes()).await.unwrap();}});(url,t)
}
#[test]fn all_ten_route_native_not_odbc(){for k in ["druid","h2","dynamodb","cassandra","nebulagraph","kafka","rocketmq","mqtt","zookeeper","etcd"]{let k=DbKind::of(k);assert_eq!(k.protocol(),crate::models::Protocol::Native);assert!(!k.is_planned());}assert_eq!(DbKind::of("kwdb").protocol(),crate::models::Protocol::Postgres);assert_eq!(DbKind::of("rqlite").default_port(),4001);}
#[tokio::test]async fn closed_ports_are_errors_not_fake_success(){let l=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=l.local_addr().unwrap().port();drop(l);for k in ["druid","dynamodb","cassandra","nebulagraph","kafka","rocketmq","mqtt","zookeeper","etcd"]{let mut r=req(k);r.port=port;r.user="bad".into();r.password="bad".into();r.params="ssl=false".into();assert!(connect(&r).await.is_err(),"{k} must not silently succeed");}}
// Public self-signed CA for protocol fixtures only; no fixture private key is shipped.
const NATIVE_TLS_FIXTURE_CA: &str = "-----BEGIN CERTIFICATE-----\nMIIBaDCCAQygAwIBAgIEfVCD2TAMBggqhkjOPQQDAgUAMB8xHTAbBgNVBAMTFE5h\ndktpdCBMb2NhbCBUZXN0IENBMCAXDTI2MTExMDAwMjEyOVoYDzIxMjYxMDE3MDAy\nMTI5WjAfMR0wGwYDVQQDExROYXZLaXQgTG9jYWwgVGVzdCBDQTBZMBMGByqGSM49\nAgEGCCqGSM49AwEHA0IABJ4XdtK3kc3aQEaDWeHirIBFMSv21EXuk5NTaT5RXWlp\nAAsGS/eL2TqIBdzkWXrJUlsjOfdVekoeQOwMtsnEDEyjMjAwMB0GA1UdDgQWBBSM\nfRclzrRCeU4wE5naLB+xcBsayjAPBgNVHRMBAf8EBTADAQH/MAwGCCqGSM49BAMC\nBQADSAAwRQIgbbyBoh485dP5/dwdeQPlgxwyvcuAMe5/D+UHGixRDx4CIQDKeDAZ\nycM6i8DFqvSxRwKnHqBcwKD7Xvvv2cD7InjyxQ==\n-----END CERTIFICATE-----\n";
struct NativeTlsFixtureCa(std::path::PathBuf);
impl NativeTlsFixtureCa {
 fn new() -> Self {
  let path=std::env::temp_dir().join(format!("navkit-native-tls-ca-{}.pem",uuid::Uuid::new_v4()));
  std::fs::write(&path,NATIVE_TLS_FIXTURE_CA).unwrap();Self(path)
 }
}
impl Drop for NativeTlsFixtureCa { fn drop(&mut self) { let _=std::fs::remove_file(&self.0); } }
#[tokio::test]
async fn native_tls_refuses_plaintext_downgrade() {
 let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
 for kind in ["cassandra","nebulagraph","rocketmq","zookeeper"] {
  let mut request=req(kind);request.port=listener.local_addr().unwrap().port();request.params="ssl=true".into();
  let error=tokio::time::timeout(Duration::from_secs(2),connect(&request)).await.unwrap().err().expect("unsupported TLS must reject before dial").to_string();
  assert!(error.contains("拒绝降级"),"{kind}: {error}");
 }
 assert!(tokio::time::timeout(Duration::from_millis(50),listener.accept()).await.is_err(),"unsupported TLS must send zero sockets");
}
#[test]
fn unsupported_native_tls_aliases_and_material_never_become_plaintext() {
 for parameters in ["tls_mode=verify_full","security.protocol=SSL","ssl_ca=missing.pem","ssl=false;ssl_ca=missing.pem","tls=invalid","ssl=false;tls=true"] {
  let mut request=req("cassandra");request.params=parameters.into();
  assert!(Params::new(&request).no_tls("Cassandra").is_err(),"must reject {parameters}");
 }
}
#[tokio::test]
async fn kafka_and_mqtt_send_tls_clienthello_without_plaintext_fallback() {
 let ca=NativeTlsFixtureCa::new();
 for kind in ["kafka","mqtt"] {
  let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();
  let (stop,mut stopped)=tokio::sync::oneshot::channel::<()>();
  let server=tokio::spawn(async move {
   let mut records=Vec::new();
   loop { tokio::select! {
    _=&mut stopped=>return records,
    accepted=listener.accept()=>{
     let (mut socket,_)=accepted.unwrap();let mut first=[0u8;6];
     tokio::time::timeout(Duration::from_secs(2),socket.read_exact(&mut first)).await.unwrap().unwrap();
     records.push(first);assert!(records.len()<=32,"bounded TLS retry fixture");
     // Fatal handshake_failure: the client must never retry this peer in plaintext.
     let _=socket.write_all(&[21,3,3,0,2,2,40]).await;
    }
   }}
  });
  let mut request=req(kind);request.port=port;request.timeout_secs=1;request.user="native-tls-no-plaintext".into();request.password="fixture-only".into();
  request.params=json!({"ssl":true,"ssl_ca":ca.0.to_string_lossy()}).to_string();
  let result=tokio::time::timeout(Duration::from_secs(5),connect(&request)).await;
  let _=stop.send(());let records=tokio::time::timeout(Duration::from_secs(3),server).await.unwrap().unwrap();
  assert!(result.unwrap().is_err(),"{kind}: a rejected TLS handshake cannot create a healthy session");
  assert!(!records.is_empty(),"{kind}: TLS must be attempted on the actual socket");
  for first in records {assert_eq!((first[0],first[1],first[5]),(22,3,1),"{kind}: every connection must start with a TLS ClientHello, got {first:?}");assert!(u16::from_be_bytes([first[3],first[4]])>=4);}
 }
}
#[tokio::test]async fn druid_sql_auth_directory_utf8_null_and_paging(){let(url,t)=mock(4,|i,h,b|{assert!(h.starts_with("POST /druid/v2/sql "));assert!(h.to_lowercase().contains("authorization: basic dXNlcjpwYXNz".to_lowercase().as_str()));assert_eq!(b["header"],true);match i{0=>{assert_eq!(b["query"],"SELECT 1");(200,json!([["one"],[1]]))},1=>(200,json!([["TABLE_SCHEMA"],["druid"]])),2=>(200,json!([["TABLE_NAME"],["sentinel"]])),_=>{assert!(b["query"].as_str().unwrap().contains("LIMIT 2 OFFSET 1"));(200,json!([["键","空"],["值;尾",null]]))}}}).await;let mut r=req("druid");r.params=json!({"url":url}).to_string();r.user="user".into();r.password="pass".into();let db=connect(&r).await.unwrap();assert_eq!(db.containers().await.unwrap()[0].0,"druid");assert_eq!(db.objects("druid").await.unwrap(),vec!["sentinel"]);let rs=db.rows("druid","sentinel",1,2).await.unwrap();assert_eq!(rs.rows[0],vec![Some("值;尾".into()),None]);t.await.unwrap();}
#[tokio::test]async fn http_bad_credentials_and_server_errors_rejected(){for k in ["druid","dynamodb","etcd"]{let(url,t)=mock(1,|_,_,_|(401,json!({"error":"denied"}))).await;let mut r=req(k);r.params=json!({"url":url}).to_string();r.user="bad".into();r.password="bad".into();let e=connect(&r).await.err().unwrap().to_string();assert!(e.contains("401"),"{k}: {e}");t.await.unwrap();}}
#[tokio::test]async fn etcd_gateway_auth_full_keyspace_and_semicolon_payload(){use base64::Engine;let enc=|s:&str|base64::engine::general_purpose::STANDARD.encode(s);let(url,t)=mock(5,move|i,h,b|{if i==0{assert!(h.starts_with("POST /v3/auth/authenticate"));assert_eq!(b["name"],"user");return(200,json!({"token":"fixture-token"}));}assert!(h.to_lowercase().contains("authorization: fixture-token"));match i{1=>(200,json!({"version":"3.5","header":{"revision":"1"}})),2=>{assert_eq!(b["key"],"AA==");assert_eq!(b["range_end"],"AA==");(200,json!({"header":{"revision":"2"},"kvs":[{"key":enc("/中文"),"value":enc("值;尾") }]}))},3=>{assert_eq!(b["value"],enc("值;尾"));(200,json!({"header":{"revision":"3"}}))},_=>{assert!(h.starts_with("POST /v3/kv/range"));(200,json!({"header":{"revision":"3"},"kvs":[{"key":enc("/中文"),"value":enc("值;尾") }]}))}}}).await;let mut r=req("etcd");r.params=json!({"url":url}).to_string();r.user="user".into();r.password="pass".into();let db=std::sync::Arc::new(crate::db::open(&r).await.unwrap());let registry=crate::db::DbRegistry::default();registry.write().insert("test".into(),db.clone());let h=match db.as_ref(){crate::db::Db::Http(h)=>h,_=>panic!("wrong route")};assert_eq!(h.objects("/").await.unwrap(),vec!["/中文"]);crate::db::execute(&registry,"test",r#"{"operation":"put","key":"/中文","value":"值;尾"}"#,r.kind.clone()).await.unwrap();let rs=h.run("/","",r#"{"operation":"get","key":"/中文"}"#).await.unwrap();assert_eq!(rs.rows[0][1].as_deref(),Some("值;尾"));t.await.unwrap();}
#[tokio::test]async fn dynamodb_sigv4_continuation_scan_and_partiql(){let(url,t)=mock(6,|i,h,b|{let h=h.to_lowercase();assert!(h.contains("aws4-hmac-sha256 credential=access/"));assert!(h.contains("signedheaders=content-type;host;x-amz-date;x-amz-target"));assert!(h.contains("signature="));match i{0=>(200,json!({"TableNames":["sentinel"]})),1=>(200,json!({"TableNames":["sentinel"],"LastEvaluatedTableName":"sentinel"})),2=>{assert_eq!(b["ExclusiveStartTableName"],"sentinel");(200,json!({"TableNames":["other"]}))},3=>{assert!(h.contains("dynamodb_20120810.scan"));(200,json!({"Items":[{"id":{"S":"a"}}],"LastEvaluatedKey":{"id":{"S":"a"}}}))},4=>{assert_eq!(b["ExclusiveStartKey"]["id"]["S"],"a");(200,json!({"Items":[{"id":{"S":"中文;尾"},"nothing":{"NULL":true}}]}))},_=>{assert_eq!(b["Statement"],"SELECT * FROM sentinel WHERE id='中文;尾'");(200,json!({"Items":[{"id":{"S":"中文;尾"}}]}))}}}).await;let mut r=req("dynamodb");r.user="access".into();r.password="secret".into();r.params=json!({"url":url,"region":"us-east-1"}).to_string();let db=std::sync::Arc::new(crate::db::open(&r).await.unwrap());let registry=crate::db::DbRegistry::default();registry.write().insert("test".into(),db.clone());let h=match db.as_ref(){crate::db::Db::Http(h)=>h,_=>panic!("wrong route")};assert_eq!(h.objects("us-east-1").await.unwrap(),vec!["sentinel","other"]);let rs=h.rows("us-east-1","sentinel",1,1).await.unwrap();assert!(rs.rows[0].iter().flatten().any(|s|s.contains("中文;尾")));let rs=crate::db::execute(&registry,"test","SELECT * FROM sentinel WHERE id='中文;尾'",r.kind.clone()).await.unwrap();assert_eq!(rs.sets.len(),1);t.await.unwrap();}
#[tokio::test]async fn rqlite_server_sql_error_propagates(){let(url,t)=mock(2,|i,_,_|if i==0{(200,json!({"results":[{"columns":["1"],"values":[[1]]}]}))}else{(200,json!({"results":[{"error":"syntax error"}]}))}).await;let mut r=req("rqlite");r.params=json!({"url":url}).to_string();r.timeout_secs=5;let db=connect(&r).await.unwrap();let error=db.run("main","","BOGUS SQL").await.err().expect("rqlite SQL failure must propagate").to_string();assert!(error.starts_with("rqlite SQL:")&&error.contains("syntax error"),"{error}");t.await.unwrap();}
#[tokio::test]#[ignore="requires real Java11+ and NAVKIT_H2_JAR; no mock"]async fn real_h2_jdbc_schema_browse_transaction_auth(){let mut r=req("h2");r.timeout_secs=20;r.user="sa".into();r.password="correct-pass".into();let dir=std::env::temp_dir().join(format!("navkit-h2-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir).unwrap();r.file=dir.join("sentinel").to_string_lossy().into_owned();if let Ok(java)=std::env::var("NAVKIT_JAVA"){r.params=json!({"java":java}).to_string();}let db=connect(&r).await.expect("real H2 JDBC required; set NAVKIT_H2_JAR and NAVKIT_JAVA");db.run("","","CREATE SCHEMA NAVKIT").await.unwrap();db.run("","","CREATE TABLE NAVKIT.SENTINEL(ID INT PRIMARY KEY, TEXT VARCHAR(100), NOTHING VARCHAR(100))").await.unwrap();db.run("","","INSERT INTO NAVKIT.SENTINEL VALUES (1,'中文;尾',NULL),(2,'second',NULL)").await.unwrap();assert!(db.containers().await.unwrap().iter().any(|(s,_)|s=="NAVKIT"));assert!(db.objects("NAVKIT").await.unwrap().iter().any(|s|s=="SENTINEL"));let rs=db.rows("NAVKIT","SENTINEL",0,1).await.unwrap();assert_eq!(rs.rows[0],vec![Some("1".into()),Some("中文;尾".into()),None]);let rs=db.rows("NAVKIT","SENTINEL",1,1).await.unwrap();assert_eq!(rs.rows.len(),1);db.run("","","SET AUTOCOMMIT FALSE").await.unwrap();db.run("","","UPDATE NAVKIT.SENTINEL SET TEXT='changed' WHERE ID=1").await.unwrap();db.run("","","ROLLBACK").await.unwrap();let rs=db.run("","","SELECT TEXT FROM NAVKIT.SENTINEL WHERE ID=1").await.unwrap();assert_eq!(rs.rows[0][0].as_deref(),Some("中文;尾"));assert!(db.run("","","SELECT * FROM MISSING_TABLE").await.is_err());db.run("","","SHUTDOWN").await.unwrap();drop(db);r.password="wrong-pass".into();let error=connect(&r).await.err().expect("bad password must fail").to_string();assert!(error.contains("28000"),"{error}");std::fs::remove_dir_all(dir).unwrap();}
#[tokio::test]
async fn malformed_json_is_not_ignored_as_plaintext_defaults() {
 for kind in ["druid","dynamodb","cassandra","nebulagraph","kafka","rocketmq","mqtt","zookeeper","etcd"] {
  for params in [r#"{"ssl":true"#,r#"["tls",true]"#] {
   let mut r=req(kind);r.params=params.into();
   let error=connect(&r).await.err().expect("malformed JSON must be rejected before transport").to_string();
   assert!(error.contains("JSON"),"{kind}: {error}");
  }
 }
}
/// 用户配置包含各原生服务的ConnectionRequest、container、object、query、expected substring。
/// 无配置明确失败，不将未验证服务伪装为PASS。
#[tokio::test]#[ignore="requires real service fixtures; NAVKIT_NATIVE_FIXTURES path"]async fn real_native_services(){let path=std::env::var("NAVKIT_NATIVE_FIXTURES").expect("NAVKIT_NATIVE_FIXTURES must name a configured JSON array");let fixtures:Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();for f in fixtures.as_array().expect("fixture array"){let r:ConnectionRequest=serde_json::from_value(f.get("request").unwrap_or(&f["connection"]).clone()).unwrap();let db=connect(&r).await.unwrap_or_else(|e|panic!("{} real connect failed: {e:#}",r.kind.id()));let c=f["container"].as_str().unwrap();let o=f["object"].as_str().unwrap();let directories=db.containers().await.unwrap();assert!(directories.iter().any(|(name,_)|name==c),"{} missing real container {c}",r.kind.id());assert!(db.objects(c).await.unwrap().iter().any(|name|name==o),"{} missing real object {o}",r.kind.id());let browsed=db.rows(c,o,0,20).await.unwrap();assert!(!browsed.rows.is_empty(),"{} needs seeded browse records",r.kind.id());let q=db.run(c,o,f["query"].as_str().unwrap()).await.unwrap();let expected=f["expected"].as_str().unwrap();assert!(q.rows.iter().flatten().flatten().any(|s|s.contains(expected)),"{} missing sentinel {expected}",r.kind.id());eprintln!("REAL SERVICE PASS {}",r.kind.id());}}

#[test]fn params_semicolon_ampersand_json_and_tls(){for text in ["ssl=TRUE;foo=bar","ssl=true&foo=bar",r#"{"ssl":true,"foo":"bar"}"#,"ssl-mode=verify-full","sslmode=required","tls=unknown-mode"]{let mut r=req("kafka");r.params=text.into();let p=Params::new(&r);assert!(p.no_tls("Kafka").is_err(),"must not downgrade {text}");}let mut r=req("kafka");r.params="ssl=FALSE;foo=bar".into();assert!(!Params::new(&r).bool("ssl",true));assert_eq!(Params::new(&r).get("foo"),Some("bar"));assert!(Params::new(&r).no_tls("Kafka").is_ok());}
#[tokio::test]async fn vendor_empty_json_is_not_healthy(){for k in ["etcd","dynamodb"]{let(url,t)=mock(1,|_,_,_|(200,json!({}))).await;let mut r=req(k);r.user="access".into();r.password="secret".into();r.params=json!({"url":url,"token":"token"}).to_string();assert!(connect(&r).await.is_err(),"{k} empty JSON must fail");t.await.unwrap();}}
#[tokio::test]async fn etcd_explicit_token_is_sent(){let(url,t)=mock(1,|_,h,_|{assert!(h.to_lowercase().contains("authorization: explicit-token"));(200,json!({"version":"3.5","header":{"revision":"1"}}))}).await;let mut r=req("etcd");r.params=json!({"url":url,"token":"explicit-token"}).to_string();connect(&r).await.unwrap();t.await.unwrap();}