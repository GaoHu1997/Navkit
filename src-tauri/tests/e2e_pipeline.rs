//! 端到端管线测试：连接 → 建表 → 分页读取 → 行编辑 → 查询。
//!
//! 覆盖本应用最核心的可执行性断言：
//! 1. 空表 page_table 必须返回列结构（否则网格无表头、无法插入）。
//! 2. execute 对 0 行 SELECT 必须返回带列名的结果集。
//! 3. 行编辑支持 NULL 主键（IS NULL 条件）。
//! 4. MySQL 连接必须真的发送密码（对真实服务器验证 using password: YES）。

use navkit_lib::db::{self, DbRegistry};
use navkit_lib::models::*;
use std::collections::HashMap;

mod common;
use common::{close_db, mysql_env_creds, shutdown_registry, tcp_probe};
use navkit_lib::db::Db as Db2;

fn sqlite_req(file: &std::path::Path) -> ConnectionRequest {
    ConnectionRequest {
        kind: DbKind::of("sqlite"),
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: file.to_string_lossy().to_string(),
        params: String::new(),
        timeout_secs: 10,
    }
}

async fn registry_with(req: &ConnectionRequest) -> (DbRegistry, &'static str) {
    // DbRegistry = RwLock<HashMap<...>>（AppState 负责再包一层 Arc）
    let reg: DbRegistry = parking_lot::RwLock::new(HashMap::new());
    db::open_and_register(&reg, "t1", req).await.expect("open connection");
    (reg, "t1")
}

fn page_req(schema: &str, table: &str) -> PageRequest {
    PageRequest {
        connection_id: "t1".into(),
        schema: schema.into(),
        table: table.into(),
        offset: 0,
        limit: 50,
        order_by: None,
        ascending: true,
        filters: Vec::new(),
        filter_logic: navkit_lib::models::FilterLogic::And,
        order_rules: None,
    }
}

#[tokio::test]
async fn sqlite_empty_table_has_columns_and_supports_insert() {
    let dir = std::env::temp_dir().join(format!("nk_e2e_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("empty.db");

    let req = sqlite_req(&file);
    let (reg, id) = registry_with(&req).await;

    // 建一张完全空白的表
    db::execute(&reg, id, "CREATE TABLE t_empty (id INTEGER PRIMARY KEY, name TEXT, val REAL)", DbKind::of("sqlite"))
        .await
        .expect("create table");

    // 关键断言 1：空表也必须有列结构
    let page = navkit_lib::schema::page_table(
        db::get_db(&reg, id).unwrap().as_ref(),
        DbKind::of("sqlite"),
        &page_req("", "t_empty"),
    )
    .await
    .expect("page empty table");
    assert_eq!(page.total, 0, "空表 total 应为 0");
    assert_eq!(page.columns.len(), 3, "空表必须有列结构");
    assert_eq!(page.columns[0].name, "id");
    assert_eq!(page.columns[0].primary_key, true, "id 应标记为主键");
    assert_eq!(page.columns[1].name, "name");
    assert_eq!(page.primary_keys, vec!["id".to_string()], "必须给出主键列名");

    // 关键断言 2：空表可以直接插入
    let n = navkit_lib::schema::insert_row(
        db::get_db(&reg, id).unwrap().as_ref(),
        DbKind::of("sqlite"),
        &RowInsert {
            connection_id: id.into(),
            schema: String::new(),
            table: "t_empty".into(),
            columns: vec!["id".into(), "name".into(), "val".into()],
            values: vec![Some("1".into()), Some("张三".into()), Some("3.25".into())],
        },
    )
    .await
    .expect("insert into empty table");
    assert_eq!(n, 1);

    let page = navkit_lib::schema::page_table(
        db::get_db(&reg, id).unwrap().as_ref(),
        DbKind::of("sqlite"),
        &page_req("", "t_empty"),
    )
    .await
    .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.rows[0][1].as_deref(), Some("张三"), "中文必须原样读出");
    assert_eq!(page.rows[0][2].as_deref(), Some("3.25"), "REAL 不应变成 3");

    shutdown_registry(&reg).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sqlite_zero_row_select_keeps_columns() {
    let dir = std::env::temp_dir().join(format!("nk_e2e_z{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("z.db");
    let req = sqlite_req(&file);
    let (reg, id) = registry_with(&req).await;

    db::execute(&reg, id, "CREATE TABLE z (a INTEGER, b TEXT)", DbKind::of("sqlite")).await.unwrap();
    db::execute(&reg, id, "INSERT INTO z VALUES (1,'x'),(2,'y')", DbKind::of("sqlite")).await.unwrap();

    // 0 行 SELECT：结果集不能凭空消失
    let res = db::execute(&reg, id, "SELECT a, b FROM z WHERE a > 99", DbKind::of("sqlite"))
        .await
        .expect("execute");
    assert_eq!(res.sets.len(), 1, "0 行 SELECT 必须仍有结果集");
    assert_eq!(res.sets[0].columns.len(), 2, "0 行结果集必须有列名");
    assert_eq!(res.sets[0].columns[0].name, "a");
    assert_eq!(res.sets[0].rows.len(), 0);

    // 前置注释的查询应被识别为查询（裸括号 (SELECT..) 是 MySQL/PG 语法，SQLite 不支持）
    let res = db::execute(&reg, id, "/* 说明 */ SELECT a FROM z LIMIT 1", DbKind::of("sqlite"))
        .await
        .expect("commented select");
    assert_eq!(res.sets.len(), 1);
    assert_eq!(res.sets[0].rows.len(), 1);

    shutdown_registry(&reg).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sqlite_row_editing_with_null_pk() {
    let dir = std::env::temp_dir().join(format!("nk_e2e_n{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("n.db");
    let req = sqlite_req(&file);
    let (reg, id) = registry_with(&req).await;
    let pool = db::get_db(&reg, id).unwrap();

    db::execute(&reg, id, "CREATE TABLE n (k TEXT PRIMARY KEY, v TEXT)", DbKind::of("sqlite")).await.unwrap();
    db::execute(&reg, id, "INSERT INTO n (k, v) VALUES ('a', '1'), ('b', '2')", DbKind::of("sqlite")).await.unwrap();
    // 一行主键为 NULL
    db::execute(&reg, id, "INSERT INTO n (v) VALUES ('nullkey')", DbKind::of("sqlite")).await.unwrap();

    // 更新 NULL 主键行：WHERE k IS NULL（此前 = NULL 永远不匹配）
    let n = navkit_lib::schema::update_row(
        pool.as_ref(),
        DbKind::of("sqlite"),
        &RowUpdate {
            connection_id: id.into(),
            schema: String::new(),
            table: "n".into(),
            primary_keys: vec![("k".into(), None)],
            changes: vec![("v".into(), Some("fixed".into()))],
            old_values: Vec::new(),
        },
    )
    .await
    .expect("update null-pk row");
    assert_eq!(n, 1, "NULL 主键行必须能被更新");

    // 删除 NULL 主键行
    let n = navkit_lib::schema::delete_row(
        pool.as_ref(),
        DbKind::of("sqlite"),
        "",
        "n",
        &[("k".into(), None)],
    )
    .await
    .expect("delete null-pk row");
    assert_eq!(n, 1, "NULL 主键行必须能被删除");

    // 带引号/特殊字符的值
    let n = navkit_lib::schema::update_row(
        pool.as_ref(),
        DbKind::of("sqlite"),
        &RowUpdate {
            connection_id: id.into(),
            schema: String::new(),
            table: "n".into(),
            primary_keys: vec![("k".into(), Some("a".into()))],
            changes: vec![("v".into(), Some("it''s \"ok\"".into()))],
            old_values: Vec::new(),
        },
    )
    .await
    .unwrap();
    assert_eq!(n, 1);
    let page = navkit_lib::schema::page_table(pool.as_ref(), DbKind::of("sqlite"), &page_req("", "n"))
        .await
        .unwrap();
    let row_a = page.rows.iter().find(|r| r[0].as_deref() == Some("a")).unwrap();
    assert_eq!(row_a[1].as_deref(), Some("it''s \"ok\""));

    shutdown_registry(&reg).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sqlite_pagination_and_sort() {
    let dir = std::env::temp_dir().join(format!("nk_e2e_p{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("p.db");
    let req = sqlite_req(&file);
    let (reg, id) = registry_with(&req).await;
    let pool = db::get_db(&reg, id).unwrap();

    db::execute(&reg, id, "CREATE TABLE p (id INTEGER PRIMARY KEY, n INTEGER)", DbKind::of("sqlite")).await.unwrap();
    let mut ins = String::new();
    for i in 1..=120 {
        ins.push_str(&format!("INSERT INTO p (n) VALUES ({});", 120 - i));
    }
    db::execute(&reg, id, &ins, DbKind::of("sqlite")).await.unwrap();

    let mut pr = page_req("", "p");
    pr.limit = 50;
    let page = navkit_lib::schema::page_table(pool.as_ref(), DbKind::of("sqlite"), &pr)
        .await
        .unwrap();
    assert_eq!(page.total, 120, "total 是全表行数");
    assert_eq!(page.rows.len(), 50, "本页 50 行");
    assert_eq!(page.rows[0][1].as_deref(), Some("119"), "默认按 rowid 升序");

    // 翻页 + 倒序
    let mut pr2 = page_req("", "p");
    pr2.limit = 50;
    pr2.offset = 50;
    pr2.order_by = Some("n".into());
    pr2.ascending = false;
    let page2 = navkit_lib::schema::page_table(pool.as_ref(), DbKind::of("sqlite"), &pr2)
        .await
        .unwrap();
    assert_eq!(page2.rows[0][1].as_deref(), Some("69"), "倒序第 51 条");
    assert_eq!(page2.columns[1].name, "n");

    // 筛选
    let mut pr3 = page_req("", "p");
    pr3.filters = vec![navkit_lib::models::FilterLine {
        logic: navkit_lib::models::FilterLogic::And,
        conds: vec![FilterCondition {
            column: "n".into(),
            operator: ">=".into(),
            value: "118".into(),
            case_sensitive: false,
            logic: None,
        }],
    }];
    let page3 = navkit_lib::schema::page_table(pool.as_ref(), DbKind::of("sqlite"), &pr3)
        .await
        .unwrap();
    assert_eq!(page3.total, 2);

    shutdown_registry(&reg).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn is_query_recognition() {
    assert!(db::is_query("SELECT 1"));
    assert!(db::is_query("  select * from t"));
    assert!(db::is_query("-- 注释\nSELECT 1"));
    assert!(db::is_query("/* xx */ SELECT 1"));
    assert!(db::is_query("(SELECT 1)"));
    assert!(db::is_query("WITH a AS (SELECT 1) SELECT * FROM a"));
    assert!(db::is_query("SHOW TABLES"));
    assert!(db::is_query("PRAGMA table_info(t)"));
    assert!(!db::is_query("UPDATE t SET a = 1"));
    assert!(!db::is_query("INSERT INTO t VALUES (1)"));
    assert!(!db::is_query("SELECTED nope"));
    assert!(!db::is_query("   "));
    assert!(!db::is_query("-- 只有一行注释"));
}

#[test]
fn statement_splitting() {
    let parts = db::split_statements("SELECT 'a;b' FROM t; INSERT INTO t VALUES ('x'); -- c; d\nSELECT 2;");
    assert_eq!(parts.len(), 3, "字符串与注释内的分号不得切分: {:?}", parts);
    assert!(parts[0].contains("'a;b'"));
}

/// 真实 MySQL：密码必须真的被发送。
/// 本机 MySQL 8.0 的 root 有密码（值未知），所以用任意密码连接必然 Access denied；
/// 服务器回显 (using password: YES) 即证明密码已随连接发送。
/// 修复前此处永远回显 (using password: NO)。
#[tokio::test]
async fn mysql_transmits_password() {
    if !tcp_probe(3306) {
        eprintln!("本机 3306 无 MySQL，跳过（需要真实服务器验证密码传输）");
        return;
    }
    let req = ConnectionRequest {
        kind: DbKind::of("mysql"),
        host: "127.0.0.1".into(),
        port: 3306,
        user: "root".into(),
        password: "definitely-not-the-password".into(),
        database: String::new(),
        file: String::new(),
        params: String::new(),
        timeout_secs: 5,
    };
    match db::open(&req).await {
        Ok(_) => panic!("错误密码不应连接成功"),
        Err(e) => {
            let msg = format!("{:#}", e);
            assert!(
                msg.contains("using password: YES"),
                "密码必须随连接发送（应为 using password: YES），实际：{}",
                msg
            );
        }
    }
}

/// 密码加密往返：encrypt -> decrypt 必须还原明文（安全敏感路径，零覆盖曾被验收标记 P1）。
/// 注意：master_key 落在 ./data/key.bin，测试前后保持原状。
#[test]
fn password_encrypt_decrypt_roundtrip() {
    let key_path = std::env::current_dir().unwrap().join("data").join("key.bin");
    let saved = std::fs::read(&key_path).ok();

    for plain in ["s3cret!", "", "中文密码123", "with 'quote' and \"double\""] {
        let enc = navkit_lib::storage::encrypt_password(plain);
        if plain.is_empty() {
            assert_eq!(enc.unwrap(), "", "空密码按约定返回空串");
            continue;
        }
        let enc = enc.unwrap();
        assert!(!enc.contains(plain), "密文不得包含明文");
        let dec = navkit_lib::storage::decrypt_password(&enc).unwrap();
        assert_eq!(dec, plain, "往返必须还原明文: {}", plain);
    }

    // 坏输入：fail-closed 返回明确错误而不是 panic 或伪明文。
    assert!(navkit_lib::storage::decrypt_password("not-base64!!").is_err(), "坏 base64 必须报错");
    assert!(navkit_lib::storage::decrypt_password("QUJD").is_err(), "密文长度不足必须报错");

    // 测试首次创建的 key.bin 清理；已存在的保留
    if saved.is_none() {
        let _ = std::fs::remove_file(&key_path);
    }
}

/// 单表 SQL 转储 → 还原回环（带主键表）：验证 PRIMARY KEY 位于列定义括号内、
/// 前导注释不吞掉 DROP / SET 语句（验收 #1 发现的三处 DDL 错位与还原跳过）。
#[tokio::test]
async fn sqlite_dump_restore_with_pk_and_comments() {
    let dir = std::env::temp_dir().join(format!("nk_e2e_d{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("d.db");
    let req = sqlite_req(&file);
    let (reg, id) = registry_with(&req).await;

    db::execute(
        &reg,
        id,
        "CREATE TABLE dept (id INTEGER PRIMARY KEY, name TEXT NOT NULL, budget REAL DEFAULT 0)",
        DbKind::of("sqlite"),
    )
    .await
    .unwrap();
    db::execute(&reg, id, "INSERT INTO dept (name, budget) VALUES ('研发', 100.5), ('市场', 80.25)", DbKind::of("sqlite"))
        .await
        .unwrap();

    let dump_file = dir.join("dept_dump.sql");
    let rows = navkit_lib::transfer::dump(
        db::get_db(&reg, id).unwrap().as_ref(),
        &DbKind::of("sqlite"),
        &DumpOptions {
            connection_id: id.into(),
            schema: String::new(),
            table: "dept".into(),
            file: dump_file.to_string_lossy().to_string(),
            data_only: false,
            structure_only: false,
            include_drop: true,
        },
    )
    .await
    .expect("dump");
    assert_eq!(rows, 2);
    let bytes = std::fs::metadata(&dump_file).unwrap().len();
    assert!(bytes > 0);

    let text = std::fs::read_to_string(&dump_file).unwrap();
    // PRIMARY KEY 必须在 CREATE TABLE 的列定义括号内（括号外是语法错误）。
    // 取从 CREATE TABLE 到结尾 ");" 的完整语句块做判断。
    let start = text
        .to_uppercase()
        .find("CREATE TABLE")
        .expect("应包含 CREATE TABLE");
    let block = &text[start..];
    let end = block.find(");").expect("CREATE TABLE 块应有收尾");
    let block = &block[..end];
    assert!(
        block.to_uppercase().contains("PRIMARY KEY"),
        "PRIMARY KEY 应在列定义括号块内: {}",
        block
    );
    // 旧 bug 特征：PRIMARY KEY 出现在右括号之后
    assert!(
        !block.to_uppercase().contains("
),"),
        "列定义括号不应在 PRIMARY KEY 之前闭合: {}",
        block
    );

    // 还原到全新库：文件带注释头 + DROP，注释不得吞掉语句
    let dst = dir.join("dst.db");
    let (reg2, id2) = {
        let r: DbRegistry = parking_lot::RwLock::new(HashMap::new());
        db::open_and_register(&r, "t2", &sqlite_req(&dst)).await.unwrap();
        (r, "t2")
    };
    let n = navkit_lib::transfer::restore_sql_file(
        db::get_db(&reg2, id2).unwrap().as_ref(),
        &DbKind::of("sqlite"),
        &dump_file.to_string_lossy(),
    )
    .await
    .expect("restore");
    assert!(n >= 3, "至少执行 DROP/CREATE/INSERT，实际 {}", n);
    let pool = db::get_db(&reg2, id2).unwrap();
    let cnt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dept")
        .fetch_one(match pool.as_ref() { Db2::Sqlite(p) => p, _ => panic!("sqlite only") })
        .await
        .unwrap();
    assert_eq!(cnt, 2, "两行数据都应还原");

    common::shutdown_registry(&reg).await;
    common::shutdown_registry(&reg2).await;
    let _ = std::fs::remove_dir_all(&dir);
}

/// 紧凑写法识别：SELECT*FROM 不带空格也应被视为查询。
#[test]
fn is_query_compact_forms() {
    assert!(db::is_query("SELECT*FROM t"));
    assert!(db::is_query("SELECT'x'"));
    assert!(!db::is_query("SELECTED"));
    assert!(!db::is_query("SET NAMES utf8mb4"));
}
