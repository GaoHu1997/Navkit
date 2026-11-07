//! HTTP contract fixtures, NOT real product/service acceptance.
use super::*;
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

struct Reply { path: &'static str, status: u16, body: String, contains: Vec<String> }
fn reply(path: &'static str, body: Value) -> Reply { Reply { path, status:200, body:body.to_string(), contains:vec![] } }
async fn server(replies: Vec<Reply>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = tokio::spawn(async move {
        for expected in replies {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(5), listener.accept()).await.unwrap().unwrap();
            let mut data = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let n = stream.read(&mut buffer).await.unwrap();
                assert!(n > 0, "request ended before body"); data.extend_from_slice(&buffer[..n]);
                assert!(data.len() < 65536);
                if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..end]).to_ascii_lowercase();
                    let size = headers.lines().find_map(|line| line.strip_prefix("content-length:").and_then(|s| s.trim().parse::<usize>().ok())).unwrap_or(0);
                    if data.len() >= end + 4 + size { break; }
                }
            }
            let request = String::from_utf8(data).unwrap();
            if !expected.path.is_empty() { assert!(request.lines().next().unwrap().contains(expected.path), "{request}"); }
            for text in &expected.contains { assert!(request.to_ascii_lowercase().contains(&text.to_ascii_lowercase()), "missing {text:?} in {request}"); }
            let response = format!("HTTP/1.1 {} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", expected.status, expected.body.len(), expected.body);
            stream.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, worker)
}
fn ctx(base: String, user: &str, pass: &str) -> Ctx {
    Ctx { http:reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(3)).build().unwrap(), base,user:user.into(),pass:pass.into(),params:vec![],headers:Default::default() }
}
fn req(kind: &str, url: &str) -> ConnectionRequest {
    ConnectionRequest {kind:DbKind::of(kind),host:"127.0.0.1".into(),port:0,user:String::new(),password:String::new(),database:String::new(),file:String::new(),params:format!("url={url}"),timeout_secs:3}
}
#[test]
fn basic_password_is_not_a_bearer_token() {
    let context = ctx("http://localhost".into(), "user", "secret");
    let request = context.request(reqwest::Method::GET, "/").build().unwrap();
    assert_eq!(request.headers()[reqwest::header::AUTHORIZATION], "Basic dXNlcjpzZWNyZXQ=");
}
#[test]
fn per_adapter_authorization_is_never_overwritten() {
    let mut context = ctx("http://localhost".into(), "user", "secret");
    context.params.push(("token".into(),"explicit".into()));
    let context = with_token_header(context, "api-key", "");
    let request = context.request(reqwest::Method::GET, "/").build().unwrap();
    assert_eq!(request.headers()["api-key"], "explicit");
    assert!(!request.headers().contains_key(reqwest::header::AUTHORIZATION));
    let context = with_milvus_auth(ctx("http://localhost".into(), "user", "secret"));
    let request = context.request(reqwest::Method::GET, "/").build().unwrap();
    assert_eq!(request.headers()[reqwest::header::AUTHORIZATION], "Bearer user:secret");
    let context = with_bearer(ctx("http://localhost".into(), "", "secret"));
    let request = context.request(reqwest::Method::GET, "/").build().unwrap();
    assert_eq!(request.headers()[reqwest::header::AUTHORIZATION], "Bearer secret");
}
#[test]
fn utf8_error_summary_and_html_rejection() {
    let text = "中文连接错误🚫".repeat(100);
    let summary = jbody(&Value::String(text));
    assert_eq!(summary.chars().count(),301);
    assert!(summary.ends_with('…'));
    assert!(Ctx::check(200, &Value::String("<!DOCTYPE html><html>login</html>".into()), "ping").is_err());
    assert!(Ctx::check(401, &serde_json::json!({"error":"bad credentials"}), "ping").is_err());
}
#[tokio::test]
async fn tdengine_sends_raw_sql_not_a_quoted_json_string() {
    let mut r = reply("/rest/sql", serde_json::json!({"code":0,"column_meta":[["v","VARCHAR",20]],"data":[["中文;sentinel"]]}));
    r.contains = vec!["authorization: Basic dXNlcjpzZWNyZXQ=".into(),"\r\n\r\nSELECT '中文;sentinel'".into()];
    let (base, worker) = server(vec![r]).await;
    let adapter = TdengineAdapter {ctx:ctx(base,"user","secret")};
    let result = adapter.sql("", "SELECT '中文;sentinel'").await.unwrap();
    assert_eq!(result.columns,vec!["v"]); assert_eq!(result.rows[0][0].as_deref(),Some("中文;sentinel"));
    worker.await.unwrap();
}
#[tokio::test]
async fn tdengine_business_failure_is_not_an_empty_success() {
    for body in [serde_json::json!({"code":9731,"desc":"bad SQL"}),serde_json::json!({"unexpected":"not TDengine"})] {
        let (base, worker) = server(vec![reply("/rest/sql",body)]).await;
        let adapter = TdengineAdapter {ctx:ctx(base,"","")};
        assert!(adapter.sql("", "invalid SQL").await.is_err()); worker.await.unwrap();
    }
}
#[tokio::test]
async fn iotdb_v2_timestamps_show_and_aggregation_are_transposed() {
    let replies = vec![
        reply("/rest/v2/query",serde_json::json!({"expressions":["root.d.s"],"timestamps":[1,2],"values":[["中文",null]]})),
        reply("/rest/v2/query",serde_json::json!({"column_names":["Database","SchemaReplicationFactor"],"values":[["root.test"],[1]]})),
        reply("/rest/v2/query",serde_json::json!({"expressions":["count(root.d.s)"],"values":[[2]]})),
    ];
    let (base, worker) = server(replies).await;
    let adapter = IotdbAdapter {ctx:with_header_auth(ctx(base,"user","secret"))};
    let a = adapter.query("SELECT s FROM root.d").await.unwrap();
    assert_eq!(a.columns,vec!["Time","root.d.s"]); assert_eq!(a.rows[0],vec![Some("1".into()),Some("中文".into())]); assert_eq!(a.rows[1][1],None);
    let b = adapter.containers().await.unwrap(); assert_eq!(b,vec![("root.test".into(),"root.test".into())]);
    let c = adapter.query("SELECT count(s) FROM root.d").await.unwrap(); assert_eq!(c.columns,vec!["count(root.d.s)"]); assert_eq!(c.rows,vec![vec![Some("2".into())]]);
    worker.await.unwrap();
}
#[tokio::test]
async fn iotdb_uses_named_timeseries_column_and_device_parent() {
    let mut browse = reply("/rest/v2/query",serde_json::json!({"expressions":["root.d.s"],"timestamps":[1],"values":[[42]]}));
    browse.contains.push("SELECT s FROM root.d LIMIT 50 OFFSET 0".into());
    let (base, worker) = server(vec![
        reply("/rest/v2/query",serde_json::json!({"column_names":["Timeseries","Alias","Database"],"values":[["root.d.s"],[null],["root.d"]]})),browse,
    ]).await;
    let adapter = IotdbAdapter {ctx:ctx(base,"","")};
    assert_eq!(adapter.objects("root.d").await.unwrap(),vec!["root.d.s"]);
    assert_eq!(adapter.rows("root.d","root.d.s",0,50).await.unwrap().rows[0][1].as_deref(),Some("42")); worker.await.unwrap();
}
#[tokio::test]
async fn iotdb_http_200_business_errors_still_fail() {
    let (base, worker) = server(vec![reply("/rest/v2/query",serde_json::json!({"code":301,"message":"wrong password"})),reply("/rest/v2/nonQuery",serde_json::json!({"code":500,"message":"bad SQL"}))]).await;
    let adapter = IotdbAdapter {ctx:ctx(base,"","")};
    assert!(adapter.query("SHOW DATABASES").await.is_err()); assert!(adapter.run("","","INSERT invalid").await.is_err()); worker.await.unwrap();
}
#[tokio::test]
async fn http_catalogue_rejects_html_instead_of_fake_connection_success() {
    for kind in ["elasticsearch","easysearch","solr","couchdb","meilisearch","qdrant","milvus","weaviate","chromadb","neo4j","tdengine","iotdb","influxdb","victoriametrics","turso","pulsar","rabbitmq","nacos","consul"] {
        let (base, worker) = server(vec![Reply{path:"",status:200,body:"<!DOCTYPE html><html>wrong service</html>".into(),contains:vec![]}]).await;
        assert!(HttpDb::connect(DbKind::of(kind), &req(kind,&base)).await.is_err(), "{kind} accepted an HTML page as a working database");
        worker.await.unwrap();
    }
}
#[tokio::test]
async fn influx_connection_verifies_query_credentials_before_ping() {
    let (base, worker) = server(vec![Reply{path:"/query?",status:401,body:"{\"error\":\"unauthorized\"}".into(),contains:vec!["authorization: Token test-token".into()]}]).await;
    let adapter = InfluxAdapter {ctx:with_token_header(ctx(base,"","test-token"),"authorization","Token ")};
    assert!(adapter.ping().await.is_err()); worker.await.unwrap();
}
#[tokio::test]
async fn influx_dbrp_token_and_response_shape_are_explicit() {
    for body in [serde_json::json!({}), serde_json::json!({"results":null})] {
        let (base, worker) = server(vec![reply("/query?", body)]).await;
        let adapter = InfluxAdapter { ctx:ctx(base,"","") };
        assert!(adapter.influxql("", "SHOW DATABASES").await.is_err());
        worker.await.unwrap();
    }
    let mut response = reply("/query?", serde_json::json!({"results":[{"statement_id":0}]}));
    response.contains = vec!["authorization: Token test-token".into(), "&db=db%20name&rp=retention%20policy".into()];
    let (base, worker) = server(vec![response]).await;
    let mut context = ctx(base,"","test-token");
    context.params.push(("rp".into(), "retention policy".into()));
    let adapter = InfluxAdapter { ctx:with_token_header(context,"authorization","Token ") };
    let result = adapter.influxql("db name", "SHOW MEASUREMENTS").await.unwrap();
    assert!(result.rows.is_empty());
    worker.await.unwrap();
}
#[tokio::test]
async fn public_health_endpoints_do_not_mask_data_access_failures() {
    for (kind, path) in [("milvus","/v2/vectordb/databases/list"),("qdrant","/collections"),("meilisearch","/indexes?"),("couchdb","/_all_dbs"),("weaviate","/v1/schema")] {
        let (base, worker) = server(vec![Reply{path,status:401,body:"{\"error\":\"denied\"}".into(),contains:vec![]}]).await;
        assert!(HttpDb::connect(DbKind::of(kind), &req(kind,&base)).await.is_err(), "{kind} bypassed data-access authentication");
        worker.await.unwrap();
        let (base, worker) = server(vec![reply(path,serde_json::json!({}))]).await;
        assert!(HttpDb::connect(DbKind::of(kind), &req(kind,&base)).await.is_err(), "{kind} accepted an empty unrelated JSON object");
        worker.await.unwrap();
    }
}
#[tokio::test]
async fn milvus_business_errors_and_valid_empty_catalogues_are_distinct() {
    let (base, worker) = server(vec![
        reply("/v2/vectordb/databases/list",serde_json::json!({"code":1800,"message":"authentication failed"})),
        reply("/v2/vectordb/databases/list",serde_json::json!({"code":0,"data":[]})),
        reply("/v2/vectordb/collections/list",serde_json::json!({"code":1800,"data":[]})),
    ]).await;
    let adapter = MilvusAdapter{ctx:ctx(base,"user","bad-password")};
    assert!(adapter.ping().await.is_err());
    assert!(adapter.containers().await.unwrap().is_empty(), "must not invent a default database");
    assert!(adapter.objects("default").await.is_err());
    worker.await.unwrap();
}

#[tokio::test]
async fn victoriametrics_scalar_vector_and_paging_are_real_values() {
    let sample = serde_json::json!({"status":"success","data":{"resultType":"vector","result":[
        {"metric":{"__name__":"metric","label":"中文;sentinel"},"value":[1,"42"]},
        {"metric":{"__name__":"metric","label":"second"},"value":[2,"43"]}
    ]}});
    let (base, worker) = server(vec![
        reply("/api/v1/query?",serde_json::json!({"status":"success","data":{"resultType":"scalar","result":[3,"1"]}})),
        reply("/api/v1/query?",sample),
        reply("/api/v1/label/__name__/values",serde_json::json!({"status":"success","data":[]})),
    ]).await;
    let adapter = VmAdapter {ctx:ctx(base,"","")};
    assert_eq!(adapter.instant("1").await.unwrap().rows,vec![vec![None,Some("1".into()),Some("3".into())]]);
    let rows = adapter.rows("metric","samples",1,1).await.unwrap();
    assert_eq!(rows.rows.len(),1);
    assert_eq!(rows.rows[0][1].as_deref(),Some("43"));
    assert!(adapter.containers().await.unwrap().is_empty());
    worker.await.unwrap();
}

#[test]
fn consul_kv_decodes_exact_text_binary_and_null_and_rejects_bad_shapes() {
    use base64::Engine;
    let text = "中文;O'Reilly\\尾🙂";
    let binary = vec![0, 255, 128, 10];
    let entries = serde_json::json!([
        {"Key":"text","Value":base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),"ModifyIndex":1},
        {"Key":"binary","Value":base64::engine::general_purpose::STANDARD.encode(&binary),"ModifyIndex":2},
        {"Key":"null","Value":null,"ModifyIndex":3}
    ]);
    let result = ConsulAdapter::kv_rows(&entries,0,100).unwrap();
    assert_eq!(result.rows[0][1].as_deref(),Some(text));
    assert_eq!(result.rows[1][1],Some(crate::native_db::text_bytes(&binary)));
    assert_eq!(result.rows[2][1],None);
    assert_eq!(ConsulAdapter::kv_rows(&entries,1,1).unwrap().rows,vec![result.rows[1].clone()]);
    assert!(ConsulAdapter::kv_rows(&entries,0,0).unwrap().rows.is_empty());
    for malformed in [serde_json::json!({}),serde_json::json!([{}]),serde_json::json!([{"Key":"bad","Value":"!bad-base64"}])] {
        assert!(ConsulAdapter::kv_rows(&malformed,0,10).is_err());
    }
}

#[tokio::test]
async fn consul_kv_api_query_returns_decoded_rows_instead_of_empty_object_columns() {
    use base64::Engine;
    let text = "中文;O'Reilly\\尾🙂";
    let (base,worker) = server(vec![reply("/v1/kv/prefix?recurse=true",serde_json::json!([
        {"Key":"prefix/item","Value":base64::engine::general_purpose::STANDARD.encode(text.as_bytes()),"ModifyIndex":1}
    ]))]).await;
    let adapter = ConsulAdapter {ctx:ctx(base,"","")};
    let result = adapter.run("","","/v1/kv/prefix?recurse=true").await.unwrap();
    assert_eq!(result.columns,vec!["key","value","modify_index"]);
    assert_eq!(result.rows[0][1].as_deref(),Some(text));
    worker.await.unwrap();
}

#[tokio::test]
async fn victoriametrics_health_does_not_bypass_permissions_or_bad_data() {
    let (base, worker) = server(vec![Reply {path:"/api/v1/query?",status:401,body:"{\"error\":\"denied\"}".into(),contains:vec![]}]).await;
    assert!(VmAdapter {ctx:ctx(base,"","bad")}.ping().await.is_err());
    worker.await.unwrap();
    for body in [serde_json::json!({"status":"success"}),serde_json::json!({"status":"success","data":{"resultType":"vector","result":[{"metric":{},"value":[1]}]}})] {
        let (base, worker) = server(vec![reply("/api/v1/query?",body)]).await;
        assert!(VmAdapter {ctx:ctx(base,"","")}.instant("metric").await.is_err());
        worker.await.unwrap();
    }
    for body in [serde_json::json!({}),serde_json::json!({"status":"error","data":[]}),serde_json::json!({"status":"success","data":[3]})] {
        let (base, worker) = server(vec![reply("/api/v1/label/__name__/values",body)]).await;
        assert!(VmAdapter {ctx:ctx(base,"","")}.containers().await.is_err());
        worker.await.unwrap();
    }
}
