//! Stateful, loopback-only wire simulators. Never constructs or substitutes a product Db.
use std::{collections::BTreeMap, io::{Read,Write}, net::{TcpListener,TcpStream}, sync::{Arc,Mutex,atomic::{AtomicBool,Ordering}}, thread::{self,JoinHandle}, time::Duration};
use serde_json::{json,Value};
use base64::Engine;
mod routes;
pub use routes::{coordinates,query};

#[derive(Clone)]
pub struct Data {
    pub kind:String, pub object:String, pub container:String, pub sentinel:String,
    pub docs:Vec<Value>, pub revision:u64, pub fault:Option<String>, pub actions:Vec<Value>,
    pub requests:Vec<Value>, pub violations:Vec<String>,
}
impl Data {
    pub fn new(kind:&str)->Self {
        let tag=uuid::Uuid::new_v4().simple().to_string();
        let object=format!("sim_{}",tag);
        let (container,object)=coordinates(kind,&object);
        let sentinel=format!("模拟中文;O'Reilly\\尾-{tag}");
        let docs=(0..4).map(|i|json!({"id":i+1,"payload":format!("{sentinel}-row{i}"),"nullable":null})).collect();
        Self {kind:kind.into(),object,container,sentinel,docs,revision:1,fault:None,actions:vec![],requests:vec![],violations:vec![]}
    }
    pub fn change(&mut self) {
        self.revision+=1;
        self.sentinel=format!("{}-changed",self.sentinel);
        for (i,d) in self.docs.iter_mut().enumerate(){d["payload"]=json!(format!("{}-row{i}",self.sentinel));}
        self.actions.push(json!({"action":"typed_fixture_mutation","revision":self.revision,"rows":self.docs.len()}));
    }
}
pub struct Request { pub method:String,pub target:String,pub path:String,pub query:BTreeMap<String,String>,pub headers:BTreeMap<String,String>,pub body:Vec<u8>,pub json:Value }
impl Request {
    pub fn text(&self)->String{String::from_utf8_lossy(&self.body).to_string()}
    pub fn q(&self,n:&str)->&str{self.query.get(n).map(String::as_str).unwrap_or("")}
    pub fn number(&self,n:&str,default:usize)->usize{self.q(n).parse().unwrap_or(default)}
}
pub struct Response{pub status:u16,pub body:Vec<u8>,pub headers:Vec<(String,String)>}
impl Response {
    pub fn json(v:Value)->Self{Self{status:200,body:v.to_string().into_bytes(),headers:vec![("Content-Type".into(),"application/json".into())]}}
    pub fn text(s:String)->Self{Self{status:200,body:s.into_bytes(),headers:vec![]}}
    pub fn error(status:u16,message:&str)->Self{let mut r=Self::json(json!({"error":message}));r.status=status;r}
}
pub struct Server{pub port:u16,pub data:Arc<Mutex<Data>>,stop:Arc<AtomicBool>,worker:Option<JoinHandle<()>>}
impl Server {
    pub fn start(kind:&str)->Self {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let port=listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let data=Arc::new(Mutex::new(Data::new(kind)));let stop=Arc::new(AtomicBool::new(false));
        let worker_data=data.clone();let worker_stop=stop.clone();
        let worker=thread::spawn(move||{
            while !worker_stop.load(Ordering::SeqCst){
                match listener.accept(){
                    Ok((mut stream,_))=>{
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                        stream.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
                        match read_request(&mut stream){
                            Ok(r)=>{let mut d=worker_data.lock().unwrap();
                                let revision=d.revision;let auth=routes::authenticated(&r,&d);
                                d.requests.push(json!({"method":r.method,"path":r.path,"query":r.query,"body":r.json,"body_text":if r.json.is_null(){Some(r.text())}else{None},"authenticated":auth,"revision":revision}));
                                let response=routes::handle(&r,&mut d);write_response(&mut stream,response);
                            },
                            Err(e)=>worker_data.lock().unwrap().violations.push(e),
                        }
                    },
                    Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>thread::park_timeout(Duration::from_millis(2)),
                    Err(e)=>{worker_data.lock().unwrap().violations.push(e.to_string());break;},
                }
            }
        });
        Self{port,data,stop,worker:Some(worker)}
    }
    pub fn snapshot(&self)->Data{self.data.lock().unwrap().clone()}
    pub fn change(&self){self.data.lock().unwrap().change();}
    pub fn fault(&self,name:&str){self.data.lock().unwrap().fault=Some(name.into());}
}
impl Drop for Server{fn drop(&mut self){self.stop.store(true,Ordering::SeqCst);if let Some(w)=self.worker.take(){w.thread().unpark();w.join().unwrap();}}}
fn read_request(stream:&mut TcpStream)->Result<Request,String>{
    let mut buf=vec![];let end=loop{let mut b=[0u8;2048];let n=stream.read(&mut b).map_err(|e|e.to_string())?;if n==0{return Err("unexpected EOF before HTTP headers".into())}buf.extend_from_slice(&b[..n]);if buf.len()>2*1024*1024{return Err("HTTP simulator size limit".into())}if let Some(i)=buf.windows(4).position(|w|w==b"\r\n\r\n"){break i+4}};
    let header=String::from_utf8(buf[..end].to_vec()).map_err(|e|e.to_string())?;let mut lines=header.split("\r\n");let request_line=lines.next().unwrap().split_whitespace().collect::<Vec<_>>();if request_line.len()!=3{return Err("invalid HTTP request line".into())}
    let method=request_line[0].into();let target=request_line[1].to_string();let mut headers=BTreeMap::new();
    for line in lines{if let Some((k,v))=line.split_once(':'){headers.insert(k.to_ascii_lowercase(),v.trim().to_string());}}
    if headers.contains_key("transfer-encoding"){return Err("simulator requires bounded Content-Length, chunked not accepted".into())}
    let len=headers.get("content-length").and_then(|s|s.parse::<usize>().ok()).unwrap_or(0);if len>1024*1024{return Err("body limit".into())}
    while buf.len()<end+len{let mut b=[0u8;4096];let n=stream.read(&mut b).map_err(|e|e.to_string())?;if n==0{return Err("unexpected body EOF".into())}buf.extend_from_slice(&b[..n]);}
    let url=reqwest::Url::parse(&format!("http://127.0.0.1{target}")).map_err(|e|e.to_string())?;
    let path=percent_decode(url.path());let query=url.query_pairs().map(|(k,v)|(k.into_owned(),v.into_owned())).collect();let body=buf[end..end+len].to_vec();let json=serde_json::from_slice(&body).unwrap_or(Value::Null);
    Ok(Request{method,target,path,query,headers,body,json})
}
fn percent_decode(s:&str)->String{let b=s.as_bytes();let mut out=vec![];let mut i=0;while i<b.len(){if b[i]==b'%'&&i+2<b.len(){if let Ok(v)=u8::from_str_radix(&s[i+1..i+3],16){out.push(v);i+=3;continue;}}out.push(b[i]);i+=1;}String::from_utf8(out).unwrap()}
fn write_response(stream:&mut TcpStream,response:Response){let reason=if response.status==200{"OK"}else{"Error"};let mut h=format!("HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",response.status,reason,response.body.len());for(k,v)in response.headers{h.push_str(&format!("{k}: {v}\r\n"));}h.push_str("\r\n");let _=stream.write_all(h.as_bytes());let _=stream.write_all(&response.body);}
pub fn b64(s:&str)->String{base64::engine::general_purpose::STANDARD.encode(s)}
pub fn unb64(v:&Value)->String{v.as_str().and_then(|s|base64::engine::general_purpose::STANDARD.decode(s).ok()).and_then(|b|String::from_utf8(b).ok()).unwrap_or_default()}
