//! SQLite 整库备份 / 还原往返测试（自建隔离夹具，不依赖未跟踪的 demo.db）。

use navkit_lib::db::{self, Db};

mod common;
use common::close_db;
use navkit_lib::models::{ConnectionRequest, DbKind};

fn sqlite_req(file: &str) -> ConnectionRequest {
    ConnectionRequest {
        kind: DbKind::of("sqlite"),
        host: String::new(),
        port: 0,
        user: String::new(),
        password: String::new(),
        database: String::new(),
        file: file.to_string(),
        params: String::new(),
        timeout_secs: 10,
    }
}

async fn demo_db(file: &std::path::Path) -> Db {
    let db = db::open(&sqlite_req(&file.to_string_lossy())).await.unwrap();
    let Db::Sqlite(pool) = &db else { panic!("sqlite only") };
    sqlx::raw_sql(
        "CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT NOT NULL); \
         CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id)); \
         CREATE VIEW active_users AS SELECT id,name FROM customers; \
         WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM ids WHERE id<25) \
         INSERT INTO customers SELECT id,'客户-' || id FROM ids; \
         WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM ids WHERE id<50) \
         INSERT INTO orders SELECT id,(id-1)%25+1 FROM ids;"
    ).execute(pool).await.unwrap();
    db
}

async fn count_rows(db: &Db, table: &str) -> i64 {
    match db {
        Db::Sqlite(pool) => {
            let sql = format!("SELECT COUNT(*) FROM \"{}\"", table);
            sqlx::query_scalar(&sql).fetch_one(pool).await.unwrap()
        }
        _ => panic!("sqlite only"),
    }
}

#[tokio::test]
async fn sqlite_dump_and_restore_roundtrip() {
    let dir = std::env::temp_dir().join(format!("navkit-bk-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // 1. 源库：独立生成相同规模的确定性数据，干净检出也可复现。
    let src = dir.join("src.db");
    let src_db = demo_db(&src).await;

    // 2. 整库备份（带 schema 名，验证 qualified() 对 SQLite 忽略 schema 的修复）
    let dump_file = dir.join("demo_20261103_000000.sql");
    let (rows, bytes) = navkit_lib::transfer::dump_database(
        &src_db,
        &DbKind::of("sqlite"),
        "demo", // SQLite 下该 schema 名必须被忽略
        &[],
        &[], // 例程：SQLite 无例程
        true,
        true,
        &dump_file.to_string_lossy(),
        "往返测试",
    )
    .await
    .unwrap();

    assert_eq!(rows, 75, "应转储 customers(25) + orders(50) 共 75 行");
    assert!(bytes > 1000, "转储文件不应为空");
    let text = std::fs::read_to_string(&dump_file).unwrap();
    assert!(text.contains("CREATE TABLE"), "应包含建表语句");
    assert!(
        text.to_uppercase().contains("INSERT INTO"),
        "应包含 INSERT 语句"
    );
    assert!(text.contains("CREATE VIEW"), "应包含视图定义: {}", {
        let i = text.find("active_users").map(|x| &text[x.saturating_sub(40)..x + 60]);
        format!("{:?}", i)
    });
    assert!(text.contains("PRAGMA foreign_keys = OFF"), "应包含外键守卫");
    assert!(text.contains("往返测试"), "应包含注释头");

    // 3. 还原到全新空库
    let dst = dir.join("dst.db");
    let dst_db = db::open(&sqlite_req(&dst.to_string_lossy())).await.unwrap();
    let restored = navkit_lib::transfer::restore_sql_file(
        &dst_db,
        &DbKind::of("sqlite"),
        &dump_file.to_string_lossy(),
    )
    .await
    .unwrap();
    assert!(restored >= 3, "至少执行 DROP/CREATE/INSERT 若干条，实际 {}", restored);

    // 4. 数据一致
    assert_eq!(count_rows(&dst_db, "customers").await, 25);
    assert_eq!(count_rows(&dst_db, "orders").await, 50);

    // 5. 视图存在且可查询
    match &dst_db {
        Db::Sqlite(pool) => {
            let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM active_users")
                .fetch_one(pool)
                .await
                .expect("还原后的视图应可查询");
            assert!(n >= 0);
        }
        _ => panic!("sqlite only"),
    }

    // 6. 清理（先显式关池，Windows 下打开中的 .db 删不掉）
    close_db(&src_db).await;
    close_db(&dst_db).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sqlite_view_definition_readable() {
    let dir = std::env::temp_dir().join(format!("navkit-view-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("src.db");
    let db = demo_db(&src).await;
    let def = navkit_lib::schema::view_definition(
        &db,
        DbKind::of("sqlite"),
        "demo",
        "active_users",
    )
    .await
    .unwrap();
    assert!(
        def.to_uppercase().contains("CREATE VIEW"),
        "视图定义应包含 CREATE VIEW：{}",
        def
    );
    close_db(&db).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn restore_tolerates_fk_guard_failures() {
    let dir = std::env::temp_dir().join(format!("navkit-bk-guard-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    // 构造一个包含 PG 专用守卫语句的 SQL 文件：SQLite 上 SET 会失败但应被跳过
    let sql_file = dir.join("guard.sql");
    std::fs::write(
        &sql_file,
        "SET session_replication_role = replica;\r\n\
         CREATE TABLE \"t1\" (\"id\" INTEGER PRIMARY KEY);\r\n\
         INSERT INTO \"t1\" (\"id\") VALUES (1);\r\n",
    )
    .unwrap();

    let empty = dir.join("empty.db");
    let db = db::open(&sqlite_req(&empty.to_string_lossy())).await.unwrap();
    let n = navkit_lib::transfer::restore_sql_file(&db, &DbKind::of("sqlite"), &sql_file.to_string_lossy())
        .await
        .unwrap();
    assert_eq!(n, 2, "守卫语句失败应跳过，成功执行 2 条");
    assert_eq!(count_rows(&db, "t1").await, 1);

    close_db(&db).await;
    let _ = std::fs::remove_dir_all(&dir);
}
