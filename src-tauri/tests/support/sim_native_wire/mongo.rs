//! MongoDB OP_QUERY/OP_MSG simulation with live typed records and real code-18 auth rejection.
//! Anonymous access is deliberately allowed; this fixture does not implement successful SCRAM.
use super::*;
use anyhow::{anyhow, bail, ensure, Result};
use mongodb::bson::{doc, Bson, Document};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::sync::atomic::{AtomicBool, Ordering};

fn cursor(namespace: &str, records: Vec<Document>) -> Document {
    doc! {"ok":1.0,"cursor":{"id":0i64,"ns":namespace,"firstBatch":records}}
}
fn documents(records: &Records) -> Vec<Document> {
    records.lock().unwrap().iter().map(|r| {
        let cell = |i:usize| r.get(i).and_then(Option::as_ref).map(|s|Bson::String(s.clone())).unwrap_or(Bson::Null);
        doc! {"_id":cell(0),"payload":cell(1),"nullable_value":cell(2)}
    }).collect()
}
fn matches(row: &Document, query: &Document) -> bool {
    query.iter().all(|(key,value)| row.get(key)==Some(value))
}
fn command(input: &Document, records: &Records, count_fault: &AtomicBool, log: &Arc<Mutex<Vec<String>>>) -> Result<Document> {
    let name=input.keys().next().map(String::as_str).unwrap_or("");
    let database=input.get_str("$db").unwrap_or("admin");
    log.lock().unwrap().push(format!("command:{name};db:{database}"));
    Ok(match name {
        "isMaster"|"ismaster"|"hello" => doc! {"ok":1.0,"isWritablePrimary":true,"ismaster":true,"helloOk":true,
            "minWireVersion":0,"maxWireVersion":17,"maxBsonObjectSize":16777216,"maxMessageSizeBytes":48000000,"maxWriteBatchSize":100000,"connectionId":1},
        "buildInfo" => doc! {"ok":1.0,"version":"6.0.0-SIM","versionArray":[6,0,0,0],"maxBsonObjectSize":16777216},
        "ping"|"endSessions"|"killCursors" => doc! {"ok":1.0},
        "saslStart"|"saslContinue" => {
            log.lock().unwrap().push("authentication_rejected:18".into());
            doc! {"ok":0.0,"code":18,"codeName":"AuthenticationFailed","errmsg":"SIM rejects unknown credential"}
        },
        "listDatabases" => doc! {"ok":1.0,"databases":[{"name":"fixture","sizeOnDisk":1024i64,"empty":records.lock().unwrap().is_empty()}],"totalSize":1024i64},
        "listCollections" => {
            let rows=if database=="fixture" {vec![doc!{"name":"items","type":"collection","options":{}}]}else{vec![]};
            cursor(&format!("{database}.$cmd.listCollections"),rows)
        },
        "find" => {
            if database!="fixture" || input.get_str("find")?!="items" {missing()}
            else {
                let filter=input.get_document("filter").cloned().unwrap_or_default();
                let skip=integer(input,"skip").unwrap_or(0).max(0) as usize;
                let limit=integer(input,"limit").unwrap_or(0).unsigned_abs() as usize;
                // Mongo wire limit=0 means unbounded. Do not hide product paging bugs.
                let count=if limit==0 {usize::MAX}else{limit};
                cursor("fixture.items",documents(records).into_iter().filter(|d|matches(d,&filter)).skip(skip).take(count).collect())
            }
        },
        "aggregate" => {
            if count_fault.load(Ordering::SeqCst) {
                log.lock().unwrap().push("business_error:count:8000".into());
                doc! {"ok":0.0,"code":8000,"codeName":"SimulationCountFault","errmsg":"SIM deliberate count business fault"}
            } else if database!="fixture" || input.get_str("aggregate")?!="items" {missing()}
            else {
                let pipeline=input.get_array("pipeline")?;
                let filter=pipeline.iter().filter_map(Bson::as_document).find_map(|d|d.get_document("$match").ok()).cloned().unwrap_or_default();
                let count=documents(records).iter().filter(|d|matches(d,&filter)).count();
                cursor("fixture.items", if count==0 {vec![]}else{vec![doc!{"n":count as i64}]})
            }
        },
        _=> doc! {"ok":0.0,"code":59,"codeName":"CommandNotFound","errmsg":format!("SIM unsupported command {name}")},
    })
}
fn missing() -> Document {doc! {"ok":0.0,"code":26,"codeName":"NamespaceNotFound","errmsg":"SIM missing collection"}}
fn integer(input: &Document, name: &str) -> Option<i64> {
    input.get(name).and_then(|v|match v {Bson::Int32(v)=>Some(*v as i64),Bson::Int64(v)=>Some(*v),_=>None})
}
pub async fn start(records: Records, count_fault: Arc<AtomicBool>) -> Result<Server> {
    serve(move |mut stream,log| {
        let records=records.clone();
        let count_fault=count_fault.clone();
        async move {
            loop {
                let size=match stream.read_i32_le().await {Ok(n)=>n,Err(_)=>break};
                ensure!((16..=1_000_000).contains(&size),"Mongo frame length");
                let id=stream.read_i32_le().await?;let _response_to=stream.read_i32_le().await?;let opcode=stream.read_i32_le().await?;
                let mut body=vec![0;size as usize-16];stream.read_exact(&mut body).await?;
                let mut input=if opcode==2004 {
                    ensure!(body.len()>=4,"Mongo OP_QUERY flags");
                    let end=body[4..].iter().position(|x|*x==0).ok_or_else(||anyhow!("OP_QUERY namespace"))?+4;
                    ensure!(end+9<=body.len(),"OP_QUERY bounds");
                    Document::from_reader(&mut &body[end+9..])?
                }else if opcode==2013 {
                    ensure!(body.len()>=5 && body[4]==0,"Mongo OP_MSG kind-0 section");
                    let flags=u32::from_le_bytes(body[..4].try_into()?);ensure!(flags==0,"unsupported OP_MSG flags");
                    Document::from_reader(&mut &body[5..])?
                }else{bail!("unsupported Mongo opcode {opcode}")};
                if opcode==2004 && !input.contains_key("$db") {input.insert("$db","admin");}
                let response=command(&input,&records,&count_fault,&log)?;
                let mut encoded=vec![];response.to_writer(&mut encoded)?;
                let mut reply=vec![];
                if opcode==2004 {
                    reply.extend(0i32.to_le_bytes());reply.extend(0i64.to_le_bytes());reply.extend(0i32.to_le_bytes());reply.extend(1i32.to_le_bytes());
                }else{reply.extend(0i32.to_le_bytes());reply.push(0);}
                reply.extend(encoded);
                stream.write_i32_le((16+reply.len()) as i32).await?;stream.write_i32_le(500).await?;stream.write_i32_le(id).await?;
                stream.write_i32_le(if opcode==2004 {1}else{2013}).await?;stream.write_all(&reply).await?;
            }
            Ok(())
        }
    }).await
}
