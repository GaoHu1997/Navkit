//! Cassandra CQL 与 NebulaGraph v3 Thrift：真实认证、目录、分页与语句执行。
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use tokio::{sync::Mutex,time::timeout};
use crate::{http_db::{HttpAdapter,HttpExportPage,HttpRows},models::ConnectionRequest,native_db::{Params,single,text_bytes,cell}};
use scylla::{Session,SessionBuilder,frame::response::result::CqlValue};
fn qi(s:&str)->String{format!("\"{}\"",s.replace('"',"\"\""))}
fn qn(s:&str)->String{format!("`{}`",s.replace('`',"``"))}
fn cv(v:CqlValue)->String{match v {CqlValue::Ascii(s)|CqlValue::Text(s)=>s,CqlValue::Blob(b)=>text_bytes(&b),CqlValue::Int(v)=>v.to_string(),CqlValue::BigInt(v)=>v.to_string(),CqlValue::SmallInt(v)=>v.to_string(),CqlValue::TinyInt(v)=>v.to_string(),CqlValue::Boolean(v)=>v.to_string(),CqlValue::Double(v)=>v.to_string(),CqlValue::Float(v)=>v.to_string(),CqlValue::Uuid(v)=>v.to_string(),CqlValue::Inet(v)=>v.to_string(),v=>format!("{v:?}")}}
/// Cassandra paging state 是不透明二进制；导出游标用十六进制字符串承载，
/// 不引入额外依赖，也不把它误解成数字 offset。
fn hex_encode(b:&[u8])->String{use std::fmt::Write as _;let mut s=String::with_capacity(b.len()*2);for x in b{let _=write!(s,"{x:02x}");}s}
fn hex_val(c:u8)->Option<u8>{match c{b'0'..=b'9'=>Some(c-b'0'),b'a'..=b'f'=>Some(c-b'a'+10),b'A'..=b'F'=>Some(c-b'A'+10),_=>None}}
fn hex_decode(s:&str)->Result<Vec<u8>>{let s=s.trim();if s.len()%2!=0{return Err(anyhow!("Cassandra 导出游标不是合法十六进制"));}let b=s.as_bytes();let mut v=Vec::with_capacity(b.len()/2);let mut i=0;while i<b.len(){let hi=hex_val(b[i]).ok_or_else(||anyhow!("Cassandra 导出游标不是合法十六进制"))?;let lo=hex_val(b[i+1]).ok_or_else(||anyhow!("Cassandra 导出游标不是合法十六进制"))?;v.push(hi*16+lo);i+=2;}Ok(v)}
pub(crate) struct CassandraDb{session:Session,p:Params,database:String}
impl CassandraDb {
 pub async fn connect(r:&ConnectionRequest)->Result<Self>{let p=Params::new(r);p.no_tls("Cassandra")?;let mut b=SessionBuilder::new().known_node(format!("{}:{}",r.host,if r.port==0{9042}else{r.port})).connection_timeout(p.timeout);if !r.user.is_empty(){b=b.user(&r.user,&r.password);}
 let session=timeout(p.timeout,b.build()).await.map_err(|_|anyhow!("Cassandra 连接超时"))??;if !r.database.is_empty(){session.use_keyspace(&r.database,true).await?;}Ok(Self{session,p,database:r.database.clone()})}
 async fn query(&self,sql:&str)->Result<HttpRows>{timeout(self.p.timeout,async{let mut state=None;let mut rows=vec![];let mut columns=vec![];loop {let r=self.session.query_paged(sql,(),state).await?;if columns.is_empty(){columns=r.col_specs.iter().map(|c|c.name.clone()).collect();}for row in r.rows.unwrap_or_default(){rows.push(row.columns.into_iter().map(|c|c.map(cv)).collect());if rows.len()>10000{return Err(anyhow!("Cassandra 结果超过10000行，请加 LIMIT"));}}state=r.paging_state;if state.is_none(){break;}}Ok(HttpRows{columns,rows})}).await.map_err(|_|anyhow!("Cassandra 查询超时"))?}
 /// 真正的 paging-state 分页：每页只取 page_size 行，游标是服务端 paging state
 /// 的十六进制编码；结束只看 paging_state 是否缺失，短页绝不等于 EOF。
 async fn export_paged(&self,sql:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{
  let raw=match token{None=>None,Some(t)=>{let v=hex_decode(t)?;if v.is_empty(){None}else{Some(v)}}};
  let page_size=limit.clamp(1,5000) as i32;
  let query=scylla::statement::query::Query::new(sql).with_page_size(page_size);
  let r=timeout(self.p.timeout,self.session.query_paged(query,(),raw.map(Into::into))).await.map_err(|_|anyhow!("Cassandra 导出一页超时"))??;
  let columns=r.col_specs.iter().map(|c|c.name.clone()).collect();
  let rows=r.rows.unwrap_or_default().into_iter().map(|row|row.columns.into_iter().map(|c|c.map(cv)).collect()).collect();
  let next=r.paging_state.map(|b|hex_encode(&b));
  Ok(HttpExportPage{data:HttpRows{columns,rows},next})
 }
}
#[async_trait]impl HttpAdapter for CassandraDb {
 async fn ping(&self)->Result<String>{let r=self.query("SELECT release_version FROM system.local").await?;Ok(r.rows.first().and_then(|r|r.first()).and_then(Clone::clone).ok_or_else(||anyhow!("Cassandra 未返回 release_version"))?)}
 async fn containers(&self)->Result<Vec<(String,String)>>{Ok(self.query("SELECT keyspace_name FROM system_schema.keyspaces").await?.rows.into_iter().filter_map(|r|r.into_iter().next().flatten()).map(|s|(s,String::new())).collect())}
 async fn objects(&self,c:&str)->Result<Vec<String>>{let c=c.replace('\'',"''");Ok(self.query(&format!("SELECT table_name FROM system_schema.tables WHERE keyspace_name='{c}'")).await?.rows.into_iter().filter_map(|r|r.into_iter().next().flatten()).collect())}
 async fn rows(&self,c:&str,o:&str,offset:u64,limit:u64)->Result<HttpRows>{let n=offset.saturating_add(limit as u64);if n>10000{return Err(anyhow!("Cassandra 浏览窗口最多10000行"));}let mut r=self.query(&format!("SELECT * FROM {}.{} LIMIT {n}",qi(c),qi(o))).await?;r.rows=r.rows.into_iter().skip(offset as usize).take(limit as usize).collect();Ok(r)}
 async fn export_page(&self,c:&str,o:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{if limit==0{return Ok(HttpExportPage{data:HttpRows{columns:vec![],rows:vec![]},next:None});}let sql=format!("SELECT * FROM {}.{}",qi(c),qi(o));self.export_paged(&sql,token,limit).await}
 async fn run(&self,c:&str,_o:&str,sql:&str)->Result<HttpRows>{if !c.is_empty()&&c!=self.database{self.session.use_keyspace(c,true).await?;}self.query(sql).await}
}
use nebula_client::v3::{GraphClient,GraphSession,GraphTransportResponseHandler};
use fbthrift_transport::{AsyncTransport,AsyncTransportConfiguration,impl_tokio::{TokioTcpStream,TokioSleep}};
type NT=AsyncTransport<TokioTcpStream,TokioSleep,GraphTransportResponseHandler>;
pub(crate) struct NebulaDb{session:Mutex<GraphSession<NT>>,p:Params}
fn nebula_json(b:&[u8])->Result<HttpRows>{let v:serde_json::Value=serde_json::from_slice(b)?;let errors=v.get("errors").and_then(|v|v.as_array()).ok_or_else(||anyhow!("Nebula response missing errors"))?;for e in errors {if e["code"].as_i64()!=Some(0){return Err(anyhow!("Nebula {}: {}",e["code"],e["message"]));}}
 let result=v["results"].as_array().and_then(|a|a.last());if let Some(r)=result{let columns=r["columns"].as_array().map(|a|a.iter().map(|v|v.as_str().unwrap_or("").to_owned()).collect()).unwrap_or_default();let mut rows=vec![];if let Some(a)=r["data"].as_array(){for item in a{let row=item.get("row").unwrap_or(item).as_array().ok_or_else(||anyhow!("Nebula malformed row"))?;rows.push(row.iter().map(cell).collect());}}Ok(HttpRows{columns,rows})}else{Ok(single("status","SUCCEEDED"))}}
impl NebulaDb {
 pub async fn connect(r:&ConnectionRequest)->Result<Self>{let p=Params::new(r);p.no_tls("NebulaGraph")?;let mut config=AsyncTransportConfiguration::new(GraphTransportResponseHandler);config.set_max_buf_size(16*1024*1024);config.set_read_timeout(p.timeout.as_millis() as u32);let t=timeout(p.timeout,NT::with_tokio_tcp_connect((r.host.as_str(),if r.port==0{9669}else{r.port}),config)).await.map_err(|_|anyhow!("NebulaGraph连接超时"))??;let s=timeout(p.timeout,GraphClient::new(t).authenticate(&r.user.as_bytes().to_vec(),&r.password.as_bytes().to_vec())).await.map_err(|_|anyhow!("NebulaGraph认证超时"))??;let db=Self{session:Mutex::new(s),p};if !r.database.is_empty(){db.query(&format!("USE {}",qn(&r.database))).await?;}Ok(db)}
 async fn query(&self,s:&str)->Result<HttpRows>{let mut lock=self.session.lock().await;let b=timeout(self.p.timeout,lock.execute_json(&s.as_bytes().to_vec())).await.map_err(|_|anyhow!("NebulaGraph查询超时；请重连"))??;nebula_json(&b)}
 async fn in_space(&self,c:&str,s:&str)->Result<HttpRows>{let s=if c.is_empty(){s.to_string()}else{format!("USE {}; {s}",qn(c))};self.query(&s).await}
}
#[async_trait]impl HttpAdapter for NebulaDb {
 async fn ping(&self)->Result<String>{self.query("SHOW SPACES").await?;Ok("NebulaGraph v3 (Thrift authenticated)".into())}
 async fn containers(&self)->Result<Vec<(String,String)>>{Ok(self.query("SHOW SPACES").await?.rows.into_iter().filter_map(|r|r.into_iter().next().flatten()).map(|s|(s,String::new())).collect())}
 async fn objects(&self,c:&str)->Result<Vec<String>>{let mut out=vec![];for(op,prefix) in [("SHOW TAGS","tag:"),("SHOW EDGES","edge:")] {for row in self.in_space(c,op).await?.rows{if let Some(Some(n))=row.first(){out.push(format!("{prefix}{n}"));}}}Ok(out)}
 async fn rows(&self,c:&str,o:&str,offset:u64,limit:u64)->Result<HttpRows>{let s=if let Some(tag)=o.strip_prefix("tag:"){format!("MATCH (v:{}) RETURN v SKIP {offset} LIMIT {limit}",qn(tag))}else if let Some(edge)=o.strip_prefix("edge:"){format!("MATCH ()-[e:{}]->() RETURN e SKIP {offset} LIMIT {limit}",qn(edge))}else{return Err(anyhow!("Nebula对象须为tag:名称或edge:名称"));};self.in_space(c,&s).await}
 async fn run(&self,c:&str,_o:&str,s:&str)->Result<HttpRows>{self.in_space(c,s).await}
}
#[cfg(test)]mod tests {use super::*;#[test]fn nebula_error_and_utf8(){assert!(nebula_json(br#"{"errors":[{"code":-1004,"message":"denied"}],"results":[]}"#).is_err());let r=nebula_json("{\"errors\":[{\"code\":0}],\"results\":[{\"columns\":[\"x\"],\"data\":[{\"row\":[\"中文\",null]}]}]}".as_bytes()).unwrap();assert_eq!(r.rows[0],vec![Some("中文".into()),None]);}#[test]fn paging_token_hex_roundtrip(){let raw=vec![0u8,1,254,255,16];let enc=hex_encode(&raw);assert_eq!(enc,"0001feff10");assert_eq!(hex_decode(&enc).unwrap(),raw);assert_eq!(hex_decode("  0001FEFF10  ").unwrap(),raw);assert!(hex_decode("abc").is_err());assert!(hex_decode("zz").is_err());}}
