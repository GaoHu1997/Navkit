//! Salesforce REST v59+: real bearer authentication, describe, SOQL pagination and CRUD.
//! No JDBC/ODBC impersonation. OAuth token acquisition/refresh remains explicit user work.
use anyhow::{anyhow,bail,Result};
use async_trait::async_trait;
use reqwest::Method;
use serde_json::Value;
use serde::{Deserialize,Serialize};
use std::collections::{BTreeSet,HashSet};
use crate::{http_db::{HttpAdapter,HttpColumn,HttpExportPage,HttpRows},models::ConnectionRequest,strict_rest::{self as rest,Session,Auth}};

const MAX_EXPORT_TOKEN_BYTES:usize=64*1024;
#[derive(Serialize,Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportToken{version:String,object:String,columns:Vec<String>,next:String,total:u64,returned:u64}
struct ExportBudget{bytes:usize}
impl std::io::Write for ExportBudget{
    fn write(&mut self,data:&[u8])->std::io::Result<usize>{
        if data.len()>rest::MAX_BODY.saturating_sub(self.bytes){return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"Salesforce 导出页超过 16 MiB；未截断为成功结果"));}
        self.bytes+=data.len();Ok(data.len())
    }
    fn flush(&mut self)->std::io::Result<()>{Ok(())}
}
struct Salesforce {http:Session,version:String}
pub(crate) async fn connect(req:&ConnectionRequest)->Result<Box<dyn HttpAdapter>>{
    let params=rest::params(&req.params,&["api_version"])?;
    if !req.user.is_empty()&&!req.user.eq_ignore_ascii_case("token"){bail!("Salesforce REST 用户名应留空或填 token；OAuth 令牌填密码字段，不能用账户密码代替");}
    if req.password.is_empty(){bail!("Salesforce REST 需要预生成 OAuth 访问令牌");}
    let version=params.get("api_version").map(String::as_str).filter(|v|!v.is_empty()).unwrap_or(if req.database.is_empty(){"v59.0"}else{&req.database});
    if !req.database.is_empty()&&req.database!=version{bail!("Salesforce API 版本参数与数据库字段冲突");}
    let Some((major,minor))=version.strip_prefix('v').and_then(|v|v.split_once('.')) else{bail!("Salesforce 数据库字段/API 版本格式应为 v59.0");};
    if minor!="0"||major.len()>3||major.is_empty()||!major.bytes().all(|b|b.is_ascii_digit())||major.parse::<u32>().unwrap_or(0)<59{bail!("Salesforce API 版本必须为 v59.0 或更高的整数主版本");}
    let base=rest::endpoint(req,&params,443,true)?;
    Ok(Box::new(Salesforce{http:Session::new(base,Auth::Bearer(req.password.clone()),req.timeout_secs)?,version:version.into()}))
}
impl Salesforce {
    fn namespace(&self,value:&str)->Result<()>{if !value.is_empty()&&value!=self.version{bail!("Salesforce 容器必须是此连接的 API 版本 {}",self.version);}Ok(())}
    fn url(&self,tail:&[&str])->Result<reqwest::Url>{let mut parts=vec!["services","data",&self.version];parts.extend_from_slice(tail);self.http.path(&parts)}
    async fn catalogue(&self)->Result<Vec<String>>{
        let v=self.http.json(Method::GET,self.url(&["sobjects"] )?,None).await?;
        let objects=v.get("sobjects").and_then(Value::as_array).ok_or_else(||anyhow!("Salesforce sobjects 响应缺少对象数组"))?;
        let mut names=Vec::new();let mut unique=HashSet::new();
        for obj in objects {let name=rest::identifier(rest::string(obj,"name")?)?;let queryable=obj.get("queryable").and_then(Value::as_bool).ok_or_else(||anyhow!("Salesforce 对象缺少 queryable 布尔元数据"))?;if !unique.insert(name.to_string()){bail!("Salesforce 返回重复对象名称");}if queryable{names.push(name.into());}}
        Ok(names)
    }
    async fn describe(&self,object:&str)->Result<Vec<HttpColumn>>{
        rest::identifier(object)?;let v=self.http.json(Method::GET,self.url(&["sobjects",object,"describe"] )?,None).await?;
        if rest::string(&v,"name")?!=object{bail!("Salesforce describe 返回了不同对象");}
        let fields=v.get("fields").and_then(Value::as_array).ok_or_else(||anyhow!("Salesforce describe 缺少 fields"))?;
        let mut cols=Vec::new();let mut unique=HashSet::new();
        for field in fields {let name=rest::identifier(rest::string(field,"name")?)?;if !unique.insert(name.to_string()){bail!("Salesforce describe 返回重复字段");}cols.push(HttpColumn{name:name.into(),data_type:rest::string(field,"type")?.into(),nullable:field.get("nillable").and_then(Value::as_bool).ok_or_else(||anyhow!("Salesforce 字段缺少 nillable"))?,primary_key:name=="Id"});}
        if cols.is_empty(){bail!("Salesforce describe 返回空字段，不能据此伪造表结构");}Ok(cols)
    }
    fn next(&self,raw:&str)->Result<reqwest::Url>{
        if raw.len()>MAX_EXPORT_TOKEN_BYTES{bail!("Salesforce nextRecordsUrl 超过 64 KiB 上限");}
        let expected=format!("/services/data/{}/query/",self.version);
        if !raw.starts_with(&expected)||raw.contains(['\\','#','%']){bail!("Salesforce nextRecordsUrl 不是当前 API 的安全相对 query 路径");}
        let url=self.http.base.join(raw)?;
        if !self.http.same_origin(&url)||!url.path().starts_with(&expected)||url.query().is_some(){bail!("Salesforce nextRecordsUrl 越过当前 origin/API 查询范围");}Ok(url)
    }
    async fn query(&self,soql:&str,hints:Vec<String>)->Result<HttpRows>{
        if soql.trim().is_empty()||soql.len()>100_000||soql.contains('\0'){bail!("Salesforce SOQL 为空或超出长度限制");}
        let mut url=self.url(&["query"])?;url.query_pairs_mut().append_pair("q",soql);
        let mut seen=HashSet::new();let mut records=Vec::new();let mut extra=BTreeSet::new();
        loop {
            if !seen.insert(url.to_string())||seen.len()>512{bail!("Salesforce 查询分页循环或超过 512 页；结果未截断为成功");}
            let v=self.http.json(Method::GET,url,None).await?;
            v.get("totalSize").and_then(Value::as_u64).ok_or_else(||anyhow!("Salesforce query 缺少 totalSize"))?;
            let done=v.get("done").and_then(Value::as_bool).ok_or_else(||anyhow!("Salesforce query 缺少 done"))?;
            let page=v.get("records").and_then(Value::as_array).ok_or_else(||anyhow!("Salesforce query 缺少 records"))?;
            for item in page {let obj=item.as_object().ok_or_else(||anyhow!("Salesforce record 必须为对象"))?;for key in obj.keys().filter(|key|key.as_str()!="attributes"){extra.insert(key.clone());}records.push(item.clone());}
            if records.len()>100_000{bail!("Salesforce 查询超过 100000 条；请添加 LIMIT，结果未截断为成功");}
            if done {break;}
            if page.is_empty(){bail!("Salesforce 未结束的分页返回空记录；拒绝无限循环/伪成功");}
            url=self.next(rest::string(&v,"nextRecordsUrl")?)?;
        }
        let mut columns=hints;for key in extra{if !columns.contains(&key){columns.push(key);}}
        let rows=records.iter().map(|row|columns.iter().map(|col|row.get(col).and_then(crate::native_db::cell)).collect()).collect();
        Ok(HttpRows{columns,rows})
    }
    fn values<'a>(&self,v:&'a Value)->Result<&'a Value>{let values=v.get("values").ok_or_else(||anyhow!("Salesforce 写命令需要 values 对象"))?;let obj=values.as_object().filter(|o|!o.is_empty()).ok_or_else(||anyhow!("Salesforce values 必须为非空对象"))?;for key in obj.keys(){rest::identifier(key)?;}Ok(values)}
    fn id<'a>(&self,v:&'a Value)->Result<&'a str>{let id=rest::string(v,"id")?;if ![15,18].contains(&id.len())||!id.bytes().all(|b|b.is_ascii_alphanumeric()){bail!("Salesforce id 必须为 15 或 18 位字母数字");}Ok(id)}
}
#[async_trait]
impl HttpAdapter for Salesforce {
    fn shares_session_namespace(&self)->bool{true}
    async fn ping(&self)->Result<String>{self.catalogue().await?;Ok(format!("Salesforce REST {}（真实 OAuth /sobjects 认证通过）",self.version))}
    async fn containers(&self)->Result<Vec<(String,String)>>{self.catalogue().await?;Ok(vec![(self.version.clone(),format!("Salesforce REST API {}",self.version))])}
    async fn objects(&self,container:&str)->Result<Vec<String>>{self.namespace(container)?;self.catalogue().await}
    async fn table_columns(&self,container:&str,object:&str)->Result<Option<Vec<HttpColumn>>>{self.namespace(container)?;Ok(Some(self.describe(object).await?))}
    async fn rows(&self,container:&str,object:&str,offset:u64,limit:u64)->Result<HttpRows>{
        self.namespace(container)?;let fields=self.describe(object).await?;let columns:Vec<String>=fields.into_iter().map(|f|f.name).collect();
        if limit>2000||offset>2000{bail!("Salesforce 浏览 limit/offset 均不能超过 SOQL 的 2000 限制；完整查询请用 SOQL 游标分页");}
        if limit==0{return Ok(HttpRows{columns,rows:Vec::new()});}
        let order=if columns.iter().any(|c|c=="Id"){" ORDER BY Id"}else{""};
        self.query(&format!("SELECT {} FROM {}{} LIMIT {} OFFSET {}",columns.join(","),rest::identifier(object)?,order,limit,offset),columns).await
    }
    async fn run(&self,container:&str,_object:&str,sql:&str)->Result<HttpRows>{
        self.namespace(container)?;let input=sql.trim();
        if !input.starts_with('{'){return self.query(input,Vec::new()).await;}
        let v:Value=serde_json::from_str(input)?;let op=rest::string(&v,"op")?;
        match op {
            "query"=>{rest::fields(&v,&["op","soql"])?;self.query(rest::string(&v,"soql")?,Vec::new()).await},
            "create"=>{rest::fields(&v,&["op","object","values"])?;let object=rest::identifier(rest::string(&v,"object")?)?;let reply=self.http.exchange(Method::POST,self.url(&["sobjects",object])?,Some(self.values(&v)?)).await?;self.http.check(&reply,&[201])?;let result=reply.json()?;if result.get("success").and_then(Value::as_bool)!=Some(true)||result.get("errors").and_then(Value::as_array).map_or(true,|e|!e.is_empty()){bail!("Salesforce 创建失败：{}",self.http.redact(&result.to_string()));}let id=self.id(&result)?;Ok(crate::native_db::single("id",id))},
            "update"|"delete"=>{rest::fields(&v,if op=="update"{&["op","object","id","values"]}else{&["op","object","id"]})?;let object=rest::identifier(rest::string(&v,"object")?)?;let id=self.id(&v)?;let reply=self.http.exchange(if op=="update"{Method::PATCH}else{Method::DELETE},self.url(&["sobjects",object,id])?,if op=="update"{Some(self.values(&v)?)}else{None}).await?;self.http.check(&reply,&[204])?;if !reply.body.is_empty(){bail!("Salesforce 204 响应不应携带结果体");}Ok(crate::native_db::single("affected_rows","1"))},
            _=>bail!("Salesforce 支持 SOQL 或 JSON op=query/create/update/delete；不伪装执行 SQL DDL")
        }
    }
    async fn export_page(&self,container:&str,object:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{
        self.namespace(container)?;rest::identifier(object)?;
        // Freeze describe metadata across pages; never buffer all records or use browse OFFSET.
        let (columns,url,expected_total,returned)=match token.filter(|_|limit!=0){
            None=>{
                let columns:Vec<String>=self.describe(object).await?.into_iter().map(|f|f.name).collect();
                if limit==0{return Ok(HttpExportPage{data:HttpRows{columns,rows:Vec::new()},next:None});}
                if serde_json::to_vec(&columns)?.len()>MAX_EXPORT_TOKEN_BYTES/2{bail!("Salesforce 导出字段元数据超过游标容量；拒绝不完整字段导出");}
                let order=if columns.iter().any(|c|c=="Id"){" ORDER BY Id"}else{""};
                let soql=format!("SELECT {} FROM {}{}",columns.join(","),object,order);
                if soql.len()>100_000{bail!("Salesforce SOQL 超出长度限制");}
                let mut query=self.url(&["query"])?;query.query_pairs_mut().append_pair("q",&soql);
                (columns,query,None,0)
            }
            Some(raw)=>{
                if raw.len()>MAX_EXPORT_TOKEN_BYTES{bail!("Salesforce 导出游标超过 64 KiB 上限");}
                let saved:ExportToken=serde_json::from_str(raw).map_err(|_|anyhow!("Salesforce 导出游标结构无效"))?;
                if saved.version!=self.version||saved.object!=object{bail!("Salesforce 导出游标属于不同 API 或对象");}
                let mut unique=HashSet::new();
                for column in &saved.columns{rest::identifier(column)?;if !unique.insert(column){bail!("Salesforce 导出游标有重复字段");}}
                if saved.columns.is_empty()||saved.returned>=saved.total{bail!("Salesforce 导出游标字段或记录计数无效");}
                let url=self.next(&saved.next)?;
                (saved.columns,url,Some(saved.total),saved.returned)
            }
        };
        let previous_path=url.path().to_string();
        let v=self.http.json(Method::GET,url,None).await?;
        let total=v.get("totalSize").and_then(Value::as_u64).ok_or_else(||anyhow!("Salesforce query 缺少 totalSize"))?;
        if expected_total.is_some_and(|n|n!=total){bail!("Salesforce 分页 totalSize 变化；拒绝不完整成功结果");}
        let done=v.get("done").and_then(Value::as_bool).ok_or_else(||anyhow!("Salesforce query 缺少 done"))?;
        let page=v.get("records").and_then(Value::as_array).ok_or_else(||anyhow!("Salesforce query 缺少 records"))?;
        let returned=returned.checked_add(page.len() as u64).ok_or_else(||anyhow!("Salesforce 导出记录计数溢出"))?;
        if returned>total||done&&returned!=total||!done&&returned>=total{bail!("Salesforce done/totalSize 与实际导出记录数不一致；拒绝截断或重复成功");}
        if !done&&page.is_empty(){bail!("Salesforce 未结束的分页返回空记录；拒绝无限循环/伪成功");}
        let known:HashSet<&str>=columns.iter().map(String::as_str).collect();
        let mut budget=ExportBudget{bytes:4};
        serde_json::to_writer(&mut budget,&columns)?;
        let mut rows=Vec::with_capacity(page.len());
        for item in page{
            let record=item.as_object().ok_or_else(||anyhow!("Salesforce record 必须为对象"))?;
            if record.get("attributes").is_some_and(|v|!v.is_object()){bail!("Salesforce attributes 必须为对象");}
            if record.keys().any(|key|key!="attributes"&&!known.contains(key.as_str())){bail!("Salesforce record 返回 describe 未声明的字段；拒绝静默丢列");}
            if columns.iter().any(|column|!record.contains_key(column)){bail!("Salesforce record 缺少所选字段；不能将缺失字段伪造为 NULL");}
            let row:Vec<Option<String>>=columns.iter().map(|column|item.get(column).and_then(crate::native_db::cell)).collect();
            std::io::Write::write_all(&mut budget,b",")?;
            serde_json::to_writer(&mut budget,&row)?;
            rows.push(row);
        }
        let next=if done{
            if v.get("nextRecordsUrl").is_some_and(|v|!v.is_null()){bail!("Salesforce 已结束分页仍返回 nextRecordsUrl");}None
        }else{
            let next=self.next(rest::string(&v,"nextRecordsUrl")?)?.path().to_string();
            if next==previous_path{bail!("Salesforce nextRecordsUrl 未推进；拒绝重复分页");}
            let token=serde_json::to_string(&ExportToken{version:self.version.clone(),object:object.into(),columns:columns.clone(),next,total,returned})?;
            if token.len()>MAX_EXPORT_TOKEN_BYTES{bail!("Salesforce 导出游标超过 64 KiB 上限");}
            Some(token)
        };
        Ok(HttpExportPage{data:HttpRows{columns,rows},next})
    }
    async fn finish_export(&self,_token:Option<&str>)->Result<()>{Ok(())}
}
