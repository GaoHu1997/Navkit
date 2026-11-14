//! Explicit SQL export grouping. This changes restoration statement semantics; never default it on.
use std::{future::Future, sync::Arc, io::Write};
use anyhow::{anyhow,bail,Result};
use parking_lot::Mutex;
use super::{Db,DbKind,ExportTaskOptions,transfer_output::FileEncoder};

pub(super) const MAX_BATCH_ROWS:u32=1000;
pub(super) const MAX_BATCH_BYTES:u64=4*1024*1024;
#[derive(Clone,Copy,Debug)]
pub(super) struct Policy {pub rows:u32,pub bytes:u64}
impl Policy {
    pub fn for_task(db:&Db,kind:&DbKind,opts:&ExportTaskOptions)->Result<Option<Self>> {
        let rows=opts.sql_batch_rows.max(1);
        if rows==1{return Ok(None);}
        if !opts.options.format.eq_ignore_ascii_case("sql") {bail!("多行 SQL 组包仅适用于 SQL 导出；CSV/JSON 保持原格式");}
        if !matches!(db,Db::Sqlite(_)|Db::Mysql(_)|Db::Postgres(_))||!matches!(kind.protocol(),crate::models::Protocol::Sqlite|crate::models::Protocol::Mysql|crate::models::Protocol::Postgres){bail!("多行 SQL 导出只支持已验证的原生 SQLite/MySQL/PostgreSQL 通道");}
        let bytes=if opts.sql_batch_bytes==0{1024*1024}else{opts.sql_batch_bytes};
        Self::checked(rows,bytes).map(Some)
    }
    pub fn checked(rows:u32,bytes:u64)->Result<Self>{
        if !(2..=MAX_BATCH_ROWS).contains(&rows)||!(1024..=MAX_BATCH_BYTES).contains(&bytes){bail!("SQL 组包须为 2–1000 行，语句预算为 1 KiB–4 MiB");}Ok(Self{rows,bytes})
    }
}
#[derive(Default,Clone,Debug)]
pub(super) struct Metrics {pub statements:u64,pub rows:u64,pub encoded_bytes:u64,pub utf16_bytes:u64,pub peak_utf8_bytes:u64,pub max_statement_encoded_bytes:u64}
#[derive(Clone)]
struct Context { policy:Option<Policy>,metrics:Arc<Mutex<Metrics>> }
tokio::task_local!{static SQL_EXPORT_CONTEXT:Context;}
pub(super) async fn scoped<F:Future>(policy:Option<Policy>,metrics:Arc<Mutex<Metrics>>,future:F)->F::Output{
    SQL_EXPORT_CONTEXT.scope(Context{policy,metrics},future).await
}
pub(super) fn current()->Option<(Policy,Arc<Mutex<Metrics>>)>{SQL_EXPORT_CONTEXT.try_with(|c|c.policy.map(|p|(p,c.metrics.clone()))).ok().flatten()}

/// Exact encoded size, without allocating an encoded copy or accepting replacement characters.
/// UTF-16 is accounted for even though the current file encoder does not expose UTF-16 output.
pub(super) fn encoded_len(text:&str,label:&str)->Result<u64>{
    match label.to_ascii_lowercase().as_str(){
        ""|"utf8"|"utf-8"=>return Ok(text.len() as u64),
        "utf-16"|"utf-16le"|"utf-16be"=>return Ok(text.encode_utf16().count() as u64*2),
        "latin1"|"iso-8859-1"=>{if text.chars().any(|c|c as u32>255){bail!("SQL 记录不能以 ISO-8859-1 无损保存");}return Ok(text.chars().count() as u64);},_=>{}
    }
    let encoding=match label.to_ascii_lowercase().as_str(){"gbk"|"gb2312"=>encoding_rs::GBK,"gb18030"=>encoding_rs::GB18030,"big5"=>encoding_rs::BIG5,_=>bail!("SQL 组包不支持所选编码")};
    let mut encoder=encoding.new_encoder();let mut scratch=[0u8;8192];let mut consumed=0;let mut bytes=0u64;
    loop{let(status,read,written)=encoder.encode_from_utf8_without_replacement(&text[consumed..],&mut scratch,true);consumed+=read;bytes=bytes.checked_add(written as u64).ok_or_else(||anyhow!("编码字节计数溢出"))?;
        match status{encoding_rs::EncoderResult::InputEmpty=>return Ok(bytes),encoding_rs::EncoderResult::Unmappable(_)=>bail!("SQL 记录不能以所选编码无损保存"),encoding_rs::EncoderResult::OutputFull if read==0&&written==0=>bail!("SQL 编码计量无法取得进展"),encoding_rs::EncoderResult::OutputFull=>{}}
    }
}

pub(super) struct Assembler {
    prefix:String,encoding:String,policy:Policy,buffer:String,pending_rows:u32,pending_encoded:u64,
    metrics:Arc<Mutex<Metrics>>,
}
impl Assembler {
    pub fn new(prefix:String,encoding:&str,policy:Policy,metrics:Arc<Mutex<Metrics>>)->Result<Self>{
        let head=encoded_len(&prefix,encoding)?;let tail=encoded_len(";\r\n",encoding)?;
        if prefix.len()+3>policy.bytes as usize||head+tail>=policy.bytes{bail!("SQL INSERT 表头超过语句预算");}
        let buffer=String::with_capacity(policy.bytes as usize);{let mut m=metrics.lock();m.peak_utf8_bytes=m.peak_utf8_bytes.max(buffer.capacity() as u64);}
        Ok(Self{prefix,encoding:encoding.into(),policy,buffer,pending_rows:0,pending_encoded:0,metrics})
    }
    pub fn row_limit(&self)->u64{self.policy.bytes}
    pub fn push(&mut self,out:&mut FileEncoder,tuple:&str)->Result<()> {
        let tuple_encoded=encoded_len(tuple,&self.encoding)?;let prefix_encoded=encoded_len(&self.prefix,&self.encoding)?;let tail_encoded=encoded_len(";\r\n",&self.encoding)?;let separator_encoded=encoded_len(", ",&self.encoding)?;
        let standalone=prefix_encoded.checked_add(tuple_encoded).and_then(|n|n.checked_add(tail_encoded)).ok_or_else(||anyhow!("SQL 记录长度溢出"))?;
        // An oversized row is a hard failure, never a silent single-row fallback. Validate it
        // before flushing an existing group; the unpublished stage is discarded on any error.
        if self.prefix.len()+tuple.len()+3>self.policy.bytes as usize||standalone>self.policy.bytes{bail!("单行 SQL 的转义后 UTF-8/目标编码字节超过组包语句上限；没有发布文件");}
        if self.pending_rows>0&&(self.pending_rows>=self.policy.rows||self.buffer.len()+2+tuple.len()+3>self.policy.bytes as usize||self.pending_encoded+separator_encoded+tuple_encoded+tail_encoded>self.policy.bytes){self.flush(out)?;}
        if self.pending_rows==0{self.buffer.push_str(&self.prefix);self.pending_encoded=prefix_encoded;}else{self.buffer.push_str(", ");self.pending_encoded+=separator_encoded;}
        self.buffer.push_str(tuple);self.pending_encoded+=tuple_encoded;self.pending_rows+=1;
        if self.pending_rows==self.policy.rows{self.flush(out)?;}
        Ok(())
    }
    pub fn flush(&mut self,out:&mut FileEncoder)->Result<()> {
        if self.pending_rows==0{return Ok(());}
        self.buffer.push_str(";\r\n");let expected=encoded_len(&self.buffer,&self.encoding)?;let before=out.bytes_written();out.write_all(self.buffer.as_bytes())?;
        let actual=out.bytes_written().checked_sub(before).ok_or_else(||anyhow!("SQL 编码计数回退"))?;
        if actual!=expected{bail!("SQL 目标编码计量与实际写入不一致；没有发布文件");}
        {let mut metrics=self.metrics.lock();metrics.statements+=1;metrics.rows+=self.pending_rows as u64;metrics.encoded_bytes+=actual;metrics.utf16_bytes+=self.buffer.encode_utf16().count() as u64*2;metrics.peak_utf8_bytes=metrics.peak_utf8_bytes.max(self.buffer.len() as u64);metrics.max_statement_encoded_bytes=metrics.max_statement_encoded_bytes.max(actual);}
        self.buffer.clear();self.pending_rows=0;self.pending_encoded=0;Ok(())
    }
}

#[cfg(test)]
#[path="transfer_sql_batch_tests.rs"]
mod tests;
