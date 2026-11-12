"""轻量编译真实原生 dump/cursor/sink；共享SQL历史边界两版本同样直通sqlx。
python tools/tests/backup-dump-perf.py --rows 50000 --rounds 5 --mysql
基线必须来自动手前保存的工作树，绝不从git HEAD生成；不构建Tauri。
"""
import argparse, hashlib, json, os, re, shutil, subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]; SRC=ROOT/'src-tauri/src'
BASE=ROOT/'tools/performance/baseline/backup-20261112'; OUT=ROOT/'target/backup-dump-harness'
def text(path): return path.read_text(encoding='utf-8')
def function(source,name):
    match=re.search(r'^(?:pub(?:\([^\n]*?\))? )?(?:async )?fn '+re.escape(name)+r'\b',source,re.M)
    assert match,name
    return source[match.start():source.index('\n}',match.start())+2]
def impl(source,prefix):
    start=source.index(prefix); return source[start:source.index('\n}',start)+2]
def generate(version,directory):
    directory.mkdir(parents=True,exist_ok=True)
    source=text(version/'transfer.rs'); cursor=text(version/'transfer_export_cursor.rs')
    output=text(version/'transfer_output.rs').split('#[cfg(test)] mod tests')[0]
    mysql=text(version/'transfer_mysql_export.rs').split('#[cfg(test)] mod tests')[0]
    (directory/'transfer_output.rs').write_text(output,encoding='utf-8')
    (directory/'transfer_mysql_export.rs').write_text(mysql,encoding='utf-8')
    cursor=cursor[:cursor.index('type TdsClient =')]
    cursor=re.sub(r'        Db::Mssql\(client\).*?mssql_cursor[^\n]*\n','',cursor)
    a=cursor.index('        Db::Mssql(database) =>'); b=cursor.index('        _=>Ok(None)',a)
    cursor=cursor[:a]+cursor[b:]
    (directory/'transfer_export_cursor.rs').write_text(cursor,encoding='utf-8')
    ddl=function(source,'table_ddl')
    a=ddl.index('        Db::Mssql(_) =>');b=ddl.index('        Db::Sqlite(pool) =>',a);ddl=ddl[:a]+ddl[b:]
    a=ddl.index('        Db::ClickHouse(client) =>');b=ddl.index('        Db::Postgres(pool) =>',a);ddl=ddl[:a]+ddl[b:]
    ddl=re.sub(r'        Db::Redis[^\n]*=> Ok\(None\),','        _ => Ok(None),',ddl)
    sink_start=source.index('pub(crate) enum ExportSink');sink=source[sink_start:]
    sink_impl=re.search(r'^impl[^\n]* Write for LimitedRecordWriter',sink,re.M)[0]
    sink=sink[:sink.index(sink_impl)]+impl(sink,sink_impl)
    imports='''use std::collections::{HashMap,HashSet}; use std::sync::{Arc,OnceLock}; use std::sync::atomic::{AtomicBool,Ordering};
use std::path::Path; use std::io::Write; use anyhow::{anyhow,bail,Context,Result}; use serde::{Serialize,Deserialize};
use sqlx::{Column,Row,TypeInfo,ValueRef}; use crate::db::{Db,qualified,quote_ident};
use crate::models::{DbKind,Protocol,DumpOptions,ExportOptions}; use crate::transfer_stream::TransferLimits;
#[path="transfer_output.rs"] mod transfer_output; #[path="transfer_export_cursor.rs"] mod transfer_export_cursor;
use transfer_output::{StagedOutput,FileEncoder};
'''
    task=source[source.index('#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct TaskProgress'):source.index('pub fn validate_where')]
    if directory.name=='baseline': task='use crate::transfer::{TaskControl,validate_names};\n'
    body=imports+task+'\n'+'\n'.join(function(source,n) for n in ['dump','dump_database','routine_ddls','routine_dump_chunk','strip_definer','ensure_writable','validate_delimiter','csv_escape','csv_cell','protect_database_path'])+'\n'+ddl+'\n'+sink
    if directory.name=='current':
        restore=function(source,'restore_sql_file')
        a=restore.index('        Db::Mssql(_) =>');b=restore.index('        Db::Postgres(pool) =>',a);restore=restore[:a]+restore[b:]
        statement=function(source,'script_statements');a=statement.index('        Ok(crate::db::split_statements');b=statement.rfind('\n    }')
        script='fn script_statements(_db:&Db,content:&str)->Result<Vec<String>> {\n'+statement[a:b]+'\n}'
        body+='\n'+restore+'\n'+script+'\n'+function(source,'is_fk_guard_stmt')
        body+='\n#[derive(Debug,Clone,Serialize,Deserialize)] pub struct ExportTaskOptions {pub options:ExportOptions,pub task_id:String}\n'+'\n'.join(function(source,n) for n in ['run_export_task','validate_export','validate_where','text_encoding'])
    (directory/'mod.rs').write_text(body,encoding='utf-8')
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--rows',type=int,default=50000);parser.add_argument('--rounds',type=int,default=5);parser.add_argument('--mysql',action='store_true');parser.add_argument('--generate-only',action='store_true');parser.add_argument('--cargo',action='store_true');parser.add_argument('--regressions',action='store_true');args=parser.parse_args()
    assert (BASE/'transfer.rs').exists(),'先保存工作树基线，不允许HEAD'
    generate(SRC,OUT/'current');generate(BASE,OUT/'baseline')
    dbsource=text(SRC/'db.rs');dbbody='\n'.join(function(dbsource,n) for n in ['quote_ident','quote_literal','qualified','split_statements','strip_leading_comments'])
    schemasource=text(SRC/'schema.rs');parts=[]
    for name,marker in [('list_tables','let mut out = Vec::new();'),('table_schema','let mut columns = Vec::new();')]:
        f=function(schemasource,name);a=f.index('    if let Db::Http');b=f.index('    '+marker,a);parts.append(f[:a]+f[b:])
    helpers='\n'.join(function(schemasource,n) for n in ['mysql_get','pg_get'])
    schema='use anyhow::{anyhow,Result};use sqlx::{Column,Row,TypeInfo,ValueRef,mysql::MySqlRow,postgres::PgRow};use crate::db::{Db,quote_ident,qualified};use crate::models::*;\n#[path="schema_relational_keys.rs"] mod relational_keys;\n'+helpers+'\n'+'\n'.join(parts)
    (OUT/'schema.rs').write_text(schema,encoding='utf-8');shutil.copyfile(SRC/'schema_relational_keys.rs',OUT/'schema_relational_keys.rs')
    syncsource=text(SRC/'sync.rs');sync='use anyhow::{anyhow,bail,Result};use crate::models::{DbKind,Protocol};use crate::db::quote_literal;\n'+re.search(r'^fn binary[^\n]*',syncsource,re.M)[0]+'\n'+function(syncsource,'value_sql')
    (OUT/'sync.rs').write_text(sync,encoding='utf-8')
    for name in ['catalog.rs','values.rs']:shutil.copyfile(SRC/name,OUT/name)
    model=text(SRC/'models.rs')
    # 完整产品DbKind/Protocol及备份结构类型；去除不相关应用设置/日志类型依赖。
    spans=[model[:model.index('/// 数据源类型目录项')]]
    for name in ['ObjectType','ColumnInfo','IndexInfo','ForeignKeyInfo','TableSchema','ExportOptions','DumpOptions']:
        pos=model.index('pub '+('enum' if name=='ObjectType' else 'struct')+' '+name)
        start=model.rfind('#[derive',0,pos);end=model.index('\n}',pos)+2;spans.append(model[start:end])
    (OUT/'models.rs').write_text('\n'.join(spans),encoding='utf-8')
    limits=text(SRC/'transfer_stream.rs');limits=limits[limits.index('pub const DEFAULT_BATCH_ROWS'):limits.index('impl TransferLimits {')]
    (OUT/'transfer_stream.rs').write_text('use serde::{Serialize,Deserialize};\n'+limits,encoding='utf-8')
    shim='''extern crate self as navkit_lib;
mod models;mod catalog;mod values;mod schema;mod sync;mod transfer_stream;
#[path="current/mod.rs"] pub mod transfer;#[path="baseline/mod.rs"] mod baseline;
mod object_identity {use crate::models::DbKind;pub struct SqlServerObject {pub catalog:String,pub schema:String,pub name:String}impl SqlServerObject {pub fn parse(_:&str,_:&str)->anyhow::Result<Self>{anyhow::bail!("非测试通道")}pub fn qualified(&self,_:&DbKind)->String{unreachable!()}}}
mod mysql_routines {use crate::models::DbKind;pub async fn definitions(_:&sqlx::MySqlPool,_:&DbKind,_:&str,_:&str)->anyhow::Result<Vec<(String,String)>>{anyhow::bail!("本harness不测例程")}pub struct RoutineKind(String);impl RoutineKind {pub fn parse(s:&str)->anyhow::Result<Self>{Ok(Self(s.to_owned()))}pub fn keyword(&self)->&str{&self.0}}}
mod sql_history {pub struct Guard;impl Guard {pub fn new(_:&str,_:Vec<String>)->Self{Self}pub fn result<T,E,F:FnOnce(&T)->i64>(&mut self,_:&Result<T,E>,_:F){}}pub use sqlx::{query,query_scalar,raw_sql};pub fn db_context(_:&crate::db::Db){}pub async fn scope<T>(_:(),f:impl std::future::Future<Output=T>)->T{f.await}pub async fn describe<'e,E:sqlx::Executor<'e>>(e:E,s:&str)->Result<sqlx::Describe<E::Database>,sqlx::Error>{e.describe(s).await}}
mod db {use crate::models::{DbKind,Protocol};pub enum Db {Sqlite(sqlx::SqlitePool),Mysql(sqlx::MySqlPool),Postgres(sqlx::PgPool)}
'''+dbbody+'\n}\n'+text(ROOT/'tools/tests/backup-dump-perf-harness.rs')
    if args.regressions:shim+='\n#[cfg(test)] mod backup_dump_streaming {\n'+text(ROOT/'src-tauri/tests/backup_dump_streaming.rs').replace('//!','//')+'\n}'
    (OUT/'main.rs').write_text(shim,encoding='utf-8')
    hashes={str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in [BASE/'transfer.rs',BASE/'transfer_export_cursor.rs',SRC/'transfer.rs',SRC/'transfer_export_cursor.rs',SRC/'values.rs',SRC/'sync.rs']}
    (OUT/'sources.json').write_text(json.dumps(hashes,indent=2),encoding='utf-8')
    manifest='''[package]
name="backup-dump-harness"
version="0.0.0"
edition="2021"
[[bin]]
name="backup-dump-harness"
path="main.rs"
[dependencies]
anyhow="1"
serde={version="1",features=["derive"]}
serde_json={version="1",features=["raw_value"]}
sqlx={version="0.8",default-features=false,features=["runtime-tokio","sqlite","mysql","postgres","chrono","uuid","json","bigdecimal","tls-rustls"]}
tokio={version="1",features=["full"]}
chrono="0.4"
uuid={version="1",features=["v4","serde"]}
futures="0.3"
encoding_rs="0.8"
parking_lot="0.12"
'''
    (OUT/'Cargo.toml').write_text(manifest,encoding='utf-8')
    if args.generate_only:print(json.dumps({'generated':str(OUT),'sources':hashes}));return
    env=os.environ.copy();env['NAVKIT_PERF_ROWS']=str(args.rows);env['NAVKIT_PERF_ROUNDS']=str(args.rounds)
    if args.mysql:
        fixture=text(ROOT/'tools/acceptance/e2e/desktop14-runner-e2c772c4.mjs')
        env['NAVKIT_PERF_MYSQL_URL']='mysql://root:'+re.search(r"'-p([^']+)'",fixture)[1]+'@127.0.0.1:3306/mysql'
    if args.cargo:command=['cargo','test' if args.regressions else 'run','--release','--manifest-path',str(OUT/'Cargo.toml')]+(['--','--nocapture','--test-threads=1'] if args.regressions else [])
    else:
        deps=ROOT/'src-tauri/target/debug/deps';exe=OUT/('backup-dump-regressions.exe' if args.regressions else f'backup-dump-harness-{args.rows}-{args.rounds}.exe')
        command=['rustc','--edition=2021','-Awarnings','-C','opt-level=2',str(OUT/'main.rs'),'-o',str(exe),'-L','dependency='+str(deps)]
        for name in ['anyhow','serde','serde_json','sqlx','tokio','chrono','uuid','futures','encoding_rs','parking_lot']:
            files=list(deps.glob('lib'+name+'-*.rlib'));assert files,name
            pinned={'serde_json':'281fc27357dd5044','uuid':'93b4540d1c522ace'}
            lib=deps/('lib'+name+'-'+pinned[name]+'.rlib') if name in pinned else max(files,key=lambda p:p.stat().st_mtime_ns)
            command+=['--extern',name+'='+str(lib)]
        for lib in (ROOT/'src-tauri/target/debug/build').glob('libsqlite3-sys-*/out/sqlite3.lib'):command+=['-L','native='+str(lib.parent)]
        if args.regressions:command+=['--test']
        subprocess.run(command,cwd=ROOT,check=True);command=[str(exe)]+(['--nocapture','--test-threads=1'] if args.regressions else [])
    report=OUT/('latest-regressions.txt' if args.regressions else f'perf-{args.rows}-{args.rounds}.txt')
    with report.open('w',encoding='utf-8') as log:
        process=subprocess.Popen(command,cwd=ROOT,env=env,text=True,stdout=subprocess.PIPE,stderr=subprocess.STDOUT)
        for line in process.stdout:print(line,end='',flush=True);log.write(line);log.flush()
        raise SystemExit(process.wait())
if __name__=='__main__':main()
