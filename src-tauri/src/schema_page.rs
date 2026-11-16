//! 分页探测契约：无 COUNT 时 total 仅是已证明下界，不能由越界 offset 推测总数。
use crate::models::{DbKind, PageRequest, ResultColumn};

/// SQLite's unqualified lookup prefers temp over main. Resolve the selected real
/// namespace explicitly, keeping the metadata path's display-label fallback.
pub(super) async fn sqlite_full(pool: &sqlx::SqlitePool, schema: &str, table: &str) -> anyhow::Result<String> {
    let rows = crate::sql_history::query("PRAGMA database_list").fetch_all(pool).await?;
    let attached: Vec<String> = rows.iter().filter_map(|row| {
        crate::values::sqlite_row_values(row, 3).get(1).cloned().flatten()
    }).collect();
    let namespace = if schema.is_empty() || !attached.iter().any(|name| name == schema) { "main" } else { schema };
    let kind = DbKind::of("sqlite");
    Ok(format!("{}.{}", crate::db::quote_ident(&kind, namespace), crate::db::quote_ident(&kind, table)))
}


pub(super) fn fetch_limit(req: &PageRequest) -> u64 {
    let limit = req.limit.min(100_000);
    if limit == 0 { 0 } else { limit + u64::from(!req.include_total) }
}

pub(super) fn finish<T>(req: &PageRequest, rows: &mut Vec<T>, counted: Option<u64>) -> (u64, bool, Option<bool>) {
    let limit = req.limit.min(100_000);
    if limit == 0 {
        rows.clear();
        return counted.map(|total| (total, true, None)).unwrap_or((0, false, None));
    }
    let more = rows.len() as u64 > limit;
    rows.truncate(limit as usize);
    if let Some(total) = counted {
        return (total, true, Some(req.offset.saturating_add(rows.len() as u64) < total));
    }
    let n = rows.len() as u64;
    if n == 0 && req.offset > 0 {
        return (0, false, Some(false));
    }
    let total = req.offset.saturating_add(n).saturating_add(u64::from(more));
    // 已拿到不足一页的非空尾页（或首个空页）才足以证明总数。
    (total, !more, Some(more))
}

/// 单列唯一主键 + 有效锚点时，生成 Keyset 谓词与用于取下一页锚点的排序列名；
/// 否则返回 None，调用方继续用 OFFSET。锚点已定位起点，调用方据此忽略 OFFSET。
pub(super) fn keyset_for_page(
    kind: &DbKind,
    req: &PageRequest,
    meta_pks: &[String],
    meta_cols: &[ResultColumn],
) -> Option<(String, String)> {
    let after = req.after.as_ref()?;
    let rules = req.order_rules.as_ref()?;
    if rules.len() != 1 {
        return None;
    }
    let rule = &rules[0];
    if meta_pks.len() != 1 || meta_pks[0] != rule.column {
        return None;
    }
    let after_value = after.first()?;
    let numeric = meta_cols
        .iter()
        .find(|c| c.name == rule.column)
        .map(|c| is_numeric_type(&c.data_type))
        .unwrap_or(false);
    let col = crate::db::quote_ident(kind, &rule.column);
    let predicate =
        crate::query_pagination::keyset_predicate(&col, rule.ascending, after_value.as_deref(), numeric)?;
    Some((predicate, rule.column.clone()))
}

pub(crate) fn is_numeric_type(data_type: &str) -> bool {
    let t = data_type.to_ascii_uppercase();
    [
        "INT", "BIGINT", "SMALLINT", "TINYINT", "MEDIUMINT", "DECIMAL", "NUMERIC", "FLOAT",
        "DOUBLE", "REAL", "BIT", "SERIAL", "NUMBER",
    ]
    .iter()
    .any(|k| t.contains(k))
}

/// 各 SQL 方言共用同一探测行预算；无 OFFSET 的 TOP/Hive 由调用者跳过前缀。
pub(super) fn rows_sql(kind: &DbKind, req: &PageRequest, full: &str, where_sql: &str, order_sql: &str) -> String {
    let limit = fetch_limit(req);
    if limit == 0 {
        let empty_where = if where_sql.is_empty() { " WHERE 1=0".to_string() } else { format!("{} AND 1=0", where_sql) };
        return format!("SELECT * FROM {}{}", full, empty_where);
    }
    if matches!(kind.id(), "sqlserver" | "mssql" | "oracle" | "dameng" | "db2") {
        let order = if order_sql.is_empty() && matches!(kind.id(), "sqlserver" | "mssql") { " ORDER BY (SELECT NULL)" } else { order_sql };
        format!("SELECT * FROM {}{}{} OFFSET {} ROWS FETCH NEXT {} ROWS ONLY", full, where_sql, order, req.offset, limit)
    } else if kind.id() == "firebird" {
        format!("SELECT FIRST {} SKIP {} * FROM {}{}{}", limit, req.offset, full, where_sql, order_sql)
    } else if kind.id() == "informix" {
        format!("SELECT SKIP {} FIRST {} * FROM {}{}{}", req.offset, limit, full, where_sql, order_sql)
    } else if matches!(kind.id(), "access" | "iris" | "cache") {
        format!("SELECT TOP {} * FROM {}{}{}", req.offset.saturating_add(limit), full, where_sql, order_sql)
    } else if matches!(kind.id(), "hive" | "inceptor" | "argodb" | "spark" | "impala" | "kyuubi") {
        format!("SELECT * FROM {}{}{} LIMIT {}", full, where_sql, order_sql, req.offset.saturating_add(limit))
    } else {
        format!("SELECT * FROM {}{}{} LIMIT {} OFFSET {}", full, where_sql, order_sql, limit, req.offset)
    }
}

#[cfg(test)]
#[path = "schema_page_tests.rs"]
mod tests;
