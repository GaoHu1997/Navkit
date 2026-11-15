//! Round-four live MySQL checks for destructive cross-schema dump restore and YEAR.
//! Explicit local disposable databases only; no credentials are logged.
use super::*;
use anyhow::{Context, ensure, Result};
use sqlx::{Connection, Executor};

async fn admin() -> Result<sqlx::MySqlPool> {
    let password = std::env::var("NAVKIT_IMPORT_MYSQL_PASSWORD").context("explicit local MySQL credentials required")?;
    let options = sqlx::mysql::MySqlConnectOptions::new().host("127.0.0.1").port(3306).username("root").password(&password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
    Ok(sqlx::mysql::MySqlPoolOptions::new().max_connections(2).after_connect(|conn, _| Box::pin(async move {
        // Same session mode as the product's physical MySQL connections.
        sqlx::query("SET SESSION sql_mode=CONCAT(@@session.sql_mode,',NO_BACKSLASH_ESCAPES')").execute(conn).await?;
        Ok(())
    })).connect_with(options).await?)
}
async fn database(pool: &sqlx::MySqlPool, name: &str) -> Result<sqlx::MySqlPool> {
    Ok(sqlx::mysql::MySqlPoolOptions::new().max_connections(2)
        .connect_with(pool.connect_options().as_ref().clone().database(name)).await?)
}
async fn values(pool: &sqlx::MySqlPool, schema: &str) -> Result<Vec<(i32, Option<String>, String)>> {
    Ok(sqlx::query_as(&format!("SELECT id,CAST(c_year AS CHAR),v FROM `{schema}`.`t_year` ORDER BY id"))
        .fetch_all(pool).await?)
}

#[tokio::test]
#[ignore = "explicit disposable local MySQL; validates same-engine CREATE keeps source precision"]
async fn live_mysql_same_engine_sync_preserves_types_and_values() -> Result<()> {
    let admin = admin().await?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let source = format!("navkit_r4_sync_src_{token}");
    let target = format!("navkit_r4_sync_dst_{token}");
    let result: Result<()> = async {
        for schema in [&source, &target] {
            sqlx::raw_sql(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4")).execute(&admin).await?;
        }
        let source_pool = database(&admin, &source).await?;
        sqlx::query("CREATE TABLE t_sync(id INT PRIMARY KEY,n INT,amount DECIMAL(12,2),moment DATETIME) ENGINE=InnoDB").execute(&source_pool).await?;
        sqlx::query("INSERT INTO t_sync VALUES(1,42,1234567890.12,'2026-11-15 12:34:56')").execute(&source_pool).await?;
        let target_pool = database(&admin, &target).await?;
        let kind = DbKind::of("mysql");
        let mapping = crate::sync::type_mapping(&Db::Mysql(source_pool.clone()), &kind, &source, "t_sync", &kind).await?;
        let mapped = mapping.iter().map(|column| (column.source.as_str(), column.target_type.as_str())).collect::<Vec<_>>();
        ensure!(mapped == vec![("id", "INT"), ("n", "INT"), ("amount", "DECIMAL(12,2)"), ("moment", "DATETIME")], "same-engine preview mapping drifted: {mapped:?}");
        let options = crate::sync::SyncOptions {
            task_id: None, source_connection: "source".into(), source_schema: source.clone(), source_table: "t_sync".into(),
            target_connection: "target".into(), target_schema: target.clone(), target_table: "t_sync".into(),
            mode: "both".into(), write_mode: "append".into(), sync_mode: "full".into(), incremental_update: false,
            create_target: true, columns: vec![], key_columns: vec![], delete_missing: false, where_clause: None,
        };
        let report = crate::sync::synchronize(&Db::Mysql(source_pool.clone()), &kind, &Db::Mysql(target_pool.clone()), &kind, &options).await?;
        ensure!(report.rows == 1 && report.created, "sync report differs: {report:?}");
        let columns = sqlx::query_as::<_, (String, String)>("SELECT CAST(COLUMN_NAME AS CHAR),CAST(COLUMN_TYPE AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME='t_sync' ORDER BY ORDINAL_POSITION")
            .bind(&target).fetch_all(&admin).await?;
        ensure!(columns == vec![
            ("id".into(), "int".into()), ("n".into(), "int".into()),
            ("amount".into(), "decimal(12,2)".into()), ("moment".into(), "datetime".into()),
        ], "target DDL type drifted: {columns:?}");
        let row = sqlx::query_as::<_, (i64, Option<i64>, Option<String>, Option<String>)>("SELECT id,n,CAST(amount AS CHAR),CAST(moment AS CHAR) FROM t_sync")
            .fetch_one(&target_pool).await?;
        ensure!(row == (1, Some(42), Some("1234567890.12".into()), Some("2026-11-15 12:34:56".into())), "target values differ: {row:?}");
        target_pool.close().await; source_pool.close().await;
        Ok(())
    }.await;
    let cleanup = async {
        sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS `{source}`;DROP DATABASE IF EXISTS `{target}`")).execute(&admin).await?;
        Ok::<(), anyhow::Error>(())
    }.await;
    admin.close().await; cleanup?; result
}

#[tokio::test]
#[ignore = "explicit disposable local MySQL; validates source A remains intact while restoring to B"]
async fn live_mysql_cross_schema_restore_preserves_source_and_year_values() -> Result<()> {
    let admin = admin().await?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let source = format!("navkit_r4_src_{token}");
    let target = format!("navkit_r4_dst_{token}");
    let directory = std::env::temp_dir().join(format!("navkit-r4-live-{token}"));
    let result: Result<()> = async {
        for schema in [&source, &target] {
            sqlx::raw_sql(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4")).execute(&admin).await?;
        }
        let source_pool = database(&admin, &source).await?;
        sqlx::query("CREATE TABLE t_year(id INT PRIMARY KEY,c_year YEAR,v VARCHAR(80)) ENGINE=InnoDB").execute(&source_pool).await?;
        sqlx::query("INSERT INTO t_year VALUES(1,1901,'中文'),(2,NULL,''),(3,2026,'quote''s end'),(4,2155,'max')").execute(&source_pool).await?;
        let expected = values(&source_pool, &source).await?;
        ensure!(expected == vec![
            (1, Some("1901".into()), "中文".into()), (2, None, "".into()),
            (3, Some("2026".into()), "quote's end".into()), (4, Some("2155".into()), "max".into()),
        ], "fixture values differ: {expected:?}");
        std::fs::create_dir(&directory)?;
        let file = directory.join("archive.sql");
        let kind = DbKind::of("mysql");
        let report = dump_database(&Db::Mysql(source_pool.clone()), &kind, &source, &[], &[], true, true, file.to_str().unwrap(), "").await?;
        ensure!(report.0 == 4, "dump rows differ: {}", report.0);
        let archive = std::fs::read_to_string(&file)?;
        ensure!(!archive.contains(&format!("`{source}`.`t_year`")), "archive still targets source schema");
        ensure!(archive.contains("INSERT INTO `t_year`"), "archive does not target a restore-relative table");
        ensure!(archive.contains("1901") && archive.contains("2155") && archive.contains("NULL"), "YEAR values absent from archive");
        let target_pool = database(&admin, &target).await?;
        let restore = restore_sql_file_detailed(&Db::Mysql(target_pool.clone()), &kind, file.to_str().unwrap()).await?;
        ensure!(restore.failed == 0, "restore failed: {:?}", restore.steps.iter().find(|step| !step.ok).map(|step| &step.error));
        ensure!(values(&source_pool, &source).await? == expected, "source rows changed during restore");
        ensure!(values(&target_pool, &target).await? == expected, "target rows differ after restore");
        target_pool.close().await; source_pool.close().await;
        Ok(())
    }.await;
    let cleanup = async {
        sqlx::raw_sql(&format!("DROP DATABASE IF EXISTS `{source}`;DROP DATABASE IF EXISTS `{target}`")).execute(&admin).await?;
        let _ = std::fs::remove_dir_all(&directory);
        Ok::<(), anyhow::Error>(())
    }.await;
    admin.close().await; cleanup?; result
}
