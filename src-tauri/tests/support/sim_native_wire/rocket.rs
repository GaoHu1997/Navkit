//! Stateful RocketMQ NameServer + Broker Remoting/MessageExt subset.
//! ACL uses independently implemented RFC2104 HMAC, not the product hmac crate.
use super::*;
use anyhow::{bail, ensure, Result};
use base64::Engine;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::collections::BTreeMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn hmac_sha1(key: &[u8], content: &[u8]) -> Vec<u8> {
    let mut block = [0u8; 64];
    if key.len() > 64 { block[..20].copy_from_slice(&Sha1::digest(key)); }
    else { block[..key.len()].copy_from_slice(key); }
    let mut inner = Sha1::new(); inner.update(block.map(|b| b ^ 0x36)); inner.update(content);
    let mut outer = Sha1::new(); outer.update(block.map(|b| b ^ 0x5c)); outer.update(inner.finalize());
    outer.finalize().to_vec()
}
fn signature(fields: &BTreeMap<String, String>, body: &[u8]) -> String {
    let mut content = Vec::new();
    for (key, value) in fields { if key != "Signature" { content.extend(value.as_bytes()); } }
    content.extend(body);
    base64::engine::general_purpose::STANDARD.encode(hmac_sha1(b"secret", &content))
}
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes { crc ^= *byte as u32; for _ in 0..8 { crc = (crc >> 1) ^ (0xedb88320u32 & (0u32.wrapping_sub(crc & 1))); } }
    !crc
}
fn message(offset: usize, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![];
    bytes.extend(0u32.to_be_bytes()); bytes.extend((-626843481i32).to_be_bytes());
    bytes.extend(crc32(body).to_be_bytes()); bytes.extend(0u32.to_be_bytes()); bytes.extend(0u32.to_be_bytes());
    bytes.extend((offset as i64).to_be_bytes()); bytes.extend(0u64.to_be_bytes()); bytes.extend(0u32.to_be_bytes());
    bytes.extend(0u64.to_be_bytes()); bytes.extend([0u8; 8]);
    bytes.extend(1700000000000i64.to_be_bytes()); bytes.extend([0u8; 8]);
    bytes.extend(0u32.to_be_bytes()); bytes.extend(0u64.to_be_bytes());
    bytes.extend((body.len() as u32).to_be_bytes()); bytes.extend(body);
    bytes.push(7); bytes.extend(b"fixture"); bytes.extend(0u16.to_be_bytes());
    let size = bytes.len() as u32; bytes[..4].copy_from_slice(&size.to_be_bytes()); bytes
}
pub async fn start() -> Result<Server> {
    // RFC2202 case 1: independently anchor the manual HMAC primitive.
    ensure!(hmac_sha1(&[0x0b; 20], b"Hi There") == [0xb6,0x17,0x31,0x86,0x55,0x05,0x72,0x64,0xe2,0x8b,0xc0,0xb6,0xfb,0x37,0x8c,0x8e,0xf1,0x46,0xbe,0x00], "RFC2202 HMAC known answer");
    let records: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let advertised = Arc::new(Mutex::new(0u16));
    let address = advertised.clone();
    let server = serve(move |mut stream, log| {
        let records = records.clone(); let address = address.clone();
        async move {
            let size = stream.read_u32().await? as usize;
            ensure!((4..=1_000_000).contains(&size), "RocketMQ frame length");
            let marker = stream.read_u32().await?;
            ensure!(marker >> 24 == 0, "RocketMQ requires JSON header marker");
            let header_size = (marker & 0x00ff_ffff) as usize;
            ensure!(header_size <= size-4, "RocketMQ header bounds");
            let mut input = vec![0; size-4]; stream.read_exact(&mut input).await?;
            let header: Value = serde_json::from_slice(&input[..header_size])?;
            let body = &input[header_size..];
            let code = header["code"].as_i64().unwrap_or(-1);
            let fields: BTreeMap<String, String> = serde_json::from_value(header["extFields"].clone())?;
            let supplied = fields.get("Signature").map(String::as_str).unwrap_or("");
            let authorized = fields.get("AccessKey").map(String::as_str) == Some("fixture") && supplied == signature(&fields, body);
            log.lock().unwrap().push(format!("code={code};acl={authorized};body_bytes={}", body.len()));
            let mut ext = json!({}); let mut response_code = 0; let mut remark = "";
            let addr = format!("127.0.0.1:{}", *address.lock().unwrap());
            let response = if !authorized { response_code = 16; remark = "SIM ACL signature/access-key rejected"; vec![] }
            else { match code {
                106 => serde_json::to_vec(&json!({"brokerAddrTable":{"broker":{"brokerName":"broker","brokerAddrs":{"0":addr}}},"clusterAddrTable":{"SIM":["broker"]}}))?,
                206 => serde_json::to_vec(&json!({"topicList":["fixture"]}))?,
                105 => {
                    if fields.get("topic").map(String::as_str) != Some("fixture") { response_code = 17; remark = "topic does not exist"; vec![] }
                    else { serde_json::to_vec(&json!({"brokerDatas":[{"brokerName":"broker","brokerAddrs":{"0":addr}}],"queueDatas":[{"brokerName":"broker","readQueueNums":1,"writeQueueNums":1}]}))? }
                }
                31 | 10 | 11 if fields.get("topic").map(String::as_str) != Some("fixture") || fields.get("queueId").map(String::as_str) != Some("0") => {
                    response_code = 17; remark = "missing topic/queue"; vec![]
                }
                31 => { ext = json!({"offset":"0"}); vec![] }
                10 => {
                    let mut records = records.lock().unwrap(); let offset = records.len(); records.push(body.to_vec());
                    ext = json!({"msgId":format!("SIM-{offset}"),"queueId":"0","queueOffset":offset.to_string()}); vec![]
                }
                11 => {
                    let offset = fields.get("queueOffset").ok_or_else(|| anyhow::anyhow!("missing queueOffset"))?.parse::<usize>()?;
                    let limit = fields.get("maxMsgNums").ok_or_else(|| anyhow::anyhow!("missing maxMsgNums"))?.parse::<usize>()?;
                    ensure!(limit > 0, "RocketMQ pull maxMsgNums must be positive");
                    let records = records.lock().unwrap(); let mut encoded = vec![];
                    for (index, payload) in records.iter().enumerate().skip(offset).take(limit) { encoded.extend(message(index, payload)); }
                    ext = json!({"nextBeginOffset":(offset+limit).min(records.len()).to_string(),"minOffset":"0","maxOffset":records.len().to_string()});
                    if encoded.is_empty() { response_code = 19; }
                    encoded
                }
                _ => { response_code = 3; remark = "unsupported remoting code"; vec![] }
            }};
            let reply_header = serde_json::to_vec(&json!({"code":response_code,"opaque":header["opaque"],"flag":1,"remark":remark,"extFields":ext}))?;
            stream.write_u32((4+reply_header.len()+response.len()) as u32).await?;
            stream.write_u32(reply_header.len() as u32).await?;
            stream.write_all(&reply_header).await?; stream.write_all(&response).await?;
            Ok(())
        }
    }).await?;
    *advertised.lock().unwrap() = server.port;
    server.log.lock().unwrap().push("acl_independent_rfc2104_hmac_sha1_rfc2202_known_answer:PASS".into());
    Ok(server)
}

/// A valid key signed over different bytes must be rejected before enqueueing.
/// This wire-only negative probe complements actual-product wrong-key/secret tests.
pub async fn reject_tampered_body(port: u16) -> Result<Value> {
    let mut fields = BTreeMap::from([("AccessKey".to_owned(), "fixture".to_owned()), ("topic".to_owned(), "fixture".to_owned()), ("queueId".to_owned(), "0".to_owned())]);
    fields.insert("Signature".into(), signature(&fields, b"original"));
    let header = serde_json::to_vec(&json!({"code":10,"opaque":99,"flag":0,"extFields":fields}))?;
    let body = b"tampered";
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
    stream.write_u32((4+header.len()+body.len()) as u32).await?;
    stream.write_u32(header.len() as u32).await?; stream.write_all(&header).await?; stream.write_all(body).await?;
    let size = stream.read_u32().await? as usize; let marker = stream.read_u32().await? as usize;
    ensure!(size >= 4 && marker <= size-4, "Rocket probe response bounds");
    let mut response = vec![0; size-4]; stream.read_exact(&mut response).await?;
    let header: Value = serde_json::from_slice(&response[..marker])?;
    ensure!(header["code"] == 16 && header["opaque"] == 99, "Rocket tampered body unexpectedly accepted: {header}");
    Ok(header)
}
