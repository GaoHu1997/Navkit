//! Real service API adapters: Druid SQL, DynamoDB SigV4, etcd v3, rqlite.
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use serde_json::{json, Value};
use crate::{http_db::{HttpAdapter, HttpExportPage, HttpRows, jtext}, models::ConnectionRequest, native_db::Params};

pub struct ApiDb { kind: String, client: reqwest::Client, base: reqwest::Url, user: String, password: String, params: Params, token: tokio::sync::RwLock<Option<String>> }
fn b64(s: &[u8]) -> String { base64::engine::general_purpose::STANDARD.encode(s) }
fn unb64(value: &Value) -> Result<Option<String>> { match value.as_str() { Some(s) => Ok(Some(crate::native_db::text_bytes(&base64::engine::general_purpose::STANDARD.decode(s)?))), None => Ok(None) } }
fn table_rows(items: &[Value]) -> HttpRows {
    let mut columns = Vec::new(); for it in items { if let Some(o) = it.as_object() { for key in o.keys() { if !columns.contains(key) { columns.push(key.clone()); } } } }
    let rows = items.iter().map(|v| columns.iter().map(|k| v.get(k).and_then(jtext)).collect()).collect(); HttpRows { columns, rows }
}
fn first_column(rows: HttpRows) -> Vec<String> { rows.rows.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect() }
fn sha(s: &[u8]) -> String { format!("{:x}", Sha256::digest(s)) }
fn mac(key: &[u8], msg: &str) -> Vec<u8> { let mut h = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts arbitrary key length"); h.update(msg.as_bytes()); h.finalize().into_bytes().to_vec() }
pub(crate) fn signing_key(secret: &str, date: &str, region: &str) -> Vec<u8> { let k = mac(format!("AWS4{}",secret).as_bytes(),date); let k = mac(&k,region); let k=mac(&k,"dynamodb"); mac(&k,"aws4_request") }
impl ApiDb {
    pub async fn connect(req: &ConnectionRequest) -> Result<Self> {
        let params = Params::new(req); let default_ssl = req.kind.id() == "dynamodb" && req.port != 8000;
        let base = params.get("url").map(str::to_owned).unwrap_or_else(|| format!("{}://{}:{}", if params.bool("ssl",params.bool("tls",default_ssl)) { "https" } else { "http" }, req.host, if req.port==0 { req.kind.default_port() } else { req.port }));
        let base = reqwest::Url::parse(&base)?;
        if !matches!(base.scheme(), "http"|"https") || base.host_str().is_none() || !base.username().is_empty() || base.password().is_some() || base.fragment().is_some() || base.query().is_some() { bail!("数据源url必须是无内嵌凭据、query和fragment的HTTP(S)地址"); }
        let token=params.get("token").map(str::to_owned); let db = Self { kind:req.kind.id().into(), client:reqwest::Client::builder().timeout(params.timeout).danger_accept_invalid_certs(!params.bool("ssl_verify",true)).build()?, base, user:req.user.clone(), password:req.password.clone(), params, token:tokio::sync::RwLock::new(token) };
        if db.kind=="etcd" && !db.user.is_empty() && db.token.read().await.is_none() {
            let v=db.post("/v3/auth/authenticate", json!({"name":db.user,"password":db.password}), None).await?;
            *db.token.write().await=Some(v.get("token").and_then(Value::as_str).ok_or_else(||anyhow!("etcd认证未返回token"))?.into());
        }
        Ok(db)
    }
    fn url(&self, path: &str) -> Result<reqwest::Url> { Ok(reqwest::Url::parse(&format!("{}{}",self.base.as_str().trim_end_matches('/'),path))?) }
    async fn post(&self, path: &str, body:Value, operation:Option<&str>) -> Result<Value> {
        let url=self.url(path)?; let text=body.to_string(); let mut r=self.client.post(url.clone()).header("content-type","application/json");
        if self.kind=="dynamodb" {
            let operation=operation.ok_or_else(||anyhow!("DynamoDB操作缺失"))?;
            if !operation.chars().all(|c|c.is_ascii_alphanumeric()) { bail!("DynamoDB operation必须是API名称"); }
            let access=self.params.get("access_key").unwrap_or(&self.user); let secret=self.params.get("secret_key").unwrap_or(&self.password);
            if access.is_empty() || secret.is_empty() { bail!("DynamoDB需要access_key/user和secret_key/password；本地服务也需测试凭据"); }
            let now=chrono::Utc::now(); let timestamp=now.format("%Y%m%dT%H%M%SZ").to_string(); let date=now.format("%Y%m%d").to_string(); let region=self.params.get("region").unwrap_or("us-east-1");
            let host=match url.port(){Some(p)=>format!("{}:{}",url.host_str().unwrap_or_default(),p),None=>url.host_str().unwrap_or_default().into()}; let target=format!("DynamoDB_20120810.{}",operation);
            let mut headers=format!("content-type:application/x-amz-json-1.0\nhost:{}\nx-amz-date:{}\n",host,timestamp);
            let mut signed="content-type;host;x-amz-date".to_string();
            if let Some(token)=self.params.get("session_token") { headers.push_str(&format!("x-amz-security-token:{}\n",token)); signed.push_str(";x-amz-security-token"); r=r.header("x-amz-security-token",token); }
            headers.push_str(&format!("x-amz-target:{}\n",target)); signed.push_str(";x-amz-target");
            let canonical=format!("POST\n{}\n\n{}\n{}\n{}",url.path(),headers,signed,sha(text.as_bytes()));
            let scope=format!("{}/{}/dynamodb/aws4_request",date,region); let string=format!("AWS4-HMAC-SHA256\n{}\n{}\n{}",timestamp,scope,sha(canonical.as_bytes()));
            let signature=mac(&signing_key(&secret,&date,&region),&string).iter().map(|b|format!("{:02x}",b)).collect::<String>();
            r=r.header("content-type","application/x-amz-json-1.0").header("x-amz-date",timestamp).header("x-amz-target",target).header("authorization",format!("AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",access,scope,signed,signature));
        } else if self.kind=="etcd" { if let Some(token)=self.token.read().await.clone() { r=r.header("authorization",token); } }
        else if !self.user.is_empty() { r=r.basic_auth(&self.user,Some(&self.password)); }
        let response=r.body(text).send().await.context("原生API请求失败")?; let status=response.status(); let text=crate::http_db::read_response_text(response).await?;
        let value:Value=serde_json::from_str(&text).with_context(||format!("{}返回非JSON（HTTP {}）：{}",self.kind,status,text.chars().take(200).collect::<String>()))?;
        if !status.is_success() || value.get("error").is_some() || value.get("__type").is_some() { bail!("{} API失败（HTTP {}）：{}",self.kind,status,value); }
        Ok(value)
    }
    async fn sql(&self, sql:&str)->Result<HttpRows> {
        if self.kind=="druid" {
            let v=self.post("/druid/v2/sql",json!({"query":sql,"resultFormat":"array","header":true}),None).await?;
            let a=v.as_array().ok_or_else(||anyhow!("Druid SQL返回不是数组"))?;
            let header=a.first().and_then(Value::as_array).ok_or_else(||anyhow!("Druid SQL未返回header"))?;
            return Ok(HttpRows{columns:header.iter().map(|v|jtext(v).unwrap_or_default()).collect(),rows:a.iter().skip(1).map(|r|r.as_array().ok_or_else(||anyhow!("Druid SQL行不是数组")).map(|r|r.iter().map(jtext).collect())).collect::<Result<_>>()?});
        }
        let v=self.post("/db/request?transaction",json!([sql]),None).await?;
        let results=v.get("results").and_then(Value::as_array).ok_or_else(||anyhow!("rqlite响应缺少results"))?;
        let r=results.first().ok_or_else(||anyhow!("rqlite没有返回语句结果"))?;
        if let Some(error)=r.get("error") { bail!("rqlite SQL: {}",error); }
        if r.get("columns").is_none() { return Ok(table_rows(&[json!({"affected":r.get("rows_affected"),"last_insert_id":r.get("last_insert_id")})])); }
        Ok(HttpRows{columns:serde_json::from_value(r["columns"].clone())?,rows:r.get("values").and_then(Value::as_array).map(|a|a.iter().map(|r|r.as_array().map(|v|v.iter().map(jtext).collect()).unwrap_or_default()).collect()).unwrap_or_default()})
    }
    async fn dynamo(&self, operation:&str, payload:Value)->Result<Value> {
        let value = self.post("/",payload,Some(operation)).await?;
        if !value.is_object() { bail!("DynamoDB {}响应不是对象", operation); }
        let array = match operation { "ListTables" => Some("TableNames"), "Scan"|"Query"|"ExecuteStatement" => Some("Items"), _ => None };
        if let Some(key) = array { if !value.get(key).is_some_and(Value::is_array) { bail!("DynamoDB {}响应缺少{}数组",operation,key); } }
        Ok(value)
    }
    async fn list_tables(&self)->Result<Vec<String>> { let mut out=Vec::new(); let mut last=None; loop { let mut p=json!({"Limit":100}); if let Some(last)=last {p["ExclusiveStartTableName"]=Value::String(last);} let v=self.dynamo("ListTables",p).await?; out.extend(serde_json::from_value::<Vec<String>>(v["TableNames"].clone())?); last=v.get("LastEvaluatedTableName").and_then(Value::as_str).map(str::to_string); if last.is_none(){break;} } Ok(out) }
    async fn etcd_range(&self, key:&str, prefix:bool, limit:u64)->Result<Value> { let mut body=json!({"key":b64(key.as_bytes()),"limit":limit.to_string()}); if prefix { body["range_end"]=Value::String(b64(&if key=="\0" { vec![0] } else { prefix_end(key.as_bytes()) })); } self.post("/v3/kv/range",body,None).await }
}
pub(crate) fn prefix_end(key:&[u8])->Vec<u8>{let mut end=key.to_vec(); for i in (0..end.len()).rev(){if end[i]!=255{end[i]+=1;end.truncate(i+1);return end;}} vec![0]}
fn etcd_rows(v:&Value)->Result<HttpRows>{
    if !v.get("header").is_some_and(Value::is_object) { bail!("etcd range响应缺少header"); }
    let mut rows=Vec::new();
    if let Some(kvs)=v.get("kvs") {
        for kv in kvs.as_array().ok_or_else(||anyhow!("etcd kvs不是数组"))? {
            if !kv.get("key").is_some_and(Value::is_string) { bail!("etcd key不是base64字符串"); }
            rows.push(vec![unb64(&kv["key"])?,unb64(&kv["value"])?,kv.get("version").and_then(jtext),kv.get("mod_revision").and_then(jtext)]);
        }
    }
    Ok(HttpRows{columns:vec!["key".into(),"value".into(),"version".into(),"mod_revision".into()],rows})
}
const EXPORT_TOKEN_MAX_BYTES: usize = 64 * 1024;

// Scan 游标绑定表名，保持 DynamoDB AttributeValue（包括 N/B）原始 JSON 表示。
fn scan_start_key(token: &str, table: &str) -> Result<Value> {
    if token.len() > EXPORT_TOKEN_MAX_BYTES { bail!("DynamoDB 导出游标超过 64 KiB 上限"); }
    let value: Value = serde_json::from_str(token).context("DynamoDB 导出游标不是合法 JSON")?;
    if value.get("table").and_then(Value::as_str) != Some(table) {
        bail!("DynamoDB 导出游标不属于当前表");
    }
    match value.get("key") {
        Some(key) if key.as_object().is_some_and(|key| !key.is_empty()) => Ok(key.clone()),
        _ => bail!("DynamoDB 导出游标缺少非空 ExclusiveStartKey"),
    }
}
fn scan_next_token(table: &str, key: Option<&Value>) -> Result<Option<String>> {
    let Some(key) = key else { return Ok(None) };
    let object = key.as_object().ok_or_else(|| anyhow!("DynamoDB LastEvaluatedKey 不是对象"))?;
    if object.is_empty() { return Ok(None); }
    let token = serde_json::to_string(&json!({"table":table,"key":key}))?;
    if token.len() > EXPORT_TOKEN_MAX_BYTES { bail!("DynamoDB 导出游标超过 64 KiB 上限"); }
    Ok(Some(token))
}
fn etcd_export_columns() -> Vec<String> {
    ["key", "value", "version", "mod_revision"].into_iter().map(str::to_owned).collect()
}

#[async_trait]
impl HttpAdapter for ApiDb {
 async fn ping(&self)->Result<String>{match self.kind.as_str(){"druid"|"rqlite"=>{let r=self.sql("SELECT 1").await?;if r.columns.is_empty()||r.rows.is_empty(){bail!("SQL健康查询未返回真实行");}} "dynamodb"=>{let v=self.dynamo("ListTables",json!({"Limit":1})).await?;if !v["TableNames"].is_array(){bail!("DynamoDB ListTables缺少TableNames数组");}} "etcd"=>{let v=self.post("/v3/maintenance/status",json!({}),None).await?;if !v["version"].is_string()||!v["header"].is_object(){bail!("etcd maintenance status缺少version/header");}} _=>bail!("未知API数据源")} Ok(format!("{} API已验证",self.kind))}
 async fn containers(&self)->Result<Vec<(String,String)>>{let names=match self.kind.as_str(){"druid"=>first_column(self.sql("SELECT DISTINCT TABLE_SCHEMA FROM INFORMATION_SCHEMA.TABLES").await?),"rqlite"=>{self.sql("SELECT 1").await?;vec!["main".into()]},"dynamodb"=>{self.list_tables().await?;vec![self.params.get("region").unwrap_or("us-east-1").to_string()]},"etcd"=>{self.etcd_range("\0",true,1).await?;vec!["/".into()]},_=>bail!("未知API数据源")};Ok(names.into_iter().map(|n|(n,self.kind.clone())).collect())}
 async fn objects(&self,container:&str)->Result<Vec<String>>{match self.kind.as_str(){"druid"=>Ok(first_column(self.sql(&format!("SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA={}",crate::db::quote_literal(container))).await?)),"rqlite"=>Ok(first_column(self.sql("SELECT name FROM sqlite_master WHERE type IN ('table','view') ORDER BY name").await?)),"dynamodb"=>self.list_tables().await,"etcd"=>{let key=if container=="/" {"\0"} else {container};let v=self.etcd_range(key,true,10000).await?;if v.get("more").and_then(Value::as_bool)==Some(true){bail!("etcd目录超过10000个key，请指定更窄前缀");}Ok(first_column(etcd_rows(&v)?))},_=>bail!("未知API数据源")}}
 async fn rows(&self,container:&str,object:&str,offset:u64,limit:u64)->Result<HttpRows>{match self.kind.as_str(){"druid"|"rqlite"=>{let kind=crate::models::DbKind::of(&self.kind);self.sql(&format!("SELECT * FROM {} LIMIT {} OFFSET {}",crate::db::qualified(&kind,if self.kind=="rqlite" {""} else {container},object),limit.min(10000),offset)).await},"dynamodb"=>{if offset.saturating_add(limit)>10000 { bail!("DynamoDB浏览窗口最多10000行，请缩小offset/limit"); } let target=offset.saturating_add(limit);let mut items=Vec::new();let mut last=None;while items.len()<(target as usize){let mut p=json!({"TableName":object,"Limit":(target-items.len() as u64).max(1)});if let Some(k)=last{p["ExclusiveStartKey"]=k;}let v=self.dynamo("Scan",p).await?;items.extend(v["Items"].as_array().ok_or_else(||anyhow!("DynamoDB Scan响应缺少Items"))?.iter().cloned());last=v.get("LastEvaluatedKey").cloned().filter(|v|v.as_object().is_some_and(|o|!o.is_empty()));if last.is_none(){break;}}let selected=items.into_iter().skip(offset as usize).take(limit.min(10000) as usize).collect::<Vec<_>>();Ok(table_rows(&selected))},"etcd"=>{let mut r=etcd_rows(&self.etcd_range(object,false,limit.min(10000)).await?)?;r.rows=r.rows.into_iter().skip(offset as usize).take(limit as usize).collect();Ok(r)},_=>bail!("未知API数据源")}}
 /// 导出不复用 DynamoDB 浏览窗口：每次 Scan 仅请求当前页，依 LastEvaluatedKey 续接。
 /// etcd 的目录对象是一个实际 key，只导出该 key 当前值，不虚构表或历史扫描。
 async fn export_page(&self,container:&str,object:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{
    if limit == 0 {
        return Ok(HttpExportPage {
            data: HttpRows { columns: if self.kind == "etcd" { etcd_export_columns() } else { vec![] }, rows: vec![] },
            next: None,
        });
    }
    match self.kind.as_str() {
        "dynamodb" => {
            let mut request = json!({"TableName":object,"Limit":limit.clamp(1, 1000)});
            let start_key = token.map(|token| scan_start_key(token, object)).transpose()?;
            if let Some(key) = &start_key { request["ExclusiveStartKey"] = key.clone(); }
            let value = self.dynamo("Scan", request).await?;
            let items = value["Items"].as_array().ok_or_else(|| anyhow!("DynamoDB Scan 响应缺少 Items"))?;
            if items.iter().any(|item| !item.is_object()) { bail!("DynamoDB Scan Items 包含非对象记录"); }
            let next = scan_next_token(object, value.get("LastEvaluatedKey"))?;
            if next.is_some() && start_key.as_ref() == value.get("LastEvaluatedKey") { bail!("DynamoDB Scan 导出游标未推进"); }
            // 空 Items 页仍可能有 LastEvaluatedKey：是否 EOF 只能由 next 决定。
            Ok(HttpExportPage { data: table_rows(items), next })
        }
        "etcd" => {
            if token.is_some() { bail!("etcd 单 key 导出没有续页游标"); }
            let key = match object.strip_prefix("base64:") {
                Some(raw) => base64::engine::general_purpose::STANDARD.decode(raw).context("etcd 目录 key 不是有效 base64")?,
                None => object.as_bytes().to_vec(),
            };
            if key.is_empty() { bail!("etcd 导出 key 不能为空"); }
            let value = self.post("/v3/kv/range", json!({"key":b64(&key),"limit":"1"}), None).await?;
            if value.get("more").and_then(Value::as_bool) == Some(true) { bail!("etcd 单 key 查询错误地返回了更多记录"); }
            let data = etcd_rows(&value)?;
            if data.rows.len() > 1 { bail!("etcd 单 key 查询返回多条记录"); }
            if let Some(kvs) = value.get("kvs").and_then(Value::as_array) {
                for kv in kvs {
                    let actual = kv.get("key").and_then(Value::as_str).ok_or_else(|| anyhow!("etcd key 缺少 base64 字符串"))?;
                    if !kv.get("value").is_some_and(Value::is_string) { bail!("etcd 单 key 查询缺少 base64 value"); }
                    if base64::engine::general_purpose::STANDARD.decode(actual)? != key { bail!("etcd 单 key 查询返回了不同 key"); }
                }
            }
            Ok(HttpExportPage { data, next: None })
        }
        _ => {
            let offset = match token { None => 0, Some(t) => t.parse::<u64>().context("导出分页游标无效")? };
            let data = self.rows(container, object, offset, limit).await?;
            let count = data.rows.len() as u64;
            let next = if count == 0 { None } else { Some(offset.checked_add(count).ok_or_else(|| anyhow!("导出分页偏移溢出"))?.to_string()) };
            Ok(HttpExportPage { data, next })
        }
    }
 }
 async fn run(&self,_:&str,_:&str,text:&str)->Result<HttpRows>{match self.kind.as_str(){"druid"|"rqlite"=>self.sql(text).await,"dynamodb"=>{let (op,payload)=if text.trim().starts_with('{'){let v:Value=serde_json::from_str(text)?;(v["operation"].as_str().ok_or_else(||anyhow!("需operation字段"))?.to_string(),v.get("payload").cloned().unwrap_or(json!({})))}else{("ExecuteStatement".into(),json!({"Statement":text}))};let v=self.dynamo(&op,payload).await?;Ok(if let Some(items)=v.get("Items").and_then(Value::as_array){table_rows(items)}else{table_rows(&[v])})},"etcd"=>{let v:Value=serde_json::from_str(text).context("etcd命令为JSON {operation:get|range|put|delete,key,value}")?;let key=v["key"].as_str().ok_or_else(||anyhow!("etcd需要key"))?;match v["operation"].as_str(){Some("get")=>etcd_rows(&self.etcd_range(key,false,10000).await?),Some("range")=>etcd_rows(&self.etcd_range(key,true,10000).await?),Some("put")=>{let val=v["value"].as_str().ok_or_else(||anyhow!("put需要value字符串"))?;Ok(table_rows(&[self.post("/v3/kv/put",json!({"key":b64(key.as_bytes()),"value":b64(val.as_bytes())}),None).await?]))},Some("delete")=>Ok(table_rows(&[self.post("/v3/kv/deleterange",json!({"key":b64(key.as_bytes())}),None).await?])),_=>bail!("未知etcd operation")}},_=>bail!("未知API数据源")}}
}
#[cfg(test)]
#[path = "native_api_export_tests.rs"]
mod native_api_export_tests;

#[cfg(test)]mod tests{use super::*;#[test]fn prefix_end_preserves_bytes(){assert_eq!(prefix_end(b"/app/"),b"/app0");assert_eq!(prefix_end(&[255]),vec![0]);}#[test]fn etcd_unicode_null(){let r=etcd_rows(&json!({"header":{},"kvs":[{"key":b64("键".as_bytes()),"value":b64("值;尾".as_bytes())}]})).unwrap();assert_eq!(r.rows[0][1].as_deref(),Some("值;尾"));}#[test]fn signing_key_is_256_bits(){assert_eq!(signing_key("secret","20260101","us-east-1").len(),32);}}
