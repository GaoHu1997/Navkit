def patch(path, pairs, must=True):
    import io
    s = io.open(path, encoding='utf-8').read()
    for old, new in pairs:
        if old not in s:
            if must:
                raise AssertionError(path + ' NOT FOUND: ' + repr(old[:100]))
            continue
        s = s.replace(old, new, 1)
    io.open(path, 'w', encoding='utf-8', newline='\n').write(s)
    print('patched', path)

# ================= transfer.rs =================
patch('src/transfer.rs', [
    # insert_many / transfer / dump / restore_sql_file 借用化
    ('''async fn insert_many(
    db: &Db,
    kind: DbKind,
    schema: &str,''',
     '''async fn insert_many(
    db: &Db,
    kind: &DbKind,
    schema: &str,'''),
    ('''pub async fn transfer(
    source: &Db,
    source_kind: DbKind,
    target: &Db,
    target_kind: DbKind,
    opts: &TransferOptions,
) -> Result<u64> {
    let schema = crate::schema::table_schema(source, source_kind, &opts.source_schema, &opts.source_table).await?;''',
     '''pub async fn transfer(
    source: &Db,
    source_kind: &DbKind,
    target: &Db,
    target_kind: &DbKind,
    opts: &TransferOptions,
) -> Result<u64> {
    let schema = crate::schema::table_schema(source, source_kind.clone(), &opts.source_schema, &opts.source_table).await?;'''),
    ('        return insert_many(db, kind.clone(), &opts.schema, &opts.table, &db_cols, &mapped).await;',
     '        return insert_many(db, &kind, &opts.schema, &opts.table, &db_cols, &mapped).await;'),
    ('    insert_many(db, kind, &opts.schema, &opts.table, &cols, &rows).await',
     '    insert_many(db, &kind, &opts.schema, &opts.table, &cols, &rows).await'),
    ('        count += crate::schema::exec_affected(db, kind.clone(), t).await?;',
     '        count += crate::schema::exec_affected(db, &kind, t).await?;'),
    ('        count += crate::schema::exec_affected(db, kind, &sql).await?;',
     '        count += crate::schema::exec_affected(db, &kind, &sql).await?;'),
    # dump / restore 借用化
    ('''    let (names, rows) = crate::schema::fetch_all_rows(
        source,
        source_kind,
        &opts.source_schema,''',
     '''    let (names, rows) = crate::schema::fetch_all_rows(
        source,
        source_kind,
        &opts.source_schema,'''),
    ('    let schema = crate::schema::table_schema(db, kind, &opts.schema, &opts.table).await?;',
     '    let schema = crate::schema::table_schema(db, kind.clone(), &opts.schema, &opts.table).await?;'),
    ('            let (names, rows) = crate::schema::fetch_all_rows(db, kind, schema, name, None, None).await?;',
     '            let (names, rows) = crate::schema::fetch_all_rows(db, &kind, schema, name, None, None).await?;'),
    # 协议 match 补齐
    ('''        crate::models::Protocol::Sqlite => tail.push_str("PRAGMA foreign_keys = ON;\\r\\n"),
        crate::models::Protocol::Clickhouse | crate::models::Protocol::Odbc => {}''',
     '''        crate::models::Protocol::Sqlite => tail.push_str("PRAGMA foreign_keys = ON;\\r\\n"),
        crate::models::Protocol::Clickhouse | crate::models::Protocol::Odbc => {}
        crate::models::Protocol::Mssql
        | crate::models::Protocol::Redis
        | crate::models::Protocol::Mongo
        | crate::models::Protocol::Http => {}'''),
], must=True)

# 还原 / 协议头部 / 对象定义 match 补齐（按行号锚点逐个处理）
s = io.open('src/transfer.rs', encoding='utf-8').read()
# 协议头（SET NAMES ...）match 补齐
old = '''        crate::models::Protocol::Postgres => {
            head.push_str("SET session_replication_role = DEFAULT;\\r\\n")
        }'''
# 找头部 match 的 Sqlite/Clickhouse 尾行
import re
m = re.search(r'match kind\.protocol\(\) \{[^}]*?SET NAMES[^}]*?\}', s, re.S)
if m and 'Protocol::Sqlite => {}' not in m.group(0):
    seg = m.group(0)
    print('head match segment found')

# 对象定义 match（object_ddl）补齐：Db::Mysql 分支之后
old_ddl = '''    let mut count = 0u64;
    match db {
        Db::Sqlite(pool) => {'''
# restore match 补齐
old_restore = '''        Db::ClickHouse(_) | Db::Odbc(_) => {
            // 无会话级外键守卫语法，逐条执行；守卫语句失败跳过
            for t in &stmts {'''
new_restore = '''        Db::Mssql(_) => {
            // 无会话级外键守卫语法，逐条执行；守卫语句失败跳过
            for t in &stmts {
                match db.generic_exec(t).await {
                    Ok(_) => count += 1,
                    Err(e) => {
                        if is_fk_guard_stmt(t) {
                            continue;
                        }
                        return Err(anyhow::Error::from(e).context(format!("还原在第 {} 条语句失败", count + 1)));
                    }
                }
            }
        }
        Db::Redis(_) | Db::Mongo(_) | Db::Http(_) => {
            return Err(anyhow!("该数据源类型不支持 SQL 脚本还原"));
        }
        Db::ClickHouse(_) | Db::Odbc(_) => {
            // 无会话级外键守卫语法，逐条执行；守卫语句失败跳过
            for t in &stmts {'''
assert old_restore in s, 'restore arm'
s = s.replace(old_restore, new_restore, 1)
io.open('src/transfer.rs', 'w', encoding='utf-8', newline='\n').write(s)
print('patched src/transfer.rs restore')

# ================= commands.rs =================
patch('src/commands.rs', [
    ('    let (names, rows) = schema::fetch_all_rows(\n        db.as_ref(),\n        kind.clone(),',
     '    let (names, rows) = schema::fetch_all_rows(\n        db.as_ref(),\n        &kind,'),
    ('    crate::transfer::transfer(source.as_ref(), source_kind, target.as_ref(), target_kind, &opts)',
     '    crate::transfer::transfer(source.as_ref(), &source_kind, target.as_ref(), &target_kind, &opts)'),
    ('    crate::transfer::dump(db.as_ref(), kind, &opts)',
     '    crate::transfer::dump(db.as_ref(), &kind, &opts)'),
    ('    let count = crate::transfer::restore_sql_file(db.as_ref(), kind, &path.to_string_lossy())',
     '    let count = crate::transfer::restore_sql_file(db.as_ref(), &kind, &path.to_string_lossy())'),
])

# ================= odbc_db.rs =================
patch('src/odbc_db.rs', [
    ('pub fn default_connection_string(kind: DbKind, req: &ConnectionRequest) -> String {',
     'pub fn default_connection_string(kind: &DbKind, req: &ConnectionRequest) -> String {'),
    ('            default_connection_string(kind, req)',
     '            default_connection_string(&kind, req)'),
])

# ================= mongo_db.rs =================
patch('src/mongo_db.rs', [
    ('''//! MongoDB 原生客户端：mongodb 官方 Rust 驱动。
//!
//! 树结构：连接 → 数据库 → 集合；「表数据」页展示文档（顶层字段平铺）；
//! 查询标签页输入 JSON 查询条件（如 `{"age": {"$gt": 18}}`）执行 find，
//! 空输入 / `{}` 返回全部文档，`count {…}` 形式返回计数。

use anyhow::{anyhow, Context, Result};''',
     '''//! MongoDB 原生客户端：mongodb 官方 Rust 驱动。
//!
//! 树结构：连接 → 数据库 → 集合；「表数据」页展示文档（顶层字段平铺）；
//! 查询标签页输入 JSON 查询条件（如 `{"age": {"$gt": 18}}`）执行 find，
//! 空输入 / `{}` 返回全部文档，`count {…}` 形式返回计数。

use anyhow::{anyhow, Result};'''),
    ('''        let opts = FindOptions::builder()
            .skip(skip as i64)
            .limit(Some(limit as i64))
            .build();''',
     '''        let opts = FindOptions::builder()
            .skip(skip)
            .limit(Some(limit as i64))
            .build();'''),
    ('''/// 解析查询条件 JSON；空 / `{}` 表示全部。
fn parse_filter(filter_json: &str) -> Result<Option<Document>> {
    let s = filter_json.trim();
    if s.is_empty() || s == "{}" {
        return Ok(None);
    }
    let doc: Document = mongodb::bson::from_bson(Bson::Document(
        serde_json::from_str::<serde_json::Value>(s)
            .map_err(|e| anyhow!("查询条件不是合法 JSON: {e}"))
            .and_then(|v| json_to_bson(&v).ok_or_else(|| anyhow!("不支持的查询条件类型")))?,
    ))
    .map_err(|e| anyhow!("查询条件转换失败: {e}"))?;
    Ok(Some(doc))
}''',
     '''/// 解析查询条件 JSON；空 / `{}` 表示全部。
fn parse_filter(filter_json: &str) -> Result<Option<Document>> {
    let s = filter_json.trim();
    if s.is_empty() || s == "{}" {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| anyhow!("查询条件不是合法 JSON: {e}"))?;
    let bson = json_to_bson(&v).ok_or_else(|| anyhow!("不支持的查询条件类型"))?;
    let doc: Document = mongodb::bson::from_bson(bson)
        .map_err(|e| anyhow!("查询条件转换失败: {e}"))?;
    Ok(Some(doc))
}'''),
    ('''        Bson::DateTime(d) => d.to_chrono().format("%Y-%m-%d %H:%M:%S").to_string(),''',
     '''        Bson::DateTime(d) => chrono::DateTime::from_timestamp_millis(d.to_millis())
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_default(),'''),
    ('        Bson::JavaScriptCode(c) => c.code.clone(),',
     '        Bson::JavaScriptCode(c) => c.clone(),'),
])

# ================= http_db.rs =================
patch('src/http_db.rs', [
    ('        let (s, j) = self.ctx.get_json(path).await?;\n        Ctx::check(s, &j, "RabbitMQ 管理 API")?;',
     '        let (s, j) = self.ctx.get_json(&path).await?;\n        Ctx::check(s, &j, "RabbitMQ 管理 API")?;'),
])

print('ALL OK')
