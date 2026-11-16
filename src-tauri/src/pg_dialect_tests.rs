//! PG 协议族方言回归：openGauss/GaussDB 的 B(MySQL) 兼容模式与 PostgreSQL 语义不同。
use super::pg_mysql_compat_mode;

#[test]
fn opengauss_mysql_compat_modes_are_recognized() {
    // A=Oracle、B=MySQL、PG=PostgreSQL；B/M/MYSQL 需要按 MySQL 语义处理。
    for mode in ["B", "b", "M", "m", "MYSQL", " mysql "] {
        assert!(pg_mysql_compat_mode(mode), "{mode} 应识别为 MySQL 兼容模式");
    }
    for mode in ["A", "PG", "ORA", "postgresql", " a ", ""] {
        assert!(!pg_mysql_compat_mode(mode), "{mode} 不应识别为 MySQL 兼容模式");
    }
}

#[test]
fn table_design_refuses_mysql_compat_and_ships_compat_sql_layers() {
    let source = include_str!("table_workflows_native_metadata.rs");
    // 读到 B 模式必须明确拒绝，不能按 PG 语义生成错误 SQL。
    assert!(source.contains("TABLE_PG_MYSQL_COMPAT_MODE_UNSUPPORTED"));
    assert!(source.contains("evidence.mysql_compat"));
    // 必须同时具备索引兼容层与列 information_schema 兜底层。
    assert!(source.contains("pg_index_compat_sql"));
    assert!(source.contains("pg_columns_information_schema"));
}

#[test]
fn sequence_metadata_falls_back_without_pg_sequence() {
    // openGauss/GaussDB 没有 pg_sequence，对象目录的序列属性要用 information_schema.sequences。
    let source = include_str!("object_catalog.rs");
    assert!(source.contains("has_pg_sequence"));
    assert!(source.contains("information_schema.sequences"));
}
