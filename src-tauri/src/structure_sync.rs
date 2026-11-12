//! 结构差异同步（对应 plan/10）：比较源/目标真实结构，预览可执行的 ALTER，
//! 复用表结构设计器 [`crate::design`] 的方言编译与读回校验。
//!
//! 约束：
//! - 预览只读，不写数据库；
//! - 目标额外列/索引默认保留，只有显式 `allow_drop` 才生成删除；
//! - 目标表不存在或方言不支持原地修改时返回 `alterable=false` 与原因，由调用方
//!   改用建表/重建路径，绝不生成假成功；
//! - 应用前用目标表结构 revision 做 CAS，避免覆盖期间的外部变更。

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

use crate::db::Db;
use crate::design::{self, ColumnDesign, IndexDesign, TableDesign};
use crate::models::{ColumnInfo, DbKind, IndexInfo, TableSchema};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureSyncRequest {
    pub source_connection: String,
    pub source_schema: String,
    pub source_table: String,
    pub target_connection: String,
    pub target_schema: String,
    pub target_table: String,
    /// (source_column, target_column)；为空表示同名映射全部源列。
    #[serde(default)]
    pub columns: Vec<(String, String)>,
    /// 是否允许删除目标额外列/索引（默认保留）。
    #[serde(default)]
    pub allow_drop: bool,
    /// 预览时返回的目标 revision；应用时用于 CAS。
    #[serde(default)]
    pub target_revision: String,
    /// 打开预览时的源结构 CAS；应用不能用 fresh 读回覆盖它。
    #[serde(default)]
    pub source_revision: String,
    /// 后端把真实 window/page、双端 FP、完整最终设计与 SQL 绑定后的 review 摘要。
    #[serde(default)]
    pub review_digest: String,
}

/// 只能由 IPC 服务用真实窗口、页面 owner 和 saved/runtime endpoint 建立；不能信任客户端同名 DTO。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureReviewBinding {
    pub principal: String,
    pub owner: String,
    pub source_connection_id: String,
    pub source_fingerprint: String,
    pub source_schema: String,
    pub source_table: String,
    pub target_connection_id: String,
    pub target_fingerprint: String,
    pub target_schema: String,
    pub target_table: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StructureChange {
    /// add_column | alter_column | drop_column | primary_key | index | table_option
    pub kind: String,
    pub object: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// safe | scan_required | destructive
    pub risk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructureDiffReport {
    pub target_exists: bool,
    pub changes: Vec<StructureChange>,
    pub warnings: Vec<String>,
    /// 是否可以原地 ALTER（否则需建表/重建）。
    pub alterable: bool,
    pub unsupported: Vec<String>,
    pub source_revision: String,
    pub target_revision: String,
    pub summary: String,
    /// 与本次预览 SQL 对应的唯一完整最终设计；目标额外项已经合并。
    pub final_design: Option<TableDesign>,
    pub steps: Vec<design::DesignStep>,
    #[serde(default)]
    pub review_digest: String,
    #[serde(default)]
    pub review_binding: Option<StructureReviewBinding>,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructureSyncReceipt {
    pub report: StructureDiffReport,
    pub receipt: design::DesignReceipt,
}

fn mapped(mapping: &[(String, String)], source: &str) -> String {
    mapping
        .iter()
        .find(|(s, _)| s == source)
        .map(|(_, t)| t.clone())
        .unwrap_or_else(|| source.to_string())
}

/// 显式映射只是覆盖项；补齐所有隐式同名映射后，再校验完整的一对一。
pub fn effective_mapping(source: &TableSchema, mapping: &[(String, String)]) -> Result<Vec<(String, String)>> {
    let mut source_names = std::collections::HashSet::new();
    for column in &source.columns {
        if column.name.is_empty() || column.name.contains('\0') || !source_names.insert(column.name.as_str()) {
            return Err(anyhow!("STRUCTURE_SOURCE_COLUMNS_UNKNOWN: 源列身份为空/重复/非法"));
        }
    }
    let mut explicit = std::collections::HashMap::new();
    for (from, to) in mapping {
        if !source_names.contains(from.as_str()) {
            return Err(anyhow!("STRUCTURE_MAPPING_SOURCE_MISSING: 源列不存在: {from}"));
        }
        if to.is_empty() || to.contains('\0') || explicit.insert(from.as_str(), to.as_str()).is_some() {
            return Err(anyhow!("STRUCTURE_MAPPING_INVALID: 重复源映射或目标列为空/非法"));
        }
    }
    let mut targets = std::collections::HashSet::new();
    let mut complete = Vec::new();
    for column in &source.columns {
        let to = explicit.get(column.name.as_str()).copied().unwrap_or(column.name.as_str());
        if !targets.insert(to) {
            return Err(anyhow!("STRUCTURE_MAPPING_COLLISION: 显式与隐式映射合并后多个源列指向 {to}"));
        }
        complete.push((column.name.clone(), to.to_string()));
    }
    Ok(complete)
}

fn col_signature(c: &ColumnInfo) -> String {
    serde_json::to_string(&(&c.data_type, c.nullable, &c.default, c.auto_increment, c.length, c.precision, c.scale, &c.comment)).expect("column signature serialization")
}

fn index_signature(i: &IndexInfo) -> String {
    // JSON tuple 保留顺序和列身份；["a,b"] 与 ["a","b"] 不会碰撞。
    serde_json::to_string(&(i.unique, &i.columns, i.method.as_deref().unwrap_or("btree").to_ascii_lowercase())).expect("index signature serialization")
}

/// 纯差异计算，便于单元测试。返回 (changes, warnings)。
pub fn diff(
    source: &TableSchema,
    target: Option<&TableSchema>,
    mapping: &[(String, String)],
    allow_drop: bool,
) -> (Vec<StructureChange>, Vec<String>) {
    let mut changes = Vec::new();
    let mut warnings = Vec::new();
    let Some(target) = target else {
        return (changes, warnings);
    };
    let expanded = match effective_mapping(source, mapping) {
        Ok(value) => value,
        Err(error) => return (vec![StructureChange { kind: "unsupported".into(), object: target.table.clone(), before: None, after: Some(error.to_string()), risk: "unsupported".into() }], vec![error.to_string()]),
    };
    let mapping = expanded.as_slice();
    if let Err(error) = final_design(source, target, mapping, allow_drop) {
        return (vec![StructureChange { kind: "unsupported".into(), object: target.table.clone(), before: None, after: Some(error.to_string()), risk: "unsupported".into() }], vec![error.to_string()]);
    }
    let target_names: Vec<String> = mapping.iter().map(|(_, t)| t.clone()).collect();

    for c in &source.columns {
        let name = mapped(mapping, &c.name);
        match target.columns.iter().find(|t| t.name == name) {
            None => {
                let risk = if !c.nullable && c.default.is_none() { "scan_required" } else { "safe" };
                changes.push(StructureChange {
                    kind: "add_column".into(),
                    object: name,
                    before: None,
                    after: Some(format!("{} {}", c.data_type, if c.nullable { "NULL" } else { "NOT NULL" })),
                    risk: risk.into(),
                });
            }
            Some(t) => {
                if col_signature(t) != col_signature(c) {
                    changes.push(StructureChange {
                        kind: "alter_column".into(),
                        object: name,
                        before: Some(col_signature(t)),
                        after: Some(col_signature(c)),
                        risk: if c.nullable && t.nullable { "safe" } else { "scan_required" }.into(),
                    });
                }
            }
        }
    }

    for t in &target.columns {
        if target_names.contains(&t.name) || source.columns.iter().any(|c| mapped(mapping, &c.name) == t.name) {
            continue;
        }
        if allow_drop {
            changes.push(StructureChange {
                kind: "drop_column".into(),
                object: t.name.clone(),
                before: Some(t.data_type.clone()),
                after: None,
                risk: "destructive".into(),
            });
        } else {
            warnings.push(format!("目标额外列 {} 已保留（未启用删除）", t.name));
        }
    }

    let source_pk = match design::ordered_primary_key(source) {
        Ok(columns) => columns.iter().map(|name| mapped(mapping, name)).collect::<Vec<_>>(),
        Err(error) => return (vec![StructureChange { kind: "unsupported".into(), object: target.table.clone(), before: None, after: Some(error.to_string()), risk: "unsupported".into() }], vec![error.to_string()]),
    };
    let target_pk = match design::ordered_primary_key(target) {
        Ok(columns) => columns,
        Err(error) => return (vec![StructureChange { kind: "unsupported".into(), object: target.table.clone(), before: None, after: Some(error.to_string()), risk: "unsupported".into() }], vec![error.to_string()]),
    };
    if source_pk != target_pk {
        changes.push(StructureChange {
            kind: "primary_key".into(),
            object: target.table.clone(),
            before: Some(serde_json::to_string(&target_pk).expect("PK serialization")),
            after: Some(serde_json::to_string(&source_pk).expect("PK serialization")),
            risk: if target_pk.is_empty() { "scan_required" } else { "destructive" }.into(),
        });
    }

    let target_indexes: Vec<String> = target.indexes.iter().filter(|i| !i.primary).map(index_signature).collect();
    for idx in source.indexes.iter().filter(|i| !i.primary) {
        let cols: Vec<String> = idx.columns.iter().map(|n| mapped(mapping, n)).collect();
        let mut mapped_index = idx.clone();
        mapped_index.columns = cols.clone();
        let signature = index_signature(&mapped_index);
        if !target_indexes.contains(&signature) {
            changes.push(StructureChange {
                kind: "index".into(),
                object: idx.name.clone(),
                before: None,
                after: Some(signature),
                risk: if cols.is_empty() { "unsupported" } else if idx.unique { "scan_required" } else { "safe" }.into(),
            });
        }
    }
    if !source.foreign_keys.is_empty() {
        warnings.push("跨库外键不会被自动复制；如需外键请在目标单独创建".into());
    }
    (changes, warnings)
}

fn column_design(column: &ColumnInfo, name: String, existing: bool) -> ColumnDesign {
    // 元数据可能已把 (length,scale) 放入完整类型文本，不能再生成 varchar(50)(50)。
    let base = column.data_type.trim().to_ascii_lowercase();
    let numeric = matches!(base.as_str(), "numeric" | "decimal" | "number");
    let has_modifier = base.contains('(');
    let length = if has_modifier { None } else if numeric { column.precision.or(column.length) } else { column.length };
    let scale = if numeric && !has_modifier { column.scale } else { None };
    ColumnDesign { orig_name: existing.then(|| name.clone()), name, data_type: column.data_type.clone(), length: length.map(|n| n.to_string()), scale: scale.map(|n| n.to_string()), nullable: column.nullable, default_value: column.default.clone(), auto_increment: column.auto_increment, comment: column.comment.clone(), unsigned: false, zerofill: false }
}

/// 预览和应用唯一共用的最终设计。按目标原物理顺序保留旧列，并追加真正新增列。
/// 同名异义索引不偷偷 DROP+CREATE；外键默认完全保留，不跨 schema 猜身份。
pub fn final_design(source: &TableSchema, target: &TableSchema, mapping: &[(String, String)], allow_drop: bool) -> Result<TableDesign> {
    let expanded = effective_mapping(source, mapping)?;
    let mut target_names = std::collections::HashSet::new();
    for column in &target.columns {
        if column.name.is_empty() || !target_names.insert(column.name.as_str()) {
            return Err(anyhow!("STRUCTURE_TARGET_COLUMNS_UNKNOWN: 目标列身份为空或重复"));
        }
    }
    let mut columns = Vec::new();
    for current in &target.columns {
        if let Some((from, _)) = expanded.iter().find(|(_, to)| to == &current.name) {
            let desired = source.columns.iter().find(|c| &c.name == from).expect("validated source mapping");
            columns.push(column_design(desired, current.name.clone(), true));
        } else if !allow_drop {
            columns.push(column_design(current, current.name.clone(), true));
        }
    }
    for desired in &source.columns {
        let name = mapped(&expanded, &desired.name);
        if !target_names.contains(name.as_str()) { columns.push(column_design(desired, name, false)); }
    }
    let primary_key = design::ordered_primary_key(source)?.iter().map(|name| mapped(&expanded, name)).collect();
    let _ = design::ordered_primary_key(target)?;
    if allow_drop && target.foreign_keys.iter().any(|fk| fk.columns.iter().any(|name| !columns.iter().any(|column| &column.name == name))) {
        return Err(anyhow!("STRUCTURE_FOREIGN_KEY_DROP_UNSUPPORTED: 删除列触及现有外键；没有完整引用 schema/批准，不自动 DROP/CASCADE"));
    }
    let current_indexes: Vec<&IndexInfo> = target.indexes.iter().filter(|i| !i.primary).collect();
    let mut names = std::collections::HashSet::new();
    for index in &current_indexes {
        if index.name.is_empty() || !names.insert(index.name.as_str()) || index.columns.is_empty() {
            return Err(anyhow!("STRUCTURE_INDEX_METADATA_UNKNOWN: 目标索引身份重复/为空或列信息不完整"));
        }
    }
    let mut indexes: Vec<IndexDesign> = if allow_drop { Vec::new() } else {
        current_indexes.iter().map(|i| IndexDesign { name: i.name.clone(), columns: i.columns.clone(), unique: i.unique, method: i.method.clone(), descending: Vec::new() }).collect()
    };
    let mut source_names = std::collections::HashSet::new();
    for index in source.indexes.iter().filter(|i| !i.primary) {
        if index.name.is_empty() || !source_names.insert(index.name.as_str()) || index.columns.is_empty() || index.columns.iter().any(|name| !source.columns.iter().any(|c| &c.name == name)) {
            return Err(anyhow!("STRUCTURE_INDEX_METADATA_UNKNOWN: 源索引名字/有序列身份缺失或重复"));
        }
        let mut desired = index.clone();
        desired.columns = index.columns.iter().map(|name| mapped(&expanded, name)).collect();
        if let Some(existing) = current_indexes.iter().find(|i| i.name == desired.name) {
            if index_signature(existing) != index_signature(&desired) {
                return Err(anyhow!("STRUCTURE_INDEX_NAME_CONFLICT: 同名索引 {} 的列顺序/唯一性/方法不同；需要独立替换预览，禁止自动 DROP", index.name));
            }
        }
        if let Some(existing) = current_indexes.iter().find(|i| index_signature(i) == index_signature(&desired)) {
            if !indexes.iter().any(|i| i.name == existing.name) { indexes.push(IndexDesign { name: existing.name.clone(), columns: existing.columns.clone(), unique: existing.unique, method: existing.method.clone(), descending: Vec::new() }); }
            continue;
        }
        if desired.method.as_deref().is_some_and(|method| !method.eq_ignore_ascii_case("btree")) {
            return Err(anyhow!("STRUCTURE_INDEX_METHOD_UNSUPPORTED: 索引方法 {} 尚无无损方言编译", desired.method.as_deref().unwrap_or("")));
        }
        indexes.push(IndexDesign { name: desired.name, columns: desired.columns, unique: desired.unique, method: desired.method, descending: Vec::new() });
    }
    Ok(TableDesign { schema: target.database.clone(), table: target.table.clone(), columns, primary_key, comment: source.comment.clone(), indexes: Some(indexes), foreign_keys: None, checks: None, triggers: None, options: None, base_revision: None, source_fingerprint: None })
}

fn is_revision(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_scope(request: &StructureSyncRequest, binding: &StructureReviewBinding) -> Result<()> {
    if binding.principal.is_empty() || binding.owner.is_empty() || !is_revision(&binding.source_fingerprint) || !is_revision(&binding.target_fingerprint) {
        return Err(anyhow!("STRUCTURE_BINDING_REQUIRED: 缺少真实窗口/页面/双端 endpoint fingerprint"));
    }
    if request.source_connection != binding.source_connection_id || request.target_connection != binding.target_connection_id || request.source_schema != binding.source_schema || request.target_schema != binding.target_schema || request.source_table != binding.source_table || request.target_table != binding.target_table {
        return Err(anyhow!("STRUCTURE_BINDING_CHANGED: 源/目标连接或物理对象身份与 review 不一致"));
    }
    if binding.source_fingerprint == binding.target_fingerprint && binding.source_schema == binding.target_schema && binding.source_table == binding.target_table {
        return Err(anyhow!("STRUCTURE_SAME_PHYSICAL_TABLE: 两个连接别名仍指向同一物理表，已拒绝"));
    }
    Ok(())
}

/// 供父 C1 服务在取 pool 之前使用：拒绝 legacy/null token、改端点/对象与缺失双 CAS。
pub fn validate_review_request(request: &StructureSyncRequest, binding: &StructureReviewBinding) -> Result<()> {
    validate_scope(request, binding)?;
    if !is_revision(&request.source_revision) || !is_revision(&request.target_revision) || !is_revision(&request.review_digest) {
        return Err(anyhow!("STRUCTURE_REVIEW_REQUIRED: 缺少打开预览时双端 revision/review 摘要；没有发送 SQL"));
    }
    Ok(())
}

fn review_digest(request: &StructureSyncRequest, report: &StructureDiffReport, binding: &StructureReviewBinding) -> Result<String> {
    validate_scope(request, binding)?;
    if !report.target_exists || !report.alterable || !report.unsupported.is_empty() || !is_revision(&report.source_revision) || !is_revision(&report.target_revision) || report.final_design.is_none() {
        return Err(anyhow!("STRUCTURE_REVIEW_NOT_EXECUTABLE: 目标/完整 metadata/最终设计没有形成可执行预览"));
    }
    if report.final_design.as_ref().is_some_and(|design| design.schema != request.target_schema || design.table != request.target_table) {
        return Err(anyhow!("STRUCTURE_PLAN_TARGET_CHANGED: 最终设计的目标不等于 review 物理对象"));
    }
    // 不含可变 token 自身；保留显式映射所有字节以及有序键/SQL/default 字面量。
    Ok(crate::object_catalog::revision(&(binding, &request.columns, request.allow_drop, &report.source_revision, &report.target_revision, &report.final_design, &report.steps, &report.changes, &report.warnings)))
}

/// 预览读取前后 actual Binding CAS 由父服务负责；此函数不查询/授权/发送 SQL。
pub fn bind_review(request: &StructureSyncRequest, report: &mut StructureDiffReport, binding: &StructureReviewBinding) -> Result<()> {
    let digest = review_digest(request, report, binding)?;
    if (!request.source_revision.is_empty() && request.source_revision != report.source_revision) || (!request.target_revision.is_empty() && request.target_revision != report.target_revision) {
        return Err(anyhow!("STRUCTURE_REVIEW_STALE: 预览输入保留了旧 CAS，必须显式重新读取而非覆盖"));
    }
    report.review_digest = digest;
    report.review_binding = Some(binding.clone());
    Ok(())
}

/// apply 时必须给刚读取并重编的 report，而不是客户端 SQL。改 mapping/端点/owner/源或目标/最终设计均使旧 review 作废。
pub fn validate_review(request: &StructureSyncRequest, report: &StructureDiffReport, binding: &StructureReviewBinding) -> Result<()> {
    validate_review_request(request, binding)?;
    if request.source_revision != report.source_revision || request.target_revision != report.target_revision {
        return Err(anyhow!("STRUCTURE_REVIEW_STALE: 源或目标目录已经变化，不覆盖预览时基线"));
    }
    if report.review_binding.as_ref().is_some_and(|saved| saved != binding) || (!report.review_digest.is_empty() && report.review_digest != request.review_digest) || review_digest(request, report, binding)? != request.review_digest {
        return Err(anyhow!("STRUCTURE_REVIEW_CHANGED: window/page/双端 endpoint/映射/破坏性选项/最终设计或 SQL 与 review 不一致"));
    }
    Ok(())
}

/// 后端 C1 request marker；不是可运行 SQL，签发和执行都必须逐字匹配。
pub fn review_request_sql(request: &StructureSyncRequest) -> Result<String> {
    if !is_revision(&request.review_digest) || !is_revision(&request.source_revision) || !is_revision(&request.target_revision) {
        return Err(anyhow!("STRUCTURE_REVIEW_REQUIRED: marker 缺少双端 revision/review"));
    }
    Ok(format!("STRUCTURE_SYNC_REVIEW {}", crate::object_catalog::revision(request)))
}

/// 父受控 worker 给同 owned 连接读取的 current；只能返回重新编译的 SQL，绝不返回客户端 steps。
pub fn prepare_reviewed_design(request: &StructureSyncRequest, report: &StructureDiffReport, binding: &StructureReviewBinding, current: &TableSchema, target_kind: &DbKind) -> Result<(TableDesign, Vec<design::DesignStep>)> {
    validate_review(request, report, binding)?;
    let final_design = report.final_design.clone().ok_or_else(|| anyhow!("STRUCTURE_PLAN_MISSING"))?;
    let steps = if request.allow_drop { design::compile(target_kind, current, &final_design)? } else { design::compile_non_destructive(target_kind, current, &final_design)? };
    if steps != report.steps { return Err(anyhow!("STRUCTURE_RECOMPILE_CHANGED: 同连接当前目录重编步骤不同，不能执行旧 SQL")); }
    Ok((final_design, steps))
}

/// 供父 owned worker 与预览使用同一 CAS 算法；未知 CHECK/trigger 元数据不得绑定为已知空目录。
pub fn revision_for_schema(metadata: &TableSchema, objects: &crate::table_objects::ConstraintSet, schema: &str, table: &str) -> Result<String> {
    if metadata.columns.is_empty() || metadata.table != table || metadata.database != schema { return Err(anyhow!("STRUCTURE_METADATA_UNKNOWN: 表不存在、列不可见或实际目录身份错误")); }
    if objects.completeness != "complete" { return Err(anyhow!("STRUCTURE_OBJECT_METADATA_UNKNOWN: {}", objects.limitations.join("；"))); }
    let _ = design::ordered_primary_key(metadata)?;
    let mut stable = metadata.clone();
    stable.row_count = None;
    Ok(crate::object_catalog::revision(&(schema, table, stable, objects)))
}

/// 结构 revision 不含行数；包含真实 CHECK/触发器，不能把它们从 CAS 证据漏掉。
pub async fn read_revision(db: &Db, kind: &DbKind, schema: &str, table: &str) -> Result<String> {
    let __sql_history_context = crate::sql_history::db_context(db);
    crate::sql_history::scope(__sql_history_context, async {
    let metadata = crate::schema::table_schema(db, kind.clone(), schema, table).await?;
    let objects = crate::table_objects::list(db, kind, schema, table).await?;
    revision_for_schema(&metadata, &objects, schema, table)

    }).await
}

/// 物理端点身份必须由父从实际 adapter 取得/规范化，不能用含连接 id 的 saved-config hash 代替。
/// parent 在 grant/pool 之前调用；两个 alias 名称不同仍必须拒同一个表。
pub fn validate_distinct_physical_tables(source_endpoint_identity: &str, source_schema: &str, source_table: &str, target_endpoint_identity: &str, target_schema: &str, target_table: &str) -> Result<()> {
    if source_endpoint_identity.is_empty() || target_endpoint_identity.is_empty() { return Err(anyhow!("STRUCTURE_PHYSICAL_IDENTITY_REQUIRED: 无法证明实际端点，不能把连接别名当物理身份")); }
    if source_endpoint_identity == target_endpoint_identity && source_schema == target_schema && source_table == target_table { return Err(anyhow!("STRUCTURE_SAME_PHYSICAL_TABLE: 连接别名指向同一物理表")); }
    Ok(())
}

/// 父 owned worker 可将同连接真实元数据/双端 revision 输入这个纯编译器，不必在持事务时再借 pool。
/// revision 必须由 revision_for_schema(真实 TableSchema + 完整 CHECK/trigger) 生成，不接受客户端 revision 冒充新读回。
pub fn preview_from_schemas(source: &TableSchema, target: Option<&TableSchema>, target_kind: &DbKind, request: &StructureSyncRequest, source_revision: &str, target_revision: &str) -> Result<StructureDiffReport> {
    if source.table != request.source_table || source.database != request.source_schema || source.columns.is_empty() || !is_revision(source_revision) {
        return Err(anyhow!("STRUCTURE_SOURCE_METADATA_CHANGED: 源真实目录/结构 revision 不完整或身份不同"));
    }
    if target.is_some_and(|current| current.table != request.target_table || current.database != request.target_schema || current.columns.is_empty()) || (target.is_some() && !is_revision(target_revision)) {
        return Err(anyhow!("STRUCTURE_TARGET_METADATA_CHANGED: 目标真实目录/结构 revision 不完整或身份不同"));
    }
    let mapping = effective_mapping(source, &request.columns)?;
    let (changes, mut warnings) = diff(source, target, &mapping, request.allow_drop);
    warnings.push("CHECK/触发器仅作为完整 CAS 证据保留，不自动复制源定义；跨库外键也不自动复制".into());
    let mut unsupported = Vec::new();
    let mut final_design_value = None;
    let mut steps = Vec::new();
    if let Some(current) = target {
        match final_design(source, current, &mapping, request.allow_drop) {
            Err(error) => unsupported.push(format!("{error:#}")),
            Ok(mut design) => {
                design.schema = request.target_schema.clone(); design.table = request.target_table.clone();
                let compiled = if request.allow_drop { design::compile(target_kind, current, &design) } else { design::compile_non_destructive(target_kind, current, &design) };
                match compiled { Ok(value) => steps = value, Err(error) => unsupported.push(format!("{error:#}")) }
                final_design_value = Some(design);
            }
        }
    } else { unsupported.push("目标表不存在：请使用建表/重建同步流程，而非原地 ALTER".into()); }
    let summary = if target.is_none() { "目标表不存在".into() } else if !unsupported.is_empty() { "结构差异需要人工处理；未生成可应用计划".into() } else if changes.is_empty() { "结构一致，无需修改".into() } else { format!("{} 项结构差异", changes.len()) };
    Ok(StructureDiffReport { target_exists: target.is_some(), changes, warnings, alterable: target.is_some() && unsupported.is_empty(), unsupported, source_revision: source_revision.to_string(), target_revision: target_revision.to_string(), summary, final_design: final_design_value, steps, review_digest: String::new(), review_binding: None })
}

pub async fn preview(
    source: &Db,
    source_kind: &DbKind,
    target: &Db,
    target_kind: &DbKind,
    request: &StructureSyncRequest,
) -> Result<StructureDiffReport> {
    let source_before = read_revision(source, source_kind, &request.source_schema, &request.source_table).await?;
    let exists = crate::schema::list_tables(target, target_kind.clone(), &request.target_schema).await?.iter().any(|(name, _)| name == &request.target_table);
    let target_before = if exists { read_revision(target, target_kind, &request.target_schema, &request.target_table).await? } else { String::new() };
    let source_schema = crate::schema::table_schema(source, source_kind.clone(), &request.source_schema, &request.source_table).await?;
    let target_schema = if exists { Some(crate::schema::table_schema(target, target_kind.clone(), &request.target_schema, &request.target_table).await?) } else { None };
    let mut report = preview_from_schemas(&source_schema, target_schema.as_ref(), target_kind, request, &source_before, &target_before)?;
    let source_revision = read_revision(source, source_kind, &request.source_schema, &request.source_table).await?;
    let target_revision = if exists { read_revision(target, target_kind, &request.target_schema, &request.target_table).await? } else { String::new() };
    if source_before != source_revision || target_before != target_revision {
        return Err(anyhow!("STRUCTURE_PREVIEW_STALE: 读取期间源或目标目录变化，不能发布混合快照/执行 SQL"));
    }
    report.source_revision = source_revision;
    report.target_revision = target_revision;
    Ok(report)
}

/// 兼容入口：必须保留双端打开时 revision；破坏性应用留给绑定 C1 的专门入口。
/// 父服务还必须用 validate_review 校验真实 window/page/双端 FP/物理身份。
pub async fn apply(
    _source: &Db,
    _source_kind: &DbKind,
    _target: &Db,
    _target_kind: &DbKind,
    _request: &StructureSyncRequest,
) -> Result<StructureSyncReceipt> {
    // 旧 IPC 没有 actual window/page/endpoint Binding，也没有 C1 Entry；不能把带 digest 的客户端请求当授权。
    Err(anyhow!("STRUCTURE_CONTROLLED_EXECUTION_REQUIRED: 请使用父服务 C1 review/重编/同连接事务入口；兼容入口未查询或发送 DDL"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, ty: &str, nullable: bool, pk: bool) -> ColumnInfo {
        ColumnInfo { name: name.into(), data_type: ty.into(), nullable, default: None, primary_key: pk, auto_increment: false, comment: None, length: None, precision: None, scale: None }
    }

    fn table(name: &str, columns: Vec<ColumnInfo>, indexes: Vec<IndexInfo>) -> TableSchema {
        TableSchema { table: name.into(), database: "db".into(), columns, indexes, foreign_keys: Vec::new(), comment: None, engine: None, collation: None, row_count: None }
    }

    #[test]
    fn equal_structure_has_no_changes() {
        let a = table("t", vec![col("id", "integer", false, true), col("name", "text", true, false)], vec![]);
        let (changes, _) = diff(&a, Some(&a), &[], false);
        assert!(changes.is_empty());
    }

    #[test]
    fn detects_added_and_altered_columns() {
        let source = table("t", vec![col("id", "integer", false, true), col("name", "text", true, false), col("qty", "integer", true, false)], vec![]);
        let target = table("t", vec![col("id", "integer", false, true), col("name", "varchar", true, false)], vec![]);
        let (changes, _) = diff(&source, Some(&target), &[], false);
        assert!(changes.iter().any(|c| c.kind == "add_column" && c.object == "qty"));
        assert!(changes.iter().any(|c| c.kind == "alter_column" && c.object == "name"));
    }

    #[test]
    fn extra_target_column_preserved_by_default() {
        let source = table("t", vec![col("id", "integer", false, true)], vec![]);
        let target = table("t", vec![col("id", "integer", false, true), col("legacy", "text", true, false)], vec![]);
        let (changes, warnings) = diff(&source, Some(&target), &[], false);
        assert!(changes.is_empty());
        assert!(warnings.iter().any(|w| w.contains("legacy")));
        let (changes, _) = diff(&source, Some(&target), &[], true);
        assert!(changes.iter().any(|c| c.kind == "drop_column" && c.risk == "destructive"));
    }

    #[test]
    fn detects_primary_key_change() {
        let source = table("t", vec![col("id", "integer", false, true)], vec![]);
        let target = table("t", vec![col("id", "integer", false, false)], vec![]);
        let (changes, _) = diff(&source, Some(&target), &[], false);
        assert!(changes.iter().any(|c| c.kind == "primary_key"));
    }

    #[test]
    fn detects_missing_index() {
        let source = table("t", vec![col("id", "integer", false, true)], vec![IndexInfo { name: "idx_id".into(), columns: vec!["id".into()], unique: false, primary: false, method: None }]);
        let target = table("t", vec![col("id", "integer", false, true)], vec![]);
        let (changes, _) = diff(&source, Some(&target), &[], false);
        assert!(changes.iter().any(|c| c.kind == "index"));
    }

    #[test]
    fn mapping_renames_target_column() {
        let source = table("t", vec![col("id", "integer", false, true), col("title", "text", true, false)], vec![]);
        let target = table("t", vec![col("id", "integer", false, true), col("name", "text", true, false)], vec![]);
        let mapping = vec![("id".to_string(), "id".to_string()), ("title".to_string(), "name".to_string())];
        let (changes, _) = diff(&source, Some(&target), &mapping, false);
        assert!(changes.is_empty(), "mapped rename should be equal: {changes:?}");
    }

    fn index(name: &str, columns: &[&str], primary: bool) -> IndexInfo {
        IndexInfo { name: name.into(), columns: columns.iter().map(|value| value.to_string()).collect(), unique: primary, primary, method: None }
    }

    fn request() -> StructureSyncRequest {
        StructureSyncRequest { source_connection: "source".into(), source_schema: "db".into(), source_table: "t".into(), target_connection: "target".into(), target_schema: "db".into(), target_table: "t".into(), columns: Vec::new(), allow_drop: false, target_revision: String::new(), source_revision: String::new(), review_digest: String::new() }
    }

    fn binding() -> StructureReviewBinding {
        StructureReviewBinding { principal: "window-real".into(), owner: "page-real".into(), source_connection_id: "source".into(), source_fingerprint: "a".repeat(64), source_schema: "db".into(), source_table: "t".into(), target_connection_id: "target".into(), target_fingerprint: "b".repeat(64), target_schema: "db".into(), target_table: "t".into() }
    }

    fn reviewed() -> (StructureSyncRequest, StructureDiffReport, StructureReviewBinding, TableSchema) {
        let target = table("t", vec![col("id", "integer", false, true)], Vec::new());
        let source = table("t", vec![col("id", "integer", false, true), col("note", "text", true, false)], Vec::new());
        let design = final_design(&source, &target, &[], false).unwrap();
        let steps = design::compile_non_destructive(&DbKind::of("sqlite"), &target, &design).unwrap();
        let (changes, warnings) = diff(&source, Some(&target), &[], false);
        let mut report = StructureDiffReport { target_exists: true, changes, warnings, alterable: true, unsupported: Vec::new(), source_revision: "c".repeat(64), target_revision: "d".repeat(64), summary: "fixture".into(), final_design: Some(design), steps, review_digest: String::new(), review_binding: None };
        let mut request = request();
        let binding = binding();
        bind_review(&request, &mut report, &binding).unwrap();
        request.source_revision = report.source_revision.clone();
        request.target_revision = report.target_revision.clone();
        request.review_digest = report.review_digest.clone();
        (request, report, binding, target)
    }

    #[test]
    fn partial_mapping_rejects_collision_with_implicit_same_name() {
        let source = table("t", vec![col("a", "text", true, false), col("b", "text", true, false)], Vec::new());
        assert!(effective_mapping(&source, &[("a".into(), "b".into())]).unwrap_err().to_string().contains("STRUCTURE_MAPPING_COLLISION"));
        assert!(effective_mapping(&source, &[("a".into(), "x".into()), ("a".into(), "y".into())]).is_err());
        assert!(effective_mapping(&source, &[("missing".into(), "x".into())]).is_err());
        assert!(effective_mapping(&source, &[("a".into(), String::new())]).is_err());
        assert_eq!(effective_mapping(&source, &[("a".into(), "x".into())]).unwrap(), vec![("a".into(), "x".into()), ("b".into(), "b".into())]);
    }

    #[test]
    fn index_signature_preserves_order_and_comma_identity() {
        assert_ne!(index_signature(&index("idx", &["a", "b"], false)), index_signature(&index("idx", &["b", "a"], false)));
        assert_ne!(index_signature(&index("idx", &["a,b"], false)), index_signature(&index("idx", &["a", "b"], false)));
        assert_ne!(index_signature(&index("idx", &["A"], false)), index_signature(&index("idx", &["a"], false)));
    }

    #[test]
    fn final_design_preserves_extra_columns_indexes_and_literal_metadata() {
        let source = table("t", vec![col("id", "integer", false, true), col("new", "text", true, false)], Vec::new());
        let mut legacy = col("legacy", "varchar(50)", true, false);
        legacy.length = Some(50);
        legacy.default = Some("'A B'".into());
        let target = table("t", vec![col("id", "integer", false, true), legacy], vec![index("idx_legacy", &["legacy"], false)]);
        let design = final_design(&source, &target, &[], false).unwrap();
        assert_eq!(design.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(), vec!["id", "legacy", "new"]);
        assert_eq!(design.columns[1].orig_name.as_deref(), Some("legacy"));
        assert_eq!(design.columns[1].default_value.as_deref(), Some("'A B'"));
        assert_eq!(design.columns[1].length, None);
        assert_eq!(design.indexes.as_ref().unwrap()[0].name, "idx_legacy");
        let steps = design::compile_non_destructive(&DbKind::of("mysql"), &target, &design).unwrap();
        assert!(steps.iter().all(|step| !step.sql.contains("DROP")));
        assert!(steps.iter().any(|step| step.sql.contains("ADD COLUMN")));
    }

    #[test]
    fn final_design_rejects_same_named_index_with_different_order() {
        let source = table("t", vec![col("a", "text", true, false), col("b", "text", true, false)], vec![index("idx_pair", &["a", "b"], false)]);
        let target = table("t", source.columns.clone(), vec![index("idx_pair", &["b", "a"], false)]);
        let error = final_design(&source, &target, &[], false).unwrap_err();
        assert!(error.to_string().contains("STRUCTURE_INDEX_NAME_CONFLICT"));
        assert!(final_design(&source, &target, &[], true).is_err());
    }

    #[test]
    fn primary_key_diff_uses_ordered_metadata_and_blocks_missing_order() {
        let columns = vec![col("a", "text", false, true), col("b", "text", false, true)];
        let source = table("t", columns.clone(), vec![index("source_pk", &["b", "a"], true)]);
        let target = table("t", columns.clone(), vec![index("target_pk", &["a", "b"], true)]);
        assert!(diff(&source, Some(&target), &[], false).0.iter().any(|change| change.kind == "primary_key" && change.risk == "destructive"));
        assert!(design::compile_non_destructive(&DbKind::of("mysql"), &target, &final_design(&source, &target, &[], false).unwrap()).is_err());
        let unknown = table("t", columns, Vec::new());
        assert!(final_design(&unknown, &target, &[], false).is_err());
        assert!(diff(&unknown, Some(&target), &[], false).0.iter().any(|change| change.kind == "unsupported"));
    }

    #[test]
    fn review_binds_owner_endpoint_mapping_both_revisions_and_final_sql() {
        let (request, report, binding, _) = reviewed();
        validate_review(&request, &report, &binding).unwrap();
        let mut changed_binding = binding.clone();
        changed_binding.owner = "another-page".into();
        assert!(validate_review(&request, &report, &changed_binding).is_err());
        changed_binding = binding.clone();
        changed_binding.source_fingerprint = "f".repeat(64);
        assert!(validate_review(&request, &report, &changed_binding).is_err());
        changed_binding = binding.clone();
        changed_binding.target_fingerprint = "e".repeat(64);
        assert!(validate_review(&request, &report, &changed_binding).is_err());
        let mut changed_request = request.clone();
        changed_request.columns = vec![("note".into(), "other".into())];
        assert!(validate_review(&changed_request, &report, &binding).is_err());
        changed_request = request.clone();
        changed_request.source_revision = "e".repeat(64);
        assert!(validate_review(&changed_request, &report, &binding).is_err());
        changed_request = request.clone();
        changed_request.target_revision = "f".repeat(64);
        assert!(validate_review(&changed_request, &report, &binding).is_err());
        let mut changed_report = report.clone();
        changed_report.steps[0].sql.push_str("; DROP TABLE t");
        assert!(validate_review(&request, &changed_report, &binding).is_err());
        changed_report = report.clone();
        changed_report.final_design.as_mut().unwrap().columns[1].default_value = Some("'A B'".into());
        assert!(validate_review(&request, &changed_report, &binding).is_err());
    }

    #[test]
    fn same_physical_table_aliases_and_legacy_review_are_rejected_before_pool() {
        let (request, report, binding, _) = reviewed();
        let mut alias = binding.clone();
        alias.target_fingerprint = alias.source_fingerprint.clone();
        assert!(validate_review(&request, &report, &alias).unwrap_err().to_string().contains("STRUCTURE_SAME_PHYSICAL_TABLE"));
        let mut legacy = request.clone();
        legacy.review_digest.clear();
        assert!(validate_review_request(&legacy, &binding).is_err());
        let mut missing = binding.clone();
        missing.principal.clear();
        assert!(validate_review_request(&request, &missing).is_err());
    }

    #[test]
    fn review_marker_and_recompile_do_not_accept_client_sql_or_concurrent_extra() {
        let (request, report, binding, mut current) = reviewed();
        let (_, steps) = prepare_reviewed_design(&request, &report, &binding, &current, &DbKind::of("sqlite")).unwrap();
        assert_eq!(steps, report.steps);
        let marker = review_request_sql(&request).unwrap();
        assert!(!marker.contains("ADD COLUMN"));
        let mut changed = request.clone();
        changed.allow_drop = true;
        assert_ne!(review_request_sql(&changed).unwrap(), marker);
        assert!(validate_review(&changed, &report, &binding).is_err());
        current.columns.push(col("concurrent", "text", true, false));
        assert!(prepare_reviewed_design(&request, &report, &binding, &current, &DbKind::of("sqlite")).is_err());
    }

    #[test]
    fn structure_revision_ignores_row_count_but_rejects_unknown_constraints() {
        let mut metadata = table("t", vec![col("id", "integer", false, true)], Vec::new());
        let mut objects = crate::table_objects::ConstraintSet { checks: Vec::new(), triggers: Vec::new(), completeness: "complete".into(), limitations: Vec::new(), table_definition: Some("CREATE TABLE t (id INTEGER PRIMARY KEY)".into()) };
        let before = revision_for_schema(&metadata, &objects, "db", "t").unwrap();
        metadata.row_count = Some(9999);
        assert_eq!(revision_for_schema(&metadata, &objects, "db", "t").unwrap(), before);
        objects.completeness = "unknown".into();
        objects.limitations.push("permission denied".into());
        assert!(revision_for_schema(&metadata, &objects, "db", "t").is_err());
    }

    #[tokio::test]
    async fn sqlite_real_preview_and_final_design_preserve_extra_column_index_and_data() {
        let source_pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        let target_pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY, name TEXT, email TEXT)").execute(&source_pool).await.unwrap();
        crate::sql_history::query("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY, name TEXT, legacy TEXT DEFAULT 'A B')").execute(&target_pool).await.unwrap();
        crate::sql_history::query("CREATE INDEX idx_legacy ON t(legacy)").execute(&target_pool).await.unwrap();
        crate::sql_history::query("INSERT INTO t(id, name) VALUES (1, 'old')").execute(&target_pool).await.unwrap();
        let source = Db::Sqlite(source_pool.clone());
        let target = Db::Sqlite(target_pool.clone());
        let kind = DbKind::of("sqlite");
        let mut request = request();
        request.source_schema = "main".into(); request.target_schema = "main".into();
        let mut binding = binding();
        binding.source_schema = "main".into(); binding.target_schema = "main".into();
        let mut report = preview(&source, &kind, &target, &kind, &request).await.unwrap();
        assert!(report.alterable, "{:?}", report.unsupported);
        assert!(report.steps.iter().all(|step| !step.sql.contains("DROP")));
        bind_review(&request, &mut report, &binding).unwrap();
        request.source_revision = report.source_revision.clone(); request.target_revision = report.target_revision.clone(); request.review_digest = report.review_digest.clone();
        let current = crate::schema::table_schema(&target, kind.clone(), "main", "t").await.unwrap();
        let (final_design, steps) = prepare_reviewed_design(&request, &report, &binding, &current, &kind).unwrap();
        assert_eq!(steps, report.steps);
        assert_eq!(final_design.columns.iter().map(|column| column.name.as_str()).collect::<Vec<_>>(), vec!["id", "name", "legacy", "email"]);
        // 这是原生表设计执行测试，不冒充父 IPC/C1 窗口端到端验收。
        let receipt = design::apply_non_destructive(&target, &kind, &final_design).await.unwrap();
        assert!(receipt.verified, "{:?}", receipt.mismatches);
        let values: (String, String, Option<String>) = crate::sql_history::query_as("SELECT name, legacy, email FROM t WHERE id = 1").fetch_one(&target_pool).await.unwrap();
        assert_eq!(values, ("old".into(), "A B".into(), None));
        let count: i64 = crate::sql_history::query_scalar("SELECT count(*) FROM main.sqlite_schema WHERE type='index' AND name='idx_legacy'").fetch_one(&target_pool).await.unwrap();
        assert_eq!(count, 1);
        source_pool.close().await; target_pool.close().await;
    }

    #[tokio::test]
    async fn sqlite_real_revision_includes_trigger_definition_not_rows() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        crate::sql_history::query("CREATE TABLE t (id INTEGER NOT NULL PRIMARY KEY, note TEXT CHECK (note <> 'A B'))").execute(&pool).await.unwrap();
        let db = Db::Sqlite(pool.clone()); let kind = DbKind::of("sqlite");
        let before = read_revision(&db, &kind, "main", "t").await.unwrap();
        crate::sql_history::query("INSERT INTO t VALUES (1, 'row')").execute(&pool).await.unwrap();
        assert_eq!(read_revision(&db, &kind, "main", "t").await.unwrap(), before);
        crate::sql_history::query("CREATE TRIGGER tr_t AFTER INSERT ON t BEGIN SELECT 'A B'; END").execute(&pool).await.unwrap();
        let first = read_revision(&db, &kind, "main", "t").await.unwrap(); assert_ne!(first, before);
        crate::sql_history::query("DROP TRIGGER tr_t").execute(&pool).await.unwrap();
        crate::sql_history::query("CREATE TRIGGER tr_t AFTER INSERT ON t BEGIN SELECT 'ab'; END").execute(&pool).await.unwrap();
        assert_ne!(read_revision(&db, &kind, "main", "t").await.unwrap(), first);
        pool.close().await;
    }

    #[tokio::test]
    async fn legacy_apply_has_zero_database_access_without_c1() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
        let held = pool.acquire().await.unwrap();
        let db = Db::Sqlite(pool.clone()); let kind = DbKind::of("sqlite");
        let error = tokio::time::timeout(std::time::Duration::from_secs(1), apply(&db, &kind, &db, &kind, &request())).await.unwrap().unwrap_err();
        assert!(error.to_string().contains("STRUCTURE_CONTROLLED_EXECUTION_REQUIRED"));
        drop(held); pool.close().await;
    }
}
