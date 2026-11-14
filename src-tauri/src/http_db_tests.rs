//! HTTP contract fixtures, NOT real product/service acceptance.
use super::*;
use async_trait::async_trait;
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

/// 极简 offset 游标适配器，用于验证默认 export_page 语义。
/// 共 4 行、每页最多 2 行：offset 0→2 行、2→2 行（短页）、4→0 行（EOF）。
struct OffsetAdapter;

#[async_trait]
impl HttpAdapter for OffsetAdapter {
    async fn ping(&self) -> Result<String> { Ok("ok".into()) }
    async fn containers(&self) -> Result<Vec<(String, String)>> { Ok(Vec::new()) }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> { Ok(Vec::new()) }
    async fn rows(&self, _c: &str, _o: &str, offset: u64, limit: u64) -> Result<HttpRows> {
        let remaining = 4usize.saturating_sub(offset as usize);
        let n = remaining.min(2).min(limit as usize);
        Ok(HttpRows {
            columns: vec!["v".into()],
            rows: (0..n).map(|i| vec![Some((offset as usize + i).to_string())]).collect(),
        })
    }
    async fn run(&self, _c: &str, _o: &str, _sql: &str) -> Result<HttpRows> {
        Ok(HttpRows { columns: Vec::new(), rows: Vec::new() })
    }
}

#[tokio::test]
async fn default_export_page_is_offset_cursor_and_short_page_is_not_eof() {
    let a = OffsetAdapter;
    let p1 = a.export_page("c", "o", None, 10).await.unwrap();
    assert_eq!(p1.data.rows.len(), 2);
    assert_eq!(p1.next.as_deref(), Some("2"));
    // 第 2 页仍只有 2 行（短页），不得当作 EOF：必须继续。
    let p2 = a.export_page("c", "o", p1.next.as_deref(), 10).await.unwrap();
    assert_eq!(p2.data.rows.len(), 2);
    assert_eq!(p2.next.as_deref(), Some("4"));
    // 只有空页才是结束。
    let p3 = a.export_page("c", "o", p2.next.as_deref(), 10).await.unwrap();
    assert_eq!(p3.data.rows.len(), 0);
    assert_eq!(p3.next, None);
    assert!(a.export_page("c", "o", Some("bad-token"), 10).await.is_err());
}

#[tokio::test]
async fn qdrant_export_page_uses_opaque_next_page_offset_and_fixed_columns() {
    let (base, worker) = server(vec![
        reply("/points/scroll", serde_json::json!({"result":{"points":[
            {"id":42,"payload":{"id":"must-not-override","name":"a"},"vector":[0.1,0.2]},
            {"id":43,"payload":{},"vector":[0.3]}
        ],"next_page_offset":43}})),
        Reply {
            path: "/points/scroll",
            status: 200,
            body: serde_json::json!({"result":{"points":[],"next_page_offset":null}}).to_string(),
            contains: vec!["\"offset\":43".into(), "\"with_vector\":true".into()],
        },
    ]).await;
    let a = QdrantAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("c", "points", None, 100).await.unwrap();
    assert_eq!(p1.data.columns, vec!["id".to_string(), "payload".to_string(), "vector".to_string()]);
    assert_eq!(p1.data.rows.len(), 2);
    // 顶层的 id 是点的真实 id，payload 内的 id 不得覆盖它。
    assert_eq!(p1.data.rows[0][0].as_deref(), Some("42"));
    assert!(p1.data.rows[0][1].as_deref().unwrap().contains("\"name\":\"a\""));
    assert!(p1.data.rows[0][1].as_deref().unwrap().contains("must-not-override"));
    assert!(p1.data.rows[0][2].as_deref().unwrap().contains("0.1"));
    // 游标必须原样沿用 next_page_offset（point id），不能被当作数字 offset。
    assert_eq!(p1.next.as_deref(), Some("43"));
    let p2 = a.export_page("c", "points", p1.next.as_deref(), 100).await.unwrap();
    assert_eq!(p2.data.rows.len(), 0);
    assert_eq!(p2.next, None);
    worker.await.unwrap();
}

#[test]
fn es_export_token_roundtrip_and_rejects_malformed() {
    assert_eq!(parse_es_export_token("120:500").unwrap(), (120, Some(500)));
    assert_eq!(parse_es_export_token("120:-").unwrap(), (120, None));
    assert!(parse_es_export_token("x:1").is_err());
    assert!(parse_es_export_token("1:x").is_err());
    assert!(parse_es_export_token("1").is_err());
}

#[tokio::test]
async fn weaviate_export_page_uses_after_cursor_and_short_page_is_not_eof() {
    let (base, worker) = server(vec![
        reply("/v1/objects", serde_json::json!({"objects":[
            {"id":"a","properties":{"x":1}},
            {"id":"b","properties":{"y":2}}
        ]})),
        Reply {
            path: "/v1/objects",
            status: 200,
            body: serde_json::json!({"objects":[{"id":"c","properties":{"z":3}}]}).to_string(),
            contains: vec!["after=b".into()],
        },
        Reply {
            path: "/v1/objects",
            status: 200,
            body: serde_json::json!({"objects":[]}).to_string(),
            contains: vec!["after=c".into()],
        },
    ]).await;
    let a = WeaviateAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("C", "objects", None, 10).await.unwrap();
    assert_eq!(p1.data.rows.len(), 2);
    assert_eq!(p1.next.as_deref(), Some("b"));
    // 短页（1 < limit）不得当作结束，必须继续用 after 游标。
    let p2 = a.export_page("C", "objects", p1.next.as_deref(), 10).await.unwrap();
    assert_eq!(p2.data.rows.len(), 1);
    assert_eq!(p2.next.as_deref(), Some("c"));
    let p3 = a.export_page("C", "objects", p2.next.as_deref(), 10).await.unwrap();
    assert_eq!(p3.data.rows.len(), 0);
    assert_eq!(p3.next, None);
    worker.await.unwrap();
}

// 本组仅为本机 HTTP 契约夹具，不是 Weaviate 厂商实库验收。
#[tokio::test]
async fn weaviate_export_page_malformed_objects_is_error_not_eof() {
    for body in [
        serde_json::json!({}),
        serde_json::json!({"unexpected":"not a Weaviate page"}),
        serde_json::json!({"objects":null}),
        serde_json::json!({"objects":{}}),
        serde_json::json!({"objects":"[]"}),
        serde_json::json!([]),
    ] {
        // 初始页及 after 续页都不能把缺失或错误形状默认为空数组。
        for token in [None, Some("previous")] {
            let (base, worker) = server(vec![reply("/v1/objects", body.clone())]).await;
            let a = WeaviateAdapter { ctx: ctx(base, "", "") };
            let error = a.export_page("C", "objects", token, 10).await.err()
                .expect("malformed objects must fail, not end export successfully");
            let message = format!("{error}");
            // Ctx::check 已拒绝空对象；其它形状由协议必需字段校验拒绝。
            assert!(message.contains("objects 数组") || (body == serde_json::json!({}) && message.contains("缺失结果结构")), "{message}");
            worker.await.unwrap();
        }
    }
}

#[tokio::test]
async fn weaviate_export_page_validates_every_row_and_id() {
    for (invalid, expected) in [
        (serde_json::json!(null), "不是对象"),
        (serde_json::json!(42), "不是对象"),
        (serde_json::json!([]), "不是对象"),
        (serde_json::json!({"properties":{"x":1}}), "有效对象 id"),
        (serde_json::json!({"id":null}), "有效对象 id"),
        (serde_json::json!({"id":42}), "有效对象 id"),
        (serde_json::json!({"id":""}), "有效对象 id"),
        (serde_json::json!({"id":" \t\n"}), "有效对象 id"),
    ] {
        // 合法末行不能掩盖前面缺 id / 非对象的数据行。
        let body = serde_json::json!({"objects":[invalid,{"id":"last","properties":{}}]});
        let (base, worker) = server(vec![reply("/v1/objects", body)]).await;
        let a = WeaviateAdapter { ctx: ctx(base, "", "") };
        let error = a.export_page("C", "objects", None, 10).await.err()
            .expect("every row must be validated, not just the cursor row");
        let message = format!("{error}");
        assert!(message.contains("第 1 行") && message.contains(expected), "{message}");
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn weaviate_export_page_rejects_malformed_properties_without_silent_loss() {
    for properties in [serde_json::json!([]), serde_json::json!("lost"), serde_json::json!(1), serde_json::json!(true)] {
        let (base, worker) = server(vec![reply("/v1/objects", serde_json::json!({"objects":[
            {"id":"first","properties":properties}, {"id":"last","properties":{}}
        ]}))]).await;
        let a = WeaviateAdapter { ctx: ctx(base, "", "") };
        let error = a.export_page("C", "objects", None, 10).await.err()
            .expect("invalid properties must not disappear from an exported row");
        assert!(format!("{error}").contains("properties 必须是对象或 null"), "{error}");
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn weaviate_export_page_preserves_optional_properties_and_dynamic_values() {
    let (base, worker) = server(vec![reply("/v1/objects", serde_json::json!({"objects":[
        {"id":"a"},
        {"id":"b","properties":null},
        {"id":"c","properties":{"x":1,"nested":{"value":"中文"}}},
        {"id":"d","properties":{"y":null}}
    ]}))]).await;
    let a = WeaviateAdapter { ctx: ctx(base, "", "") };
    let page = a.export_page("C", "objects", None, 10).await.unwrap();
    assert_eq!(page.next.as_deref(), Some("d"));
    let columns: std::collections::BTreeSet<_> = page.data.columns.iter().map(String::as_str).collect();
    assert_eq!(columns, std::collections::BTreeSet::from(["id", "nested", "x", "y"]));
    assert_eq!(page.data.columns.len(), 4);
    assert_eq!(page.data.rows.len(), 4);
    let cell = |row: usize, name: &str| page.data.rows[row][page.data.columns.iter().position(|col| col == name).unwrap()].as_deref();
    for (row, id) in ["a", "b", "c", "d"].iter().enumerate() { assert_eq!(cell(row, "id"), Some(*id)); }
    for row in [0, 1] { for name in ["nested", "x", "y"] { assert_eq!(cell(row, name), None); } }
    assert_eq!(cell(2, "nested"), Some("{\"value\":\"中文\"}"));
    assert_eq!(cell(2, "x"), Some("1"));
    assert_eq!(cell(3, "y"), None);
    worker.await.unwrap();
}

#[tokio::test]
async fn weaviate_export_page_rejects_stalled_after_cursor() {
    let (base, worker) = server(vec![Reply {
        path: "/v1/objects",
        status: 200,
        body: serde_json::json!({"objects":[{"id":"same","properties":{"x":1}}]}).to_string(),
        contains: vec!["after=same".into()],
    }]).await;
    let a = WeaviateAdapter { ctx: ctx(base, "", "") };
    let error = a.export_page("C", "objects", Some("same"), 10).await.err()
        .expect("a repeated last id must not keep exporting the same page");
    assert!(format!("{error}").contains("游标未推进"), "{error}");
    worker.await.unwrap();
}

#[tokio::test]
async fn es_rotated_pit_is_used_for_continuation_and_final_cleanup() {
    let (base, worker) = server(vec![
        reply("/_pit", serde_json::json!({"id":"original"})),
        Reply { path:"/_search", status:200, body:serde_json::json!({"pit_id":"rotated","hits":{"hits":[{"_id":"a","_source":{},"sort":[1]}]}}).to_string(), contains:vec!["\"id\":\"original\"".into()] },
        Reply { path:"/_search", status:200, body:serde_json::json!({"pit_id":"final","hits":{"hits":[]}}).to_string(), contains:vec!["\"id\":\"rotated\"".into(), "\"search_after\":[1]".into()] },
        Reply { path:"/_pit", status:200, body:"{}".into(), contains:vec!["DELETE".into(), "\"id\":\"final\"".into()] },
    ]).await;
    let a = EsAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("i", "idx", None, 100).await.unwrap();
    assert_eq!(es_decode_pit_token(p1.next.as_deref().unwrap()).unwrap().0, "rotated");
    assert!(a.export_page("i", "idx", p1.next.as_deref(), 100).await.unwrap().next.is_none());
    worker.await.unwrap();
}

#[tokio::test]
async fn es_partial_or_malformed_success_cleans_latest_pit_and_fails() {
    for body in [
        serde_json::json!({"pit_id":"latest","timed_out":true,"hits":{"hits":[]}}),
        serde_json::json!({"pit_id":"latest","_shards":{"failed":1},"hits":{"hits":[]}}),
        serde_json::json!({"pit_id":"latest","hits":{}}),
    ] {
        let (base, worker) = server(vec![
            reply("/_pit", serde_json::json!({"id":"original"})),
            reply("/_search", body),
            Reply { path:"/_pit", status:200, body:"{}".into(), contains:vec!["DELETE".into(), "\"id\":\"latest\"".into()] },
        ]).await;
        let a = EsAdapter { ctx: ctx(base, "", "") };
        assert!(a.export_page("i", "idx", None, 100).await.is_err());
        worker.await.unwrap();
    }
}

#[tokio::test]
async fn es_export_page_uses_pit_search_after_and_closes_on_eof() {
    let (base, worker) = server(vec![
        Reply { path: "/_pit", status: 200, body: serde_json::json!({"id":"pit-1"}).to_string(), contains: vec![] },
        Reply {
            path: "/_search",
            status: 200,
            body: serde_json::json!({"hits":{"hits":[
                {"_id":"a","_source":{"x":1},"sort":[10]},
                {"_id":"b","_source":{"x":2},"sort":[20]}
            ]}}).to_string(),
            contains: vec!["\"pit\":{\"id\":\"pit-1\"".into(), "\"_shard_doc\"".into()],
        },
        Reply {
            path: "/_search",
            status: 200,
            body: serde_json::json!({"hits":{"hits":[]}}).to_string(),
            contains: vec!["\"search_after\":[20]".into(), "\"pit-1\"".into()],
        },
        Reply {
            path: "/_pit",
            status: 200,
            body: serde_json::json!({}).to_string(),
            contains: vec!["DELETE".into(), "\"id\":\"pit-1\"".into()],
        },
    ]).await;
    let a = EsAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("i", "idx", None, 2).await.unwrap();
    assert_eq!(p1.data.rows.len(), 2);
    assert_eq!(p1.data.columns, vec!["_id".to_string(), "x".to_string()]);
    let (pit, api, sort) = es_decode_pit_token(p1.next.as_deref().unwrap()).unwrap();
    assert_eq!(pit, "pit-1");
    assert_eq!(api, "es");
    assert_eq!(sort, vec![serde_json::json!(20)]);
    // 空命中页才是 EOF；此时必须自己关闭 PIT，不能让服务端游标泄漏。
    let p2 = a.export_page("i", "idx", p1.next.as_deref(), 2).await.unwrap();
    assert_eq!(p2.data.rows.len(), 0);
    assert_eq!(p2.next, None);
    worker.await.unwrap();
}

#[tokio::test]
async fn es_export_page_limit_zero_does_not_open_pit() {
    let a = EsAdapter { ctx: ctx("http://127.0.0.1:1".into(), "", "") };
    let probe = a.export_page("i", "idx", None, 0).await.unwrap();
    assert_eq!(probe.data.rows.len(), 0);
    assert_eq!(probe.next, None);
}

#[tokio::test]
async fn es_open_pit_falls_back_to_opensearch_endpoint() {
    let (base, worker) = server(vec![
        Reply { path: "/_pit", status: 400, body: serde_json::json!({"error":"unsupported"}).to_string(), contains: vec![] },
        Reply { path: "/point_in_time", status: 200, body: serde_json::json!({"pit_id":"os-1"}).to_string(), contains: vec![] },
        Reply {
            path: "/_search",
            status: 200,
            body: serde_json::json!({"hits":{"hits":[]}}).to_string(),
            contains: vec!["\"pit\":{\"id\":\"os-1\"".into()],
        },
        Reply {
            path: "/_search/point_in_time",
            status: 200,
            body: serde_json::json!({}).to_string(),
            contains: vec!["DELETE".into(), "\"pit_id\":[\"os-1\"]".into()],
        },
    ]).await;
    let a = EsAdapter { ctx: ctx(base, "", "") };
    let p = a.export_page("i", "idx", None, 2).await.unwrap();
    assert_eq!(p.data.rows.len(), 0);
    assert_eq!(p.next, None);
    worker.await.unwrap();
}

#[tokio::test]
async fn es_export_page_cleans_pit_when_search_fails() {
    let (base, worker) = server(vec![
        Reply { path: "/_pit", status: 200, body: serde_json::json!({"id":"pit-fail"}).to_string(), contains: vec![] },
        Reply { path: "/_search", status: 500, body: serde_json::json!({"error":"boom"}).to_string(), contains: vec!["\"pit-fail\"".into()] },
        Reply {
            path: "/_pit",
            status: 200,
            body: serde_json::json!({}).to_string(),
            contains: vec!["DELETE".into(), "\"id\":\"pit-fail\"".into()],
        },
    ]).await;
    let a = EsAdapter { ctx: ctx(base, "", "") };
    let err = a.export_page("i", "idx", None, 2).await.err().expect("500 必须失败");
    assert!(format!("{err}").contains("500"), "{err}");
    worker.await.unwrap();
}

#[tokio::test]
async fn es_finish_export_closes_token_pit() {
    let (base, worker) = server(vec![
        Reply { path: "/_pit", status: 200, body: serde_json::json!({"id":"pit-finish"}).to_string(), contains: vec![] },
        Reply {
            path: "/_search",
            status: 200,
            body: serde_json::json!({"hits":{"hits":[{"_id":"a","_source":{},"sort":[7]}]}}).to_string(),
            contains: vec!["\"pit-finish\"".into()],
        },
        Reply {
            path: "/_pit",
            status: 200,
            body: serde_json::json!({}).to_string(),
            contains: vec!["DELETE".into(), "\"id\":\"pit-finish\"".into()],
        },
    ]).await;
    let a = EsAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("i", "idx", None, 2).await.unwrap();
    a.finish_export(p1.next.as_deref()).await.unwrap();
    worker.await.unwrap();
}

#[tokio::test]
async fn es_export_page_search_after_beyond_10000_without_from_size_window() {
    let (base, worker) = server(vec![
        Reply { path: "/_pit", status: 200, body: serde_json::json!({"id":"pit-deep"}).to_string(), contains: vec![] },
        Reply {
            path: "/_search",
            status: 200,
            body: serde_json::json!({"hits":{"hits":[{"_id":"deep","_source":{"x":1},"sort":[10001]}]}}).to_string(),
            contains: vec!["\"pit-deep\"".into()],
        },
        Reply {
            path: "/_search",
            status: 200,
            body: serde_json::json!({"hits":{"hits":[]}}).to_string(),
            contains: vec!["\"search_after\":[10001]".into()],
        },
        Reply {
            path: "/_pit",
            status: 200,
            body: serde_json::json!({}).to_string(),
            contains: vec!["DELETE".into()],
        },
    ]).await;
    let a = EsAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("i", "idx", None, 2).await.unwrap();
    assert_eq!(p1.data.rows.len(), 1);
    let p2 = a.export_page("i", "idx", p1.next.as_deref(), 2).await.unwrap();
    assert_eq!(p2.data.rows.len(), 0);
    assert_eq!(p2.next, None);
    worker.await.unwrap();
}

#[tokio::test]
async fn export_response_text_is_bounded_inside_scope() {
    let (base, worker) = server(vec![
        reply("/", Value::String("0123456789abcdef".into())),
        reply("/", Value::String("0123456789abcdef".into())),
    ])
    .await;
    let context = ctx(base, "", "");
    // 直接调用有界 helper：4 字节上限必须报错，而不是把整段读进内存。
    let resp = context
        .request(reqwest::Method::GET, "/")
        .send()
        .await
        .unwrap();
    let err = read_response_text_limited(resp, 4)
        .await
        .err()
        .expect("超过 4 字节必须报错");
    assert!(format!("{err}").contains('4'), "{err}");
    // 导出作用域外的普通请求仍是无界语义；作用域内走同一条有界路径。
    let resp = context
        .request(reqwest::Method::GET, "/")
        .send()
        .await
        .unwrap();
    let err = EXPORT_MAX_BYTES
        .scope(4, async { read_response_text(resp).await })
        .await
        .err()
        .expect("导出作用域内超过 4 字节必须报错");
    assert!(format!("{err}").contains('4'), "{err}");
    worker.await.unwrap();
}

#[test]
fn local_scan_limit_rejects_large_collection() {
    assert!(reject_local_scan("demo", EXPORT_LOCAL_SCAN_MAX_ROWS).is_ok());
    let err = reject_local_scan("demo", EXPORT_LOCAL_SCAN_MAX_ROWS + 1)
        .expect_err("超过本地扫描上限必须报错");
    assert!(format!("{err}").contains("本地全量抓取"), "{err}");
}

#[tokio::test]
async fn meili_export_page_uses_documents_and_total_not_search_max_total_hits() {
    let (base, worker) = server(vec![
        Reply {
            path: "/documents",
            status: 200,
            body: serde_json::json!({"results":[{"id":1},{"id":2}],"offset":0,"limit":2,"total":3}).to_string(),
            contains: vec!["offset=0".into(), "limit=2".into()],
        },
        Reply {
            path: "/documents",
            status: 200,
            body: serde_json::json!({"results":[{"id":3}],"offset":2,"limit":2,"total":3}).to_string(),
            contains: vec!["offset=2".into(), "limit=2".into()],
        },
    ])
    .await;
    let a = MeiliAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("i", "documents", None, 2).await.unwrap();
    assert_eq!(p1.data.rows.len(), 2);
    assert_eq!(p1.next.as_deref(), Some("2:3"));
    let p2 = a.export_page("i", "documents", p1.next.as_deref(), 2).await.unwrap();
    assert_eq!(p2.data.rows.len(), 1);
    assert_eq!(p2.next, None);
    worker.await.unwrap();
}

#[tokio::test]
async fn solr_export_page_sorts_by_unique_key_and_uses_num_found() {
    let (base, worker) = server(vec![
        reply("/schema/uniquekey", serde_json::json!({"uniqueKey":"id"})),
        Reply {
            path: "/select",
            status: 200,
            body: serde_json::json!({"response":{"numFound":2,"docs":[{"id":"a"},{"id":"b"}]}}).to_string(),
            contains: vec!["sort=id%20asc".into(), "start=0".into()],
        },
    ])
    .await;
    let a = SolrAdapter { ctx: ctx(base, "", "") };
    let p1 = a.export_page("core", "docs", None, 2).await.unwrap();
    assert_eq!(p1.data.rows.len(), 2);
    assert_eq!(p1.next, None);
    worker.await.unwrap();
}
