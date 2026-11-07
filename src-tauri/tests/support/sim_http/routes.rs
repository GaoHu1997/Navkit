use super::*;
use sha2::{Digest,Sha256};
use hmac::{Hmac,Mac};

pub fn coordinates(kind:&str,name:&str)->(String,String){
    match kind{
        "elasticsearch"|"easysearch"=>("cluster".into(),name.into()),
        "solr"=>(name.into(),"docs".into()),"couchdb"|"meilisearch"=>(name.into(),"documents".into()),
        "qdrant"=>(name.into(),"points".into()),"weaviate"=>(name.into(),"objects".into()),"chromadb"=>(name.into(),"items".into()),
        "victoriametrics"=>(name.into(),"samples".into()),"turso"=>("default".into(),name.into()),"d1"=>("database".into(),name.into()),
        "pulsar"=>(format!("tenant/{name}"),format!("{name}:persistent")),"rabbitmq"=>(name.into(),format!("queue:{name}")),
        "nacos"=>(name.into(),format!("svc:{name}")),"consul"=>("dc_sim".into(),format!("dir:{name}")),
        "iotdb"=>("root.sim".into(),format!("root.sim.{name}.payload")),"dynamodb"=>("us-east-1".into(),name.into()),"etcd"=>("/".into(),format!("/{name}")),
        "rqlite"=>("main".into(),name.into()),_ =>("sim_db".into(),name.into())
    }
}
pub fn query(d:&Data)->String{match d.kind.as_str(){
    "elasticsearch"|"easysearch"=>json!({"query":{"match_all":{}},"size":50}).to_string(),
    "couchdb"=>json!({"selector":{},"limit":50}).to_string(),"meilisearch"=>"sim".into(),
    "qdrant"=>json!({"limit":50,"offset":null}).to_string(),"milvus"=>json!({"collectionName":d.object,"filter":"","limit":50}).to_string(),
    "weaviate"=>json!({"query":format!("{{ Get {{ {} {{ id payload nullable }} }} }}",d.container)}).to_string(),
    "chromadb"=>json!({"where":{}}).to_string(),"neo4j"=>format!("MATCH (n:`{}`) RETURN n.payload AS payload, n.nullable AS nullable",d.object),
    "victoriametrics"=>d.container.clone(),"pulsar"=>format!("/admin/v2/persistent/{}/{}/stats",d.container,d.object.trim_end_matches(":persistent")),
    "rabbitmq"=>format!("/api/queues/{}/{}",d.container,d.object.trim_start_matches("queue:")),
    "nacos"=>format!("svc:{}",d.object.trim_start_matches("svc:")),"consul"=>format!("/v1/kv/{}?recurse=true",d.object.trim_start_matches("dir:")),
    "dynamodb"=>json!({"operation":"Scan","payload":{"TableName":d.object,"Limit":50}}).to_string(),"etcd"=>json!({"operation":"range","key":"/"}).to_string(),
    "iotdb"=>format!("SELECT payload FROM {} LIMIT 50 OFFSET 0",d.object.trim_end_matches(".payload")),
    _=>format!("SELECT * FROM \"{}\" LIMIT 50 OFFSET 0",d.object)
}}
fn hash(b:&[u8])->String{format!("{:x}",Sha256::digest(b))}
fn mac(k:&[u8],v:&str)->Vec<u8>{let mut m=Hmac::<Sha256>::new_from_slice(k).unwrap();m.update(v.as_bytes());m.finalize().into_bytes().to_vec()}
fn signature_valid(r:&Request)->bool{
    let Some(auth)=r.headers.get("authorization")else{return false};let Some(date)=r.headers.get("x-amz-date")else{return false};if date.len()!=16{return false};
    let Some((credential,tail))=auth.strip_prefix("AWS4-HMAC-SHA256 Credential=").and_then(|s|s.split_once(", SignedHeaders="))else{return false};
    let Some((signed,signature))=tail.split_once(", Signature=")else{return false};let Some((access,scope))=credential.split_once('/')else{return false};
    if access!="sim_user"||scope!=format!("{}/us-east-1/dynamodb/aws4_request",&date[..8])||signed!="content-type;host;x-amz-date;x-amz-target"{return false;}
    let mut headers=String::new();for h in signed.split(';'){let Some(v)=r.headers.get(h)else{return false};headers.push_str(&format!("{h}:{v}\n"));}
    let canonical=format!("POST\n{}\n\n{}\n{}\n{}",r.path,headers,signed,hash(&r.body));
    let k=mac(b"AWS4sim_password",&date[..8]);let k=mac(&k,"us-east-1");let k=mac(&k,"dynamodb");let k=mac(&k,"aws4_request");
    let expected=mac(&k,&format!("AWS4-HMAC-SHA256\n{date}\n{scope}\n{}",hash(canonical.as_bytes()))).iter().map(|b|format!("{b:02x}")).collect::<String>();signature==expected
}
pub fn authenticated(r:&Request,d:&Data)->bool{match d.kind.as_str(){
    "dynamodb"=>signature_valid(r),"etcd"=>r.path=="/v3/auth/authenticate"&&r.json==json!({"name":"sim_user","password":"sim_password"})||r.headers.get("authorization").map(String::as_str)==Some("sim_etcd_token"),
    "nacos" if r.path=="/nacos/v1/auth/login"=>{let pairs=reqwest::Url::parse(&format!("http://local/?{}",r.text())).unwrap();let map=pairs.query_pairs().collect::<BTreeMap<_,_>>();map.get("username").map(|v|v.as_ref())==Some("sim_user")&&map.get("password").map(|v|v.as_ref())==Some("sim_password")},
    "nacos"=>r.q("accessToken")=="sim_nacos_token",
    "qdrant"=>r.headers.get("api-key").map(String::as_str)==Some("sim_password"),
    "meilisearch"|"weaviate"|"turso"|"d1"=>r.headers.get("authorization").map(String::as_str)==Some("Bearer sim_password"),
    "milvus"=>r.headers.get("authorization").map(String::as_str)==Some("Bearer sim_user:sim_password"),
    "consul"=>r.headers.get("x-consul-token").map(String::as_str)==Some("sim_password"),
    _=>r.headers.get("authorization").map(String::as_str)==Some(format!("Basic {}",b64("sim_user:sim_password")).as_str())
}}
fn slice(d:&Data,offset:usize,limit:usize)->Vec<Value>{d.docs.iter().skip(offset).take(limit).cloned().collect()}
fn rows(docs:&[Value])->Vec<Value>{docs.iter().map(|v|json!([v["id"],v["payload"],v["nullable"]])).collect()}
fn num(v:&Value,key:&str,default:usize)->usize{v[key].as_u64().map(|v|v as usize).unwrap_or(default)}
fn sql_page(sql:&str)->(usize,usize){let words=sql.split_whitespace().collect::<Vec<_>>();let find=|s:&str,default|words.windows(2).find(|w|w[0].eq_ignore_ascii_case(s)).and_then(|w|w[1].trim_end_matches(';').parse().ok()).unwrap_or(default);(find("OFFSET",0),find("LIMIT",100))}
fn require(r:&Request,method:&str,path:&str)->bool{r.method==method&&r.path==path}
fn invalid(r:&Request,d:&mut Data,msg:&str)->Response{d.violations.push(format!("{} {}: {msg}",r.method,r.target));Response::error(400,msg)}
pub fn handle(r:&Request,d:&mut Data)->Response{
    if !authenticated(r,d){d.actions.push(json!({"action":"rejected_auth","status":401}));return Response::error(401,"simulated invalid credentials");}
    if let Some(fault)=d.fault.take(){d.actions.push(json!({"action":"injected_response_fault","fault":fault}));return if fault=="malformed"{Response::json(json!({}))}else{business_error(d)};}
    let kind=d.kind.clone();match kind.as_str(){
        "elasticsearch"|"easysearch"=>{
            if require(r,"GET","/"){return Response::json(json!({"version":{"number":"8.0-sim"}}))}
            if require(r,"GET","/_cat/indices")&&r.q("format")=="json"{return Response::json(json!([{"index":d.object}]))}
            if r.path==format!("/{}/_search",d.object)||r.path=="/_all/_search"{
                let (off,lim)=if r.method=="POST"&&r.json["query"]["match_all"].is_object(){(num(&r.json,"from",0),num(&r.json,"size",100))}else if r.method=="GET"&&!r.q("q").contains("INVALID"){(0,r.number("size",100))}else{return Response::error(400,"invalid search DSL")};
                return Response::json(json!({"hits":{"hits":slice(d,off,lim).iter().map(|v|json!({"_id":v["id"],"_source":v})).collect::<Vec<_>>()}}));
            }
        },
        "solr"=>{
            if require(r,"GET","/solr/admin/cores")&&r.q("action")=="STATUS"&&r.q("wt")=="json"{return Response::json(json!({"status":{d.container.clone():{}}}))}
            if r.method=="GET"&&r.path==format!("/solr/{}/select",d.container)&&r.q("q")=="*:*"{return Response::json(json!({"response":{"docs":slice(d,r.number("start",0),r.number("rows",100))}}))}
            if r.method=="GET"&&r.path==format!("/solr/{}/sql",d.container)&&r.q("stmt").starts_with("SELECT "){let(o,l)=sql_page(r.q("stmt"));return Response::json(json!({"response":{"docs":slice(d,o,l)}}))}
        },
        "couchdb"=>{
            if require(r,"GET","/_all_dbs"){return Response::json(json!([d.container]))}
            if require(r,"GET",&format!("/{}/_all_docs",d.container))&&r.q("include_docs")=="true"{return Response::json(json!({"rows":slice(d,r.number("skip",0),r.number("limit",100)).iter().map(|v|json!({"doc":v})).collect::<Vec<_>>()}))}
            if require(r,"POST",&format!("/{}/_find",d.container))&&r.json["selector"].is_object(){return Response::json(json!({"docs":slice(d,num(&r.json,"skip",0),num(&r.json,"limit",100))}))}
        },
        "meilisearch"=>{
            if require(r,"GET","/indexes")&&r.q("limit")=="100"{return Response::json(json!({"results":[{"uid":d.container}]}))}
            if require(r,"GET",&format!("/indexes/{}/documents",d.container)){return Response::json(json!({"results":slice(d,r.number("offset",0),r.number("limit",100))}))}
            if require(r,"GET",&format!("/indexes/{}/search",d.container))&&r.q("q")=="sim"{return Response::json(json!({"hits":d.docs}))}
        },
        "qdrant"=>{
            if require(r,"GET","/collections"){return Response::json(json!({"result":{"collections":[{"name":d.container}]},"status":"ok"}))}
            if require(r,"POST",&format!("/collections/{}/points/scroll",d.container))&&r.json["with_payload"]==true{
                // Qdrant numeric offset is point-ID cursor, not a SQL row offset.
                let off=r.json["offset"].as_u64().unwrap_or(0) as usize;return Response::json(json!({"result":{"points":slice(d,off,num(&r.json,"limit",100)).iter().map(|v|json!({"id":v["id"],"payload":v})).collect::<Vec<_>>(),"next_page_offset":null},"status":"ok"}));
            }
        },
        "milvus"=>{
            if require(r,"POST","/v2/vectordb/databases/list")&&r.json==json!({}){return Response::json(json!({"code":0,"data":[d.container]}))}
            if require(r,"POST","/v2/vectordb/collections/list")&&r.json["dbName"]==d.container{return Response::json(json!({"code":0,"data":[d.object]}))}
            if require(r,"POST","/v2/vectordb/entities/query")&&r.json["dbName"]==d.container&&r.json["collectionName"]==d.object&&r.json["outputFields"]==json!(["*"]){return Response::json(json!({"code":0,"data":slice(d,num(&r.json,"offset",0),num(&r.json,"limit",100))}))}
        },
        "weaviate"=>{
            if require(r,"GET","/v1/schema"){return Response::json(json!({"classes":[{"class":d.container}]}))}
            if require(r,"GET","/v1/objects")&&r.q("class")==d.container{return Response::json(json!({"objects":slice(d,0,r.number("limit",100)).iter().map(|v|json!({"id":v["id"],"properties":v})).collect::<Vec<_>>()}))}
            if require(r,"POST","/v1/graphql")&&r.json["query"].as_str().is_some_and(|q|q.contains(&d.container)){return Response::json(json!({"data":{"Get":{d.container.clone():d.docs}}}))}
        },
        "chromadb"=>{
            if require(r,"GET","/api/v1/version"){return Response::json(json!("1.0-sim"))}
            if require(r,"GET","/api/v1/collections"){return Response::json(json!([{"name":d.container,"id":"sim_collection_id"}]))}
            if require(r,"GET",&format!("/api/v1/collections/{}",d.container)){return Response::json(json!({"id":"sim_collection_id","name":d.container}))}
            if require(r,"POST","/api/v1/collections/sim_collection_id/get")&&r.json["include"]==json!(["metadatas","documents"]){let docs=slice(d,num(&r.json,"offset",0),num(&r.json,"limit",100));return Response::json(json!({"ids":docs.iter().map(|v|v["id"].to_string()).collect::<Vec<_>>(),"documents":docs.iter().map(|v|v["payload"].clone()).collect::<Vec<_>>(),"metadatas":docs.iter().map(|v|v["nullable"].clone()).collect::<Vec<_>>()}))}
        },
        "neo4j"=>{if require(r,"POST",&format!("/db/{}/tx/commit",d.container)){let Some(sql)=r.json.pointer("/statements/0/statement").and_then(Value::as_str)else{return invalid(r,d,"Cypher statement missing")};let (cols,rs)=sql_result(d,sql);if sql.contains("INVALID"){return business_error(d)};let rs=if sql.contains("properties(n)"){let (off,lim)=cypher_page(sql);slice(d,off,lim).iter().map(|v|json!([v])).collect()}else{rs};return Response::json(json!({"results":[{"columns":cols,"data":rs.iter().map(|v|json!({"row":v})).collect::<Vec<_>>()}],"errors":[]}));}},
        "tdengine"|"iotdb"|"influxdb"|"turso"|"d1"|"druid"|"rqlite"|"clickhouse"=>return sql_handle(r,d),
        "victoriametrics"=>{
            if require(r,"GET","/api/v1/label/__name__/values"){return Response::json(json!({"status":"success","data":[d.container]}))}
            if require(r,"GET","/api/v1/query"){let q=r.q("query");if q=="1"{return Response::json(json!({"status":"success","data":{"resultType":"scalar","result":[1700000000,"1"]}}))}if q==d.container{return Response::json(json!({"status":"success","data":{"resultType":"vector","result":d.docs.iter().map(|v|json!({"metric":v,"value":[1700000000,"42"]})).collect::<Vec<_>>()}}))}return business_error(d);}
        },
        "pulsar"=>{
            if require(r,"GET","/admin/v2/tenants"){return Response::json(json!(["tenant"]))}
            if require(r,"GET","/admin/v2/namespaces/tenant"){return Response::json(json!([d.container]))}
            if require(r,"GET",&format!("/admin/v2/persistent/{}",d.container)){return Response::json(json!([format!("persistent://{}/{}",d.container,d.object.trim_end_matches(":persistent"))]))}
            if require(r,"GET",&format!("/admin/v2/non-persistent/{}",d.container)){return Response::json(json!([]))}
            if require(r,"GET",&format!("/admin/v2/persistent/{}/{}/stats",d.container,d.object.trim_end_matches(":persistent"))){return Response::json(json!({"msgInCounter":d.revision,"subscriptions":d.docs,"metadata":d.sentinel,"nullable":null}))}
        },
        "rabbitmq"=>{
            if require(r,"GET","/api/overview"){return Response::json(json!({"rabbitmq_version":"3.13-sim"}))}
            if require(r,"GET","/api/vhosts"){return Response::json(json!([{"name":d.container}]))}
            if require(r,"GET",&format!("/api/queues/{}",d.container)){return Response::json(json!([{"name":d.object.trim_start_matches("queue:")}]))}
            if require(r,"GET",&format!("/api/exchanges/{}",d.container)){return Response::json(json!([]))}
            if require(r,"GET",&format!("/api/queues/{}/{}",d.container,d.object.trim_start_matches("queue:"))){return Response::json(json!({"message_stats":d.revision,"payload":d.sentinel,"nullable":null,"consumers":d.docs}))}
        },
        "nacos"=>{
            if require(r,"POST","/nacos/v1/auth/login"){return Response::json(json!({"accessToken":"sim_nacos_token"}))}
            if require(r,"GET","/nacos/v1/console/server/state"){return Response::json(json!({"version":"2.0-sim"}))}
            if require(r,"GET","/nacos/v1/console/namespaces"){return Response::json(json!({"data":[{"namespace":d.container,"namespaceShowName":"Simulated"}]}))}
            if require(r,"GET","/nacos/v1/ns/service/list")&&r.q("namespaceId")==d.container{return Response::json(json!({"doms":[d.object.trim_start_matches("svc:")]}))}
            if require(r,"GET","/nacos/v1/cs/configs")&&r.q("search")=="blur"&&r.q("tenant")==d.container{return Response::json(json!({"pageItems":[]}))}
            if require(r,"GET","/nacos/v1/ns/instance/list")&&r.q("serviceName")==d.object.trim_start_matches("svc:")&&r.q("namespaceId")==d.container{return Response::json(json!({"hosts":d.docs}))}
        },
        "consul"=>{
            if require(r,"GET","/v1/agent/self"){return Response::json(json!({"Config":{"Version":"1.0-sim"}}))}
            if require(r,"GET","/v1/catalog/datacenters"){return Response::json(json!([d.container]))}
            if require(r,"GET","/v1/kv/")&&r.query.contains_key("keys")&&r.q("dc")==d.container{return Response::json(json!([format!("{}/key1",d.object.trim_start_matches("dir:"))]))}
            if require(r,"GET",&format!("/v1/kv/{}",d.object.trim_start_matches("dir:")))&&r.q("recurse")=="true"{return Response::json(json!(d.docs.iter().map(|v|json!({"Key":format!("{}/{}",d.object.trim_start_matches("dir:"),v["id"]),"Value":b64(v["payload"].as_str().unwrap()),"ModifyIndex":d.revision})).collect::<Vec<_>>()))}
            // Bare text is a valid KV key, including the negative test's absent key.
            // A missing key is a Consul 404, not an invalid wire envelope.
            if r.method=="GET" && r.path.starts_with("/v1/kv/") { return Response::error(404,"No key exists"); }
        },
        "dynamodb"=>return dynamo(r,d),"etcd"=>return etcd(r,d),_=>{}
    }
    invalid(r,d,"unexpected method/path/query/payload for simulated protocol")
}
fn cypher_page(sql:&str)->(usize,usize){let(off,lim)=sql_page(sql);let words=sql.split_whitespace().collect::<Vec<_>>();let skip=words.windows(2).find(|w|w[0]=="SKIP").and_then(|w|w[1].parse().ok()).unwrap_or(off);(skip,lim)}
fn sql_result(d:&Data,sql:&str)->(Vec<String>,Vec<Value>){
    let up=sql.to_ascii_uppercase();
    if up.contains("SELECT 1")||up=="RETURN 1"{return(vec!["one".into()],vec![json!([1])])}
    if up.contains("SERVER_VERSION"){return(vec!["version".into()],vec![json!(["3.0-sim"])])}
    if up.starts_with("SHOW DATABASES")||up.contains("DISTINCT TABLE_SCHEMA"){return(vec!["Database".into()],vec![json!([d.container])])}
    if up.starts_with("SHOW TABLES")||up.contains("SQLITE_MASTER")||up.contains("SELECT TABLE_NAME")||up.contains("SHOW MEASUREMENTS")||up.contains(".TABLES")||up.contains(".STABLES"){return(vec!["name".into()],vec![json!([d.object])])}
    if up.contains("SHOW TIMESERIES"){return(vec!["Timeseries".into()],vec![json!([d.object])])}
    if up.contains("DISTINCT LABELS"){return(vec!["label".into()],vec![json!([d.object])])}
    if up.starts_with("DESCRIBE TABLE"){return(vec!["name".into(),"type".into()],vec![json!(["id","UInt64"]),json!(["payload","String"]),json!(["nullable","Nullable(String)"])])}
    if up.contains("COUNT("){return(vec!["count".into()],vec![json!([d.docs.len()])])}
    if up.contains("INSERT INTO"){return(vec!["affected".into()],vec![json!([1])])}
    let(off,lim)=sql_page(sql);if up.contains("WHERE 1=0") { return (vec!["id".into(),"payload".into(),"nullable".into()],vec![]); } (vec!["id".into(),"payload".into(),"nullable".into()],rows(&slice(d,off,lim)))
}
fn business_error(d:&Data)->Response{Response::json(match d.kind.as_str(){
    "neo4j"=>json!({"results":[],"errors":[{"code":"Neo.ClientError.Statement.SyntaxError","message":"simulated syntax error"}]}),
    "tdengine"=>json!({"code":9721,"desc":"simulated syntax error"}),"iotdb"=>json!({"code":803,"message":"simulated permission error"}),
    "influxdb"=>json!({"results":[{"error":"simulated syntax error"}]}),"victoriametrics"=>json!({"status":"error","error":"simulated invalid query"}),
    "milvus"=>json!({"code":1100,"message":"simulated error"}),"turso"=>json!({"results":[{"type":"error","error":{"message":"SQL error"}}]}),
    "d1"=>json!({"success":false,"errors":[{"message":"SQL error"}]}),"rqlite"=>json!({"results":[{"error":"syntax error"}]}),
    "dynamodb"=>json!({"__type":"ValidationException","message":"simulated error"}),"weaviate"=>json!({"errors":[{"message":"simulated GraphQL error"}]}),
    "elasticsearch"|"easysearch"=>json!({"error":{"type":"parsing_exception"},"status":400}),"solr"=>json!({"error":{"code":400,"msg":"simulated error"}}),
    "meilisearch"=>json!({"code":"invalid_search_filter","message":"simulated error"}),"qdrant"=>json!({"status":{"error":"simulated error"}}),
    _=>json!({"error":"simulated business error"})
})}
fn sql_handle(r:&Request,d:&mut Data)->Response{
    let k=d.kind.as_str();if k=="influxdb"&&require(r,"GET","/ping"){let mut p=Response::text(String::new());p.headers.push(("X-Influxdb-Version".into(),"1.8-sim".into()));return p;}
    if k=="d1"&&require(r,"GET","/client/v4/accounts/sim_account/d1/sim_database"){return Response::json(json!({"success":true,"result":{"name":d.object}}))}
    let sql=match k{
        "tdengine" if r.method=="POST"&&r.path.starts_with("/rest/sql")=>r.text(),
        "iotdb" if r.method=="POST"&&matches!(r.path.as_str(),"/rest/v2/query"|"/rest/v2/nonQuery")=>r.json["sql"].as_str().unwrap_or("").into(),
        "influxdb" if require(r,"GET","/query")&&r.q("epoch")=="ms"=>r.q("q").into(),
        "turso" if require(r,"POST","/v2/pipeline")&&r.json.pointer("/requests/0/type")==Some(&json!("execute"))=>r.json.pointer("/requests/0/stmt/sql").and_then(Value::as_str).unwrap_or("").into(),
        "d1" if require(r,"POST","/client/v4/accounts/sim_account/d1/sim_database/query")=>r.json["sql"].as_str().unwrap_or("").into(),
        "druid" if require(r,"POST","/druid/v2/sql")&&r.json["resultFormat"]=="array"&&r.json["header"]==true=>r.json["query"].as_str().unwrap_or("").into(),
        "rqlite" if r.method=="POST"&&r.path=="/db/request"&&r.query.contains_key("transaction")=>r.json[0].as_str().unwrap_or("").into(),
        "clickhouse" if require(r,"POST","/")&&r.headers.get("x-clickhouse-database")==Some(&d.container)=>r.text().replace("\nFORMAT JSONCompactEachRowWithNamesAndTypes",""),
        _=>return invalid(r,d,"SQL protocol envelope/method/path invalid")
    };
    if sql.contains("INVALID")||sql.is_empty(){return if k=="clickhouse"{Response::error(400,"ClickHouse syntax error")}else{business_error(d)}}
    let (cols,rs)=sql_result(d,&sql);
    match k{
        "tdengine"=>Response::json(json!({"code":0,"column_meta":cols.iter().map(|c|json!([c,"VARCHAR",100])).collect::<Vec<_>>(),"data":rs})),
        "iotdb" if r.path=="/rest/v2/nonQuery"=>Response::json(json!({"code":200})),
        "iotdb"=>Response::json(json!({"column_names":cols,"values":(0..cols.len()).map(|i|rs.iter().map(|r|r[i].clone()).collect::<Vec<_>>()).collect::<Vec<_>>()})),
        "influxdb"=>Response::json(json!({"results":[{"series":[{"name":d.object,"columns":cols,"values":rs}]}]})),
        "turso"=>Response::json(json!({"results":[{"type":"ok","response":{"type":"execute","result":{"cols":cols.iter().map(|c|json!({"name":c})).collect::<Vec<_>>(),"rows":rs.iter().map(|r|r.as_array().unwrap().iter().map(|v|if v.is_null(){json!({"type":"null"})}else{json!({"type":"text","value":v})}).collect::<Vec<_>>()).collect::<Vec<_>>()}}}]})),
        "d1"=>Response::json(json!({"success":true,"result":[{"results":rs.iter().map(|r|cols.iter().enumerate().map(|(i,c)|(c.clone(),r[i].clone())).collect::<serde_json::Map<_,_>>()).collect::<Vec<_>>(),"meta":{"columns":cols}}]})),
        "druid"=>{let mut all=vec![json!(cols)];all.extend(rs);Response::json(json!(all))},
        "rqlite"=>Response::json(json!({"results":[{"columns":cols,"types":cols.iter().map(|_|"text").collect::<Vec<_>>(),"values":rs}]})),
        "clickhouse"=>{let mut text=format!("{}\n{}\n",json!(cols),json!(cols.iter().map(|_|"String").collect::<Vec<_>>()));for row in rs{text.push_str(&format!("{row}\n"));}Response::text(text)},
        _=>Response::error(500,"unhandled SQL simulator")
    }
}
fn dynamo(r:&Request,d:&mut Data)->Response{
    if !require(r,"POST","/"){return invalid(r,d,"DynamoDB requires POST /")};let op=r.headers.get("x-amz-target").and_then(|v|v.strip_prefix("DynamoDB_20120810.")).unwrap_or("");match op{
        "ListTables"=>{if r.json["Limit"].as_u64().is_none(){return invalid(r,d,"ListTables limit required")};Response::json(json!({"TableNames":[d.object]}))},
        "Scan" if r.json["TableName"]==d.object=>{let off=r.json.pointer("/ExclusiveStartKey/id/N").and_then(Value::as_str).and_then(|s|s.parse::<usize>().ok()).unwrap_or(0);let lim=num(&r.json,"Limit",100).min(2);let docs=slice(d,off,lim);let items=docs.iter().map(|v|json!({"id":{"N":v["id"].to_string()},"payload":{"S":v["payload"]},"nullable":{"NULL":true}})).collect::<Vec<_>>();let mut result=json!({"Items":items,"Count":docs.len()});if off+docs.len()<d.docs.len(){result["LastEvaluatedKey"]=json!({"id":{"N":(off+docs.len()).to_string()}})}Response::json(result)},
        "PutItem" if r.json["TableName"]==d.object=>{let item=&r.json["Item"];let Some(payload)=item.pointer("/payload/S").and_then(Value::as_str)else{return invalid(r,d,"typed item payload required")};d.docs[0]["payload"]=json!(payload);d.actions.push(json!({"action":"product_write","operation":"PutItem"}));Response::json(json!({}))},
        _=>business_error(d)
    }
}
fn etcd(r:&Request,d:&mut Data)->Response{
    if r.method!="POST"{return invalid(r,d,"etcd requires POST")};match r.path.as_str(){
        "/v3/auth/authenticate"=>Response::json(json!({"token":"sim_etcd_token"})),"/v3/maintenance/status"=>Response::json(json!({"header":{"revision":d.revision},"version":"3.5-sim"})),
        "/v3/kv/range"=>{let key=unb64(&r.json["key"]);let prefix=r.json.get("range_end").is_some();let docs=if prefix{d.docs.iter().collect::<Vec<_>>()}else if key==d.object{vec![&d.docs[0]]}else{vec![]};let lim=r.json["limit"].as_str().and_then(|s|s.parse::<usize>().ok()).unwrap_or(100);Response::json(json!({"header":{"revision":d.revision},"kvs":docs.iter().take(lim).enumerate().map(|(i,v)|json!({"key":b64(if i==0{&d.object}else{v["id"].as_str().unwrap_or("/other")}),"value":b64(v["payload"].as_str().unwrap()),"version":"1","mod_revision":d.revision.to_string()})).collect::<Vec<_>>(),"more":false}))},
        "/v3/kv/put"=>{let key=unb64(&r.json["key"]);let value=unb64(&r.json["value"]);if key!=d.object{return invalid(r,d,"put key mismatch")};d.docs[0]["payload"]=json!(value);d.actions.push(json!({"action":"product_write","operation":"put"}));Response::json(json!({"header":{"revision":d.revision}}))},
        _=>business_error(d)
    }
}
