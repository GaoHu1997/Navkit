"""独立 rustc harness：同步源码原样包含；复用已构建依赖，不获取 Cargo 锁。
共享 metadata/history 是显式测试适配，任务/分页/值转换取自工作树真实源码。
先 --source target/sync-perf-baseline --label baseline，再 --label optimized。
"""
import argparse, os, subprocess, shutil, tempfile, json, re
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--source');p.add_argument('--baseline',action='store_true');p.add_argument('--mode',choices=['all','full','incremental'],default='all');p.add_argument('--label',default='optimized');p.add_argument('--filter',default='sync_performance::');p.add_argument('--rows',default='20000');p.add_argument('--rounds',default='3');a=p.parse_args();a.source=a.source or ('tools/performance/baseline/sync-20261112' if a.baseline else 'src-tauri/src')
root=Path(__file__).resolve().parents[2];src=root/'src-tauri/src';deps=root/'src-tauri/target/debug/deps';out=root/'target'/('sync-perf-build-'+a.label);out.mkdir(parents=True,exist_ok=True)
def text(name):return (src/name).read_text(encoding='utf8')
def section(s,start,end):return s[s.index(start):s.index(end,s.index(start))]
for name in ['sync.rs','sync_stream.rs','sync_stream_spill.rs']:shutil.copyfile(root/a.source/name,out/name)
shutil.copyfile(src/'sync_stream_tests.rs',out/'sync_stream_tests.rs')
shutil.copyfile(src/'sync_stream_performance_tests.rs',out/'sync_stream_performance_tests.rs')
shutil.copyfile(src/'sync_performance_tests.rs',out/'sync_performance_tests.rs')
shutil.copyfile(src/'sync_full_performance_tests.rs',out/'sync_full_performance_tests.rs')
transfer=text('transfer.rs')
tasks=section(transfer,'#[derive(Debug, Clone, Serialize, Deserialize)]\npub struct TaskProgress','pub fn validate_export(')
pages=section(transfer,'pub async fn fetch_transfer_rows(','/// 流式导出写入器')
quote=section(text('db.rs'),'pub fn quote_ident(','/// 用列名 + 类型构造')
quote=quote.replace('if kind.protocol() == Protocol::Mssql {','if false {')
# Unused SQL Server identity seam: the harness never dispatches it.
identity='mod object_identity {pub struct SqlServerObject;impl SqlServerObject {pub fn parse(_: &str,_: &str)->anyhow::Result<Self>{anyhow::bail!("not a harness channel")}pub fn qualified(&self,_:&crate::models::DbKind)->String{unreachable!()}}}'
header='#![allow(dead_code,unused_imports)]\n'+identity+'\n'
model=text('models.rs');start=model.rfind('#[derive',0,model.index('pub struct ColumnInfo'));end=model.index('/// 对象列表')
header+='mod models {'+'\n'.join(model.splitlines()[:96])+model[start:end]+'}\n'
for name in ['catalog','values']:header+=f'#[path={json.dumps(str(src/(name+".rs")))}] mod {name};\n'
header+='''mod sql_history {pub use sqlx::{query,query_scalar,raw_sql};pub type Query<'q,DB,A> = sqlx::query::Query<'q,DB,A>;pub struct Guard;impl Guard{pub fn new(_: &str,_:Vec<()>)->Self{Self}pub fn result<T,F:FnOnce(&T)->i64>(&mut self,_:&Result<T,sqlx::Error>,_:F){}}pub fn db_context(_: &crate::db::Db){}pub async fn scope<F:std::future::Future>(_:(),f:F)->F::Output{f.await}}
mod db {use crate::models::{DbKind,Protocol};#[derive(Clone)] pub enum Db{Sqlite(sqlx::SqlitePool),Mysql(sqlx::MySqlPool),Postgres(sqlx::PgPool),Mssql(()),ClickHouse(()),Odbc(()),Other}impl Db{pub async fn generic_exec(&self,_:&str)->anyhow::Result<u64>{anyhow::bail!("unsupported harness channel")}pub async fn generic_rows(&self,_:&str)->anyhow::Result<(Vec<String>,Vec<Vec<Option<String>>>)>{anyhow::bail!("unsupported harness channel")}}
'''+quote+'}\n'
header+='''mod schema {use crate::{db::Db,models::*};use anyhow::{Result,bail};use sqlx::Row;
pub async fn list_tables(db:&Db,_:DbKind,namespace:&str)->Result<Vec<(String,bool)>>{match db{Db::Sqlite(p)=>Ok(sqlx::query("SELECT name,type FROM sqlite_schema WHERE type IN ('table','view')").fetch_all(p).await?.into_iter().map(|r|(r.get("name"),r.get::<String,_>("type")=="view")).collect()),Db::Mysql(p)=>Ok(sqlx::query("SELECT CAST(TABLE_NAME AS CHAR) AS TABLE_NAME,CAST(TABLE_TYPE AS CHAR) AS TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA=?").bind(namespace).fetch_all(p).await?.into_iter().map(|r|(r.get("TABLE_NAME"),r.get::<String,_>("TABLE_TYPE")=="VIEW")).collect()),_=>bail!("unsupported metadata")}}
pub async fn table_schema(db:&Db,kind:DbKind,namespace:&str,table:&str)->Result<TableSchema>{let columns=match db{Db::Sqlite(p)=>sqlx::query(&format!("PRAGMA table_info({})",crate::db::quote_ident(&kind,table))).fetch_all(p).await?.into_iter().map(|r|ColumnInfo{name:r.get("name"),data_type:r.get("type"),nullable:r.get::<i64,_>("notnull")==0,default:r.get("dflt_value"),primary_key:r.get::<i64,_>("pk")>0,auto_increment:false,comment:None,length:None,precision:None,scale:None}).collect(),Db::Mysql(p)=>sqlx::query("SELECT CAST(COLUMN_NAME AS CHAR) AS COLUMN_NAME,CAST(COLUMN_TYPE AS CHAR) AS COLUMN_TYPE,CAST(IS_NULLABLE AS CHAR) AS IS_NULLABLE,CAST(COLUMN_DEFAULT AS CHAR) AS COLUMN_DEFAULT,CAST(COLUMN_KEY AS CHAR) AS COLUMN_KEY FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION").bind(namespace).bind(table).fetch_all(p).await?.into_iter().map(|r|ColumnInfo{name:r.get("COLUMN_NAME"),data_type:r.get("COLUMN_TYPE"),nullable:r.get::<String,_>("IS_NULLABLE")=="YES",default:r.get("COLUMN_DEFAULT"),primary_key:r.get::<String,_>("COLUMN_KEY")=="PRI",auto_increment:false,comment:None,length:None,precision:None,scale:None}).collect(),_=>bail!("unsupported metadata")};Ok(TableSchema{table:table.into(),database:namespace.into(),columns,indexes:vec![],foreign_keys:vec![],comment:None,engine:None,collation:None,row_count:None})}}
mod transfer {use crate::db::{Db,qualified,quote_ident};use crate::models::DbKind;use anyhow::{Result,anyhow,bail};use serde::{Serialize,Deserialize};use sqlx::{Row,Column,TypeInfo,ValueRef};use std::collections::{HashMap,HashSet};use std::sync::{Arc,OnceLock,atomic::{AtomicBool,Ordering}};
'''+tasks+pages+'''
pub async fn execute_atomic(db:&Db,stmts:&[(String,bool)],task:Option<&TaskControl>)->Result<u64>{let Db::Sqlite(p)=db else{bail!("unsupported atomic")};let mut tx=p.begin().await?;let mut n=0;for(sql,row)in stmts{if let Some(t)=task{t.check()?;}let r=sqlx::query(sql).execute(&mut *tx).await?;if *row{n+=r.rows_affected();}}if let Some(t)=task{t.check()?;}tx.commit().await?;Ok(n)}}
#[path="sync.rs"] mod sync;
#[path="sync_performance_tests.rs"] mod sync_performance;
'''
(out/'harness.rs').write_text(header,encoding='utf8')
externs=['sqlx','anyhow','serde','serde_json','tokio','futures','sha2','uuid','zeroize','libsqlite3_sys','parking_lot','chrono']
args=['rustc','--test','--edition=2021',str(out/'harness.rs'),'-L','dependency='+str(deps),'-L','native='+str(deps),'-C','opt-level=2','-C','debuginfo=0','-o',str(out/'harness.exe')]
fingerprints=root/'src-tauri/target/debug/.fingerprint'
records=list(fingerprints.glob('navkit-*/test-lib-navkit_lib.json'));record=max(records,key=lambda f:f.stat().st_mtime_ns)
pinned={d[1]:d[3] for d in json.loads(record.read_text())['deps']}
for name in externs:
    candidates=[]
    for marker in fingerprints.glob('*/lib-'+name):
        if int.from_bytes(bytes.fromhex(marker.read_text().strip()),'little')==pinned[name]:
            suffix=marker.parent.name.rsplit('-',1)[1];artifact=deps/('lib'+name+'-'+suffix+'.rlib')
            if artifact.exists():candidates.append(artifact)
    assert len(candidates)==1,(name,candidates,record)
    args+=['--extern',name+'='+str(candidates[0])]
# SQLite native static library directory from the existing Cargo build.
for native in (root/'src-tauri/target/debug/build').glob('libsqlite3-sys-*/out'):args+=['-L','native='+str(native)]
subprocess.run(args,check=True)
env=os.environ.copy();env.update(NAVKIT_SYNC_PERF_ROWS=a.rows,NAVKIT_SYNC_PERF_ROUNDS=a.rounds,NAVKIT_SYNC_PERF_LABEL=a.label,NAVKIT_SYNC_PERF_MODE=a.mode)
if 'mysql_real_product' in a.filter:
    fixture=(root/'tools/acceptance/e2e/desktop14-runner-e2c772c4.mjs').read_text(encoding='utf8')
    password=os.environ.get('NAVKIT_MYSQL_PASSWORD') or re.search(r"'-p([^']+)'",fixture).group(1)
    from urllib.parse import quote
    env['NAVKIT_SYNC_MYSQL_URL']='mysql://root:'+quote(password,safe='')+'@127.0.0.1:3306'
subprocess.run([str(out/'harness.exe'),a.filter,'--nocapture','--test-threads=1']+(['--ignored'] if a.filter.startswith('sync_performance::') else []),env=env,check=True)
