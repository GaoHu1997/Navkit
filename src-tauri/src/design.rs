//! 表结构设计器：后端生成计划、执行并读回比对。
//!
//! 设计原则（对应 plan/05、plan/02）：
//! - 前端只提交「目标结构」，不提交 SQL；方言编译只发生在后端。
//! - 不支持的能力明确拒绝，不返回成功，也不静默跳过。
//! - 执行后必须读回实际元数据并比对；执行成功但读回失败要标为未验证。
//! - 可事务的方言整批执行，失败回滚；MySQL DDL 隐式提交，逐条记录部分成功。

use anyhow::{anyhow, ensure, Result};
use serde::{Deserialize, Serialize};

use crate::db::{self, Db};
use crate::models::{ColumnInfo, DbKind, ForeignKeyInfo, IndexInfo, Protocol, TableSchema};

/// 目标列定义（完整状态，而不是差异）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnDesign {
    pub name: String,
    /// 原列名；`None` 表示新增列。
    #[serde(default)]
    pub orig_name: Option<String>,
    pub data_type: String,
    #[serde(default)]
    pub length: Option<String>,
    #[serde(default)]
    pub scale: Option<String>,
    pub nullable: bool,
    /// SQL 默认值原文；`None` 表示无默认值。
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default)]
    pub auto_increment: bool,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub unsigned: bool,
    #[serde(default)]
    pub zerofill: bool,
}

/// 目标表结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDesign {
    #[serde(default)]
    pub schema: String,
    pub table: String,
    pub columns: Vec<ColumnDesign>,
    #[serde(default)]
    pub primary_key: Vec<String>,
    /// `None` 表示不修改表注释；`Some("")` 表示清除注释。
    #[serde(default)]
    pub comment: Option<String>,
    /// `None` 表示不修改索引（主键索引不在内）。
    #[serde(default)]
    pub indexes: Option<Vec<IndexDesign>>,
    /// `None` 表示不修改外键。
    #[serde(default)]
    pub foreign_keys: Option<Vec<ForeignKeyDesign>>,
    #[serde(default)]
    pub checks: Option<Vec<CheckDesign>>,
    #[serde(default)]
    pub triggers: Option<Vec<TriggerDesign>>,
    #[serde(default)]
    pub options: Option<TableOptionsDesign>,
    #[serde(default)]
    pub base_revision: Option<String>,
    #[serde(default)]
    pub source_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TableOptionsDesign {
    pub engine: Option<String>, pub charset: Option<String>, pub collation: Option<String>, pub row_format: Option<String>,
    pub strict: Option<bool>, pub without_rowid: Option<bool>, pub fillfactor: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckDesign {
    pub name: String, #[serde(default)] pub orig_name: Option<String>, pub expression: String,
    #[serde(default)] pub generated_name: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerDesign {
    pub name: String, #[serde(default)] pub orig_name: Option<String>, pub timing: String, pub event: String, pub statement: String,
    #[serde(default)] pub definition: Option<String>,
}

/// 目标索引定义（名称/列/唯一/方法/降序列；主键索引不在此列）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexDesign {
    pub name: String,
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
    /// 索引方法；缺省按方言默认（BTREE）。
    #[serde(default)]
    pub method: Option<String>,
    /// columns 中需要降序的列名；未列出的按升序。
    #[serde(default)]
    pub descending: Vec<String>,
}

/// 目标外键定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKeyDesign {
    pub name: String,
    pub columns: Vec<String>,
    pub ref_table: String,
    #[serde(default)]
    pub ref_schema: Option<String>,
    pub ref_columns: Vec<String>,
    #[serde(default)]
    pub on_update: Option<String>,
    #[serde(default)]
    pub on_delete: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignStep {
    pub label: String,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesignStepResult {
    pub label: String,
    pub sql: String,
    /// applied / failed / skipped
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DesignReceipt {
    pub steps: Vec<DesignStepResult>,
    /// 执行后读回与目标一致。
    pub verified: bool,
    /// 有步骤已应用但读回不一致或读回失败；不能当成保存成功。
    pub unverified: bool,
    pub mismatches: Vec<String>,
    /// committed / rolled_back / implicit / none
    pub transaction: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<TableSchema>,
}

/// 校验默认值表达式：允许字面量与常见常量，拒绝语句拼接。
fn validate_default(expr: &str) -> Result<()> {
    let trimmed = expr.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return Err(anyhow!("默认值表达式为空或过长"));
    }
    if trimmed.contains('\0') || trimmed.contains(';') || trimmed.contains("--") || trimmed.contains("/*") || trimmed.contains("*/") {
        return Err(anyhow!("默认值包含不允许的语句分隔或注释符号：{}", trimmed));
    }
    Ok(())
}

fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn base_type(kind: &DbKind, c: &ColumnDesign) -> String {
    let mut t = c.data_type.trim().to_string();
    if let Some(len) = c.length.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        match c.scale.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(scale) => t.push_str(&format!("({},{})", len, scale)),
            None => t.push_str(&format!("({})", len)),
        }
    }
    if matches!(kind.protocol(), Protocol::Mysql) {
        if c.unsigned {
            t.push_str(" UNSIGNED");
        }
        if c.zerofill {
            t.push_str(" ZEROFILL");
        }
    }
    t
}

/// 完整列定义（用于 ADD / MODIFY / CHANGE）。
pub(crate) fn column_definition(kind: &DbKind, c: &ColumnDesign, include_auto_increment: bool) -> Result<String> {
    let mut sql = format!("{} {}", db::quote_ident(kind, &c.name), base_type(kind, c));
    if !c.nullable {
        sql.push_str(" NOT NULL");
    }
    if let Some(default) = c.default_value.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        validate_default(default)?;
        sql.push_str(&format!(" DEFAULT {}", default));
    }
    if include_auto_increment && c.auto_increment {
        if !matches!(kind.protocol(), Protocol::Mysql) {
            return Err(anyhow!("当前数据库不支持在设计器中修改自增属性"));
        }
        sql.push_str(" AUTO_INCREMENT");
    }
    if let Some(comment) = c.comment.as_deref().filter(|s| !s.is_empty()) {
        if matches!(kind.protocol(), Protocol::Mysql) {
            sql.push_str(&format!(" COMMENT {}", quote_literal(comment)));
        }
    }
    Ok(sql)
}

fn find_column<'a>(schema: &'a TableSchema, name: &str) -> Option<&'a ColumnInfo> {
    schema.columns.iter().find(|c| c.name == name)
}

fn same_type(actual: &str, design: &ColumnDesign, kind: &DbKind) -> bool {
    normalize_type(actual) == normalize_type(&base_type(kind, design))
}

fn normalize_type(raw: &str) -> String {
    let s = raw.trim().to_lowercase().replace(' ', "");
    let s = match s.split('(').next().unwrap_or(&s) {
        "int4" => "integer".to_string(),
        "int8" => "bigint".to_string(),
        "int2" => "smallint".to_string(),
        "bool" => "boolean".to_string(),
        "character varying" | "varchar" => "varchar".to_string(),
        "timestamp without time zone" => "timestamp".to_string(),
        "double precision" => "double".to_string(),
        "numeric" => "decimal".to_string(),
        other => other.to_string(),
    };
    // 保留长度/精度部分，但去掉空白与 unicode/signed 修饰的差异。
    let suffix = raw.trim().to_lowercase().replace(' ', "");
    match suffix.find('(') {
        Some(pos) => format!("{}{}", s, &suffix[pos..]),
        None => s,
    }
}

fn normalize_default(raw: Option<&str>) -> Option<String> {
    let value = raw?.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("null") { return None; }
    // 只归一整个已知常量，不能因字符串包含 CURRENT_TIMESTAMP 就当 now。
    if ["current_timestamp", "current_timestamp()", "now()", "getdate()"].iter().any(|known| value.eq_ignore_ascii_case(known)) {
        return Some("now".into());
    }
    // SQL 字符串/带引号身份的大小写与内嵌空白有语义，必须逐字保留。
    Some(value.to_string())
}

/// 主键身份是有序列向量；primary_key flag 不能证明复合列顺序。
pub fn ordered_primary_key(schema: &TableSchema) -> Result<Vec<String>> {
    let indexes: Vec<&IndexInfo> = schema.indexes.iter().filter(|i| i.primary).collect();
    if indexes.len() > 1 {
        return Err(anyhow!("PRIMARY_KEY_METADATA_UNKNOWN: 多个主键索引描述，不能猜测身份"));
    }
    if let Some(index) = indexes.first() {
        if index.columns.is_empty() || !index.unique {
            return Err(anyhow!("PRIMARY_KEY_METADATA_UNKNOWN: 主键没有完整有序列/唯一性"));
        }
        let mut seen = std::collections::HashSet::new();
        for name in &index.columns {
            if !seen.insert(name) || !schema.columns.iter().any(|c| &c.name == name) {
                return Err(anyhow!("PRIMARY_KEY_METADATA_UNKNOWN: 主键列重复或不存在"));
            }
        }
        return Ok(index.columns.clone());
    }
    let members: Vec<String> = schema.columns.iter().filter(|c| c.primary_key).map(|c| c.name.clone()).collect();
    if members.len() > 1 {
        return Err(anyhow!("PRIMARY_KEY_ORDER_UNKNOWN: 缺少真实 primary index.columns 顺序"));
    }
    Ok(members)
}

fn pk_index_name(schema: &TableSchema) -> Option<&str> {
    schema.indexes.iter().find(|i| i.primary).map(|i| i.name.as_str())
}

/// 把目标结构编译成方言步骤。`current` 是刚读取的真实结构。
pub fn compile(kind: &DbKind, current: &TableSchema, design: &TableDesign) -> Result<Vec<DesignStep>> {
    let protocol = kind.protocol();
    if !matches!(protocol, Protocol::Sqlite | Protocol::Mysql | Protocol::Postgres) {
        return Err(anyhow!("当前数据库类型尚未验证表结构设计保存；已拒绝执行，没有发送任何 SQL"));
    }
    let full = db::qualified(kind, &design.schema, &design.table);
    let orig_pk = ordered_primary_key(current)?;
    let target_pk: Vec<String> = design.primary_key.iter().filter(|s| !s.trim().is_empty()).cloned().collect();
    if target_pk.iter().any(|name| !design.columns.iter().any(|c| &c.name == name && !c.name.trim().is_empty())) {
        return Err(anyhow!("目标主键引用了不存在的列"));
    }
    if design.columns.iter().any(|c| c.name.trim().is_empty()) {
        return Err(anyhow!("列名不能为空"));
    }
    let mut seen = std::collections::HashSet::new();
    for c in &design.columns {
        let identity = if matches!(protocol, Protocol::Postgres) { c.name.clone() } else { c.name.to_ascii_lowercase() };
        if !seen.insert(identity) {
            return Err(anyhow!("列名重复：{}", c.name));
        }
    }
    if matches!(protocol, Protocol::Mysql) && design.columns.iter().filter(|c| c.auto_increment).count() > 1 {
        return Err(anyhow!("MySQL 只允许一个自增列"));
    }
    for c in &design.columns {
        let original = c.orig_name.as_deref().and_then(|name| find_column(current, name));
        if matches!(protocol, Protocol::Sqlite | Protocol::Postgres)
            && original.is_some_and(|old| old.auto_increment != c.auto_increment)
        {
            return Err(anyhow!("当前数据库不支持在设计器中修改自增属性；已拒绝执行"));
        }
        if c.auto_increment {
            match protocol {
                Protocol::Sqlite => {
                    if !target_pk.iter().any(|k| k == &c.name) {
                        return Err(anyhow!("SQLite 自增列「{}」必须是主键的一部分", c.name));
                    }
                }
                Protocol::Mysql => {
                    // InnoDB 允许非主键自增，但自增列必须是某个最终索引的首列。
                    let secondary_key = match &design.indexes {
                        Some(indexes) => indexes.iter().any(|i| i.columns.first() == Some(&c.name)),
                        None => current.indexes.iter().filter(|i| !i.primary)
                            .filter(|i| i.method.as_deref().map_or(true, |method| method.eq_ignore_ascii_case("BTREE")))
                            .any(|i| i.columns.first().is_some_and(|name| design.columns.iter().any(|target| target.name == c.name && target.orig_name.as_deref() == Some(name.as_str())))),
                    };
                    if target_pk.first() != Some(&c.name) && !secondary_key {
                        return Err(anyhow!("MySQL 自增列「{}」必须是一个索引的首列", c.name));
                    }
                    let existing_auto = original.is_some_and(|old| old.auto_increment);
                    if !existing_auto {
                        let ty = c.data_type.trim().split(['(', ' ']).next().unwrap_or("").to_ascii_lowercase();
                        if !["tinyint", "smallint", "mediumint", "int", "integer", "bigint", "float", "double", "real"].contains(&ty.as_str()) || c.nullable || c.default_value.is_some() {
                            return Err(anyhow!("MySQL 新自增列必须为非空数值列且不能设置默认值"));
                        }
                    }
                }
                _ => {}
            }
            if matches!(protocol, Protocol::Sqlite | Protocol::Postgres) && !original.is_some_and(|old| old.auto_increment && old.name == c.name && same_type(&old.data_type, c, kind) && old.default.as_deref() == c.default_value.as_deref()) {
                return Err(anyhow!("当前数据库不支持在设计器中修改自增属性或既有自增默认值；已拒绝执行"));
            }
        }
        if let Some(d) = c.default_value.as_deref() {
            validate_default(d)?;
        }
    }
    if matches!(protocol, Protocol::Sqlite) {
        // SQLite 只能安全地增列、删列、改名；改类型/默认值/主键需要整表重建，必须拒绝而不是静默跳过。
        for c in &design.columns {
            let Some(orig_name) = c.orig_name.as_deref() else { continue };
            let Some(actual) = find_column(current, orig_name) else { continue };
            if c.name != orig_name {
                continue;
            }
            let type_changed = !same_type(&actual.data_type, c, kind);
            let null_changed = actual.nullable != c.nullable;
            let default_changed = normalize_default(actual.default.as_deref()) != normalize_default(c.default_value.as_deref());
            if type_changed || null_changed || default_changed {
                return Err(anyhow!(
                    "SQLite 不支持直接修改列「{}」的类型/可空/默认值；请使用重建表，已拒绝执行",
                    c.name
                ));
            }
        }
        if orig_pk != target_pk {
            return Err(anyhow!("SQLite 不支持在现有表上调整主键；已拒绝执行"));
        }
        let existing: Vec<&str> = current.columns.iter().map(|c| c.name.as_str()).collect();
        for name in &existing {
            if !design.columns.iter().any(|c| c.orig_name.as_deref() == Some(name) || c.name == *name) {
                return Err(anyhow!("SQLite 删除列需要确认；当前版本拒绝在无法验证约束影响时删除列"));
            }
        }
    }
    let (index_drops, mut index_adds, fk_drops, fk_adds) =
        plan_indexes(kind, protocol, current, design)?;
    let mut steps: Vec<DesignStep> = Vec::new();
    steps.extend(index_drops);
    steps.extend(fk_drops);
    let pk_changed = orig_pk != target_pk;
    // 先撤销旧主键，避免删除/修改主键列时被约束阻塞。
    if pk_changed && !orig_pk.is_empty() {
        match protocol {
            Protocol::Mysql => steps.push(DesignStep { label: "撤销原主键".into(), sql: format!("ALTER TABLE {} DROP PRIMARY KEY", full) }),
            Protocol::Postgres => {
                let name = pk_index_name(current).ok_or_else(|| anyhow!("无法读取现有主键约束名，已拒绝执行"))?;
                steps.push(DesignStep {
                    label: "撤销原主键".into(),
                    sql: format!("ALTER TABLE {} DROP CONSTRAINT {}", full, db::quote_ident(kind, name)),
                });
            }
            _ => {}
        }
    }
    // 新增列（自增属性最后再补，避免「必须先有键」的方言限制）。
    for c in design.columns.iter().filter(|c| c.orig_name.is_none()) {
        let def = column_definition(kind, c, false)?;
        steps.push(DesignStep { label: format!("新增列 {}", c.name), sql: format!("ALTER TABLE {} ADD COLUMN {}", full, def) });
        if matches!(protocol, Protocol::Postgres) {
            if let Some(comment) = c.comment.as_deref().filter(|s| !s.is_empty()) {
                steps.push(DesignStep { label: format!("新增列 {} 注释", c.name), sql: format!("COMMENT ON COLUMN {}.{} IS {}", full, db::quote_ident(kind, &c.name), quote_literal(comment)) });
            }
        }
    }
    // 改名。
    for c in design.columns.iter().filter(|c| c.orig_name.is_some()) {
        let orig = c.orig_name.as_deref().unwrap();
        if c.name == orig {
            continue;
        }
        match protocol {
            Protocol::Sqlite => steps.push(DesignStep {
                label: format!("重命名列 {} → {}", orig, c.name),
                sql: format!("ALTER TABLE {} RENAME COLUMN {} TO {}", full, db::quote_ident(kind, orig), db::quote_ident(kind, &c.name)),
            }),
            Protocol::Postgres => steps.push(DesignStep {
                label: format!("重命名列 {} → {}", orig, c.name),
                sql: format!("ALTER TABLE {} RENAME COLUMN {} TO {}", full, db::quote_ident(kind, orig), db::quote_ident(kind, &c.name)),
            }),
            Protocol::Mysql => {
                let def = column_definition(kind, c, c.orig_name.as_deref().and_then(|name| find_column(current, name)).is_some_and(|old| old.auto_increment))?;
                steps.push(DesignStep {
                    label: format!("修改列 {} → {}", orig, c.name),
                    sql: format!("ALTER TABLE {} CHANGE COLUMN {} {}", full, db::quote_ident(kind, orig), def),
                });
            }
            _ => {}
        }
    }
    // 修改既有列定义。
    for c in design.columns.iter().filter(|c| c.orig_name.is_some()) {
        let orig = c.orig_name.as_deref().unwrap();
        let Some(actual) = find_column(current, orig) else { continue };
        let renamed = c.name != orig;
        match protocol {
            Protocol::Mysql => {
                if renamed {
                    continue; // CHANGE COLUMN 已携带完整定义。
                }
                let desired = column_definition(kind, c, actual.auto_increment)?;
                let current_type = base_type(kind, &ColumnDesign { data_type: actual.data_type.clone(), length: None, scale: None, unsigned: false, zerofill: false, ..c.clone() });
                let mut current_def = format!("{} {}", db::quote_ident(kind, &actual.name), actual.data_type);
                if !actual.nullable {
                    current_def.push_str(" NOT NULL");
                }
                if let Some(d) = actual.default.as_deref() {
                    current_def.push_str(&format!(" DEFAULT {}", d));
                }
                if actual.auto_increment {
                    current_def.push_str(" AUTO_INCREMENT");
                }
                if let Some(comment) = actual.comment.as_deref().filter(|s| !s.is_empty()) {
                    current_def.push_str(&format!(" COMMENT {}", quote_literal(comment)));
                }
                if current_def != desired {
                    steps.push(DesignStep { label: format!("修改列 {}", c.name), sql: format!("ALTER TABLE {} MODIFY COLUMN {}", full, desired) });
                }
                let _ = current_type;
            }
            Protocol::Postgres => {
                if renamed {
                    if !same_type(&actual.data_type, c, kind) {
                        steps.push(DesignStep {
                            label: format!("修改列 {} 类型", c.name),
                            sql: format!("ALTER TABLE {} ALTER COLUMN {} TYPE {}", full, db::quote_ident(kind, &c.name), base_type(kind, c)),
                        });
                    }
                    if actual.nullable != c.nullable {
                        steps.push(DesignStep {
                            label: format!("修改列 {} 可空性", c.name),
                            sql: format!(
                                "ALTER TABLE {} ALTER COLUMN {} {}",
                                full,
                                db::quote_ident(kind, &c.name),
                                if c.nullable { "DROP NOT NULL" } else { "SET NOT NULL" }
                            ),
                        });
                    }
                    continue;
                }
                if !same_type(&actual.data_type, c, kind) {
                    steps.push(DesignStep {
                        label: format!("修改列 {} 类型", c.name),
                        sql: format!("ALTER TABLE {} ALTER COLUMN {} TYPE {}", full, db::quote_ident(kind, &c.name), base_type(kind, c)),
                    });
                }
                if actual.nullable != c.nullable {
                    steps.push(DesignStep {
                        label: format!("修改列 {} 可空性", c.name),
                        sql: format!(
                            "ALTER TABLE {} ALTER COLUMN {} {}",
                            full,
                            db::quote_ident(kind, &c.name),
                            if c.nullable { "DROP NOT NULL" } else { "SET NOT NULL" }
                        ),
                    });
                }
                let desired_default = normalize_default(c.default_value.as_deref());
                if normalize_default(actual.default.as_deref()) != desired_default {
                    let sql = match c.default_value.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                        Some(d) => format!("ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {}", full, db::quote_ident(kind, &c.name), d),
                        None => format!("ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT", full, db::quote_ident(kind, &c.name)),
                    };
                    steps.push(DesignStep { label: format!("修改列 {} 默认值", c.name), sql });
                }
                if actual.comment.as_deref().unwrap_or("") != c.comment.as_deref().unwrap_or("") {
                    let value = c.comment.as_deref().filter(|s| !s.is_empty()).map(quote_literal).unwrap_or_else(|| "NULL".into());
                    steps.push(DesignStep {
                        label: format!("修改列 {} 注释", c.name),
                        sql: format!("COMMENT ON COLUMN {}.{} IS {}", full, db::quote_ident(kind, &c.name), value),
                    });
                }
            }
            _ => {}
        }
    }
    // 删除列。
    for actual in current.columns.iter() {
        let kept = design.columns.iter().any(|c| c.orig_name.as_deref() == Some(actual.name.as_str()) || c.name == actual.name);
        if !kept {
            steps.push(DesignStep {
                label: format!("删除列 {}", actual.name),
                sql: format!("ALTER TABLE {} DROP COLUMN {}", full, db::quote_ident(kind, &actual.name)),
            });
        }
    }
    // 主键变更。
    if pk_changed && !target_pk.is_empty() {
        steps.push(DesignStep {
            label: "建立目标主键".into(),
            sql: format!("ALTER TABLE {} ADD PRIMARY KEY ({})", full, target_pk.iter().map(|c| db::quote_ident(kind, c)).collect::<Vec<_>>().join(", ")),
        });
    }
    // MySQL 自增列最后追加 MODIFY，确保此时主键和普通/唯一索引都已建立。
    if matches!(protocol, Protocol::Mysql) {
        steps.append(&mut index_adds);
        for c in design.columns.iter().filter(|c| c.auto_increment) {
            let Some(actual) = c.orig_name.as_deref().and_then(|n| find_column(current, n)) else {
                if c.orig_name.is_none() {
                    steps.push(DesignStep {
                        label: format!("设置自增 {}", c.name),
                        sql: format!("ALTER TABLE {} MODIFY COLUMN {}", full, column_definition(kind, c, true)?),
                    });
                }
                continue;
            };
            if !actual.auto_increment {
                steps.push(DesignStep {
                    label: format!("设置自增 {}", c.name),
                    sql: format!("ALTER TABLE {} MODIFY COLUMN {}", full, column_definition(kind, c, true)?),
                });
            }
        }
    }
    // 表注释。
    if let Some(comment) = design.comment.as_deref() {
        match protocol {
            Protocol::Mysql => steps.push(DesignStep {
                label: "修改表注释".into(),
                sql: format!("ALTER TABLE {} COMMENT = {}", full, quote_literal(comment)),
            }),
            Protocol::Postgres => steps.push(DesignStep {
                label: "修改表注释".into(),
                sql: format!("COMMENT ON TABLE {} IS {}", full, if comment.is_empty() { "NULL".into() } else { quote_literal(comment) }),
            }),
            Protocol::Sqlite => {
                if !comment.is_empty() {
                    return Err(anyhow!("SQLite 不支持表注释，已拒绝执行"));
                }
            }
            _ => {}
        }
    }
    steps.extend(index_adds);
    steps.extend(fk_adds);
    Ok(steps)
}

/// 读回并与目标比对，返回差异说明。
pub fn verify(current: &TableSchema, design: &TableDesign, kind: &DbKind) -> Vec<String> {
    let mut problems = Vec::new();
    let actual_names: Vec<String> = current.columns.iter().map(|c| c.name.clone()).collect();
    let target_names: Vec<String> = design.columns.iter().map(|c| c.name.clone()).filter(|n| !n.is_empty()).collect();
    for name in &target_names {
        if !actual_names.iter().any(|a| a == name) {
            problems.push(format!("读回缺少列「{}」", name));
        }
    }
    for name in &actual_names {
        if !target_names.iter().any(|t| t == name) {
            problems.push(format!("读回仍存在应删除的列「{}」", name));
        }
    }
    for c in &design.columns {
        let Some(actual) = current.columns.iter().find(|a| a.name == c.name) else { continue };
        if !same_type(&actual.data_type, c, kind) {
            problems.push(format!("列「{}」类型不一致：实际 {}，目标 {}", c.name, actual.data_type, base_type(kind, c)));
        }
        if actual.nullable != c.nullable {
            problems.push(format!("列「{}」可空性不一致", c.name));
        }
        if normalize_default(actual.default.as_deref()) != normalize_default(c.default_value.as_deref()) {
            problems.push(format!("列「{}」默认值不一致：实际 {:?}，目标 {:?}", c.name, actual.default, c.default_value));
        }
        if actual.auto_increment != c.auto_increment {
            problems.push(format!("列「{}」自增属性不一致", c.name));
        }
        let actual_comment = actual.comment.as_deref().unwrap_or("");
        let target_comment = c.comment.as_deref().unwrap_or("");
        if matches!(kind.protocol(), Protocol::Mysql) && actual_comment != target_comment {
            problems.push(format!("列「{}」注释不一致", c.name));
        }
    }
    let target_pk: Vec<String> = design.primary_key.iter().filter(|s| !s.trim().is_empty()).cloned().collect();
    match ordered_primary_key(current) {
        Ok(actual_pk) if actual_pk != target_pk => problems.push(format!("主键不一致：实际 {:?}，目标 {:?}", actual_pk, target_pk)),
        Ok(_) => {},
        Err(error) => problems.push(format!("主键无法核验：{error:#}")),
    }
    if let Some(target_comment) = design.comment.as_deref() {
        let actual = current.comment.as_deref().unwrap_or("");
        if actual != target_comment {
            problems.push(format!("表注释不一致：实际 {:?}，目标 {:?}", current.comment, target_comment));
        }
    }
    if let Some(target) = &design.indexes {
        let actual: Vec<&IndexInfo> = current.indexes.iter().filter(|i| !i.primary).collect();
        for t in target {
            match actual.iter().find(|a| norm_name(&a.name) == norm_name(&t.name)) {
                None => problems.push(format!("索引「{}」未创建", t.name)),
                Some(a) => {
                    if a.unique != t.unique || !cols_equal(&a.columns, &t.columns) || !same_index_method(a.method.as_deref(), t.method.as_deref()) {
                        problems.push(format!("索引「{}」定义不一致", t.name));
                    }
                }
            }
        }
        for a in &actual {
            if !target.iter().any(|t| norm_name(&t.name) == norm_name(&a.name)) {
                problems.push(format!("索引「{}」仍然存在", a.name));
            }
        }
    }
    if let Some(target) = &design.foreign_keys {
        for t in target {
            match current.foreign_keys.iter().find(|a| norm_name(&a.name) == norm_name(&t.name)) {
                None => problems.push(format!("外键「{}」未创建", t.name)),
                Some(a) => {
                    if !fk_equal(a, t, &design.schema) {
                        problems.push(format!("外键「{}」定义不一致", t.name));
                    }
                }
            }
        }
        for a in &current.foreign_keys {
            if !target.iter().any(|t| norm_name(&t.name) == norm_name(&a.name)) {
                problems.push(format!("外键「{}」仍然存在", a.name));
            }
        }
    }
    problems
}

/// 给结构同步使用的非破坏白名单：按结构属性校验，不靠 SQL substring 判风险。
pub fn compile_non_destructive(kind: &DbKind, current: &TableSchema, design: &TableDesign) -> Result<Vec<DesignStep>> {
    for column in &current.columns {
        if !design.columns.iter().any(|target| target.orig_name.as_deref() == Some(column.name.as_str())) {
            return Err(anyhow!("STRUCTURE_DROP_NOT_APPROVED: 非破坏计划丢失目标列 {}", column.name));
        }
    }
    let original_pk = ordered_primary_key(current)?;
    if !original_pk.is_empty() && original_pk != design.primary_key {
        return Err(anyhow!("STRUCTURE_PRIMARY_KEY_DROP_NOT_APPROVED: 修改既有有序主键需要显式破坏性批准"));
    }
    if let Some(indexes) = &design.indexes {
        for original in current.indexes.iter().filter(|i| !i.primary) {
            let Some(target) = indexes.iter().find(|i| i.name == original.name) else {
                return Err(anyhow!("STRUCTURE_INDEX_DROP_NOT_APPROVED: 非破坏计划丢失目标索引 {}", original.name));
            };
            if original.columns != target.columns || original.unique != target.unique {
                return Err(anyhow!("STRUCTURE_INDEX_REPLACEMENT_NOT_APPROVED: 同名索引有序列/唯一性被替换"));
            }
        }
    }
    if design.foreign_keys.is_some() {
        return Err(anyhow!("STRUCTURE_FOREIGN_KEY_CHANGE_UNSUPPORTED: 结构同步缺完整引用 schema，不自动改外键"));
    }
    compile(kind, current, design)
}

fn norm_name(s: &str) -> String {
    s.to_string()
}

fn norm_action(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn cols_equal(a: &[String], b: &[String]) -> bool {
    // 复合索引/外键是有序 tuple；不可排序、trim、折叠引用身份或逗号 join。
    a == b
}

fn fk_equal(a: &ForeignKeyInfo, t: &ForeignKeyDesign, schema: &str) -> bool {
    cols_equal(&a.columns, &t.columns)
        && cols_equal(&a.ref_columns, &t.ref_columns)
        && a.ref_table == t.ref_table
        && a.ref_schema.as_deref().is_some_and(|actual| actual == t.ref_schema.as_deref().unwrap_or(schema))
        && norm_action(a.on_delete.as_deref().unwrap_or("")) == norm_action(t.on_delete.as_deref().unwrap_or(""))
        && norm_action(a.on_update.as_deref().unwrap_or("")) == norm_action(t.on_update.as_deref().unwrap_or(""))
}

fn drop_index(kind: &DbKind, protocol: Protocol, schema: &str, name: &str, full: &str) -> String {
    match protocol {
        Protocol::Mysql => format!("ALTER TABLE {} DROP INDEX {}", full, db::quote_ident(kind, name)),
        Protocol::Postgres => format!("DROP INDEX {}", db::qualified(kind, schema, name)),
        _ => format!("DROP INDEX {}", db::quote_ident(kind, name)),
    }
}

/// 索引方法归一：缺省/空与 BTREE 等价。
fn same_index_method(a: Option<&str>, b: Option<&str>) -> bool {
    fn norm(m: Option<&str>) -> String { m.unwrap_or("").trim().to_ascii_lowercase() }
    let (x, y) = (norm(a), norm(b));
    (x.is_empty() || x == "btree") == (y.is_empty() || y == "btree")
}

fn create_index(kind: &DbKind, protocol: Protocol, name: &str, columns: &[String], unique: bool, method: Option<&str>, descending: &[String], full: &str) -> String {
    let cols: Vec<String> = columns.iter().map(|c| {
        let quoted = db::quote_ident(kind, c);
        if descending.iter().any(|d| norm_name(d) == norm_name(c)) { format!("{} DESC", quoted) } else { quoted }
    }).collect();
    let uniq = if unique { "UNIQUE " } else { "" };
    let method = method.map(str::trim).filter(|m| !m.is_empty());
    match protocol {
        Protocol::Mysql => {
            let using = method.filter(|m| !m.eq_ignore_ascii_case("btree")).map(|m| format!(" USING {}", m.to_ascii_uppercase())).unwrap_or_default();
            format!("ALTER TABLE {} ADD {}INDEX {} ({}){}", full, uniq, db::quote_ident(kind, name), cols.join(", "), using)
        }
        Protocol::Postgres => match method {
            Some(m) => format!("CREATE {}INDEX {} ON {} USING {} ({})", uniq, db::quote_ident(kind, name), full, m.to_ascii_lowercase(), cols.join(", ")),
            None => format!("CREATE {}INDEX {} ON {} ({})", uniq, db::quote_ident(kind, name), full, cols.join(", ")),
        },
        _ => format!("CREATE {}INDEX {} ON {} ({})", uniq, db::quote_ident(kind, name), full, cols.join(", ")),
    }
}

fn drop_foreign_key(kind: &DbKind, protocol: Protocol, name: &str, full: &str) -> String {
    match protocol {
        Protocol::Mysql => format!("ALTER TABLE {} DROP FOREIGN KEY {}", full, db::quote_ident(kind, name)),
        _ => format!("ALTER TABLE {} DROP CONSTRAINT {}", full, db::quote_ident(kind, name)),
    }
}

fn create_foreign_key(kind: &DbKind, schema: &str, t: &ForeignKeyDesign, full: &str) -> String {
    let cols: Vec<String> = t.columns.iter().map(|c| db::quote_ident(kind, c)).collect();
    let refs: Vec<String> = t.ref_columns.iter().map(|c| db::quote_ident(kind, c)).collect();
    let ref_schema = t.ref_schema.as_deref().unwrap_or(schema);
    let ref_full = db::qualified(kind, ref_schema, &t.ref_table);
    let mut sql = format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        full,
        db::quote_ident(kind, &t.name),
        cols.join(", "),
        ref_full,
        refs.join(", ")
    );
    if let Some(od) = t.on_delete.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        sql.push_str(&format!(" ON DELETE {}", od));
    }
    if let Some(ou) = t.on_update.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        sql.push_str(&format!(" ON UPDATE {}", ou));
    }
    sql
}

/// 生成索引/外键的删除与新增步骤；不支持的情形明确拒绝。
fn plan_indexes(
    kind: &DbKind,
    protocol: Protocol,
    current: &TableSchema,
    design: &TableDesign,
) -> Result<(Vec<DesignStep>, Vec<DesignStep>, Vec<DesignStep>, Vec<DesignStep>)> {
    let full = db::qualified(kind, &design.schema, &design.table);
    let mut index_drops = Vec::new();
    let mut index_adds = Vec::new();
    let mut fk_drops = Vec::new();
    let mut fk_adds = Vec::new();

    if let Some(target) = &design.indexes {
        let existing: Vec<&IndexInfo> = current.indexes.iter().filter(|i| !i.primary).collect();
        for t in target {
            if t.name.trim().is_empty() || t.columns.is_empty() {
                return Err(anyhow!("索引名称与列不能为空"));
            }
            for c in &t.columns {
                if !design.columns.iter().any(|d| norm_name(&d.name) == norm_name(c)) {
                    return Err(anyhow!("索引「{}」引用了不存在的列「{}」", t.name, c));
                }
            }
        }
        for a in &existing {
            match target.iter().find(|t| norm_name(&t.name) == norm_name(&a.name)) {
                Some(t) => {
                    if a.unique != t.unique || !cols_equal(&a.columns, &t.columns) || !same_index_method(a.method.as_deref(), t.method.as_deref()) || !t.descending.is_empty() {
                        index_drops.push(DesignStep {
                            label: format!("重建索引 {}", a.name),
                            sql: drop_index(kind, protocol, &design.schema, &a.name, &full),
                        });
                        index_adds.push(DesignStep {
                            label: format!("重建索引 {}", t.name),
                            sql: create_index(kind, protocol, &t.name, &t.columns, t.unique, t.method.as_deref(), &t.descending, &full),
                        });
                    }
                }
                None => index_drops.push(DesignStep {
                    label: format!("删除索引 {}", a.name),
                    sql: drop_index(kind, protocol, &design.schema, &a.name, &full),
                }),
            }
        }
        for t in target {
            if !existing.iter().any(|a| norm_name(&a.name) == norm_name(&t.name)) {
                index_adds.push(DesignStep {
                    label: format!("创建索引 {}", t.name),
                    sql: create_index(kind, protocol, &t.name, &t.columns, t.unique, t.method.as_deref(), &t.descending, &full),
                });
            }
        }
    }

    if let Some(target) = &design.foreign_keys {
        if matches!(protocol, Protocol::Sqlite) {
            let same = current.foreign_keys.len() == target.len()
                && target.iter().all(|t| {
                    current
                        .foreign_keys
                        .iter()
                        .any(|a| norm_name(&a.name) == norm_name(&t.name) && fk_equal(a, t, &design.schema))
                });
            if !same {
                return Err(anyhow!("SQLite 不支持通过设计器增删外键；如需修改请重建表。已拒绝执行"));
            }
        } else {
            for a in &current.foreign_keys {
                match target.iter().find(|t| norm_name(&t.name) == norm_name(&a.name)) {
                    Some(t) if fk_equal(a, t, &design.schema) => {}
                    Some(t) => {
                        fk_drops.push(DesignStep {
                            label: format!("重建外键 {}", a.name),
                            sql: drop_foreign_key(kind, protocol, &a.name, &full),
                        });
                        fk_adds.push(DesignStep {
                            label: format!("重建外键 {}", t.name),
                            sql: create_foreign_key(kind, &design.schema, t, &full),
                        });
                    }
                    None => fk_drops.push(DesignStep {
                        label: format!("删除外键 {}", a.name),
                        sql: drop_foreign_key(kind, protocol, &a.name, &full),
                    }),
                }
            }
            for t in target {
                if t.columns.is_empty() || t.ref_columns.is_empty() || t.ref_table.trim().is_empty() {
                    return Err(anyhow!("外键「{}」的定义不完整", t.name));
                }
                if !current.foreign_keys.iter().any(|a| norm_name(&a.name) == norm_name(&t.name)) {
                    fk_adds.push(DesignStep {
                        label: format!("创建外键 {}", t.name),
                        sql: create_foreign_key(kind, &design.schema, t, &full),
                    });
                }
            }
        }
    }

    Ok((index_drops, index_adds, fk_drops, fk_adds))
}

async fn run_sqlite(db: &Db, steps: &[DesignStep]) -> (Vec<DesignStepResult>, String) {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let Db::Sqlite(pool) = db else { unreachable!() };
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (fail_all(steps, &format!("开启事务失败：{e:#}"), 0), "none".into()),
    };
    let mut results = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        match crate::sql_history::query(&step.sql).execute(&mut *tx).await {
            Ok(_) => results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "applied".into(), error: None }),
            Err(e) => {
                let message = format!("{e:#}");
                results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "failed".into(), error: Some(message.clone()) });
                results.extend(skip_rest(steps, index + 1));
                let _ = tx.rollback().await;
                return (results, "rolled_back".into());
            }
        }
    }
    match tx.commit().await {
        Ok(_) => (results, "committed".into()),
        Err(e) => {
            let message = format!("提交失败：{e:#}");
            let mut out = vec![DesignStepResult { label: "提交事务".into(), sql: String::new(), status: "failed".into(), error: Some(message) }];
            out.extend(skip_rest(steps, 0));
            (out, "rolled_back".into())
        }
    }

    }).await
}

async fn run_postgres(db: &Db, steps: &[DesignStep]) -> (Vec<DesignStepResult>, String) {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let Db::Postgres(pool) = db else { unreachable!() };
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return (fail_all(steps, &format!("开启事务失败：{e:#}"), 0), "none".into()),
    };
    let mut results = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        match crate::sql_history::query(&step.sql).execute(&mut *tx).await {
            Ok(_) => results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "applied".into(), error: None }),
            Err(e) => {
                let message = format!("{e:#}");
                results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "failed".into(), error: Some(message) });
                results.extend(skip_rest(steps, index + 1));
                let _ = tx.rollback().await;
                return (results, "rolled_back".into());
            }
        }
    }
    match tx.commit().await {
        Ok(_) => (results, "committed".into()),
        Err(e) => {
            let message = format!("提交失败：{e:#}");
            let mut out = vec![DesignStepResult { label: "提交事务".into(), sql: String::new(), status: "failed".into(), error: Some(message) }];
            out.extend(skip_rest(steps, 0));
            (out, "rolled_back".into())
        }
    }

    }).await
}

async fn run_mysql(db: &Db, steps: &[DesignStep]) -> (Vec<DesignStepResult>, String) {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let Db::Mysql(pool) = db else { unreachable!() };
    let mut results = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        match crate::sql_history::query(&step.sql).execute(pool).await {
            Ok(_) => results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "applied".into(), error: None }),
            Err(e) => {
                results.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "failed".into(), error: Some(format!("{e:#}")) });
                results.extend(skip_rest(steps, index + 1));
                return (results, "implicit".into());
            }
        }
    }
    (results, "implicit".into())

    }).await
}

fn fail_all(steps: &[DesignStep], message: &str, from: usize) -> Vec<DesignStepResult> {
    let mut out = Vec::new();
    if let Some(step) = steps.get(from) {
        out.push(DesignStepResult { label: step.label.clone(), sql: step.sql.clone(), status: "failed".into(), error: Some(message.into()) });
    }
    out.extend(skip_rest(steps, from + 1));
    out
}

fn skip_rest(steps: &[DesignStep], from: usize) -> Vec<DesignStepResult> {
    steps.iter().skip(from).map(|s| DesignStepResult {
        label: s.label.clone(),
        sql: s.sql.clone(),
        status: "skipped".into(),
        error: Some("因前一步失败未执行".into()),
    }).collect()
}

/// 完整流程：读取当前结构 → 编译 → 执行 → 读回比对。
pub async fn apply(db: &Db, kind: &DbKind, design: &TableDesign) -> Result<DesignReceipt> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    apply_with_policy(db, kind, design, true).await

    }).await
}

/// 结构同步的受限执行策略：即使预览 CAS 之后新列/索引出现，也不能被完整草稿误删。
pub async fn apply_non_destructive(db: &Db, kind: &DbKind, design: &TableDesign) -> Result<DesignReceipt> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    apply_with_policy(db, kind, design, false).await

    }).await
}

async fn apply_with_policy(db: &Db, kind: &DbKind, design: &TableDesign, allow_drop: bool) -> Result<DesignReceipt> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    ensure!(design.checks.is_none() && design.triggers.is_none() && design.options.is_none() && design.base_revision.is_none() && design.source_fingerprint.is_none(), "TABLE_CONTROLLED_DESIGN_REQUIRED: 扩展设计必须通过基线/预览/一次性授权受控接口保存，旧入口禁止忽略字段");
    let current = crate::schema::table_schema(db, kind.clone(), &design.schema, &design.table).await?;
    let steps = if allow_drop { compile(kind, &current, design)? } else { compile_non_destructive(kind, &current, design)? };
    if steps.is_empty() {
        let mismatches = verify(&current, design, kind);
        return Ok(DesignReceipt {
            steps: Vec::new(),
            verified: mismatches.is_empty(),
            unverified: !mismatches.is_empty(),
            mismatches,
            transaction: "none".into(),
            schema: Some(current),
        });
    }
    let (results, transaction) = match db {
        Db::Sqlite(_) => run_sqlite(db, &steps).await,
        Db::Postgres(_) => run_postgres(db, &steps).await,
        Db::Mysql(_) => run_mysql(db, &steps).await,
        _ => return Err(anyhow!("当前适配器不支持表结构设计保存")),
    };
    let applied = results.iter().any(|r| r.status == "applied");
    let all_ok = results.iter().all(|r| r.status == "applied");
    if !applied {
        return Ok(DesignReceipt { steps: results, verified: false, unverified: false, mismatches: Vec::new(), transaction, schema: None });
    }
    // 读回：即使整批失败也尝试读回，给出真实状态。
    match crate::schema::table_schema(db, kind.clone(), &design.schema, &design.table).await {
        Ok(actual) => {
            let mismatches = if all_ok { verify(&actual, design, kind) } else { Vec::new() };
            let verified = all_ok && mismatches.is_empty();
            let unverified = all_ok && !mismatches.is_empty();
            Ok(DesignReceipt { steps: results, verified, unverified, mismatches, transaction, schema: Some(actual) })
        }
        Err(e) => {
            let mut mismatches = vec![format!("执行后读取表结构失败，无法确认结果：{e:#}")];
            if !all_ok {
                mismatches.clear();
            }
            let verified = false;
            let unverified = all_ok;
            Ok(DesignReceipt { steps: results, verified, unverified, mismatches, transaction, schema: None })
        }
    }

    }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DbKind, Protocol};

    fn sqlite() -> DbKind {
        DbKind::of("sqlite")
    }

    fn current_schema() -> TableSchema {
        TableSchema {
            table: "t".into(),
            database: "main".into(),
            columns: vec![
                ColumnInfo { name: "id".into(), data_type: "integer".into(), nullable: false, default: None, primary_key: true, auto_increment: false, comment: None, length: None, precision: None, scale: None },
                ColumnInfo { name: "name".into(), data_type: "varchar(50)".into(), nullable: true, default: None, primary_key: false, auto_increment: false, comment: None, length: Some(50), precision: None, scale: None },
            ],
            indexes: vec![],
            foreign_keys: vec![],
            comment: None,
            engine: None,
            collation: None,
            row_count: None,
        }
    }

    fn design(columns: Vec<ColumnDesign>, pk: Vec<String>) -> TableDesign {
        TableDesign { schema: "main".into(), table: "t".into(), columns, primary_key: pk, comment: None, indexes: None, foreign_keys: None, checks: None, triggers: None, options: None, base_revision: None, source_fingerprint: None }
    }

    fn col(name: &str, orig: Option<&str>, ty: &str, nullable: bool) -> ColumnDesign {
        ColumnDesign { name: name.into(), orig_name: orig.map(str::to_string), data_type: ty.into(), length: None, scale: None, nullable, default_value: None, auto_increment: false, comment: None, unsigned: false, zerofill: false }
    }

    #[test]
    fn sqlite_add_column_compiles() {
        let mut columns = vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)];
        columns.push(col("email", None, "varchar(120)", true));
        let steps = compile(&sqlite(), &current_schema(), &design(columns, vec!["id".into()])).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].sql.contains("ADD COLUMN"));
        assert!(steps[0].sql.contains("\"email\""));
    }

    #[test]
    fn sqlite_rejects_type_change() {
        let mut columns = vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "text", true)];
        let err = compile(&sqlite(), &current_schema(), &design(columns.drain(..).collect(), vec!["id".into()])).unwrap_err();
        assert!(err.to_string().contains("重建表"));
    }

    #[test]
    fn sqlite_rejects_default_change() {
        let mut changed = col("name", Some("name"), "varchar(50)", true);
        changed.default_value = Some("'x'".into());
        let columns = vec![col("id", Some("id"), "integer", false), changed];
        let err = compile(&sqlite(), &current_schema(), &design(columns, vec!["id".into()])).unwrap_err();
        assert!(err.to_string().contains("默认值"));
    }

    #[test]
    fn rejects_unknown_protocol() {
        let kind = DbKind::of("mssql");
        let err = compile(&kind, &current_schema(), &design(vec![], vec![])).unwrap_err();
        assert!(err.to_string().contains("拒绝"));
    }

    #[test]
    fn rejects_dangling_primary_key() {
        let err = compile(&sqlite(), &current_schema(), &design(vec![col("id", Some("id"), "integer", false)], vec!["missing".into()])).unwrap_err();
        assert!(err.to_string().contains("主键"));
    }

    #[test]
    fn rejects_duplicate_column_names() {
        let columns = vec![col("id", Some("id"), "integer", false), col("ID", None, "integer", true)];
        let err = compile(&sqlite(), &current_schema(), &design(columns, vec!["id".into()])).unwrap_err();
        assert!(err.to_string().contains("重复"));
    }

    #[test]
    fn rejects_injection_in_default() {
        let mut c = col("x", None, "text", true);
        c.default_value = Some("'a'; DROP TABLE t".into());
        let err = compile(&sqlite(), &current_schema(), &design(vec![col("id", Some("id"), "integer", false), c], vec!["id".into()])).unwrap_err();
        assert!(err.to_string().contains("默认值"));
    }

    #[test]
    fn verify_reports_missing_column() {
        let problems = verify(&current_schema(), &design(vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true), col("x", None, "text", true)], vec!["id".into()]), &sqlite());
        assert!(problems.iter().any(|p| p.contains("缺少列")));
    }

    #[test]
    fn verify_clean_when_equal() {
        let problems = verify(&current_schema(), &design(vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)], vec!["id".into()]), &sqlite());
        assert!(problems.is_empty(), "{problems:?}");
    }

    #[test]
    fn sqlite_adds_index() {
        let d = TableDesign {
            schema: "main".into(),
            table: "t".into(),
            columns: vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)],
            primary_key: vec!["id".into()],
            comment: None,
            indexes: Some(vec![IndexDesign { name: "idx_name".into(), columns: vec!["name".into()], unique: false , ..Default::default() }]),
            foreign_keys: None, checks: None, triggers: None, options: None, base_revision: None, source_fingerprint: None,
        };
        let steps = compile(&sqlite(), &current_schema(), &d).unwrap();
        assert!(steps.iter().any(|s| s.sql.contains("CREATE INDEX") && s.sql.contains("idx_name")));
    }

    #[test]
    fn sqlite_rejects_foreign_key_change() {
        let d = TableDesign {
            schema: "main".into(),
            table: "t".into(),
            columns: vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)],
            primary_key: vec!["id".into()],
            comment: None,
            indexes: None,
            foreign_keys: Some(vec![ForeignKeyDesign {
                name: "fk1".into(),
                columns: vec!["id".into()],
                ref_table: "other".into(),
                ref_schema: None,
                ref_columns: vec!["id".into()],
                on_update: None,
                on_delete: None,
            }]),
            checks: None, triggers: None, options: None, base_revision: None, source_fingerprint: None,
        };
        let err = compile(&sqlite(), &current_schema(), &d).unwrap_err();
        assert!(err.to_string().contains("外键"));
    }

    #[test]
    fn mysql_drops_removed_index() {
        let mut cur = current_schema();
        cur.indexes = vec![IndexInfo {
            name: "idx_old".into(),
            columns: vec!["name".into()],
            unique: false,
            primary: false,
            method: None,
        }];
        let d = TableDesign {
            schema: "main".into(),
            table: "t".into(),
            columns: vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)],
            primary_key: vec!["id".into()],
            comment: None,
            indexes: Some(vec![]),
            foreign_keys: None, checks: None, triggers: None, options: None, base_revision: None, source_fingerprint: None,
        };
        let steps = compile(&DbKind::of("mysql"), &cur, &d).unwrap();
        assert!(steps.iter().any(|s| s.sql.contains("DROP INDEX") && s.sql.contains("idx_old")));
    }

    #[test]
    fn verify_reports_missing_index() {
        let d = TableDesign {
            schema: "main".into(),
            table: "t".into(),
            columns: vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)],
            primary_key: vec!["id".into()],
            comment: None,
            indexes: Some(vec![IndexDesign { name: "idx_new".into(), columns: vec!["name".into()], unique: true , ..Default::default() }]),
            foreign_keys: None, checks: None, triggers: None, options: None, base_revision: None, source_fingerprint: None,
        };
        let problems = verify(&current_schema(), &d, &sqlite());
        assert!(problems.iter().any(|p| p.contains("idx_new")));
    }

    #[test]
    fn ordered_primary_key_uses_index_not_column_flags() {
        let mut current = current_schema();
        current.columns[1].primary_key = true;
        current.indexes.push(IndexInfo { name: "pk_t".into(), columns: vec!["name".into(), "id".into()], unique: true, primary: true, method: None });
        assert_eq!(ordered_primary_key(&current).unwrap(), vec!["name", "id"]);
        current.indexes.clear();
        assert!(ordered_primary_key(&current).unwrap_err().to_string().contains("PRIMARY_KEY_ORDER_UNKNOWN"));
    }

    #[test]
    fn ordered_primary_key_rejects_unknown_or_duplicate_members() {
        let mut current = current_schema();
        current.indexes.push(IndexInfo { name: "pk_t".into(), columns: vec!["id".into(), "id".into()], unique: true, primary: true, method: None });
        assert!(ordered_primary_key(&current).is_err());
        current.indexes[0].columns = vec!["missing".into()];
        assert!(ordered_primary_key(&current).is_err());
        current.indexes[0].columns = vec!["id".into()];
        current.indexes[0].unique = false;
        assert!(ordered_primary_key(&current).is_err());
    }

    #[test]
    fn composite_index_order_and_quoted_identity_are_not_sets() {
        assert!(!cols_equal(&["a".into(), "b".into()], &["b".into(), "a".into()]));
        assert!(!cols_equal(&["a,b".into()], &["a".into(), "b".into()]));
        assert!(!cols_equal(&["A".into()], &["a".into()]));
        assert!(!cols_equal(&[" a".into()], &["a".into()]));
        let mut current = current_schema();
        current.indexes.push(IndexInfo { name: "idx_pair".into(), columns: vec!["id".into(), "name".into()], unique: false, primary: false, method: None });
        let mut draft = design(vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)], vec!["id".into()]);
        draft.indexes = Some(vec![IndexDesign { name: "idx_pair".into(), columns: vec!["name".into(), "id".into()], unique: false , ..Default::default() }]);
        assert!(verify(&current, &draft, &sqlite()).iter().any(|problem| problem.contains("索引")));
        assert!(compile_non_destructive(&DbKind::of("mysql"), &current, &draft).unwrap_err().to_string().contains("REPLACEMENT_NOT_APPROVED"));
    }

    #[test]
    fn non_destructive_compile_rejects_fresh_extra_column_and_index() {
        let mut current = current_schema();
        let mut draft = design(vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)], vec!["id".into()]);
        current.columns.push(ColumnInfo { name: "concurrent".into(), data_type: "text".into(), nullable: true, default: None, primary_key: false, auto_increment: false, comment: None, length: None, precision: None, scale: None });
        assert!(compile_non_destructive(&DbKind::of("mysql"), &current, &draft).unwrap_err().to_string().contains("STRUCTURE_DROP_NOT_APPROVED"));
        current.columns.pop();
        current.indexes.push(IndexInfo { name: "concurrent_index".into(), columns: vec!["name".into()], unique: false, primary: false, method: None });
        draft.indexes = Some(Vec::new());
        assert!(compile_non_destructive(&DbKind::of("mysql"), &current, &draft).unwrap_err().to_string().contains("STRUCTURE_INDEX_DROP_NOT_APPROVED"));
    }

    #[test]
    fn non_destructive_compile_rejects_existing_primary_key_reordering() {
        let mut current = current_schema();
        current.columns[1].primary_key = true;
        current.indexes.push(IndexInfo { name: "pk_t".into(), columns: vec!["id".into(), "name".into()], unique: true, primary: true, method: None });
        let draft = design(vec![col("id", Some("id"), "integer", false), col("name", Some("name"), "varchar(50)", true)], vec!["name".into(), "id".into()]);
        assert!(compile_non_destructive(&DbKind::of("mysql"), &current, &draft).unwrap_err().to_string().contains("STRUCTURE_PRIMARY_KEY_DROP_NOT_APPROVED"));
    }

    #[test]
    fn foreign_key_compares_reference_schema_and_order_without_guessing_unknown() {
        let mut actual = ForeignKeyInfo { name: "fk_t".into(), columns: vec!["id".into(), "name".into()], ref_table: "ref".into(), ref_schema: Some("left".into()), ref_columns: vec!["a".into(), "b".into()], on_update: None, on_delete: None };
        let mut desired = ForeignKeyDesign { name: "fk_t".into(), columns: actual.columns.clone(), ref_table: "ref".into(), ref_schema: Some("left".into()), ref_columns: actual.ref_columns.clone(), on_update: None, on_delete: None };
        assert!(fk_equal(&actual, &desired, "local"));
        desired.ref_schema = Some("right".into());
        assert!(!fk_equal(&actual, &desired, "local"));
        desired.ref_schema = Some("left".into());
        desired.ref_columns.reverse();
        assert!(!fk_equal(&actual, &desired, "local"));
        desired.ref_columns.reverse();
        actual.ref_schema = None;
        assert!(!fk_equal(&actual, &desired, "local"));
    }

    #[test]
    fn pg_existing_non_primary_serial_and_identity_compile_without_rebuilding_defaults() {
        let pg = DbKind::of("postgresql");
        for default in [Some("nextval('main.t_id_seq'::regclass)"), None] {
            let mut current = current_schema();
            current.columns[0].primary_key = false;
            current.columns[0].auto_increment = true;
            current.columns[0].data_type = "bigint".into();
            current.columns[0].default = default.map(str::to_string);
            let mut id = col("id", Some("id"), "bigint", false);
            id.auto_increment = true;
            id.default_value = default.map(str::to_string);
            let mut draft = design(vec![id, col("name", Some("name"), "varchar(50)", true)], vec![]);
            draft.comment = Some("旁项修改，保留非主键序列".into());
            let steps = compile(&pg, &current, &draft).unwrap();
            assert!(steps.iter().any(|step| step.sql.starts_with("COMMENT ON TABLE")));
            assert!(!steps.iter().any(|step| ["ALTER COLUMN", "SEQUENCE", "OWNED BY", "IDENTITY", "SET DEFAULT", "DROP DEFAULT"].iter().any(|sql| step.sql.contains(sql))));
        }
    }

    #[test]
    fn pg_existing_auto_default_and_attribute_changes_remain_rejected() {
        let mut current = current_schema();
        current.columns[0].primary_key = false;
        current.columns[0].auto_increment = true;
        current.columns[0].default = Some("nextval('main.original_seq'::regclass)".into());
        let mut id = col("id", Some("id"), "integer", false);
        id.auto_increment = true;
        id.default_value = current.columns[0].default.clone();
        let mut draft = design(vec![id, col("name", Some("name"), "varchar(50)", true)], vec![]);
        assert!(compile(&DbKind::of("postgresql"), &current, &draft).is_ok());
        draft.columns[0].default_value = Some("nextval('main.other_seq'::regclass)".into());
        assert!(compile(&DbKind::of("postgresql"), &current, &draft).is_err());
        draft.columns[0].default_value = current.columns[0].default.clone();
        draft.columns[0].auto_increment = false;
        assert!(compile(&DbKind::of("postgresql"), &current, &draft).is_err());
    }

    #[test]
    fn mysql_existing_unique_key_auto_is_preserved_on_unrelated_comment_change() {
        let mysql = DbKind::of("mysql");
        let mut current = current_schema();
        current.columns[0].primary_key = false;
        current.columns[0].auto_increment = true;
        current.indexes.push(IndexInfo { name: "unique_id".into(), columns: vec!["id".into()], unique: true, primary: false, method: Some("BTREE".into()) });
        let mut id = col("id", Some("id"), "integer", false);
        id.auto_increment = true;
        let mut draft = design(vec![id, col("name", Some("name"), "varchar(50)", true)], vec![]);
        draft.comment = Some("保留 UNIQUE 自增".into());
        let steps = compile(&mysql, &current, &draft).unwrap();
        for step in steps.iter().filter(|step| step.sql.contains("MODIFY COLUMN `id`")) {
            assert!(step.sql.contains("AUTO_INCREMENT"));
        }
        assert!(!steps.iter().any(|step| step.sql.contains("PRIMARY KEY")));
        draft.indexes = Some(vec![IndexDesign { name: "unique_id".into(), columns: vec!["id".into()], unique: true , ..Default::default() }]);
        assert!(compile(&mysql, &current, &draft).is_ok());
        draft.indexes = Some(vec![]);
        assert!(compile(&mysql, &current, &draft).is_err());
    }

    #[test]
    fn mysql_new_auto_unique_key_is_created_before_setting_auto() {
        let mysql = DbKind::of("mysql");
        let mut current = current_schema();
        current.columns[0].primary_key = false;
        let mut id = col("id", Some("id"), "integer", false);
        id.auto_increment = true;
        let mut draft = design(vec![id, col("name", Some("name"), "varchar(50)", true)], vec![]);
        draft.indexes = Some(vec![IndexDesign { name: "unique_id".into(), columns: vec!["id".into()], unique: true , ..Default::default() }]);
        let steps = compile(&mysql, &current, &draft).unwrap();
        let key = steps.iter().position(|step| step.sql.contains(" ADD UNIQUE INDEX ")).unwrap();
        let auto = steps.iter().position(|step| step.sql.contains("AUTO_INCREMENT")).unwrap();
        assert!(key < auto);
        draft.indexes = Some(vec![IndexDesign { name: "wrong_first".into(), columns: vec!["name".into(), "id".into()], unique: true , ..Default::default() }]);
        assert!(compile(&mysql, &current, &draft).is_err());
        draft.indexes = Some(vec![IndexDesign { name: "unique_id".into(), columns: vec!["id".into()], unique: true , ..Default::default() }]);
        for (data_type, nullable, default) in [("text", false, None), ("integer", true, None), ("integer", false, Some("1"))] {
            draft.columns[0].data_type = data_type.into();
            draft.columns[0].nullable = nullable;
            draft.columns[0].default_value = default.map(str::to_string);
            assert!(compile(&mysql, &current, &draft).is_err());
        }
    }

    #[test]
    fn sqlite_auto_still_requires_original_primary_key_and_unchanged_attribute() {
        let mut current = current_schema();
        current.columns[0].auto_increment = true;
        let mut id = col("id", Some("id"), "integer", false);
        id.auto_increment = true;
        let mut draft = design(vec![id, col("name", Some("name"), "varchar(50)", true)], vec!["id".into()]);
        assert!(compile(&sqlite(), &current, &draft).is_ok());
        draft.primary_key.clear();
        assert!(compile(&sqlite(), &current, &draft).is_err());
        draft.primary_key.push("id".into());
        draft.columns[0].auto_increment = false;
        assert!(compile(&sqlite(), &current, &draft).is_err());
        draft.columns[0].auto_increment = true;
        draft.columns[0].default_value = Some("1".into());
        assert!(compile(&sqlite(), &current, &draft).is_err());
        draft.columns[0].default_value = None;
        draft.columns.push({let mut added = col("extra", None, "integer", false); added.auto_increment = true; added});
        assert!(compile(&sqlite(), &current, &draft).is_err());
    }

    #[test]
    fn default_literal_case_spaces_and_timestamp_text_are_not_normalized_away() {
        assert_ne!(normalize_default(Some("'A B'")), normalize_default(Some("'ab'")));
        assert_ne!(normalize_default(Some("'CURRENT_TIMESTAMP'")), normalize_default(Some("CURRENT_TIMESTAMP")));
        assert_ne!(normalize_default(Some("CURRENT_TIMESTAMP(3)")), normalize_default(Some("CURRENT_TIMESTAMP")));
        assert_eq!(normalize_default(Some(" CURRENT_TIMESTAMP ")), normalize_default(Some("current_timestamp")));
        assert_eq!(normalize_default(Some(" '' ")), Some("''".into()));
    }
}
