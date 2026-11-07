//! ClickHouse HTTP 响应解析与方言 SQL 生成的单元测试（不依赖服务器）。

use navkit_lib::clickhouse::parse_json_compact;

#[test]
fn parse_json_compact_basic() {
    // JSONCompactEachRowWithNamesAndTypes：第 1 行列名、第 2 行类型、其余数据
    let data = "[\"id\",\"name\",\"score\"]\n[\"UInt32\",\"String\",\"Nullable(Float64)\"]\n[1,\"alice\",95.5]\n[2,\"bob\",null]\n[3,\"中文\",88]\n".as_bytes();
    let res = parse_json_compact(data).unwrap();
    assert_eq!(res.columns, vec!["id", "name", "score"]);
    assert_eq!(res.rows.len(), 3);
    assert_eq!(res.rows[0], vec![
        Some("1".into()),
        Some("alice".into()),
        Some("95.5".into())
    ]);
    // NULL 保持 None
    assert_eq!(res.rows[1][2], None);
    // 数字类型文本化；非 ASCII 字符正常透传
    assert_eq!(res.rows[2][1], Some("中文".into()));
    assert_eq!(res.rows[2][2], Some("88".into()));
}

#[test]
fn parse_json_compact_empty_result() {
    // 无结果集（空输出）
    let res = parse_json_compact(b"").unwrap();
    assert!(res.columns.is_empty());
    assert!(res.rows.is_empty());
}

#[test]
fn parse_json_compact_bool_and_date() {
    let data = br#"["flag","d"]
["Bool","Date"]
[true,"2026-11-04"]
"#;
    let res = parse_json_compact(data).unwrap();
    assert_eq!(res.rows[0][0], Some("1".into()));
    assert_eq!(res.rows[0][1], Some("2026-11-04".into()));
}

#[test]
fn parse_json_compact_rejects_garbage() {
    let res = parse_json_compact(b"[not json");
    assert!(res.is_err(), "损坏的响应应返回错误");
}

#[test]
fn dialect_catalog_sql() {
    // 各方言的目录查询 SQL 与标识符引用
    let ch = navkit_lib::db::quote_ident(&navkit_lib::models::DbKind::of("clickhouse"), "db`x");
    assert_eq!(ch, "`db``x`");
    let hive = navkit_lib::db::quote_ident(&navkit_lib::models::DbKind::of("hive"), "t1");
    assert_eq!(hive, "`t1`");
    let dm = navkit_lib::db::quote_ident(&navkit_lib::models::DbKind::of("dameng"), "T1");
    assert_eq!(dm, "\"T1\"");
    // qualified：SQLite 忽略 schema；其余带 schema
    let q = navkit_lib::db::qualified(&navkit_lib::models::DbKind::of("dameng"), "MAIN", "T");
    assert_eq!(q, "\"MAIN\".\"T\"");
}
