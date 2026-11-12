//! 性能改动的正确性回归：保留 SQLite 真实 affinity 和所有认证完整性检查。
use super::*;
#[tokio::test]
async fn returning_probe_matches_old_delete_insert_select_for_all_affinities(){
    let mut spill=Spill::new().await.unwrap();spill.configure_probe(&["INTEGER","REAL","TEXT","NUMERIC","BLOB"]).await.unwrap();
    let mut reference=SqliteConnection::connect("sqlite::memory:").await.unwrap();sqlx::query("CREATE TABLE probe(a INTEGER,b REAL,c TEXT,d NUMERIC,e BLOB)").execute(&mut reference).await.unwrap();
    let values=[vec![Cell::Integer(i64::MAX),Cell::Integer(7),Cell::Integer(9),Cell::Text("1.50".into()),Cell::Blob(vec![0,255])],vec![Cell::Text("01".into()),Cell::Real(0.25f64.to_bits()),Cell::Text("nul\0尾".into()),Cell::Text("12345678901234567890.12345678901234567890".into()),Cell::Text("blob affinity text".into())],vec![Cell::Null;5],vec![Cell::Integer(i64::MIN),Cell::Text("2.25".into()),Cell::Blob(vec![]),Cell::Text("not numeric".into()),Cell::Null]];
    for _ in 0..3 {for cells in &values {sqlx::query("DELETE FROM probe").execute(&mut reference).await.unwrap();bind(crate::sql_history::query("INSERT INTO probe VALUES(?,?,?,?,?)"),cells).execute(&mut reference).await.unwrap();let row=sqlx::query("SELECT * FROM probe").fetch_one(&mut reference).await.unwrap();assert_eq!(spill.probe(cells).await.unwrap(),decode_row(&row).unwrap());}}
    assert!(spill.probe(&[]).await.is_err());reference.close().await.unwrap();spill.close().await.unwrap();
}
#[tokio::test]
async fn streaming_source_auth_rejects_missing_extra_reordered_and_modified_records(){
    for mutation in ["DELETE FROM source_rows WHERE seq=1","INSERT INTO source_rows SELECT 9,X'00',body,mac FROM source_rows WHERE seq=0","UPDATE source_rows SET seq=7 WHERE seq=1","UPDATE source_rows SET body=X'00' WHERE seq=1"] {
        let mut spill=Spill::new().await.unwrap();for id in 0..3{spill.push_source(&[Cell::Integer(id)],&[Cell::Integer(id)]).await.unwrap();}spill.seal().await.unwrap();
        let path=spill.test_path();let mut external=SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path)).await.unwrap();sqlx::raw_sql(mutation).execute(&mut external).await.unwrap();external.close().await.unwrap();assert!(spill.verify_source().await.is_err(),"{mutation}");spill.close().await.unwrap();assert!(!path.exists());
    }
}
#[tokio::test]
async fn streaming_key_auth_rejects_modified_target_and_mapped_records(){
    let mut spill=Spill::new().await.unwrap();spill.push_source(&[Cell::Integer(1)],&[Cell::Integer(1)]).await.unwrap();spill.seal().await.unwrap();spill.push_target_key(&[Cell::Integer(1)]).await.unwrap();spill.push_mapped_key(0,&[Cell::Integer(1)]).await.unwrap();spill.verify_target_keys().await.unwrap();spill.verify_mapped_keys().await.unwrap();let path=spill.test_path();let mut external=SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path)).await.unwrap();sqlx::raw_sql("UPDATE target_keys SET mac=zeroblob(32);DELETE FROM mapped_keys").execute(&mut external).await.unwrap();external.close().await.unwrap();assert!(spill.verify_target_keys().await.is_err());assert!(spill.verify_mapped_keys().await.is_err());spill.close().await.unwrap();assert!(!path.exists());
}

#[tokio::test]
async fn spill_chunk_boundaries_and_seals_publish_every_authenticated_record(){
    let mut spill=Spill::new().await.unwrap();let path=spill.test_path();let mut external=SqliteConnection::connect_with(&sqlx::sqlite::SqliteConnectOptions::new().filename(&path)).await.unwrap();
    for id in 0..127{spill.push_source(&[Cell::Integer(id)],&[Cell::Integer(id)]).await.unwrap();}
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM source_rows").fetch_one(&mut external).await.unwrap(),0,"未封存的私有chunk不对外发布");
    spill.push_source(&[Cell::Integer(127)],&[Cell::Integer(127)]).await.unwrap();assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM source_rows").fetch_one(&mut external).await.unwrap(),128);
    spill.push_source(&[Cell::Integer(128)],&[Cell::Integer(128)]).await.unwrap();spill.seal().await.unwrap();assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM source_rows").fetch_one(&mut external).await.unwrap(),129);assert_eq!(spill.source(128).await.unwrap().0,[Cell::Integer(128)]);
    for id in 0..129{spill.push_mapped_key(id as u64,&[Cell::Integer(id)]).await.unwrap();}spill.verify_mapped_keys().await.unwrap();assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM mapped_keys").fetch_one(&mut external).await.unwrap(),129);
    spill.push_verified(&[Cell::Integer(1)],&[Cell::Integer(1)]).await.unwrap();spill.verify_receipts().await.unwrap();assert_eq!(sqlx::query_scalar::<_,i64>("SELECT COUNT(*) FROM verified_rows").fetch_one(&mut external).await.unwrap(),1);external.close().await.unwrap();spill.close().await.unwrap();assert!(!path.exists());
}
