// 由 backup-dump-perf.py 与工作树产品原生函数一起编译；不含模拟数据读取器。
use anyhow::{Context,Result};
use std::{path::{Path,PathBuf},time::Instant};
use crate::{db::Db,models::{DbKind,DumpOptions}};
use sqlx::Row;
use futures::FutureExt;
async fn exec(db:&Db,sql:&str)->Result<()> {
    match db {Db::Sqlite(p)=>{sqlx::raw_sql(sql).execute(p).await?;},Db::Mysql(p)=>{sqlx::raw_sql(sql).execute(p).await?;},Db::Postgres(p)=>{sqlx::raw_sql(sql).execute(p).await?;}}
    Ok(())
}
async fn scalar(db:&Db,sql:&str)->Result<i64> {
    Ok(match db {Db::Sqlite(p)=>sqlx::query_scalar(sql).fetch_one(p).await?,Db::Mysql(p)=>sqlx::query_scalar(sql).fetch_one(p).await?,Db::Postgres(p)=>sqlx::query_scalar(sql).fetch_one(p).await?})
}
async fn close(db:&Db) {match db {Db::Sqlite(p)=>p.close().await,Db::Mysql(p)=>p.close().await,Db::Postgres(p)=>p.close().await}}
fn normalized(text:&str)->String {text.split_inclusive('\n').filter(|line|!line.starts_with("-- 备份时间")).collect()}
async fn backup(db:&Db,kind:&DbKind,schema:&str,file:&Path,whole:bool,baseline:bool)->Result<(u64,u64)> {
    if whole {
        if baseline {crate::baseline::dump_database(db,kind,schema,&[],&[],true,true,file.to_str().unwrap(),"perf").await}
        else {crate::transfer::dump_database(db,kind,schema,&[],&[],true,true,file.to_str().unwrap(),"perf").await}
    } else {
        let opts=DumpOptions {connection_id:"perf".into(),schema:schema.into(),table:"items".into(),file:file.to_string_lossy().into_owned(),data_only:true,structure_only:false,include_drop:false};
        let rows=if baseline {crate::baseline::dump(db,kind,&opts).await?} else {crate::transfer::dump(db,kind,&opts).await?};
        Ok((rows,std::fs::metadata(file)?.len()))
    }
}
fn median(times:&[f64])->f64 {let mut x=times.to_vec();x.sort_by(f64::total_cmp);x[x.len()/2]}
async fn seed(db:&Db,kind:&DbKind,schema:&str,rows:usize,wide:bool)->Result<()> {
    let full=db::qualified(kind,schema,"items");
    let binary=if kind.is_pg_family(){"BYTEA"}else{"BLOB"};
    let id=if kind.is_sqlite(){"INTEGER"}else{"BIGINT"};
    exec(db,&format!("CREATE TABLE {full}(id {id} PRIMARY KEY, amount TEXT, note TEXT, payload {binary});")).await?;
    let note=if wide {"中文\nquote'".repeat(48)} else {"中文\nquote'".into()};
    let payload=if wide {"00FF8027".repeat(64)}else{"00FF8027".into()};
    let note_hex=values::format_binary(note.as_bytes());
    let nhex=note_hex.strip_prefix("0x").unwrap();
    let note_sql=if kind.is_pg_family(){db::quote_literal(&note)}else{format!("CAST(X'{nhex}' AS {} )",if kind.is_sqlite(){"TEXT"}else{"CHAR CHARACTER SET utf8mb4"})};
    let blob_sql=if kind.is_pg_family(){format!("decode('{payload}','hex')")}else{format!("X'{payload}'")};
    for first in (1..=rows).step_by(500) {
        let last=(first+499).min(rows);
        let values=(first..=last).map(|i|format!("({},'123456789012345678901234567890.12345678901234567890',{}, {})",i,if i%3==0{"NULL"}else{&note_sql},blob_sql)).collect::<Vec<_>>().join(",");
        exec(db,&format!("INSERT INTO {full} VALUES {values}")).await?;
    }
    let empty=db::qualified(kind,schema,"z_empty");let view=db::qualified(kind,schema,"a_view");
    exec(db,&format!("CREATE TABLE {empty}(id {id}); CREATE VIEW {view} AS SELECT id FROM {full}")).await?;
    if kind.is_sqlite() {
        // 动态SQLite存储、i64边界、NUL/空二进制独立回归，不影响大量行性能样本。
        exec(db,"CREATE TABLE edge(id INTEGER, note TEXT, payload BLOB);INSERT INTO edge VALUES(9223372036854775807,CAST(X'610062' AS TEXT),X''),(-9223372036854775808,NULL,X'00FF80');").await?;
    }
    Ok(())
}
async fn native_precision(db:&Db,kind:&DbKind,schema:&str,dir:&Path)->Result<()> {
    if kind.is_sqlite(){return Ok(());}
    let full=db::qualified(kind,schema,"native_values");
    let (id_type,maximum,float_type,time_type,json_type,binary_type)=if kind.is_mysql_family(){("BIGINT UNSIGNED","18446744073709551615","DOUBLE","DATETIME(6)","JSON","BLOB")}else{("BIGINT","9223372036854775807","DOUBLE PRECISION","TIMESTAMP(6)","JSONB","BYTEA")};
    let blob=if kind.is_mysql_family(){"X'00FF8027'"}else{"decode('00FF8027','hex')"};
    exec(db,&format!("CREATE TABLE {full}(id {id_type} PRIMARY KEY,d DECIMAL(65,30),f {float_type},ts {time_type},doc {json_type},bin {binary_type});INSERT INTO {full} VALUES({maximum},12345678901234567890123456789012345.123456789012345678901234567890,1.2345678901234567,'2026-11-12 01:02:03.123456','{{\"n\":123456789012345678901234567890123456789,\"small\":0.12345678901234567890123456789}}',{blob})")).await?;
    let select=format!("SELECT id,d,f,ts,doc,bin FROM {full}");
    let before=match db {Db::Mysql(p)=>values::my_row_values(&sqlx::query(&select).fetch_one(p).await?,6),Db::Postgres(p)=>values::pg_row_values(&sqlx::query(&select).fetch_one(p).await?,6),_=>unreachable!()};
    let file=dir.join(format!("native-{schema}.sql"));
    let opts=DumpOptions{connection_id:"fixture".into(),schema:schema.into(),table:"native_values".into(),file:file.to_string_lossy().into_owned(),structure_only:false,data_only:false,include_drop:true};
    assert_eq!(transfer::dump(db,kind,&opts).await?,1);exec(db,&std::fs::read_to_string(&file)?).await?;
    let after=match db {Db::Mysql(p)=>values::my_row_values(&sqlx::query(&select).fetch_one(p).await?,6),Db::Postgres(p)=>values::pg_row_values(&sqlx::query(&select).fetch_one(p).await?,6),_=>unreachable!()};
    assert_eq!(before,after,"原生unsigned/i64、DECIMAL、DOUBLE、微秒时间、JSON数值、二进制必须真实往返相等");
    std::fs::remove_file(file)?;exec(db,&format!("DROP TABLE {full}")).await?;
    println!("{}",serde_json::json!({"protocol":kind.id(),"native_precision_roundtrip":true,"decimal":true,"json_lexemes":true,"microseconds":true,"binary":true}));Ok(())
}
async fn run_case(db:&Db,kind:&DbKind,schema:&str,rows:usize,rounds:usize,wide:bool,dir:&Path)->Result<()> {
    native_precision(db,kind,schema,dir).await?;
    seed(db,kind,schema,rows,wide).await?;
    for whole in [false,true] {
        let mut before=Vec::new();let mut after=Vec::new();let mut files=None;let mut size=0;
        // AB/BA交错，不把后运行版本的缓存优势误当改进。
        for round in 0..=rounds {
            let mut pair=[PathBuf::new(),PathBuf::new()];let mut texts=[String::new(),String::new()];
            for baseline in if round%2==0 {[true,false]}else{[false,true]} {
                let idx=if baseline{0}else{1};let file=dir.join(format!("{}-{}-{whole}-{wide}-{round}-{baseline}.sql",kind.id(),schema));
                let start=Instant::now();let result=backup(db,kind,schema,&file,whole,baseline).await?;let ms=start.elapsed().as_secs_f64()*1000.;
                assert_eq!(result.0,rows as u64+if whole&&kind.is_sqlite(){2}else{0});assert_eq!(result.1,std::fs::metadata(&file)?.len());size=result.1;
                if round>0 {if baseline{before.push(ms)}else{after.push(ms)}}
                texts[idx]=normalized(&std::fs::read_to_string(&file)?);pair[idx]=file;
            }
            assert_eq!(texts[0],texts[1],"产品基线与优化输出必须逐字节相同（仅忽略动态时间头）");
            if round==rounds {files=Some(pair)}else{for file in pair{std::fs::remove_file(file)?;}}
        }
        println!("{}",serde_json::json!({"protocol":kind.id(),"path":if whole{"dump_database"}else{"dump_data_only"},"wide":wide,"rows":rows,"bytes":size,"baseline_ms":before,"optimized_ms":after,"baseline_median_ms":median(&before),"optimized_median_ms":median(&after),"speedup":median(&before)/median(&after),"byte_equal":true}));
        for file in files.unwrap() {
            let text=std::fs::read_to_string(&file)?;
            if whole {
                // PG不允许DROP被旧视图依赖的表；仅清理本次随机schema中的测试视图。
                if kind.is_pg_family() {exec(db,&format!("DROP VIEW IF EXISTS {}",db::qualified(kind,schema,"a_view"))).await?;}
                // 恢复只作为兼容回归，不进入dump计时；在测试专用MySQL会话关闭逐行autocommit fsync。
                if kind.is_mysql_family() {exec(db,"SET autocommit=0").await?;}
                if kind.is_sqlite() {transfer::restore_sql_file(db,kind,file.to_str().unwrap()).await?;} else {exec(db,&text).await?;}
                if kind.is_mysql_family() {exec(db,"COMMIT; SET autocommit=1").await?;}
                let count=scalar(db,&format!("SELECT COUNT(*) FROM {}",db::qualified(kind,schema,"items"))).await?;assert_eq!(count,rows as i64);
                let precision=scalar(db,&format!("SELECT COUNT(*) FROM {} WHERE amount='123456789012345678901234567890.12345678901234567890'",db::qualified(kind,schema,"items"))).await?;assert_eq!(precision,rows as i64);
                let count=scalar(db,&format!("SELECT COUNT(*) FROM {} WHERE note IS NULL",db::qualified(kind,schema,"items"))).await?;assert_eq!(count,(rows/3) as i64);
                let binary=if kind.is_pg_family(){"encode(payload,'hex')=lower('00FF8027')"}else{"hex(payload)='00FF8027'"};
                let binary=if wide {if kind.is_pg_family(){"octet_length(payload)=256"}else{"length(payload)=256"}}else{binary};
                assert_eq!(scalar(db,&format!("SELECT COUNT(*) FROM {} WHERE {binary}",db::qualified(kind,schema,"items"))).await?,rows as i64);
                assert_eq!(scalar(db,&format!("SELECT COUNT(*) FROM {}",db::qualified(kind,schema,"a_view"))).await?,rows as i64);
                if kind.is_sqlite(){assert_eq!(scalar(db,"SELECT COUNT(*) FROM edge WHERE id=9223372036854775807 AND hex(note)='610062' AND typeof(note)='text' AND typeof(payload)='blob' AND length(payload)=0").await?,1);}
            }
            // 所有目标均create-only，包括已经成功发布的文件。
            assert!(backup(db,kind,schema,&file,whole,false).await.is_err());assert_eq!(std::fs::read_to_string(&file)?,text);
            std::fs::remove_file(file)?;
        }
    }
    println!("{}",serde_json::json!({"protocol":kind.id(),"wide":wide,"roundtrip":true,"no_clobber":true,"view":true,"precision":true,"binary":true}));
    assert!(!std::fs::read_dir(dir)?.any(|e|e.unwrap().file_name().to_string_lossy().starts_with(".navkit-")));
    Ok(())
}
#[tokio::main]
async fn main()->Result<()> {
    let rows=std::env::var("NAVKIT_PERF_ROWS").unwrap_or("50000".into()).parse::<usize>()?;
    let rounds=std::env::var("NAVKIT_PERF_ROUNDS").unwrap_or("5".into()).parse::<usize>()?;assert!(rows>0&&rounds>0);
    let dir=std::env::temp_dir().join(format!("navkit-backup-perf-{}",uuid::Uuid::new_v4()));std::fs::create_dir(&dir)?;
    let result:Result<()>=async {
        for wide in [false,true] {
            let p=sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;let db=Db::Sqlite(p);
            let result=run_case(&db,&DbKind::of("sqlite"),"main",rows,rounds,wide,&dir).await;close(&db).await;result?;
        }
        if let Ok(url)=std::env::var("NAVKIT_PERF_MYSQL_URL") {
            let admin=sqlx::mysql::MySqlPoolOptions::new().max_connections(1).connect(&url).await.context("连接本机MySQL测试环境失败")?;
            for wide in [false,true] {
                let namespace=format!("navkit_perf_{}",uuid::Uuid::new_v4().simple());assert!(namespace.starts_with("navkit_perf_"));
                sqlx::raw_sql(&format!("CREATE DATABASE {namespace} CHARACTER SET utf8mb4")).execute(&admin).await?;
                let result=std::panic::AssertUnwindSafe(async {
                    let mut options=url.parse::<sqlx::mysql::MySqlConnectOptions>()?;options=options.database(&namespace);
                    let p=sqlx::mysql::MySqlPoolOptions::new().max_connections(1).connect_with(options).await?;let db=Db::Mysql(p);
                    let result=run_case(&db,&DbKind::of("mysql"),&namespace,rows,rounds,wide,&dir).await;close(&db).await;result
                }).catch_unwind().await;
                // 即使连接/断言失败也只删除本次新建的随机库；不触碰已有数据库。
                sqlx::raw_sql(&format!("DROP DATABASE {namespace}")).execute(&admin).await?;
                result.map_err(|_|anyhow::anyhow!("MySQL备份断言失败，测试数据库已清理"))??;
            }admin.close().await;
        }
        if let Ok(url)=std::env::var("NAVKIT_PERF_PG_URL") {
            let p=sqlx::postgres::PgPoolOptions::new().max_connections(1).connect(&url).await?;let db=Db::Postgres(p);
            for wide in [false,true] {
                let namespace=format!("navkit_perf_{}",uuid::Uuid::new_v4().simple());exec(&db,&format!("CREATE SCHEMA {namespace}")).await?;
                let result=std::panic::AssertUnwindSafe(run_case(&db,&DbKind::of("postgresql"),&namespace,rows,rounds,wide,&dir)).catch_unwind().await;
                exec(&db,&format!("DROP SCHEMA {namespace} CASCADE")).await?;
                result.map_err(|_|anyhow::anyhow!("PG备份断言失败，测试schema已清理"))??;
            }close(&db).await;
        }
        Ok(())
    }.await;
    std::fs::remove_dir_all(&dir)?;result
}
