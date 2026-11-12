//! SQL 输入逐语句扫描：完整预扫描成功前不修改数据库。
//! 原文件由固定物理句柄读取，复核原始字节摘要；执行的是受控、带摘要的有界帧快照。
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;
use crate::models::{DbKind, Protocol};
use crate::transfer_stream::DecodeReader;
use super::transfer_output::{checked_absolute, identity, StagedOutput, BUFFER_BYTES};

const MAX_LOOKAHEAD: usize = 128;
const DEFAULT_SPILL_BUDGET: u64 = 8 * 1024 * 1024 * 1024;
#[derive(Clone, Copy, PartialEq, Eq)] pub(super) enum ScriptDialect { Sqlite, Mysql, Postgres }
impl ScriptDialect {
    pub(super) fn from_kind(kind:&DbKind)->Result<Self>{match kind.protocol(){
        Protocol::Sqlite=>Ok(Self::Sqlite),Protocol::Mysql=>Ok(Self::Mysql),Protocol::Postgres=>Ok(Self::Postgres),
        _=>bail!("当前通道尚未验证流式 SQL 脚本边界和事务回执；未发送 SQL"),
    }}
}
#[derive(Debug)] pub(super) struct Statement {pub(super) sql:String, keywords:Vec<String>}
enum Mode { Normal, Quote{close:u8,escaped:bool}, LineComment, BlockComment(usize), Dollar(Vec<u8>) }

pub(super) struct SqlStatementReader<R:Read> {
    reader:BufReader<R>, look:VecDeque<u8>, dialect:ScriptDialect, delimiter:Vec<u8>, line_start:bool, max_bytes:usize, backup_guards:bool,
}
impl<R:Read> SqlStatementReader<R> {
    pub(super) fn new(reader:R,dialect:ScriptDialect,max_bytes:usize)->Self {
        Self {reader:BufReader::with_capacity(BUFFER_BYTES,reader),look:VecDeque::with_capacity(MAX_LOOKAHEAD),dialect,delimiter:vec![b';'],line_start:true,max_bytes,backup_guards:false}
    }
    /// 仅本地备份还原允许生成器的三个精确会话守卫，普通导入仍完全拒绝控制命令。
    pub(super) fn for_backup_restore(reader:R,dialect:ScriptDialect,max_bytes:usize)->Self {
        let mut reader=Self::new(reader,dialect,max_bytes);reader.backup_guards=true;reader
    }
    fn peek(&mut self,n:usize)->Result<Option<u8>> {
        if n>=MAX_LOOKAHEAD {bail!("SQL 词法标记超过上限");}
        while self.look.len()<=n {
            let mut byte=[0u8];
            let read=loop {match self.reader.read(&mut byte){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,other=>break other?}};
            if read==0{return Ok(None);}self.look.push_back(byte[0]);
        }
        Ok(self.look.get(n).copied())
    }
    fn take(&mut self)->Result<Option<u8>> {
        if self.peek(0)?.is_none(){return Ok(None);}let b=self.look.pop_front().unwrap();
        if b==b'\n'||b==b'\r'{self.line_start=true;}else if !b.is_ascii_whitespace(){self.line_start=false;}
        Ok(Some(b))
    }
    fn starts(&mut self,bytes:&[u8],case_insensitive:bool)->Result<bool> {
        for (i,b) in bytes.iter().enumerate(){let Some(current)=self.peek(i)? else{return Ok(false)};
            if if case_insensitive{!current.eq_ignore_ascii_case(b)}else{current!=*b}{return Ok(false);}}
        Ok(true)
    }
    fn append(&self,out:&mut Vec<u8>,b:u8)->Result<()> {
        if out.len()==self.max_bytes{bail!("SQL 单条语句超过上限 {} 字节；预扫描失败，未修改目标",self.max_bytes);}
        if out.len()==out.capacity(){out.try_reserve_exact((BUFFER_BYTES).min(self.max_bytes-out.len()))?;}
        out.push(b);Ok(())
    }
    fn copy(&mut self,out:&mut Vec<u8>,count:usize)->Result<()> {for _ in 0..count{let b=self.take()?.ok_or_else(||anyhow!("SQL 词法标记意外结束"))?;self.append(out,b)?;}Ok(())}
    fn dollar_tag(&mut self)->Result<Option<Vec<u8>>> {
        if self.peek(0)?!=Some(b'$'){return Ok(None);}if self.peek(1)?==Some(b'$'){return Ok(Some(b"$$".to_vec()));}
        if !self.peek(1)?.is_some_and(|b|b.is_ascii_alphabetic()||b==b'_'){return Ok(None);}
        let mut tag=vec![b'$'];
        for i in 1..=64 {let Some(b)=self.peek(i)? else{return Ok(None)};
            if b==b'$'{tag.push(b);return Ok(Some(tag));}
            if !b.is_ascii_alphanumeric()&&b!=b'_'{return Ok(None);}tag.push(b);
        }
        bail!("PostgreSQL dollar 引用标签超过 63 字节，未修改目标")
    }
    fn directive(&mut self)->Result<bool> {
        if self.dialect!=ScriptDialect::Mysql||!self.line_start||!self.starts(b"DELIMITER",true)?||!self.peek(9)?.is_some_and(|b|b.is_ascii_whitespace()){return Ok(false);}
        for _ in 0..9{self.take()?;}let mut line=Vec::new();
        while let Some(b)=self.peek(0)? {if b==b'\r'||b==b'\n'{self.take()?;break;}if line.len()>64{bail!("MySQL DELIMITER 指令过长");}line.push(self.take()?.unwrap());}
        let delimiter=String::from_utf8(line)?.trim().as_bytes().to_vec();
        if delimiter.is_empty()||delimiter.len()>16||delimiter.iter().any(|b|b.is_ascii_whitespace()||b.is_ascii_alphanumeric()||matches!(*b,b'\''|b'"'|b'`'|b'\\'|b'#'))||delimiter.starts_with(b"--")||delimiter.starts_with(b"/*")||delimiter.starts_with(b"*/") {
            bail!("MySQL DELIMITER 必须是 1—16 字节、非注释/引号/字母数字的分隔符");}
        self.delimiter=delimiter;Ok(true)
    }
    pub(super) fn next_statement(&mut self)->Result<Option<Statement>> {
        let mut out=Vec::with_capacity(BUFFER_BYTES.min(self.max_bytes));let mut mode=Mode::Normal;
        let mut word=Vec::with_capacity(64);let mut word_long=false;let mut keywords=Vec::new();let mut has_sql=false;
        let mut trigger=false;let mut trigger_body=false;let mut body_depth=0usize;let mut case_depth=0usize;
        loop {
            let Some(b)=self.peek(0)? else {
                observe_word(&mut word,&mut word_long,&mut keywords,self.dialect,&mut trigger,&mut trigger_body,&mut body_depth,&mut case_depth,&self.delimiter)?;
                if !matches!(mode,Mode::Normal|Mode::LineComment){bail!("SQL 引号、dollar 引用或块注释未结束；预扫描失败，未修改目标");}
                if body_depth!=0||case_depth!=0 {bail!("SQLite 触发器 BEGIN/CASE 没有对应 END；未修改目标");}
                if !has_sql{return Ok(None);}if self.delimiter!=b";"{bail!("SQL 末尾缺少当前 DELIMITER；未修改目标");}
                return make_statement(out,keywords,self.backup_guards);
            };
            match &mut mode {
                Mode::LineComment=>{self.copy(&mut out,1)?;if b==b'\n'||b==b'\r'{mode=Mode::Normal;}continue;},
                Mode::BlockComment(depth)=>{
                    if self.starts(b"/*",false)? {
                        if self.dialect!=ScriptDialect::Postgres{bail!("当前方言的嵌套注释不受支持；未修改目标");}
                        *depth+=1;if *depth>128{bail!("SQL 嵌套注释超过上限");}self.copy(&mut out,2)?;
                    }else if self.starts(b"*/",false)?{*depth-=1;self.copy(&mut out,2)?;if *depth==0{mode=Mode::Normal;}}
                    else {self.copy(&mut out,1)?;}continue;
                },
                Mode::Dollar(tag)=>{if self.starts(tag,false)?{let length=tag.len();self.copy(&mut out,length)?;mode=Mode::Normal;}else{self.copy(&mut out,1)?;}continue;},
                Mode::Quote{close,escaped}=>{
                    if b==*close {self.copy(&mut out,1)?;if self.peek(0)?==Some(*close){self.copy(&mut out,1)?;}else{mode=Mode::Normal;}}
                    else if b==b'\\' {
                        if *escaped {self.copy(&mut out,2)?;}
                        else if self.dialect==ScriptDialect::Mysql||self.dialect==ScriptDialect::Postgres&&*close==b'\'' {
                            bail!("SQL 字符串含依赖会话模式的反斜杠；请使用无歧义字面量（PostgreSQL 可用 E 字符串），未修改目标");
                        } else {self.copy(&mut out,1)?;}
                    }else{self.copy(&mut out,1)?;}continue;
                },
                Mode::Normal=>{},
            }
            let word_byte=b.is_ascii_alphanumeric()||b==b'_'||b>=0x80||(b==b'$'&&!word.is_empty());
            if !word_byte {observe_word(&mut word,&mut word_long,&mut keywords,self.dialect,&mut trigger,&mut trigger_body,&mut body_depth,&mut case_depth,&self.delimiter)?;}
            if !has_sql&&self.directive()? {out.clear();continue;}
            if self.starts(&self.delimiter.clone(),false)?&&body_depth==0 {
                let delimiter=self.delimiter.len();for _ in 0..delimiter{self.take()?;}
                if !has_sql {out.clear();keywords.clear();continue;}
                if self.delimiter==b";"{self.append(&mut out,b';')?;}
                return make_statement(out,keywords,self.backup_guards);
            }
            let mysql_line=self.dialect!=ScriptDialect::Mysql||!self.peek(2)?.is_some_and(|third|!third.is_ascii_whitespace()&&third>0x20);
            if self.starts(b"--",false)?&&mysql_line {self.copy(&mut out,2)?;mode=Mode::LineComment;continue;}
            if b==b'#'&&self.dialect==ScriptDialect::Mysql{self.copy(&mut out,1)?;mode=Mode::LineComment;continue;}
            if self.starts(b"/*",false)? {
                if self.peek(2)?==Some(b'!')||self.peek(2)?.is_some_and(|c|c.eq_ignore_ascii_case(&b'M'))&&self.peek(3)?==Some(b'!') {bail!("SQL 脚本不允许 MySQL/MariaDB 可执行注释；完整预扫描拒绝，未修改目标");}
                self.copy(&mut out,2)?;mode=Mode::BlockComment(1);continue;
            }
            if matches!(b,b'\''|b'"'|b'`')||b==b'['&&self.dialect==ScriptDialect::Sqlite {
                has_sql=true;let escaped=self.dialect==ScriptDialect::Postgres&&b==b'\''&&out.last().is_some_and(|c|c.eq_ignore_ascii_case(&b'E'));
                self.copy(&mut out,1)?;mode=Mode::Quote{close:if b==b'['{b']'}else{b},escaped};continue;
            }
            if b==b'$'&&word.is_empty()&&self.dialect==ScriptDialect::Postgres {if let Some(tag)=self.dollar_tag()?{has_sql=true;self.copy(&mut out,tag.len())?;mode=Mode::Dollar(tag);continue;}}
            if b==0 {bail!("SQL 脚本包含 NUL；未修改目标");}
            if !b.is_ascii_whitespace(){has_sql=true;}
            if word_byte {if word.len()<64{word.push(b);}else{word_long=true;}}
            self.copy(&mut out,1)?;
        }
    }
}
fn observe_word(word:&mut Vec<u8>,long:&mut bool,keywords:&mut Vec<String>,dialect:ScriptDialect,trigger:&mut bool,body_seen:&mut bool,body:&mut usize,cases:&mut usize,delimiter:&[u8])->Result<()> {
    if word.is_empty(){return Ok(());}let text=if *long{String::new()}else{String::from_utf8_lossy(word).to_ascii_uppercase()};word.clear();*long=false;
    if keywords.len()<32 {keywords.push(text.clone());}
    if keywords.first().map(String::as_str)==Some("CREATE")&&keywords.len()<=8&&text=="TRIGGER" { *trigger=true; }
    if dialect==ScriptDialect::Mysql&&delimiter==b";"&&keywords.first().map(String::as_str)==Some("CREATE")&&text=="BEGIN"&&keywords.iter().any(|k|matches!(k.as_str(),"TRIGGER"|"PROCEDURE"|"FUNCTION"|"EVENT")) {
        bail!("MySQL 复合例程/触发器请显式使用 DELIMITER；未修改目标");
    }
    if dialect==ScriptDialect::Sqlite&&*trigger {
        if text=="BEGIN"&&!*body_seen {*body_seen=true;*body=1;}
        else if *body>0&&text=="CASE"{*cases+=1;if *cases>128{bail!("SQLite CASE 嵌套超过上限");}}
        else if *body>0&&text=="END"{if *cases>0{*cases-=1;}else{*body-=1;}}
    }
    Ok(())
}
fn make_statement(bytes:Vec<u8>,keywords:Vec<String>,backup_guards:bool)->Result<Option<Statement>> {
    let sql=String::from_utf8(bytes).context("SQL 输入不是有效 UTF-8")?;
    let statement=Statement{sql,keywords};
    let normalized=crate::db::strip_leading_comments(&statement.sql).trim().trim_end_matches(';').trim().to_ascii_uppercase();
    if !(backup_guards && matches!(normalized.as_str(), "SET NAMES UTF8MB4" | "SET FOREIGN_KEY_CHECKS = 0" | "SET FOREIGN_KEY_CHECKS = 1")) { validate_statement(&statement)?; }
    Ok(Some(statement))
}
fn validate_statement(statement:&Statement)->Result<()> {
    let first=statement.keywords.first().map(String::as_str).unwrap_or("");
    // 此处输入是流词法器完整划定的一条顶层语句。公共门禁只检查顶层前缀，不能再次把 trigger/function body 的分号拆开。
    crate::execution_policy::ordinary_native_batch(&statement.keywords.join(" "))?;
    if matches!(first,"VACUUM"|"ABORT"|"COPY"|"PREPARE"|"EXECUTE"|"CALL"|"DO"|"DELIMITER"|"GRANT"|"REVOKE"|"LOAD"|"INSTALL"|"UNINSTALL"|"FLUSH") {
        bail!("SQL 脚本包含需要专门执行通道的控制/过程/文件命令；完整预扫描拒绝，未修改目标");
    }
    if matches!(first,"CREATE"|"DROP"|"ALTER")&&statement.keywords.iter().take(4).any(|word|matches!(word.as_str(),"DATABASE"|"ROLE"|"USER"|"TABLESPACE"|"SERVER"|"EXTENSION")) {
        bail!("SQL 脚本不能改变数据库/身份/服务器范围；未修改目标");
    }
    if statement.keywords.windows(2).any(|pair|pair[0]=="INTO"&&matches!(pair[1].as_str(),"OUTFILE"|"DUMPFILE")) {bail!("SQL 脚本不能请求服务器文件输出；未修改目标");}
    Ok(())
}

struct HashingReader {file:File,digest:Arc<Mutex<Sha256>>}
impl Read for HashingReader {fn read(&mut self,bytes:&mut[u8])->io::Result<usize>{let read=self.file.read(bytes)?;self.digest.lock().unwrap().update(&bytes[..read]);Ok(read)}}

fn frame_mac(key:&[u8;32],index:u64,length:&[u8;4],sql:&[u8])->[u8;32] {
    let mut inner_pad=[0x36u8;64];let mut outer_pad=[0x5cu8;64];
    for i in 0..key.len(){inner_pad[i]^=key[i];outer_pad[i]^=key[i];}
    let mut inner=Sha256::new();inner.update(inner_pad);inner.update(index.to_le_bytes());inner.update(length);inner.update(sql);
    let mut outer=Sha256::new();outer.update(outer_pad);outer.update(inner.finalize());
    inner_pad.zeroize();outer_pad.zeroize();outer.finalize().into()
}
pub(super) struct PreparedScript {
    _stage:StagedOutput, reader:BufReader<File>, digest:[u8;32], read_digest:Sha256, key:[u8;32], count:u64, remaining:u64, max_bytes:usize, last_counts_rows:bool,
}
impl Drop for PreparedScript {fn drop(&mut self){self.key.zeroize();}}
impl PreparedScript {
    pub(super) fn prepare(path:&str,encoding:&str,kind:&DbKind,max_bytes:usize,spill_budget:Option<&str>,mut check:impl FnMut()->Result<()>)->Result<Self> {
        check()?;let dialect=ScriptDialect::from_kind(kind)?;let path=checked_absolute(Path::new(path))?;
        let mut options=OpenOptions::new();options.read(true);
        #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(1);}
        let mut source=options.open(&path).context("无法固定 SQL 输入文件")?;let source_id=identity(&source)?;let before=source.metadata()?;
        if !before.is_file(){bail!("SQL 输入必须是普通文件");}
        let budget=spill_budget.map(str::parse::<u64>).transpose().context("SQL spill 上限必须是十进制字节字符串")?.unwrap_or(DEFAULT_SPILL_BUDGET);
        let target=std::env::temp_dir().join(format!("navkit-sql-preflight-{}.snapshot",uuid::Uuid::new_v4()));
        let stage=StagedOutput::new(target.to_str().ok_or_else(||anyhow!("SQL 临时目录不是有效文本路径"))?)?;
        let mut output=BufWriter::with_capacity(BUFFER_BYTES,stage.file_clone()?);let source_digest=Arc::new(Mutex::new(Sha256::new()));
        let input=DecodeReader::new(HashingReader{file:source.try_clone()?,digest:source_digest.clone()},encoding)?;
        let mut parser=SqlStatementReader::new(input,dialect,max_bytes);let mut hash=Sha256::new();let mut size=0u64;let mut count=0u64;
        let mut secret=Sha256::new();secret.update(uuid::Uuid::new_v4().as_bytes());secret.update(uuid::Uuid::new_v4().as_bytes());let key:[u8;32]=secret.finalize().into();
        while let Some(statement)=parser.next_statement()? {
            check()?;let bytes=u32::try_from(statement.sql.len()).context("SQL 单条语句超过可用帧大小")?;
            if bytes>0x7fff_ffff {bail!("SQL 单条语句超过可用帧大小");}
            let counts_rows=statement.keywords.first().is_some_and(|word|matches!(word.as_str(),"INSERT"|"UPDATE"|"DELETE"|"REPLACE"));
            let length=(bytes|if counts_rows {0x8000_0000}else{0}).to_le_bytes();
            size=size.checked_add(36+statement.sql.len() as u64).ok_or_else(||anyhow!("SQL 快照大小溢出"))?;
            if size>budget{bail!("SQL 预扫描快照超过 spill 上限；未修改目标");}
            let mac=frame_mac(&key,count,&length,statement.sql.as_bytes());
            output.write_all(&length)?;output.write_all(&mac)?;output.write_all(statement.sql.as_bytes())?;hash.update(length);hash.update(mac);hash.update(statement.sql.as_bytes());count+=1;
        }
        drop(parser);check()?;if count==0{bail!("SQL 文件不包含可执行语句；未修改目标");}
        output.flush()?;output.get_ref().sync_all()?;drop(output);
        // Unix 写入者可能不遵守 advisory lock，因此同物理句柄复读原字节，并与实际解析输入摘要严格相等。
        let expected_source:[u8;32]=source_digest.lock().unwrap().clone().finalize().into();source.seek(SeekFrom::Start(0))?;
        let mut rehash=Sha256::new();let mut bytes=vec![0;BUFFER_BYTES];
        loop {check()?;let read=loop{match source.read(&mut bytes){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,other=>break other?}};if read==0{break;}rehash.update(&bytes[..read]);}
        let after=source.metadata()?;let current=options.open(&path)?;
        if identity(&current)?!=source_id||before.len()!=after.len()||before.modified()?!=after.modified()?||<[u8;32]>::from(rehash.finalize())!=expected_source {bail!("SQL 输入在预扫描时发生变化；未修改目标");}
        let digest=hash.finalize().into();let mut file=stage.file_clone()?;file.seek(SeekFrom::Start(0))?;
        let mut prepared=Self{_stage:stage,reader:BufReader::with_capacity(BUFFER_BYTES,file),digest,read_digest:Sha256::new(),key,count,remaining:count,max_bytes,last_counts_rows:false};
        prepared.verify()?;check()?;Ok(prepared)
    }
    pub(super) fn count(&self)->u64{self.count}
    pub(super) fn counts_rows(&self)->bool{self.last_counts_rows}
    pub(super) fn verify(&mut self)->Result<()> {
        self.reader.seek(SeekFrom::Start(0))?;let mut hash=Sha256::new();let mut bytes=vec![0;BUFFER_BYTES];
        loop{let read=self.reader.read(&mut bytes)?;if read==0{break;}hash.update(&bytes[..read]);}
        if <[u8;32]>::from(hash.finalize())!=self.digest {bail!("SQL 预扫描快照摘要不匹配；未修改目标");}
        self.reader.seek(SeekFrom::Start(0))?;self.read_digest=Sha256::new();self.remaining=self.count;Ok(())
    }
    pub(super) fn next_statement(&mut self)->Result<Option<String>> {
        if self.remaining==0 {
            let mut tail=[0];if self.reader.read(&mut tail)?!=0||<[u8;32]>::from(self.read_digest.clone().finalize())!=self.digest {bail!("SQL 快照在执行过程中变化；停止发送后续语句");}return Ok(None);
        }
        let mut header=[0u8;4];self.reader.read_exact(&mut header)?;let bits=u32::from_le_bytes(header);let length=(bits&0x7fff_ffff) as usize;
        if length==0||length>self.max_bytes{bail!("SQL 快照帧超过单条语句上限");}
        let mut mac=[0u8;32];self.reader.read_exact(&mut mac)?;let mut bytes=vec![0;length];self.reader.read_exact(&mut bytes)?;
        let expected=frame_mac(&self.key,self.count-self.remaining,&header,&bytes);
        if mac.iter().zip(expected.iter()).fold(0u8,|diff,(a,b)|diff|(*a^*b))!=0 {bail!("SQL 快照帧授权/顺序不匹配；本条及后续语句未发送");}
        self.read_digest.update(header);self.read_digest.update(mac);self.read_digest.update(&bytes);self.remaining-=1;self.last_counts_rows=bits&0x8000_0000!=0;
        Ok(Some(String::from_utf8(bytes).context("SQL 快照帧含无效 UTF-8")?))
    }
}

#[cfg(test)] mod tests {
    use super::*;
    fn statements(text:&str,dialect:ScriptDialect)->Result<Vec<String>> {
        let mut parser=SqlStatementReader::new(text.as_bytes(),dialect,16*1024*1024);let mut out=Vec::new();while let Some(s)=parser.next_statement()?{out.push(s.sql);}Ok(out)
    }
    #[test] fn sql_reader_quotes_comments_and_literal_controls_are_not_split() {
        let sql="-- comment ;\nCREATE TABLE \"t;quote\" (v TEXT); INSERT INTO \"t;quote\" VALUES ('a;''b; BEGIN COMMIT'); /* ; */ SELECT 'x';";
        let out=statements(sql,ScriptDialect::Sqlite).unwrap();assert_eq!(out.len(),3);assert!(out[1].contains("a;''b; BEGIN COMMIT"));
    }
    #[test] fn sql_reader_sqlite_trigger_keeps_case_and_internal_statements() {
        let sql="CREATE TABLE t(v INTEGER);CREATE TABLE audit(v TEXT);CREATE TRIGGER \"tr;one\" AFTER INSERT ON t BEGIN INSERT INTO audit VALUES (CASE WHEN NEW.v=1 THEN 'a; END' ELSE 'b' END); UPDATE t SET v=2 WHERE v=1; END;INSERT INTO t VALUES(1);";
        let out=statements(sql,ScriptDialect::Sqlite).unwrap();assert_eq!(out.len(),4);assert!(out[2].ends_with("END;"));assert!(out[2].contains("UPDATE t SET"));
    }
    #[test] fn sql_reader_pg_dollar_e_strings_and_nested_comments() {
        let out=statements("/* outer /* nested */ end */ CREATE FUNCTION f() RETURNS void AS $body$ BEGIN PERFORM 'x;'; END; $body$ LANGUAGE plpgsql; SELECT E'a\\\';b';",ScriptDialect::Postgres).unwrap();
        assert_eq!(out.len(),2);assert!(out[0].contains("PERFORM 'x;'"));
    }
    #[test] fn sql_reader_mysql_delimiter_procedure_and_reset() {
        let out=statements("DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT ';'; IF 1=1 THEN SELECT 2; END IF; END$$\nDELIMITER ;\nINSERT INTO t VALUES ('literal $$');",ScriptDialect::Mysql).unwrap();
        assert_eq!(out.len(),2);assert!(out[0].contains("END IF; END"));assert!(!out[0].contains("DELIMITER"));
    }
    #[test] fn sql_reader_entire_preflight_rejects_late_controls_and_executable_comments() {
        for tail in ["BEGIN;","/* hidden */ COMMIT;","SET sql_mode='';","USE other;","ATTACH 'x' AS x;","/*! SET autocommit=0 */;","/*M! ROLLBACK */;","COPY t FROM STDIN;","CALL p();"] {
            assert!(statements(&format!("INSERT INTO t VALUES(1);{tail}"),ScriptDialect::Mysql).is_err(),"tail {tail}");
        }
    }
    #[test] fn sql_reader_unterminated_or_ambiguous_literals_fail_closed() {
        for text in ["SELECT 'x", "SELECT /* x", "CREATE TRIGGER t AFTER INSERT ON x BEGIN SELECT 1;"]{assert!(statements(text,ScriptDialect::Sqlite).is_err(),"{text}");}
        assert!(statements("SELECT 'a\\b';",ScriptDialect::Mysql).is_err());assert!(statements("SELECT 'a\\b';",ScriptDialect::Postgres).is_err());
        assert_eq!(statements("SELECT 'a\\b';",ScriptDialect::Sqlite).unwrap().len(),1);
        assert!(statements("CREATE FUNCTION f() RETURNS void AS $tag$ x;",ScriptDialect::Postgres).is_err());
        assert!(statements("DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END",ScriptDialect::Mysql).is_err());
        assert!(statements("CREATE PROCEDURE p() BEGIN SELECT 1; END;",ScriptDialect::Mysql).is_err());
    }
    struct ShortRead<'a>{bytes:&'a[u8],chunk:usize}
    impl Read for ShortRead<'_>{fn read(&mut self,b:&mut[u8])->io::Result<usize>{let n=b.len().min(self.chunk).min(self.bytes.len());b[..n].copy_from_slice(&self.bytes[..n]);self.bytes=&self.bytes[n..];Ok(n)}}
    #[test] fn sql_reader_dollar_directive_and_quote_markers_cross_all_read_boundaries() {
        for (dialect,sql,count) in [(ScriptDialect::Postgres,"CREATE FUNCTION f() RETURNS text AS $abc$ SELECT '中;'; $abc$ LANGUAGE sql;SELECT 'é';",2),
            (ScriptDialect::Mysql,"DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 'a;''b'; END//\nDELIMITER ;\nSELECT 2;",2),
            (ScriptDialect::Sqlite,"CREATE TRIGGER tr AFTER INSERT ON t BEGIN SELECT CASE WHEN 1 THEN 'é;' ELSE '中' END; END;SELECT 2;",2)] {
            for chunk in 1..=9 {let input=ShortRead{bytes:sql.as_bytes(),chunk};let mut parser=SqlStatementReader::new(input,dialect,4096);let mut n=0;while parser.next_statement().unwrap().is_some(){n+=1;}assert_eq!(n,count);}
        }
    }
    struct Generated {left:usize,at:usize,pattern:&'static[u8],max_requested:usize}
    impl Read for Generated {fn read(&mut self,b:&mut[u8])->io::Result<usize>{self.max_requested=self.max_requested.max(b.len());let n=b.len().min(self.left);for item in &mut b[..n]{*item=self.pattern[self.at%self.pattern.len()];self.at+=1;}self.left-=n;Ok(n)}}
    #[test] fn sql_reader_large_generated_input_keeps_only_one_bounded_statement() {
        let pattern=b"INSERT INTO t VALUES(1);";let mut parser=SqlStatementReader::new(Generated{left:pattern.len()*250_000,at:0,pattern,max_requested:0},ScriptDialect::Sqlite,1024);
        let mut count=0;while let Some(statement)=parser.next_statement().unwrap(){assert!(statement.sql.len()<=1024);assert!(parser.look.capacity()<=MAX_LOOKAHEAD);count+=1;}
        assert_eq!(count,250_000);assert!(parser.reader.get_ref().max_requested<=BUFFER_BYTES);
    }
    #[test] fn sql_reader_oversized_single_statement_fails_at_bound() {
        const TOTAL:usize=1024*1024;
        for bound in [1,4096,BUFFER_BYTES-1,BUFFER_BYTES,BUFFER_BYTES+1] {
            let input=Generated{left:TOTAL,at:0,pattern:b"SELECT 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",max_requested:0};
            let mut parser=SqlStatementReader::new(input,ScriptDialect::Sqlite,bound);
            let error=parser.next_statement().unwrap_err();assert!(error.to_string().contains("超过上限"));
            let input=parser.reader.get_ref();let fetched=TOTAL-input.left;
            // take 在 append 拒绝前最多消费 bound+1；64 KiB BufReader 与固定 lookahead 的预读不属于语句内存。
            let logical_consumed=fetched-parser.reader.buffer().len()-parser.look.len();
            assert_eq!(logical_consumed,bound+1,"single-statement bound {bound}");
            let read_ceiling=(bound+1+MAX_LOOKAHEAD).div_ceil(BUFFER_BYTES)*BUFFER_BYTES;
            assert!(fetched<=read_ceiling,"source read {fetched} exceeds fixed prefetch budget {read_ceiling}");
            assert!(input.max_requested<=BUFFER_BYTES);assert!(parser.look.len()<=MAX_LOOKAHEAD);
            if bound==4096 {assert_eq!(fetched,BUFFER_BYTES);assert_eq!(input.left,TOTAL-BUFFER_BYTES);}
        }
    }
    struct Directory(std::path::PathBuf);
    impl Directory {fn new()->Self{let p=std::env::temp_dir().join(format!("navkit-sql-test-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&p).unwrap();Self(p)}}
    impl Drop for Directory{fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);}}
    #[test] fn sql_snapshot_complete_preflight_uses_immutable_normalized_frames() {
        let dir=Directory::new();let source=dir.0.join("input.sql");std::fs::write(&source,"CREATE TABLE t(v TEXT);INSERT INTO t VALUES('é');").unwrap();
        let mut script=PreparedScript::prepare(source.to_str().unwrap(),"utf8",&DbKind::of("sqlite"),4096,None,||Ok(())).unwrap();
        std::fs::write(&source,"COMMIT;").unwrap();assert_eq!(script.count(),2);assert!(script.next_statement().unwrap().unwrap().contains("CREATE TABLE"));assert!(script.next_statement().unwrap().unwrap().contains("'é'"));assert!(script.next_statement().unwrap().is_none());
    }
    #[test] fn sql_snapshot_bad_late_statement_and_spill_limit_fail_before_execution() {
        let dir=Directory::new();let source=dir.0.join("input.sql");std::fs::write(&source,"CREATE TABLE t(v TEXT);COMMIT;").unwrap();
        assert!(PreparedScript::prepare(source.to_str().unwrap(),"utf8",&DbKind::of("sqlite"),4096,None,||Ok(())).is_err());
        std::fs::write(&source,"CREATE TABLE t(v TEXT);").unwrap();assert!(PreparedScript::prepare(source.to_str().unwrap(),"utf8",&DbKind::of("sqlite"),4096,Some("8"),||Ok(())).is_err());
    }
    #[test] fn sql_snapshot_non_utf8_and_invalid_tail_are_strict() {
        let dir=Directory::new();let source=dir.0.join("latin.sql");std::fs::write(&source,b"INSERT INTO t VALUES('\xe9');").unwrap();
        let mut script=PreparedScript::prepare(source.to_str().unwrap(),"latin1",&DbKind::of("sqlite"),4096,None,||Ok(())).unwrap();assert!(script.next_statement().unwrap().unwrap().contains("é"));
        std::fs::write(&source,b"CREATE TABLE t(v);\xe4\xb8").unwrap();assert!(PreparedScript::prepare(source.to_str().unwrap(),"utf8",&DbKind::of("sqlite"),4096,None,||Ok(())).is_err());
    }
    #[test] fn sql_snapshot_cancel_preflight_and_frame_tampering_do_not_send() {
        let dir=Directory::new();let source=dir.0.join("input.sql");std::fs::write(&source,"INSERT INTO t VALUES(1);").unwrap();
        assert!(PreparedScript::prepare(source.to_str().unwrap(),"utf8",&DbKind::of("sqlite"),4096,None,||bail!("cancel")).is_err());
        let mut script=PreparedScript::prepare(source.to_str().unwrap(),"utf8",&DbKind::of("sqlite"),4096,None,||Ok(())).unwrap();let mut file=script._stage.file_clone().unwrap();file.seek(SeekFrom::Start(4)).unwrap();file.write_all(b"COMMIT").unwrap();file.flush().unwrap();assert!(script.verify().is_err());
    }
}
