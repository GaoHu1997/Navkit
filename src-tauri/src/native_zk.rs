//! ZooKeeper 原生协议，保留ACL错误；目录来自真实znode而非占位。
use anyhow::{anyhow,Result};use async_trait::async_trait;use serde_json::{Value,json};use tokio::time::timeout;
use crate::{http_db::{HttpAdapter,HttpRows},models::ConnectionRequest,native_db::{Params,text_bytes,single}};
use zookeeper_client::{Client,CreateMode,Acls};
pub(crate)struct ZkDb{client:Client,p:Params,root:String,authenticated:bool}
impl ZkDb{
 pub async fn connect(r:&ConnectionRequest)->Result<Self>{let p=Params::new(r);p.no_tls("ZooKeeper")?;let mut co=Client::connector().with_connection_timeout(p.timeout);if !r.user.is_empty(){co=co.with_auth(p.get("auth_scheme").unwrap_or("digest"),format!("{}:{}",r.user,r.password).as_bytes());}let hosts=p.get("hosts").map(str::to_owned).unwrap_or_else(||format!("{}:{}",r.host,if r.port==0{2181}else{r.port}));let client=timeout(p.timeout,co.connect(&hosts)).await.map_err(|_|anyhow!("ZooKeeper连接超时"))??;let root=if r.database.is_empty(){"/".into()}else{r.database.clone()};if !root.starts_with('/'){return Err(anyhow!("ZooKeeper数据库须为/开头的znode路径"));}timeout(p.timeout,client.get_data(&root)).await.map_err(|_|anyhow!("ZooKeeper根节点访问超时"))??;Ok(Self{client,p,root,authenticated:!r.user.is_empty()})}
 fn path(c:&str,o:&str)->String{if o.starts_with('/'){o.to_owned()}else if o.is_empty(){if c.is_empty(){"/".into()}else{c.to_string()}}else{format!("{}/{}",c.trim_end_matches('/'),o)}}
 async fn get(&self,path:&str)->Result<HttpRows>{let(data,stat)=timeout(self.p.timeout,self.client.get_data(path)).await.map_err(|_|anyhow!("ZooKeeper读取超时"))??;Ok(HttpRows{columns:["path","data","version","children","created_ms","modified_ms","ephemeral_owner"].iter().map(|s|s.to_string()).collect(),rows:vec![vec![Some(path.into()),Some(text_bytes(&data)),Some(stat.version.to_string()),Some(stat.num_children.to_string()),Some(stat.ctime.to_string()),Some(stat.mtime.to_string()),Some(stat.ephemeral_owner.to_string())]]})}
}
#[async_trait]impl HttpAdapter for ZkDb{
 async fn ping(&self)->Result<String>{self.get(&self.root).await?;Ok("ZooKeeper (native session and ACL verified)".into())}
 async fn containers(&self)->Result<Vec<(String,String)>>{let(nodes,_)=timeout(self.p.timeout,self.client.get_children(&self.root)).await.map_err(|_|anyhow!("ZooKeeper目录超时"))??;Ok(nodes.into_iter().map(|n|(Self::path(&self.root,&n),"znode".into())).collect())}
 async fn objects(&self,c:&str)->Result<Vec<String>>{let(nodes,_)=timeout(self.p.timeout,self.client.get_children(c)).await.map_err(|_|anyhow!("ZooKeeper目录超时"))??;Ok(nodes.into_iter().map(|n|Self::path(c,&n)).collect())}
 async fn rows(&self,c:&str,o:&str,offset:u64,limit:u64)->Result<HttpRows>{let mut r=self.get(&Self::path(c,o)).await?;r.rows=r.rows.into_iter().skip(offset as usize).take(limit as usize).collect();Ok(r)}
 async fn run(&self,c:&str,o:&str,s:&str)->Result<HttpRows>{let v:Value=serde_json::from_str(s).map_err(|_|anyhow!("ZooKeeper命令须为JSON operation=get/children/set/create/delete"))?;let path=v["path"].as_str().map(str::to_owned).unwrap_or_else(||Self::path(c,o));let version=v["version"].as_i64().map(|v|v as i32);match v["operation"].as_str().unwrap_or("get"){
 "get"=>self.get(&path).await,
 "children"=>{let(nodes,_)=timeout(self.p.timeout,self.client.get_children(&path)).await.map_err(|_|anyhow!("ZooKeeper目录超时"))??;Ok(HttpRows{columns:vec!["path".into()],rows:nodes.into_iter().map(|n|vec![Some(Self::path(&path,&n))]).collect()})},
 "set"|"create"=>{let data=v.get("data").ok_or_else(||anyhow!("缺少data"))?;let data=data.as_str().map(str::to_owned).unwrap_or_else(||data.to_string()).into_bytes();if v["operation"]=="set"{let stat=timeout(self.p.timeout,self.client.set_data(&path,&data,version)).await.map_err(|_|anyhow!("ZooKeeper写入超时"))??;Ok(single("version",stat.version.to_string()))}else{let acls=if self.authenticated{Acls::creator_all()}else{Acls::anyone_all()};let opts=CreateMode::Persistent.with_acls(acls);let(stat,_)=timeout(self.p.timeout,self.client.create(&path,&data,&opts)).await.map_err(|_|anyhow!("ZooKeeper创建超时"))??;Ok(single("version",stat.version.to_string()))}},
 "delete"=>{timeout(self.p.timeout,self.client.delete(&path,version)).await.map_err(|_|anyhow!("ZooKeeper删除超时"))??;Ok(single("deleted",json!(path).to_string()))},_=>Err(anyhow!("未知ZooKeeper操作"))}}
}
#[cfg(test)]mod tests{use super::*;#[test]fn paths(){assert_eq!(ZkDb::path("/","中文"),"/中文");assert_eq!(ZkDb::path("/a","/b"),"/b");}}
