//! Versioned, deterministic native recipes. Total is never materialized; one row at a time.
use std::{collections::HashSet, sync::OnceLock};
use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use crate::{data_generator::GeneratedRowsRequest, models::{ColumnInfo, TableSchema}};

pub const MAX_TOTAL: u64 = 10_000_000;
pub const ROW_BYTES: usize = 64 * 1024;
const TASK_BYTES: usize = 4 * 1024 * 1024;
const GLOBAL_BYTES: usize = 8 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub version: u32, pub connection_id: String, pub schema: String, pub table: String,
    pub total: u64, pub seed: String, pub frozen_time: String, pub schema_fingerprint: String,
    pub transaction_mode: String, pub fields: Vec<Rule>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule { pub name: String, pub mode: Mode, pub value: String }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum Mode { Auto, Sequence, Constant, Null, Omit }
#[derive(Debug, Clone, Serialize)]
pub struct Preview { pub recipe: Recipe, pub columns: Vec<String>, pub rows: Vec<Vec<Option<String>>>, pub database_decided: Vec<String>, pub plan_hash: String }
#[derive(Default)]
struct Admission { bytes: usize, targets: HashSet<String> }
static ADMISSION: OnceLock<Mutex<Admission>> = OnceLock::new();
fn admission()-> &'static Mutex<Admission> { ADMISSION.get_or_init(||Mutex::new(Admission::default())) }
/// Owned by the payload/work, not the request or supervise future. Unknown does not release it.
pub struct Lease { target: String }
impl Lease {
    pub fn acquire(target: String)->Result<Self> {
        let mut budget=admission().lock();
        if budget.targets.contains(&target) { bail!("目标已有 recipe 写入/预览任务；等待实际排空后重试"); }
        if budget.bytes + TASK_BYTES > GLOBAL_BYTES { bail!("全局 recipe 任务/缓冲预算已占满；等待实际排空后重试"); }
        budget.bytes += TASK_BYTES; budget.targets.insert(target.clone()); Ok(Self { target })
    }
}
impl Drop for Lease { fn drop(&mut self) { let mut b=admission().lock(); b.targets.remove(&self.target); b.bytes=b.bytes.saturating_sub(TASK_BYTES); } }
impl Recipe {
    pub fn validate(&self, preview: bool)->Result<()> {
        if self.version!=1 || !matches!(self.transaction_mode.as_str(),"atomic"|"segmented") { bail!("只支持 version=1 的 atomic 或显式 segmented recipe"); }
        if self.total==0 || self.total>MAX_TOTAL { bail!("recipe 总量必须为 1–{MAX_TOTAL}"); }
        if self.table.is_empty() || self.table.len()>1024 || self.schema.len()>1024 || self.connection_id.len()>256 || self.table.contains('\0') || self.schema.contains('\0') { bail!("recipe 目标无效"); }
        if self.seed.len()>320 || self.seed.contains('\0') || self.fields.len()>128 || self.fields.is_empty() { bail!("recipe 种子/列规则超限"); }
        let time=DateTime::parse_from_rfc3339(&self.frozen_time).map_err(|_|anyhow!("冻结时间必须为 RFC3339"))?;
        if !(1970..=9990).contains(&chrono::Datelike::year(&time)) { bail!("冻结时间超出支持范围"); }
        if !preview && (self.schema_fingerprint.len()!=64 || !self.schema_fingerprint.bytes().all(|b|b.is_ascii_hexdigit())) { bail!("执行 recipe 必须来自原生结构指纹预览"); }
        let mut names=HashSet::new(); let mut bytes=0;
        for f in &self.fields { if f.name.is_empty() || f.name.len()>1024 || f.name.contains('\0') || !names.insert(&f.name) || f.value.len()>16384 { bail!("recipe 字段重复、无效或值超限"); } bytes+=f.name.len()+f.value.len(); }
        if bytes>ROW_BYTES { bail!("recipe 规则字节超出 64 KiB"); } Ok(())
    }
    pub fn shape(&self)->GeneratedRowsRequest {
        let columns:Vec<_>=self.fields.iter().filter(|f|f.mode!=Mode::Omit).map(|f|f.name.clone()).collect();
        GeneratedRowsRequest {connection_id:self.connection_id.clone(),schema:self.schema.clone(),table:self.table.clone(),rows:vec![columns.iter().map(|_|Some(String::new())).collect()],columns}
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind { Integer, Number, Boolean, Date, Datetime, Time, Uuid, Json, Text, Enum, Unsupported }
fn kind(c:&ColumnInfo)->Kind {
    let t=c.data_type.to_lowercase(); let base=t.split(|c:char| !c.is_ascii_alphanumeric()).next().unwrap_or("");
    match base {"bool"|"boolean"|"bit"=>Kind::Boolean,"uuid"|"uniqueidentifier"=>Kind::Uuid,"json"|"jsonb"=>Kind::Json,"datetime"|"timestamp"|"timestamptz"=>Kind::Datetime,"date"=>Kind::Date,"time"|"timetz"=>Kind::Time,"tinyint"|"smallint"|"mediumint"|"int"|"integer"|"bigint"|"int2"|"int4"|"int8"|"serial"|"bigserial"|"smallserial"=>Kind::Integer,"decimal"|"numeric"|"number"|"real"|"float"|"double"|"money"=>Kind::Number,"enum"=>Kind::Enum,""|"character"|"bpchar"|"char"|"varchar"|"nvarchar"|"nchar"|"text"|"tinytext"|"mediumtext"|"longtext"|"clob"|"string"|"citext"=>Kind::Text,_=>Kind::Unsupported}
}
fn dimensions(c:&ColumnInfo)->(Option<usize>,Option<usize>) {
    let inner=c.data_type.split_once('(').and_then(|(_,v)|v.split_once(')').map(|(s,_)|s));
    let mut parts=inner.unwrap_or("").split(',').map(|n|n.trim().parse::<usize>().ok());
    (parts.next().flatten(),parts.next().flatten())
}
fn enum_values(c:&ColumnInfo)->Vec<String> {
    // MySQL's SHOW column type escapes quotes with backslash or doubled quote.
    let mut values=vec![]; let mut chars=c.data_type.chars().peekable();
    while let Some(ch)=chars.next() { if ch!='\'' {continue;} let mut v=String::new(); while let Some(ch)=chars.next() { if ch=='\\' {if let Some(next)=chars.next(){v.push(next);}else{break;}} else if ch=='\'' {if chars.peek()==Some(&'\''){chars.next();v.push('\'');}else{break;}}else{v.push(ch);} } values.push(v); } values
}
fn integer_bounds(c:&ColumnInfo)->(i128,i128) {
    let t=c.data_type.to_lowercase(); let bits=if t.contains("tinyint"){8}else if t.contains("smallint")||t=="int2"||t=="smallserial"{16}else if t.contains("mediumint"){24}else if t.contains("bigint")||t=="int8"||t=="bigserial"{64}else{32};
    if t.contains("unsigned") {(0,(1i128<<bits)-1)}else{(-(1i128<<(bits-1)),(1i128<<(bits-1))-1)}
}
fn validate_value(c:&ColumnInfo,value:&Option<String>)->Result<()> {
    let Some(v)=value else {if !c.nullable||c.primary_key {bail!("{} 不允许 NULL",c.name);}return Ok(());};
    if v.len()>16384 || v.contains('\0') {bail!("{} 的单值超限或包含 NUL",c.name);}
    match kind(c) {
        Kind::Integer=>{let n=v.parse::<i128>().map_err(|_|anyhow!("{} 必须为整数",c.name))?;let(min,max)=integer_bounds(c);if n<min||n>max{bail!("{} 整数超出类型范围",c.name);}},
        Kind::Number=>{if !v.parse::<f64>().is_ok_and(|n|n.is_finite()){bail!("{} 必须为有限数值",c.name);}let(precision,scale)=dimensions(c);if c.data_type.to_lowercase().starts_with("decimal")||c.data_type.to_lowercase().starts_with("numeric") {if let Some(p)=precision{let raw=v.trim_start_matches(['+','-']);let(w,d)=raw.split_once('.').unwrap_or((raw,""));let s=scale.unwrap_or(0);if !w.bytes().all(|b|b.is_ascii_digit())||!d.bytes().all(|b|b.is_ascii_digit())||w.trim_start_matches('0').len().max(1)>p.saturating_sub(s)||d.len()>s{bail!("{} 超出 decimal 精度/小数位",c.name);}}}},
        Kind::Boolean=>if !matches!(v.to_ascii_lowercase().as_str(),"0"|"1"|"true"|"false"){bail!("{} 必须为布尔值",c.name);},
        Kind::Uuid=>{uuid::Uuid::parse_str(v).map_err(|_|anyhow!("{} 必须为 UUID",c.name))?;},
        Kind::Json=>{serde_json::from_str::<serde_json::Value>(v).map_err(|_|anyhow!("{} 必须为有效 JSON",c.name))?;},
        Kind::Date=>{chrono::NaiveDate::parse_from_str(v,"%Y-%m-%d").map_err(|_|anyhow!("{} 日期无效",c.name))?;},
        Kind::Datetime=>{if DateTime::parse_from_rfc3339(v).is_err() && chrono::NaiveDateTime::parse_from_str(v,"%Y-%m-%d %H:%M:%S%.f").is_err(){bail!("{} 日期时间无效",c.name);}},
        Kind::Time=>{chrono::NaiveTime::parse_from_str(v,"%H:%M:%S").map_err(|_|anyhow!("{} 时间无效",c.name))?;},
        Kind::Text|Kind::Enum=>{if let Some(max)=dimensions(c).0.filter(|_|kind(c)==Kind::Text){if v.chars().count()>max{bail!("{} 超出字段长度",c.name);}}if kind(c)==Kind::Enum&&!enum_values(c).contains(v){bail!("{} 不属于枚举值",c.name);}},
        Kind::Unsupported=>{},
    } Ok(())
}
pub struct Engine { recipe: Recipe, columns: Vec<(ColumnInfo, Rule)>, time: DateTime<Utc> }
impl Engine {
    pub fn new(recipe:&Recipe,info:&TableSchema)->Result<Self> {
        let mut columns=vec![];
        for f in &recipe.fields {if !info.columns.iter().any(|c|c.name==f.name) && f.mode!=Mode::Omit{bail!("{} 不是可写真实字段",f.name);}}
        for c in &info.columns {
            let f=recipe.fields.iter().find(|f|f.name==c.name).ok_or_else(||anyhow!("缺少 {} 的规则",c.name))?;
            if c.auto_increment&&f.mode!=Mode::Omit{bail!("{} 必须交给数据库生成",c.name);}
            if f.mode==Mode::Omit{if !c.auto_increment&&c.default.is_none()&&(!c.nullable||c.primary_key){bail!("{} 必填，不能省略",c.name);}continue;}
            if f.mode==Mode::Sequence&&kind(c)!=Kind::Integer{bail!("{} 顺序规则仅支持整数",c.name);}
            if f.mode==Mode::Auto&&kind(c)==Kind::Unsupported{bail!("{} 类型不支持自动生成，请用固定值",c.name);}
            columns.push((c.clone(),f.clone()));
        }
        // Requested order defines bind order, independent of metadata order.
        columns.sort_by_key(|(_,f)|recipe.fields.iter().position(|r|r.name==f.name).unwrap());
        let engine=Self{recipe:recipe.clone(),columns,time:DateTime::parse_from_rfc3339(&recipe.frozen_time)?.with_timezone(&Utc)};
        engine.row(0)?; engine.row(recipe.total-1)?; Ok(engine)
    }
    pub fn row(&self,index:u64)->Result<Vec<Option<String>>> {
        if index>=self.recipe.total{bail!("全局行号超出授权总量");}
        let mut row=Vec::with_capacity(self.columns.len());let mut bytes=0;
        for(c,f)in &self.columns {
            let value=match f.mode {Mode::Null=>None,Mode::Constant=>Some(f.value.clone()),Mode::Sequence=>Some(f.value.parse::<i128>().map_err(|_|anyhow!("{} 起始值必须为整数",c.name))?.checked_add(index as i128).ok_or_else(||anyhow!("序列溢出"))?.to_string()),Mode::Auto=>Some(self.auto(c,index)?),Mode::Omit=>unreachable!()};
            validate_value(c,&value)?; bytes+=value.as_ref().map_or(0,String::len)+32;if bytes>ROW_BYTES{bail!("生成单行超过 64 KiB 预算");}row.push(value);
        } Ok(row)
    }
    fn auto(&self,c:&ColumnInfo,i:u64)->Result<String> {
        let offset=Duration::seconds(i as i64);let instant=self.time.checked_add_signed(offset).ok_or_else(||anyhow!("时间溢出"))?;if matches!(kind(c),Kind::Datetime|Kind::Date)&&chrono::Datelike::year(&instant)>9999{bail!("{} 自动日期时间超出四位年份支持范围",c.name);}
        Ok(match kind(c) {
            Kind::Integer=> (i+1).to_string(),
            Kind::Number=>{let scale=dimensions(c).1.unwrap_or(0);if scale>0{format!("{}.{}",(i+1)/10,format!("{}{}",(i+1)%10,"0".repeat(scale.min(8)-1)))}else{(i+1).to_string()}},
            Kind::Boolean=>if i%2==0{"0".into()}else{"1".into()},
            Kind::Date=>{let date=self.time.checked_add_signed(Duration::days(i as i64)).ok_or_else(||anyhow!("日期溢出"))?;if chrono::Datelike::year(&date)>9999{bail!("{} 自动日期超出四位年份支持范围",c.name);}date.format("%Y-%m-%d").to_string()},
            Kind::Datetime=>instant.format("%Y-%m-%d %H:%M:%S").to_string(),Kind::Time=>instant.format("%H:%M:%S").to_string(),
            Kind::Uuid=>{let digest=Sha256::digest(serde_json::to_vec(&(1u32,&self.recipe.seed,&c.name,i,&self.recipe.frozen_time))?);let mut b=[0u8;16];b.copy_from_slice(&digest[..16]);b[6]=(b[6]&15)|64;b[8]=(b[8]&63)|128;uuid::Uuid::from_bytes(b).to_string()},
            Kind::Json=>format!("{{\"generated\":true,\"index\":{}}}",i+1),
            Kind::Enum=>{let values=enum_values(c);values.get(i as usize % values.len().max(1)).cloned().ok_or_else(||anyhow!("枚举值不可解析"))?},
            // Never truncate a would-be unique value. Narrow columns fail before sending.
            Kind::Text=>format!("{}_{}_{}",self.recipe.seed,i+1,c.name),Kind::Unsupported=>bail!("不支持自动类型"),
        })
    }
    pub fn preview(&self, fingerprint:String,_info:&TableSchema)->Result<Preview> {
        let mut recipe=self.recipe.clone();recipe.schema_fingerprint=fingerprint;
        let rows=(0..recipe.total.min(20)).map(|i|self.row(i)).collect::<Result<Vec<_>>>()?;
        let bytes:usize=rows.iter().flatten().map(|v|v.as_ref().map_or(32,|v|v.len()+32)).sum();if bytes>1024*1024{bail!("样本超过 1 MiB 预算");}
        let plan_hash=format!("{:x}",Sha256::digest(serde_json::to_vec(&recipe)?));
        Ok(Preview{recipe,columns:self.columns.iter().map(|(c,_)|c.name.clone()).collect(),rows,database_decided:self.recipe.fields.iter().filter(|f|f.mode==Mode::Omit).map(|f|f.name.clone()).collect(),plan_hash})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recipe()->Recipe{Recipe{version:1,connection_id:"test".into(),schema:"main".into(),table:"t".into(),total:1000000,seed:"fixed".into(),frozen_time:"2026-11-13T08:00:00Z".into(),schema_fingerprint:"a".repeat(64),transaction_mode:"atomic".into(),fields:vec![Rule{name:"id".into(),mode:Mode::Sequence,value:"10".into()},Rule{name:"u".into(),mode:Mode::Auto,value:"".into()}]}}
    fn info()->TableSchema{TableSchema{table:"t".into(),database:"main".into(),columns:vec![col("id","BIGINT"),col("u","uuid")],indexes:vec![],foreign_keys:vec![],comment:None,engine:None,collation:None,row_count:None}}
    fn col(name:&str,ty:&str)->ColumnInfo{ColumnInfo{name:name.into(),data_type:ty.into(),nullable:false,default:None,primary_key:false,auto_increment:false,comment:None,length:None,precision:None,scale:None}}
    #[test]fn recipe_deterministic_global_index_across_blocks(){let r=recipe();let e=Engine::new(&r,&info()).unwrap();let whole=(0..1000).map(|i|e.row(i).unwrap()).collect::<Vec<_>>();for chunk in [1,17,256,500]{let split=(0..1000).step_by(chunk).flat_map(|start|(start..(start+chunk).min(1000)).map(|i|e.row(i as u64).unwrap())).collect::<Vec<_>>();assert_eq!(whole,split);}assert_eq!(e.row(999999).unwrap()[0],Some("1000009".into()));let p=e.preview("a".repeat(64),&info()).unwrap();assert_eq!(p.rows.len(),20);for(i,row)in p.rows.iter().enumerate(){assert_eq!(row,&e.row(i as u64).unwrap());}}
    #[test]fn recipe_capacity_and_atomic_contract(){let mut r=recipe();r.total=MAX_TOTAL+1;assert!(r.validate(false).is_err());r.total=1000;r.transaction_mode="per_chunk".into();assert!(r.validate(false).is_err());r.transaction_mode="segmented".into();assert!(r.validate(false).is_ok());r.transaction_mode="atomic".into();r.fields[0].value=i64::MAX.to_string();assert!(Engine::new(&r,&info()).is_err());}
    #[test]fn recipe_admission_held_until_drop(){let target=uuid::Uuid::new_v4().to_string();let lease=Lease::acquire(target.clone()).unwrap();assert!(Lease::acquire(target.clone()).is_err());drop(lease);assert!(Lease::acquire(target).is_ok());}
}
