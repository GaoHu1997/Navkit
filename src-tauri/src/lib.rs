//! NavKit —— Rust 后端入口。

pub mod catalog;
pub mod clickhouse;
mod background_process;
mod activity;
mod commands;
mod datasource_sync;
mod manual_transaction;
pub mod db;
mod error;
pub mod models;
mod http_db;
mod native_db;
mod strict_rest;
mod salesforce_rest;
mod hbase_rest;
mod native_http;
#[cfg(test)]
mod native_rest_tests;
mod native_cql;
mod native_mq;
mod native_zk;
mod native_rocket;
mod h2_db;
mod jdbc_db;
mod probe_cache;
mod mongo_db;
mod mssql;
mod data_generator;
mod driver_manager;
mod monitor;
mod odbc_db;
// Pure connection-template contracts are public; keep driver storage and loading modules private.
pub use driver_manager::escape_value as escape_odbc_connection_value;
pub use odbc_db::default_connection_string as default_odbc_connection_string;
mod redis_db;
pub mod schema;
pub mod storage;
pub mod sync;
pub mod transfer;
mod values;

use std::sync::Arc;

use commands::AppState;
use db::DbRegistry;
use storage::Store;

/// Tauri 应用入口。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 便携模式：WebView2 引擎数据重定向到 data/cache/webview，
    // 否则写入 %LOCALAPPDATA%\<identifier>\EBWebView，违反「不写 AppData」承诺。
    {
        let wv = storage::data_dir().join("cache").join("webview");
        let _ = std::fs::create_dir_all(&wv);
        std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &wv);
    }

    let store = Store::load();
    let datasource_sync = datasource_sync::Service::new(&store);
    let registry: Arc<DbRegistry> = Arc::new(parking_lot::RwLock::new(
        std::collections::HashMap::new(),
    ));
    let state = AppState { store, registry, manual_transactions: Default::default(), activity: Default::default() };
    storage::log_line("info", "应用启动");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|_app| {
            // 启动后预热驱动状态缓存与 Java 探针缓存：用户点「新建连接」时结果已在手，
            // 全程无感。缓存按指纹自动失效（下载/删除/替换驱动都会变化），无过期误报。
            tauri::async_runtime::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                let _ = crate::driver_manager::downloads::list_driver_statuses().await;
            });
            Ok(())
        })
        .manage(state)
        .manage(datasource_sync)
        .invoke_handler(tauri::generate_handler![
            datasource_sync::datasource_sync_get_config,
            datasource_sync::datasource_sync_set_config,
            datasource_sync::datasource_sync_test,
            datasource_sync::datasource_sync_preview,
            datasource_sync::datasource_sync_commit,
            datasource_sync::datasource_sync_cancel,
            commands::get_settings,
            commands::set_settings,
            commands::list_connections,
            commands::list_groups,
            commands::add_group,
            commands::move_connection,
            commands::ensure_connection,
            commands::open_database,
            commands::read_sql_file,
            commands::run_sync,
            commands::sync_type_mapping,
            commands::run_import_task,
            commands::run_export_task,
            commands::transfer_task_progress,
            commands::cancel_transfer_task,
            commands::save_connection,
            commands::delete_connection,
            commands::rename_group,
            commands::test_connection,
            commands::open_connection,
            commands::close_connection,
            commands::close_all_connections,
            commands::list_databases,
            commands::list_objects,
            commands::list_tables,
            commands::list_routines,
            commands::get_table_schema,
            commands::page_table,
            commands::execute_sql,
            commands::manual_transaction_start,
            commands::manual_transaction_status,
            commands::manual_transaction_finish,
            commands::update_row,
            commands::insert_row,
            commands::delete_row,
            commands::execute_ddl,
            commands::list_favorites,
            commands::add_favorite,
            commands::remove_favorite,
            commands::list_recent,
            commands::push_recent,
            commands::clear_recent,
            commands::run_import,
            commands::run_export,
            commands::export_query_result,
            commands::preview_csv,
            commands::run_transfer,
            commands::run_dump,
            commands::list_backups,
            commands::create_backup,
            commands::restore_backup,
            commands::restore_backup_detailed,
            commands::delete_backup,
            commands::open_backup_folder,
            commands::list_db_users,
            commands::create_db_user,
            commands::drop_db_user,
            commands::alter_db_user_password,
            commands::get_view_definition,
            commands::get_routine_definition,
            commands::table_status,
            commands::get_table_ddl,
            commands::list_jobs,
            commands::save_job,
            commands::delete_job,
            commands::list_job_history,
            commands::run_job_now,
            commands::server_info,
            commands::completion_data,
            commands::read_log,
            commands::list_sql_log,
            commands::clear_sql_log,
            monitor::list_db_processes,
            monitor::kill_db_process,
            commands::run_sync_database,
            commands::sync_diff,
            commands::db_kinds,
            driver_manager::list_custom_drivers,
            driver_manager::downloads::list_driver_statuses,
            driver_manager::downloads::download_database_driver,
            driver_manager::downloads::delete_managed_driver,
            data_generator::insert_generated_rows,
            driver_manager::save_custom_driver,
            driver_manager::delete_custom_driver,
            driver_manager::list_odbc_inventory,
            driver_manager::test_custom_driver,
            driver_manager::test_connection_with_saved_password,
            commands::db_categories,
            commands::about_info,
        ])
        .run(tauri::generate_context!())
        .expect("启动 NavKit 失败");
}
