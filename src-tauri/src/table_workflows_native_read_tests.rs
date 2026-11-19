//! STORY-ER-LOAD-001：真实 SQLx 驱动 + 显式 loopback 协议模拟，不连接厂商或用户数据库。
//! 沿用 execution_stream_mysql_tests 的握手/预处理/二进制行机制，记录实际执行 SQL 和会话生命周期。
use super::*;
use std::{collections::{BTreeSet,HashMap}, io, sync::atomic::{AtomicBool,AtomicUsize,Ordering}};
use tokio::{io::{AsyncReadExt,AsyncWriteExt}, net::{TcpListener,TcpStream}, sync::Notify};

#[derive(Clone)]
struct CatalogError{code:u16,state:[u8;5],message:String}
impl CatalogError{
    fn denied()->Self{Self{code:1142,state:*b"42000",message:"fixture catalog permission denied".into()}}
    fn syntax()->Self{Self{code:1064,state:*b"42000",message:"fixture catalog syntax error".into()}}
    fn missing()->Self{Self{code:1146,state:*b"42S02",message:"fixture relation does not exist".into()}}
}
#[derive(Clone)]
struct Catalog{
    version:String,uuid:bool,server_id:bool,table_ids:Vec<Option<String>>,omit_table_id:bool,
    grants:Vec<String>,errors:BTreeMap<String,CatalogError>,innodb_missing:bool,innodb_empty:bool,
}
impl Catalog{
    fn tidb()->Self{Self{version:"5.7.25-TiDB-v8.5.0 TiDB Server".into(),uuid:true,server_id:true,
        table_ids:vec![Some("128".into())],omit_table_id:false,grants:vec!["GRANT ALL PRIVILEGES ON *.* TO 'fixture'@'localhost'".into()],
        errors:BTreeMap::new(),innodb_missing:false,innodb_empty:false}}
    fn mysql()->Self{Self{version:"8.0.36 MySQL Community Server".into(),..Self::tidb()}}
    fn mariadb()->Self{Self{version:"10.11.8-MariaDB MariaDB Server".into(),uuid:false,innodb_missing:true,..Self::tidb()}}
}
#[derive(Clone)]
enum Reply{Ok,Rows{columns:Vec<String>,rows:Vec<Vec<Option<String>>>},Error(CatalogError)}
fn records(columns:&[&str],rows:Vec<Record>)->Reply{Reply::Rows{columns:columns.iter().map(|s|(*s).into()).collect(),rows:rows.into_iter().map(|r|columns.iter().map(|k|r.get(*k).cloned().flatten()).collect()).collect()}}
fn record(fields:&[(&str,Option<&str>)])->Record{fields.iter().map(|(k,v)|((*k).into(),v.map(str::to_string))).collect()}
fn scalar(value:&str)->Reply{records(&["value"],vec![record(&[("value",Some(value))])])}
fn reply(sql:&str,catalog:&Catalog,mode:&str)->Reply{
    if let Some((_,e))=catalog.errors.iter().find(|(needle,_)|sql.contains(needle.as_str())){return Reply::Error(e.clone());}
    if sql.contains("@@version"){return scalar(&catalog.version);}
    if sql=="SELECT @@session.sql_mode"{return scalar(mode);}
    if sql=="SELECT @@session.autocommit"{return scalar("1");}
    if sql=="SHOW VARIABLES LIKE 'server_uuid'"||sql=="SHOW VARIABLES LIKE 'server_id'"{
        let uuid=sql.contains("server_uuid");let present=if uuid{catalog.uuid}else{catalog.server_id};
        return records(&["Variable_name","Value"],if present{vec![record(&[("Variable_name",Some(if uuid{"server_uuid"}else{"server_id"})),("Value",Some("fixture-server"))])]}else{vec![]});
    }
    if sql.contains("AS server, @@hostname"){
        return records(&["server","host","port","db","mode","names","charset_client","collation_connection"],vec![record(&[
            ("server",Some(if catalog.uuid{"fixture-uuid"}else if catalog.server_id{"1"}else{""})),("host",Some("fixture")),("port",Some("3306")),
            ("db",Some("fixture")),("mode",Some(mode)),("names",Some("0")),("charset_client",Some("utf8mb4")),("collation_connection",Some("utf8mb4_bin")),
        ])]);
    }
    if sql=="SHOW GRANTS"{return records(&["Grants"],catalog.grants.iter().map(|g|record(&[("Grants",Some(g))])).collect());}
    if sql.contains("FROM information_schema.TABLES"){
        let with_id=sql.contains("TIDB_TABLE_ID")&&!catalog.omit_table_id;
        let mut columns=vec!["kind","engine","collation","row_format","extra","comment","created"];
        if with_id{columns.push("id");}
        let rows=catalog.table_ids.iter().map(|id|{
            let mut row=record(&[("kind",Some("BASE TABLE")),("engine",Some("InnoDB")),("collation",Some("utf8mb4_bin")),("row_format",Some("DYNAMIC")),
                ("extra",Some("")),("comment",Some("fixture table")),("created",Some("2026-11-18 01:02:03"))]);
            if with_id{row.insert("id".into(),id.clone());}row
        }).collect();
        return records(&columns,rows);
    }
    if sql.starts_with("SHOW CREATE TABLE "){
        return records(&["Table","Create Table"],vec![record(&[("Table",Some("sample")),("Create Table",Some("CREATE TABLE `sample` (`id` bigint NOT NULL, PRIMARY KEY (`id`)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"))])]);
    }
    if sql.contains("FROM information_schema.COLUMNS"){
        return records(&["name","type","nullable","default","auto","comment","extra","generated","collation","ordinal"],vec![record(&[
            ("name",Some("id")),("type",Some("bigint")),("nullable",Some("0")),("default",None),("auto",Some("0")),("comment",Some("")),
            ("extra",Some("")),("generated",Some("")),("collation",None),("ordinal",Some("1")),
        ])]);
    }
    if sql.starts_with("SHOW INDEX FROM "){
        return records(&["Key_name","Seq_in_index","Column_name","Non_unique","Index_type","Expression","Cardinality"],vec![record(&[
            ("Key_name",Some("PRIMARY")),("Seq_in_index",Some("1")),("Column_name",Some("id")),("Non_unique",Some("0")),("Index_type",Some("BTREE")),("Expression",None),("Cardinality",Some("1")),
        ])]);
    }
    if sql.contains("information_schema.KEY_COLUMN_USAGE"){return records(&["name","col","ref_table","ref_schema","ref_col","on_update","on_delete"],vec![]);}
    if sql.contains("information_schema.TRIGGERS"){return records(&["name","timing","event","body","orientation","ordering","mode","definer","charset","collation"],vec![]);}
    if sql.contains("information_schema.INNODB_TABLES")&&catalog.innodb_missing{return Reply::Error(CatalogError::missing());}
    if sql.contains("information_schema.INNODB_TABLES")||sql.contains("information_schema.INNODB_SYS_TABLES"){
        return records(&["id"],if catalog.innodb_empty{vec![]}else{vec![record(&[("id",Some("64"))])]});
    }
    if sql.starts_with("SELECT * FROM ")&&sql.ends_with(" LIMIT 0"){return records(&["id"],vec![]);}
    if sql.contains("CONNECTION_ID()"){return scalar("7");}
    if sql.contains("RELEASE_LOCK("){return scalar("1");}
    if sql.starts_with("SET ")||sql.starts_with("START TRANSACTION")||sql=="ROLLBACK"||sql=="UNLOCK TABLES"{return Reply::Ok;}
    Reply::Error(CatalogError{code:1064,state:*b"42000",message:format!("unhandled fixture SQL: {sql}")})
}

struct Block{sql:&'static str,reached:AtomicBool,arrived:Notify,release:Notify}
impl Block{
    fn new(sql:&'static str)->Arc<Self>{Arc::new(Self{sql,reached:AtomicBool::new(false),arrived:Notify::new(),release:Notify::new()})}
    async fn wait(&self){tokio::time::timeout(Duration::from_secs(3),async{while !self.reached.load(Ordering::Acquire){self.arrived.notified().await;}}).await.expect("fixture gate must be reached");}
    fn arrive(&self){self.reached.store(true,Ordering::Release);self.arrived.notify_one();}
}
struct Audit{
    catalog:parking_lot::Mutex<Catalog>,sql:parking_lot::Mutex<Vec<(usize,String)>>,
    next_session:AtomicUsize,open:parking_lot::Mutex<BTreeSet<usize>>,closed:Notify,
    block:Option<Arc<Block>>,return_block:Arc<Block>,hold_return:AtomicBool,
}
struct LiveSession{audit:Arc<Audit>,id:usize}
impl Drop for LiveSession{fn drop(&mut self){self.audit.open.lock().remove(&self.id);self.audit.closed.notify_one();}}
impl Audit{
    fn count(&self,needle:&str)->usize{self.sql.lock().iter().filter(|(_,sql)|sql.contains(needle)).count()}
    async fn wait_closed(&self,id:usize){tokio::time::timeout(Duration::from_secs(2),async{while self.open.lock().contains(&id){self.closed.notified().await;}}).await.expect("the original physical socket must close");}
}
struct Fixture{db:Arc<Db>,pool:sqlx::MySqlPool,audit:Arc<Audit>,server:tokio::task::JoinHandle<()>}
impl Fixture{
    async fn new(catalog:Catalog,block:Option<Arc<Block>>)->Self{
        let audit=Arc::new(Audit{catalog:parking_lot::Mutex::new(catalog),sql:parking_lot::Mutex::new(vec![]),next_session:AtomicUsize::new(0),
            open:parking_lot::Mutex::new(BTreeSet::new()),closed:Notify::new(),block,return_block:Block::new("return_to_pool"),hold_return:AtomicBool::new(false)});
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let accepted=audit.clone();
        let server=tokio::spawn(async move{let mut tasks=tokio::task::JoinSet::new();loop{tokio::select!{
            r=listener.accept()=>{let (stream,_)=r.unwrap();let audit=accepted.clone();let id=audit.next_session.fetch_add(1,Ordering::SeqCst)+1;audit.open.lock().insert(id);
                tasks.spawn(async move{let _live=LiveSession{audit:audit.clone(),id};let _=serve(stream,audit,id).await;});},
            _=tasks.join_next(),if !tasks.is_empty()=>{},
        }}});
        let hook=audit.clone();
        let pool=sqlx::mysql::MySqlPoolOptions::new().max_connections(1).acquire_timeout(Duration::from_secs(3))
            .after_release(move |_,_|{let audit=hook.clone();Box::pin(async move{if audit.hold_return.load(Ordering::Acquire){audit.return_block.arrive();audit.return_block.release.notified().await;}Ok(true)})})
            .connect_with(sqlx::mysql::MySqlConnectOptions::new().host("127.0.0.1").port(port).username("fixture").database("fixture").ssl_mode(sqlx::mysql::MySqlSslMode::Disabled))
            .await.expect("real SQLx driver must connect to the owned loopback fixture");
        Self{db:Arc::new(Db::Mysql(pool.clone())),pool,audit,server}
    }
    async fn finish(self){tokio::time::timeout(Duration::from_secs(3),self.pool.close()).await.expect("fixture pool must close");self.server.abort();}
    async fn read(&self,manual:&crate::manual_transaction::Registry,budget:ReadBudget)->CmdResult<Stored>{let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();read_owned_with_budget(self.db.clone(),binding(),"sample".into(),false,lease,budget).await}
}
impl Drop for Fixture{fn drop(&mut self){self.server.abort();}}
fn binding()->Binding{Binding{principal:"isolated-wire".into(),owner:"fixture-owner".into(),connection:"fixture-connection".into(),namespace:"fixture".into(),fingerprint:"fixture-epoch".into()}}
fn budget()->ReadBudget{ReadBudget{acquire:Duration::from_secs(2),read:Duration::from_secs(2),cleanup:Duration::from_secs(2)}}
async fn available(manual:&crate::manual_transaction::Registry)->bool{manual.try_ordinary(&["fixture-connection"]).await.unwrap().is_some()}

// 最小 MySQL 4.1 协议：所有目录列用 VAR_STRING，覆盖真实 prepare/execute 与 NULL 解码。
fn lenenc(out:&mut Vec<u8>,bytes:&[u8]){let n=bytes.len();if n<251{out.push(n as u8);}else{out.push(0xfc);out.extend_from_slice(&(n as u16).to_le_bytes());}out.extend_from_slice(bytes);}
fn handshake()->Vec<u8>{let caps=0x0000_0001u32|0x0000_0002|0x0000_0004|0x0000_0008|0x0000_0200|0x0000_2000|0x0000_8000|0x0002_0000|0x0004_0000|0x0008_0000;
    let mut out=vec![10];out.extend_from_slice(b"8.0.36\0");out.extend_from_slice(&7u32.to_le_bytes());out.extend_from_slice(b"abcdefgh");out.push(0);out.extend_from_slice(&(caps as u16).to_le_bytes());out.push(33);out.extend_from_slice(&2u16.to_le_bytes());out.extend_from_slice(&((caps>>16)as u16).to_le_bytes());out.push(21);out.extend_from_slice(&[0;10]);out.extend_from_slice(b"ijklmnopqrst\0mysql_native_password\0");out}
fn column(name:&str)->Vec<u8>{let mut out=Vec::new();for value in ["def","fixture","sample","sample",name,name]{lenenc(&mut out,value.as_bytes());}out.push(12);out.extend_from_slice(&33u16.to_le_bytes());out.extend_from_slice(&4096u32.to_le_bytes());out.push(0xfd);out.extend_from_slice(&[0;5]);out}
fn ok()->Vec<u8>{vec![0,0,0,2,0,0,0]}
fn eof()->Vec<u8>{vec![0xfe,0,0,2,0]}
fn error(e:&CatalogError)->Vec<u8>{let mut out=vec![0xff];out.extend_from_slice(&e.code.to_le_bytes());out.push(b'#');out.extend_from_slice(&e.state);out.extend_from_slice(e.message.as_bytes());out}
async fn packet(stream:&mut TcpStream,sequence:u8,bytes:&[u8])->io::Result<()>{let n=bytes.len();let mut out=vec![n as u8,(n>>8)as u8,(n>>16)as u8,sequence];out.extend_from_slice(bytes);stream.write_all(&out).await}
async fn receive(stream:&mut TcpStream)->io::Result<(u8,Vec<u8>)>{let mut header=[0;4];stream.read_exact(&mut header).await?;let n=header[0]as usize|((header[1]as usize)<<8)|((header[2]as usize)<<16);let mut bytes=vec![0;n];stream.read_exact(&mut bytes).await?;Ok((header[3],bytes))}
async fn send_reply(stream:&mut TcpStream,sequence:u8,reply:Reply,binary:bool)->io::Result<()>{
    let mut packets=Vec::new();match reply{
        Reply::Ok=>packets.push(ok()),Reply::Error(e)=>packets.push(error(&e)),Reply::Rows{columns,rows}=>{
            packets.push(vec![columns.len()as u8]);for name in &columns{packets.push(column(name));}packets.push(eof());
            for values in rows{let mut row=if binary{vec![0;1+(columns.len()+9)/8]}else{vec![]};for (i,value) in values.into_iter().enumerate(){
                match value{Some(s)=>lenenc(&mut row,s.as_bytes()),None if binary=>row[1+(i+2)/8]|=1<<((i+2)%8),None=>row.push(0xfb)}
            }packets.push(row);}packets.push(eof());
        }
    }
    for (i,bytes) in packets.iter().enumerate(){packet(stream,sequence.wrapping_add(i as u8),bytes).await?;}Ok(())
}
async fn executed(stream:&mut TcpStream,audit:&Audit,id:usize,sql:&str,mode:&mut String,sequence:u8,binary:bool)->io::Result<()>{
    audit.sql.lock().push((id,sql.into()));
    if let Some(block)=audit.block.as_ref().filter(|b|sql.contains(b.sql)){
        block.arrive();let mut byte=[0];tokio::select!{_=block.release.notified()=>{},r=stream.peek(&mut byte)=>{let _=r?;return Err(io::Error::new(io::ErrorKind::ConnectionAborted,"owned connection destroyed while fixture response held"));}}
    }
    let response=reply(sql,&audit.catalog.lock(),mode);
    if matches!(response,Reply::Ok){if let Some(value)=sql.strip_prefix("SET SESSION sql_mode='").and_then(|s|s.strip_suffix('\'')){*mode=value.into();}}
    send_reply(stream,sequence,response,binary).await
}
async fn serve(mut stream:TcpStream,audit:Arc<Audit>,id:usize)->io::Result<()>{
    stream.set_nodelay(true)?;packet(&mut stream,0,&handshake()).await?;let (seq,_)=receive(&mut stream).await?;packet(&mut stream,seq.wrapping_add(1),&ok()).await?;
    let mut mode="STRICT_TRANS_TABLES".to_string();let mut statements=HashMap::<u32,String>::new();let mut next=1u32;
    loop{let (seq,bytes)=match receive(&mut stream).await{Ok(r)=>r,Err(_)=>return Ok(())};let sequence=seq.wrapping_add(1);match bytes.first().copied(){
        Some(1)|None=>return Ok(()),Some(0x0e)=>packet(&mut stream,sequence,&ok()).await?,
        Some(3)=>{let sql=String::from_utf8_lossy(&bytes[1..]);executed(&mut stream,&audit,id,&sql,&mut mode,sequence,false).await?;},
        Some(0x16)=>{let sql=String::from_utf8_lossy(&bytes[1..]).into_owned();let response=reply(&sql,&audit.catalog.lock(),&mode);
            if let Reply::Error(e)=response{packet(&mut stream,sequence,&error(&e)).await?;continue;}
            let columns=match response{Reply::Rows{columns,..}=>columns,_=>vec![]};let statement_id=next;next+=1;statements.insert(statement_id,sql);
            let mut prepared=vec![0];prepared.extend_from_slice(&statement_id.to_le_bytes());prepared.extend_from_slice(&(columns.len()as u16).to_le_bytes());prepared.extend_from_slice(&[0;5]);packet(&mut stream,sequence,&prepared).await?;
            let mut sequence=sequence.wrapping_add(1);for name in &columns{packet(&mut stream,sequence,&column(name)).await?;sequence=sequence.wrapping_add(1);}if !columns.is_empty(){packet(&mut stream,sequence,&eof()).await?;}
        },
        Some(0x17)=>{let statement_id=u32::from_le_bytes(bytes[1..5].try_into().unwrap());let sql=statements.get(&statement_id).unwrap().clone();executed(&mut stream,&audit,id,&sql,&mut mode,sequence,true).await?;},
        Some(0x19)=>{let statement_id=u32::from_le_bytes(bytes[1..5].try_into().unwrap());statements.remove(&statement_id);},
        Some(2)=>packet(&mut stream,sequence,&ok()).await?,
        _=>return Err(io::Error::new(io::ErrorKind::InvalidData,"unsupported fixture command")),
    }}
}

#[tokio::test]
async fn tidb_owned_read_skips_grants_triggers_and_duplicate_table_identity_query(){
    let mut catalog=Catalog::tidb();catalog.errors.insert("SHOW GRANTS".into(),CatalogError::denied());catalog.errors.insert("information_schema.TRIGGERS".into(),CatalogError::syntax());
    let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let result=f.read(&manual,budget()).await.unwrap();
    assert_eq!(result.public.completeness,"complete");assert_eq!(result.public.schema.columns.len(),1);assert!(result.public.objects.triggers.is_empty());
    assert!(!result.public.capabilities.rebuild.supported);assert!(result.public.capabilities.rebuild.reason.as_deref().unwrap().contains("TiDB"));
    assert_eq!(f.audit.count("SHOW GRANTS"),0);assert_eq!(f.audit.count("information_schema.TRIGGERS"),0);assert_eq!(f.audit.count("SHOW CREATE TRIGGER"),0);
    assert_eq!(f.audit.count("@@version"),1);assert_eq!(f.audit.count("SHOW VARIABLES LIKE 'server_uuid'"),1);assert_eq!(f.audit.count("FROM information_schema.TABLES"),1);
    assert_eq!(f.audit.count("TIDB_TABLE_ID"),1);assert_eq!(f.audit.count("information_schema.INNODB"),0);assert_eq!(f.audit.count("START TRANSACTION READ ONLY"),0);
    let sql=f.audit.sql.lock().clone();assert!(sql.iter().all(|(session,_)|*session==1));
    assert!(sql.iter().position(|(_,sql)|sql=="ROLLBACK").unwrap()<sql.iter().position(|(_,sql)|sql=="SET SESSION sql_mode='STRICT_TRANS_TABLES'").unwrap());
    assert!(available(&manual).await);assert_eq!(f.pool.size(),1);f.finish().await;
}

#[tokio::test]
async fn mysql_evidence_is_reused_per_owned_session_but_not_across_new_sessions(){
    let mut catalog=Catalog::tidb();catalog.uuid=false;let f=Fixture::new(catalog,None).await;let mut s=Session::acquire(&f.db).await.unwrap();s.setup().await.unwrap();s.begin_read().await.unwrap();
    let first=snapshot(&mut s,&binding(),"sample",false).await.unwrap();let second=snapshot(&mut s,&binding(),"sample",false).await.unwrap();
    assert_eq!(first.public.revision,second.public.revision);assert_eq!(f.audit.count("@@version"),1);assert_eq!(f.audit.count("SHOW VARIABLES LIKE 'server_uuid'"),1);assert_eq!(f.audit.count("SHOW VARIABLES LIKE 'server_id'"),1);
    s.execute("ROLLBACK").await.unwrap();s.release(false).await.unwrap();
    *f.audit.catalog.lock()=Catalog::mysql();let mut s=Session::acquire(&f.db).await.unwrap();s.setup().await.unwrap();s.begin_read().await.unwrap();let read=snapshot(&mut s,&binding(),"sample",false).await.unwrap();
    assert!(read.public.capabilities.rebuild.supported);assert_eq!(f.audit.count("@@version"),2);assert_eq!(f.audit.count("SHOW VARIABLES LIKE 'server_uuid'"),2);assert_eq!(f.audit.count("SHOW GRANTS"),1);assert_eq!(f.audit.count("information_schema.TRIGGERS"),1);
    assert_eq!(f.audit.next_session.load(Ordering::Acquire),1,"a new Session must reset evidence even when the pool reuses its physical connection");
    s.execute("ROLLBACK").await.unwrap();s.release(false).await.unwrap();f.finish().await;
}

#[tokio::test]
async fn merged_tidb_id_keeps_definition_base_and_changes_physical_identity(){
    let f=Fixture::new(Catalog::tidb(),None).await;let mut s=Session::acquire(&f.db).await.unwrap();s.setup().await.unwrap();s.begin_read().await.unwrap();
    let first=snapshot(&mut s,&binding(),"sample",false).await.unwrap();f.audit.catalog.lock().table_ids=vec![Some("129".into())];let second=snapshot(&mut s,&binding(),"sample",false).await.unwrap();
    assert_eq!(first.base,second.base);assert_ne!(first.public.revision,second.public.revision);assert_ne!(first.native.as_ref().unwrap().identity,second.native.as_ref().unwrap().identity);
    assert_eq!(first.native.as_ref().unwrap().physical,second.native.as_ref().unwrap().physical);assert!(!second.native.as_ref().unwrap().table.contains_key("id"));
    s.execute("ROLLBACK").await.unwrap();s.release(false).await.unwrap();f.finish().await;
}

#[tokio::test]
async fn tidb_merged_id_requires_one_non_null_positive_u64(){
    for ids in [vec![],vec![None],vec![Some("".into())],vec![Some("0".into())],vec![Some("-1".into())],vec![Some("abc".into())],vec![Some("18446744073709551616".into())],vec![Some("1".into()),Some("2".into())]]{
        let mut catalog=Catalog::tidb();catalog.table_ids=ids.clone();let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let error=f.read(&manual,budget()).await.err().expect("invalid physical identity cannot publish a snapshot");
        assert!(error.message.contains("TABLE_MYSQL_PHYSICAL_IDENTITY"),"{ids:?}: {error:?}");assert_eq!(f.audit.count("FROM information_schema.TABLES"),1);assert_eq!(f.audit.count("SHOW CREATE TABLE"),0);assert!(available(&manual).await);f.finish().await;
    }
    let mut catalog=Catalog::tidb();catalog.omit_table_id=true;let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let error=f.read(&manual,budget()).await.err().unwrap();assert!(error.message.contains("TABLE_METADATA_FIELD_MISSING: id"));f.finish().await;
}

#[tokio::test]
async fn mysql_and_mariadb_keep_direct_trigger_permission_gate(){
    for catalog in [Catalog::mysql(),Catalog::mariadb()]{
        for grants in [vec![],vec!["GRANT SELECT ON *.* TO 'fixture'@'localhost'".into()],vec!["GRANT TRIGGER ON `other`.* TO 'fixture'@'localhost'".into()],vec!["GRANT `trigger_role` TO 'fixture'@'localhost'".into()]]{
            let mut catalog=catalog.clone();catalog.grants=grants;let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let error=f.read(&manual,budget()).await.err().unwrap();
            assert!(error.message.contains("TABLE_TRIGGER_VISIBILITY_UNPROVEN"),"{error:?}");assert_eq!(f.audit.count("SHOW GRANTS"),1);assert_eq!(f.audit.count("information_schema.TRIGGERS"),0);assert_eq!(f.audit.count("SHOW CREATE TABLE"),0);f.finish().await;
        }
    }
}

#[tokio::test]
async fn native_grants_and_trigger_permission_or_syntax_errors_are_not_empty_catalogs(){
    for needle in ["SHOW GRANTS","information_schema.TRIGGERS"]{for error in [CatalogError::denied(),CatalogError::syntax()]{
        let mut catalog=Catalog::mysql();catalog.errors.insert(needle.into(),error.clone());let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let actual=f.read(&manual,budget()).await.err().expect("catalog failure must not publish an empty complete snapshot");
        assert!(actual.message.contains(&error.message),"{actual:?}");assert!(available(&manual).await);f.finish().await;
    }}
}

#[tokio::test]
async fn mariadb_innodb_missing_relation_falls_back_but_permissions_and_syntax_do_not(){
    let f=Fixture::new(Catalog::mariadb(),None).await;let manual=crate::manual_transaction::Registry::default();assert!(f.read(&manual,budget()).await.is_ok());
    assert_eq!(f.audit.count("information_schema.INNODB_SYS_TABLES"),1);assert_eq!(f.audit.count("TIDB_TABLE_ID"),0);f.finish().await;
    for error in [CatalogError::denied(),CatalogError::syntax()]{let mut catalog=Catalog::mariadb();catalog.errors.insert("information_schema.INNODB_TABLES".into(),error.clone());let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let actual=f.read(&manual,budget()).await.err().unwrap();assert!(actual.message.contains(&error.message));assert_eq!(f.audit.count("information_schema.INNODB_SYS_TABLES"),0);f.finish().await;}
}

#[tokio::test]
async fn mysql_still_requires_real_innodb_id_while_compatibility_fallback_is_explicit(){
    let mut catalog=Catalog::mysql();catalog.innodb_empty=true;let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let error=f.read(&manual,budget()).await.err().unwrap();assert!(error.message.contains("TABLE_MYSQL_PHYSICAL_IDENTITY_UNPROVEN"));f.finish().await;
    let mut catalog=Catalog::mariadb();catalog.innodb_empty=true;catalog.server_id=false;let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();let read=f.read(&manual,budget()).await.unwrap();assert!(read.public.capabilities.limitations.iter().any(|v|v.contains("未提供可证明的物理表 ID")));f.finish().await;
}

#[tokio::test]
async fn ordinary_lease_is_held_through_rollback_and_sql_mode_restore(){
    let gate=Block::new("ROLLBACK");let f=Fixture::new(Catalog::tidb(),Some(gate.clone())).await;let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();
    let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget()));gate.wait().await;
    assert!(!task.is_finished());assert!(!available(&manual).await);assert_eq!(f.audit.count("SET SESSION sql_mode='STRICT_TRANS_TABLES'"),0);
    gate.release.notify_one();let result=task.await.unwrap().unwrap();assert_eq!(result.public.completeness,"complete");assert!(available(&manual).await);assert_eq!(f.pool.size(),1);f.finish().await;
}

#[tokio::test]
async fn read_budget_destroys_the_owned_session_without_publishing_or_unbounded_rollback(){
    let gate=Block::new("SHOW CREATE TABLE");let f=Fixture::new(Catalog::tidb(),Some(gate.clone())).await;let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();let mut budget=budget();budget.read=Duration::from_millis(150);
    let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget));gate.wait().await;assert!(!available(&manual).await);
    let error=tokio::time::timeout(Duration::from_secs(2),task).await.unwrap().unwrap().err().unwrap();assert_eq!(error.code,"EXECUTION_EFFECTS_UNKNOWN");assert!(error.message.contains("TABLE_METADATA_READ_TIMEOUT"));
    assert_eq!(f.pool.size(),0,"the original raw connection must be removed before the lease can be released");assert!(available(&manual).await);f.audit.wait_closed(1).await;assert_eq!(f.audit.count("ROLLBACK"),0);f.finish().await;
}

#[tokio::test]
async fn cleanup_budget_destroys_the_session_and_discards_the_already_read_snapshot(){
    let gate=Block::new("ROLLBACK");let f=Fixture::new(Catalog::tidb(),Some(gate.clone())).await;let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();let mut budget=budget();budget.cleanup=Duration::from_millis(150);
    let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget));gate.wait().await;assert!(!available(&manual).await);
    let error=task.await.unwrap().err().unwrap();assert_eq!(error.code,"EXECUTION_EFFECTS_UNKNOWN");assert!(error.message.contains("TABLE_METADATA_CLEANUP_TIMEOUT"));assert_eq!(f.pool.size(),0);assert!(available(&manual).await);f.audit.wait_closed(1).await;
    assert_eq!(f.audit.count("SET SESSION sql_mode='STRICT_TRANS_TABLES'"),0);f.finish().await;
}

#[tokio::test]
async fn restore_failure_is_unknown_and_never_returns_the_modified_mode_to_the_pool(){
    let mut catalog=Catalog::tidb();catalog.errors.insert("SET SESSION sql_mode='STRICT_TRANS_TABLES'".into(),CatalogError::denied());let f=Fixture::new(catalog,None).await;let manual=crate::manual_transaction::Registry::default();
    let error=f.read(&manual,budget()).await.err().unwrap();assert_eq!(error.code,"EXECUTION_EFFECTS_UNKNOWN");assert!(error.message.contains("TABLE_METADATA_SQL_MODE_UNKNOWN"));assert_eq!(f.pool.size(),0);assert!(available(&manual).await);f.audit.wait_closed(1).await;f.finish().await;
}

#[tokio::test]
async fn pool_return_budget_holds_the_lease_until_floating_raw_connection_is_dropped(){
    let f=Fixture::new(Catalog::tidb(),None).await;f.audit.hold_return.store(true,Ordering::Release);let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();let mut budget=budget();budget.cleanup=Duration::from_millis(150);
    let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget));f.audit.return_block.wait().await;
    assert_eq!(f.audit.count("SET SESSION sql_mode='STRICT_TRANS_TABLES'"),1);assert!(!available(&manual).await);assert_eq!(f.pool.size(),1);
    let error=task.await.unwrap().err().unwrap();assert!(error.message.contains("TABLE_METADATA_CLEANUP_TIMEOUT"));assert_eq!(f.pool.size(),0);assert!(available(&manual).await);f.audit.wait_closed(1).await;f.finish().await;
}

#[tokio::test]
async fn safe_pool_return_keeps_the_lease_until_the_return_hook_and_ping_finish(){
    let f=Fixture::new(Catalog::tidb(),None).await;f.audit.hold_return.store(true,Ordering::Release);let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();
    let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget()));f.audit.return_block.wait().await;assert!(!available(&manual).await);assert!(!task.is_finished());
    f.audit.hold_return.store(false,Ordering::Release);f.audit.return_block.release.notify_one();assert!(task.await.unwrap().is_ok());assert!(available(&manual).await);assert_eq!(f.pool.num_idle(),1);f.finish().await;
}

#[tokio::test]
async fn dropping_the_ipc_future_cancels_real_owned_read_and_cleanup(){
    for blocked in ["SHOW CREATE TABLE","ROLLBACK"]{
        let gate=Block::new(blocked);let f=Fixture::new(Catalog::tidb(),Some(gate.clone())).await;let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();
        let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget()));gate.wait().await;assert!(!available(&manual).await);task.abort();assert!(task.await.err().unwrap().is_cancelled());
        f.audit.wait_closed(1).await;tokio::time::timeout(Duration::from_secs(2),async{while !available(&manual).await{tokio::task::yield_now().await;}}).await.unwrap();assert_eq!(f.pool.size(),0);
        if blocked=="SHOW CREATE TABLE"{assert_eq!(f.audit.count("ROLLBACK"),0);}f.finish().await;
    }
}

#[tokio::test]
async fn dropping_the_ipc_during_pool_handoff_destroys_the_floating_connection(){
    let f=Fixture::new(Catalog::tidb(),None).await;f.audit.hold_return.store(true,Ordering::Release);let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();
    let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget()));f.audit.return_block.wait().await;assert!(!available(&manual).await);task.abort();assert!(task.await.err().unwrap().is_cancelled());
    f.audit.wait_closed(1).await;tokio::time::timeout(Duration::from_secs(2),async{while !available(&manual).await{tokio::task::yield_now().await;}}).await.unwrap();assert_eq!(f.pool.size(),0);f.finish().await;
}

#[tokio::test]
async fn acquisition_budget_and_cancellation_do_not_start_metadata_or_steal_the_borrowed_session(){
    for cancel in [false,true]{let f=Fixture::new(Catalog::tidb(),None).await;let borrowed=f.pool.acquire().await.unwrap();let manual=Arc::new(crate::manual_transaction::Registry::default());let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();let mut budget=budget();budget.acquire=Duration::from_millis(100);
        let task=tokio::spawn(read_owned_with_budget(f.db.clone(),binding(),"sample".into(),false,lease,budget));tokio::task::yield_now().await;assert!(!available(&manual).await);
        if cancel{task.abort();assert!(task.await.err().unwrap().is_cancelled());tokio::time::timeout(Duration::from_secs(2),async{while !available(&manual).await{tokio::task::yield_now().await;}}).await.unwrap();}
        else{let error=task.await.unwrap().err().unwrap();assert_eq!(error.code,"TABLE_METADATA_ACQUIRE_TIMEOUT");assert!(available(&manual).await);}
        assert_eq!(f.audit.count("@@version"),0);assert_eq!(f.pool.size(),1);drop(borrowed);f.finish().await;
    }
}

#[tokio::test]
async fn tidb_real_controlled_save_is_refused_before_locks_and_ddl(){
    let f=Fixture::new(Catalog::tidb(),None).await;
    let manual=crate::manual_transaction::Registry::default();
    let before=f.read(&manual,budget()).await.unwrap();
    let work=TableWork::Design(before.public.design);
    let b=binding();
    let registry=execution::Registry::default();
    let mut request=crate::execution_contracts::ExecutionRequest{
        execution_id:String::new(),connection_id:b.connection.clone(),namespace:b.namespace.clone(),owner_id:b.owner.clone(),
        sql:workflow::marker(&work).unwrap(),transaction_owner:None,timeout_ms:Some(30000),origin:Some("isolated-er-load".into()),
        execution_grant:None,cancel_before_send:false,
    };
    registry.grant_request(&mut request,b.clone()).unwrap();
    let entry=registry.register(&request,b,30).unwrap();
    let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();
    let result=run(f.db.clone(),entry,lease,work,false).await;
    let error=result.result.err().unwrap();
    assert_eq!(error.code,"TABLE_MYSQL_LOCK_UNSUPPORTED");
    assert!(result.design.is_none());assert!(result.operation.is_none());
    assert!(!f.audit.sql.lock().iter().any(|(_,sql)|sql.contains("GET_LOCK(")||sql.starts_with("LOCK TABLES ")||sql.starts_with("ALTER TABLE ")||sql.starts_with("CREATE TABLE ")));
    assert!(available(&manual).await);f.finish().await;
}

#[tokio::test]
async fn mysql_and_pg_acquire_timeout_or_cancellation_close_in_progress_handshake(){
    for mysql in [true,false]{for cancel in [false,true]{
        let listener=TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();
        let (entered,accepted)=tokio::sync::oneshot::channel();
        let server=tokio::spawn(async move{
            let (mut socket,_)=listener.accept().await.unwrap();let _=entered.send(());
            // 接收 PG startup 或等待 MySQL greeting；只在客户端实际关 socket 后完成。
            let mut bytes=vec![];let _=socket.read_to_end(&mut bytes).await;
        });
        let db=if mysql{
            Db::Mysql(sqlx::mysql::MySqlPoolOptions::new().max_connections(1).acquire_timeout(Duration::from_secs(3)).connect_lazy_with(
                sqlx::mysql::MySqlConnectOptions::new().host("127.0.0.1").port(port).username("fixture").ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)))
        }else{
            Db::Postgres(sqlx::postgres::PgPoolOptions::new().max_connections(1).acquire_timeout(Duration::from_secs(3)).connect_lazy_with(
                sqlx::postgres::PgConnectOptions::new().host("127.0.0.1").port(port).username("fixture").database("fixture").ssl_mode(sqlx::postgres::PgSslMode::Disable)))
        };
        let db=Arc::new(db);let manual=Arc::new(crate::manual_transaction::Registry::default());
        let lease=manual.ordinary(&["fixture-connection"]).await.unwrap();assert!(!available(&manual).await);
        let mut budget=budget();
        // 超时路径保持 150 ms；取消路径用独立长预算，不能把已到期的正常释放当作提前释放。
        if !cancel{budget.acquire=Duration::from_millis(150);}
        let task=tokio::spawn(read_owned_with_budget(db.clone(),binding(),"sample".into(),false,lease,budget));
        tokio::time::timeout(Duration::from_secs(2),accepted).await.unwrap().unwrap();
        if cancel{assert!(!available(&manual).await);task.abort();assert!(task.await.err().unwrap().is_cancelled());}
        else{assert_eq!(task.await.unwrap().err().unwrap().code,"TABLE_METADATA_ACQUIRE_TIMEOUT");}
        tokio::time::timeout(Duration::from_secs(2),server).await.expect("acquire cancellation must actually close the socket").unwrap();
        tokio::time::timeout(Duration::from_secs(2),async{while !available(&manual).await{tokio::task::yield_now().await;}}).await.unwrap();
        match db.as_ref(){Db::Mysql(p)=>{assert_eq!(p.size(),0);p.close().await;},Db::Postgres(p)=>{assert_eq!(p.size(),0);p.close().await;},_=>unreachable!()}
    }}
}

#[test]
fn default_read_budget_is_local_and_exact(){assert_eq!(READ_BUDGET.acquire,Duration::from_secs(15));assert_eq!(READ_BUDGET.read,Duration::from_secs(30));assert_eq!(READ_BUDGET.cleanup,Duration::from_secs(5));}
