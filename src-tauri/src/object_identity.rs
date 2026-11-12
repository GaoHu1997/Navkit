//! SQL Server tree identities: a selected catalog plus separately quoted schema/name.
use anyhow::{bail, Result};
use crate::db::{quote_ident, quote_literal};
use crate::models::DbKind;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlServerObject { pub catalog: String, pub schema: String, pub name: String }
impl SqlServerObject {
    pub fn parse(catalog: &str, object: &str) -> Result<Self> {
        if catalog.is_empty() || catalog.contains('\0') { bail!("请选择真实 SQL Server 数据库"); }
        let parts = sqlserver_parts(object)?;
        let (schema, name) = match parts.as_slice() {
            [_] => bail!("SQL Server 对象缺少真实 schema，请从当前数据库目录重新选择"),
            [schema, name] => (schema.clone(), name.clone()),
            _ => bail!("SQL Server 对象身份必须是 schema 和 name，不允许跨库覆盖选定数据库"),
        };
        Ok(Self { catalog: catalog.into(), schema, name })
    }
    pub fn qualified(&self, kind: &DbKind) -> String { [self.catalog.as_str(), self.schema.as_str(), self.name.as_str()].map(|part| quote_ident(kind, part)).join(".") }
    pub fn columns_query(&self, kind: &DbKind) -> String {
        format!("SELECT COLUMN_NAME, CASE WHEN DATA_TYPE = 'timestamp' THEN 'rowversion' ELSE DATA_TYPE END AS DATA_TYPE, IS_NULLABLE, COLUMN_DEFAULT FROM {}.INFORMATION_SCHEMA.COLUMNS WHERE TABLE_CATALOG = N{} AND TABLE_SCHEMA = N{} AND TABLE_NAME = N{} ORDER BY ORDINAL_POSITION", quote_ident(kind, &self.catalog), quote_literal(&self.catalog), quote_literal(&self.schema), quote_literal(&self.name))
    }
    pub fn primary_key_query(&self, kind: &DbKind) -> String {
        let catalog = quote_ident(kind, &self.catalog);
        format!("SELECT ku.COLUMN_NAME FROM {catalog}.INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc JOIN {catalog}.INFORMATION_SCHEMA.KEY_COLUMN_USAGE ku ON tc.CONSTRAINT_CATALOG = ku.CONSTRAINT_CATALOG AND tc.CONSTRAINT_SCHEMA = ku.CONSTRAINT_SCHEMA AND tc.CONSTRAINT_NAME = ku.CONSTRAINT_NAME AND tc.TABLE_CATALOG = ku.TABLE_CATALOG AND tc.TABLE_SCHEMA = ku.TABLE_SCHEMA AND tc.TABLE_NAME = ku.TABLE_NAME WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY' AND tc.TABLE_CATALOG = N{} AND tc.TABLE_SCHEMA = N{} AND tc.TABLE_NAME = N{} ORDER BY ku.ORDINAL_POSITION", quote_literal(&self.catalog), quote_literal(&self.schema), quote_literal(&self.name))
    }
    pub fn view_query(&self, kind: &DbKind) -> String {
        let catalog = quote_ident(kind, &self.catalog);
        format!("SELECT m.definition FROM {catalog}.sys.views v JOIN {catalog}.sys.schemas s ON s.schema_id = v.schema_id JOIN {catalog}.sys.sql_modules m ON m.object_id = v.object_id WHERE s.name = N{} AND v.name = N{}", quote_literal(&self.schema), quote_literal(&self.name))
    }
    pub fn type_query(&self, kind: &DbKind) -> String {
        format!("SELECT o.type FROM {}.sys.objects o JOIN {}.sys.schemas s ON s.schema_id = o.schema_id WHERE s.name = N{} AND o.name = N{} AND o.type IN ('U','V')", quote_ident(kind, &self.catalog), quote_ident(kind, &self.catalog), quote_literal(&self.schema), quote_literal(&self.name))
    }
}
pub fn sqlserver_object_label(kind: &DbKind, schema: &str, name: &str) -> String { format!("{}.{}", quote_ident(kind, schema), quote_ident(kind, name)) }
pub fn sqlserver_list_query(kind: &DbKind, catalog: &str) -> Result<String> {
    if catalog.is_empty() || catalog.contains('\0') { bail!("请选择真实 SQL Server 数据库"); }
    let catalog = quote_ident(kind, catalog);
    Ok(format!("SELECT s.name, o.name, o.type FROM {catalog}.sys.objects o JOIN {catalog}.sys.schemas s ON s.schema_id = o.schema_id WHERE o.type IN ('U','V') AND o.is_ms_shipped = 0 ORDER BY s.name, o.name"))
}
fn sqlserver_parts(value: &str) -> Result<Vec<String>> {
    if value.is_empty() || value.contains('\0') { bail!("SQL Server 对象名不能为空或含空字符"); }
    let mut input = value.chars().peekable(); let mut parts = Vec::new();
    loop {
        let mut part = String::new();
        if input.peek() == Some(&'[') {
            input.next(); let mut closed = false;
            while let Some(ch) = input.next() {
                if ch == ']' {
                    if input.peek() == Some(&']') { input.next(); part.push(']'); }
                    else { closed = true; break; }
                } else { part.push(ch); }
            }
            if !closed { bail!("SQL Server 对象引用未闭合"); }
        } else {
            while input.peek().is_some_and(|ch| *ch != '.') { part.push(input.next().unwrap()); }
        }
        if part.is_empty() { bail!("SQL Server 对象身份包含空段"); }
        parts.push(part);
        match input.next() { None => break, Some('.') => { if input.peek().is_none() { bail!("SQL Server 对象身份包含空段"); } }, _ => bail!("SQL Server 对象身份引用后存在多余内容") }
    }
    Ok(parts)
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn selected_catalog_schema_and_name_are_not_quoted_as_one_identifier() {
        let kind = DbKind::of("sqlserver");
        let object = SqlServerObject::parse("selected.db", "[sales].[Order.Detail]]Archive]").unwrap();
        assert_eq!((&object.catalog, &object.schema, &object.name), (&"selected.db".to_string(), &"sales".to_string(), &"Order.Detail]Archive".to_string()));
        assert_eq!(object.qualified(&kind), "[selected.db].[sales].[Order.Detail]]Archive]");
        assert!(object.columns_query(&kind).contains("TABLE_SCHEMA = N'sales'"));
        assert!(object.primary_key_query(&kind).contains("CONSTRAINT_CATALOG = ku.CONSTRAINT_CATALOG"));
        assert!(object.view_query(&kind).contains("[selected.db].sys.views"));
    }
    #[test] fn quoted_roundtrip_preserves_dots_and_brackets_and_refuses_cross_catalog_override() {
        let kind = DbKind::of("sqlserver");
        for (schema,name) in [("dbo","same"),("archive","same"),("a.b","x]y.z"),("a'b","t'1")] {
            let label = sqlserver_object_label(&kind, schema, name);
            let object = SqlServerObject::parse("target", &label).unwrap();
            assert_eq!((object.schema.as_str(), object.name.as_str()), (schema,name));
        }
        for invalid in ["same", "[same]", "other.dbo.t", "[dbo].[t", "dbo.", "[dbo]junk.t"] { assert!(SqlServerObject::parse("selected",invalid).is_err()); }
        assert!(SqlServerObject::parse("", "dbo.t").is_err());
    }
}
