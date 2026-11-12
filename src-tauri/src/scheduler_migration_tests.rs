//! Real legacy-file/AES-GCM migration tests. No database executor is involved in migration.
use super::*;
use std::fs;

struct Fixture { root: PathBuf, store: SchedulerStore, registry: LegacyMigrationRegistry }
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("navkit-legacy-migration-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap(); fs::write(root.join("key.bin"), [7; 32]).unwrap();
        Self { store: SchedulerStore::new(root.clone()), root, registry: LegacyMigrationRegistry::default() }
    }
    fn source(&self, rows: &[Value]) { fs::write(self.root.join("jobs.json"), serde_json::to_vec(rows).unwrap()).unwrap(); }
    fn preview(&self) -> LegacyMigrationPreview { self.registry.prepare(&self.store, "Asia/Shanghai", "main", now()).unwrap() }
    fn confirm(&self, p: &LegacyMigrationPreview) -> Result<LegacyMigrationReceipt> { self.registry.confirm(&self.store, &p.plan_id, &p.review_digest, "main") }
}
impl Drop for Fixture { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.root); } }
fn now() -> DateTime<Utc> { DateTime::parse_from_rfc3339("2026-11-10T06:00:00Z").unwrap().with_timezone(&Utc) }
fn sql(id: &str) -> Value {
    json!({ "id": id, "name": format!("old {id}"), "connection_id": "connection", "database": "main", "sql": "SELECT 12345;", "interval_secs": 60, "enabled": true, "last_run": "2026/11/10 13:00:00", "last_status": "成功" })
}
fn task(id: &str, kind: &str, options: Value) -> Value {
    let mut row = sql(id); row["sql"] = json!(""); row["task"] = json!({ "kind": kind, "options": options }); row
}
fn error(result: Result<LegacyMigrationReceipt>, code: &str) { assert!(result.unwrap_err().to_string().contains(code)); }

#[test]
fn cron_daily_interval_and_manual_are_converted_without_invented_history() {
    let mut cron = sql("cron"); cron["cron"] = json!("0 9 * * 1-5"); cron["daily_at"] = json!("08:30");
    let (c, _, _) = convert_legacy(&cron, "America/New_York", now()).unwrap();
    assert!(matches!(&c.job.schedule, ScheduleSpec::Calendar { cron, timezone, .. } if cron == "0 9 * * 1-5" && timezone == "America/New_York"));
    let mut daily = sql("daily"); daily["daily_at"] = json!("9:05");
    let (d, _, _) = convert_legacy(&daily, "Asia/Shanghai", now()).unwrap();
    assert!(matches!(&d.job.schedule, ScheduleSpec::Calendar { cron, .. } if cron == "5 9 * * *"));
    let (i, _, _) = convert_legacy(&sql("interval"), "Asia/Shanghai", now()).unwrap();
    assert!(matches!(&i.job.schedule, ScheduleSpec::Interval { seconds: 60, anchor_utc } if *anchor_utc == now()));
    let mut manual = sql("manual"); manual["interval_secs"] = json!(0);
    let (m, _, _) = convert_legacy(&manual, "Asia/Shanghai", now()).unwrap(); assert_eq!(m.job.schedule, ScheduleSpec::Manual);
    for item in [c, d, i, m] { assert!(!item.job.enabled); assert!(item.job.actions.iter().all(|a| a.approved_scope_revision.is_empty())); assert_eq!(item.legacy.last_run_local_text.as_deref(), Some("2026/11/10 13:00:00")); }
}
#[test]
fn real_confirm_keeps_original_bytes_history_and_imports_only_paused_unapproved_jobs() {
    let f = Fixture::new(); f.source(&[sql("one")]); fs::write(f.root.join("job_history.json"), b"old history\r\n").unwrap();
    let source = fs::read(f.root.join("jobs.json")).unwrap(); let history = fs::read(f.root.join("job_history.json")).unwrap();
    let p = f.preview(); assert_eq!(p.items[0].status, LegacyMigrationStatus::Ready); assert!(f.store.list_jobs().unwrap().is_empty());
    let r = f.confirm(&p).unwrap(); assert_eq!(r.plan_id, p.plan_id); assert_eq!(r.jobs.len(), 1);
    let job = &r.jobs[0]; assert!(!job.job.enabled); assert!(job.next_run_utc.is_none()); assert!(job.approvals.is_empty()); assert!(job.queued.is_none()); assert!(job.paused_reason.is_some());
    assert_eq!(fs::read(f.root.join("jobs.json")).unwrap(), source); assert_eq!(fs::read(f.root.join("job_history.json")).unwrap(), history);
    let template = f.store.load_template(&job.job.actions[0].template_ref).unwrap(); assert_eq!(template.kind, ActionKind::Sql);
    let encrypted = fs::read(f.root.join(format!("scheduler/template-{}.enc", template.id))).unwrap(); assert!(!String::from_utf8_lossy(&encrypted).contains("SELECT 12345"));
    assert!(f.store.history(None).unwrap().is_empty()); assert_eq!(f.preview().items[0].status, LegacyMigrationStatus::AlreadyImported);
    error(f.confirm(&p), "SCHEDULE_LEGACY_PLAN_CONSUMED");
}
#[test]
fn source_bytes_changed_after_review_are_rejected_even_when_json_is_semantically_equal() {
    let f = Fixture::new(); f.source(&[sql("one")]); let p = f.preview();
    fs::write(f.root.join("jobs.json"), serde_json::to_vec_pretty(&vec![sql("one")]).unwrap()).unwrap();
    error(f.confirm(&p), "SCHEDULE_LEGACY_SOURCE_CHANGED"); assert!(f.store.list_jobs().unwrap().is_empty()); assert!(f.store.list_templates().unwrap().is_empty());
}
#[test]
fn foreign_window_cannot_consume_the_owner_plan_and_wrong_digest_cannot_be_retried() {
    let f = Fixture::new(); f.source(&[sql("one")]); let p = f.preview();
    error(f.registry.confirm(&f.store, &p.plan_id, &p.review_digest, "other"), "SCHEDULE_LEGACY_PLAN_FORBIDDEN");
    error(f.registry.confirm(&f.store, &p.plan_id, &"f".repeat(64), "main"), "SCHEDULE_LEGACY_PLAN_STALE");
    error(f.confirm(&p), "SCHEDULE_LEGACY_PLAN_CONSUMED"); assert!(f.store.list_jobs().unwrap().is_empty());
}
#[test]
fn elapsed_plan_is_consumed_without_any_template_write() {
    let f = Fixture::new(); f.source(&[sql("one")]); let p = f.preview();
    f.registry.plans.lock().unwrap().get_mut(&p.plan_id).unwrap().created = Instant::now() - TTL - MonotonicDuration::from_secs(1);
    error(f.confirm(&p), "SCHEDULE_LEGACY_PLAN_STALE"); assert!(f.store.list_templates().unwrap().is_empty());
}
#[test]
fn a_plan_is_bound_to_the_actual_store_root() {
    let f = Fixture::new(); let other = Fixture::new(); f.source(&[sql("one")]); other.source(&[sql("one")]); let p = f.preview();
    error(f.registry.confirm(&other.store, &p.plan_id, &p.review_digest, "main"), "SCHEDULE_LEGACY_SOURCE_CHANGED"); assert!(other.store.list_jobs().unwrap().is_empty());
}
#[test]
fn target_business_state_changed_requires_a_new_preview() {
    let f = Fixture::new(); f.source(&[sql("one")]); let p = f.preview();
    let other = f.registry.prepare(&f.store, "UTC", "main", now()).unwrap(); f.confirm(&other).unwrap();
    error(f.confirm(&p), "SCHEDULE_LEGACY_TARGET_CHANGED"); assert_eq!(f.store.list_jobs().unwrap().len(), 1);
}
#[test]
fn unsupported_tools_unknown_fields_and_unsafe_options_are_explicit_blocked_items() {
    let f = Fixture::new(); let mut unknown = sql("unknown-field"); unknown["future_safety_option"] = json!(true);
    let backup = task("all-tables", "backup", json!({"connection_id":"connection","schema":"main","tables":[]}));
    let destructive = task("create", "import", json!({"connection_id":"connection","schema":"main","table":"items","file":"data.csv","format":"csv","create_table":true}));
    let unknown_tool = task("generator", "data_generation", json!({}));
    f.source(&[sql("good"), unknown, backup, destructive, unknown_tool]); let p = f.preview();
    assert_eq!(p.items.iter().filter(|i| i.status == LegacyMigrationStatus::Ready).count(), 1); assert_eq!(p.items.iter().filter(|i| i.status == LegacyMigrationStatus::Blocked).count(), 4);
    assert!(p.items.iter().filter(|i| i.status == LegacyMigrationStatus::Blocked).all(|i| i.code.is_some() && i.job.is_none() && i.template.is_none()));
    let receipt = f.confirm(&p).unwrap(); assert_eq!(receipt.blocked_count, 4); assert_eq!(receipt.jobs.len(), 1);
}
#[test]
fn real_tool_options_preserve_exact_target_and_legacy_transfer_becomes_append_only() {
    let f = Fixture::new(); let mut export = task("export", "export", json!({"connection_id":"connection","schema":"main","table":"items","file":"data.csv","format":"csv","limit":9007199254740991u64})); export["cron"] = json!("*/5 * * * *");
    let sync = task("sync", "data_sync", json!({"source_connection":"connection","source_schema":"main","source_table":"source","target_connection":"target","target_schema":"other","target_table":"dest","copy_data":true}));
    f.source(&[export, sync]); let p = f.preview(); assert!(p.items.iter().all(|i| i.status == LegacyMigrationStatus::Ready));
    let template = p.items[1].template.as_ref().unwrap(); assert_eq!(template.connection_id, "target"); assert_eq!(template.namespace, "other");
    let TemplateConfiguration::Tool { options_json } = &template.configuration else { panic!("expected tool"); };
    let value: Value = serde_json::from_str(options_json).unwrap(); assert_eq!(value["options"]["write_mode"], "append"); assert_eq!(value["options"]["create_target"], false); assert_eq!(value["options"]["delete_missing"], false);
    assert_eq!(f.confirm(&p).unwrap().jobs.len(), 2);
}
#[test]
fn unknown_nested_tool_options_and_unsafe_json_numbers_are_not_silently_discarded() {
    let mut unknown = task("unknown", "export", json!({"connection_id":"connection","schema":"main","table":"t","file":"f.csv","format":"csv","unrecognized_safety":true}));
    assert!(convert_legacy(&unknown, "UTC", now()).is_err()); unknown["task"]["options"].as_object_mut().unwrap().remove("unrecognized_safety"); unknown["task"]["options"]["limit"] = json!(9007199254740993u64);
    assert!(convert_legacy(&unknown, "UTC", now()).unwrap_err().to_string().contains("UNSAFE_NUMBER"));
}
#[test]
fn corrupt_duplicate_json_or_duplicate_legacy_ids_never_become_an_empty_source() {
    let f = Fixture::new();
    for bytes in [b"{".as_slice(), br#"[{"id":"x","id":"y"}]"#.as_slice(), br#"[{"id":"x"},{"id":"x"}]"#.as_slice()] {
        fs::write(f.root.join("jobs.json"), bytes).unwrap(); assert!(f.registry.prepare(&f.store, "UTC", "main", now()).is_err()); assert!(f.store.list_jobs().unwrap().is_empty());
    }
}
#[test]
fn missing_original_key_does_not_create_a_replacement_or_destroy_source() {
    let f = Fixture::new(); f.source(&[sql("one")]); let p = f.preview(); let before = fs::read(f.root.join("jobs.json")).unwrap(); fs::remove_file(f.root.join("key.bin")).unwrap();
    assert!(f.confirm(&p).is_err()); assert!(!f.root.join("key.bin").exists()); assert_eq!(fs::read(f.root.join("jobs.json")).unwrap(), before); assert!(f.store.list_jobs().unwrap().is_empty());
}
#[test]
fn orphan_template_conflict_preflights_the_entire_import_before_any_write() {
    let f = Fixture::new(); f.source(&[sql("one"), sql("two")]); let p = f.preview();
    let mut conflict = p.items[1].template.clone().unwrap(); conflict.revision.clear(); conflict.configuration = TemplateConfiguration::Sql { sql: "SELECT different".into() };
    f.store.save_template(conflict.clone(), None).unwrap(); let path = f.root.join(format!("scheduler/template-{}.enc", conflict.id)); let before = fs::read(&path).unwrap();
    error(f.confirm(&p), "SCHEDULE_LEGACY_TEMPLATE_CONFLICT"); assert_eq!(fs::read(path).unwrap(), before); assert_eq!(f.store.list_templates().unwrap().len(), 1); assert!(f.store.list_jobs().unwrap().is_empty());
}
#[test]
fn exact_orphan_template_is_reused_without_overwriting_or_enabling() {
    let f = Fixture::new(); f.source(&[sql("one")]); let p = f.preview(); let mut template = p.items[0].template.clone().unwrap(); template.revision.clear();
    let saved = f.store.save_template(template, None).unwrap(); let path = f.root.join(format!("scheduler/template-{}.enc", saved.id)); let before = fs::read(&path).unwrap();
    assert_eq!(f.confirm(&p).unwrap().jobs.len(), 1); assert_eq!(fs::read(path).unwrap(), before); assert!(!f.store.list_jobs().unwrap()[0].job.enabled);
}
#[test]
fn invalid_timezone_and_empty_source_never_create_claimable_jobs() {
    let f = Fixture::new(); f.source(&[]); assert!(f.registry.prepare(&f.store, "Not/A_Zone", "main", now()).is_err()); let p = f.preview(); assert!(p.items.is_empty()); error(f.confirm(&p), "SCHEDULE_LEGACY_NOTHING_TO_IMPORT"); assert!(f.store.list_jobs().unwrap().is_empty());
}
