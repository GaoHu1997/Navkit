//! Apache HBase Stargate REST (2.6.x JSON contract), not Phoenix or a pretend SQL driver.
//! Scanners are real server resources and are deleted after success and every read error.
use anyhow::{anyhow,bail,Context,Result};
use async_trait::async_trait;
use base64::{engine::general_purpose::{STANDARD,URL_SAFE},Engine};
use reqwest::{Method,Url};
use serde_json::{json,Value};
use std::collections::{BTreeMap,BTreeSet,HashMap,HashSet};
use crate::{http_db::{HttpAdapter,HttpColumn,HttpExportPage,HttpRows},models::ConnectionRequest,strict_rest::{self as rest,Session,Auth}};
struct HBase {http:Session,namespace:String,export:std::sync::Mutex<HashMap<String,ExportState>>}
const MAX_EXPORT_TOKEN_BYTES:usize=64*1024;
const MAX_EXPORT_ROW_BYTES:u64=16*1024*1024;
const MAX_EXPORT_COLUMNS:usize=16384;
struct ExportState{location:Url,seq:u64,pending_key:Option<Vec<u8>>,pending:BTreeMap<Vec<u8>,(u64,Vec<u8>)>,pending_bytes:u64}
// Count serialized output without allocating another page-sized buffer.
struct ExportBudget{bytes:usize}
impl std::io::Write for ExportBudget{
    fn write(&mut self,data:&[u8])->std::io::Result<usize>{
        if data.len()>rest::MAX_BODY.saturating_sub(self.bytes){return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"HBase 导出页超过 16 MiB；未截断为成功结果"));}
        self.bytes+=data.len();Ok(data.len())
    }
    fn flush(&mut self)->std::io::Result<()>{Ok(())}
}
type Cells=BTreeMap<Vec<u8>,BTreeMap<Vec<u8>,(u64,Vec<u8>)>>;
pub(crate) async fn connect(req:&ConnectionRequest)->Result<Box<dyn HttpAdapter>> {
    let params=rest::params(&req.params,&[])?;
    let namespace=if req.database.is_empty(){"default"}else{&req.database}; name(namespace)?;
    if req.user.is_empty()&&!req.password.is_empty(){bail!("HBase REST 密码需要配套用户名；Kerberos/SPNEGO 请用显式自定义驱动");}
    let auth=if req.user.is_empty(){Auth::None}else{Auth::Basic(req.user.clone(),req.password.clone())};
    Ok(Box::new(HBase{http:Session::new(rest::endpoint(req,&params,8080,false)?,auth,req.timeout_secs)?,namespace:namespace.into(),export:std::sync::Mutex::new(HashMap::new())}))
}
fn name(value:&str)->Result<&str>{if value.is_empty()||value.len()>255||!value.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'_'|b'-'|b'.'))||matches!(value,"."|".."){bail!("HBase 名称必须为非空字母数字、点、连字符或下划线");}Ok(value)}
impl HBase {
    fn ns<'a>(&'a self,value:&'a str)->Result<&'a str>{name(if value.is_empty(){&self.namespace}else{value})}
    fn table(&self,namespace:&str,object:&str)->Result<String>{let ns=self.ns(namespace)?;name(object)?;Ok(if ns=="default"{object.into()}else{format!("{ns}:{object}")})}
    async fn namespaces(&self)->Result<Vec<String>> {
        let v=self.http.json(Method::GET,self.http.path(&["namespaces"] )?,None).await?;
        let list=v.get("Namespace").and_then(Value::as_array).ok_or_else(||anyhow!("HBase namespaces 缺少 Namespace 数组"))?;
        let mut seen=HashSet::new();let mut out=Vec::new();for ns in list {let ns=name(ns.as_str().ok_or_else(||anyhow!("HBase Namespace 项不是字符串"))?)?;if !seen.insert(ns.to_string()){bail!("HBase 返回重复 namespace");}out.push(ns.into());}Ok(out)
    }
    async fn schema(&self,namespace:&str,object:&str)->Result<Vec<String>> {
        let table=self.table(namespace,object)?;let v=self.http.json(Method::GET,self.http.path(&[&table,"schema"] )?,None).await?;
        let actual=rest::string(&v,"name")?;let alternate=format!("default:{object}");if actual!=table&&!(self.ns(namespace)?=="default"&&actual==alternate){bail!("HBase schema 返回了不同表名称");}
        let families=v.get("ColumnSchema").and_then(Value::as_array).ok_or_else(||anyhow!("HBase schema 缺少 ColumnSchema 数组"))?;
        let mut out=Vec::new();let mut seen=HashSet::new();for f in families {let family=name(rest::string(f,"name")?)?;if !seen.insert(family.to_string()){bail!("HBase schema 返回重复列族");}out.push(family.into());}if out.is_empty(){bail!("HBase 表缺少真实列族元数据");}Ok(out)
    }
    fn cellset(&self,value:&Value,rows:&mut Cells)->Result<()> {
        let list=value.get("Row").and_then(Value::as_array).ok_or_else(||anyhow!("HBase CellSet 缺少 Row 数组"))?;
        for row in list {let key=STANDARD.decode(rest::string(row,"key")?).context("HBase row key 不是有效 base64")?;
            let cells=row.get("Cell").and_then(Value::as_array).ok_or_else(||anyhow!("HBase Row 缺少 Cell 数组"))?;
            let target=rows.entry(key).or_default();for cell in cells {let column=STANDARD.decode(rest::string(cell,"column")?).context("HBase column 不是有效 base64")?;if !column.contains(&b':'){bail!("HBase Cell column 缺少 family:qualifier 分隔符");}
                let value=STANDARD.decode(rest::string(cell,"$")?).context("HBase value 不是有效 base64")?;let ts=cell.get("timestamp").and_then(Value::as_u64).ok_or_else(||anyhow!("HBase Cell timestamp 不是非负整数"))?;
                if target.get(&column).map_or(true,|(old,_)|ts>*old){target.insert(column,(ts,value));}
            }
        }Ok(())
    }
    fn grid(&self,rows:Cells,offset:u64,limit:u64)->HttpRows {
        let rows:Vec<_>=rows.into_iter().skip(offset as usize).take(limit as usize).collect();let columns:BTreeSet<Vec<u8>>=rows.iter().flat_map(|(_,cells)|cells.keys().cloned()).collect();
        let mut names=vec!["row".into()];names.extend(columns.iter().map(|c|crate::native_db::text_bytes(c)));
        HttpRows{columns:names,rows:rows.into_iter().map(|(key,cells)|{let mut row=vec![Some(crate::native_db::text_bytes(&key))];row.extend(columns.iter().map(|c|cells.get(c).map(|(_,v)|crate::native_db::text_bytes(v))));row}).collect()}
    }
    fn scanner_location(&self,table:&str,reply:&rest::Reply)->Result<Url>{
        let location=reply.headers.get(reqwest::header::LOCATION).ok_or_else(||anyhow!("HBase scanner 创建缺少 Location"))?.to_str()?;
        let mut parent=self.http.path(&[table,"scanner"])?;parent.set_path(&format!("{}/",parent.path()));let url=parent.join(location)?;
        let prefix=self.http.path(&[table,"scanner"] )?.path().to_string()+"/";
        if !self.http.same_origin(&url)||url.query().is_some()||!url.path().starts_with(&prefix){bail!("HBase scanner Location 跨 origin 或越过当前表；拒绝泄漏凭据");}
        let id=&url.path()[prefix.len()..];if id.is_empty()||!id.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'-'|b'_')){bail!("HBase scanner Location 不是安全的实例路径");}Ok(url)
    }
    async fn scan(&self,namespace:&str,object:&str,offset:u64,limit:u64)->Result<HttpRows>{
        if offset>100_000||limit>10_000{bail!("HBase 浏览 offset 上限 100000、limit 上限 10000；不能无界扫描");}
        self.schema(namespace,object).await?;if limit==0{return Ok(HttpRows{columns:vec!["row".into()],rows:Vec::new()});}
        let table=self.table(namespace,object)?;let reply=self.http.exchange(Method::PUT,self.http.path(&[&table,"scanner"] )?,Some(&json!({"batch":1000,"caching":100,"maxVersions":1}))).await?;self.http.check(&reply,&[201])?;
        let location=self.scanner_location(&table,&reply)?;
        let result:Result<HttpRows>=async {
            let mut rows=Cells::new();let mut cells=0usize;
            // A scanner batch counts cells, not rows: always merge partial rows across GETs.
            // Continue to 204 before paging, so no selected row is cut in half.
            for _ in 0..2048 {
                let mut url=location.clone();url.query_pairs_mut().append_pair("n","100").append_pair("c","1000");let reply=self.http.exchange(Method::GET,url,None).await?;
                if reply.status==204 {return Ok(self.grid(rows,offset,limit));}
                self.http.check(&reply,&[200])?;let page=reply.json()?;let list=page.get("Row").and_then(Value::as_array).ok_or_else(||anyhow!("HBase scanner 返回无效 CellSet"))?;if list.is_empty(){bail!("HBase scanner 返回空 200，未使用 204 结束；拒绝无界循环");}
                cells+=list.iter().filter_map(|r|r.get("Cell").and_then(Value::as_array)).map(Vec::len).sum::<usize>();self.cellset(&page,&mut rows)?;
                if rows.len()>100_000||cells>1_000_000{bail!("HBase 扫描超过 100000 行或 1000000 个 cell；结果未截断为成功");}
            }bail!("HBase scanner 超过 2048 页；结果未截断为成功")
        }.await;
        let cleanup:Result<()>=async {let reply=self.http.exchange(Method::DELETE,location,None).await?;self.http.check(&reply,&[200,204,404,410])}.await;
        match (result,cleanup){(Ok(rows),Ok(()))=>Ok(rows),(Err(error),Ok(()))=>Err(error),(Ok(_),Err(error))=>Err(error.context("HBase scanner 释放失败")),(Err(error),Err(cleanup))=>Err(anyhow!("{error:#}；scanner 释放也失败：{cleanup:#}"))}
    }
    fn bytes(&self,v:&Value,text:&str,encoded:&str)->Result<Vec<u8>>{match(v.get(text),v.get(encoded)){(Some(Value::String(s)),None)=>Ok(s.as_bytes().to_vec()),(None,Some(Value::String(s)))=>STANDARD.decode(s).context("HBase 命令字段不是有效 base64"),_=>bail!("HBase 命令必须且只能提供 {text} 或 {encoded} 字符串")}}
    fn row_url(&self,table:&str,row:&[u8])->Result<Url>{if row.is_empty(){bail!("HBase row key 不能为空");}let mut url=self.http.path(&[table,&URL_SAFE.encode(row)])?;url.query_pairs_mut().append_pair("keyencoding","b64");Ok(url)}
    fn scanner_id(&self,location:&Url)->Result<String>{
        let segments:Vec<&str>=location.path_segments().map(|p|p.collect()).unwrap_or_default();
        if segments.len()<2||segments[segments.len()-2]!="scanner"{bail!("HBase scanner Location 不是安全的实例路径");}
        let id=segments[segments.len()-1];
        if id.is_empty()||!id.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'-'|b'_')){bail!("HBase scanner Location 不是安全的实例路径");}
        Ok(id.to_string())
    }
    fn export_token(&self,location:&Url,seq:u64)->Result<String>{
        let mut url=location.clone();url.set_query(None);url.query_pairs_mut().append_pair("p",&seq.to_string());let token=url.to_string();
        if token.len()>MAX_EXPORT_TOKEN_BYTES{bail!("HBase 导出游标超过 {} 字节上限",MAX_EXPORT_TOKEN_BYTES);}
        Ok(token)
    }
    fn parse_export_token(&self,token:&str)->Result<(String,Url,u64)>{
        if token.len()>MAX_EXPORT_TOKEN_BYTES{bail!("HBase 导出游标超过 {} 字节上限",MAX_EXPORT_TOKEN_BYTES);}
        let url=Url::parse(token).map_err(|_|anyhow!("HBase 导出游标不是合法 URL"))?;
        if !self.http.same_origin(&url)||url.fragment().is_some(){bail!("HBase 导出游标越过当前 origin；拒绝泄漏凭据");}
        let mut seq=None;
        for (key,value) in url.query_pairs(){if key=="p"&&seq.is_none(){seq=Some(value.parse::<u64>().map_err(|_|anyhow!("HBase 导出游标序号无效"))?);}else{bail!("HBase 导出游标包含未知参数");}}
        let seq=seq.ok_or_else(||anyhow!("HBase 导出游标缺少序号"))?;
        let mut base=url.clone();base.set_query(None);
        let id=self.scanner_id(&base)?;
        Ok((id,base,seq))
    }
    async fn delete_scanner(&self,location:&Url)->Result<()>{
        let reply=self.http.exchange(Method::DELETE,location.clone(),None).await?;
        self.http.check(&reply,&[200,204,404,410])
    }
    fn grid_rows(&self,rows:Vec<(Vec<u8>,BTreeMap<Vec<u8>,(u64,Vec<u8>)>)>)->Result<HttpRows>{
        let columns:BTreeSet<Vec<u8>>=rows.iter().flat_map(|(_,cells)|cells.keys().cloned()).collect();
        if columns.len()>MAX_EXPORT_COLUMNS{bail!("HBase 单页列数超过 {} 上限",MAX_EXPORT_COLUMNS);}
        let mut names=vec!["row".into()];names.extend(columns.iter().map(|c|crate::native_db::text_bytes(c)));
        if rows.len().checked_mul(names.len()).is_none_or(|cells|cells>rest::MAX_BODY/5){bail!("HBase 稀疏列展开的导出页超过 16 MiB；未截断为成功结果");}
        let mut budget=ExportBudget{bytes:4};
        serde_json::to_writer(&mut budget,&names).context("HBase 导出列名超过页字节上限")?;
        let mut out=Vec::with_capacity(rows.len());
        for (key,cells) in rows{
            let mut bytes=key.len() as u64;
            for (column,(_,value)) in &cells{bytes=bytes.checked_add(column.len() as u64).and_then(|n|n.checked_add(value.len() as u64)).ok_or_else(||anyhow!("HBase 单行字节数溢出"))?;}
            if bytes>MAX_EXPORT_ROW_BYTES{bail!("HBase 单行超过 {} 字节上限",MAX_EXPORT_ROW_BYTES);}
            let mut row=vec![Some(crate::native_db::text_bytes(&key))];
            row.extend(columns.iter().map(|c|cells.get(c).map(|(_,v)|crate::native_db::text_bytes(v))));
            std::io::Write::write_all(&mut budget,b",").context("HBase 导出页超过 16 MiB")?;
            serde_json::to_writer(&mut budget,&row).context("HBase 导出页超过 16 MiB")?;
            out.push(row);
        }
        Ok(HttpRows{columns:names,rows:out})
    }
    async fn export_scan(&self,container:&str,object:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{
        if limit==0{
            let columns=self.table_columns(container,object).await?.unwrap_or_default().into_iter().map(|column|column.name).collect();
            return Ok(HttpExportPage{data:HttpRows{columns,rows:Vec::new()},next:None});
        }
        let table=self.table(container,object)?;
        let mut state=match token{
            None=>{
                self.schema(container,object).await?;
                let reply=self.http.exchange(Method::PUT,self.http.path(&[&table,"scanner"])?,Some(&json!({"batch":100,"caching":100,"maxVersions":1}))).await?;
                self.http.check(&reply,&[201])?;
                let location=self.scanner_location(&table,&reply)?;
                ExportState{location,seq:0,pending_key:None,pending:BTreeMap::new(),pending_bytes:0}
            }
            Some(raw)=>{
                let (_,location,seq)=self.parse_export_token(raw)?;
                let prefix=self.http.path(&[&table,"scanner"])?.path().to_string()+"/";
                if !location.path().starts_with(&prefix){bail!("HBase 导出游标属于不同表");}
                let mut active=self.export.lock().map_err(|_|anyhow!("HBase 导出状态锁不可用"))?;
                let saved=active.get(location.as_str()).ok_or_else(||anyhow!("HBase 导出游标已失效或已完成；请重新开始导出"))?;
                if saved.seq!=seq{bail!("HBase 导出游标序号已过期；拒绝重复推进 scanner");}
                active.remove(location.as_str()).ok_or_else(||anyhow!("HBase 导出游标已失效"))?
            }
        };
        let mut completed:Vec<(Vec<u8>,BTreeMap<Vec<u8>,(u64,Vec<u8>)>)>=Vec::new();
        let mut completed_bytes=0u64;
        let mut eof=false;
        let result:Result<HttpExportPage>=async{
            for _ in 0..512{
                let mut url=state.location.clone();url.query_pairs_mut().append_pair("n","100").append_pair("c","1000");
                let reply=self.http.exchange(Method::GET,url,None).await?;
                if reply.status==204{eof=true;break;}
                self.http.check(&reply,&[200])?;
                let page=reply.json()?;
                let list=page.get("Row").and_then(Value::as_array).ok_or_else(||anyhow!("HBase scanner 返回无效 CellSet"))?;
                if list.is_empty(){bail!("HBase scanner 返回空 200，未使用 204 结束；拒绝无界循环");}
                for row in list{
                    let key=STANDARD.decode(rest::string(row,"key")?).context("HBase row key 不是有效 base64")?;
                    if key.is_empty(){bail!("HBase row key 不能为空");}
                    let cells=row.get("Cell").and_then(Value::as_array).filter(|cells|!cells.is_empty()).ok_or_else(||anyhow!("HBase Row 缺少非空 Cell 数组"))?;
                    if state.pending_key.as_ref()!=Some(&key){
                        if let Some(previous)=state.pending_key.take(){
                            if key<=previous{bail!("HBase scanner row key 未按升序推进；拒绝重复或丢失行");}
                            completed_bytes=completed_bytes.checked_add(state.pending_bytes).ok_or_else(||anyhow!("HBase 导出页字节数溢出"))?;
                            if completed_bytes>MAX_EXPORT_ROW_BYTES{bail!("HBase 导出页超过 16 MiB；未截断为成功结果");}
                            completed.push((previous,std::mem::take(&mut state.pending)));
                        }
                        state.pending_bytes=key.len() as u64;
                        state.pending_key=Some(key);
                    }
                    for cell in cells{
                        let column=STANDARD.decode(rest::string(cell,"column")?).context("HBase column 不是有效 base64")?;
                        if column.first()==Some(&b':')||!column.contains(&b':'){bail!("HBase Cell column 缺少非空 family:qualifier 分隔符");}
                        let value=STANDARD.decode(rest::string(cell,"$")?).context("HBase value 不是有效 base64")?;
                        let timestamp=cell.get("timestamp").and_then(Value::as_u64).ok_or_else(||anyhow!("HBase Cell timestamp 不是非负整数"))?;
                        match state.pending.get(&column){
                            Some((old,old_value)) if timestamp==*old&&value!=*old_value=>bail!("HBase 同一 cell/timestamp 返回冲突值；拒绝静默覆盖"),
                            Some((old,_)) if timestamp<=*old=>continue,
                            _=>{}
                        }
                        let old_bytes=state.pending.get(&column).map_or(0,|(_,v)|column.len()+v.len());
                        let new_bytes=column.len().checked_add(value.len()).ok_or_else(||anyhow!("HBase 单行字节数溢出"))?;
                        state.pending_bytes=state.pending_bytes.checked_sub(old_bytes as u64).and_then(|n|n.checked_add(new_bytes as u64)).ok_or_else(||anyhow!("HBase 单行字节数溢出"))?;
                        if state.pending_bytes>MAX_EXPORT_ROW_BYTES{bail!("HBase 单行超过 {} 字节上限",MAX_EXPORT_ROW_BYTES);}
                        if !state.pending.contains_key(&column)&&state.pending.len()>=MAX_EXPORT_COLUMNS{bail!("HBase 单行列数超过 {} 上限",MAX_EXPORT_COLUMNS);}
                        state.pending.insert(column,(timestamp,value));
                    }
                }
                // Never discard the tail of a broker batch to satisfy a requested row count.
                // The last row stays pending until the next key or EOF proves it complete.
                if !completed.is_empty(){break;}
            }
            if !eof&&completed.is_empty(){bail!("HBase scanner 连续 512 页未产出完整行；结果未截断为成功");}
            if eof{
                if let Some(key)=state.pending_key.take(){completed.push((key,std::mem::take(&mut state.pending)));}
            }
            let data=self.grid_rows(completed)?;
            let next=if eof{None}else{
                let next_seq=state.seq.checked_add(1).ok_or_else(||anyhow!("HBase 导出游标序号溢出"))?;
                let token=self.export_token(&state.location,next_seq)?;
                state.seq=next_seq;
                Some(token)
            };
            Ok(HttpExportPage{data,next})
        }.await;
        let page=match result{
            Ok(page)=>page,
            Err(error)=>{
                return match self.delete_scanner(&state.location).await{
                    Ok(())=>Err(error),Err(cleanup)=>Err(anyhow!("{error:#}；scanner 释放也失败：{cleanup:#}"))
                };
            }
        };
        if eof{self.delete_scanner(&state.location).await.context("HBase scanner 释放失败")?;}else{
            let location=state.location.clone();
            let inserted=match self.export.lock(){
                Ok(mut active)=>{active.insert(location.to_string(),state);true},Err(_)=>false
            };
            if !inserted{let _=self.delete_scanner(&location).await;bail!("HBase 导出状态锁不可用");}
        }
        Ok(page)
    }
}
#[async_trait]
impl HttpAdapter for HBase {
    fn shares_session_namespace(&self)->bool{true}
    async fn ping(&self)->Result<String>{let v=self.http.json(Method::GET,self.http.path(&["version","cluster"] )?,None).await?;let version=rest::string(&v,"Version")?;if version.is_empty(){bail!("HBase cluster Version 为空");}self.namespaces().await?;Ok(format!("HBase Stargate REST {version}"))}
    async fn containers(&self)->Result<Vec<(String,String)>>{Ok(self.namespaces().await?.into_iter().map(|n|(n.clone(),n)).collect())}
    async fn objects(&self,container:&str)->Result<Vec<String>>{let ns=self.ns(container)?;let v=self.http.json(Method::GET,self.http.path(&["namespaces",ns,"tables"] )?,None).await?;let tables=v.get("table").and_then(Value::as_array).ok_or_else(||anyhow!("HBase namespace tables 缺少 table 数组"))?;let mut out=Vec::new();let mut seen=HashSet::new();for t in tables {let table=name(rest::string(t,"name")?)?;if !seen.insert(table.to_string()){bail!("HBase 返回重复表");}out.push(table.into());}Ok(out)}
    async fn table_columns(&self,container:&str,object:&str)->Result<Option<Vec<HttpColumn>>>{let families=self.schema(container,object).await?;let mut columns=vec![HttpColumn{name:"row".into(),data_type:"binary row key".into(),nullable:false,primary_key:true}];columns.extend(families.into_iter().map(|name|HttpColumn{name,data_type:"column family (dynamic qualifiers)".into(),nullable:true,primary_key:false}));Ok(Some(columns))}
    async fn rows(&self,container:&str,object:&str,offset:u64,limit:u64)->Result<HttpRows>{self.scan(container,object,offset,limit).await}
    async fn run(&self,container:&str,object:&str,sql:&str)->Result<HttpRows>{
        let v:Value=serde_json::from_str(sql).context("HBase 接受 JSON op=scan/get/put/delete；不是 SQL 引擎")?;let op=rest::string(&v,"op")?;
        let table_name=v.get("table").and_then(Value::as_str).unwrap_or(object);let table=self.table(container,table_name)?;
        match op {
            "scan"=>{rest::fields(&v,&["op","table","offset","limit"])?;let number=|key:&str,default:u64|->Result<u64>{v.get(key).map_or(Ok(default),|x|x.as_u64().ok_or_else(||anyhow!("HBase {key} 必须为非负整数")))};self.scan(container,table_name,number("offset",0)?,number("limit",200)?).await},
            "get"|"delete"=>{rest::fields(&v,&["op","table","row","row_base64"])?;let row=self.bytes(&v,"row","row_base64")?;let reply=self.http.exchange(if op=="get"{Method::GET}else{Method::DELETE},self.row_url(&table,&row)?,None).await?;if op=="get"{self.http.check(&reply,&[200,404])?;let mut rows=Cells::new();if reply.status==200{self.cellset(&reply.json()?,&mut rows)?;if rows.len()!=1||!rows.contains_key(&row){bail!("HBase get 返回了不同 row key");}}Ok(self.grid(rows,0,1))}else{self.http.check(&reply,&[200,204])?;Ok(crate::native_db::single("acknowledged","true"))}},
            "put"=>{rest::fields(&v,&["op","table","row","row_base64","cells"])?;let row=self.bytes(&v,"row","row_base64")?;let cells=v.get("cells").and_then(Value::as_array).filter(|c|!c.is_empty()&&c.len()<=10000).ok_or_else(||anyhow!("HBase cells 必须为 1..10000 个 cell"))?;let mut encoded=Vec::new();for cell in cells {rest::fields(cell,&["column","column_base64","value","value_base64"])?;let column=self.bytes(cell,"column","column_base64")?;if column.first()==Some(&b':')||!column.contains(&b':'){bail!("HBase column 必须包含非空 family:qualifier");}let value=self.bytes(cell,"value","value_base64")?;encoded.push(json!({"column":STANDARD.encode(column),"$":STANDARD.encode(value)}));}let body=json!({"Row":[{"key":STANDARD.encode(&row),"Cell":encoded}]});let reply=self.http.exchange(Method::PUT,self.row_url(&table,&row)?,Some(&body)).await?;self.http.check(&reply,&[200])?;Ok(crate::native_db::single("acknowledged","true"))},
            _=>bail!("HBase 支持 JSON op=scan/get/put/delete；未知命令不会假成功")
        }
    }
    async fn export_page(&self,container:&str,object:&str,token:Option<&str>,limit:u64)->Result<HttpExportPage>{self.export_scan(container,object,token,limit).await}
    async fn finish_export(&self,token:Option<&str>)->Result<()>{
        let Some(raw)=token else{return Ok(());};
        let (_,location,seq)=self.parse_export_token(raw)?;
        let saved={
            let mut active=self.export.lock().map_err(|_|anyhow!("HBase 导出状态锁不可用"))?;
            let Some(saved)=active.get(location.as_str()) else{return Ok(());};
            if saved.seq!=seq{bail!("HBase 导出游标序号已过期；拒绝释放正在使用的 scanner");}
            active.remove(location.as_str())
        };
        if let Some(saved)=saved{self.delete_scanner(&saved.location).await.context("HBase scanner 释放失败")?;}
        Ok(())
    }
}

#[cfg(test)]
#[path = "rest_export_tests.rs"]
mod rest_export_tests;
