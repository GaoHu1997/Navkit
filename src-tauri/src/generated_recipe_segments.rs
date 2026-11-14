// Explicit SQLite-only recipe segmentation. Included in batch_execution; no parallel executor.
// Limits account for UTF-8 bound values + 32B per field (Engine's row budget), NOT WAL bytes.
const RECIPE_SEGMENT_ROWS: u64 = 1000;
const RECIPE_SEGMENT_BYTES: usize = 8 * 1024 * 1024;

fn recipe_segmented_unsupported(kind:&DbKind)->AppError {
    AppError::coded("RECIPE_SEGMENTED_UNSUPPORTED",format!("{} 不支持显式 segmented recipe；目前仅认证 SQLite 原生逐段事务，尚未发送 SQL",kind.as_str()))
}

fn recipe_rollback_confirmed(record:&BatchRecord,atomic_message:&str) {
    record.transaction("rolled_back");
    let prefix={let mut p=record.progress.lock();p.affected=p.committed_prefix.map_or(0,|prefix|prefix.1);p.committed_prefix};
    let message=prefix.map_or_else(||atomic_message.to_owned(),|(rows,affected)|format!("当前段 ROLLBACK 已确认；此前已确认提交 {rows} 行 / 持久影响 {affected} 行保留，不能撤销"));
    record.log("success","rollback",None,None,&message,None);
}

// Shared by the old atomic handler and the segmented helper. Must only be called AFTER BEGIN.
async fn sqlite_generated_info(conn:&mut sqlx::SqliteConnection,kind:&DbKind,req:&GeneratedRowsRequest)->Result<TableSchema> {
    let definition:Option<String>=crate::sql_history::query_scalar("SELECT sql FROM main.sqlite_master WHERE type='table' AND name=?").bind(&req.table).fetch_optional(&mut *conn).await?;
    let definition=definition.ok_or_else(||anyhow!("目标必须为固定 main 中的真实表"))?;
    let metadata=crate::sql_history::query(&format!("PRAGMA main.table_xinfo({})",db::quote_ident(kind,&req.table))).fetch_all(&mut *conn).await?;
    let mut cols=vec![];
    for row in metadata {
        let name:String=row.try_get("name")?;let hidden:i64=row.try_get("hidden")?;
        if hidden!=0 {if req.columns.contains(&name){bail!("生成/隐藏字段不可写入");}continue;}
        cols.push(column(name,row.try_get("type")?,row.try_get::<i64,_>("notnull")?==0,row.try_get("dflt_value")?,row.try_get::<i64,_>("pk")?>0,false));
    }
    let primary=cols.iter().filter(|c|c.primary_key).count();
    if primary==1&&!definition.to_ascii_uppercase().contains("WITHOUT ROWID") {
        // Inline INTEGER PRIMARY KEY DESC is SQLite's historical exception:
        // it owns a separate PK index and omission yields NULL, not a generated
        // rowid alias. Read the real index origin under this same transaction.
        let indexes=crate::sql_history::query(&format!("PRAGMA main.index_list({})",db::quote_ident(kind,&req.table))).fetch_all(&mut *conn).await?;
        let separate_primary=indexes.iter().try_fold(false,|found,row|->std::result::Result<bool,sqlx::Error>{Ok(found||row.try_get::<String,_>("origin")?=="pk")})?;
        if !separate_primary {
            for c in &mut cols {if c.primary_key&&c.data_type.eq_ignore_ascii_case("INTEGER")&&!req.columns.contains(&c.name){c.auto_increment=true;}}
        }
    }
    let info=table_info(req,cols);data_generator::validate_columns(req,&info)?;Ok(info)
}
fn sqlite_generated_sql(kind:&DbKind,req:&GeneratedRowsRequest)->String {
    let unqualified=data_generator::insert_sql(kind,req,&[]);let table=db::quote_ident(kind,&req.table);
    unqualified.replacen(&format!("INSERT INTO {}",table),&format!("INSERT INTO \"main\".{}",table),1)
}
async fn sqlite_generated_bound_row(conn:&mut sqlx::SqliteConnection,record:&BatchRecord,sql:&str,row:&[Option<String>],index:u64)->Result<()> {
    let mut query=crate::sql_history::query(sql);for value in row{query=query.bind(value);}
    if record.entry.stopping(){return Err(stop_error());}
    record.progress.lock().sent+=1;let began=Instant::now();
    let result=query.execute(&mut *conn).await.map(|r|r.rows_affected());
    generated_row(record,index+1,sql,&result,began.elapsed().as_millis() as u64);result?;Ok(())
}
fn recipe_bound_bytes(row:&[Option<String>])->usize {
    row.iter().map(|v|v.as_ref().map_or(0,String::len)+32).sum()
}
async fn recipe_segment_commit(conn:&mut sqlx::SqliteConnection,record:&BatchRecord,rows:u64)->Result<()> {
    if record.entry.stopping(){return Err(stop_error());}
    record.log("info","commit",None,None,"等待当前段 COMMIT 回执；已确认前缀不可撤销",None);
    crate::sql_history::query("COMMIT").execute(&mut *conn).await.map_err(|e|AppError::coded("EXECUTION_EFFECTS_UNKNOWN",format!("当前段 COMMIT 回执失败；本段结果未知，已确认前缀保留，禁止重放；原因：{e}")))?;
    {
        let mut p=record.progress.lock();let (committed,_)=p.committed_prefix.unwrap_or((0,0));
        let affected=p.affected;p.committed_prefix=Some((committed+rows,affected));
    }
    record.transaction("committed");
    let prefix=record.progress.lock().committed_prefix.unwrap();
    record.log_detail("success","commit",None,None,&format!("当前段 COMMIT 已确认；累计提交 {} 行，失败/取消不能撤销此前缀",prefix.0),None,None,Some(prefix.1),None);
    Ok(())
}
async fn generate_sqlite_segments(conn:&mut sqlx::SqliteConnection,kind:&DbKind,record:&BatchRecord,req:&GeneratedRowsRequest,work:&RecipeWork)->Result<MultiResult> {
    data_generator::validate_shape(req)?;
    record.progress.lock().committed_prefix=Some((0,0));
    let sql=sqlite_generated_sql(kind,req);let total=work.recipe.total;
    let mut index=0;let mut rows=0;let mut bytes=0;let mut engine=None;
    while index<total {
        if record.entry.stopping(){return Err(stop_error());}
        if engine.is_none() {
            // Reacquire the real write lock and reread ALL metadata inside every segment.
            crate::sql_history::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;record.transaction("active");
            let info=sqlite_generated_info(conn,kind,req).await?;
            let fingerprint=recipe_fingerprint_sqlite(conn,kind,req,&info).await?;
            engine=Some(prepare_recipe(work,&info,fingerprint)?);
            if index==0 {generated_validated(record,req,&sql);}
            rows=0;bytes=0;
        }
        // Only this row is retained; global index/seed/time do not restart at boundaries.
        let row=engine.as_ref().unwrap().row(index)?;let row_bytes=recipe_bound_bytes(&row);
        if rows>0&&bytes+row_bytes>RECIPE_SEGMENT_BYTES {
            recipe_segment_commit(conn,record,rows).await?;engine=None;
            recipe_segment_boundary(record).await;
            continue;
        }
        {let mut p=record.progress.lock();p.scanned+=1;p.scan_complete=p.scanned==total;}
        sqlite_generated_bound_row(conn,record,&sql,&row,index).await?;
        index+=1;rows+=1;bytes+=row_bytes;
        if rows>=RECIPE_SEGMENT_ROWS||bytes>=RECIPE_SEGMENT_BYTES||index==total {
            recipe_segment_commit(conn,record,rows).await?;engine=None;
            if index<total {recipe_segment_boundary(record).await;}
        } else if index%256==0 {tokio::task::yield_now().await;}
    }
    Ok(record.result())
}
// Yield at unlocked boundaries; tests can arrange DDL at this exact native boundary without races.
async fn recipe_segment_boundary(_record:&BatchRecord) {
    #[cfg(test)] {
        let hook=recipe_segment_hooks().lock().remove(&_record.entry.id);
        if let Some(hook)=hook {hook().await;}
    }
    tokio::task::yield_now().await;
}
#[cfg(test)]
type RecipeSegmentHook=Box<dyn FnOnce()->futures::future::BoxFuture<'static,()>+Send>;
#[cfg(test)]
fn recipe_segment_hooks()->&'static Mutex<HashMap<String,RecipeSegmentHook>> {
    static HOOKS:OnceLock<Mutex<HashMap<String,RecipeSegmentHook>>>=OnceLock::new();
    HOOKS.get_or_init(||Mutex::new(HashMap::new()))
}
