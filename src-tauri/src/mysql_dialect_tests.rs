//! TiDB 方言回归：`START TRANSACTION READ ONLY` 在 TiDB 默认是 noop 占位（1235）。
use super::mysql_read_only_unsupported;

#[test]
fn tidb_is_detected_from_server_identity() {
    // TiDB 的 @@version 形如 `8.0.11-TiDB-v7.5.0`，社区版也带 `-TiDB-`。
    assert!(mysql_read_only_unsupported("8.0.11-TiDB-v7.5.0", ""));
    assert!(mysql_read_only_unsupported(
        "5.7.25-TiDB-v4.0.16",
        "TiDB Server (Apache License 2.0) Community Edition, MySQL 5.7 compatible"
    ));
    // 代理/兼容层可能只把产品名放在 version_comment 里。
    assert!(mysql_read_only_unsupported("8.0.11", "TiDB Server (Apache License 2.0)"));
}

#[test]
fn mysql_and_mariadb_keep_read_only_transactions() {
    assert!(!mysql_read_only_unsupported("8.0.36", "MySQL Community Server - GPL"));
    assert!(!mysql_read_only_unsupported("8.4.0", "MySQL Community Server - GPL"));
    assert!(!mysql_read_only_unsupported("10.11.6-MariaDB", "mariadb.org binary distribution"));
    assert!(!mysql_read_only_unsupported("8.0.32-0ubuntu0.22.04.2", ""));
    assert!(!mysql_read_only_unsupported("", ""));
}

#[test]
fn read_only_transaction_call_sites_probe_the_server_dialect() {
    // 打开表设计（table_workflows_native）与旧原生读/数据对比共用同一判定；
    // 任一路径漏掉探测都会在 TiDB 上退回 1235。
    for (name, source) in [
        ("table_workflows_native.rs", include_str!("table_workflows_native.rs")),
        ("table_native.rs", include_str!("table_native.rs")),
        ("sync_diff.rs", include_str!("sync_diff.rs")),
    ] {
        assert!(source.contains("mysql_read_only_unsupported"), "{name} 必须识别 TiDB 并跳过 READ ONLY 事务");
    }
}

#[test]
fn table_metadata_probes_dialect_capabilities_before_reading() {
    let source = include_str!("table_workflows_native_metadata.rs");
    // MySQL 族：server_uuid / 物理表 ID 来源都按实际目录探测，不能硬编码 MySQL 8 的表。
    for marker in ["MysqlEvidence::probe", "INNODB_SYS_TABLES", "TIDB_TABLES", "SHOW VARIABLES LIKE 'server_uuid'"] {
        assert!(source.contains(marker), "缺少方言探测：{marker}");
    }
    // PG 族：版本相关列必须探测后降级，否则 openGauss/GaussDB 会在 attgenerated/pg_sequence 上 42703。
    for marker in ["PgEvidence::probe", "attgenerated:has(", "indnkeyatts:has(", "evidence.sequences"] {
        assert!(source.contains(marker), "缺少 PG 能力探测：{marker}");
    }
    // 读取不能无条件引用 TiDB 不存在的 INNODB_TABLES / PG 9.x 不存在的 pg_sequence。
    assert!(!source.contains("let innodb=s.rows(&format!(\"SELECT CAST(TABLE_ID"));
}
