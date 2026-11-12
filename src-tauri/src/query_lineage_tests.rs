use super::*;
use std::sync::atomic::{AtomicUsize,Ordering};
async fn fixture()->sqlx::SqliteConnection {
    let mut c=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT NOT NULL, b BLOB, n REAL, g TEXT GENERATED ALWAYS AS (v||'!') VIRTUAL)").execute(&mut c).await.unwrap();
    sqlx::query("INSERT INTO t(id,v,b,n) VALUES(9223372036854775807,'空😀',X'0000FF',1.5),(2,'',X'',NULL)").execute(&mut c).await.unwrap(); c
}
#[test]
fn complete_ast_accepts_only_direct_single_table_subset() {
    for sql in ["SELECT id,v FROM main.t", "-- c\nSELECT t.id AS x, t.v label FROM main.t AS t WHERE (id >= 2 AND v <> 'FROM z') OR id IN (1,2) ORDER BY t.id DESC NULLS LAST LIMIT 3 OFFSET 1;", "SELECT [别名].* FROM [中文 表] AS [别名] WHERE [名] LIKE 'a%' ESCAPE '!'", "SELECT `id` FROM `t` WHERE NOT id BETWEEN -2 AND +2"] { assert!(classify(sql).is_ok(),"{sql}"); }
    for sql in ["SELECT id FROM t;SELECT id FROM t", "SELECT DISTINCT id FROM t", "SELECT v||'' FROM t", "SELECT count(*) FROM t", "SELECT t.id FROM t JOIN u USING(id)", "WITH x AS(SELECT * FROM t) SELECT * FROM x", "SELECT (SELECT id FROM t) FROM t", "SELECT id FROM t UNION SELECT id FROM t", "SELECT id FROM t WHERE id IN (SELECT id FROM t)", "SELECT id FROM t WHERE random()>0", "SELECT id FROM t GROUP BY id", "SELECT id FROM t ORDER BY 1", "SELECT id FROM attached.t", "SELECT z.id FROM t", "SELECT id FROM t WHERE id=?", "SELECT id FROM t /*", "SELECT id FROM t LIMIT -1", "SELECT id FROM t; ;"] { assert!(classify(sql).is_err(),"{sql}"); }
}
#[tokio::test]
async fn real_origin_alias_opaque_ids_generated_and_lossless_storage_values() {
    let mut conn=fixture().await; let mut spooled=Vec::new();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT t.id AS renamed,t.v AS label,t.b,t.n,t.g FROM main.t AS t ORDER BY id DESC",CaptureLimits::default(),|_,_,row| {spooled.push(row.to_vec());Ok(())}).await.unwrap() else {panic!("eligible")};
    let proof=c.proof.unwrap(); assert_eq!(c.total,2); assert_eq!(spooled.len(),2); assert_eq!(c.display_rows,spooled);
    assert_eq!(proof.columns[0].base_column,"id"); assert_eq!(proof.columns[0].name,"renamed"); assert!(!proof.columns[0].writable); assert!(proof.columns[4].generated); assert!(!proof.columns[4].writable);
    assert_eq!(c.typed_rows[0][0],DbValue::Integer("9223372036854775807".into())); assert_eq!(c.typed_rows[0][1],DbValue::Text("空😀".into())); assert_eq!(c.typed_rows[0][2],DbValue::Binary("AAD/".into()));
    assert_eq!(c.typed_rows[1][1],DbValue::Text(String::new())); assert_eq!(c.typed_rows[1][2],DbValue::Binary(String::new())); assert_eq!(c.typed_rows[1][3],DbValue::Null);
}
#[tokio::test]
async fn actual_origin_rejects_duplicate_view_temp_shadow_missing_key_and_nullable_key() {
    let mut conn=fixture().await;
    sqlx::query("CREATE VIEW view_t AS SELECT * FROM t").execute(&mut conn).await.unwrap();
    for sql in ["SELECT id,id AS id2,v FROM t", "SELECT v FROM t", "SELECT * FROM view_t"] {
        let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,sql,CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else {panic!("syntax remains executable")}; assert!(c.proof.is_none(),"{sql}"); assert!(!c.read_only_reason.is_empty()); assert!(c.typed_rows.is_empty()); assert_eq!(c.total,2);
    }
    sqlx::query("CREATE TEMP TABLE t(id INTEGER PRIMARY KEY,v TEXT)").execute(&mut conn).await.unwrap();sqlx::query("INSERT INTO temp.t VALUES(1,'shadow')").execute(&mut conn).await.unwrap();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM t",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else {panic!()};assert!(c.proof.is_none());assert_eq!(c.total,1);assert_eq!(c.display_rows[0][1].as_deref(),Some("shadow"));
    sqlx::query("CREATE TABLE nullable(k TEXT UNIQUE,v TEXT)").execute(&mut conn).await.unwrap();sqlx::query("INSERT INTO nullable VALUES(NULL,'x')").execute(&mut conn).await.unwrap();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM nullable",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else {panic!()};assert!(c.proof.is_none());assert!(c.read_only_reason.contains("NULL"));
}
#[tokio::test]
async fn same_ddl_drop_recreate_is_rejected_even_when_rootpage_is_reused() {
    let mut conn=fixture().await;
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM t",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else {panic!()};let old=c.proof.unwrap().source;
    sqlx::query("DROP TABLE t").execute(&mut conn).await.unwrap();sqlx::query("CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT NOT NULL, b BLOB, n REAL, g TEXT GENERATED ALWAYS AS (v||'!') VIRTUAL)").execute(&mut conn).await.unwrap();
    let new=source(&mut conn,"t").await.unwrap();assert_eq!(old.rootpage,new.rootpage);assert_eq!(old.metadata_revision,new.metadata_revision);assert_ne!(old.schema_cookie,new.schema_cookie);assert!(validate_source(&mut conn,&old).await.is_err());
}
unsafe extern "C" fn trace(_:u32,ctx:*mut std::ffi::c_void,stmt:*mut std::ffi::c_void,_:*mut std::ffi::c_void)->i32 {
    let sql=libsqlite3_sys::sqlite3_sql(stmt.cast());if !sql.is_null() && CStr::from_ptr(sql).to_bytes().windows(b"pl21_original".len()).any(|v|v==b"pl21_original") { (&*(ctx as *const AtomicUsize)).fetch_add(1,Ordering::SeqCst); }0
}
#[tokio::test]
async fn single_real_step_full_spool_total_and_both_prefix_budgets() {
    let mut conn=fixture().await;let count=AtomicUsize::new(0);
    { let mut h=conn.lock_handle().await.unwrap();unsafe {libsqlite3_sys::sqlite3_trace_v2(h.as_raw_handle().as_ptr(),libsqlite3_sys::SQLITE_TRACE_STMT as u32,Some(trace),(&count as *const AtomicUsize).cast_mut().cast());} }
    let mut seen=0;let result=capture_sqlite_select(&mut conn,"SELECT * FROM t /*pl21_original*/",CaptureLimits{preview_rows:1,preview_bytes:1024,token_rows:1,token_bytes:1024},|_,_,_|{seen+=1;Ok(())}).await;
    { let mut h=conn.lock_handle().await.unwrap();unsafe {libsqlite3_sys::sqlite3_trace_v2(h.as_raw_handle().as_ptr(),0,None,std::ptr::null_mut());} }
    let CaptureAttempt::Executed(c)=result.unwrap() else {panic!()};assert_eq!(count.load(Ordering::SeqCst),1);assert_eq!(seen,2);assert_eq!(c.total,2);assert_eq!(c.typed_rows.len(),1);assert_eq!(c.display_rows.len(),1);
    let CaptureAttempt::NotEligible(_)=capture_sqlite_select(&mut conn,"SELECT count(*) FROM t",CaptureLimits::default(),|_,_,_|panic!("no query was sent")).await.unwrap() else {panic!()};
}
#[tokio::test]
async fn physical_primary_index_does_not_claim_rowid_generation_and_unsupported_decl_blocks_delete() {
    let mut conn=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE desc_pk(id INTEGER PRIMARY KEY DESC,v TEXT)").execute(&mut conn).await.unwrap();
    sqlx::query("INSERT INTO desc_pk VALUES(NULL,'not-an-alias')").execute(&mut conn).await.unwrap();
    let (metadata,_)=table_metadata(&mut conn,"desc_pk").await.unwrap(); assert!(!metadata.columns[0].auto_increment); assert!(metadata.columns[0].nullable);
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM desc_pk",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!()}; assert!(c.proof.is_none());assert!(c.read_only_reason.contains("NULL"));
    sqlx::query("CREATE TABLE no_rowid(id INTEGER PRIMARY KEY,v TEXT) WITHOUT ROWID").execute(&mut conn).await.unwrap();
    let (metadata,_)=table_metadata(&mut conn,"no_rowid").await.unwrap(); assert!(!metadata.columns[0].auto_increment);assert!(!metadata.columns[0].nullable);
    sqlx::query("CREATE TABLE unknown_type(id INTEGER PRIMARY KEY,v TEXT,n NUMERIC)").execute(&mut conn).await.unwrap();
    sqlx::query("INSERT INTO unknown_type VALUES(1,'known',1.5)").execute(&mut conn).await.unwrap();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT * FROM unknown_type",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!()};let proof=c.proof.unwrap();assert!(!proof.columns[2].writable);assert!(!proof.delete_supported);
}
unsafe extern "C" fn deny_column_metadata(_: *mut std::ffi::c_void, action:i32, p1:*const std::ffi::c_char, _:*const std::ffi::c_char, _:*const std::ffi::c_char, _:*const std::ffi::c_char)->i32 {
    if action==libsqlite3_sys::SQLITE_PRAGMA && !p1.is_null() && CStr::from_ptr(p1).to_bytes().eq_ignore_ascii_case(b"table_xinfo") { libsqlite3_sys::SQLITE_DENY } else { libsqlite3_sys::SQLITE_OK }
}
#[tokio::test]
async fn metadata_access_failure_keeps_original_display_without_retry_or_edit_tokens() {
    let mut conn=fixture().await;
    {let mut h=conn.lock_handle().await.unwrap();unsafe{libsqlite3_sys::sqlite3_set_authorizer(h.as_raw_handle().as_ptr(),Some(deny_column_metadata),std::ptr::null_mut());}}
    let mut rows=0;let result=capture_sqlite_select(&mut conn,"SELECT * FROM t",CaptureLimits::default(),|_,_,_|{rows+=1;Ok(())}).await;
    {let mut h=conn.lock_handle().await.unwrap();unsafe{libsqlite3_sys::sqlite3_set_authorizer(h.as_raw_handle().as_ptr(),None,std::ptr::null_mut());}}
    let CaptureAttempt::Executed(c)=result.unwrap() else{panic!()};assert_eq!(rows,2);assert_eq!(c.total,2);assert!(c.proof.is_none());assert!(c.typed_rows.is_empty());assert!(!c.read_only_reason.is_empty());
}
#[tokio::test]
async fn real_insert_column_evidence_copies_physical_flags_defaults_and_not_display_names() {
    let mut conn=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE insert_meta(id INTEGER PRIMARY KEY, label TEXT NOT NULL DEFAULT 'server default', note TEXT DEFAULT NULL, payload BLOB DEFAULT X'00FF', computed TEXT GENERATED ALWAYS AS (label||'!') STORED, unverified NUMERIC DEFAULT 0)").execute(&mut conn).await.unwrap();
    sqlx::query("INSERT INTO insert_meta DEFAULT VALUES").execute(&mut conn).await.unwrap();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT id AS manually_named_key,label AS generated_looking_name,note,payload,computed,unverified FROM main.insert_meta",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!("eligible direct projection")};
    let proof=c.proof.unwrap(); assert!(proof.insert_supported); assert_eq!(c.total,1);
    for origin in &proof.columns {
        let base=proof.metadata.columns.iter().find(|column|column.name==origin.base_column).unwrap();
        assert_eq!(origin.nullable,base.nullable); assert_eq!(origin.auto_increment,base.auto_increment); assert_eq!(origin.default_value,base.default);
        assert_eq!(origin.insertable,!origin.generated&&!base.auto_increment&&writable_decl(&base.data_type));
    }
    let id=&proof.columns[0]; assert_eq!(id.name,"manually_named_key"); assert!(id.key); assert!(!id.writable); assert!(!id.insertable); assert!(id.auto_increment); assert!(!id.nullable); assert_eq!(id.default_value,None);
    let label=&proof.columns[1]; assert_eq!(label.name,"generated_looking_name"); assert!(label.insertable); assert!(!label.auto_increment); assert!(!label.nullable); assert_eq!(label.default_value.as_deref(),Some("'server default'"));
    assert!(proof.columns[2].nullable); assert!(proof.columns[2].insertable); assert_eq!(proof.columns[2].default_value.as_deref(),Some("NULL"));
    assert_eq!(proof.columns[3].default_value.as_deref(),Some("X'00FF'")); assert!(proof.columns[3].insertable);
    assert!(proof.columns[4].generated); assert!(!proof.columns[4].writable); assert!(!proof.columns[4].insertable); assert!(!proof.columns[4].auto_increment);
    assert!(!proof.columns[5].writable); assert!(!proof.columns[5].insertable); assert_eq!(proof.columns[5].default_value.as_deref(),Some("0"));
    assert_eq!(c.typed_rows[0][1],DbValue::Text("server default".into())); assert_eq!(c.typed_rows[0][2],DbValue::Null); assert_eq!(c.typed_rows[0][3],DbValue::Binary("AP8=".into()));
    let wire=serde_json::to_value(&proof.columns).unwrap(); assert_eq!(wire[0]["insertable"],false); assert_eq!(wire[0]["nullable"],false); assert_eq!(wire[0]["auto_increment"],true); assert!(wire[0]["default_value"].is_null()); assert_eq!(wire[1]["default_value"],"'server default'");
}
#[tokio::test]
async fn real_composite_primary_key_can_be_insertable_without_becoming_updatable_or_auto_generated() {
    let mut conn=sqlx::SqliteConnection::connect("sqlite::memory:").await.unwrap();
    sqlx::query("CREATE TABLE composite_keys(a TEXT NOT NULL,b INTEGER NOT NULL,value TEXT NOT NULL DEFAULT 'from database',PRIMARY KEY(a,b)) WITHOUT ROWID").execute(&mut conn).await.unwrap();
    sqlx::query("INSERT INTO composite_keys(a,b) VALUES('part-a',7)").execute(&mut conn).await.unwrap();
    let CaptureAttempt::Executed(c)=capture_sqlite_select(&mut conn,"SELECT b AS generated_id,a AS auto_name,value FROM main.composite_keys",CaptureLimits::default(),|_,_,_|Ok(())).await.unwrap() else{panic!("eligible direct projection")};
    let proof=c.proof.unwrap(); assert!(proof.insert_supported); assert_eq!(proof.key_column_ids.len(),2);
    for column in &proof.columns[..2] { assert!(column.key); assert!(!column.writable); assert!(column.insertable); assert!(!column.auto_increment); assert!(!column.nullable); assert_eq!(column.default_value,None); }
    assert_eq!(proof.columns[0].base_column,"b"); assert_eq!(proof.columns[1].base_column,"a"); assert!(proof.columns[2].insertable); assert!(!proof.columns[2].nullable); assert_eq!(proof.columns[2].default_value.as_deref(),Some("'from database'"));
    assert_eq!(proof.key_column_ids,vec![proof.columns[1].result_column_id.clone(),proof.columns[0].result_column_id.clone()]);
    assert_eq!(c.typed_rows[0],vec![DbValue::Integer("7".into()),DbValue::Text("part-a".into()),DbValue::Text("from database".into())]);
}
#[tokio::test]
async fn callback_error_rolls_back_and_does_not_retry_original_select() {
    let mut conn=fixture().await;let mut calls=0;assert!(capture_sqlite_select(&mut conn,"SELECT * FROM t",CaptureLimits::default(),|_,_,_|{calls+=1;bail!("spool failed")}).await.is_err());assert_eq!(calls,1);
    let n:i64=sqlx::query_scalar("SELECT count(*) FROM t").fetch_one(&mut conn).await.unwrap();assert_eq!(n,2);
}
