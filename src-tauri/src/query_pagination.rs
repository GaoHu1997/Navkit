//! 查询页服务器分页 SQL 规划 + 导出可重跑判定 + 表数据 Keyset 谓词。
//!
//! 设计思路借鉴 dbx（https://github.com/t8y2/dbx，Apache-2.0）的分页规划：
//! 只对“能证明安全”的单条 SELECT/WITH 生成服务器分页 SQL；其余返回
//! `Unsupported`，由调用方回退到原有“执行一次 + 完整缓存”路径，绝不猜测结构。
//!
//! 覆盖 NavKit 已完整支持、且使用标准 `LIMIT/OFFSET` 语法的方言
//! （SQLite / MySQL / PostgreSQL / ClickHouse）。SQL Server、ODBC、JDBC 等
//! 暂不前置，返回 `Unsupported` 走回退路径。
//!
//! 分页 SQL 只用于“查询页显示当前页”，不是全量导出的数据源；全量导出走
//! 导出侧的重新查询路径（`export_query_requery`），两者不可混用。

use crate::models::{DbKind, Protocol};
use sqlparser::ast::{LimitClause, SetExpr, Statement};
use sqlparser::dialect::{
    ClickHouseDialect, Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect,
};
use sqlparser::parser::Parser;

/// 每页最多行数，与前端分页上限保持一致。
pub const MAX_PAGE_ROWS: u64 = 100_000;

/// 分页规划结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PagePlan {
    /// 生成成功：可直接执行的当前页 SQL。
    Page(String),
    /// 不能安全分页：调用方应回退到原有执行路径，并向用户说明原因。
    Unsupported(&'static str),
}

impl PagePlan {
    pub fn into_sql(self) -> Option<String> {
        match self {
            PagePlan::Page(sql) => Some(sql),
            PagePlan::Unsupported(_) => None,
        }
    }
    pub fn reason(&self) -> Option<&'static str> {
        match self {
            PagePlan::Page(_) => None,
            PagePlan::Unsupported(reason) => Some(reason),
        }
    }
}

fn dialect_for(kind: &DbKind) -> Option<Box<dyn Dialect>> {
    match kind.protocol() {
        Protocol::Sqlite => Some(Box::new(SQLiteDialect {})),
        Protocol::Mysql => Some(Box::new(MySqlDialect {})),
        Protocol::Postgres => Some(Box::new(PostgreSqlDialect {})),
        Protocol::Clickhouse => Some(Box::new(ClickHouseDialect {})),
        _ => None,
    }
}

/// MySQL `/*! ... */`、MariaDB `/*M! ... */`、优化器提示 `/*+ ... */`。
fn has_semantic_comment(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            let mut j = i + 2;
            while j < bytes.len() && bytes[j] == b' ' {
                j += 1;
            }
            if j < bytes.len() {
                if matches!(bytes[j], b'!' | b'+') {
                    return true;
                }
                if bytes[j] == b'M' && j + 1 < bytes.len() && bytes[j + 1] == b'!' {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

fn set_expr_has_into(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(select) => select.into.is_some(),
        SetExpr::Query(query) => set_expr_has_into(query.body.as_ref()),
        SetExpr::SetOperation { left, right, .. } => {
            set_expr_has_into(left.as_ref()) || set_expr_has_into(right.as_ref())
        }
        _ => false,
    }
}

/// 为当前页规划可执行 SQL。
///
/// - `limit`/`offset` 来自前端分页请求；`limit` 被夹在 `1..=MAX_PAGE_ROWS`。
/// - 只有单条、非 `SELECT INTO`、不含可执行注释的 SELECT/WITH 才会改写。
pub fn plan_query_page(kind: &DbKind, sql: &str, limit: u64, offset: u64) -> PagePlan {
    let limit = limit.clamp(1, MAX_PAGE_ROWS);
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return PagePlan::Unsupported("SQL 为空");
    }
    let Some(dialect) = dialect_for(kind) else {
        return PagePlan::Unsupported("当前数据库类型暂不支持查询页服务器分页");
    };
    if has_semantic_comment(trimmed) {
        return PagePlan::Unsupported("SQL 含可执行注释或优化器提示，保持原样执行");
    }
    let statements = match Parser::parse_sql(dialect.as_ref(), trimmed) {
        Ok(statements) => statements,
        Err(_) => return PagePlan::Unsupported("无法解析为单条语句"),
    };
    let [statement] = statements.as_slice() else {
        return PagePlan::Unsupported("仅支持单条语句的分页浏览");
    };
    let query = match statement {
        Statement::Query(query) => query,
        _ => return PagePlan::Unsupported("仅 SELECT / WITH 查询可分页"),
    };
    if let Some(LimitClause::LimitOffset { limit_by, .. }) = &query.limit_clause {
        if !limit_by.is_empty() {
            return PagePlan::Unsupported("LIMIT ... BY 不走通用分页");
        }
    }
    if set_expr_has_into(query.body.as_ref()) {
        return PagePlan::Unsupported("含 SELECT INTO，不能分页执行");
    }
    let has_top_limit = query.limit_clause.is_some() || query.fetch.is_some();
    // `to_string` 重新生成语句，杜绝尾随注释把 LIMIT 吞掉的隐患；
    // 可执行注释已在上面回退，普通注释丢失不影响执行语义。
    let body = statement.to_string();
    if has_top_limit {
        PagePlan::Page(wrap_with_limit(&body, limit, offset))
    } else {
        PagePlan::Page(with_limit(&body, limit, offset))
    }
}

/// 查询页 Keyset 的形状：单表 SELECT + 单列 ORDER BY。
///
/// 只有这种形状才可能在“排序列是唯一键”时安全改用锚点定位；其余一律回退 OFFSET。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeysetShape {
    pub schema: String,
    pub table: String,
    pub column: String,
    pub ascending: bool,
}

/// 分析查询页 SQL 是否具备 Keyset 形状（纯结构判断，不访问数据库）。
pub fn analyze_keyset(kind: &DbKind, sql: &str) -> Option<KeysetShape> {
    let dialect = dialect_for(kind)?;
    let trimmed = sql.trim();
    if trimmed.is_empty() || has_semantic_comment(trimmed) { return None; }
    let statements = Parser::parse_sql(dialect.as_ref(), trimmed).ok()?;
    let [Statement::Query(query)] = statements.as_slice() else { return None; };
    if query.limit_clause.is_some() || query.fetch.is_some() { return None; }
    let SetExpr::Select(select) = query.body.as_ref() else { return None; };
    if select.distinct.is_some() || select.group_by != sqlparser::ast::GroupByExpr::Expressions(Vec::new(), Vec::new())
        || select.having.is_some() || !select.lateral_views.is_empty() { return None; }
    // 投影必须是纯列引用（出现表达式/聚合就无法保证 ORDER BY 列可作锚点）。
    let only_columns = select.projection.iter().all(|item| matches!(
        item, sqlparser::ast::SelectItem::UnnamedExpr(sqlparser::ast::Expr::Identifier(_))
            | sqlparser::ast::SelectItem::UnnamedExpr(sqlparser::ast::Expr::CompoundIdentifier(_))
            | sqlparser::ast::SelectItem::Wildcard(_)
    ));
    if !only_columns { return None; }
    // FROM 必须是单个普通表（可带 schema）。
    let [table_with_joins] = select.from.as_slice() else { return None; };
    if !table_with_joins.joins.is_empty() { return None; }
    let sqlparser::ast::TableFactor::Table { name, .. } = &table_with_joins.relation else { return None; };
    if name.0.len() > 2 { return None; }
    let parts: Vec<String> = name.0.iter()
        .map(|part| part.to_string().trim_matches('"').trim_matches('`').to_string())
        .collect();
    let (schema, table) = match parts.as_slice() {
        [table] => (String::new(), table.clone()),
        [schema, table] => (schema.clone(), table.clone()),
        _ => return None,
    };
    let sqlparser::ast::OrderByKind::Expressions(order_exprs) = &query.order_by.as_ref()?.kind else { return None; };
    let [order_by_expr] = order_exprs.as_slice() else { return None; };
    let column = match &order_by_expr.expr {
        sqlparser::ast::Expr::Identifier(ident) => ident.value.clone(),
        sqlparser::ast::Expr::CompoundIdentifier(idents) => idents.last()?.value.clone(),
        _ => return None,
    };
    Some(KeysetShape { schema, table, column, ascending: order_by_expr.options.asc != Some(false) })
}

/// 在已有单表 SELECT 上追加 Keyset 谓词并限页。
///
/// 谓词并入原 WHERE（而不是外层子查询），才能让数据库用唯一键索引定位；
/// `after` 已是安全字面量（由 `keyset_predicate` 生成），不再二次拼接用户输入。
pub fn plan_keyset_page(kind: &DbKind, sql: &str, predicate: &str, limit: u64) -> Option<String> {
    let limit = limit.clamp(1, MAX_PAGE_ROWS);
    let dialect = dialect_for(kind)?;
    let mut statements = Parser::parse_sql(dialect.as_ref(), sql.trim()).ok()?;
    let [Statement::Query(query)] = statements.as_mut_slice() else { return None; };
    let SetExpr::Select(select) = query.body.as_mut() else { return None; };
    let extra = Parser::parse_sql(dialect.as_ref(), &format!("SELECT 1 WHERE {predicate}"))
        .ok()?
        .into_iter()
        .find_map(|statement| match statement {
            Statement::Query(inner) => match *inner.body {
                SetExpr::Select(inner_select) => inner_select.selection,
                _ => None,
            },
            _ => None,
        })?;
    select.selection = Some(match select.selection.take() {
        Some(existing) => sqlparser::ast::Expr::BinaryOp {
            left: Box::new(existing),
            op: sqlparser::ast::BinaryOperator::And,
            right: Box::new(extra),
        },
        None => extra,
    });
    let body = Statement::Query(query.clone()).to_string();
    Some(with_limit(&body, limit, 0))
}

/// 导出“重新查询全量”前的可重跑判定。
///
/// 只有能证明安全的**单条只读 SELECT/WITH** 才允许导出时重新执行；
/// 其余（写语句、多语句、SELECT INTO、含可执行注释、无法解析）一律拒绝，
/// 由调用方提示“仅可导出当前页/已加载行”，绝不静默全量重跑。
///
/// 注意：SQL 文本无法证明 SELECT 内调用的函数是否产生副作用（如自增序列、
/// 写日志的 UDF）。因此重新执行导出仍属“最佳努力”，调用方需向用户提示
/// “导出会重新执行查询，结果可能与页面不同，且要求语句本身可安全重跑”。
pub fn rerunnable_read(kind: &DbKind, sql: &str) -> Result<(), &'static str> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err("SQL 为空");
    }
    let Some(dialect) = dialect_for(kind) else {
        return Err("当前数据库类型不支持重新查询导出");
    };
    if has_semantic_comment(trimmed) {
        return Err("SQL 含可执行注释或优化器提示，不能重新执行");
    }
    let statements = Parser::parse_sql(dialect.as_ref(), trimmed)
        .map_err(|_| "无法解析为单条语句，不能重新执行导出")?;
    let [statement] = statements.as_slice() else {
        return Err("仅支持单条只读语句的重新查询导出");
    };
    match statement {
        Statement::Query(query) => {
            if set_expr_has_into(query.body.as_ref()) {
                return Err("含 SELECT INTO，不能重新执行导出");
            }
            Ok(())
        }
        _ => Err("仅只读 SELECT/WITH 查询可重新执行导出"),
    }
}

/// 单列唯一键的 Keyset 分页谓词。
///
/// `column` 必须是调用方已正确 quote 的列名；`numeric` 来自列类型元数据，
/// 决定值是否裸拼（数字）或按字符串字面量转义（文本/日期）。
/// 返回 `None` 表示不能安全生成（NULL 锚点、非数字值配数字列、空值），
/// 调用方应回退到 OFFSET 分页，绝不生成可能注入或类型错误的谓词。
pub fn keyset_predicate(
    column: &str,
    ascending: bool,
    value: Option<&str>,
    numeric: bool,
) -> Option<String> {
    let raw = value?;
    let literal = if numeric {
        if !is_numeric_literal(raw) {
            return None;
        }
        raw.to_string()
    } else {
        format!("'{}'", raw.replace('\'', "''"))
    };
    let op = if ascending { ">" } else { "<" };
    Some(format!("{column} {op} {literal}"))
}

fn is_numeric_literal(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    let mut dots = 0;
    let mut digits = 0;
    for (index, ch) in value.chars().enumerate() {
        match ch {
            '-' | '+' if index == 0 => {}
            '.' => dots += 1,
            c if c.is_ascii_digit() => digits += 1,
            _ => return false,
        }
    }
    digits > 0 && dots <= 1
}

fn strip_semi(sql: &str) -> &str {
    sql.trim().trim_end_matches(';').trim_end()
}

fn with_limit(body: &str, limit: u64, offset: u64) -> String {
    let body = strip_semi(body);
    if offset == 0 {
        format!("{} LIMIT {};", body, limit)
    } else {
        format!("{} LIMIT {} OFFSET {};", body, limit, offset)
    }
}

fn wrap_with_limit(body: &str, limit: u64, offset: u64) -> String {
    let body = strip_semi(body);
    if offset == 0 {
        format!("SELECT * FROM ({}) AS navkit_page LIMIT {};", body, limit)
    } else {
        format!(
            "SELECT * FROM ({}) AS navkit_page LIMIT {} OFFSET {};",
            body, limit, offset
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite() -> DbKind {
        DbKind::of("sqlite")
    }
    fn mysql() -> DbKind {
        DbKind::of("mysql")
    }
    fn postgres() -> DbKind {
        DbKind::of("postgresql")
    }

    fn page(plan: PagePlan) -> String {
        match plan {
            PagePlan::Page(sql) => sql,
            PagePlan::Unsupported(reason) => panic!("expected page, got unsupported: {reason}"),
        }
    }

    #[test]
    fn simple_select_gets_first_page_limit() {
        let sql = page(plan_query_page(&sqlite(), "SELECT * FROM orders", 100, 0));
        assert_eq!(sql, "SELECT * FROM orders LIMIT 100;");
    }

    #[test]
    fn select_appends_offset_for_later_pages() {
        let sql = page(plan_query_page(&mysql(), "SELECT id FROM users", 50, 200));
        assert_eq!(sql, "SELECT id FROM users LIMIT 50 OFFSET 200;");
    }

    #[test]
    fn user_limit_is_preserved_by_wrapping() {
        let sql = page(plan_query_page(&postgres(), "SELECT * FROM events LIMIT 1000", 100, 0));
        assert_eq!(
            sql,
            "SELECT * FROM (SELECT * FROM events LIMIT 1000) AS navkit_page LIMIT 100;"
        );
    }

    #[test]
    fn user_limit_with_offset_wraps_with_offset() {
        let sql = page(plan_query_page(&mysql(), "SELECT * FROM t LIMIT 500", 100, 200));
        assert_eq!(
            sql,
            "SELECT * FROM (SELECT * FROM t LIMIT 500) AS navkit_page LIMIT 100 OFFSET 200;"
        );
    }

    #[test]
    fn union_query_paginates_directly() {
        let direct = page(plan_query_page(
            &mysql(),
            "SELECT id FROM a UNION SELECT id FROM b",
            10,
            0,
        ));
        assert_eq!(direct, "SELECT id FROM a UNION SELECT id FROM b LIMIT 10;");
    }

    #[test]
    fn select_into_is_refused() {
        let plan = plan_query_page(&mysql(), "SELECT id INTO backup FROM users", 100, 0);
        assert!(matches!(plan, PagePlan::Unsupported(_)));
    }

    #[test]
    fn multiple_statements_are_refused() {
        let plan = plan_query_page(&sqlite(), "SELECT 1; SELECT 2", 100, 0);
        assert!(matches!(plan, PagePlan::Unsupported(_)));
    }

    #[test]
    fn writes_are_refused() {
        let plan = plan_query_page(&mysql(), "UPDATE users SET active = 0", 100, 0);
        assert!(matches!(plan, PagePlan::Unsupported(_)));
    }

    #[test]
    fn executable_comment_is_refused() {
        let plan = plan_query_page(&mysql(), "SELECT /*! SQL_NO_CACHE */ * FROM t", 100, 0);
        assert!(matches!(plan, PagePlan::Unsupported(_)));
    }

    #[test]
    fn optimizer_hint_is_refused() {
        let plan = plan_query_page(&postgres(), "SELECT /*+ SeqScan(t) */ * FROM t", 100, 0);
        assert!(matches!(plan, PagePlan::Unsupported(_)));
    }

    #[test]
    fn ordinary_comment_is_regenerated_without_it() {
        let sql = page(plan_query_page(&mysql(), "SELECT id FROM t -- note\n", 100, 0));
        assert!(sql.ends_with("LIMIT 100;"));
        assert!(sql.contains("FROM t"));
    }

    #[test]
    fn unsupported_protocol_falls_back() {
        let plan = plan_query_page(&DbKind::of("oracle"), "SELECT * FROM t", 100, 0);
        assert!(matches!(plan, PagePlan::Unsupported(_)));
    }

    #[test]
    fn limit_is_clamped_to_max_page_rows() {
        let sql = page(plan_query_page(&sqlite(), "SELECT * FROM t", 999_999, 0));
        assert_eq!(sql, format!("SELECT * FROM t LIMIT {};", MAX_PAGE_ROWS));
    }

    #[test]
    fn empty_sql_is_refused() {
        assert!(matches!(plan_query_page(&sqlite(), "   ", 100, 0), PagePlan::Unsupported(_)));
    }

    #[test]
    fn rerunnable_read_accepts_simple_select_and_cte() {
        assert!(rerunnable_read(&sqlite(), "SELECT * FROM t").is_ok());
        assert!(rerunnable_read(&mysql(), "WITH x AS (SELECT 1 AS a) SELECT * FROM x").is_ok());
    }

    #[test]
    fn rerunnable_read_rejects_writes_and_multiple_statements() {
        assert!(rerunnable_read(&mysql(), "UPDATE t SET a = 1").is_err());
        assert!(rerunnable_read(&sqlite(), "SELECT 1; SELECT 2").is_err());
        assert!(rerunnable_read(&postgres(), "DELETE FROM t").is_err());
    }

    #[test]
    fn rerunnable_read_rejects_select_into_and_executable_comment() {
        assert!(rerunnable_read(&mysql(), "SELECT a INTO b FROM t").is_err());
        assert!(rerunnable_read(&mysql(), "SELECT /*! SQL_NO_CACHE */ * FROM t").is_err());
    }

    #[test]
    fn keyset_numeric_passes_through() {
        assert_eq!(
            keyset_predicate("\"id\"", true, Some("42"), true).as_deref(),
            Some("\"id\" > 42")
        );
        assert_eq!(
            keyset_predicate("\"id\"", false, Some("42"), true).as_deref(),
            Some("\"id\" < 42")
        );
    }

    #[test]
    fn keyset_text_is_quoted_and_escaped() {
        assert_eq!(
            keyset_predicate("\"name\"", true, Some("O'Brien"), false).as_deref(),
            Some("\"name\" > 'O''Brien'")
        );
    }

    #[test]
    fn keyset_refuses_null_and_non_numeric_on_numeric_column() {
        assert!(keyset_predicate("\"id\"", true, None, true).is_none());
        assert!(keyset_predicate("\"id\"", true, Some("1 OR 1=1"), true).is_none());
        assert!(keyset_predicate("\"id\"", true, Some("2.5.5"), true).is_none());
        assert!(keyset_predicate("\"id\"", true, Some("-7"), true).is_some());
    }

    #[test]
    fn analyze_keyset_accepts_simple_single_table_ordered_query() {
        let shape = analyze_keyset(&mysql(), "SELECT id, name FROM users ORDER BY id").unwrap();
        assert_eq!(shape.table, "users");
        assert!(shape.schema.is_empty());
        assert_eq!(shape.column, "id");
        assert!(shape.ascending);
    }

    #[test]
    fn analyze_keyset_reads_desc_and_schema_qualification() {
        let shape = analyze_keyset(&postgres(), "SELECT * FROM sales.orders ORDER BY id DESC").unwrap();
        assert_eq!((shape.schema.as_str(), shape.table.as_str()), ("sales", "orders"));
        assert!(!shape.ascending);
    }

    #[test]
    fn analyze_keyset_rejects_shapes_it_cannot_prove_safe() {
        // 多表 / JOIN / 聚合 / 多列排序 / 已有 LIMIT / 表达式投影 都不做锚点定位。
        for sql in [
            "SELECT * FROM a, b ORDER BY id",
            "SELECT * FROM a JOIN b ON a.id=b.id ORDER BY a.id",
            "SELECT count(*) FROM a ORDER BY 1",
            "SELECT * FROM a ORDER BY id, name",
            "SELECT * FROM a ORDER BY id LIMIT 10",
            "SELECT id+1 FROM a ORDER BY id",
            "SELECT * FROM a",
        ] {
            assert!(analyze_keyset(&mysql(), sql).is_none(), "应拒绝: {sql}");
        }
    }

    #[test]
    fn plan_keyset_page_merges_predicate_into_where() {
        let sql = plan_keyset_page(&mysql(), "SELECT id, name FROM users ORDER BY id", "\"id\" > 100", 50).unwrap();
        assert!(sql.contains("\"id\" > 100"), "应包含锚点谓词: {sql}");
        assert!(sql.to_uppercase().contains("LIMIT 50"), "应限页: {sql}");
        assert!(!sql.to_uppercase().contains("OFFSET"), "锚点定位不应带 OFFSET: {sql}");
    }

    #[test]
    fn plan_keyset_page_keeps_existing_where() {
        let sql = plan_keyset_page(&postgres(), "SELECT * FROM t WHERE active = true ORDER BY id",
            "\"id\" > 5", 20).unwrap();
        assert!(sql.contains("active = true"), "原 WHERE 必须保留: {sql}");
        assert!(sql.contains("\"id\" > 5"), "锚点谓词必须并入: {sql}");
    }
}
