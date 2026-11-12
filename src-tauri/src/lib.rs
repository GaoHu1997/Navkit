//! NavKit —— Rust 后端入口。

pub mod catalog;
pub mod clickhouse;
mod background_process;
mod activity;
mod commands;
mod db_users;
mod datasource_sync;
mod design;
mod table_design_execution;
mod controlled_execution;
mod object_commands;
mod object_mutation;
mod manual_transaction;
mod mutation;
mod mysql_options;
mod tls_options;
mod ssh_tunnel;
mod native_tls;
mod connection_health;
mod runtime_commands;
mod scheduler;
mod scheduler_desktop;
mod scheduler_tools;
mod routine_create;
mod routine_design;
pub mod object_catalog;
mod routine_call;
mod mysql_routines;
mod execution_bound;
mod structure_sync;
mod structure_execution;
mod structure_commands;
mod table_objects;
mod table_copy;
mod table_workflows;
mod table_workflows_native;
pub mod db;
mod error;
pub mod execution_contracts;
pub mod execution;
mod query_spool;
mod query_execution;
mod query_lineage;
mod query_result_store;
mod query_result_mutation;
mod query_commands;
mod execution_grants;
#[cfg(test)] mod execution_tests;
mod execution_policy;
pub mod models;
mod http_db;
mod http_tunnel_protocol;
mod http_tunnel;
mod http_tunnel_adapter;
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
mod java_runtime;
mod jdbc_db;
mod probe_cache;
mod mongo_db;
mod mssql;
mod batch_execution;
mod batch_sql_reader;
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
pub mod sql_history;
pub mod sync;
pub mod transfer;
mod transfer_stream;
mod object_identity;
mod batch_export;
mod values;
mod updater;
pub mod data_dictionary;

use std::sync::Arc;
use tauri::Manager;

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
    // 更新残留清理：删除上次换名替换留下的 *.old 与下载半成品（删除失败静默留给下次）。
    updater::cleanup_old_versions();
    match ssh_tunnel::profiles::commit::recover_all(&store, store.data_root()) {
        Ok(report) if report.recovered > 0 || report.blocked > 0 => storage::log_line("info", &format!("本机运行配置恢复：{} 项已恢复，{} 项保留待核对", report.recovered, report.blocked)),
        Ok(_) => {},
        Err(_) => storage::log_line("error", "本机运行配置恢复不可用；已保留原记录，不会创建替代密钥或重放数据库操作"),
    }
    let datasource_sync = datasource_sync::Service::new(&store);
    let registry: Arc<DbRegistry> = Arc::new(parking_lot::RwLock::new(
        std::collections::HashMap::new(),
    ));
    let state = AppState { store, registry, manual_transactions: Default::default(), activity: Default::default(), executions: Default::default(), mutations: Default::default(), query_results: Default::default(), connection_runtime: Default::default() };
    storage::log_line("info", "应用启动");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            let scheduler = scheduler_desktop::SchedulerHost::new(app.handle().clone(), app.state::<AppState>().store.data_root().to_owned());
            app.manage(scheduler);
            let scheduler_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if scheduler_app.state::<scheduler_desktop::SchedulerHost>().start().is_err() {
                    storage::log_line("error", "持久调度服务未启动；已保留任务和运行记录，不会重放未知执行");
                }
            });
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
            scheduler_desktop::scheduler_list_jobs,
            scheduler_desktop::scheduler_list_templates,
            scheduler_desktop::scheduler_load_template,
            scheduler_desktop::scheduler_save_template,
            scheduler_desktop::scheduler_save_job,
            scheduler_desktop::scheduler_pause_job,
            scheduler_desktop::scheduler_delete_job,
            scheduler_desktop::scheduler_history,
            scheduler_desktop::scheduler_cancel_run,
            scheduler_desktop::scheduler_preview_times,
            scheduler_desktop::scheduler_prepare_approval,
            scheduler_desktop::scheduler_approve_job,
            scheduler_desktop::scheduler_run_now,
            scheduler_desktop::scheduler_prepare_legacy_migration,
            scheduler_desktop::scheduler_confirm_legacy_migration,
            runtime_commands::runtime_profile_view,
            runtime_commands::test_runtime_connection,
            commands::check_connection_health,
            commands::get_connection_health,
            datasource_sync::datasource_sync_get_config,
            datasource_sync::datasource_sync_set_config,
            datasource_sync::datasource_sync_test,
            datasource_sync::datasource_sync_preview,
            datasource_sync::datasource_sync_commit,
            datasource_sync::datasource_sync_cancel,
            object_commands::read_object_mutation_baseline,
            object_commands::preview_object_mutation,
            object_commands::execute_object_mutation,
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
            commands::validate_connection_options,
            commands::open_connection,
            commands::close_connection,
            commands::close_all_connections,
            commands::list_databases,
            commands::list_objects,
            commands::list_tables,
            commands::list_routines,
            object_commands::list_object_catalog,
            object_commands::get_object_definition,
            object_commands::read_routine_creation_baseline,
            object_commands::preview_routine_creation,
            object_commands::execute_routine_creation,
            object_commands::read_exact_routine_design,
            object_commands::get_routine_call_metadata,
            object_commands::preview_routine_call,
            object_commands::execute_routine_call,
            object_commands::preview_routine_replacement,
            object_commands::execute_routine_replacement,
            commands::get_table_schema,
            commands::apply_table_design,
            table_workflows::read_table_design_snapshot,
            table_workflows::preview_table_design,
            table_workflows::execute_table_design,
            table_workflows::preview_table_operation,
            table_workflows::execute_table_operation,
            commands::read_routine_design,
            commands::apply_routine_replacement,
            commands::preview_row_mutation,
            commands::execute_row_mutation,
            query_commands::query_result_snapshot,
            query_commands::preview_query_row_mutation,
            query_commands::release_query_result,
            commands::list_table_constraints,
            commands::drop_table_trigger,
            commands::preview_structure_sync,
            commands::apply_structure_sync,
            structure_commands::preview_controlled_structure_sync,
            structure_commands::execute_structure_sync,
            commands::page_table,
            commands::execute_sql,
            commands::issue_execution_grant,
            commands::execute_sql_v2,
            commands::cancel_execution,
            commands::execution_status,
            commands::execution_capabilities,
            commands::manual_transaction_start,
            commands::manual_transaction_status,
            commands::manual_transaction_finish,
            commands::manual_transaction_mutate,
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
            batch_export::run_batch_export,
            commands::export_query_result,
            commands::fetch_query_rows,
            commands::export_query_spooled,
            commands::drop_query_spool,
            commands::preview_csv,
            commands::run_transfer,
            commands::run_dump,
            commands::list_backups,
            commands::create_backup,
            commands::restore_backup,
            commands::restore_backup_detailed,
            commands::delete_backup,
            commands::open_backup_folder,
            commands::rename_backup,
            commands::run_sql_file,
            commands::dump_database_to_file,
            commands::list_db_users,
            commands::create_db_user,
            commands::drop_db_user,
            commands::alter_db_user_password,
            db_users::read_db_user,
            db_users::preview_db_user_changes,
            db_users::apply_db_user_changes,
            commands::get_view_definition,
            commands::get_routine_definition,
            commands::table_status,
            commands::get_table_ddl,
            commands::list_jobs,
            commands::save_job,
            commands::delete_job,
            commands::list_job_history,
            commands::run_job_now,
            commands::record_job_history,
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
            java_runtime::java_runtime_status,
            java_runtime::install_java_runtime,
            batch_execution::register_batch_execution,
            batch_execution::run_batch_execution,
            batch_execution::batch_execution_status,
            data_generator::insert_generated_rows,
            driver_manager::save_custom_driver,
            driver_manager::delete_custom_driver,
            driver_manager::list_odbc_inventory,
            driver_manager::test_custom_driver,
            driver_manager::test_connection_with_saved_password,
            commands::db_categories,
            commands::about_info,
            updater::update_check,
            updater::update_download,
            updater::update_status,
            updater::update_apply,
            commands::dictionary_metadata,
            commands::dictionary_export,
            commands::open_path_in_explorer,
        ])
        .build(tauri::generate_context!())
        .expect("启动 NavKit 失败")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit | tauri::RunEvent::ExitRequested { .. }) {
                if let Some(host) = app.try_state::<scheduler_desktop::SchedulerHost>() { host.request_stop(); }
            }
        });
}
