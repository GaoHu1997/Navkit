//! MySQL 例程读取：类型来自目录或精确身份，绝不通过错误 SQL 探测类型。
use anyhow::{anyhow, bail, Result};
use sqlx::{Column, Row};
use crate::{db, error::AppError, models::DbKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoutineKind { Function, Procedure }
impl RoutineKind {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        if value.eq_ignore_ascii_case("function") { Ok(Self::Function) }
        else if value.eq_ignore_ascii_case("procedure") { Ok(Self::Procedure) }
        else { Err(AppError::coded("ROUTINE_KIND_INVALID", format!("仅支持函数或存储过程，收到未知类型：{value}")).into()) }
    }
    pub(crate) fn keyword(self) -> &'static str { match self { Self::Function => "FUNCTION", Self::Procedure => "PROCEDURE" } }
    fn definition_column(self) -> &'static str { match self { Self::Function => "Create Function", Self::Procedure => "Create Procedure" } }
}

/// SHOW CREATE 的列位置不是契约；只读取对应类型的命名定义列，不解码无关列。
pub(crate) async fn definition(pool: &sqlx::MySqlPool, kind: &DbKind, schema: &str, name: &str, routine: RoutineKind) -> Result<String> {
    let sql = format!("SHOW CREATE {} {}", routine.keyword(), db::qualified(kind, schema, name));
    let row = crate::sql_history::query(&sql).fetch_optional(pool).await?
        .ok_or_else(|| AppError::coded("ROUTINE_NOT_FOUND", format!("{} {}.{} 不存在或不可见", routine.keyword(), schema, name)))?;
    let column = routine.definition_column();
    let index = row.columns().iter().position(|c| c.name().eq_ignore_ascii_case(column))
        .ok_or_else(|| AppError::coded("ROUTINE_DEFINITION_UNREADABLE", format!("服务器未返回 {column} 定义列")))?;
    let text = match row.try_get::<Option<String>, _>(index) {
        Ok(text) => text,
        Err(_) => row.try_get::<Option<Vec<u8>>, _>(index)?.map(String::from_utf8).transpose()?,
    };
    text.filter(|text| !text.trim().is_empty()).ok_or_else(|| AppError::coded("ROUTINE_DEFINITION_UNREADABLE", format!("{} {}.{} 的定义不可读取；请检查权限，未跳过该对象", routine.keyword(), schema, name)).into())
}

/// 备份旧契约只有名字，必须先查真实类型；同名函数/过程全部保留，不猜其中一个。
pub(crate) async fn definitions(pool: &sqlx::MySqlPool, kind: &DbKind, schema: &str, name: &str) -> Result<Vec<(String, String)>> {
    let rows = crate::sql_history::query("SELECT CAST(ROUTINE_TYPE AS CHAR) AS routine_type FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA=? AND ROUTINE_NAME=? ORDER BY ROUTINE_TYPE")
        .bind(schema).bind(name).fetch_all(pool).await?;
    if rows.is_empty() { bail!(AppError::coded("ROUTINE_NOT_FOUND", format!("例程 {schema}.{name} 不存在或当前用户不可见；备份未完成"))); }
    let mut found = Vec::new();
    for row in rows {
        let value = match row.try_get::<String, _>("routine_type") {
            Ok(value) => value,
            Err(_) => String::from_utf8(row.try_get::<Vec<u8>, _>("routine_type")?)?,
        };
        let routine = RoutineKind::parse(&value)?;
        if found.iter().any(|(keyword, _): &(String, String)| keyword == routine.keyword()) { return Err(anyhow!("ROUTINE_METADATA_AMBIGUOUS: 目录返回重复例程类型")); }
        found.push((routine.keyword().into(), definition(pool, kind, schema, name, routine).await?));
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routine_kind_is_explicit_and_never_defaults_unknown_to_function() {
        for value in ["function", "FUNCTION", "Function"] { assert_eq!(RoutineKind::parse(value).unwrap(), RoutineKind::Function); }
        for value in ["procedure", "PROCEDURE", "Procedure"] { assert_eq!(RoutineKind::parse(value).unwrap(), RoutineKind::Procedure); }
        for value in ["", "trigger", "event", "sequence", "function; DROP TABLE x", " function"] { assert!(RoutineKind::parse(value).is_err()); }
        assert_eq!(RoutineKind::Function.keyword(), "FUNCTION");
        assert_eq!(RoutineKind::Function.definition_column(), "Create Function");
        assert_eq!(RoutineKind::Procedure.definition_column(), "Create Procedure");
    }
}
