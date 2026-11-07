//! 原生协议适配器共享参数及分派。不通过 ODBC 回退。
use anyhow::{anyhow, Result};
use std::{collections::BTreeMap, time::Duration};
use crate::{http_db::{HttpAdapter,HttpDb,HttpRows},models::ConnectionRequest};
#[derive(Clone)]
pub(crate) struct Params { pub values:BTreeMap<String,String>, pub timeout:Duration }
impl Params {
 pub fn new(req:&ConnectionRequest)->Self { let mut values=BTreeMap::new();
  if let Ok(serde_json::Value::Object(m))=serde_json::from_str(&req.params) { for(k,v) in m {values.insert(k.to_lowercase(),v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string()));} }
  else {for(k,v) in crate::db::extra_params(&req.params){values.insert(k.trim().to_lowercase(),v.trim().to_string());}}
  Self{values,timeout:Duration::from_secs(req.timeout_secs.max(1).min(120).max(if req.timeout_secs==0 {10}else{1}))}
 }
 pub fn get(&self,k:&str)->Option<&str>{self.values.get(k).map(String::as_str)}
 pub fn bool(&self,k:&str,default:bool)->bool {self.get(k).map(|v|match v.trim().to_ascii_lowercase().as_str(){"true"|"1"|"yes"|"require"|"required"|"verify-ca"|"verify-full"=>true,"false"|"0"|"no"|"disable"|"disabled"=>false,_=>default}).unwrap_or(default)}
 pub fn no_tls(&self,kind:&str)->Result<()> {if ["ssl","tls","sslmode","ssl-mode"].iter().any(|k|self.get(k).is_some_and(|v|!matches!(v.trim().to_ascii_lowercase().as_str(),"false"|"0"|"no"|"disable"|"disabled"|""))){Err(anyhow!("{kind} 适配器当前不支持 TLS；拒绝降级为明文"))}else{Ok(())}}
}
pub(crate) fn cell(v:&serde_json::Value)->Option<String>{if v.is_null(){None}else{Some(v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string()))}}
pub(crate) fn text_bytes(b:&[u8])->String {String::from_utf8(b.to_vec()).unwrap_or_else(|_|{use base64::Engine;format!("base64:{}",base64::engine::general_purpose::STANDARD.encode(b))})}
pub(crate) fn single(name:&str,value:impl Into<String>)->HttpRows{HttpRows{columns:vec![name.into()],rows:vec![vec![Some(value.into())]]}}
pub(crate) async fn connect(req:&ConnectionRequest)->Result<HttpDb>{
 let input=req.params.trim();
 if input.starts_with('{')||input.starts_with('['){
  let value:serde_json::Value=serde_json::from_str(input).map_err(|_|anyhow!("原生连接 JSON 参数格式无效"))?;
  if !value.is_object(){return Err(anyhow!("原生连接 JSON 参数必须为对象"));}
 }
 let adapter:Box<dyn HttpAdapter>=match req.kind.id(){
  "salesforce"=>crate::salesforce_rest::connect(req).await?,
  "hbase"=>crate::hbase_rest::connect(req).await?,
  "h2"=>Box::new(crate::h2_db::H2Db::connect(req).await?),
  "druid"|"dynamodb"|"etcd"|"rqlite"=>Box::new(crate::native_http::ApiDb::connect(req).await?),
  "cassandra"=>Box::new(crate::native_cql::CassandraDb::connect(req).await?),
  "nebulagraph"=>Box::new(crate::native_cql::NebulaDb::connect(req).await?),
  "kafka"=>Box::new(crate::native_mq::KafkaDb::connect(req).await?),
  "mqtt"=>Box::new(crate::native_mq::MqttDb::connect(req).await?),
  "zookeeper"=>Box::new(crate::native_zk::ZkDb::connect(req).await?),
  "rocketmq"=>Box::new(crate::native_rocket::RocketDb::connect(req).await?),
  k=>return Err(anyhow!("未注册原生适配器: {k}"))};
 HttpDb::from_adapter(req,adapter).await
}
#[cfg(test)] mod tests {use super::*;#[test] fn binary_lossless(){assert_eq!(text_bytes("中文".as_bytes()),"中文");assert_eq!(text_bytes(&[255]),"base64:/w==");}}

#[cfg(test)]
#[path = "native_tests.rs"]
mod integration_tests;
