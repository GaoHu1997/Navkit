//! D2 专用有界逐语句词法器。旧 transfer_sql_reader 的完整预扫描导入保持不变。
use std::{collections::VecDeque, fs::{File, OpenOptions}, io::{self, BufReader, Read}, path::Path, sync::Arc, time::SystemTime};
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use crate::{error::AppError, execution::Entry, models::{DbKind, Protocol}};

const BUFFER: usize = 64 * 1024;
const LOOK: usize = 128;
pub(crate) const MAX_STATEMENT_BYTES: usize = 8 * 1024 * 1024;
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect { Sqlite, Mysql, Postgres }
impl Dialect {
    pub(crate) fn from_kind(kind: &DbKind) -> Result<Self> {
        match kind.protocol() { Protocol::Sqlite => Ok(Self::Sqlite), Protocol::Mysql => Ok(Self::Mysql), Protocol::Postgres => Ok(Self::Postgres), _ => bail!("当前通道没有已验证的流式 SQL 词法及取消通道") }
    }
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceIdentity { pub canonical_path: String, pub device: u64, pub file: u64, pub bytes: u64, pub modified_ns: Option<u128> }
pub(crate) struct SourceFile { file: File, identity: SourceIdentity, modified: Option<SystemTime> }
impl SourceFile {
    /// 打开一次；不读取正文。Windows 禁止其他写入/替换共享，Unix 固定 inode 并检查变更。
    pub(crate) fn open(path: &str) -> Result<Self> {
        let canonical = std::fs::canonicalize(Path::new(path)).context("SQL 源文件不存在或无法固定路径")?;
        let mut options = OpenOptions::new(); options.read(true);
        #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
        let file = options.open(&canonical).context("无法固定 SQL 源文件的只读句柄")?;
        let metadata = file.metadata()?;
        if !metadata.is_file() { bail!("SQL 源必须是普通文件，不能是目录/管道/设备"); }
        let (device, inode) = file_identity(&file)?;
        let modified = metadata.modified().ok();
        let identity = SourceIdentity { canonical_path: canonical.to_string_lossy().into_owned(), device, file: inode, bytes: metadata.len(), modified_ns: modified.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_nanos()) };
        Ok(Self { file, identity, modified })
    }
    pub(crate) fn identity(&self) -> &SourceIdentity { &self.identity }
}
impl Read for SourceFile {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let metadata = self.file.metadata()?;
        if metadata.len() != self.identity.bytes || metadata.modified().ok() != self.modified {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "SQL 固定源句柄内容发生变化；停止扫描，既有执行不可重放"));
        }
        self.file.read(output)
    }
}
#[cfg(unix)]
fn file_identity(file: &File) -> Result<(u64, u64)> { use std::os::unix::fs::MetadataExt; let m=file.metadata()?; Ok((m.dev(),m.ino())) }
#[cfg(windows)]
fn file_identity(file: &File) -> Result<(u64, u64)> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)] struct Information { attributes:u32, creation:[u32;2], access:[u32;2], write:[u32;2], volume:u32, size_high:u32, size_low:u32, links:u32, index_high:u32, index_low:u32 }
    #[link(name="kernel32")] extern "system" { fn GetFileInformationByHandle(handle:*mut std::ffi::c_void, info:*mut Information)->i32; }
    let mut info=std::mem::MaybeUninit::<Information>::uninit();
    // SAFETY: 持有有效 File HANDLE，系统只写入 repr(C) Information。
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(),info.as_mut_ptr()) }==0 { return Err(io::Error::last_os_error().into()); }
    let info=unsafe { info.assume_init() }; Ok((info.volume as u64,((info.index_high as u64)<<32)|info.index_low as u64))
}
#[cfg(not(any(unix,windows)))]
fn file_identity(_file:&File)->Result<(u64,u64)> { bail!("当前平台不能验证 SQL 文件物理身份") }

#[derive(Debug)]
pub(crate) struct Statement { pub sql: String, pub keywords: Vec<String> }
enum Mode { Normal, Quote { close:u8, escaped:bool }, Line, Block(usize), Dollar(Vec<u8>) }
pub(crate) struct Reader<R:Read> { reader:BufReader<R>, look:VecDeque<u8>, dialect:Dialect, delimiter:Vec<u8>, line_start:bool, stop:Option<Arc<Entry>> }
impl<R:Read> Reader<R> {
    pub(crate) fn new(reader:R,dialect:Dialect,stop:Option<Arc<Entry>>)->Self { Self { reader:BufReader::with_capacity(BUFFER,reader),look:VecDeque::with_capacity(LOOK),dialect,delimiter:vec![b';'],line_start:true,stop } }
    fn peek(&mut self,n:usize)->Result<Option<u8>> {
        if self.stop.as_ref().is_some_and(|e|e.stopping()) { return Err(AppError::coded("BATCH_SCAN_STOPPED","扫描已停止，没有提交下一条 SQL").into()); }
        if n>=LOOK { bail!("SQL 词法标记超过上限"); }
        // Refill bounded lookahead in chunks instead of invoking Read once per SQL byte.
        // peek still checks stop before every access; decoding and source identity checks stay unchanged.
        while self.look.len()<=n { let mut bytes=[0;LOOK]; let available=LOOK-self.look.len(); let read=loop { match self.reader.read(&mut bytes[..available]) { Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue, other=>break other? } }; if read==0 { return Ok(None); } self.look.extend(&bytes[..read]); }
        Ok(self.look.get(n).copied())
    }
    fn take(&mut self)->Result<Option<u8>> { if self.peek(0)?.is_none() { return Ok(None); } let b=self.look.pop_front().unwrap(); if b==b'\n'||b==b'\r' { self.line_start=true; } else if !b.is_ascii_whitespace() { self.line_start=false; } Ok(Some(b)) }
    fn starts(&mut self,bytes:&[u8],fold:bool)->Result<bool> { for (i,b) in bytes.iter().enumerate() { let Some(c)=self.peek(i)? else { return Ok(false); }; if if fold { !c.eq_ignore_ascii_case(b) } else { c!=*b } { return Ok(false); } } Ok(true) }
    fn starts_delimiter(&mut self)->Result<bool> {
        // Copy one byte, not the delimiter Vec: this is the hot path for every unquoted byte.
        for i in 0..self.delimiter.len() { let expected=self.delimiter[i]; if self.peek(i)?!=Some(expected) { return Ok(false); } } Ok(true)
    }
    fn copy(&mut self,out:&mut Vec<u8>,count:usize)->Result<()> { for _ in 0..count { if out.len()>=MAX_STATEMENT_BYTES { bail!("SQL 单条语句超过 8 MiB 上限；仅停止未扫描尾部，不能推断既有写入已回滚"); } out.push(self.take()?.ok_or_else(||anyhow!("SQL 词法标记意外结束"))?); } Ok(()) }
    fn dollar(&mut self)->Result<Option<Vec<u8>>> {
        if self.peek(0)?!=Some(b'$') { return Ok(None); } if self.peek(1)?==Some(b'$') { return Ok(Some(b"$$".to_vec())); }
        if !self.peek(1)?.is_some_and(|b|b.is_ascii_alphabetic()||b==b'_') { return Ok(None); }
        let mut tag=vec![b'$']; for i in 1..=64 { let Some(b)=self.peek(i)? else { return Ok(None); }; if b==b'$' { tag.push(b); return Ok(Some(tag)); } if !b.is_ascii_alphanumeric()&&b!=b'_' { return Ok(None); } tag.push(b); } bail!("PostgreSQL dollar 标签超过 63 字节")
    }
    fn directive(&mut self)->Result<bool> {
        if self.dialect!=Dialect::Mysql||!self.line_start||!self.starts(b"DELIMITER",true)?||!self.peek(9)?.is_some_and(|b|b.is_ascii_whitespace()) { return Ok(false); }
        for _ in 0..9 { self.take()?; } let mut line=Vec::new(); while let Some(b)=self.peek(0)? { if b==b'\r'||b==b'\n' { self.take()?; break; } if line.len()>64 { bail!("MySQL DELIMITER 指令过长"); } line.push(self.take()?.unwrap()); }
        let delimiter=String::from_utf8(line)?.trim().as_bytes().to_vec();
        if delimiter.is_empty()||delimiter.len()>16||delimiter.iter().any(|b|b.is_ascii_whitespace()||b.is_ascii_alphanumeric()||matches!(*b,b'\''|b'"'|b'`'|b'\\'|b'#'))||delimiter.starts_with(b"--")||delimiter.starts_with(b"/*")||delimiter.starts_with(b"*/") { bail!("MySQL DELIMITER 必须是 1—16 字节非注释/引号/字母数字的分隔符"); }
        self.delimiter=delimiter; Ok(true)
    }
    pub(crate) fn next_statement(&mut self)->Result<Option<Statement>> {
        let mut out=Vec::with_capacity(BUFFER); let mut mode=Mode::Normal; let mut word=Vec::with_capacity(64); let mut long=false; let mut words=Vec::new(); let mut has_sql=false;
        let mut trigger=false; let mut body_seen=false; let mut body=0usize; let mut cases=0usize;
        loop {
            let Some(b)=self.peek(0)? else {
                observe(&mut word,&mut long,&mut words,self.dialect,&mut trigger,&mut body_seen,&mut body,&mut cases,&self.delimiter)?;
                if !matches!(mode,Mode::Normal|Mode::Line) { bail!("SQL 引号、dollar 引用或块注释未结束；未扫描尾部没有执行"); }
                if body!=0||cases!=0 { bail!("SQLite 触发器 BEGIN/CASE 没有对应 END"); }
                if !has_sql { return Ok(None); } if self.delimiter!=b";" { bail!("SQL 末尾缺少当前 DELIMITER"); } return make(out,words,self.dialect);
            };
            match &mut mode {
                Mode::Line=>{ self.copy(&mut out,1)?; if b==b'\n'||b==b'\r' { mode=Mode::Normal; } continue; }
                Mode::Block(depth)=>{ if self.starts(b"/*",false)? { if self.dialect!=Dialect::Postgres { bail!("当前方言不支持嵌套注释"); } *depth+=1; if *depth>128 { bail!("SQL 嵌套注释超过上限"); } self.copy(&mut out,2)?; } else if self.starts(b"*/",false)? { *depth-=1; self.copy(&mut out,2)?; if *depth==0 { mode=Mode::Normal; } } else { self.copy(&mut out,1)?; } continue; }
                Mode::Dollar(tag)=>{ if self.starts(tag,false)? { let n=tag.len(); self.copy(&mut out,n)?; mode=Mode::Normal; } else { self.copy(&mut out,1)?; } continue; }
                Mode::Quote{close,escaped}=>{ if b==*close { self.copy(&mut out,1)?; if self.peek(0)?==Some(*close) { self.copy(&mut out,1)?; } else { mode=Mode::Normal; } } else if b==b'\\' { if *escaped { self.copy(&mut out,2)?; } else if self.dialect==Dialect::Mysql||self.dialect==Dialect::Postgres&&*close==b'\'' { bail!("字符串含依赖会话模式的反斜杠；PostgreSQL 请使用 E 字符串"); } else { self.copy(&mut out,1)?; } } else { self.copy(&mut out,1)?; } continue; }
                Mode::Normal=>{}
            }
            let word_byte=b.is_ascii_alphanumeric()||b==b'_'||b>=0x80||(b==b'$'&&!word.is_empty());
            if !word_byte { observe(&mut word,&mut long,&mut words,self.dialect,&mut trigger,&mut body_seen,&mut body,&mut cases,&self.delimiter)?; }
            if !has_sql&&self.directive()? { out.clear(); continue; }
            if self.starts_delimiter()?&&body==0 {
                let n=self.delimiter.len(); for _ in 0..n { self.take()?; } if !has_sql { out.clear(); words.clear(); continue; }
                if self.delimiter==b";" { if out.len()>=MAX_STATEMENT_BYTES { bail!("SQL 单条语句超过 8 MiB 上限"); } out.push(b';'); } return make(out,words,self.dialect);
            }
            let mysql_line=self.dialect!=Dialect::Mysql||!self.peek(2)?.is_some_and(|c|!c.is_ascii_whitespace()&&c>0x20);
            if self.starts(b"--",false)?&&mysql_line { self.copy(&mut out,2)?; mode=Mode::Line; continue; }
            if b==b'#'&&self.dialect==Dialect::Mysql { self.copy(&mut out,1)?; mode=Mode::Line; continue; }
            if self.starts(b"/*",false)? { if self.peek(2)?==Some(b'!')||self.peek(2)?.is_some_and(|c|c.eq_ignore_ascii_case(&b'M'))&&self.peek(3)?==Some(b'!') { bail!("不允许 MySQL/MariaDB 可执行注释；既有写入不可推断回滚"); } self.copy(&mut out,2)?; mode=Mode::Block(1); continue; }
            if matches!(b,b'\''|b'"'|b'`')||b==b'['&&self.dialect==Dialect::Sqlite { has_sql=true; let escaped=self.dialect==Dialect::Postgres&&b==b'\''&&out.last().is_some_and(|c|c.eq_ignore_ascii_case(&b'E')); self.copy(&mut out,1)?; mode=Mode::Quote{close:if b==b'[' {b']'} else {b},escaped}; continue; }
            if b==b'$'&&word.is_empty()&&self.dialect==Dialect::Postgres { if let Some(tag)=self.dollar()? { has_sql=true; self.copy(&mut out,tag.len())?; mode=Mode::Dollar(tag); continue; } }
            if b==0 { bail!("SQL 脚本包含 NUL"); } if !b.is_ascii_whitespace() { has_sql=true; } if word_byte { if word.len()<64 { word.push(b); } else { long=true; } } self.copy(&mut out,1)?;
        }
    }
}
fn observe(word:&mut Vec<u8>,long:&mut bool,words:&mut Vec<String>,dialect:Dialect,trigger:&mut bool,seen:&mut bool,body:&mut usize,cases:&mut usize,delimiter:&[u8])->Result<()> {
    if word.is_empty() { return Ok(()); } let text=if *long { String::new() } else { String::from_utf8_lossy(word).to_ascii_uppercase() }; word.clear(); *long=false;
    if words.len()<32 { words.push(text.clone()); }
    if words.first().map(String::as_str)==Some("CREATE")&&words.len()<=8&&text=="TRIGGER" { *trigger=true; }
    // SQL 标准函数体 BEGIN ATOMIC 的内部分号不是语句边界。尚未实现其完整语法，
    // 必须在发送任何函数体片段前拒绝；引用/dollar 内的 BEGIN 不进入 observe。
    if dialect==Dialect::Postgres&&text=="BEGIN"&&words.first().map(String::as_str)==Some("CREATE")&&words.iter().any(|w|matches!(w.as_str(),"FUNCTION"|"PROCEDURE")) { bail!("PostgreSQL 未引用的 BEGIN 函数/过程体暂不支持；请使用 dollar 引用的完整函数体"); }
    if dialect==Dialect::Mysql&&delimiter==b";"&&words.first().map(String::as_str)==Some("CREATE")&&text=="BEGIN"&&words.iter().any(|w|matches!(w.as_str(),"TRIGGER"|"PROCEDURE"|"FUNCTION"|"EVENT")) { bail!("MySQL 复合例程/触发器必须显式使用 DELIMITER"); }
    if dialect==Dialect::Sqlite&&*trigger { if text=="BEGIN"&&!*seen { *seen=true; *body=1; } else if *body>0&&text=="CASE" { *cases+=1; if *cases>128 { bail!("SQLite CASE 嵌套超过上限"); } } else if *body>0&&text=="END" { if *cases>0 { *cases-=1; } else { *body-=1; } } } Ok(())
}
fn make(bytes:Vec<u8>,keywords:Vec<String>,dialect:Dialect)->Result<Option<Statement>> {
    let sql=String::from_utf8(bytes).context("SQL 输入不是有效 UTF-8")?;
    // 仅检查词法器划定的顶层前缀，不再次拆 trigger/function body 的内部分号。
    let first=keywords.first().map(String::as_str).unwrap_or("");
    // Only SET/PRAGMA can match the exact dump-guard whitelist. Do not lex entire
    // INSERT payloads again just to prove they are not session-control statements.
    if !(matches!(first,"SET"|"PRAGMA")&&dump_guard(&sql,dialect)) { crate::execution_policy::ordinary_native_batch(&keywords.join(" "))?; }
    if matches!(first,"VACUUM"|"ABORT"|"COPY"|"PREPARE"|"EXECUTE"|"CALL"|"DO"|"DELIMITER"|"GRANT"|"REVOKE"|"LOAD"|"INSTALL"|"UNINSTALL"|"FLUSH") { bail!("SQL 含需要专门通道的过程/文件/控制命令"); }
    if matches!(first,"CREATE"|"DROP"|"ALTER")&&keywords.iter().take(4).any(|w|matches!(w.as_str(),"DATABASE"|"ROLE"|"USER"|"TABLESPACE"|"SERVER"|"EXTENSION")) { bail!("SQL 不能改变数据库/身份/服务器授权范围"); }
    if keywords.windows(2).any(|p|p[0]=="INTO"&&matches!(p[1].as_str(),"OUTFILE"|"DUMPFILE")) { bail!("SQL 不能请求服务器文件输出"); }
    Ok(Some(Statement{sql,keywords}))
}

pub(crate) fn bounded_template(text:&str)->String {
    let mut end=text.len().min(2048);while !text.is_char_boundary(end){end-=1;}text[..end].to_owned()
}
// 日志与 guard 共用词法边界，但 guard 使用未脱敏 token 做完整精确比较。
fn tokens(sql:&str,dialect:Dialect)->Vec<(String,bool)> {
    let c:Vec<char>=sql.chars().collect();let mut i=0;let mut out=Vec::new();let mut token_bytes=0usize;
    macro_rules! emit { ($value:expr)=>{{let token:(String,bool)=$value;token_bytes+=token.0.len()+usize::from(!out.is_empty());out.push(token);}}; }
    while i<c.len() {
        // The caller truncates at 2048 bytes after a complete lexical token. Stop
        // producing unused tail tokens at exactly the same point, never mid-literal.
        if token_bytes>=2048 { break; }
        if c[i].is_whitespace(){i+=1;continue;}
        if c[i]=='-'&&c.get(i+1)==Some(&'-') || c[i]=='#'&&dialect==Dialect::Mysql {
            while i<c.len()&&c[i]!='\n'{i+=1;}emit!(("/*?*/".into(),true));continue;
        }
        if c[i]=='/'&&c.get(i+1)==Some(&'*') {
            i+=2;let mut depth=1;while i<c.len()&&depth>0 {if c[i]=='/'&&c.get(i+1)==Some(&'*'){depth+=1;i+=2;}else if c[i]=='*'&&c.get(i+1)==Some(&'/'){depth-=1;i+=2;}else{i+=1;}}emit!(("/*?*/".into(),true));continue;
        }
        if c[i]=='$'&&dialect==Dialect::Postgres {
            let mut j=i+1;while j<c.len()&&(c[j].is_ascii_alphanumeric()||c[j]=='_'){j+=1;}
            if c.get(j)==Some(&'$') {let tag=&c[i..=j];i=j+1;while i<c.len()&&!c[i..].starts_with(tag){i+=1;}i=(i+tag.len()).min(c.len());emit!(("?".into(),false));continue;}
        }
        let prefix=(c[i]=='E'||c[i]=='e'||c[i]=='X'||c[i]=='x'||c[i]=='B'||c[i]=='b'||c[i]=='N'||c[i]=='n')&&c.get(i+1)==Some(&'\'');
        if prefix||matches!(c[i],'\''|'"'|'`')||c[i]=='['&&dialect==Dialect::Sqlite {
            let start=i;if prefix{i+=1;}let open=c[i];let close=if open=='['{']'}else{open};i+=1;
            while i<c.len(){if c[i]==close {i+=1;if c.get(i)==Some(&close){i+=1;}else{break;}}else if c[i]=='\\'&&(prefix||dialect==Dialect::Mysql){i=(i+2).min(c.len());}else{i+=1;}}
            // SQLite DQS 兼容模式下双引号也能表示值；只有 PostgreSQL 双引号可确定为标识符。
            let identifier=!prefix&&(open=='`'||open=='['||open=='"'&&dialect==Dialect::Postgres);
            emit!((if identifier{c[start..i].iter().collect()}else{"?".into()},false));continue;
        }
        if c[i].is_ascii_digit()||c[i]=='.'&&c.get(i+1).is_some_and(char::is_ascii_digit) {
            i+=1;while i<c.len()&&(c[i].is_ascii_alphanumeric()||matches!(c[i],'.'|'_' )||matches!(c[i],'+'|'-')&&matches!(c.get(i-1),Some('e'|'E'))){i+=1;}emit!(("?".into(),false));continue;
        }
        let start=i;i+=1;
        if c[start].is_alphabetic()||c[start]=='_' {while i<c.len()&&(c[i].is_alphanumeric()||matches!(c[i],'_'|'$')){i+=1;}}
        emit!((c[start..i].iter().collect(),false));
    }out
}
pub(crate) fn redacted_template(sql:&str,dialect:Dialect)->String {
    let mut result=String::new();for (token,_) in tokens(sql,dialect){if !result.is_empty(){result.push(' ');}result.push_str(&token);if result.len()>=2048{break;}}bounded_template(&result)
}
fn dump_guard(sql:&str,dialect:Dialect)->bool {
    // 数字不能从脱敏 token 恢复：先在原文中做普通注释剥离，再精确比较。
    let chars:Vec<char>=sql.chars().collect();let mut plain=String::new();let mut i=0;
    while i<chars.len(){
        if chars[i]=='/'&&chars.get(i+1)==Some(&'*'){i+=2;let mut depth=1;while i<chars.len()&&depth>0{if chars[i]=='/'&&chars.get(i+1)==Some(&'*'){depth+=1;i+=2;}else if chars[i]=='*'&&chars.get(i+1)==Some(&'/'){depth-=1;i+=2;}else{i+=1;}}plain.push(' ');}
        else if chars[i]=='-'&&chars.get(i+1)==Some(&'-')&&(dialect!=Dialect::Mysql||chars.get(i+2).is_none_or(|c|c.is_whitespace())) {while i<chars.len()&&chars[i]!='\n'{i+=1;}plain.push(' ');}
        else {plain.push(chars[i]);i+=1;}
    }
    let mut words=Vec::new();let mut word=String::new();
    for c in plain.chars(){if c.is_ascii_alphanumeric()||c=='_'{word.push(c.to_ascii_uppercase());}else{if !word.is_empty(){words.push(std::mem::take(&mut word));}if !c.is_whitespace(){words.push(c.to_string());}}}if !word.is_empty(){words.push(word);}if words.last().is_some_and(|w|w==";"){words.pop();}
    let exact=words.join(" ");match dialect {
        Dialect::Mysql=>matches!(exact.as_str(),"SET NAMES UTF8MB4"|"SET FOREIGN_KEY_CHECKS = 0"|"SET FOREIGN_KEY_CHECKS = 1"),
        Dialect::Postgres=>matches!(exact.as_str(),"SET SESSION_REPLICATION_ROLE = REPLICA"|"SET SESSION_REPLICATION_ROLE = DEFAULT"),
        Dialect::Sqlite=>matches!(exact.as_str(),"PRAGMA FOREIGN_KEYS = OFF"|"PRAGMA FOREIGN_KEYS = ON"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn batch_dump_guard_exact_dialect_whitelist_and_injection_rejection(){
        for (d,sql) in [(Dialect::Mysql,"SET NAMES utf8mb4; SET FOREIGN_KEY_CHECKS = 0; SET FOREIGN_KEY_CHECKS=1;"),(Dialect::Postgres,"SET session_replication_role=replica; SET session_replication_role=DEFAULT;"),(Dialect::Sqlite,"PRAGMA foreign_keys=OFF; PRAGMA foreign_keys=ON;")] {assert!(collect(sql,d).is_ok());}
        for sql in ["SET GLOBAL FOREIGN_KEY_CHECKS=0;","SET FOREIGN_KEY_CHECKS=0+0;","SET FOREIGN_KEY_CHECKS='0';","SET NAMES utf8mb4 COLLATE utf8mb4_bin;","SET autocommit=0;","SET sql_mode='';","SET search_path=public;","USE x;","BEGIN;","COMMIT;","ROLLBACK;","ATTACH 'x' AS x;","PRAGMA foreign_keys=OFF+0;","PRAGMA foreign_keys(OFF);","SET session_replication_role=replica,search_path=evil;","/*! SET FOREIGN_KEY_CHECKS=0 */;"] {for d in [Dialect::Mysql,Dialect::Postgres,Dialect::Sqlite]{assert!(collect(sql,d).is_err(),"{sql}");}}
        assert!(collect("SET FOREIGN_KEY_CHECKS=0--1;",Dialect::Mysql).is_err());
        assert!(collect("SET /* safe */ NAMES utf8mb4;",Dialect::Mysql).is_ok());
        assert!(collect("SET NAMES utf8mb4; USE other;",Dialect::Mysql).is_err());
    }
    #[test] fn batch_templates_keep_structure_and_identifiers_without_literals(){
        let sql="INSERT INTO \"表名\" (\"字段\", amount) VALUES ('机密''值', 98765.432, X'abcd', E'escape\\secret', $body$hidden; SELECT 777$body$); /* confidential /* nested */ text */ -- private\nSELECT 0xdead, .123e-9;";
        let t=redacted_template(sql,Dialect::Postgres);
        for secret in ["机密","98765","abcd","secret","hidden","777","confidential","private","dead","123"]{assert!(!t.contains(secret),"{t}");}
        for structure in ["INSERT","表名","字段","VALUES","SELECT","?"]{assert!(t.contains(structure),"{t}");}
        assert!(!redacted_template("SELECT \"mysql-secret\", 'secret'",Dialect::Mysql).contains("secret"));
        let sqlite=redacted_template("SELECT [列], `字段`, \"sqlite-secret\", \"机密\"\"值\" FROM 表",Dialect::Sqlite);
        for identifier in ["列","字段","表"]{assert!(sqlite.contains(identifier),"{sqlite}");}
        for secret in ["sqlite-secret","机密","值"]{assert!(!sqlite.contains(secret),"{sqlite}");}
        assert!(redacted_template(&"界".repeat(5000),Dialect::Sqlite).len()<=2048);
    }
    fn collect(sql:&str,dialect:Dialect)->Result<Vec<String>> { let mut reader=Reader::new(sql.as_bytes(),dialect,None); let mut out=Vec::new(); while let Some(s)=reader.next_statement()? { out.push(s.sql); } Ok(out) }
    #[test] fn bounded_audit_does_not_tokenize_unused_tail_or_leak_literals() {
        let prefix=(0..1000).map(|i|format!("identifier_{i},")).collect::<String>();
        let sql=format!("SELECT {prefix} 'tail-secret' /* private */ {}", "12345,".repeat(100000));
        let t=redacted_template(&sql,Dialect::Sqlite);assert_eq!(t.len(),2048);assert!(!t.contains("tail-secret"));assert!(!t.contains("private"));assert!(tokens(&sql,Dialect::Sqlite).len()<400);
        let long_literal=format!("SELECT '{}secret'; SELECT 12345;", "界".repeat(100000));
        assert_eq!(redacted_template(&long_literal,Dialect::Sqlite),"SELECT ? ; SELECT ? ;");
    }
    #[test] fn quotes_comments_dollar_and_delimiter_are_not_split() {
        assert_eq!(collect("INSERT INTO t VALUES ('a;''b'); -- ;\nSELECT \"x;y\"; /* ; */",Dialect::Sqlite).unwrap().len(),2);
        assert_eq!(collect("CREATE FUNCTION f() RETURNS text AS $tag$ SELECT 'a;b'; $tag$ LANGUAGE sql; SELECT E'a\\\';b';",Dialect::Postgres).unwrap().len(),2);
        assert_eq!(collect("DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; SELECT 'a;b'; END$$\nDELIMITER ;\nSELECT 2;",Dialect::Mysql).unwrap().len(),2);
    }
    #[test] fn sqlite_trigger_case_is_one_statement() { assert_eq!(collect("CREATE TRIGGER tr AFTER INSERT ON t BEGIN INSERT INTO x VALUES(CASE WHEN 1 THEN 'a;b' ELSE 'c' END); UPDATE x SET a=2; END; SELECT 1;",Dialect::Sqlite).unwrap().len(),2); }
    #[test] fn postgres_unquoted_atomic_body_is_rejected_before_any_fragment() {
        for signature in [String::new(),(0..40).map(|i|format!("p{i} integer")).collect::<Vec<_>>().join(",")] {
            let sql=format!("SELECT 1; CREATE OR REPLACE FUNCTION f({signature}) RETURNS integer LANGUAGE sql BEGIN /* separator */ ATOMIC SELECT 2; SELECT 3; END; SELECT 4;");
            let mut reader=Reader::new(sql.as_bytes(),Dialect::Postgres,None);
            assert_eq!(reader.next_statement().unwrap().unwrap().sql,"SELECT 1;");
            assert!(reader.next_statement().unwrap_err().to_string().contains("BEGIN"));
        }
    }
    #[test] fn unsafe_or_truncated_tail_reports_error_only_after_prefix() { let mut r=Reader::new("INSERT INTO t VALUES(1); SELECT 'broken".as_bytes(),Dialect::Sqlite,None); assert!(r.next_statement().unwrap().is_some()); assert!(r.next_statement().is_err()); for sql in ["BEGIN;","ATTACH 'x' AS x;","/*! SELECT 1 */;","LOAD DATA INFILE 'x' INTO TABLE t;"] { assert!(collect(sql,Dialect::Mysql).is_err()); } }
    #[test] fn chunked_lookahead_preserves_boundaries_short_reads_and_interruptions() {
        struct Short { data:std::io::Cursor<Vec<u8>>, calls:usize }
        impl Read for Short { fn read(&mut self,b:&mut[u8])->io::Result<usize> { self.calls+=1;if self.calls%7==0{return Err(io::ErrorKind::Interrupted.into());}let n=b.len().min(3);self.data.read(&mut b[..n]) } }
        for (dialect,sql) in [(Dialect::Sqlite,format!("{}INSERT INTO t VALUES('界;''x'); /* boundary */ SELECT 2;"," ".repeat(127))), (Dialect::Mysql,format!("{}DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT '界;'; END$$\nDELIMITER ;\nSELECT 2;"," ".repeat(126))), (Dialect::Postgres,format!("{}SELECT $tag$界; value$tag$; SELECT E'x\\\';y';"," ".repeat(127)))] {
            let expected=collect(&sql,dialect).unwrap();let mut r=Reader::new(Short{data:std::io::Cursor::new(sql.into_bytes()),calls:0},dialect,None);let mut actual=vec![];while let Some(s)=r.next_statement().unwrap(){actual.push(s.sql);}assert_eq!(actual,expected);assert_eq!(actual.len(),2);
        }
    }
    #[test] fn dump_guard_fast_path_does_not_accept_control_looking_payloads() {
        for d in [Dialect::Sqlite,Dialect::Mysql,Dialect::Postgres] {
            assert!(collect("INSERT INTO t VALUES('SET FOREIGN_KEY_CHECKS=0; PRAGMA foreign_keys=OFF');",d).is_ok());
            for sql in ["/* INSERT */ SET GLOBAL FOREIGN_KEY_CHECKS=0;","-- SELECT\nPRAGMA query_only=OFF;","/* guard */ SET FOREIGN_KEY_CHECKS=0+0;"]{assert!(collect(sql,d).is_err(),"{sql}");}
        }
    }
    #[test] fn first_statement_does_not_require_eof() {
        struct Meter { data:std::io::Cursor<Vec<u8>>, reads:Arc<std::sync::atomic::AtomicUsize> }
        impl Read for Meter { fn read(&mut self,b:&mut[u8])->io::Result<usize> { let n=self.data.read(b)?; self.reads.fetch_add(n,std::sync::atomic::Ordering::SeqCst); Ok(n) } }
        let mut bytes=b"SELECT 1;".to_vec(); bytes.resize(2*1024*1024,b' '); let reads=Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut r=Reader::new(Meter{data:std::io::Cursor::new(bytes),reads:reads.clone()},Dialect::Sqlite,None); assert!(r.next_statement().unwrap().is_some()); assert!(reads.load(std::sync::atomic::Ordering::SeqCst)<=BUFFER);
    }
}
