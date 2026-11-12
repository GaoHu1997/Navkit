//! Reviewed legacy migration. Confirmation imports disabled local definitions, never database work.
//! The opaque plan binds the exact source bytes, target job state, principal, timezone and payload.
use super::{clock::*, contracts::*, store::SchedulerStore};
use anyhow::{anyhow, bail, ensure, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{de::{self, MapAccess, SeqAccess, Visitor}, Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, fmt, fs::{self, File, OpenOptions}, io::Read,
    path::{Path, PathBuf}, sync::Mutex, time::{Duration as MonotonicDuration, Instant}};

#[cfg(test)]
#[path = "scheduler_migration_tests.rs"]
mod tests;

const SOURCE_LIMIT: usize = 16 * 1024 * 1024;
const PLAN_BYTES_LIMIT: usize = 32 * 1024 * 1024;
const PLAN_LIMIT: usize = 64;
const TTL: MonotonicDuration = MonotonicDuration::from_secs(120);
const SAFE_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LegacyMigrationStatus { Ready, Blocked, AlreadyImported }
#[derive(Debug, Clone, Serialize)]
pub struct LegacyMigrationItem {
    pub legacy_id: String,
    pub name: String,
    pub status: LegacyMigrationStatus,
    pub code: Option<String>,
    pub was_enabled: Option<bool>,
    pub warnings: Vec<String>,
    pub job: Option<ScheduledJobV2>,
    pub template: Option<ActionTemplate>,
}
#[derive(Debug, Clone, Serialize)]
pub struct LegacyMigrationPreview {
    pub plan_id: String,
    pub review_digest: String,
    pub source_digest: String,
    pub target_revision: String,
    pub timezone: String,
    pub prepared_at_utc: DateTime<Utc>,
    pub expires_at_utc: DateTime<Utc>,
    pub items: Vec<LegacyMigrationItem>,
}
#[derive(Debug, Clone, Serialize)]
pub struct LegacyMigrationReceipt {
    pub plan_id: String,
    pub review_digest: String,
    pub source_digest: String,
    pub jobs: Vec<JobRecord>,
    pub blocked_count: usize,
}
#[derive(Debug, Clone)]
pub(crate) struct PreparedLegacyJob {
    pub job: ScheduledJobV2,
    pub template: ActionTemplate,
    pub legacy: LegacyJobReference,
}
struct PreparedMigration {
    principal: String,
    root: PathBuf,
    created: Instant,
    bytes: usize,
    preview: LegacyMigrationPreview,
    jobs: Vec<PreparedLegacyJob>,
}
#[derive(Default)]
pub struct LegacyMigrationRegistry { plans: Mutex<BTreeMap<String, PreparedMigration>> }

/// serde_json::Value alone accepts duplicate object keys. Legacy files must not do so.
struct StrictValue(Value);
impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("JSON without duplicate fields") }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<StrictValue, E> { Ok(StrictValue(Value::Bool(v))) }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<StrictValue, E> { Ok(StrictValue(v.into())) }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<StrictValue, E> { Ok(StrictValue(v.into())) }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<StrictValue, E> {
                serde_json::Number::from_f64(v).map(|n| StrictValue(Value::Number(n))).ok_or_else(|| E::custom("nonfinite JSON"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<StrictValue, E> { Ok(StrictValue(Value::String(v.into()))) }
            fn visit_string<E: de::Error>(self, v: String) -> std::result::Result<StrictValue, E> { Ok(StrictValue(Value::String(v))) }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<StrictValue, E> { Ok(StrictValue(Value::Null)) }
            fn visit_none<E: de::Error>(self) -> std::result::Result<StrictValue, E> { self.visit_unit() }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<StrictValue, A::Error> {
                let mut values = Vec::new();
                while let Some(v) = seq.next_element::<StrictValue>()? { values.push(v.0); }
                Ok(StrictValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<StrictValue, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((k, v)) = map.next_entry::<String, StrictValue>()? {
                    if values.insert(k, v.0).is_some() { return Err(de::Error::custom("duplicate JSON field")); }
                }
                Ok(StrictValue(Value::Object(values)))
            }
        }
        d.deserialize_any(JsonVisitor)
    }
}
fn reject_link(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    ensure!(!meta.file_type().is_symlink(), "SCHEDULE_LEGACY_PATH_UNPROVEN");
    #[cfg(windows)] { use std::os::windows::fs::MetadataExt;
        ensure!(meta.file_attributes() & 0x400 == 0, "SCHEDULE_LEGACY_PATH_UNPROVEN"); }
    Ok(())
}
fn checked_root(root: &Path) -> Result<PathBuf> {
    for parent in root.ancestors() { if !parent.as_os_str().is_empty() { reject_link(parent)?; } }
    ensure!(root.is_dir(), "SCHEDULE_LEGACY_PATH_UNPROVEN");
    Ok(fs::canonicalize(root)?)
}
pub(crate) struct LegacySource {
    root: PathBuf,
    pub digest: String,
    pub entries: Vec<Value>,
    // Windows denies writer/delete sharing until the disabled import has really settled.
    _file: Option<File>,
}
impl LegacySource {
    pub(crate) fn read(root: &Path) -> Result<Self> {
        let root = checked_root(root)?;
        let path = root.join("jobs.json");
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                ensure!(!root.join("jobs.json.bak").exists(), "SCHEDULE_LEGACY_RECOVERY_REQUIRED");
                return Ok(Self { root, digest: bytes_digest(b"navkit:legacy:absent"), entries: vec![], _file: None });
            }
            Err(_) => bail!("SCHEDULE_LEGACY_SOURCE_UNREADABLE"),
            Ok(_) => reject_link(&path)?,
        }
        let mut options = OpenOptions::new(); options.read(true);
        #[cfg(windows)] { use std::os::windows::fs::OpenOptionsExt; options.share_mode(1); }
        let mut file = options.open(&path).map_err(|_| anyhow!("SCHEDULE_LEGACY_SOURCE_UNREADABLE"))?;
        ensure!(file.metadata()?.is_file() && file.metadata()?.len() <= SOURCE_LIMIT as u64, "SCHEDULE_LEGACY_SOURCE_LIMIT");
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        (&mut file).take(SOURCE_LIMIT as u64 + 1).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= SOURCE_LIMIT, "SCHEDULE_LEGACY_SOURCE_LIMIT");
        let StrictValue(parsed) = serde_json::from_slice(&bytes).map_err(|_| anyhow!("SCHEDULE_LEGACY_SOURCE_INVALID: 旧文件不是有效且无重复字段的 JSON，未当作空任务表"))?;
        let Value::Array(entries) = parsed else { bail!("SCHEDULE_LEGACY_SOURCE_INVALID"); };
        ensure!(entries.len() <= MAX_JOBS, "SCHEDULE_LEGACY_SOURCE_LIMIT");
        let mut ids = BTreeSet::new();
        for row in &entries { if let Some(id) = row.get("id").and_then(Value::as_str) {
            ensure!(ids.insert(id.to_owned()), "SCHEDULE_LEGACY_DUPLICATE_ID");
        }}
        Ok(Self { root, digest: bytes_digest(&bytes), entries, _file: Some(file) })
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        ensure!(Self::read(&self.root)?.digest == self.digest, "SCHEDULE_LEGACY_SOURCE_CHANGED: 旧文件已变化，须重新预览");
        Ok(())
    }
}
fn bytes_digest(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
pub(crate) fn legacy_target_revision(state: &SchedulerState) -> Result<String> {
    // Uninitialized state has a fresh UUID on every read; review its stable business state instead.
    digest(&(&state.jobs, &state.imported_legacy_digests))
}
pub(crate) fn legacy_template_equal(a: &ActionTemplate, b: &ActionTemplate) -> bool {
    let mut a = a.clone(); let mut b = b.clone(); a.revision.clear(); b.revision.clear(); a == b
}
fn stable_uuid(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes()); let mut bytes = [0u8; 16]; bytes.copy_from_slice(&hash[..16]);
    bytes[6] = (bytes[6] & 15) | 0x50; bytes[8] = (bytes[8] & 63) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn check_text(value: &str, max: usize, empty: bool) -> Result<()> {
    ensure!(value.len() <= max && !value.chars().any(char::is_control) && (empty || !value.trim().is_empty()), "SCHEDULE_LEGACY_INVALID"); Ok(())
}
fn exact_json(value: &Value, depth: usize) -> Result<()> {
    ensure!(depth <= 32, "SCHEDULE_LEGACY_OPTIONS_UNPROVEN");
    match value {
        Value::Number(n) => ensure!(n.as_u64().is_some_and(|v| v <= SAFE_INTEGER)
            || n.as_i64().is_some_and(|v| v.unsigned_abs() <= SAFE_INTEGER), "SCHEDULE_LEGACY_UNSAFE_NUMBER"),
        Value::Array(values) => for v in values { exact_json(v, depth + 1)?; },
        Value::Object(values) => for (key, v) in values {
            ensure!(!matches!(key.as_str(), "__proto__" | "constructor" | "prototype"), "SCHEDULE_LEGACY_OPTIONS_UNPROVEN");
            exact_json(v, depth + 1)?;
        },
        _ => {},
    }
    Ok(())
}
fn preserved_fields(original: &Value, normalized: &Value) -> Result<()> {
    match original {
        Value::Object(fields) => for (key, value) in fields {
            let target = normalized.get(key).ok_or_else(|| anyhow!("SCHEDULE_LEGACY_UNKNOWN_FIELD"))?;
            preserved_fields(value, target)?;
        },
        Value::Array(values) => {
            let target = normalized.as_array().ok_or_else(|| anyhow!("SCHEDULE_LEGACY_OPTIONS_UNPROVEN"))?;
            ensure!(values.len() == target.len(), "SCHEDULE_LEGACY_OPTIONS_UNPROVEN");
            for (a, b) in values.iter().zip(target) { preserved_fields(a, b)?; }
        }
        _ => ensure!(original == normalized, "SCHEDULE_LEGACY_OPTIONS_UNPROVEN: 参数不可静默舍入或改写"),
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyTask { kind: String, options: Value }
fn tool_template(old: &crate::models::ScheduleJob, task: &Value, id: String) -> Result<(ActionTemplate, Vec<String>)> {
    ensure!(old.sql.trim().is_empty(), "SCHEDULE_LEGACY_AMBIGUOUS_ACTION");
    let task: LegacyTask = serde_json::from_value(task.clone()).map_err(|_| anyhow!("SCHEDULE_LEGACY_OPTIONS_UNPROVEN"))?;
    ensure!(task.options.is_object(), "SCHEDULE_LEGACY_OPTIONS_UNPROVEN");
    let mut options = task.options;
    let mut warnings = vec!["工具输出使用每次运行的独立文件，不覆盖既有文件；执行前仍需核对文件和元数据范围。".into()];
    if task.kind == "data_sync" && options.get("mode").is_none() {
        let source: crate::models::TransferOptions = serde_json::from_value(options.clone()).map_err(|_| anyhow!("SCHEDULE_LEGACY_OPTIONS_UNPROVEN"))?;
        preserved_fields(&options, &serde_json::to_value(&source)?)?;
        ensure!(!source.create_target && !source.drop_existing && !source.copy_structure && source.copy_data,
            "SCHEDULE_LEGACY_DESTRUCTIVE_TOOL: 旧同步含隐式建表/复制结构/删除，不能改成另一种任务");
        options = json!({ "task_id": null, "source_connection": source.source_connection, "source_schema": source.source_schema,
            "source_table": source.source_table, "target_connection": source.target_connection, "target_schema": source.target_schema,
            "target_table": source.target_table, "mode": "data", "write_mode": "append", "sync_mode": "full", "incremental_update": false,
            "create_target": false, "columns": [], "key_columns": [], "delete_missing": false, "where_clause": source.where_clause });
        warnings.push("旧数据传输按等效的已有表追加动作迁移；模板主连接绑定实际写入目标，审批包含源与目标。".into());
    }
    ensure!(matches!(task.kind.as_str(), "backup" | "export" | "import" | "data_sync" | "structure_sync"), "SCHEDULE_LEGACY_TOOL_UNSUPPORTED");
    let raw = json!({ "kind": task.kind, "options": options });
    let definition: crate::scheduler_tools::ToolDefinition = serde_json::from_value(raw.clone()).map_err(|_| anyhow!("SCHEDULE_LEGACY_OPTIONS_UNPROVEN"))?;
    preserved_fields(&raw, &serde_json::to_value(&definition)?)?;
    let (kind, connection, namespace, read_only) = match &definition {
        crate::scheduler_tools::ToolDefinition::Backup(o) => {
            ensure!(!o.tables.is_empty() && o.tables.len() <= 512 && o.routines.is_empty() && (o.include_data || o.include_structure), "SCHEDULE_LEGACY_BACKUP_SCOPE_REQUIRED");
            let mut tables = BTreeSet::new(); for table in &o.tables { check_text(table, 1024, false)?; ensure!(tables.insert(table), "SCHEDULE_LEGACY_OPTIONS_UNPROVEN"); }
            if let Some(file) = &o.file_name { ensure!(file.is_empty() || (!file.contains(['/', '\\']) && file != "." && file != ".."), "SCHEDULE_LEGACY_OPTIONS_UNPROVEN"); }
            (ActionKind::Backup, &o.connection_id, &o.schema, true)
        },
        crate::scheduler_tools::ToolDefinition::Export(o) => { check_text(&o.table, 1024, false)?; check_text(&o.file, 16_384, false)?;
            ensure!(matches!(o.format.as_str(), "csv" | "json" | "sql"), "SCHEDULE_LEGACY_OPTIONS_UNPROVEN");
            (ActionKind::Export, &o.connection_id, &o.schema, true) },
        crate::scheduler_tools::ToolDefinition::Import(o) => { check_text(&o.table, 1024, false)?; check_text(&o.file, 16_384, false)?;
            (ActionKind::Import, &o.connection_id, &o.schema, false) },
        crate::scheduler_tools::ToolDefinition::DataSync(o) => { check_text(&o.source_table, 1024, false)?; check_text(&o.target_table, 1024, false)?;
            ensure!((old.connection_id == o.source_connection && old.database == o.source_schema) || (old.connection_id == o.target_connection && old.database == o.target_schema), "SCHEDULE_LEGACY_BINDING_MISMATCH");
            (ActionKind::DataSync, &o.target_connection, &o.target_schema, false) },
        crate::scheduler_tools::ToolDefinition::StructureSync(o) => (ActionKind::StructureSync, &o.target_connection, &o.target_schema, false),
    };
    if !matches!(kind, ActionKind::DataSync | ActionKind::StructureSync) {
        ensure!(old.connection_id == *connection && old.database == *namespace, "SCHEDULE_LEGACY_BINDING_MISMATCH");
    }
    let template = ActionTemplate { id, revision: uuid::Uuid::new_v4().to_string(), name: old.name.clone(), kind,
        connection_id: connection.clone(), namespace: namespace.clone(), read_only,
        configuration: TemplateConfiguration::Tool { options_json: serde_json::to_string(&raw)? } };
    crate::scheduler_tools::validate_template_options(&template)?;
    Ok((template, warnings))
}
pub(crate) fn convert_legacy(raw: &Value, zone: &str, now: DateTime<Utc>) -> Result<(PreparedLegacyJob, Vec<String>, bool)> {
    exact_json(raw, 0)?;
    let fields = raw.as_object().ok_or_else(|| anyhow!("SCHEDULE_LEGACY_INVALID"))?;
    const KNOWN: &[&str] = &["id", "name", "connection_id", "database", "sql", "interval_secs", "daily_at", "cron", "task", "enabled", "last_run", "last_status"];
    ensure!(fields.keys().all(|k| KNOWN.contains(&k.as_str())), "SCHEDULE_LEGACY_UNKNOWN_FIELD");
    let old: crate::models::ScheduleJob = serde_json::from_value(raw.clone()).map_err(|_| anyhow!("SCHEDULE_LEGACY_INVALID"))?;
    check_text(&old.id, 128, false)?; check_text(&old.name, 256, false)?; check_text(&old.connection_id, 128, false)?; check_text(&old.database, 1024, true)?;
    timezone(zone)?;
    let id = stable_uuid(&format!("legacy-job:{}", old.id));
    let template_id = stable_uuid(&format!("legacy-template:{}", old.id));
    let action_id = stable_uuid(&format!("legacy-action:{}", old.id));
    let (template, mut warnings) = if let Some(task) = &old.task { tool_template(&old, task, template_id)? } else {
        let template = ActionTemplate { id: template_id, revision: uuid::Uuid::new_v4().to_string(), name: old.name.clone(), kind: ActionKind::Sql,
            connection_id: old.connection_id.clone(), namespace: old.database.clone(), read_only: false,
            configuration: TemplateConfiguration::Sql { sql: old.sql.clone() } };
        validate_template(&template)?; (template, vec!["SQL 模板没有数据库强制只读证明；启用前必须核对完整 SQL 及无行数上限的副作用风险。".into()])
    };
    let schedule = if let Some(cron) = &old.cron {
        ensure!(!cron.trim().is_empty(), "SCHEDULE_LEGACY_CRON_INVALID");
        if old.daily_at.is_some() || old.interval_secs > 0 { warnings.push("按旧定义的优先级保留 Cron，不另运行每日或间隔配置。".into()); }
        ScheduleSpec::Calendar { cron: cron.clone(), timezone: zone.into(), dst_gap: DstGap::Skip, dst_fold: DstFold::First }
    } else if let Some(at) = &old.daily_at {
        let (h, m) = at.trim().split_once(':').ok_or_else(|| anyhow!("SCHEDULE_LEGACY_DAILY_INVALID"))?;
        ensure!(h.len() <= 2 && m.len() == 2 && h.bytes().all(|v| v.is_ascii_digit()) && m.bytes().all(|v| v.is_ascii_digit()), "SCHEDULE_LEGACY_DAILY_INVALID");
        let h: u8 = h.parse()?; let m: u8 = m.parse()?; ensure!(h < 24 && m < 60, "SCHEDULE_LEGACY_DAILY_INVALID");
        ScheduleSpec::Calendar { cron: format!("{m} {h} * * *"), timezone: zone.into(), dst_gap: DstGap::Skip, dst_fold: DstFold::First }
    } else if old.interval_secs > 0 {
        warnings.push("固定间隔以本次预览 UTC 为新锚点；含糊的旧本地运行时刻仅保留为文本，不伪造 UTC 历史。".into());
        ScheduleSpec::Interval { seconds: old.interval_secs, anchor_utc: now }
    } else { ScheduleSpec::Manual };
    let job = ScheduledJobV2 { version: 2, id, revision: uuid::Uuid::new_v4().to_string(), name: old.name.clone(), enabled: false,
        execution_mode: ExecutionMode::AppOnline, schedule, misfire: MisfirePolicy::Skip, overlap: OverlapPolicy::Skip, timeout_ms: 300_000,
        actions: vec![ScheduledAction { id: action_id, kind: template.kind, depends_on: vec![], template_ref: template.id.clone(), approved_scope_revision: String::new() }] };
    validate_template(&template)?; validate_job(&job)?;
    if let Some(last) = &old.last_run { check_text(last, 256, true)?; }
    warnings.push("新任务默认暂停且没有端点批准；旧定义和旧历史文件保持原样。".into());
    let legacy = LegacyJobReference { legacy_id: old.id.clone(), last_run_local_text: old.last_run.clone(),
        last_status: old.last_status.as_ref().map(|s| if s == "成功" { "成功".into() } else { "既有历史（查看旧记录）".into() }), source_digest: digest(&old)? };
    Ok((PreparedLegacyJob { job, template, legacy }, warnings, old.enabled))
}
fn public_label(raw: &Value, field: &str) -> String {
    raw.get(field).and_then(Value::as_str).unwrap_or("无法识别").chars().filter(|c| !c.is_control()).take(256).collect()
}
fn public_code(error: &anyhow::Error) -> String {
    let message = error.to_string(); let code = message.split(':').next().unwrap_or("");
    if code.starts_with("SCHEDULE_") && code.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_') { code.into() }
    else { "SCHEDULE_LEGACY_INVALID".into() }
}
impl LegacyMigrationRegistry {
    pub fn prepare(&self, store: &SchedulerStore, zone: &str, principal: &str, now: DateTime<Utc>) -> Result<LegacyMigrationPreview> {
        check_text(principal, 128, false)?; timezone(zone)?;
        let source = LegacySource::read(store.data_root())?;
        let state = store.snapshot()?; let target_revision = legacy_target_revision(&state)?;
        let mut items = Vec::new(); let mut jobs = Vec::new();
        for raw in &source.entries {
            let mut item = LegacyMigrationItem { legacy_id: public_label(raw, "id"), name: public_label(raw, "name"), status: LegacyMigrationStatus::Blocked,
                code: None, was_enabled: raw.get("enabled").and_then(Value::as_bool), warnings: vec![], job: None, template: None };
            match convert_legacy(raw, zone, now) {
                Ok((prepared, warnings, enabled)) => {
                    item.was_enabled = Some(enabled); item.warnings = warnings;
                    if state.imported_legacy_digests.contains(&prepared.legacy.source_digest) { item.status = LegacyMigrationStatus::AlreadyImported; }
                    else if state.jobs.contains_key(&prepared.job.id) { item.code = Some("SCHEDULE_LEGACY_CHANGED".into()); }
                    else { item.status = LegacyMigrationStatus::Ready; item.job = Some(prepared.job.clone()); item.template = Some(prepared.template.clone()); jobs.push(prepared); }
                }
                Err(error) => item.code = Some(public_code(&error)),
            }
            items.push(item);
        }
        ensure!(state.jobs.len() + jobs.len() <= MAX_JOBS, "SCHEDULE_JOB_QUOTA");
        source.recheck()?;
        let plan_id = uuid::Uuid::new_v4().to_string();
        let mut preview = LegacyMigrationPreview { plan_id: plan_id.clone(), review_digest: String::new(), source_digest: source.digest,
            target_revision, timezone: zone.into(), prepared_at_utc: now, expires_at_utc: now + Duration::seconds(120), items };
        preview.review_digest = digest(&preview)?;
        let bytes = serde_json::to_vec(&preview)?.len(); ensure!(bytes <= SOURCE_LIMIT, "SCHEDULE_LEGACY_PLAN_LIMIT");
        let mut plans = self.plans.lock().map_err(|_| anyhow!("SCHEDULE_LEGACY_PLAN_UNAVAILABLE"))?;
        plans.retain(|_, plan| plan.created.elapsed() < TTL);
        ensure!(plans.len() < PLAN_LIMIT && plans.values().map(|p| p.bytes).sum::<usize>() + bytes <= PLAN_BYTES_LIMIT, "SCHEDULE_LEGACY_PLAN_LIMIT");
        plans.insert(plan_id, PreparedMigration { principal: principal.into(), root: source.root, created: Instant::now(), bytes, preview: preview.clone(), jobs });
        Ok(preview)
    }
    pub fn confirm(&self, store: &SchedulerStore, plan_id: &str, review_digest: &str, principal: &str) -> Result<LegacyMigrationReceipt> {
        canonical_uuid(plan_id)?; check_text(principal, 128, false)?; ensure!(is_digest(review_digest), "SCHEDULE_LEGACY_PLAN_INVALID");
        let prepared = {
            let mut plans = self.plans.lock().map_err(|_| anyhow!("SCHEDULE_LEGACY_PLAN_UNAVAILABLE"))?;
            let plan = plans.get(plan_id).ok_or_else(|| anyhow!("SCHEDULE_LEGACY_PLAN_CONSUMED"))?;
            ensure!(plan.principal == principal, "SCHEDULE_LEGACY_PLAN_FORBIDDEN");
            // Consume before any persistence. Unknown acknowledgements are never retried.
            plans.remove(plan_id).ok_or_else(|| anyhow!("SCHEDULE_LEGACY_PLAN_CONSUMED"))?
        };
        ensure!(prepared.created.elapsed() < TTL && prepared.preview.review_digest == review_digest, "SCHEDULE_LEGACY_PLAN_STALE");
        ensure!(checked_root(store.data_root())? == prepared.root, "SCHEDULE_LEGACY_SOURCE_CHANGED");
        let source = LegacySource::read(&prepared.root)?;
        ensure!(source.digest == prepared.preview.source_digest, "SCHEDULE_LEGACY_SOURCE_CHANGED");
        ensure!(!prepared.jobs.is_empty(), "SCHEDULE_LEGACY_NOTHING_TO_IMPORT");
        let jobs = store.import_legacy_prepared(&prepared.jobs, &prepared.preview.target_revision, || source.recheck())?;
        Ok(LegacyMigrationReceipt { plan_id: plan_id.into(), review_digest: review_digest.into(), source_digest: source.digest,
            jobs, blocked_count: prepared.preview.items.iter().filter(|i| i.status == LegacyMigrationStatus::Blocked).count() })
    }
}
