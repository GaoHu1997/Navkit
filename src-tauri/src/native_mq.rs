//! Kafka 原生 broker API 与 MQTT 3.1.1 客户端。MQTT 无 topic 目录 API，目录仅含订阅实际观察到的主题。
use anyhow::{anyhow,bail,Result};use async_trait::async_trait;use serde_json::{Value,json};use std::{collections::BTreeMap,time::Duration};use tokio::{sync::Mutex,time::{timeout,Instant}};
use crate::{http_db::{HttpAdapter,HttpExportPage,HttpRows},models::ConnectionRequest,native_db::{Params,text_bytes,single}};
use rskafka::{client::{Client,ClientBuilder,Credentials,SaslConfig,partition::{UnknownTopicHandling,OffsetAt,Compression}},record::Record};
pub(crate)struct KafkaDb{client:Client,p:Params}
/// Kafka 导出游标固定 `first:last:next` 三个 broker 偏移：first/last 是导出开始
/// 时的最早/最新边界（后续页锁定，不随新写入漂移），next 是下一页实际起点。
/// 绝不按“已返回行数”累计 offset，否则 compaction/retention 空洞会静默截断。
fn parse_kafka_token(t:&str)->Result<(i64,i64,i64)>{let parts:Vec<&str>=t.split(':').collect();if parts.len()!=3{return Err(anyhow!("Kafka 导出游标应为 first:last:next"));}let f=parts[0].parse::<i64>().map_err(|_|anyhow!("Kafka 导出游标 first 非法"))?;let l=parts[1].parse::<i64>().map_err(|_|anyhow!("Kafka 导出游标 last 非法"))?;let n=parts[2].parse::<i64>().map_err(|_|anyhow!("Kafka 导出游标 next 非法"))?;if f<0||l<f||n<0{return Err(anyhow!("Kafka 导出游标边界非法"));}Ok((f,l,n))}
impl KafkaDb {
 pub async fn connect(r:&ConnectionRequest)->Result<Self>{let p=Params::new(r);let tls=crate::native_tls::parse(&r.params)?;let brokers=p.get("brokers").map(|v|v.split(',').map(str::to_owned).collect()).unwrap_or_else(||vec![format!("{}:{}",r.host,if r.port==0{9092}else{r.port})]);let mut b=ClientBuilder::new(brokers).client_id("navkit");if tls.enabled{b=b.tls_config(tls.kafka()?);}if !r.user.is_empty(){let c=Credentials::new(r.user.clone(),r.password.clone());let s=match p.get("sasl_mechanism").unwrap_or("PLAIN").to_uppercase().as_str(){"PLAIN"=>SaslConfig::Plain(c),"SCRAM-SHA-256"=>SaslConfig::ScramSha256(c),"SCRAM-SHA-512"=>SaslConfig::ScramSha512(c),_=>return Err(anyhow!("Kafka只支持PLAIN/SCRAM-SHA-256/SCRAM-SHA-512"))};b=b.sasl_config(s);}let client=timeout(p.timeout,b.build()).await.map_err(|_|anyhow!("Kafka连接超时"))??;Ok(Self{client,p})}
 async fn fetch(&self,topic:&str,partition:i32,offset:u64,limit:u64)->Result<HttpRows>{timeout(self.p.timeout,async{let pc=self.client.partition_client(topic,partition,UnknownTopicHandling::Error).await?;let first=pc.get_offset(OffsetAt::Earliest).await?;let last=pc.get_offset(OffsetAt::Latest).await?;let mut pos=first.saturating_add(i64::try_from(offset)?);let mut out=HttpRows{columns:["partition","offset","key","value","timestamp","headers"].iter().map(|s|s.to_string()).collect(),rows:vec![]};while pos<last&&out.rows.len()<(limit as usize){let(rs,_)=pc.fetch_records(pos,1..1_048_576,100).await?;if rs.is_empty(){break;}for r in rs {pos=r.offset+1;let headers:BTreeMap<_,_>=r.record.headers.into_iter().map(|(k,v)|(k,text_bytes(&v))).collect();out.rows.push(vec![Some(partition.to_string()),Some(r.offset.to_string()),r.record.key.as_deref().map(text_bytes),r.record.value.as_deref().map(text_bytes),Some(r.record.timestamp.to_rfc3339()),Some(serde_json::to_string(&headers)?)]);if out.rows.len()>=limit as usize{break;}}}Ok(out)}).await.map_err(|_|anyhow!("Kafka查询超时"))?}
 /// 导出一页：按固定 first/last 边界与真实 broker 偏移递进。
 async fn export_partition(&self,topic:&str,partition:i32,token:Option<&str>,limit:u64)->Result<HttpExportPage>{
  timeout(self.p.timeout,async{
   let pc=self.client.partition_client(topic,partition,UnknownTopicHandling::Error).await?;
   let (first,last,mut pos)=match token{
    None=>{let f=pc.get_offset(OffsetAt::Earliest).await?;let l=pc.get_offset(OffsetAt::Latest).await?;(f,l,f)},
    Some(t)=>{let(f,l,n)=parse_kafka_token(t)?;(f,l,n)}};
   if pos<first||pos>last{return Err(anyhow!("Kafka 导出游标 next={pos} 超出边界 {first}..{last}"));}
   let mut out=HttpRows{columns:["partition","offset","key","value","timestamp","headers"].iter().map(|s|s.to_string()).collect(),rows:vec![]};
   while pos<last&&out.rows.len()<(limit as usize){
    let(rs,_)=pc.fetch_records(pos,1..1_048_576,100).await?;
    if rs.is_empty(){return Err(anyhow!("Kafka topic={topic} partition={partition} 偏移 {pos}..{last} 之间没有可读记录（compaction/retention 空洞），拒绝静默截断导出"));}
    let start_rows=out.rows.len();
    for r in rs {
     // 首屏锁定的 highwater 是导出快照边界：若 fetch 带回快照之后新写入的
     // 记录（offset >= last），直接停在 last，绝不把新增数据写进本次导出。
     if r.offset>=last {
      if out.rows.len()==start_rows{return Err(anyhow!("Kafka topic={topic} partition={partition} 偏移 {pos}..{last} 之间没有可读记录（compaction/retention 空洞），拒绝静默截断导出"));}
      pos=last;break;
     }
     pos=r.offset+1;
     let headers:BTreeMap<_,_>=r.record.headers.into_iter().map(|(k,v)|(k,text_bytes(&v))).collect();
     out.rows.push(vec![Some(partition.to_string()),Some(r.offset.to_string()),r.record.key.as_deref().map(text_bytes),r.record.value.as_deref().map(text_bytes),Some(r.record.timestamp.to_rfc3339()),Some(serde_json::to_string(&headers)?)]);
     if out.rows.len()>=limit as usize{break;}
    }
   }
   let next=if pos<last{Some(format!("{first}:{last}:{pos}"))}else{None};
   Ok(HttpExportPage{data:out,next})
  }).await.map_err(|_|anyhow!("Kafka 导出一页超时"))?
 }
}
#[async_trait]impl HttpAdapter for KafkaDb{
 async fn ping(&self)->Result<String>{timeout(self.p.timeout,self.client.list_topics()).await.map_err(|_|anyhow!("Kafka元数据超时"))??;Ok("Kafka broker API (authenticated metadata)".into())}
 async fn containers(&self)->Result<Vec<(String,String)>>{Ok(timeout(self.p.timeout,self.client.list_topics()).await.map_err(|_|anyhow!("Kafka元数据超时"))??.into_iter().map(|t|(t.name,format!("{} partitions",t.partitions.len()))).collect())}
 async fn objects(&self,c:&str)->Result<Vec<String>>{let ts=timeout(self.p.timeout,self.client.list_topics()).await.map_err(|_|anyhow!("Kafka元数据超时"))??;let t=ts.into_iter().find(|t|t.name==c).ok_or_else(||anyhow!("Kafka topic不存在: {c}"))?;Ok(t.partitions.into_iter().map(|p|p.to_string()).collect())}
 async fn rows(&self,c:&str,o:&str,offset:u64,limit:u64)->Result<HttpRows>{self.fetch(c,o.parse()?,offset,limit).await}
 async fn export_page(&self,c:&str,o:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{let part:i32=o.parse()?;if limit==0{return Ok(HttpExportPage{data:HttpRows{columns:["partition","offset","key","value","timestamp","headers"].iter().map(|s|s.to_string()).collect(),rows:vec![]},next:None});}self.export_partition(c,part,token,limit).await}
 async fn run(&self,c:&str,o:&str,s:&str)->Result<HttpRows>{let v:Value=serde_json::from_str(s).map_err(|_|anyhow!("Kafka命令须为JSON: operation=fetch/produce/topics"))?;let topic=v["topic"].as_str().unwrap_or(c);let part=v["partition"].as_i64().unwrap_or(o.parse().unwrap_or(0)) as i32;match v["operation"].as_str().unwrap_or("fetch"){
 "topics"=>{let r=self.containers().await?;Ok(HttpRows{columns:vec!["topic".into(),"partitions".into()],rows:r.into_iter().map(|(a,b)|vec![Some(a),Some(b)]).collect()})},
 "fetch"=>self.fetch(topic,part,v["offset"].as_u64().unwrap_or(0),v["limit"].as_u64().unwrap_or(100).min(10000)).await,
 "produce"=>{let key=v.get("key").filter(|v|!v.is_null()).map(|v|v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string()).into_bytes());let value=v.get("value").filter(|v|!v.is_null()).map(|v|v.as_str().map(str::to_owned).unwrap_or_else(||v.to_string()).into_bytes());let offsets=timeout(self.p.timeout,async{let pc=self.client.partition_client(topic,part,UnknownTopicHandling::Error).await?;pc.produce(vec![Record{key,value,headers:BTreeMap::new(),timestamp:chrono::Utc::now()}],Compression::NoCompression).await}).await.map_err(|_|anyhow!("Kafka生产超时"))??;Ok(single("offsets",json!(offsets).to_string()))},_=>Err(anyhow!("未知Kafka操作"))}}
}
use rumqttc::{AsyncClient,EventLoop,MqttOptions,Event,Packet,QoS,ConnectReturnCode,SubscribeReasonCode};
struct MState{client:AsyncClient,event:EventLoop,messages:Vec<Value>}
pub(crate)struct MqttDb{state:Mutex<MState>,p:Params,filter:String}
impl MqttDb {
 pub async fn connect(r:&ConnectionRequest)->Result<Self>{let p=Params::new(r);let tls=crate::native_tls::parse(&r.params)?;let id=p.get("client_id").map(str::to_owned).unwrap_or_else(||format!("navkit-{}",uuid::Uuid::new_v4()));let mut opt=MqttOptions::new(id,&r.host,if r.port==0{1883}else{r.port});opt.set_keep_alive(Duration::from_secs(15));if tls.enabled{opt.set_transport(rumqttc::Transport::tls_with_config(tls.mqtt()?));}if !r.user.is_empty(){opt.set_credentials(&r.user,&r.password);}let(client,mut event)=AsyncClient::new(opt,32);timeout(p.timeout,async{loop{if let Event::Incoming(Packet::ConnAck(a))=event.poll().await?{if a.code!=ConnectReturnCode::Success{return Err(anyhow!("MQTT CONNACK拒绝: {:?}",a.code));}return Ok(());}}}).await.map_err(|_|anyhow!("MQTT连接超时"))??;let filter=p.get("topic_filter").unwrap_or(if r.database.is_empty(){"#"}else{&r.database}).to_string();Ok(Self{state:Mutex::new(MState{client,event,messages:vec![]}),p,filter})}
 fn remember(st:&mut MState,p:rumqttc::Publish){st.messages.push(json!({"topic":p.topic,"payload":text_bytes(&p.payload),"qos":format!("{:?}",p.qos),"retain":p.retain,"received":chrono::Utc::now().to_rfc3339()}));if st.messages.len()>10000{st.messages.drain(0..st.messages.len()-10000);}}
 async fn capture(&self,filter:&str,window:Duration)->Result<Vec<Value>>{let mut st=self.state.lock().await;st.client.subscribe(filter,QoS::AtLeastOnce).await?;timeout(self.p.timeout,async{loop {match st.event.poll().await?{Event::Incoming(Packet::SubAck(a))=>{if a.return_codes.iter().any(|x|matches!(x,SubscribeReasonCode::Failure)){return Err(anyhow!("MQTT SUBACK拒绝"));}return Ok(());},Event::Incoming(Packet::Publish(p))=>Self::remember(&mut st,p),_=>{}}}}).await.map_err(|_|anyhow!("MQTT订阅超时"))??;let end=Instant::now()+window;while Instant::now()<end{match timeout(end.saturating_duration_since(Instant::now()),st.event.poll()).await {Ok(Ok(Event::Incoming(Packet::Publish(p))))=>Self::remember(&mut st,p),Ok(Ok(_))=>{},Ok(Err(e))=>return Err(e.into()),Err(_)=>break}}
 // broker 标准不提供 topic list：仅返回此连接实际观察的消息。
 Ok(st.messages.clone())}
 fn table(rs:Vec<Value>,topic:Option<&str>,offset:u64,limit:u64)->HttpRows{let columns=["topic","payload","qos","retain","received"];HttpRows{columns:columns.iter().map(|s|s.to_string()).collect(),rows:rs.into_iter().filter(|v|topic.map(|t|v["topic"].as_str()==Some(t)).unwrap_or(true)).skip(offset as usize).take(limit as usize).map(|v|columns.iter().map(|c|crate::native_db::cell(&v[c])).collect()).collect()}}
}
#[async_trait]impl HttpAdapter for MqttDb{
 async fn ping(&self)->Result<String>{let mut st=self.state.lock().await;st.client.subscribe(&self.filter,QoS::AtLeastOnce).await?;timeout(self.p.timeout,async{loop{match st.event.poll().await?{Event::Incoming(Packet::SubAck(a))=>{if a.return_codes.iter().any(|x|matches!(x,SubscribeReasonCode::Failure)){return Err(anyhow!("MQTT SUBACK拒绝"));}return Ok("MQTT 3.1.1 (CONNACK/SUBACK verified)".into());},Event::Incoming(Packet::Publish(p))=>Self::remember(&mut st,p),_=>{}}}}).await.map_err(|_|anyhow!("MQTT会话检查超时"))?}
 async fn containers(&self)->Result<Vec<(String,String)>>{self.capture(&self.filter,Duration::from_millis(250)).await?;Ok(vec![(self.filter.clone(),"订阅过滤器；仅含实际观察主题（无标准全量目录API）".into())])}
 async fn objects(&self,c:&str)->Result<Vec<String>>{let mut topics=self.capture(c,Duration::from_millis(250)).await?.into_iter().filter_map(|v|v["topic"].as_str().map(str::to_owned)).collect::<Vec<_>>();topics.sort();topics.dedup();Ok(topics)}
 async fn rows(&self,c:&str,o:&str,offset:u64,limit:u64)->Result<HttpRows>{Ok(Self::table(self.capture(if o.is_empty(){c}else{o},Duration::from_millis(500)).await?,if o.is_empty(){None}else{Some(o)},offset,limit))}
 /// MQTT 没有标准的历史消息存储，无法做全量导出。这里必须明确拒绝，
 /// 绝不能把实时捕获的缓存（仅连接期间观察到的少量消息）重复当全量导出。
 async fn export_page(&self,_c:&str,_o:&str,_token:Option<&str>,_limit:u64)->Result<HttpExportPage>{bail!("MQTT 无标准历史存储，无法全量导出历史消息；实时缓存不是全量数据，请改用订阅预览")}
 async fn run(&self,c:&str,o:&str,s:&str)->Result<HttpRows>{let v:Value=serde_json::from_str(s).map_err(|_|anyhow!("MQTT命令须为JSON operation=subscribe/publish"))?;let topic=v["topic"].as_str().unwrap_or(if o.is_empty(){c}else{o});match v["operation"].as_str().unwrap_or("subscribe"){
 "subscribe"=>Ok(Self::table(self.capture(topic,Duration::from_millis(v["window_ms"].as_u64().unwrap_or(500).min(self.p.timeout.as_millis() as u64))).await?,None,0,v["limit"].as_u64().unwrap_or(100).min(10000))),
 "publish"=>{let payload=v.get("payload").ok_or_else(||anyhow!("缺少payload"))?;let payload=payload.as_str().map(str::to_owned).unwrap_or_else(||payload.to_string());let mut st=self.state.lock().await;st.client.publish(topic,QoS::AtLeastOnce,v["retain"].as_bool().unwrap_or(false),payload).await?;timeout(self.p.timeout,async{loop{match st.event.poll().await?{Event::Incoming(Packet::PubAck(_))=>return Ok(single("status","PUBACK")),Event::Incoming(Packet::Publish(p))=>Self::remember(&mut st,p),_=>{}}}}).await.map_err(|_|anyhow!("MQTT PUBACK超时"))?},_=>Err(anyhow!("未知MQTT操作"))}}
}
#[cfg(test)]mod tests{use super::*;#[test]fn kafka_token_roundtrip(){let(f,l,n)=parse_kafka_token("10:25:18").unwrap();assert_eq!((f,l,n),(10,25,18));assert_eq!(format!("{f}:{l}:{n}"),"10:25:18");}#[test]fn kafka_token_rejects_malformed(){for t in ["","1:2","a:2:3","1:2:x","-1:2:0","5:3:5","1:2:-1"]{assert!(parse_kafka_token(t).is_err(),"{t}");}}}
