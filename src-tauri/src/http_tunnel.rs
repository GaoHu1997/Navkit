//! PL17 HTTPS transport. Only the SQL tunnel adapter exposes this resource to C1.
//! A reserved sequence is never retried; missing completion quarantines its session.
use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use parking_lot::Mutex;
use reqwest::{header::{HeaderMap, HeaderName, HeaderValue, AUTHORIZATION, CONTENT_TYPE}, Method};
use serde::{de::DeserializeOwned, Serialize};
use std::{collections::HashMap, sync::{Arc, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;
use zeroize::Zeroizing;
use crate::{error::{AppError, CmdResult}, execution::Entry, models::{MultiResult, ResultColumn, ResultSet}, tls_options::{TlsMode, TlsOptions}};
use crate::http_tunnel_protocol as wire;

pub(crate) struct TunnelConfig {
    pub endpoint: String, pub expected_server_id: String, pub expected_target_id: String,
    pub target_alias: String, pub connect_timeout_ms: u32, pub tls: TlsOptions,
    pub bearer: Zeroizing<String>,
}
struct Transport { endpoint: url::Url, work: reqwest::Client, control: reqwest::Client }
fn client(config: &TunnelConfig, owner: &str, control: bool) -> Result<reqwest::Client> {
    if config.tls.mode != TlsMode::VerifyFull { bail!("HTTP_TUNNEL_TLS_REQUIRED: 隧道必须验证CA和原始服务主机名"); }
    if !(1_000..=60_000).contains(&config.connect_timeout_ms) || config.bearer.is_empty()
        || config.bearer.len() > 16_384 || config.bearer.chars().any(char::is_control) { bail!("HTTP_TUNNEL_CONFIG_INVALID"); }
    let mut headers=HeaderMap::new();
    let secret=Zeroizing::new(format!("Bearer {}",config.bearer.as_str()));
    let mut auth=HeaderValue::from_str(&secret).map_err(|_|anyhow!("HTTP_TUNNEL_TOKEN_INVALID"))?; auth.set_sensitive(true);
    headers.insert(AUTHORIZATION,auth); headers.insert(HeaderName::from_bytes(wire::PROTOCOL_HEADER.as_bytes())?,HeaderValue::from_static(wire::PROTOCOL));
    headers.insert(HeaderName::from_bytes(wire::OWNER_HEADER.as_bytes())?,HeaderValue::from_str(owner)?);
    let mut builder=wire::client_builder().default_headers(headers).connect_timeout(Duration::from_millis(config.connect_timeout_ms as u64))
        .read_timeout(Duration::from_secs(if control { 10 } else { 310 }));
    let material=config.tls.material();
    if let Some(ca)=&material.ca {
        let certs=crate::tls_options::material::certificates(ca)?.iter().map(|cert|reqwest::Certificate::from_der(cert.as_ref())).collect::<std::result::Result<Vec<_>,_>>()?;
        builder=builder.tls_certs_only(certs);
    }
    if let (Some(cert),Some(key))=(&material.cert,&material.key) {
        let mut pem=Zeroizing::new(cert.clone()); pem.push(b'\n'); pem.extend_from_slice(key); builder=builder.identity(reqwest::Identity::from_pem(&pem)?);
    }
    builder.build().map_err(|_|anyhow!("HTTP_TUNNEL_TLS_CLIENT: 无法建立验证身份的HTTPS客户端"))
}
impl Transport {
    fn request(&self, method: Method, path:&str, control:bool)->Result<reqwest::RequestBuilder> {
        if !path.starts_with("/v1/") || path.contains(['?','#']) { bail!("HTTP_TUNNEL_PATH_INVALID"); }
        let target=self.endpoint.join(path)?;
        if target.origin()!=self.endpoint.origin() { bail!("HTTP_TUNNEL_ORIGIN_CHANGED"); }
        Ok(if control { &self.control } else { &self.work }.request(method,target))
    }
    async fn json<T:DeserializeOwned>(&self, method:Method, path:&str, body:Option<&impl Serialize>, control:bool)->Result<T> {
        let mut request=self.request(method,path,control)?;
        if let Some(body)=body { let bytes=serde_json::to_vec(body)?; if bytes.len()>wire::MAX_REQUEST_BYTES { bail!("HTTP_TUNNEL_REQUEST_LIMIT"); } request=request.header(CONTENT_TYPE,wire::JSON_TYPE).body(bytes); }
        let mut response=request.send().await.map_err(|_|anyhow!("HTTP_TUNNEL_TRANSPORT: HTTPS请求没有取得已验证回执"))?;
        let status=response.status().as_u16(); let content=response.headers().get(CONTENT_TYPE).and_then(|h|h.to_str().ok()).unwrap_or("").to_owned();
        // Reject status/media type before reading an untrusted HTML/redirect body.
        if !(200..300).contains(&status) || content.split(';').next().unwrap_or("").trim()!=wire::JSON_TYPE { bail!("HTTP_TUNNEL_RESPONSE: HTTP {status}，没有取得v1 JSON回执；不会跟随重定向或重试SQL"); }
        let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await.map_err(|_|anyhow!("HTTP_TUNNEL_TRANSPORT: JSON回执未完整读取"))? {
            if bytes.len().saturating_add(chunk.len())>wire::MAX_FRAME_BYTES { bail!("HTTP_TUNNEL_RESPONSE_LIMIT"); } bytes.extend_from_slice(&chunk);
        }
        serde_json::from_value(wire::decode_http_json(status,&content,&bytes)?).map_err(|_|anyhow!("HTTP_TUNNEL_PROTOCOL: JSON回执字段不符合已冻结协议"))
    }
}

pub(crate) struct TunnelRuntime {
    transport:Arc<Transport>, pub capabilities:wire::CapabilityResponse,
    expected_target_id:String, target_alias:String, owner:String,
    sessions:AsyncMutex<HashMap<(String,bool),Arc<RemoteSession>>>, closing:AtomicBool,
}
impl TunnelRuntime {
    pub async fn connect(config:TunnelConfig)->Result<Arc<Self>> {
        let endpoint=wire::validate_https_endpoint(&config.endpoint)?; let owner=Uuid::new_v4().to_string();
        let transport=Arc::new(Transport{endpoint,work:client(&config,&owner,false)?,control:client(&config,&owner,true)?});
        let capabilities:wire::CapabilityResponse=transport.json(Method::GET,wire::CAPABILITIES_PATH,None::<&()>,true).await?;
        capabilities.validate(&config.expected_server_id)?;
        let target=capabilities.targets.iter().find(|target|target.alias==config.target_alias && target.target_id==config.expected_target_id)
            .ok_or_else(||anyhow!("HTTP_TUNNEL_TARGET_CHANGED: 服务未授权配置的目标别名和身份"))?;
        if target.product!="H2" || target.product_version.split_whitespace().next()!=Some("2.3.232") { bail!("HTTP_TUNNEL_TARGET_UNCERTIFIED: 当前仅已认证H2 2.3.232参考服务"); }
        Ok(Arc::new(Self{transport,capabilities,expected_target_id:config.expected_target_id,target_alias:config.target_alias,owner,sessions:AsyncMutex::new(HashMap::new()),closing:AtomicBool::new(false)}))
    }
    pub fn default_namespace(&self)->&str { &self.capabilities.targets.iter().find(|t|t.target_id==self.expected_target_id).expect("negotiated target").namespaces[0] }
    pub fn namespaces(&self)->Vec<String> { self.capabilities.targets.iter().find(|t|t.target_id==self.expected_target_id).expect("negotiated target").namespaces.clone() }
    pub async fn session(&self,namespace:&str,read_only:bool)->Result<Arc<RemoteSession>> {
        if self.closing.load(Ordering::Acquire) { bail!("HTTP_TUNNEL_CLOSING"); }
        let namespace=if namespace.is_empty(){self.default_namespace()}else{namespace};
        if !self.namespaces().iter().any(|n|n==namespace) { bail!("HTTP_TUNNEL_NAMESPACE_FORBIDDEN"); }
        let mut sessions=self.sessions.lock().await; let key=(namespace.to_owned(),read_only);
        if let Some(session)=sessions.get(&key) { if session.poisoned.load(Ordering::Acquire)||session.closed.load(Ordering::Acquire){bail!("HTTP_TUNNEL_SESSION_QUARANTINED: 需要明确断开并重新连接，禁止重试原SQL");}return Ok(session.clone()); }
        if sessions.len()>=64 {bail!("HTTP_TUNNEL_SESSION_LIMIT");}
        let request=wire::OpenSessionRequest{protocol:wire::PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),owner_id:self.owner.clone(),target_alias:self.target_alias.clone(),expected_server_id:self.capabilities.server_id.clone(),expected_target_id:self.expected_target_id.clone(),namespace:namespace.into(),read_only};
        let response:wire::OpenSessionResponse=self.transport.json(Method::POST,wire::SESSIONS_PATH,Some(&request),true).await?;response.validate(&request,&self.capabilities)?;
        let session=Arc::new(RemoteSession{transport:self.transport.clone(),binding:response.binding,catalog:response.catalog,sequence:AsyncMutex::new(0),poisoned:AtomicBool::new(false),closed:AtomicBool::new(false)});
        if self.closing.load(Ordering::Acquire) { session.drain_until_closed().await; bail!("HTTP_TUNNEL_CLOSING"); }
        sessions.insert(key,session.clone());Ok(session)
    }
    pub async fn shutdown(&self) {
        self.closing.store(true,Ordering::Release);
        let sessions:Vec<_>=self.sessions.lock().await.values().cloned().collect();
        for session in sessions { session.drain_until_closed().await; }
        self.sessions.lock().await.clear();
    }
    pub async fn run_owned(self:Arc<Self>,namespace:String,sql:String,entry:Arc<Entry>,read_only:bool,lease:crate::manual_transaction::Lease)->(CmdResult<MultiResult>,bool) {
        let working=entry.clone();
        crate::execution::supervise(entry,async move {
            let _lease=lease;
            let session=match self.session(&namespace,read_only).await {Ok(session)=>session,Err(_)=>return(Err(AppError::coded("HTTP_TUNNEL_SESSION_UNAVAILABLE","没有取得已验证的隧道会话；SQL未发送")),false)};
            let sequence=tokio::select!{lock=session.sequence.lock()=>lock,_=working.wait_stop()=>return(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","等待隧道会话期间取消；SQL未发送")),true)};
            if !working.start(){return(Err(AppError::coded("EXECUTION_CANCELLED_BEFORE_SEND","发送前已停止；SQL未发送")),true);}
            let timeout=working.status().deadline.budget_ms.unwrap_or(60_000).clamp(1,300_000) as u32;
            let prepared=match session.prepare(sql,Vec::new(),timeout,sequence).await {Ok(request)=>request,Err(_)=>return(Err(AppError::coded("HTTP_TUNNEL_REQUEST_REJECTED","隧道请求验证失败；SQL未发送")),false)};
            let request=prepared.request.clone();let state=Arc::new(Mutex::new(None::<RemoteFinal>));let observed=state.clone();
            working.record_remote_transaction("unknown");
            let task=session.stream(&request,Some(&working),&observed);
            let control=||session.cancel(&request);
            let (result,_)=crate::execution::drive(task,&working,control).await;
            let final_state=state.lock().clone();
            if let Some(final_state)=&final_state { working.record_remote_transaction(&final_state.transaction); }
            let confirmed=working.stopping()&&final_state.as_ref().is_some_and(|f|f.state=="cancelled"&&f.transaction=="rolled_back");
            // The session mutex remains owned while its work AND independent control drain.
            if final_state.as_ref().map_or(true,|f|f.state=="unknown"||f.poisoned) || result.is_err()&&final_state.is_none() {
                session.poisoned.store(true,Ordering::Release);session.drain_until_closed().await;
            }
            drop(prepared);
            (result.map_err(|error|if final_state.as_ref().map_or(true,|f|f.state=="unknown"){AppError::coded("EXECUTION_EFFECTS_UNKNOWN","HTTPS执行回执缺失或未知；原SQL禁止重放")}else{AppError::from(error)}),confirmed)
        }).await
    }
}
struct PreparedExecution<'a>{request:wire::ExecuteRequest,_sequence:tokio::sync::MutexGuard<'a,u64>}
#[derive(Clone)]
struct RemoteFinal{state:String,transaction:String,poisoned:bool}
pub(crate) struct RemoteSession{transport:Arc<Transport>,pub binding:wire::SessionBinding,pub catalog:String,sequence:AsyncMutex<u64>,poisoned:AtomicBool,closed:AtomicBool}
impl RemoteSession {
    async fn prepare<'a>(&self,sql:String,binds:Vec<wire::TunnelValue>,timeout_ms:u32,mut sequence:tokio::sync::MutexGuard<'a,u64>)->Result<PreparedExecution<'a>> {
        if self.poisoned.load(Ordering::Acquire)||self.closed.load(Ordering::Acquire){bail!("HTTP_TUNNEL_SESSION_QUARANTINED");}
        let next=sequence.checked_add(1).ok_or_else(||anyhow!("HTTP_TUNNEL_SEQUENCE_EXHAUSTED"))?;
        let request=wire::ExecuteRequest{protocol:wire::PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),binding:self.binding.clone(),execution_id:wire::execution_id(&self.binding,&next.to_string())?,sequence:next.to_string(),sql,binds,timeout_ms};request.validate()?;
        *sequence=next;Ok(PreparedExecution{request,_sequence:sequence})
    }
    async fn stream(&self,request:&wire::ExecuteRequest,entry:Option<&Arc<Entry>>,observed:&Mutex<Option<RemoteFinal>>)->Result<MultiResult> {
        let started=Instant::now();let mut validator=wire::StreamValidator::new(request)?;
        let builder=self.transport.request(Method::POST,&wire::executions_path(&self.binding.session_id)?,false)?.header(CONTENT_TYPE,wire::JSON_TYPE).body(serde_json::to_vec(request)?);
        let mut context=crate::sql_history::current_context();context.database=self.catalog.clone();context.namespace=request.binding.namespace.clone();context.operation_id=Some(request.execution_id.clone());
        let parameters=request.binds.iter().enumerate().map(|(i,value)|crate::models::SqlLogParameter{ordinal:i+1,type_name:"TunnelValue".into(),value:serde_json::to_string(value).unwrap_or_default(),redacted:false}).collect();
        let mut history=crate::sql_history::Guard::in_context(&request.sql,parameters,context);
        let result:Result<MultiResult>=async {
        let mut response=builder.send().await.map_err(|_|anyhow!("EXECUTION_EFFECTS_UNKNOWN: 隧道发送后未取得完整回执"))?;
        wire::validate_stream_content_type(response.status().as_u16(),response.headers().get(CONTENT_TYPE).and_then(|v|v.to_str().ok()).unwrap_or(""))?;
        let mut parser=FrameLines::default();let mut columns=Vec::new();let mut rows=Vec::new();let mut affected=0;let mut rows_read=0;let mut terminal_error=None;
        let mut final_value=None;
        let mut target:Option<Arc<crate::query_spool::SpoolSet>>=None;let cache=entry.and_then(|entry|entry.spool());
        while let Some(chunk)=response.chunk().await.map_err(|_|anyhow!("EXECUTION_EFFECTS_UNKNOWN: 隧道流中断；禁止重放"))? {
            for line in parser.feed(&chunk)? {
                let frame=validator.push_json(std::str::from_utf8(&line).map_err(|_|anyhow!("HTTP_TUNNEL_UTF8_INVALID"))?)?;
                match frame {
                    wire::StreamFrame::Header{columns:actual,..}=>{columns=actual.iter().map(|c|ResultColumn{name:c.name.clone(),data_type:c.data_type.clone(),nullable:c.nullable.unwrap_or(true),primary_key:false,auto_increment:false}).collect();if let Some(cache)=&cache{let names=actual.iter().map(|c|c.name.clone()).collect::<Vec<_>>();let types=actual.iter().map(|c|c.data_type.clone()).collect::<Vec<_>>();target=Some(cache.begin_set(0,&names,&types));}},
                    wire::StreamFrame::Row{values,..}=>{let display=values.iter().map(display_value).collect::<Result<Vec<_>>>()?;if let Some(target)=&target{target.push(&display);}rows.push(display);},
                    wire::StreamFrame::Progress{..}=>{},
                    wire::StreamFrame::Final{state,transaction,affected_rows,rows_read:count,connection_poisoned,code,..}=>{rows_read=wire::decimal_count(&count)?;affected=affected_rows.as_deref().map(wire::decimal_count).transpose()?.unwrap_or(0);final_value=Some(RemoteFinal{state:state.clone(),transaction:transaction.clone(),poisoned:connection_poisoned});if state!="succeeded"{terminal_error=Some(if state=="unknown"{AppError::coded("EXECUTION_EFFECTS_UNKNOWN","远端提交状态未知；禁止重放SQL")}else{AppError::coded(&code.filter(|c|c.len()<=80&&c.bytes().all(|b|b.is_ascii_uppercase()||b.is_ascii_digit()||b==b'_')).unwrap_or_else(||"HTTP_TUNNEL_EXECUTION_FAILED".into()),"远端确认执行未成功；请核对执行状态")});}},
                }
            }
        }
        parser.finish()?;validator.finish()?;
        // Publish terminal evidence only after strict EOF: a valid final followed by
        // another frame, truncation or transport failure is still Unknown.
        *observed.lock()=final_value;
        if let Some(error)=terminal_error{return Err(error.into());}
        if let Some(target)=target{target.finish();}if let Some(cache)=&cache{cache.finish_all();}
        let elapsed=started.elapsed().as_millis() as u64;let has_rows=!columns.is_empty();
        Ok(MultiResult{sets:vec![ResultSet{columns,rows,affected,elapsed_ms:elapsed,ok:true,message:String::new(),has_rows,primary_keys:vec![],total:rows_read}],messages:vec![],elapsed_ms:elapsed,spool_id:cache.map(|c|c.id.clone())})
        }.await;
        if observed.lock().is_some(){history.result(&result,|r|r.sets.iter().map(|s|if s.has_rows{s.rows.len() as i64}else{s.affected as i64}).sum());}
        result
    }
    pub async fn execute(&self,sql:String,binds:Vec<wire::TunnelValue>,timeout_ms:u32)->Result<MultiResult> {
        let prepared=self.prepare(sql,binds,timeout_ms,self.sequence.lock().await).await?;let observed=Mutex::new(None);
        let result=self.stream(&prepared.request,None,&observed).await;
        if result.is_err()&&observed.lock().as_ref().map_or(true,|f|f.state=="unknown"||f.poisoned){self.poisoned.store(true,Ordering::Release);self.drain_until_closed().await;}
        result
    }
    pub async fn catalog(&self)->Result<Vec<wire::CatalogTable>> {
        let _sequence=self.sequence.lock().await;
        if self.poisoned.load(Ordering::Acquire)||self.closed.load(Ordering::Acquire){bail!("HTTP_TUNNEL_SESSION_QUARANTINED");}
        let request=wire::BoundRequest{protocol:wire::PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),binding:self.binding.clone()};
        let reply:wire::CatalogResponse=self.transport.json(Method::POST,&wire::catalog_path(&self.binding.session_id)?,Some(&request),true).await?;
        reply.validate(&request)?;Ok(reply.tables)
    }
    pub async fn metadata(&self,object:wire::ObjectId)->Result<crate::http_db::relational::JdbcMetadataSnapshot> {
        let _sequence=self.sequence.lock().await;if self.poisoned.load(Ordering::Acquire)||self.closed.load(Ordering::Acquire){bail!("HTTP_TUNNEL_SESSION_QUARANTINED");}
        let request=wire::MetadataRequest{protocol:wire::PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),binding:self.binding.clone(),object};
        let reply:wire::MetadataResponse=self.transport.json(Method::POST,&wire::metadata_path(&self.binding.session_id)?,Some(&request),true).await?;reply.validate(&request)?;Ok(reply.snapshot)
    }
    async fn cancel(&self,request:&wire::ExecuteRequest)->Result<()> {
        let body=wire::BoundRequest{protocol:wire::PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),binding:self.binding.clone()};
        let reply:wire::CancelResponse=self.transport.json(Method::POST,&wire::cancel_path(&request.execution_id)?,Some(&body),true).await?;
        if reply.protocol!=wire::PROTOCOL||reply.request_id!=body.request_id||reply.binding!=self.binding||reply.execution_id!=request.execution_id||reply.sequence!=request.sequence||!["running","succeeded","failed","cancelled","unknown"].contains(&reply.state.as_str()){bail!("HTTP_TUNNEL_CANCEL_IDENTITY_CHANGED");}Ok(())
    }
    pub async fn status(&self,request:&wire::ExecuteRequest)->Result<wire::StatusResponse> {
        let reply:wire::StatusResponse=self.transport.json(Method::GET,&wire::status_path(&request.execution_id)?,None::<&()>,true).await?;reply.validate(request)?;Ok(reply)
    }
    async fn drain_until_closed(&self) {
        if self.closed.load(Ordering::Acquire) { return; }
        loop {
            let request=wire::BoundRequest{protocol:wire::PROTOCOL.into(),request_id:Uuid::new_v4().to_string(),binding:self.binding.clone()};
            let reply=self.transport.json::<wire::DrainResponse>(Method::DELETE,&wire::session_path(&self.binding.session_id).expect("validated session UUID"),Some(&request),true).await;
            if let Ok(reply)=reply {if reply.validate(&request).is_ok()&&reply.state=="closed"{self.closed.store(true,Ordering::Release);return;}}
            // Owned background cleanup retains C1/connection leases until the server proves drain.
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}
#[derive(Default)]
struct FrameLines{pending:Vec<u8>,total:usize}
impl FrameLines {
    fn feed(&mut self,bytes:&[u8])->Result<Vec<Vec<u8>>> {
        self.total=self.total.checked_add(bytes.len()).ok_or_else(||anyhow!("HTTP_TUNNEL_RESULT_LIMIT"))?;if self.total>wire::MAX_RESULT_BYTES{bail!("HTTP_TUNNEL_RESULT_LIMIT");}
        let mut output=Vec::new();for part in bytes.split_inclusive(|b|*b==b'\n') {if self.pending.len().saturating_add(part.len())>wire::MAX_FRAME_BYTES+1{bail!("HTTP_TUNNEL_FRAME_LIMIT");}self.pending.extend_from_slice(part);if self.pending.last()==Some(&b'\n'){self.pending.pop();if self.pending.last()==Some(&b'\r'){self.pending.pop();}if self.pending.is_empty(){bail!("HTTP_TUNNEL_EMPTY_FRAME");}output.push(std::mem::take(&mut self.pending));}}Ok(output)
    }
    fn finish(&self)->Result<()> {if !self.pending.is_empty(){bail!("EXECUTION_EFFECTS_UNKNOWN: 隧道帧缺少结尾换行；禁止重放");}Ok(())}
}
fn display_value(value:&wire::TunnelValue)->Result<Option<String>> {
    use wire::TunnelValue::*;Ok(match value{Null=>None,Text(s)|Integer(s)|Decimal(s)|Date(s)|Time(s)|Timestamp(s)|Timestamptz(s)|Uuid(s)=>Some(s.clone()),Float(f)=>Some(f.to_string()),Bool(v)=>Some(if *v{"true"}else{"false"}.into()),Binary(s)=>Some(crate::values::format_binary(&STANDARD.decode(s)?))})
}
#[cfg(test)]
#[path = "http_tunnel_tests.rs"]
mod integration_tests;

#[cfg(test)]
#[path = "http_tunnel_production_tests.rs"]
mod production_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn ndjson_framing_handles_multibyte_and_every_chunk_boundary(){let text="{\"kind\":\"row\",\"value\":\"中文\"}\r\n{\"kind\":\"final\"}\n";for size in 1..=text.len(){let mut parser=FrameLines::default();let mut lines=Vec::new();for part in text.as_bytes().chunks(size){lines.extend(parser.feed(part).unwrap());}parser.finish().unwrap();assert_eq!(lines.len(),2);assert_eq!(std::str::from_utf8(&lines[0]).unwrap(),"{\"kind\":\"row\",\"value\":\"中文\"}");}}
    #[test]fn partial_empty_and_oversized_frames_never_become_success(){let mut parser=FrameLines::default();parser.feed(b"{}").unwrap();assert!(parser.finish().is_err());assert!(FrameLines::default().feed(b"\n").is_err());assert!(FrameLines::default().feed(&vec![b'x';wire::MAX_FRAME_BYTES+2]).is_err());}
    #[test]fn display_preserves_exact_values_and_null_binary(){assert_eq!(display_value(&wire::TunnelValue::Integer(i64::MAX.to_string())).unwrap(),Some("9223372036854775807".into()));assert_eq!(display_value(&wire::TunnelValue::Decimal("12345678901234567890.123456789".into())).unwrap(),Some("12345678901234567890.123456789".into()));assert_eq!(display_value(&wire::TunnelValue::Null).unwrap(),None);assert_eq!(display_value(&wire::TunnelValue::Binary("AP8BgA==".into())).unwrap(),Some("0x00FF0180".into()));}
}
