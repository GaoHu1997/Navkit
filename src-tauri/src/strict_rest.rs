//! Bounded, same-origin HTTP for credential-bearing native REST adapters.
//! Never follows a redirect, accepts invalid certificates, or puts credentials in a URL.
use anyhow::{anyhow, bail, Context, Result};
use reqwest::{header::{HeaderMap, ACCEPT, CONTENT_TYPE}, Method, Url};
use serde_json::Value;
use std::{collections::BTreeMap, net::IpAddr, time::Duration};
use crate::models::ConnectionRequest;

pub(crate) const MAX_BODY: usize = 16 * 1024 * 1024;
pub(crate) fn params(raw: &str, extra: &[&str]) -> Result<BTreeMap<String,String>> {
    let mut result = BTreeMap::new();
    let pairs: Vec<(String,String)> = if raw.trim().starts_with(['{','[']) {
        let v: Value = serde_json::from_str(raw).context("REST 连接参数不是有效 JSON")?;
        v.as_object().ok_or_else(|| anyhow!("REST 连接参数必须为 JSON 对象"))?.iter().map(|(k,v)| {
            let value = match v { Value::String(s)=>s.clone(),Value::Bool(b)=>b.to_string(),Value::Number(n)=>n.to_string(), _=>String::new() };
            if !matches!(v,Value::String(_)|Value::Bool(_)|Value::Number(_)) { return Err(anyhow!("REST 参数 {k} 必须为字符串、布尔或数字")); }
            Ok((k.clone(),value))
        }).collect::<Result<_>>()?
    } else {
        let mut pairs=Vec::new();
        for part in raw.split([';','&']).map(str::trim).filter(|p|!p.is_empty()) {
            let (k,v)=part.split_once('=').ok_or_else(||anyhow!("REST 连接参数应为 key=value 或 JSON 对象"))?;
            pairs.push((k.trim().into(),v.trim().into()));
        }
        pairs
    };
    for (key,value) in pairs {
        let key=key.to_ascii_lowercase();
        if !["url","ssl","tls","sslmode","ssl_verify"].contains(&key.as_str()) && !extra.contains(&key.as_str()) { bail!("不支持的 REST 参数：{key}；其它认证/驱动请使用显式自定义绑定"); }
        if value.contains('\0') || result.insert(key.clone(),value).is_some() { bail!("REST 参数重复或包含 NUL：{key}"); }
    }
    Ok(result)
}
fn bool_value(value:&str)->Result<bool> {match value.to_ascii_lowercase().as_str(){"true"|"1"|"require"|"verify-ca"|"verify-full"=>Ok(true),"false"|"0"|"disable"=>Ok(false),_=>bail!("REST TLS 参数无效")}}
pub(crate) fn endpoint(req:&ConnectionRequest, values:&BTreeMap<String,String>, port:u16, secure_default:bool)->Result<Url> {
    if !req.file.is_empty(){bail!("REST 连接不接受数据库文件路径");}
    let mut tls=None;
    for key in ["ssl","tls","sslmode"] {if let Some(v)=values.get(key){let v=bool_value(v)?;if tls.is_some_and(|old|old!=v){bail!("REST TLS 参数相互冲突");}tls=Some(v);}}
    if values.get("ssl_verify").map(|v|bool_value(v)).transpose()?.is_some_and(|v|!v){bail!("REST 连接必须验证服务器证书；拒绝关闭 TLS 验证");}
    let mut url=if let Some(raw)=values.get("url"){Url::parse(raw).context("REST url 无效")?}else{
        let host=req.host.trim();
        if host.is_empty()||host.chars().any(|c|c.is_whitespace()||matches!(c,'/'|'\\'|'?'|'#'|'@'|'%')){bail!("REST 主机必须是单独的主机名或 IP 地址");}
        let host=host.trim_start_matches('[').trim_end_matches(']');
        if host.contains(':')&&host.parse::<IpAddr>().is_err(){bail!("REST 主机不能包含端口或 URL");}
        let host=if host.contains(':'){format!("[{host}]")}else{host.into()};
        Url::parse(&format!("{}://{host}:{}",if tls.unwrap_or(secure_default){"https"}else{"http"},if req.port==0{port}else{req.port})).context("REST 地址无效")?
    };
    if !["https","http"].contains(&url.scheme())||url.host_str().is_none()||!url.username().is_empty()||url.password().is_some()||url.query().is_some()||url.fragment().is_some()||url.path()!="/"||url.port()==Some(0){bail!("REST 地址必须是无凭据、路径、查询或片段的 HTTP(S) origin");}
    if tls.is_some_and(|tls|tls!=(url.scheme()=="https")){bail!("REST url 与 TLS 参数冲突；拒绝静默降级");}
    if secure_default&&url.scheme()!="https"&&!literal_loopback(&url){bail!("此服务的 HTTP 明文端点仅允许字面回环 IP；云端必须 HTTPS");}
    url.set_path("/");Ok(url)
}
pub(crate) fn literal_loopback(url:&Url)->bool {url.host_str().is_some_and(|h|h.trim_matches(['[',']']).parse::<IpAddr>().is_ok_and(|ip|ip.is_loopback()))}
pub(crate) enum Auth { None, Bearer(String), Basic(String,String) }
#[derive(Clone)]
pub(crate) struct Session {pub base:Url,client:reqwest::Client,auth:std::sync::Arc<Auth>}
pub(crate) struct Reply {pub status:u16,pub headers:HeaderMap,pub body:Vec<u8>}
impl Reply {
    pub fn json(&self)->Result<Value>{serde_json::from_slice(&self.body).context("REST 响应不是有效 JSON（拒绝空响应、HTML 或伪造成功）")}
}
impl Session {
    pub fn new(base:Url,auth:Auth,seconds:u64)->Result<Self>{
        if matches!(&auth,Auth::Basic(_,_)|Auth::Bearer(_))&&base.scheme()!="https"&&!literal_loopback(&base){bail!("携带凭据的 REST 连接必须 HTTPS；HTTP 仅允许字面回环 IP");}
        let mut builder=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(seconds.clamp(1,300))).connect_timeout(Duration::from_secs(seconds.clamp(1,10)));
        if literal_loopback(&base){builder=builder.no_proxy();}
        Ok(Self{base,client:builder.build()?,auth:std::sync::Arc::new(auth)})
    }
    pub fn path(&self,parts:&[&str])->Result<Url>{let mut url=self.base.clone();{let mut p=url.path_segments_mut().map_err(|_|anyhow!("REST origin 不支持路径"))?;p.clear();for part in parts {if part.is_empty()||matches!(*part,"."|"..")||part.contains('\0'){bail!("REST 路径段无效");}p.push(part);}}Ok(url)}
    pub fn same_origin(&self,url:&Url)->bool{self.base.origin()==url.origin()&&url.username().is_empty()&&url.password().is_none()&&url.fragment().is_none()}
    pub fn redact(&self,text:&str)->String{let mut s=text.to_string();match self.auth.as_ref(){Auth::Bearer(t)=>{if !t.is_empty(){s=s.replace(t,"[REDACTED]");}},Auth::Basic(u,p)=>{if !p.is_empty(){s=s.replace(p,"[REDACTED]");}use base64::Engine;s=s.replace(&base64::engine::general_purpose::STANDARD.encode(format!("{u}:{p}")),"[REDACTED]");},Auth::None=>{}}s.chars().take(2000).collect()}
    pub async fn exchange(&self,method:Method,url:Url,body:Option<&Value>)->Result<Reply>{
        if !self.same_origin(&url){bail!("拒绝向跨 origin 地址发送 REST 请求或凭据");}
        let mut req=self.client.request(method,url).header(ACCEPT,"application/json");
        match self.auth.as_ref(){Auth::None=>{},Auth::Bearer(token)=>{req=req.bearer_auth(token);},Auth::Basic(user,pass)=>{req=req.basic_auth(user,Some(pass));}}
        if let Some(body)=body{req=req.header(CONTENT_TYPE,"application/json").json(body);}
        let mut response=req.send().await.map_err(|e|anyhow!("REST 传输失败：{}",self.redact(&format!("{e:?}"))))?;
        let status=response.status().as_u16();let headers=response.headers().clone();
        if response.content_length().is_some_and(|n|n>MAX_BODY as u64){bail!("REST 响应超过 16 MiB；未截断为成功结果");}
        let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await.map_err(|e|anyhow!("REST 响应读取失败：{}",self.redact(&format!("{e:?}"))))?{if bytes.len()+chunk.len()>MAX_BODY{bail!("REST 响应超过 16 MiB；未截断为成功结果");}bytes.extend_from_slice(&chunk);}
        Ok(Reply{status,headers,body:bytes})
    }
    pub fn check(&self,reply:&Reply,allowed:&[u16])->Result<()>{if !allowed.contains(&reply.status){bail!("REST HTTP {}：{}",reply.status,self.redact(&String::from_utf8_lossy(&reply.body)));}Ok(())}
    pub async fn json(&self,method:Method,url:Url,body:Option<&Value>)->Result<Value>{let reply=self.exchange(method,url,body).await?;self.check(&reply,&[200])?;reply.json()}
}
pub(crate) fn fields(value:&Value,allowed:&[&str])->Result<()>{let obj=value.as_object().ok_or_else(||anyhow!("REST 命令必须为 JSON 对象"))?;for key in obj.keys(){if !allowed.contains(&key.as_str()){bail!("REST 命令含未知字段：{key}");}}Ok(())}
pub(crate) fn string<'a>(value:&'a Value,key:&str)->Result<&'a str>{value.get(key).and_then(Value::as_str).ok_or_else(||anyhow!("REST 响应/命令缺少字符串字段 {key}"))}
pub(crate) fn identifier(value:&str)->Result<&str>{if value.is_empty()||value.len()>255||!value.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_')||value.as_bytes()[0].is_ascii_digit(){bail!("REST 对象/字段标识符无效");}Ok(value)}
