//! 私有、有预算的 SQLite spill。原生值用长度前缀二进制记录，不借 JSON/f64 传精确整数或二进制。
use anyhow::{anyhow, bail, Context, Result};
use futures::TryStreamExt;
use sha2::{Digest, Sha256};
use sqlx::{Connection, Row, Sqlite, SqliteConnection, TypeInfo, ValueRef};
use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions, SqliteJournalMode, SqliteRow};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use zeroize::Zeroize;

pub(super) const RECORD_BYTES: usize = 16 * 1024 * 1024;
pub(super) const SPILL_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_COLUMNS: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Cell { Null, Integer(i64), Real(u64), Text(String), Blob(Vec<u8>) }
impl Cell {
    pub(super) fn decimal(&self) -> Option<sqlx::types::BigDecimal> {
        match self {
            Self::Integer(value) => value.to_string().parse().ok(),
            Self::Real(bits) => sqlx::types::BigDecimal::try_from(f64::from_bits(*bits)).ok(),
            // 控制指数，避免一个很短的恶意数值文本诱发不受限的十进制展开。
            Self::Text(value) if value.len() <= 16384 => {
                let text = value.trim();
                if let Some((_, exponent)) = text.rsplit_once(['e', 'E']) {
                    if exponent.parse::<i64>().map_or(true, |exponent| exponent.unsigned_abs() > 16384) { return None; }
                }
                text.parse().ok()
            }
            _ => None,
        }
    }
}
pub(super) fn decode_row(row: &SqliteRow) -> Result<Vec<Cell>> {
    if row.len() > MAX_COLUMNS { bail!("SQLite 同步列数超过已验证范围"); }
    let mut cells = Vec::with_capacity(row.len());
    let mut bytes = 4usize;
    for index in 0..row.len() {
        let raw = row.try_get_raw(index)?;
        let cell = if raw.is_null() { Cell::Null } else {
            match raw.type_info().name() {
                "INTEGER" => Cell::Integer(row.try_get(index)?),
                "REAL" => { let value: f64 = row.try_get(index)?; if !value.is_finite() { bail!("SQLite 同步不接受非有限 REAL"); } Cell::Real(value.to_bits()) },
                "TEXT" => Cell::Text(row.try_get(index).context("SQLite TEXT 不是可无损读取的 UTF-8，未伪造 NULL")?),
                "BLOB" => Cell::Blob(row.try_get(index).context("SQLite BLOB 不能无损读取，未伪造 NULL")?),
                ty => bail!("SQLite 存储类型 {ty} 尚未验证，未伪造 NULL"),
            }
        };
        bytes = bytes.checked_add(cell_bytes(&cell)?).ok_or_else(|| anyhow!("同步记录长度溢出"))?;
        if bytes > RECORD_BYTES { bail!("同步单条记录超过16MiB，目标尚未提交"); }
        cells.push(cell);
    }
    Ok(cells)
}
fn cell_bytes(cell: &Cell) -> Result<usize> { Ok(match cell { Cell::Null => 1, Cell::Integer(_) | Cell::Real(_) => 9, Cell::Text(value) => value.len().checked_add(5).ok_or_else(||anyhow!("记录长度溢出"))?, Cell::Blob(value) => value.len().checked_add(5).ok_or_else(||anyhow!("记录长度溢出"))? }) }
fn write_bytes(out: &mut Vec<u8>, value: &[u8]) -> Result<()> { out.extend_from_slice(&u32::try_from(value.len())?.to_le_bytes()); out.extend_from_slice(value); Ok(()) }
pub(super) fn encode_cells(cells: &[Cell]) -> Result<Vec<u8>> {
    if cells.len() > MAX_COLUMNS { bail!("记录列数超限"); }
    let length = cells.iter().try_fold(4usize, |length, cell| length.checked_add(cell_bytes(cell)?).ok_or_else(||anyhow!("记录长度溢出")))?;
    if length > RECORD_BYTES { bail!("同步单条记录超过16MiB"); }
    let mut out = Vec::with_capacity(length); out.extend_from_slice(&u32::try_from(cells.len())?.to_le_bytes());
    for cell in cells { match cell {
        Cell::Null => out.push(0),
        Cell::Integer(value) => { out.push(1); out.extend_from_slice(&value.to_le_bytes()); },
        Cell::Real(value) => { if !f64::from_bits(*value).is_finite() { bail!("记录 REAL 非有限"); } out.push(2); out.extend_from_slice(&value.to_le_bytes()); },
        Cell::Text(value) => { out.push(3); write_bytes(&mut out, value.as_bytes())?; },
        Cell::Blob(value) => { out.push(4); write_bytes(&mut out, value)?; },
    } }
    Ok(out)
}
pub(super) fn decode_cells(bytes: &[u8]) -> Result<Vec<Cell>> {
    if bytes.len() > RECORD_BYTES { bail!("spill记录超过16MiB"); }
    fn take<'a>(input: &mut &'a [u8], length: usize) -> Result<&'a [u8]> { if input.len() < length { bail!("spill记录截断"); } let (head, tail)=input.split_at(length); *input=tail; Ok(head) }
    fn length(input: &mut &[u8]) -> Result<usize> { Ok(u32::from_le_bytes(take(input,4)?.try_into()?) as usize) }
    let mut input = bytes; let count = length(&mut input)?; if count > MAX_COLUMNS || count > input.len() { bail!("spill记录列数损坏"); }
    let mut cells = Vec::with_capacity(count);
    for _ in 0..count { let tag=take(&mut input,1)?[0]; cells.push(match tag {
        0 => Cell::Null,
        1 => Cell::Integer(i64::from_le_bytes(take(&mut input,8)?.try_into()?)),
        2 => { let bits=u64::from_le_bytes(take(&mut input,8)?.try_into()?); if !f64::from_bits(bits).is_finite() {bail!("spill REAL 非有限");} Cell::Real(bits) },
        3 => { let size=length(&mut input)?; Cell::Text(std::str::from_utf8(take(&mut input,size)?)?.to_owned()) },
        4 => { let size=length(&mut input)?; Cell::Blob(take(&mut input,size)?.to_vec()) },
        _ => bail!("spill记录类型损坏"),
    }); }
    if !input.is_empty() { bail!("spill记录含未认证尾部"); } Ok(cells)
}
pub(super) fn key(cells: &[Cell]) -> Result<Vec<u8>> {
    if cells.is_empty() || cells.len()>8 {bail!("同步需要1至8列完整业务键或主键");}
    let _=encode_cells(cells)?;
    let mut out=Vec::new(); out.extend_from_slice(&u32::try_from(cells.len())?.to_le_bytes());
    for cell in cells { match cell {
        Cell::Null => bail!("同步键为NULL，目标未提交"),
        Cell::Integer(_) | Cell::Real(_) => { out.push(5); let value=cell.decimal().ok_or_else(||anyhow!("同步数值键不能精确解码"))?.normalized().to_string(); write_bytes(&mut out,value.as_bytes())?; },
        Cell::Text(value) => {out.push(3);write_bytes(&mut out,value.as_bytes())?;},
        Cell::Blob(value) => {out.push(4);write_bytes(&mut out,value)?;},
    } if out.len()>RECORD_BYTES {bail!("同步复合键超过16MiB");} }
    Ok(out)
}
pub(super) fn bind<'q>(mut query: crate::sql_history::Query<'q, Sqlite, SqliteArguments<'q>>, cells: &'q [Cell]) -> crate::sql_history::Query<'q, Sqlite, SqliteArguments<'q>> {
    for cell in cells {query=match cell { Cell::Null=>query.bind(Option::<String>::None), Cell::Integer(value)=>query.bind(*value), Cell::Real(bits)=>query.bind(f64::from_bits(*bits)), Cell::Text(value)=>query.bind(value.as_str()), Cell::Blob(value)=>query.bind(value.as_slice()) };} query
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct FileId(u64,u64);
#[cfg(unix)] fn file_id(file: &File) -> Result<FileId> {use std::os::unix::fs::MetadataExt; let metadata=file.metadata()?;Ok(FileId(metadata.dev(),metadata.ino()))}
#[cfg(windows)] fn file_id(file: &File) -> Result<FileId> {use std::os::windows::io::AsRawHandle; windows_id(file.as_raw_handle())}
#[cfg(windows)] fn windows_id(handle: *mut std::ffi::c_void) -> Result<FileId> {
    #[repr(C)] struct Info {attributes:u32, creation:[u32;2], access:[u32;2], written:[u32;2], volume:u32, size_hi:u32,size_lo:u32,links:u32,index_hi:u32,index_lo:u32}
    #[link(name="kernel32")] unsafe extern "system" {fn GetFileInformationByHandle(handle:*mut std::ffi::c_void,info:*mut Info)->i32;}
    let mut info=Info{attributes:0,creation:[0;2],access:[0;2],written:[0;2],volume:0,size_hi:0,size_lo:0,links:0,index_hi:0,index_lo:0};
    if unsafe{GetFileInformationByHandle(handle,&mut info)}==0 {return Err(std::io::Error::last_os_error().into());}
    Ok(FileId(info.volume as u64,((info.index_hi as u64)<<32)|info.index_lo as u64))
}
#[cfg(not(any(unix,windows)))] fn file_id(_file:&File)->Result<FileId>{bail!("当前平台未验证SQLite物理文件身份")}
pub(super) struct Pin {pub(super) path:PathBuf, file:File, id:FileId, directory:File, directory_id:FileId}
impl Pin {
    pub(super) fn open(path:&Path, create:bool)->Result<Self>{
        let parent=std::fs::canonicalize(path.parent().filter(|path|!path.as_os_str().is_empty()).unwrap_or(Path::new(".")))?;
        let path=parent.join(path.file_name().ok_or_else(||anyhow!("SQLite路径无文件名"))?);
        if !create {if std::fs::symlink_metadata(&path)?.file_type().is_symlink(){bail!("SQLite路径含符号链接，物理身份未验证");}}
        let mut dir=OpenOptions::new();dir.read(true);
        #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;dir.custom_flags(0x02000000).share_mode(3);}
        let directory=dir.open(&parent)?;let directory_id=file_id(&directory)?;
        let mut options=OpenOptions::new();options.read(true).write(create).create_new(create);
        #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.share_mode(3);}
        #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.mode(0o600);}
        let file=options.open(&path)?;if !file.metadata()?.is_file(){bail!("SQLite路径必须是普通文件");}let id=file_id(&file)?;
        Ok(Self{path,file,id,directory,directory_id})
    }
    pub(super) fn same(&self,other:&Self)->bool{self.id==other.id}
    pub(super) fn identity(&self)->FileId{self.id}
    pub(super) fn check(&self)->Result<()>{
        let parent=self.path.parent().ok_or_else(||anyhow!("SQLite路径无父目录"))?;
        let mut options=OpenOptions::new();options.read(true);
        #[cfg(windows)] {use std::os::windows::fs::OpenOptionsExt;options.custom_flags(0x02000000).share_mode(3);}
        if file_id(&options.open(parent)?)?!=self.directory_id || file_id(&self.directory)?!=self.directory_id || file_id(&File::open(&self.path)?)?!=self.id || file_id(&self.file)?!=self.id {bail!("SQLite物理文件/目录身份已变化");}Ok(())
    }
    pub(super) async fn verify_database(&self,connection:&mut SqliteConnection)->Result<()>{
        self.check()?;
        #[cfg(windows)] {
            let mut locked=connection.lock_handle().await?;let mut raw_handle:*mut std::ffi::c_void=std::ptr::null_mut();
            let result=unsafe{libsqlite3_sys::sqlite3_file_control(locked.as_raw_handle().as_ptr(),b"main\0".as_ptr().cast(),libsqlite3_sys::SQLITE_FCNTL_WIN32_GET_HANDLE,(&mut raw_handle as *mut *mut std::ffi::c_void).cast())};
            if result!=libsqlite3_sys::SQLITE_OK || raw_handle.is_null(){bail!("当前SQLite VFS未提供可验证的物理文件句柄");}
            if windows_id(raw_handle)?!=self.id{bail!("SQLite连接仍指向被替换的旧文件，拒绝错靶同步");}
        }
        #[cfg(unix)] {
            let mut stream=crate::sql_history::query("PRAGMA database_list").fetch(&mut *connection);let mut found=false;
            while let Some(row)=stream.try_next().await?{let name:String=row.try_get("name")?;if name=="main"{let path:String=row.try_get("file")?;if file_id(&File::open(path)?)?!=self.id{bail!("SQLite连接主库路径与固定物理身份不符");}found=true;}}drop(stream);if !found{bail!("SQLite连接缺少可验证的main库");}
            let mut locked=connection.lock_handle().await?;let mut moved=0i32;let result=unsafe{libsqlite3_sys::sqlite3_file_control(locked.as_raw_handle().as_ptr(),b"main\0".as_ptr().cast(),libsqlite3_sys::SQLITE_FCNTL_HAS_MOVED,(&mut moved as *mut i32).cast())};
            if result!=libsqlite3_sys::SQLITE_OK||moved!=0{bail!("当前SQLite VFS无法证明连接仍属于固定文件，拒绝错靶同步");}
        }
        #[cfg(not(any(unix,windows)))] {let _: &mut SqliteConnection=connection;bail!("当前平台未验证SQLite连接物理身份");}
        self.check()?;Ok(())
    }
}

fn mac(secret:&[u8;32],domain:u8,sequence:u64,key:&[u8],body:&[u8])->[u8;32]{
    let mut inner_pad=[0x36u8;64];let mut outer_pad=[0x5cu8;64];for (index,byte) in secret.iter().enumerate(){inner_pad[index]^=*byte;outer_pad[index]^=*byte;}
    let mut hash=Sha256::new();hash.update(inner_pad);hash.update(b"navkit-sync-spill-v1");hash.update([domain]);hash.update(sequence.to_le_bytes());hash.update((key.len() as u64).to_le_bytes());hash.update(key);hash.update((body.len() as u64).to_le_bytes());hash.update(body);
    let mut outer=Sha256::new();outer.update(outer_pad);outer.update(hash.finalize());outer.finalize().into()
}
fn feed(hash:&mut Sha256,sequence:u64,key:&[u8],body:&[u8],mac:&[u8]){hash.update(sequence.to_le_bytes());hash.update((key.len() as u64).to_le_bytes());hash.update(key);hash.update((body.len() as u64).to_le_bytes());hash.update(body);hash.update(mac);}
pub(super) struct Spill {connection:Option<SqliteConnection>, pin:Option<Pin>, secret:[u8;32], pub(super) rows:u64, bytes:u64, source_hash:Sha256, source_seal:Option<[u8;32]>, target_rows:u64, target_hash:Sha256, mapped_rows:u64, mapped_hash:Sha256, pub(super) verified_rows:u64, verified_hash:Sha256, probe_columns:usize, probe_sql:String, write_transaction:bool, pending_writes:usize}
impl Spill {
    pub(super) async fn new()->Result<Self>{
        let path=std::fs::canonicalize(std::env::temp_dir())?.join(format!("navkit-sync-{}.sqlite",uuid::Uuid::new_v4().simple()));let pin=Pin::open(&path,true)?;
        let options=SqliteConnectOptions::new().filename(&pin.path).create_if_missing(false).journal_mode(SqliteJournalMode::Off).pragma("cache_size","-2048").pragma("temp_store","FILE").pragma("page_size","4096").pragma("max_page_count","2097152").row_buffer_size(1).statement_cache_capacity(8);
        let mut secret=[0u8;32];secret[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());secret[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        let mut spill=Self{connection:None,pin:Some(pin),secret,rows:0,bytes:0,source_hash:Sha256::new(),source_seal:None,target_rows:0,target_hash:Sha256::new(),mapped_rows:0,mapped_hash:Sha256::new(),verified_rows:0,verified_hash:Sha256::new(),probe_columns:0,probe_sql:String::new(),write_transaction:false,pending_writes:0};
        let setup:Result<()>=async{spill.connection=Some(SqliteConnection::connect_with(&options).await?);crate::sql_history::query("CREATE TABLE source_rows(seq INTEGER PRIMARY KEY,k BLOB NOT NULL UNIQUE,body BLOB NOT NULL,mac BLOB NOT NULL); CREATE TABLE target_keys(seq INTEGER PRIMARY KEY,k BLOB NOT NULL UNIQUE,mac BLOB NOT NULL); CREATE TABLE mapped_keys(seq INTEGER PRIMARY KEY,k BLOB NOT NULL UNIQUE,mac BLOB NOT NULL); CREATE TABLE verified_rows(seq INTEGER PRIMARY KEY,k BLOB NOT NULL,body BLOB NOT NULL,mac BLOB NOT NULL)").execute(spill.connection()?).await?;Ok(())}.await;
        if let Err(error)=setup{let cleanup=spill.close().await;return Err(if let Err(cleanup)=cleanup{error.context(format!("TRANSFER_EFFECTS_UNKNOWN: spill初始化失败且清理未确认: {cleanup}"))}else{error});}Ok(spill)
    }
    fn connection(&mut self)->Result<&mut SqliteConnection>{self.connection.as_mut().ok_or_else(||anyhow!("spill已关闭"))}
    // 只对私有且最终删除的 scratch spill 分组提交，目标数据库事务完全不变。
    // journal=OFF 仍在自动提交时同步主文件，128行共享一次提交但不跳过封存fsync。
    async fn begin_write(&mut self)->Result<()> {
        if !self.write_transaction {crate::sql_history::query("BEGIN").execute(self.connection()?).await?;self.write_transaction=true;}Ok(())
    }
    async fn flush_writes(&mut self)->Result<()> {
        if self.write_transaction {crate::sql_history::query("COMMIT").execute(self.connection()?).await?;self.write_transaction=false;self.pending_writes=0;}Ok(())
    }
    async fn wrote(&mut self)->Result<()> {self.pending_writes+=1;if self.pending_writes>=128{self.flush_writes().await?;}Ok(())}
    fn charge(&mut self,bytes:usize)->Result<()>{self.bytes=self.bytes.checked_add(u64::try_from(bytes)?.checked_add(1024).ok_or_else(||anyhow!("spill长度溢出"))?).ok_or_else(||anyhow!("spill长度溢出"))?;if self.bytes>SPILL_BYTES {bail!("同步spill超过8GiB预算，目标未提交");}self.pin.as_ref().ok_or_else(||anyhow!("spill未拥有文件"))?.check()?;Ok(())}
    pub(super) async fn push_source(&mut self,cells:&[Cell],keys:&[Cell])->Result<()>{
        if self.source_seal.is_some(){bail!("已封存的源snapshot不可扩写");}let body=encode_cells(cells)?;let key=key(keys)?;self.charge(body.len().checked_add(key.len()).ok_or_else(||anyhow!("spill长度溢出"))?)?;
        let sequence=self.rows;let signature=mac(&self.secret,0,sequence,&key,&body);
        self.begin_write().await?;
        crate::sql_history::query("INSERT INTO source_rows(seq,k,body,mac) VALUES(?,?,?,?)").bind(i64::try_from(sequence)?).bind(&key).bind(&body).bind(signature.as_slice()).execute(self.connection()?).await.map_err(|error|anyhow!(error).context("源表键重复或spill写入失败，目标未修改"))?;
        self.wrote().await?;feed(&mut self.source_hash,sequence,&key,&body,&signature);self.rows=self.rows.checked_add(1).ok_or_else(||anyhow!("源行数溢出"))?;Ok(())
    }
    pub(super) async fn seal(&mut self)->Result<String>{self.flush_writes().await?;self.source_seal=Some(self.source_hash.clone().finalize().into());self.pin.as_ref().ok_or_else(||anyhow!("spill未拥有文件"))?.file.sync_all()?;self.verify_source().await?;Ok(format!("{:x}",self.source_hash.clone().finalize()))}
    pub(super) async fn source(&mut self,sequence:u64)->Result<(Vec<Cell>,Vec<u8>,Vec<u8>,Vec<u8>)>{
        self.pin.as_ref().ok_or_else(||anyhow!("spill未拥有文件"))?.check()?;
        let row=crate::sql_history::query("SELECT seq, CASE WHEN length(k)<=16777216 THEN k END AS k, CASE WHEN length(body)<=16777216 THEN body END AS body, CASE WHEN length(mac)=32 THEN mac END AS mac FROM source_rows WHERE seq=?").bind(i64::try_from(sequence)?).fetch_optional(self.connection()?).await?.ok_or_else(||anyhow!("源snapshot缺少认证记录，目标未提交"))?;
        let key:Vec<u8>=row.try_get("k")?;let body:Vec<u8>=row.try_get("body")?;let signature:Vec<u8>=row.try_get("mac")?;
        let expected=mac(&self.secret,0,sequence,&key,&body);if signature.len()!=32 || signature.iter().zip(expected).fold(0u8,|difference,(a,b)|difference|(*a^b))!=0{bail!("源snapshot认证失败，目标未提交");}
        Ok((decode_cells(&body)?,key,body,signature))
    }
    /// 验证阶段单行缓冲游标，保持每行文件身份/MAC/长度/序号/原生解码检查。
    /// 坏记录不提前丢弃已发送的 SELECT：排空后报告首个错误，不认证截断快照。
    async fn verify_records(&mut self, table:&str, domain:u8, rows:u64, expected:[u8;32])->Result<()> {
        self.flush_writes().await?;
        let body=if matches!(domain,0|3){"CASE WHEN length(body)<=16777216 THEN body END"}else{"X''"};
        let sql=format!("SELECT seq,CASE WHEN length(k)<=16777216 THEN k END AS k,{body} AS body,CASE WHEN length(mac)=32 THEN mac END AS mac FROM {table} ORDER BY seq");
        let pin=self.pin.as_ref().ok_or_else(||anyhow!("spill未拥有文件"))?;
        let connection=self.connection.as_mut().ok_or_else(||anyhow!("spill已关闭"))?;
        let mut stream=crate::sql_history::query(&sql).fetch(connection);let mut hash=Sha256::new();let mut sequence=0u64;let mut failure=None;
        while let Some(row)=stream.try_next().await? {
            if failure.is_some(){continue;}
            let result:Result<()>=async {
                pin.check()?;
                if sequence>=rows || row.try_get::<i64,_>("seq")?!=i64::try_from(sequence)? {bail!("spill总数/连续序号变化，目标未提交");}
                let key:Vec<u8>=row.try_get("k")?;let body:Vec<u8>=row.try_get("body")?;let signature:Vec<u8>=row.try_get("mac")?;
                let calculated=mac(&self.secret,domain,sequence,&key,&body);
                if signature.len()!=32||signature.iter().zip(calculated).fold(0u8,|difference,(a,b)|difference|(*a^b))!=0{bail!("spill认证失败，目标未提交");}
                if domain==0||domain==3{let _=decode_cells(&body)?;}if domain==3{let _=decode_cells(&key)?;}
                feed(&mut hash,sequence,&key,&body,&signature);sequence+=1;Ok(())
            }.await;
            if let Err(error)=result{failure=Some(error);}
        }
        drop(stream);if let Some(error)=failure{return Err(error);}pin.check()?;
        if sequence!=rows || <[u8;32]>::from(hash.finalize())!=expected {bail!("spill总数/摘要变化，目标未提交");}Ok(())
    }
    pub(super) async fn verify_source(&mut self)->Result<()>{
        let expected=self.source_seal.ok_or_else(||anyhow!("源snapshot未封存"))?;
        self.verify_records("source_rows",0,self.rows,expected).await
    }
    pub(super) async fn push_target_key(&mut self,cells:&[Cell])->Result<()>{
        let key=key(cells)?;self.charge(key.len())?;let sequence=self.target_rows;let signature=mac(&self.secret,1,sequence,&key,&[]);
        self.begin_write().await?;
        crate::sql_history::query("INSERT INTO target_keys(seq,k,mac) VALUES(?,?,?)").bind(i64::try_from(sequence)?).bind(&key).bind(signature.as_slice()).execute(self.connection()?).await.map_err(|error|anyhow!(error).context("目标表键重复或spill写入失败，目标未提交"))?;
        self.wrote().await?;feed(&mut self.target_hash,sequence,&key,&[],&signature);self.target_rows=self.target_rows.checked_add(1).ok_or_else(||anyhow!("目标键行数溢出"))?;Ok(())
    }
    pub(super) async fn verify_target_keys(&mut self)->Result<()>{
        self.verify_records("target_keys",1,self.target_rows,self.target_hash.clone().finalize().into()).await
    }
    pub(super) async fn configure_probe(&mut self,affinities:&[&str])->Result<()>{
        if self.probe_columns!=0 || affinities.is_empty() || affinities.len()>MAX_COLUMNS{bail!("SQLite类型预检定义无效或重复配置");}
        if affinities.iter().any(|affinity|!["INTEGER","REAL","TEXT","NUMERIC","BLOB"].contains(affinity)){bail!("SQLite类型预检affinity未经白名单验证");}
        let columns=affinities.iter().enumerate().map(|(index,affinity)|format!("c{index} {affinity}")).collect::<Vec<_>>().join(", ");
        crate::sql_history::query(&format!("CREATE TEMP TABLE typed_probe({columns})")).execute(self.connection()?).await?;
        let names=(0..affinities.len()).map(|index|format!("c{index}")).collect::<Vec<_>>().join(", ");
        // 私有 TEMP 表无触发器/约束；固定 rowid 只保留一行，RETURNING 读取真实 SQLite affinity。
        self.probe_sql=format!("INSERT OR REPLACE INTO temp.typed_probe(rowid, {names}) VALUES (1, {}) RETURNING {names}",vec!["?";affinities.len()].join(", "));
        self.probe_columns=affinities.len();Ok(())
    }
    pub(super) async fn probe(&mut self,cells:&[Cell])->Result<Vec<Cell>>{
        if cells.len()!=self.probe_columns || self.probe_columns==0{bail!("SQLite类型预检列数不符");}let _=encode_cells(cells)?;
        let sql=&self.probe_sql;
        let connection=self.connection.as_mut().ok_or_else(||anyhow!("spill已关闭"))?;
        let rows=bind(crate::sql_history::query(sql),cells).fetch_all(connection).await?;
        if rows.len()!=1{bail!("SQLite类型探针并非唯一1行");}decode_row(&rows[0])
    }
    pub(super) async fn push_mapped_key(&mut self,sequence:u64,cells:&[Cell])->Result<()>{
        if sequence!=self.mapped_rows || sequence>=self.rows{bail!("目标映射键预检顺序不符");}let key=key(cells)?;self.charge(key.len())?;let signature=mac(&self.secret,2,sequence,&key,&[]);
        self.begin_write().await?;
        crate::sql_history::query("INSERT INTO mapped_keys(seq,k,mac) VALUES(?,?,?)").bind(i64::try_from(sequence)?).bind(&key).bind(signature.as_slice()).execute(self.connection()?).await.map_err(|error|anyhow!(error).context("源键经目标类型转换后发生碰撞，目标未提交"))?;
        self.wrote().await?;feed(&mut self.mapped_hash,sequence,&key,&[],&signature);self.mapped_rows+=1;Ok(())
    }
    pub(super) async fn verify_mapped_keys(&mut self)->Result<()>{
        if self.mapped_rows!=self.rows{bail!("目标映射键预检未覆盖完整源快照");}
        self.verify_records("mapped_keys",2,self.mapped_rows,self.mapped_hash.clone().finalize().into()).await
    }
    pub(super) async fn push_verified(&mut self,keys:&[Cell],cells:&[Cell])->Result<()>{
        let key=encode_cells(keys)?;let body=encode_cells(cells)?;self.charge(key.len().checked_add(body.len()).ok_or_else(||anyhow!("spill长度溢出"))?)?;
        let sequence=self.verified_rows;let signature=mac(&self.secret,3,sequence,&key,&body);
        self.begin_write().await?;
        crate::sql_history::query("INSERT INTO verified_rows(seq,k,body,mac) VALUES(?,?,?,?)").bind(i64::try_from(sequence)?).bind(&key).bind(&body).bind(signature.as_slice()).execute(self.connection()?).await?;
        self.wrote().await?;feed(&mut self.verified_hash,sequence,&key,&body,&signature);self.verified_rows=self.verified_rows.checked_add(1).ok_or_else(||anyhow!("读回行数溢出"))?;Ok(())
    }
    pub(super) async fn verified(&mut self,sequence:u64)->Result<(Vec<Cell>,Vec<Cell>)>{
        self.pin.as_ref().ok_or_else(||anyhow!("spill未拥有文件"))?.check()?;
        let row=crate::sql_history::query("SELECT CASE WHEN length(k)<=16777216 THEN k END AS k,CASE WHEN length(body)<=16777216 THEN body END AS body,CASE WHEN length(mac)=32 THEN mac END AS mac FROM verified_rows WHERE seq=?").bind(i64::try_from(sequence)?).fetch_one(self.connection()?).await?;
        let key:Vec<u8>=row.try_get("k")?;let body:Vec<u8>=row.try_get("body")?;let signature:Vec<u8>=row.try_get("mac")?;let expected=mac(&self.secret,3,sequence,&key,&body);
        if signature.len()!=32||signature.iter().zip(expected).fold(0u8,|difference,(a,b)|difference|(*a^b))!=0{bail!("目标最终读回凭据认证失败");}Ok((decode_cells(&key)?,decode_cells(&body)?))
    }
    pub(super) async fn verify_receipts(&mut self)->Result<()>{
        self.verify_records("verified_rows",3,self.verified_rows,self.verified_hash.clone().finalize().into()).await
    }
    #[cfg(test)] pub(super) fn test_path(&self)->PathBuf{self.pin.as_ref().expect("spill owns file").path.clone()}
    #[cfg(test)] pub(super) fn test_charge(&mut self,bytes:usize)->Result<()>{self.charge(bytes)}
    #[cfg(test)] pub(super) fn test_bytes(&self)->u64{self.bytes}
    pub(super) async fn close(&mut self)->Result<()>{
        let flush=self.flush_writes().await;
        if let Some(connection)=self.connection.take(){connection.close().await.context("同步spill连接未确认关闭")?;}
        flush.context("同步spill未确认结束私有写事务")?;
        self.secret.zeroize();if let Some(pin)=self.pin.take(){pin.check()?;let path=pin.path.clone();let id=pin.id;drop(pin);if file_id(&File::open(&path)?)?!=id{bail!("spill路径被替换，未删除外部文件");}std::fs::remove_file(path).context("同步临时数据清理失败")?;}Ok(())
    }
}
impl Drop for Spill{fn drop(&mut self){self.secret.zeroize(); /* 不能在Drop中丢弃/超时在途SQL；工作流程必须显式close().await。 */}}
