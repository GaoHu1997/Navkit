//! Backend-only SQL compilation and lexical metadata safety. Never execute client steps.
use super::*;
use crate::design::{DesignStep, TableDesign};
pub(super) fn ident(name: &str) -> Result<()> { ensure!(!name.is_empty() && name.len() <= 1024 && !name.contains('\0'), "STRUCTURE_SCOPE_INVALID"); Ok(()) }
pub(super) fn safe_type(raw: &str, pg: bool) -> Result<()> {
    ensure!(raw.len() <= 128 && raw.is_ascii(), "STRUCTURE_TYPE_UNSUPPORTED");
    let normalized = raw.trim().to_ascii_lowercase(); let mut base = normalized.clone();
    if let Some(start) = normalized.find('(') {
        let end = normalized.find(')').ok_or_else(|| anyhow!("STRUCTURE_TYPE_UNSUPPORTED"))?;
        ensure!(end > start && !normalized[end+1..].contains(['(', ')']), "STRUCTURE_TYPE_UNSUPPORTED");
        let sizes = normalized[start+1..end].split(',').collect::<Vec<_>>();
        ensure!(!sizes.is_empty() && sizes.len() <= 2 && sizes.iter().all(|s| { let s = s.trim(); !s.is_empty() && s.len() <= 10 && s.bytes().all(|b| b.is_ascii_digit()) }), "STRUCTURE_TYPE_UNSUPPORTED");
        base = format!("{}{}", &normalized[..start], &normalized[end+1..]);
    }
    let base = base.split_ascii_whitespace().collect::<Vec<_>>().join(" ");
    let allowed = if pg { ["smallint", "integer", "bigint", "real", "double precision", "numeric", "decimal", "boolean", "text", "character", "character varying", "varchar", "char", "bytea", "date", "time without time zone", "time with time zone", "timestamp without time zone", "timestamp with time zone", "uuid", "json", "jsonb"].contains(&base.as_str()) }
        else { let b = base.strip_suffix(" unsigned").unwrap_or(&base); ["tinyint", "smallint", "mediumint", "int", "integer", "bigint", "decimal", "numeric", "float", "double", "real", "boolean", "char", "varchar", "tinytext", "text", "mediumtext", "longtext", "binary", "varbinary", "tinyblob", "blob", "mediumblob", "longblob", "date", "datetime", "timestamp", "time", "year"].contains(&b) };
    ensure!(allowed, "STRUCTURE_TYPE_UNSUPPORTED: 不执行自定义/数组/enum/表达式类型 {raw}"); Ok(())
}
pub(super) fn safe_default(raw: &str, pg: bool) -> Result<()> {
    let mut value = raw.trim(); ensure!(!value.is_empty() && value.len() <= 256, "STRUCTURE_DEFAULT_UNSUPPORTED");
    if pg { if let Some((literal, cast)) = value.rsplit_once("::") { safe_type(cast, true)?; value = literal.trim(); } }
    if ["NULL", "TRUE", "FALSE", "CURRENT_DATE", "CURRENT_TIME", "CURRENT_TIMESTAMP"].iter().any(|v| value.eq_ignore_ascii_case(v)) || pg && value.eq_ignore_ascii_case("now()") { return Ok(()); }
    if value.starts_with('\'') {
        let bytes = value.as_bytes(); let mut i = 1;
        while i < bytes.len() { if bytes[i] == b'\'' { i += 1; if i < bytes.len() && bytes[i] == b'\'' { i += 1; continue; } ensure!(i == bytes.len(), "STRUCTURE_DEFAULT_UNSUPPORTED: literal 后包含 SQL"); return Ok(()); } i += 1; }
        return Err(anyhow!("STRUCTURE_DEFAULT_UNSUPPORTED: 未闭合 literal"));
    }
    let v = value.strip_prefix('+').or_else(|| value.strip_prefix('-')).unwrap_or(value);
    ensure!(!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.') && v.bytes().filter(|b| *b == b'.').count() <= 1 && v.bytes().any(|b| b.is_ascii_digit()), "STRUCTURE_DEFAULT_UNSUPPORTED: 自定义表达式或函数不执行"); Ok(())
}
fn pg_shape(raw: &str) -> (String, Vec<u32>) {
    let text = raw.trim().to_ascii_lowercase();
    if let Some(start) = text.find('(') { if let Some(end) = text.find(')') {
        let sizes = text[start+1..end].split(',').filter_map(|s| s.trim().parse().ok()).collect();
        return (format!("{}{}", &text[..start], &text[end+1..]).split_ascii_whitespace().collect::<Vec<_>>().join(" "), sizes);
    } }
    (text.split_ascii_whitespace().collect::<Vec<_>>().join(" "), Vec::new())
}
pub(super) fn pg_lossless_type(before: &str, after: &str) -> bool {
    let (a, x) = pg_shape(before); let (b, y) = pg_shape(after);
    if a == b && x == y { return true; }
    if matches!((a.as_str(), b.as_str()), ("smallint", "integer" | "bigint") | ("integer", "bigint")) { return true; }
    if ["character varying", "varchar"].contains(&a.as_str()) && b == "text" { return true; }
    if a == b && ["character varying", "varchar"].contains(&a.as_str()) { return y.is_empty() || x.len() == 1 && y.len() == 1 && y[0] >= x[0]; }
    if a == b && ["numeric", "decimal"].contains(&a.as_str()) {
        if y.is_empty() { return true; }
        let scale = |v: &[u32]| v.get(1).copied().unwrap_or(0);
        return !x.is_empty() && !y.is_empty() && scale(&x) == scale(&y) && y[0] >= x[0];
    }
    false
}
fn safe_pg_changes(current: &crate::models::TableSchema, design: &TableDesign) -> Result<()> {
    for column in design.columns.iter().filter(|c| c.orig_name.is_some()) {
        let before = current.columns.iter().find(|c| Some(c.name.as_str()) == column.orig_name.as_deref()).ok_or_else(|| anyhow!("STRUCTURE_COLUMN_IDENTITY_UNPROVEN"))?;
        let mut after = column.data_type.clone(); if let Some(length) = &column.length { after.push_str(&format!("({}{})", length, column.scale.as_ref().map(|s| format!(",{s}")).unwrap_or_default())); }
        ensure!(pg_lossless_type(&before.data_type, &after), "STRUCTURE_PG_LOSSY_TYPE_UNSUPPORTED: 列 {} 的 {} → {} 没有无损转换证明；拒绝窄化/舍入/自定义 USING，不发送 DDL", column.name, before.data_type, after);
    } Ok(())
}
fn safe_design(design: &TableDesign, pg: bool) -> Result<()> {
    ident(&design.schema)?; ident(&design.table)?;
    ensure!(design.columns.len() <= 4096 && design.foreign_keys.is_none() && design.checks.is_none() && design.triggers.is_none() && design.options.is_none(), "STRUCTURE_DDL_SCOPE_UNPROVEN");
    for c in &design.columns { ident(&c.name)?; safe_type(&c.data_type, pg)?;
        ensure!(!c.auto_increment && !c.zerofill && !c.unsigned, "STRUCTURE_GENERATED_COLUMN_UNSUPPORTED");
        for value in [&c.length, &c.scale].into_iter().flatten() { ensure!(!value.is_empty() && value.len() <= 10 && value.bytes().all(|b| b.is_ascii_digit()), "STRUCTURE_TYPE_UNSUPPORTED"); }
        if let Some(default) = &c.default_value { safe_default(default, pg)?; }
    }
    for i in design.indexes.as_deref().unwrap_or(&[]) { ident(&i.name)?; ensure!(!i.columns.is_empty() && i.descending.is_empty() && i.method.as_deref().map_or(true, |m| m.eq_ignore_ascii_case("btree")), "STRUCTURE_INDEX_UNSUPPORTED"); for c in &i.columns { ident(c)?; } }
    Ok(())
}
pub(super) fn artifacts(request: &StructureSyncRequest, binding: &StructureReviewBinding, source_revision: &str, target_revision: &str) -> (String, String) {
    let digest = crate::object_catalog::revision(&("MYSQL_STRUCTURE_SHADOW_V1", binding, &request.columns, request.allow_drop, source_revision, target_revision));
    (format!("__navkit_sync_{}", &digest[..24]), format!("__navkit_backup_{}", &digest[..24]))
}
pub(super) fn retained(design: &TableDesign, target: &crate::models::TableSchema) -> Vec<String> {
    design.columns.iter().filter_map(|c| c.orig_name.as_ref()).filter(|n| target.columns.iter().any(|c| &c.name == *n)).cloned().collect()
}
pub(super) fn complete(report: &mut StructureDiffReport, source: &metadata::Snapshot, target: &metadata::Snapshot, kind: &DbKind, request: &StructureSyncRequest, binding: &StructureReviewBinding) -> Result<()> {
    if !report.unsupported.is_empty() { return Ok(()); }
    let pg = kind.protocol() == Protocol::Postgres;
    if pg && source.identity.database.identity == target.identity.database.identity && source.identity.schema == target.identity.schema {
        if let Some(design) = report.final_design.as_mut() { for index in design.indexes.as_mut().into_iter().flatten() {
            if source.schema.indexes.iter().any(|i| i.name == index.name) && !target.schema.indexes.iter().any(|i| i.name == index.name) {
                let original = index.name.clone();
                index.name = format!("__navkit_idx_{}", &crate::object_catalog::revision(&(&request.target_schema, &request.target_table, &original))[..32]);
                report.warnings.push(format!("PostgreSQL 索引名在 schema 中唯一：源索引 {original} 在目标使用 {}；不会删除或重命名源索引", index.name));
                for change in &mut report.changes { if change.kind == "index" && change.object == original { change.object = index.name.clone(); } }
            }
        } }
        if let Some(design) = &report.final_design { report.steps = if request.allow_drop { design::compile(kind, &target.schema, design)? } else { design::compile_non_destructive(kind, &target.schema, design)? }; }
    }
    let design = report.final_design.as_ref().ok_or_else(|| anyhow!("STRUCTURE_PLAN_MISSING"))?;
    if !pg { ensure!(source.schema.collation == target.schema.collation, "STRUCTURE_MYSQL_COLLATION_CHANGE_UNSUPPORTED: 双端字符集/排序规则不同，不能无损同步列定义"); }
    safe_design(design, pg)?;
    if pg { safe_pg_changes(&target.schema, design)?; }
    let canonical = if request.allow_drop { design::compile(kind, &target.schema, design)? } else { design::compile_non_destructive(kind, &target.schema, design)? };
    ensure!(canonical == report.steps && canonical.len() <= 1024, "STRUCTURE_RECOMPILE_CHANGED");
    if canonical.is_empty() { return Ok(()); }
    if pg { report.warnings.push("PostgreSQL 双端表锁内 CAS/重编/DDL/读回；目标事务 COMMIT，失败/停止必须确认 ROLLBACK；不承诺跨数据库分布式提交".into()); return Ok(()); }
    let (shadow, backup) = artifacts(request, binding, &source.revision, &target.revision);
    let full = db::qualified(kind, &request.target_schema, &request.target_table);
    let shadow_full = db::qualified(kind, &request.target_schema, &shadow); let backup_full = db::qualified(kind, &request.target_schema, &backup);
    let mut current = target.schema.clone(); current.table = shadow.clone();
    let mut shadow_design = design.clone(); shadow_design.table = shadow.clone();
    let shadow_steps = if request.allow_drop { design::compile(kind, &current, &shadow_design)? } else { design::compile_non_destructive(kind, &current, &shadow_design)? };
    let retained = retained(design, &target.schema); ensure!(!retained.is_empty(), "STRUCTURE_COPY_WITHOUT_RETAINED_COLUMNS_UNSUPPORTED");
    let columns = retained.iter().map(|c| db::quote_ident(kind, c)).collect::<Vec<_>>().join(", ");
    let mut steps = vec![DesignStep { label: format!("创建影子表 {shadow}（不修改原表）"), sql: format!("CREATE TABLE {shadow_full} LIKE {full}") }];
    steps.extend(shadow_steps);
    steps.push(DesignStep { label: "严格模式复制全部目标数据到影子表；保留列值逐字节核验".into(), sql: format!("INSERT INTO {shadow_full} ({columns}) SELECT {columns} FROM {full}") });
    steps.push(DesignStep { label: format!("原子切换；原表备份保留为 {backup}"), sql: format!("RENAME TABLE {full} TO {backup_full}, {shadow_full} TO {full}") });
    report.steps = steps;
    report.warnings.push(format!("MySQL DDL 隐式提交：使用影子表 {shadow}，原子 RENAME 后永久保留原表备份 {backup}；不自动删除备份，不声称事务回滚"));
    report.warnings.push(format!("恢复仅在核验身份/无并发改动后由操作者执行，恢复 SQL：RENAME TABLE {full} TO {shadow_full}, {backup_full} TO {full}；必须先确认 {shadow_full} 不存在。"));
    report.warnings.push("新增列使用 DEFAULT/NULL；既有列转换一旦改变保留数据字节则拒绝切换。删除列的数据仍在保留备份中。影子创建后到加锁前的外部 DDL 由锁内双端 CAS 拒绝。".into());
    Ok(())
}
