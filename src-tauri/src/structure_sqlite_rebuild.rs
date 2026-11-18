//! SQLite 结构同步专用编译器。只消费同一 owned 快照；所有写入由父 C1 事务执行。
//! 不关闭外键，不 rename 旧表，不复制数据到带原触发器的表，不接受客户端 SQL。
use anyhow::{anyhow, ensure, Result};
use sqlx::{Row, SqliteConnection};
use crate::{design::{self, ColumnDesign, DesignStep, TableDesign}, models::DbKind,
    structure_sync::StructureDiffReport};
use super::{metadata::{ident, OwnedTableSnapshot}, validate_constant, validate_type};

#[derive(Debug, Clone)]
struct Token { text: String, start: usize, end: usize, quoted: bool, literal: bool }
impl Token {
    fn is(&self, word: &str) -> bool { !self.quoted && !self.literal && self.text.eq_ignore_ascii_case(word) }
    fn name(&self) -> Option<&str> { if self.literal { None } else { Some(&self.text) } }
}
// 单独分开所有运算符，防止 CHECK(a>=0) 的依赖识别把 a>=0 误当列名。
fn lex(sql: &str) -> Result<Vec<Token>> {
    ensure!(sql.len() <= 8 * 1024 * 1024, "STRUCTURE_REBUILD_DDL_QUOTA");
    let b = sql.as_bytes(); let mut i = 0; let mut tokens = Vec::new();
    while i < b.len() {
        if b[i].is_ascii_whitespace() { i += 1; continue; }
        if b[i..].starts_with(b"--") { while i < b.len() && b[i] != b'\n' { i += 1; } continue; }
        if b[i..].starts_with(b"/*") { i += 2; while i + 1 < b.len() && !b[i..].starts_with(b"*/") { i += 1; } ensure!(i + 1 < b.len(), "STRUCTURE_REBUILD_DDL_INVALID"); i += 2; continue; }
        let start = i; let mut quoted = false; let mut literal = false;
        let text = if matches!(b[i], b'\'' | b'"' | b'`' | b'[') {
            let opening = b[i]; let closing = if opening == b'[' { b']' } else { opening }; i += 1;
            loop { ensure!(i < b.len(), "STRUCTURE_REBUILD_DDL_INVALID"); if b[i] == closing { i += 1; if opening != b'[' && i < b.len() && b[i] == closing { i += 1; continue; } break; } i += 1; }
            literal = opening == b'\''; quoted = !literal;
            if quoted { sql[start+1..i-1].replace(&format!("{}{}", closing as char, closing as char), &(closing as char).to_string()) } else { sql[start..i].to_owned() }
        } else if b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 128 {
            i += 1; while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'$' || b[i] >= 128) { i += 1; } sql[start..i].to_owned()
        } else { i += 1; sql[start..i].to_owned() };
        tokens.push(Token { text, start, end: i, quoted, literal });
        ensure!(tokens.len() <= 262144, "STRUCTURE_REBUILD_DDL_QUOTA");
    }
    Ok(tokens)
}
fn references(sql: &str, name: &str) -> Result<bool> {
    Ok(lex(sql)?.iter().any(|t| t.name().is_some_and(|v| v.eq_ignore_ascii_case(name))))
}
fn step(steps: &mut Vec<DesignStep>, label: impl Into<String>, sql: String) { steps.push(DesignStep { label: label.into(), sql }); }
fn declared(column: &ColumnDesign) -> Result<String> {
    let mut value = column.data_type.trim().to_owned();
    if let Some(length) = &column.length {
        ensure!(!value.contains('('), "STRUCTURE_TYPE_UNSUPPORTED: 重复类型尺寸");
        value.push('('); value.push_str(length);
        if let Some(scale) = &column.scale { value.push(','); value.push_str(scale); }
        value.push(')');
    } else { ensure!(column.scale.is_none(), "STRUCTURE_TYPE_UNSUPPORTED: 缺少精度"); }
    validate_type(&value)?; Ok(value)
}
fn validate_design(current: &OwnedTableSnapshot, desired: &TableDesign, allow_drop: bool) -> Result<()> {
    ensure!(desired.schema == "main" && desired.table == current.identity.table && !desired.columns.is_empty(), "STRUCTURE_REBUILD_SCOPE_INVALID");
    ensure!(desired.foreign_keys.is_none() && desired.checks.is_none() && desired.triggers.is_none() && desired.options.is_none(), "STRUCTURE_REBUILD_OBJECT_CHANGE_UNSUPPORTED");
    let mut names = std::collections::HashSet::new();
    for column in &desired.columns {
        ident(&column.name)?; ensure!(names.insert(column.name.to_ascii_lowercase()), "STRUCTURE_REBUILD_COLUMN_COLLISION");
        declared(column)?; if let Some(default) = &column.default_value { validate_constant(default)?; }
        ensure!(!column.auto_increment && !column.unsigned && !column.zerofill, "STRUCTURE_REBUILD_COLUMN_ATTRIBUTE_UNSUPPORTED");
        ensure!(column.orig_name.as_ref().map_or(true, |name| name == &column.name && current.schema.columns.iter().any(|c| &c.name == name)), "STRUCTURE_REBUILD_RENAME_UNSUPPORTED");
    }
    let pk = design::ordered_primary_key(&current.schema)?;
    if !allow_drop {
        ensure!(current.schema.columns.iter().all(|c| desired.columns.iter().any(|d| d.name == c.name)), "STRUCTURE_DROP_NOT_APPROVED");
        ensure!(pk.is_empty() || pk == desired.primary_key, "STRUCTURE_PRIMARY_KEY_DROP_NOT_APPROVED");
        ensure!(current.schema.indexes.iter().filter(|i| !i.primary).all(|i| desired.indexes.as_deref().unwrap_or(&[]).iter().any(|d| d.name == i.name && d.columns == i.columns && d.unique == i.unique)), "STRUCTURE_INDEX_DROP_NOT_APPROVED");
    }
    let mut pk_names = std::collections::HashSet::new();
    ensure!(desired.primary_key.iter().all(|name| names.contains(&name.to_ascii_lowercase()) && pk_names.insert(name)), "STRUCTURE_REBUILD_PRIMARY_KEY_INVALID");
    // 自动索引属于原 UNIQUE 约束；即便只有索引变化也不能伪装成可 DROP 的普通索引。
    for index in current.schema.indexes.iter().filter(|i| !i.primary && i.name.starts_with("sqlite_autoindex_")) {
        ensure!(desired.indexes.as_deref().unwrap_or(&[]).iter().any(|i| i.name == index.name && i.columns == index.columns && i.unique == index.unique), "STRUCTURE_REBUILD_UNIQUE_CONSTRAINT_DEPENDENCY: {} 不能作为额外普通索引删除", index.name);
    }
    for index in desired.indexes.as_deref().unwrap_or(&[]) {
        ident(&index.name)?;
        ensure!(!index.name.to_ascii_lowercase().starts_with("sqlite_") || current.schema.indexes.iter().any(|i| i.name == index.name && i.columns == index.columns && i.unique == index.unique), "STRUCTURE_REBUILD_NEW_AUTO_INDEX_UNSUPPORTED: {}", index.name);
        ensure!(!index.columns.is_empty() && index.columns.iter().all(|name| desired.columns.iter().any(|c| &c.name == name)) && index.descending.is_empty() && index.method.as_deref().map_or(true, |m| m.eq_ignore_ascii_case("btree")), "STRUCTURE_REBUILD_INDEX_UNSUPPORTED: {}", index.name);
    }
    Ok(())
}
fn needs_rebuild(current: &OwnedTableSnapshot, desired: &TableDesign) -> Result<bool> {
    if design::ordered_primary_key(&current.schema)? != desired.primary_key { return Ok(true); }
    for column in &current.schema.columns {
        let Some(next) = desired.columns.iter().find(|c| c.name == column.name) else { return Ok(true); };
        if column.data_type.trim().to_ascii_uppercase() != declared(next)?.to_ascii_uppercase() || column.nullable != next.nullable || column.default != next.default_value { return Ok(true); }
    }
    // CURRENT_* 默认值不能原地 ADD，但可以通过影子复制生成新列。
    Ok(desired.columns.iter().any(|c| c.orig_name.is_none() && c.default_value.as_deref().is_some_and(|d| d.trim().to_ascii_uppercase().starts_with("CURRENT_"))))
}

#[derive(Debug, Clone)]
struct Dependency { kind: String, name: String, sql: String }

/// 捕获依赖闭包，事务内暂拆并原文恢复视图/跨表触发器；不关闭 FK，入站 FK 明确拒绝。
async fn dependencies(conn: &mut SqliteConnection, current: &OwnedTableSnapshot, dropped: &[String]) -> Result<Vec<Dependency>> {
    let (catalog_count, catalog_bytes): (i64, i64) = crate::sql_history::query_as("SELECT count(*),COALESCE(sum(length(sql)),0) FROM main.sqlite_schema WHERE type IN ('table','trigger','view')").fetch_one(&mut *conn).await?;
    ensure!(catalog_count <= 16384 && catalog_bytes <= 32 * 1024 * 1024, "STRUCTURE_REBUILD_CATALOG_QUOTA");
    let rows: Vec<(String, String, String, Option<String>)> = crate::sql_history::query_as("SELECT type,name,tbl_name,sql FROM main.sqlite_schema WHERE type IN ('table','trigger','view') ORDER BY type,name").fetch_all(&mut *conn).await?;
    ensure!(rows.len() <= 16384 && rows.iter().map(|r| r.3.as_ref().map_or(0, String::len)).sum::<usize>() <= 32 * 1024 * 1024, "STRUCTURE_REBUILD_CATALOG_QUOTA");
    for (kind, name, _, _) in &rows {
        if kind != "table" { continue; }
        let foreign_keys = crate::sql_history::query("SELECT \"table\",\"from\",\"to\" FROM pragma_foreign_key_list(?)").bind(name).fetch_all(&mut *conn).await?;
        for fk in foreign_keys {
            let target: String = fk.try_get("table")?;
            ensure!(!target.eq_ignore_ascii_case(&current.identity.table), "STRUCTURE_REBUILD_INBOUND_FOREIGN_KEY: 表 {name} 引用 {target}；保持 FK 开启时 DROP 会触发级联/失效，拒绝自动重建");
        }
    }
    let mut affected = vec![current.identity.table.clone()];
    let mut result: Vec<Dependency> = Vec::new();
    loop {
        let mut added = false;
        for (kind, name, _, sql) in rows.iter().filter(|r| r.0 == "view") {
            if affected.iter().any(|v| v.eq_ignore_ascii_case(name)) { continue; }
            let sql = sql.as_ref().ok_or_else(|| anyhow!("STRUCTURE_REBUILD_DEPENDENCY_UNKNOWN: {kind} {name}"))?;
            if affected.iter().map(|v| references(sql, v)).collect::<Result<Vec<_>>>()?.into_iter().any(|v| v) {
                for column in dropped { ensure!(!references(sql, column)? && !lex(sql)?.iter().any(|t| t.is("*")), "STRUCTURE_REBUILD_VIEW_DEPENDENCY: 视图 {name} 引用待删除列 {column} 或使用未认证的通配投影"); }
                // 只编译原视图，不读取用户行；已有坏视图不成为可应用计划。
                let view = ident(name)?;
                let row = crate::sql_history::query(&format!("SELECT * FROM {view} LIMIT 0")).fetch_all(&mut *conn).await?;
                ensure!(row.is_empty(), "STRUCTURE_REBUILD_VIEW_VALIDATION_FAILED");
                result.push(Dependency { kind: kind.clone(), name: name.clone(), sql: sql.clone() });
                affected.push(name.clone()); added = true;
            }
        }
        if !added { break; }
    }
    for (kind, name, parent, sql) in rows.iter().filter(|r| r.0 == "trigger") {
        let sql = sql.as_ref().ok_or_else(|| anyhow!("STRUCTURE_REBUILD_DEPENDENCY_UNKNOWN: {kind} {name}"))?;
        let owned = parent.eq_ignore_ascii_case(&current.identity.table);
        let external = !owned && (affected.iter().any(|v| v.eq_ignore_ascii_case(parent)) || affected.iter().map(|v| references(sql, v)).collect::<Result<Vec<_>>>()?.into_iter().any(|v| v));
        if owned || external {
            for column in dropped { ensure!(!references(sql, column)?, "STRUCTURE_REBUILD_TRIGGER_DEPENDENCY: 触发器 {name} 引用待删除列 {column}"); }
        }
        if external { result.push(Dependency { kind: kind.clone(), name: name.clone(), sql: sql.clone() }); }
    }
    let trigger_parents: Vec<String> = rows.iter().filter(|r| r.0 == "trigger" && result.iter().any(|d| d.kind == "trigger" && d.name == r.1)).map(|r| r.2.clone()).collect();
    for (kind, name, parent, sql) in rows.iter().filter(|r| r.0 == "trigger") {
        if trigger_parents.iter().any(|v| v.eq_ignore_ascii_case(parent)) && !result.iter().any(|d| d.kind == "trigger" && d.name == *name) {
            result.push(Dependency { kind: kind.clone(), name: name.clone(), sql: sql.clone().ok_or_else(|| anyhow!("STRUCTURE_REBUILD_DEPENDENCY_UNKNOWN: trigger {name}"))? });
        }
    }
    Ok(result)
}

pub(super) async fn verify_dependencies(conn: &mut SqliteConnection, before: &[(String, String, String)]) -> Result<()> {
    let after: Vec<(String, String, String)> = crate::sql_history::query_as("SELECT type,name,sql FROM main.sqlite_schema WHERE type IN ('view','trigger') ORDER BY type,name").fetch_all(&mut *conn).await?;
    ensure!(before == after, "STRUCTURE_READBACK_DEPENDENCY_CHANGED: 视图/触发器目录未原文保留");
    for (kind, name, _) in before {
        if kind == "view" { crate::sql_history::query(&format!("SELECT * FROM {} LIMIT 0", ident(name)?)).fetch_all(&mut *conn).await?; }
    }
    Ok(())
}

// 解析 CREATE TABLE 的顶层成员；只提取、重新编译认证的普通约束。未知语法明确拒绝。
fn members(sql: &str, table: &str) -> Result<Vec<String>> {
    let t = lex(sql)?;
    ensure!(t.len() >= 5 && t[0].is("CREATE") && t[1].is("TABLE") && t[2].name() == Some(table) && t[3].is("("), "STRUCTURE_REBUILD_TABLE_DDL_UNSUPPORTED");
    let mut depth = 1; let mut start = 4; let mut members = Vec::new(); let mut end = None;
    for i in 4..t.len() {
        if t[i].is("(") { depth += 1; }
        if t[i].is(")") { depth -= 1; }
        if depth == 0 || (depth == 1 && t[i].is(",")) {
            ensure!(i > start, "STRUCTURE_REBUILD_TABLE_DDL_INVALID"); members.push(sql[t[start].start..t[i-1].end].to_owned()); start = i+1;
            if depth == 0 { end = Some(i); break; }
        }
    }
    let end = end.ok_or_else(|| anyhow!("STRUCTURE_REBUILD_TABLE_DDL_INVALID"))?;
    ensure!(t[end+1..].is_empty() || (t[end+1..].len() == 1 && t[end+1].is(";")), "STRUCTURE_REBUILD_TABLE_OPTIONS_UNSUPPORTED");
    Ok(members)
}
fn balanced(t: &[Token], start: usize) -> Result<usize> {
    ensure!(t.get(start).is_some_and(|t| t.is("(")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID");
    let mut depth = 0;
    for i in start..t.len() { if t[i].is("(") { depth += 1; } if t[i].is(")") { depth -= 1; if depth == 0 { return Ok(i+1); } } }
    Err(anyhow!("STRUCTURE_REBUILD_CONSTRAINT_INVALID"))
}
pub(super) fn ordinary_constraints(sql: &str, column: bool) -> Result<Vec<String>> {
    let t = lex(sql)?; ensure!(!t.iter().any(|t| t.is(";")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID");
    let boundary = |v: &Token| ["CONSTRAINT", "PRIMARY", "NOT", "NULL", "UNIQUE", "CHECK", "DEFAULT", "REFERENCES"].iter().any(|s| v.is(s));
    let mut i = if column { 1 } else { 0 };
    if column { while i < t.len() && !boundary(&t[i]) { i += 1; } }
    let mut result = Vec::new();
    while i < t.len() {
        let start = i;
        if t[i].is("CONSTRAINT") { ensure!(i+2 < t.len() && t[i+1].name().is_some(), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); i += 2; }
        let keep;
        if t[i].is("CHECK") || t[i].is("UNIQUE") || t[i].is("PRIMARY") {
            keep = true;
            if t[i].is("PRIMARY") { i += 1; ensure!(t.get(i).is_some_and(|t| t.is("KEY")), "STRUCTURE_REBUILD_PRIMARY_KEY_INVALID"); }
            i += 1;
            if t.get(i).is_some_and(|t| t.is("(")) { i = balanced(&t, i)?; }
            else { ensure!(column && !t[i-1].is("CHECK"), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); }
            // 不默默改变 PRIMARY KEY DESC / ON CONFLICT 策略。
            ensure!(!t.get(i).is_some_and(|t| t.is("ASC") || t.is("DESC") || t.is("ON")), "STRUCTURE_REBUILD_CONFLICT_OR_SORT_UNSUPPORTED");
        } else if t[i].is("NOT") {
            i += 1; ensure!(t.get(i).is_some_and(|t| t.is("NULL")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); i += 1; keep = false;
        } else if t[i].is("NULL") { i += 1; keep = false;
        } else if t[i].is("DEFAULT") {
            i += 1; ensure!(i < t.len(), "STRUCTURE_REBUILD_DEFAULT_INVALID");
            if t[i].is("(") { i = balanced(&t, i)?; }
            else { i += 1; while i < t.len() && !boundary(&t[i]) { i += 1; } }
            keep = false;
        } else if t[i].is("FOREIGN") || t[i].is("REFERENCES") {
            keep = true;
            if t[i].is("FOREIGN") {
                i += 1; ensure!(t.get(i).is_some_and(|t| t.is("KEY")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID");
                i = balanced(&t, i + 1)?;
            }
            ensure!(t.get(i).is_some_and(|t| t.is("REFERENCES")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID");
            i += 1; ensure!(t.get(i).and_then(Token::name).is_some(), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); i += 1;
            if t.get(i).is_some_and(|t| t.is("(")) { i = balanced(&t, i)?; }
            loop {
                if t.get(i).is_some_and(|t| t.is("MATCH")) {
                    i += 1; ensure!(t.get(i).and_then(Token::name).is_some(), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); i += 1;
                } else if t.get(i).is_some_and(|t| t.is("ON")) {
                    i += 1; ensure!(t.get(i).is_some_and(|t| t.is("UPDATE") || t.is("DELETE")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); i += 1;
                    if t.get(i).is_some_and(|t| t.is("SET") || t.is("NO")) { i += 1; ensure!(t.get(i).is_some_and(|t| t.is("NULL") || t.is("DEFAULT") || t.is("ACTION")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); }
                    else { ensure!(t.get(i).is_some_and(|t| t.is("CASCADE") || t.is("RESTRICT")), "STRUCTURE_REBUILD_CONSTRAINT_INVALID"); }
                    i += 1;
                } else { break; }
            }
            // DEFERRABLE/INITIALLY 在 metadata 阶段拒绝，绝不吞掉后续 NOT NULL/DEFAULT/CHECK。
        } else { return Err(anyhow!("STRUCTURE_REBUILD_CONSTRAINT_UNSUPPORTED: {}", t[i].text)); }
        if keep { result.push(sql[t[start].start..t[i-1].end].to_owned()); }
    }
    Ok(result)
}
fn preserved_constraints(current: &OwnedTableSnapshot, desired: &TableDesign) -> Result<(std::collections::HashMap<String,Vec<String>>, Vec<String>, bool)> {
    let ddl = current.objects.table_definition.as_deref().ok_or_else(|| anyhow!("STRUCTURE_REBUILD_TABLE_DDL_UNKNOWN"))?;
    let mut columns = std::collections::HashMap::new(); let mut constraints = Vec::new(); let mut primary_emitted = false;
    let old_pk = design::ordered_primary_key(&current.schema)?;
    let has_unique = current.schema.indexes.iter().any(|i| !i.primary && i.name.starts_with("sqlite_autoindex_"));
    let rowid_alias = |names: &[String], columns: &[ColumnDesign]| -> Result<bool> {
        Ok(names.len() == 1 && columns.iter().find(|c| c.name == names[0]).map(declared).transpose()?.is_some_and(|t| t.eq_ignore_ascii_case("INTEGER")))
    };
    let old_alias = old_pk.len() == 1 && current.schema.columns.iter().any(|c| c.name == old_pk[0] && c.data_type.eq_ignore_ascii_case("INTEGER"));
    ensure!(!has_unique || (old_pk == desired.primary_key && old_alias == rowid_alias(&desired.primary_key,&desired.columns)?), "STRUCTURE_REBUILD_UNIQUE_PRIMARY_KEY_DEPENDENCY: PK 改动可能改变 UNIQUE 自动索引身份，未认证无损改写");
    let dropped: Vec<_> = current.schema.columns.iter().filter(|c| !desired.columns.iter().any(|d| d.name == c.name)).map(|c| &c.name).collect();
    for member in members(ddl, &current.identity.table)? {
        let tokens = lex(&member)?; let first = tokens.first().ok_or_else(|| anyhow!("STRUCTURE_REBUILD_TABLE_DDL_INVALID"))?;
        let column = current.schema.columns.iter().find(|c| first.name() == Some(c.name.as_str()));
        let mut retained = Vec::new();
        for constraint in ordinary_constraints(&member, column.is_some())? {
            let t = lex(&constraint)?;
            let primary = t.first().is_some_and(|v| v.is("PRIMARY")) || (t.first().is_some_and(|v| v.is("CONSTRAINT")) && t.get(2).is_some_and(|v| v.is("PRIMARY")));
            if primary {
                if desired.primary_key.is_empty() { continue; }
                if let Some(column) = column {
                    if desired.primary_key.len() == 1 && desired.primary_key[0] == column.name {
                        retained.push(constraint); primary_emitted = true;
                    }
                } else {
                    retained.push(format!("PRIMARY KEY ({})", desired.primary_key.iter().map(|n| ident(n)).collect::<Result<Vec<_>>>()?.join(", "))); primary_emitted = true;
                }
                continue;
            }
            for name in &dropped { ensure!(!references(&constraint, name)?, "STRUCTURE_REBUILD_CONSTRAINT_DEPENDENCY: 约束 {constraint} 引用待删除列 {name}"); }
            retained.push(constraint);
        }
        if let Some(column) = column {
            if desired.columns.iter().any(|c| c.name == column.name) { columns.insert(column.name.clone(), retained); }
            else { ensure!(retained.is_empty(), "STRUCTURE_REBUILD_CONSTRAINT_DEPENDENCY: 待删除列 {} 带 CHECK/UNIQUE/外键约束", column.name); }
        } else { constraints.extend(retained); }
    }
    Ok((columns, constraints, primary_emitted))
}
fn affinity(data_type: &str) -> &'static str {
    let t = data_type.to_ascii_uppercase();
    if t.contains("INT") { "INTEGER" } else if ["CHAR","CLOB","TEXT"].iter().any(|s| t.contains(s)) { "TEXT" }
    else if t.contains("BLOB") { "BLOB" } else if ["REAL","FLOA","DOUB"].iter().any(|s| t.contains(s)) { "REAL" } else { "NUMERIC" }
}
fn assert_step(steps: &mut Vec<DesignStep>, guard: &str, label: String, violation: String) {
    step(steps, label, format!("INSERT INTO {guard}(ok) SELECT CASE WHEN EXISTS({violation}) THEN 0 ELSE 1 END"));
}

async fn rebuild(conn: &mut SqliteConnection, current: &OwnedTableSnapshot, desired: &TableDesign) -> Result<Vec<DesignStep>> {
    let dropped = current.schema.columns.iter().filter(|c| !desired.columns.iter().any(|d| d.name == c.name)).map(|c| c.name.clone()).collect::<Vec<_>>();
    let dependencies = dependencies(conn, current, &dropped).await?;
    let trigger_order: Vec<String> = crate::sql_history::query_scalar("SELECT name FROM main.sqlite_schema WHERE type='trigger' ORDER BY rowid").fetch_all(&mut *conn).await?;
    let (column_constraints, constraints, primary_emitted) = preserved_constraints(current, desired)?;
    let suffix = crate::object_catalog::revision(&("sqlite-rebuild/v1", &current.revision, desired));
    let shadow_name = format!("__navkit_rebuild_{}", &suffix[..24]); let guard_name = format!("__navkit_check_{}", &suffix[..24]);
    let shadow = ident(&shadow_name)?; let guard = ident(&guard_name)?; let table = ident(&desired.table)?;
    let count: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE name COLLATE NOCASE IN (?,?)").bind(&shadow_name).bind(&guard_name).fetch_one(&mut *conn).await?;
    ensure!(count == 0, "STRUCTURE_REBUILD_TEMP_NAME_COLLISION: 不清理/覆盖已有对象");
    let rowid = ["_rowid_", "rowid", "oid"].iter().find(|name| !current.schema.columns.iter().any(|c| c.name.eq_ignore_ascii_case(name)) && !desired.columns.iter().any(|c| c.name.eq_ignore_ascii_case(name))).ok_or_else(|| anyhow!("STRUCTURE_REBUILD_ROWID_SHADOWED: 三个 rowid 名均被遮蔽，不能证明物理行身份保留"))?;
    let rowid = ident(rowid)?;
    let mut definitions = Vec::new();
    for column in &desired.columns {
        let mut definition = format!("{} {}", ident(&column.name)?, declared(column)?);
        if !column.nullable { definition.push_str(" NOT NULL"); }
        if let Some(default) = &column.default_value { definition.push_str(" DEFAULT "); definition.push_str(default); }
        for extra in column_constraints.get(&column.name).into_iter().flatten() { definition.push(' '); definition.push_str(extra); }
        definitions.push(definition);
    }
    if !desired.primary_key.is_empty() && !primary_emitted { definitions.push(format!("PRIMARY KEY ({})", desired.primary_key.iter().map(|n| ident(n)).collect::<Result<Vec<_>>>()?.join(", "))); }
    definitions.extend(constraints);
    // UNIQUE 自动索引只能由原约束重建，绝不能 CREATE sqlite_autoindex_* 或默默删除约束。
    for index in current.schema.indexes.iter().filter(|i| i.name.starts_with("sqlite_autoindex_")) {
        ensure!(desired.indexes.as_deref().unwrap_or(&[]).iter().any(|i| i.name == index.name && i.columns == index.columns && i.unique == index.unique), "STRUCTURE_REBUILD_UNIQUE_CONSTRAINT_DEPENDENCY: {} 是普通 UNIQUE 约束，不能作为额外索引删除", index.name);
    }
    let mut steps = Vec::new();
    step(&mut steps, "建立事务数据校验表", format!("CREATE TABLE {guard}(ok INTEGER NOT NULL CONSTRAINT navkit_lossless_copy CHECK(ok=1))"));
    step(&mut steps, "建立无原触发器的影子表", format!("CREATE TABLE {shadow} ({})", definitions.join(", ")));
    for column in &desired.columns {
        let Some(old) = current.schema.columns.iter().find(|c| c.name == column.name) else { continue; };
        let name = ident(&column.name)?; let data_type = declared(column)?;
        if old.data_type.trim().eq_ignore_ascii_case(&data_type) { continue; }
        let conversion = match affinity(&data_type) {
            "INTEGER" => format!("typeof({name})='integer' OR (typeof({name})='real' AND {name}>=-9223372036854775808.0 AND {name}<9223372036854775808.0 AND {name}=CAST({name} AS INTEGER)) OR (typeof({name})='text' AND {name}=CAST(CAST({name} AS INTEGER) AS TEXT))"),
            "REAL" => format!("typeof({name})='real' OR (typeof({name})='integer' AND {name} BETWEEN -9007199254740992 AND 9007199254740992) OR (typeof({name})='text' AND {name}=CAST(CAST({name} AS REAL) AS TEXT))"),
            "NUMERIC" => format!("typeof({name}) IN ('integer','real') OR (typeof({name})='text' AND {name}=CAST(CAST({name} AS NUMERIC) AS TEXT))"),
            "TEXT" => format!("typeof({name}) IN ('text','integer') OR (typeof({name})='real' AND CAST(CAST({name} AS TEXT) AS REAL) IS {name})"),
            _ => "1".into(),
        };
        if affinity(&old.data_type) != affinity(&data_type) {
            assert_step(&mut steps, &guard, format!("扫描列 {} 的无损类型转换", column.name), format!("SELECT 1 FROM {table} WHERE {name} IS NOT NULL AND NOT ({conversion})"));
        }
        if let Some((_, sizes)) = data_type.split_once('(') {
            let sizes = sizes.trim_end_matches(')').split(',').map(|s| s.trim().parse::<u32>()).collect::<std::result::Result<Vec<_>,_>>()?;
            if affinity(&data_type) == "TEXT" {
                assert_step(&mut steps, &guard, format!("检查列 {} 长度，拒绝截断", column.name), format!("SELECT 1 FROM {table} WHERE length({name})>{}", sizes[0]));
            } else if affinity(&data_type) == "NUMERIC" {
                let scale = sizes.get(1).copied().unwrap_or(0); ensure!(sizes[0] > 0 && sizes[0] <= 38 && scale <= sizes[0], "STRUCTURE_REBUILD_NUMERIC_PRECISION_UNSUPPORTED");
                let bound = 10f64.powi((sizes[0]-scale) as i32);
                assert_step(&mut steps, &guard, format!("检查列 {} 精度，拒绝舍入/溢出", column.name), format!("SELECT 1 FROM {table} WHERE round(CAST({name} AS NUMERIC),{scale})!=CAST({name} AS NUMERIC) OR abs(CAST({name} AS NUMERIC))>={bound}"));
            }
        }
    }
    if desired.primary_key.len() == 1 {
        let pk = desired.columns.iter().find(|c| c.name == desired.primary_key[0]).expect("validated PK");
        if declared(pk)?.eq_ignore_ascii_case("INTEGER") {
            let violation = if pk.orig_name.is_some() { format!("SELECT 1 FROM {table} WHERE {} IS NOT {rowid}", ident(&pk.name)?) } else { format!("SELECT 1 FROM {table}") };
            assert_step(&mut steps, &guard, "确认 INTEGER 主键与原 rowid 一致".into(), violation);
        }
    }
    let existing = desired.columns.iter().filter(|c| c.orig_name.is_some()).map(|c| ident(&c.name)).collect::<Result<Vec<_>>>()?;
    let copy = std::iter::once(rowid.clone()).chain(existing.iter().cloned()).collect::<Vec<_>>().join(", ");
    step(&mut steps, "复制原数据及 rowid（不执行原表触发器）", format!("INSERT INTO {shadow} ({copy}) SELECT {copy} FROM {table}"));
    assert_step(&mut steps, &guard, "确认影子表行数完整".into(), format!("SELECT 1 WHERE (SELECT count(*) FROM {table})!=(SELECT count(*) FROM {shadow})"));
    let equal = desired.columns.iter().filter(|c| c.orig_name.is_some()).map(|c| {
        let name = ident(&c.name)?; let a = affinity(&declared(c)?);
        let old = current.schema.columns.iter().find(|v| v.name == c.name).ok_or_else(|| anyhow!("STRUCTURE_REBUILD_RETAINED_COLUMN_MISSING"))?;
        Ok(if affinity(&old.data_type) == a || a == "BLOB" { format!("o.{name} IS n.{name}") } else { format!("CAST(o.{name} AS {a}) IS n.{name}") })
    }).collect::<Result<Vec<_>>>()?;
    if !equal.is_empty() { assert_step(&mut steps, &guard, "逐行确认保留列与 rowid 无数据损失".into(), format!("SELECT 1 FROM {table} AS o LEFT JOIN {shadow} AS n ON o.{rowid}=n.{rowid} WHERE n.{rowid} IS NULL OR NOT ({})", equal.join(" AND "))); }
    for dependency in dependencies.iter().rev() {
        step(&mut steps, format!("暂拆依赖 {} {}", dependency.kind, dependency.name), format!("DROP {} {}", dependency.kind.to_ascii_uppercase(), ident(&dependency.name)?));
    }
    step(&mut steps, "替换原表（所有依赖已预检，外键保持开启）", format!("DROP TABLE {table}"));
    step(&mut steps, "恢复目标物理表名", format!("ALTER TABLE {shadow} RENAME TO {table}"));
    for index in desired.indexes.as_deref().unwrap_or(&[]) {
        if index.name.starts_with("sqlite_autoindex_") {
            ensure!(current.schema.indexes.iter().any(|i| i.name == index.name && i.columns == index.columns && i.unique == index.unique), "STRUCTURE_REBUILD_NEW_AUTO_INDEX_UNSUPPORTED"); continue;
        }
        step(&mut steps, format!("恢复索引 {}", index.name), format!("CREATE {}INDEX {} ON {table} ({})", if index.unique { "UNIQUE " } else { "" }, ident(&index.name)?, index.columns.iter().map(|n| ident(n)).collect::<Result<Vec<_>>>()?.join(", ")));
    }
    for dependency in dependencies.iter().filter(|d| d.kind == "view") { step(&mut steps, format!("恢复依赖 view {}", dependency.name), dependency.sql.clone()); }
    let mut triggers = Vec::new();
    for trigger in &current.objects.triggers {
        let tokens = lex(&trigger.statement)?;
        ensure!(tokens.len() >= 8 && tokens[0].is("CREATE") && tokens[1].is("TRIGGER") && tokens[2].name() == Some(trigger.name.as_str()), "STRUCTURE_REBUILD_TRIGGER_DDL_INVALID");
        let on = tokens.iter().position(|t| t.is("ON")).ok_or_else(|| anyhow!("STRUCTURE_REBUILD_TRIGGER_DDL_INVALID"))?;
        ensure!(tokens.get(on+1).and_then(Token::name) == Some(desired.table.as_str()), "STRUCTURE_REBUILD_TRIGGER_PARENT_CHANGED");
        let mut end = tokens.len(); if tokens[end-1].is(";") { end -= 1; }
        ensure!(tokens[end-1].is("END") && tokens.iter().any(|t| t.is("BEGIN")), "STRUCTURE_REBUILD_TRIGGER_DDL_INVALID");
        // sqlite_schema 的现有触发器定义；仅允许这一条 CREATE TRIGGER（body 内分号属于原触发器）。
        ensure!(!tokens[3..].iter().any(|t| t.is("CREATE") || t.is("ATTACH") || t.is("DETACH") || t.is("PRAGMA")), "STRUCTURE_REBUILD_TRIGGER_DDL_UNSUPPORTED");
        triggers.push((trigger.name.clone(), trigger.statement.clone()));
    }
    for dependency in dependencies.iter().filter(|d| d.kind == "trigger") { triggers.push((dependency.name.clone(), dependency.sql.clone())); }
    ensure!(triggers.iter().all(|(name,_)| trigger_order.iter().any(|v| v == name)), "STRUCTURE_REBUILD_TRIGGER_ORDER_UNKNOWN");
    triggers.sort_by_key(|(name,_)| trigger_order.iter().position(|v| v == name).expect("validated trigger order"));
    for (name, sql) in triggers { step(&mut steps, format!("恢复原触发器 {name}"), sql); }
    step(&mut steps, "清理事务校验表", format!("DROP TABLE {guard}"));
    Ok(steps)
}

pub(super) async fn compile(conn: &mut SqliteConnection, current: &OwnedTableSnapshot, desired: &TableDesign, allow_drop: bool) -> Result<Vec<DesignStep>> {
    validate_design(current, desired, allow_drop)?;
    let steps = if needs_rebuild(current, desired)? { rebuild(conn, current, desired).await? } else {
        if allow_drop { design::compile(&DbKind::of("sqlite"), &current.schema, desired)? } else { design::compile_non_destructive(&DbKind::of("sqlite"), &current.schema, desired)? }
    };
    ensure!(steps.len() <= 1024, "STRUCTURE_DDL_QUOTA"); Ok(steps)
}

/// 通用 DTO 编译器没有完整 SQLite 依赖；仅在 owned 快照链上补全重建计划，然后再绑定 digest。
pub(super) async fn complete_report(conn: &mut SqliteConnection, current: &OwnedTableSnapshot, report: &mut StructureDiffReport, allow_drop: bool) -> Result<()> {
    let Some(desired) = report.final_design.as_ref() else { return Ok(()); };
    match compile(conn, current, desired, allow_drop).await {
        Ok(steps) => { report.steps = steps; report.unsupported.clear(); report.alterable = true; report.summary = if report.changes.is_empty() { "结构一致，无需修改".into() } else { format!("{} 项结构差异", report.changes.len()) }; }
        Err(error) => { report.steps.clear(); report.alterable = false; report.unsupported = vec![format!("{error:#}")]; report.summary = "结构差异需要人工处理；未生成可应用计划".into(); }
    }
    Ok(())
}
