//! RocketMQ NameServer/Broker原生Remoting协议（JSON header frame），不需要HTTP console或Go agent。
use anyhow::{anyhow,bail,Result};use async_trait::async_trait;use serde_json::{Value,json};use std::{collections::BTreeMap,sync::atomic::{AtomicI32,Ordering}};use tokio::{io::{AsyncReadExt,AsyncWriteExt},net::TcpStream,time::timeout};
use crate::{http_db::{HttpAdapter,HttpExportPage,HttpRows},models::ConnectionRequest,native_db::{Params,text_bytes,single}};
pub(crate)struct RocketDb{names:String,p:Params,user:String,password:String,opaque:AtomicI32}
struct Reply{header:Value,body:Vec<u8>}
fn parse_json(bytes:&[u8])->Result<Value>{// RocketMQ FastJSON maps may serialize numeric broker ids without quotes.
 if let Ok(v)=serde_json::from_slice(bytes){return Ok(v);}let text=std::str::from_utf8(bytes)?;let mut out=String::new();let mut in_string=false;let mut escape=false;let mut key=false;let bs=text.as_bytes();let mut i=0;while i<bs.len(){let c=bs[i] as char;if in_string&&!c.is_ascii(){let ch=text[i..].chars().next().ok_or_else(||anyhow!("invalid UTF8"))?;out.push(ch);i+=ch.len_utf8();continue;}if in_string{out.push(c);if escape{escape=false;}else if c=='\\'{escape=true;}else if c=='"'{in_string=false;}i+=1;continue;}if c=='"'{in_string=true;out.push(c);i+=1;continue;}if c=='{'||c==','{key=true;out.push(c);i+=1;continue;}if key&&c.is_ascii_digit(){let start=i;while i<bs.len()&&bs[i].is_ascii_digit(){i+=1;}out.push('"');out.push_str(&text[start..i]);out.push('"');key=false;continue;}if !c.is_ascii_whitespace(){key=false;}if c.is_ascii(){out.push(c);i+=1;}else{let ch=text[i..].chars().next().ok_or_else(||anyhow!("invalid UTF8"))?;out.push(ch);i+=ch.len_utf8();}}Ok(serde_json::from_str(&out)?)}
/// RocketMQ 导出游标固定 `first:last:next` 的 broker queue offset。first/last
/// 是导出开始时的最小/最大 offset 边界（后续页锁定），next 是下一页真实起点。
fn parse_rocket_token(t:&str)->Result<(u64,u64,u64)>{let parts:Vec<&str>=t.split(':').collect();if parts.len()!=3{return Err(anyhow!("RocketMQ 导出游标应为 first:last:next"));}let f=parts[0].parse::<u64>().map_err(|_|anyhow!("RocketMQ 导出游标 first 非法"))?;let l=parts[1].parse::<u64>().map_err(|_|anyhow!("RocketMQ 导出游标 last 非法"))?;let n=parts[2].parse::<u64>().map_err(|_|anyhow!("RocketMQ 导出游标 next 非法"))?;if l<f{return Err(anyhow!("RocketMQ 导出游标边界非法"));}Ok((f,l,n))}
impl RocketDb {
 pub async fn connect(r:&ConnectionRequest)->Result<Self>{let p=Params::new(r);p.no_tls("RocketMQ")?;let names=p.get("nameserver").map(str::to_owned).unwrap_or_else(||format!("{}:{}",r.host,if r.port==0{9876}else{r.port}));Ok(Self{names,p,user:r.user.clone(),password:r.password.clone(),opaque:AtomicI32::new(1)})}
 async fn request(&self,address:&str,code:i32,mut fields:BTreeMap<String,String>,body:Vec<u8>,accepted:&[i64])->Result<Reply>{let opaque=self.opaque.fetch_add(1,Ordering::Relaxed);if !self.user.is_empty(){use base64::Engine;use hmac::{Hmac,Mac};fields.insert("AccessKey".into(),self.user.clone());if let Some(t)=self.p.get("security_token"){fields.insert("SecurityToken".into(),t.into());}let mut content=Vec::new();for(k,v)in &fields{if k!="Signature"{content.extend(v.as_bytes());}}content.extend(&body);let mut h=Hmac::<sha1::Sha1>::new_from_slice(self.password.as_bytes())?;h.update(&content);fields.insert("Signature".into(),base64::engine::general_purpose::STANDARD.encode(h.finalize().into_bytes()));}
 let header=serde_json::to_vec(&json!({"code":code,"language":"JAVA","version":433,"opaque":opaque,"flag":0,"extFields":fields}))?;let n=header.len()+body.len()+4;if n>16*1024*1024{bail!("RocketMQ request too large");}timeout(self.p.timeout,async{let mut stream=TcpStream::connect(address).await?;stream.write_all(&(n as u32).to_be_bytes()).await?;stream.write_all(&(header.len() as u32).to_be_bytes()).await?;stream.write_all(&header).await?;stream.write_all(&body).await?;let n=stream.read_u32().await? as usize;if !(4..=16*1024*1024).contains(&n){bail!("RocketMQ invalid frame length: {n}");}let marker=stream.read_u32().await?;if marker>>24!=0{bail!("RocketMQ响应非JSON header编码");}let hn=(marker&0x00ffffff)as usize;if hn>n-4{bail!("RocketMQ invalid header length");}let mut buf=vec![0;n-4];stream.read_exact(&mut buf).await?;let header=parse_json(&buf[..hn])?;if header["opaque"].as_i64()!=Some(opaque as i64){bail!("RocketMQ opaque mismatch");}let rc=header["code"].as_i64().ok_or_else(||anyhow!("RocketMQ missing response code"))?;if !accepted.contains(&rc){bail!("RocketMQ response {rc}: {}",header["remark"]);}Ok(Reply{header,body:buf[hn..].to_vec()})}).await.map_err(|_|anyhow!("RocketMQ remoting请求超时"))?}
 async fn ns(&self,code:i32,fields:BTreeMap<String,String>)->Result<Value>{let mut error=None;for address in self.names.split(';'){match self.request(address,code,fields.clone(),vec![],&[0]).await{Ok(r)=>return parse_json(&r.body),Err(e)=>error=Some(e)}}Err(error.unwrap_or_else(||anyhow!("缺少NameServer地址")))}
 async fn topics(&self)->Result<Vec<String>>{let v=self.ns(206,BTreeMap::new()).await?;Ok(serde_json::from_value(v["topicList"].clone())?)}
 async fn route(&self,topic:&str)->Result<Value>{self.ns(105,BTreeMap::from([("topic".into(),topic.into())])).await}
 async fn queues(&self,topic:&str,writable:bool)->Result<Vec<(String,i32,String)>>{let v=self.route(topic).await?;let mut brokers=BTreeMap::new();for b in v["brokerDatas"].as_array().ok_or_else(||anyhow!("RocketMQ route missing brokerDatas"))?{let name=b["brokerName"].as_str().ok_or_else(||anyhow!("brokerName missing"))?;let addr=b["brokerAddrs"]["0"].as_str().ok_or_else(||anyhow!("RocketMQ master address missing"))?;brokers.insert(name.to_string(),addr.to_string());}let mut out=vec![];for q in v["queueDatas"].as_array().ok_or_else(||anyhow!("queueDatas missing"))?{let name=q["brokerName"].as_str().ok_or_else(||anyhow!("brokerName missing"))?;let addr=brokers.get(name).ok_or_else(||anyhow!("RocketMQ broker mapping missing"))?;let count=q[if writable {"writeQueueNums"} else {"readQueueNums"}].as_i64().ok_or_else(||anyhow!("readQueueNums missing"))?;for id in 0..count{out.push((name.to_string(),id as i32,addr.clone()));}}Ok(out)}
 async fn pull(&self,topic:&str,object:&str,offset:u64,limit:u64)->Result<HttpRows>{let queues=self.queues(topic,false).await?;let(b,id,addr)=queues.into_iter().find(|(b,id,_)|format!("{b}:{id}")==object).ok_or_else(||anyhow!("RocketMQ队列不存在: {object}"))?;let _=b;let basefields=BTreeMap::from([("topic".into(),topic.into()),("queueId".into(),id.to_string())]);let min=self.request(&addr,31,basefields.clone(),vec![],&[0]).await?;let first=min.header["extFields"]["offset"].as_str().ok_or_else(||anyhow!("minOffset missing"))?.parse::<u64>()?;let mut fields=basefields;fields.extend(BTreeMap::from([("consumerGroup".into(),"_NAVKIT_BROWSER".into()),("queueOffset".into(),first.saturating_add(offset).to_string()),("maxMsgNums".into(),limit.min(1000).max(1).to_string()),("sysFlag".into(),"4".into()),("commitOffset".into(),"0".into()),("suspendTimeoutMillis".into(),"0".into()),("subscription".into(),"*".into()),("subVersion".into(),"0".into()),("expressionType".into(),"TAG".into())]));let r=self.request(&addr,11,fields,vec![],&[0,19,20]).await?;decode_messages(&r.body)}
 /// 导出一页：固定 first/last 边界，按真实 queue offset 递进；空页但未到 last
 /// 明确报错，绝不把 compaction/retention 空洞当成功截断。
 async fn export_queue(&self,topic:&str,object:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{
  let queues=self.queues(topic,false).await?;
  let(_,id,addr)=queues.into_iter().find(|(b,id,_)|format!("{b}:{id}")==object).ok_or_else(||anyhow!("RocketMQ队列不存在: {object}"))?;
  let basefields=BTreeMap::from([("topic".to_string(),topic.to_string()),("queueId".to_string(),id.to_string())]);
  let (first,last,mut next)=match token{
   None=>{let min=self.request(&addr,31,basefields.clone(),vec![],&[0]).await?;let f=min.header["extFields"]["offset"].as_str().ok_or_else(||anyhow!("minOffset missing"))?.parse::<u64>()?;let max=self.request(&addr,30,basefields.clone(),vec![],&[0]).await?;let l=max.header["extFields"]["offset"].as_str().ok_or_else(||anyhow!("maxOffset missing"))?.parse::<u64>()?;(f,l,f)},
   Some(t)=>{let(f,l,n)=parse_rocket_token(t)?;(f,l,n)}};
  if next<first||next>last{return Err(anyhow!("RocketMQ 导出游标 next={next} 超出边界 {first}..{last}"));}
  let mut out=HttpRows{columns:["queue_id","queue_offset","body","topic","properties","store_timestamp"].iter().map(|s|s.to_string()).collect(),rows:vec![]};
  while next<last&&out.rows.len()<(limit as usize){
   let mut fields=basefields.clone();
   fields.extend(BTreeMap::from([("consumerGroup".to_string(),"_NAVKIT_EXPORT".to_string()),("queueOffset".to_string(),next.to_string()),("maxMsgNums".to_string(),limit.min(1000).max(1).to_string()),("sysFlag".to_string(),"4".to_string()),("commitOffset".to_string(),"0".to_string()),("suspendTimeoutMillis".to_string(),"0".to_string()),("subscription".to_string(),"*".to_string()),("subVersion".to_string(),"0".to_string()),("expressionType".to_string(),"TAG".to_string())]));
   let r=self.request(&addr,11,fields,vec![],&[0,19,20]).await?;
   let page=decode_messages(&r.body)?;
   if page.rows.is_empty(){return Err(anyhow!("RocketMQ topic={topic} queue={object} 偏移 {next}..{last} 无消息（compaction/retention 空洞），拒绝静默截断导出"));}
   let mut advanced=false;
   for row in page.rows{let off=row.get(1).and_then(|v|v.as_deref()).and_then(|s|s.parse::<u64>().ok()).ok_or_else(||anyhow!("RocketMQ 消息缺少 queue_offset"))?;
    // 首屏锁定的 maxOffset 是导出快照边界：绝不把之后新写入的消息写进本页。
    if off>=last{break;}
    if off<next{continue;}next=off+1;advanced=true;out.rows.push(row);if out.rows.len()>=limit as usize{break;}}
   if !advanced{return Err(anyhow!("RocketMQ 拉取未推进 queue offset（next 仍为 {next}），拒绝死循环"));}
  }
  let nxt=if next<last{Some(format!("{first}:{last}:{next}"))}else{None};
  Ok(HttpExportPage{data:out,next:nxt})
 }
}
fn decode_messages(bytes:&[u8])->Result<HttpRows>{let cols=["queue_id","queue_offset","body","topic","properties","store_timestamp"];let mut rows=vec![];let mut pos=0;while pos<bytes.len(){if pos+4>bytes.len(){bail!("RocketMQ truncated message length");}let n=u32::from_be_bytes(bytes[pos..pos+4].try_into()?)as usize;if n<80||pos+n>bytes.len(){bail!("RocketMQ invalid message size {n}");}let msg=&bytes[pos..pos+n];let magic=i32::from_be_bytes(msg[4..8].try_into()?);let queue=i32::from_be_bytes(msg[12..16].try_into()?);let off=i64::from_be_bytes(msg[20..28].try_into()?);let flags=i32::from_be_bytes(msg[36..40].try_into()?);let born_host=if flags&16!=0{20}else{8};let store_host=if flags&32!=0{20}else{8};let timestamp_pos=48+born_host;let timestamp=i64::from_be_bytes(msg.get(timestamp_pos..timestamp_pos+8).ok_or_else(||anyhow!("truncated timestamp"))?.try_into()?);let mut at=timestamp_pos+8+store_host+4+8;let bl=u32::from_be_bytes(msg.get(at..at+4).ok_or_else(||anyhow!("truncated body length"))?.try_into()?)as usize;at+=4;let body=msg.get(at..at+bl).ok_or_else(||anyhow!("truncated body"))?;at+=bl;let tl=if magic==-626843477{let n=u16::from_be_bytes(msg.get(at..at+2).ok_or_else(||anyhow!("truncated topic length"))?.try_into()?)as usize;at+=2;n}else{let n=*msg.get(at).ok_or_else(||anyhow!("truncated topic length"))?as usize;at+=1;n};let topic=msg.get(at..at+tl).ok_or_else(||anyhow!("truncated topic"))?;at+=tl;let pl=u16::from_be_bytes(msg.get(at..at+2).ok_or_else(||anyhow!("truncated properties length"))?.try_into()?)as usize;at+=2;let prop=msg.get(at..at+pl).ok_or_else(||anyhow!("truncated properties"))?;let body=if flags&1!=0{use std::io::Read;let mut out=vec![];flate2::read::ZlibDecoder::new(body).take(16*1024*1024+1).read_to_end(&mut out)?;if out.len()>16*1024*1024{bail!("RocketMQ 解压后的消息体超过 16 MiB 上限，拒绝截断消息");}text_bytes(&out)}else{text_bytes(body)};rows.push(vec![Some(queue.to_string()),Some(off.to_string()),Some(body),Some(text_bytes(topic)),Some(text_bytes(prop)),Some(timestamp.to_string())]);pos+=n;}Ok(HttpRows{columns:cols.iter().map(|s|s.to_string()).collect(),rows})}
#[async_trait]impl HttpAdapter for RocketDb{
 async fn ping(&self)->Result<String>{let v=self.ns(106,BTreeMap::new()).await?;if !v["brokerAddrTable"].is_object()||!v["clusterAddrTable"].is_object(){bail!("RocketMQ cluster响应结构无效");}Ok("RocketMQ NameServer native remoting verified".into())}
 async fn containers(&self)->Result<Vec<(String,String)>>{Ok(self.topics().await?.into_iter().map(|t|(t,"topic".into())).collect())}
 async fn objects(&self,c:&str)->Result<Vec<String>>{Ok(self.queues(c,false).await?.into_iter().map(|(b,id,_)|format!("{b}:{id}")).collect())}
 async fn rows(&self,c:&str,o:&str,offset:u64,limit:u64)->Result<HttpRows>{self.pull(c,o,offset,limit).await}
 async fn export_page(&self,c:&str,o:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{if limit==0{return Ok(HttpExportPage{data:HttpRows{columns:["queue_id","queue_offset","body","topic","properties","store_timestamp"].iter().map(|s|s.to_string()).collect(),rows:vec![]},next:None});}self.export_queue(c,o,token,limit).await}
 async fn run(&self,c:&str,o:&str,s:&str)->Result<HttpRows>{let v:Value=serde_json::from_str(s).map_err(|_|anyhow!("RocketMQ JSON operation=topics/route/pull/send"))?;let topic=v["topic"].as_str().unwrap_or(c);match v["operation"].as_str().unwrap_or("pull"){
 "topics"=>Ok(HttpRows{columns:vec!["topic".into()],rows:self.topics().await?.into_iter().map(|s|vec![Some(s)]).collect()}),"route"=>Ok(single("route",self.route(topic).await?.to_string())),"pull"=>self.pull(topic,v["queue"].as_str().unwrap_or(o),v["offset"].as_u64().unwrap_or(0),v["limit"].as_u64().unwrap_or(100)).await,
 "send"=>{let queues=self.queues(topic,true).await?;let(_,id,addr)=queues.first().ok_or_else(||anyhow!("没有可写队列"))?;let payload=v["body"].as_str().ok_or_else(||anyhow!("send需要body字符串"))?;let fields=BTreeMap::from([("producerGroup".into(),"_NAVKIT_PRODUCER".into()),("topic".into(),topic.into()),("defaultTopic".into(),"TBW102".into()),("defaultTopicQueueNums".into(),"4".into()),("queueId".into(),id.to_string()),("sysFlag".into(),"0".into()),("bornTimestamp".into(),chrono::Utc::now().timestamp_millis().to_string()),("flag".into(),"0".into()),("properties".into(),String::new()),("reconsumeTimes".into(),"0".into()),("unitMode".into(),"false".into()),("batch".into(),"false".into())]);let r=self.request(addr,10,fields,payload.as_bytes().to_vec(),&[0]).await?;Ok(single("result",r.header["extFields"].to_string()))},_=>bail!("未知RocketMQ操作")}}
}
#[cfg(test)]mod tests{use super::*;
fn compressed_message(size: usize) -> Vec<u8> {
    use std::io::Write;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    let chunk = [b'x'; 8192];
    let mut remaining = size;
    while remaining != 0 {
        let n = remaining.min(chunk.len());
        encoder.write_all(&chunk[..n]).unwrap();
        remaining -= n;
    }
    let body = encoder.finish().unwrap();
    let mut message = vec![0u8; 88];
    // IPv4 born/store host widths: body length at offset 84, body starts at 88.
    message[36..40].copy_from_slice(&1i32.to_be_bytes());
    message[84..88].copy_from_slice(&(body.len() as u32).to_be_bytes());
    message.extend_from_slice(&body);
    message.extend_from_slice(&[1, b't', 0, 0]); // v1 topic length/topic/empty properties
    let total = message.len() as u32;
    message[..4].copy_from_slice(&total.to_be_bytes());
    message
}
#[test]
fn compressed_message_over_limit_is_rejected_without_partial_body() {
    let message = compressed_message(16 * 1024 * 1024 + 1);
    let error = decode_messages(&message).err().expect("oversized decompression must fail");
    assert!(error.to_string().contains("解压后的消息体超过 16 MiB"), "{error}");
}
#[test]
fn compressed_message_exact_limit_is_not_truncated() {
    let message = compressed_message(16 * 1024 * 1024);
    let rows = decode_messages(&message).unwrap();
    assert_eq!(rows.rows.len(), 1);
    let body = rows.rows[0][2].as_deref().unwrap();
    assert_eq!(body.len(), 16 * 1024 * 1024);
    assert!(body.bytes().all(|b| b == b'x'));
}
#[test]fn fastjson_numeric_keys(){assert_eq!(parse_json("{\"brokerAddrs\":{0:\"中文\",1:\"host:123\"}}".as_bytes()).unwrap()["brokerAddrs"]["0"],"中文");}#[test]fn truncated_messages_fail(){assert!(decode_messages(&[0,0,0,128]).is_err());}#[test]fn rocket_token_roundtrip(){assert_eq!(parse_rocket_token("5:9:7").unwrap(),(5,9,7));for t in ["","1:2","a:2:3","1:2:x","9:5:9"]{assert!(parse_rocket_token(t).is_err(),"{t}");}}}
