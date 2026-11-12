//! C2 驱动桥的契约/门禁测试。FakeAdapter 只测错误处理，不算真实产品验收。
use super::*;
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use crate::http_db::{HttpAdapter, HttpRows};
use crate::models::{ConnectionRequest, DbKind};

fn caps() -> JdbcCapabilities {
    JdbcCapabilities { protocol_version: 2, session_id: uuid::Uuid::new_v4().to_string(), adapter_id: "jdbc".into(),
        product: "H2".into(), product_version: "2.3.232 (2024-08-11)".into(), driver_name: "H2 JDBC Driver".into(),
        driver_version: "2.3.232".into(), catalog: "FIXTURE".into(), schema: "PUBLIC".into(), read_only: false,
        transactions: true, metadata: true, row_write: true, ddl_write: false,
        bound_types: TYPES.into_iter().map(String::from).collect(), reason: None }
}
fn col(name: &str, kind: i32, type_name: &str, precision: u32, scale: i32) -> JdbcColumnMetadata {
    JdbcColumnMetadata { name: name.into(), jdbc_type: kind, type_name: type_name.into(), ordinal: 1, precision, scale,
        nullable: Some(true), auto_increment: Some(false), generated: Some(false), read_only: false,
        default_value: None, generation_expression: None, writable_type: true }
}
fn metadata(caps: &JdbcCapabilities) -> JdbcMetadataSnapshot {
    let mut columns = vec![col("ID", -5, "BIGINT", 64, 0), col("PAY", 12, "CHARACTER VARYING", 100, 0),
        col("AMT", 3, "DECIMAL", 38, 12), col("NIL", 12, "CHARACTER VARYING", 20, 0), col("BIN", -3, "BINARY VARYING", 64, 0),
        col("D", 91, "DATE", 10, 0), col("TM", 92, "TIME", 18, 9), col("TS", 93, "TIMESTAMP", 29, 9),
        col("TZ", 2014, "TIMESTAMP WITH TIME ZONE", 35, 9), col("U", 1111, "UUID", 16, 0), col("GEN", -5, "BIGINT", 64, 0)];
    columns[0].nullable = Some(false);
    columns[10].generated = Some(true); columns[10].read_only = true; columns[10].generation_expression = Some("2".into());
    for (index, column) in columns.iter_mut().enumerate() { column.ordinal = index as u32 + 1; }
    JdbcMetadataSnapshot { object: JdbcObjectId { catalog: caps.catalog.clone(), schema: "PUBLIC".into(), name: "T".into() },
        session_id: caps.session_id.clone(), backend_object_id: "3".into(), revision: "a".repeat(64), table_type: "BASE TABLE".into(),
        definition: Some("CREATE TABLE PUBLIC.T(ID BIGINT PRIMARY KEY,PAY VARCHAR(100))".into()), columns,
        primary_key: vec!["ID".into()], unique_keys: vec![vec!["ID".into()]], indexes: json!([]), foreign_keys: json!([]),
        constraints: json!([]), triggers: json!([]), completeness: ["columns", "keys", "indexes", "foreign_keys", "constraints", "triggers", "definition"]
            .into_iter().map(|name| (name.into(), "known".into())).collect() }
}
fn baseline() -> Pairs {
    vec![("ID".into(), DbValue::Integer("1".into())), ("PAY".into(), DbValue::Text("old".into())),
        ("AMT".into(), DbValue::Decimal("12345678901234567890.123456789012".into())), ("NIL".into(), DbValue::Null),
        ("BIN".into(), DbValue::Binary("AP8BgA==".into())), ("D".into(), DbValue::Date("2026-02-28".into())),
        ("TM".into(), DbValue::Time("12:34:56.123456789".into())), ("TS".into(), DbValue::Timestamp("2026-11-09T12:34:56.123456789".into())),
        ("TZ".into(), DbValue::Timestamptz("2026-11-09T12:34:56.123456789+05:30".into())),
        ("U".into(), DbValue::Uuid("12345678-1234-1234-1234-123456789abc".into())), ("GEN".into(), DbValue::Integer("2".into()))]
}
fn draft() -> MutationDraft {
    MutationDraft { connection_id: "c1".into(), schema: "PUBLIC".into(), table: "T".into(), owner_id: "page1".into(), kind: "update".into(),
        keys: vec![("ID".into(), DbValue::Integer("1".into()))], original_values: vec![("PAY".into(), DbValue::Text("old".into()))],
        changes: vec![("PAY".into(), DbValue::Text("new".into()))] }
}
fn build(draft: &MutationDraft) -> PlanInternal {
    let caps = caps(); build_preview(draft, caps.clone(), metadata(&caps), "window1", "endpoint1".into()).unwrap()
}
fn code(error: anyhow::Error) -> String { AppError::from(error).code }

#[test]
fn typed_preview_normalizes_without_number_or_sql_literals() {
    let mut draft = draft();
    let payload = "中文🙂'; DELETE FROM another; --";
    draft.changes = vec![("PAY".into(), DbValue::Text(payload.into()))];
    let plan = build(&draft);
    assert!(plan.sql.contains("UPDATE \"PUBLIC\".\"T\" SET \"PAY\" = ? WHERE \"ID\" = ? AND \"PAY\" = ?"));
    assert!(!plan.sql.contains(payload));
    assert_eq!(plan.binds, [DbValue::Text(payload.into()), DbValue::Integer("1".into()), DbValue::Text("old".into())]);
    let request = operation(&uuid::Uuid::new_v4().to_string(), &plan, false, 10).unwrap();
    assert!(serde_json::to_value(request).unwrap().get("sql").is_none(), "driver API has no client/display SQL field");
    let metadata = &plan.jdbc.as_ref().unwrap().metadata;
    assert_eq!(normalize_value(column(metadata, "ID").unwrap(), &DbValue::Integer("+0009007199254740993".into())).unwrap(), DbValue::Integer("9007199254740993".into()));
    assert_eq!(normalize_value(column(metadata, "AMT").unwrap(), &DbValue::Decimal("1e2".into())).unwrap(), DbValue::Decimal("100.000000000000".into()));
    assert_eq!(normalize_value(column(metadata, "AMT").unwrap(), &DbValue::Decimal("0".into())).unwrap(), DbValue::Decimal("0.000000000000".into()));
    for (name, value) in baseline() { assert_eq!(normalize_value(column(metadata, &name).unwrap(), &value).unwrap(), value); }
}
#[test]
fn unsafe_integer_decimal_rounding_noncanonical_binary_and_unknown_null_are_rejected() {
    let caps = caps(); let metadata = metadata(&caps);
    for (name, value) in [("ID", DbValue::Int(9_007_199_254_740_992)), ("ID", DbValue::Integer("9223372036854775808".into())),
        ("AMT", DbValue::Decimal("1.0000000000001".into())), ("AMT", DbValue::Float(1.0)),
        ("AMT", DbValue::Decimal("123456789012345678901234567890123456789.0".into())), ("BIN", DbValue::Binary("AP8BgA".into()))] {
        assert_eq!(code(normalize_value(column(&metadata, name).unwrap(), &value).unwrap_err()), "MUTATION_VALUE_INVALID");
    }
    let mut unknown = col("JSON", 1111, "JSON", 100, 0); unknown.writable_type = false;
    assert_eq!(code(normalize_value(&unknown, &DbValue::Null).unwrap_err()), "UNSUPPORTED_MUTATION_TYPE");
    assert_eq!(code(normalize_value(&unknown, &DbValue::Json("{\"n\":9007199254740993}".into())).unwrap_err()), "UNSUPPORTED_MUTATION_TYPE");
    let real = col("REAL", 7, "REAL", 24, 0);
    assert!(normalize_value(&real, &DbValue::Float(0.1)).is_err());
    assert_eq!(normalize_value(&real, &DbValue::Float(1.5)).unwrap(), DbValue::Float(1.5));
}
#[test]
fn dates_timezone_and_java_minute_precision_are_lossless() {
    let caps = caps(); let metadata = metadata(&caps);
    assert_eq!(normalize_value(column(&metadata, "TM").unwrap(), &DbValue::Time("12:34:00".into())).unwrap(), DbValue::Time("12:34".into()));
    assert_eq!(normalize_value(column(&metadata, "TS").unwrap(), &DbValue::Timestamp("2026-11-09 12:34:00".into())).unwrap(), DbValue::Timestamp("2026-11-09T12:34".into()));
    assert_eq!(normalize_value(column(&metadata, "TZ").unwrap(), &DbValue::Timestamptz("2026-11-09T12:34:00+00:00".into())).unwrap(), DbValue::Timestamptz("2026-11-09T12:34Z".into()));
    for raw in ["2026-11-09T12:34Z", "2026-11-09T12:34+05:30", "2026-11-09T12:34-03:30"] {
        assert_eq!(normalize_value(column(&metadata, "TZ").unwrap(), &DbValue::Timestamptz(raw.into())).unwrap(), DbValue::Timestamptz(raw.into()),
            "real Java minute-only OffsetDateTime readback must remain canonical and lossless");
    }
    let mut milliseconds = column(&metadata, "TM").unwrap().clone(); milliseconds.scale = 3;
    assert!(normalize_value(&milliseconds, &DbValue::Time("12:34:56.123456789".into())).is_err());
    assert!(normalize_value(column(&metadata, "D").unwrap(), &DbValue::Date("2026-02-30".into())).is_err());
    assert!(normalize_value(column(&metadata, "TZ").unwrap(), &DbValue::Timestamptz("2026-11-09T12:34:00".into())).is_err());
    assert!(normalize_value(column(&metadata, "TZ").unwrap(), &DbValue::Timestamptz("2026-11-09T12:34:00+19:00".into())).is_err());
}
#[test]
fn old_changed_values_full_delete_baseline_and_complete_real_keys_are_required() {
    let mut draft = draft(); draft.original_values.clear();
    assert_eq!(code(build_preview(&draft, caps(), metadata(&caps()), "window1", "endpoint1".into()).unwrap_err()), "MUTATION_METADATA_UNKNOWN",
        "metadata from another physical UUID cannot pass before checking old values");
    let caps = caps(); let metadata = metadata(&caps);
    assert_eq!(code(build_preview(&draft, caps.clone(), metadata.clone(), "window1", "endpoint1".into()).unwrap_err()), "MUTATION_BASELINE_REQUIRED");
    draft.kind = "delete".into(); draft.changes.clear();
    assert_eq!(code(build_preview(&draft, caps.clone(), metadata.clone(), "window1", "endpoint1".into()).unwrap_err()), "MUTATION_BASELINE_REQUIRED");
    draft.original_values = baseline(); assert!(build_preview(&draft, caps.clone(), metadata.clone(), "window1", "endpoint1".into()).is_ok());
    draft.keys[0].1 = DbValue::Null; draft.original_values.clear();
    assert_eq!(code(build_preview(&draft, caps.clone(), metadata.clone(), "window1", "endpoint1".into()).unwrap_err()), "MUTATION_MISSING_UNIQUE_KEY");
    let mut no_keys = metadata.clone(); no_keys.primary_key.clear(); no_keys.unique_keys.clear();
    draft.keys = vec![("ID".into(), DbValue::Integer("1".into()))];
    assert_eq!(code(build_preview(&draft, caps, no_keys, "window1", "endpoint1".into()).unwrap_err()), "MUTATION_MISSING_UNIQUE_KEY");
}
#[test]
fn real_generated_identity_readonly_flags_and_unknown_sections_are_not_guessed() {
    let caps = caps(); let metadata = metadata(&caps);
    for name in ["GEN", "ID"] {
        let mut metadata = metadata.clone(); if name == "ID" { metadata.columns[0].auto_increment = Some(true); }
        let mut draft = draft(); draft.changes = vec![(name.into(), DbValue::Integer("2".into()))];
        assert_eq!(code(build_preview(&draft, caps.clone(), metadata, "window1", "endpoint1".into()).unwrap_err()), "READ_ONLY_COLUMN");
    }
    let mut unknown = metadata.clone(); unknown.columns[1].generated = None;
    assert_eq!(code(build_preview(&draft(), caps.clone(), unknown, "window1", "endpoint1".into()).unwrap_err()), "READ_ONLY_COLUMN");
    let mut unknown = metadata.clone(); unknown.completeness.insert("constraints".into(), "unknown".into());
    assert_eq!(code(build_preview(&draft(), caps.clone(), unknown, "window1", "endpoint1".into()).unwrap_err()), "MUTATION_METADATA_UNKNOWN");
    let mut view = metadata; view.table_type = "VIEW".into();
    assert_eq!(code(build_preview(&draft(), caps, view, "window1", "endpoint1".into()).unwrap_err()), "UNSUPPORTED_MUTATION");
}
#[test]
fn null_auto_identity_is_omitted_but_null_and_empty_remain_distinct() {
    let caps = caps(); let mut metadata = metadata(&caps); metadata.columns[0].auto_increment = Some(true);
    let mut draft = draft(); draft.kind = "insert".into(); draft.keys.clear(); draft.original_values.clear();
    draft.changes = vec![("ID".into(), DbValue::Null), ("NIL".into(), DbValue::Null), ("PAY".into(), DbValue::Text(String::new()))];
    let plan = build_preview(&draft, caps, metadata, "window1", "endpoint1".into()).unwrap();
    assert!(!plan.changes.iter().any(|(name, _)| name == "ID"));
    assert_eq!(plan.changes, [("NIL".into(), DbValue::Null), ("PAY".into(), DbValue::Text(String::new()))]);
    assert!(!plan.sql.contains("\"ID\"")); assert!(plan.jdbc.is_some());
}
#[test]
fn same_registry_owner_once_and_snapshot_bytes_remain_authoritative() {
    let registry = super::super::Registry::default();
    let plan = build(&draft()); let bytes = serde_json::to_vec(&plan).unwrap().len();
    let id = registry.put(plan).unwrap();
    assert!(registry.take(&id, "wrong-window", "page1").is_err());
    assert!(registry.take(&id, "window1", "wrong-page").is_err());
    let plan = registry.take(&id, "window1", "page1").unwrap(); assert!(plan.jdbc.is_some());
    assert!(registry.take(&id, "window1", "page1").is_err());
    assert!(bytes > serde_json::to_vec(&plan.changes).unwrap().len(), "full capability/metadata must consume shared budget");
    let mut huge = plan; huge.jdbc.as_mut().unwrap().metadata.definition = Some("x".repeat(super::super::PLAN_LIMIT));
    assert!(registry.put(huge).is_err(), "cannot hide large metadata outside the shared plan byte limit");
}

#[derive(Clone, Copy)]
enum Fault { None, PreviewMetadata, PreviewSession, TemporaryNamespace, Transport, Unknown, NullCount,
    TooMany, Unverified, Poisoned, WrongReadback, BadGenerated, KnownConflict }
struct Fake {
    caps: JdbcCapabilities, metadata: JdbcMetadataSnapshot, fault: Fault,
    caps_calls: AtomicUsize, metadata_calls: AtomicUsize, apply_calls: Arc<AtomicUsize>,
}
#[async_trait::async_trait]
impl HttpAdapter for Fake {
    async fn ping(&self) -> Result<String> { Ok("contract fixture only".into()) }
    async fn containers(&self) -> Result<Vec<(String, String)>> { Ok(vec![]) }
    async fn objects(&self, _: &str) -> Result<Vec<String>> { Ok(vec![]) }
    async fn rows(&self, _: &str, _: &str, _: u64, _: u64) -> Result<HttpRows> { Ok(HttpRows { columns: vec![], rows: vec![] }) }
    async fn run(&self, _: &str, _: &str, _: &str) -> Result<HttpRows> { panic!("C2 bridge cannot fall back to text SQL"); }
    async fn relational_capabilities(&self) -> Result<JdbcCapabilities> {
        let mut caps = self.caps.clone(); let call = self.caps_calls.fetch_add(1, Ordering::SeqCst);
        if call > 0 { match self.fault { Fault::PreviewSession => caps.session_id = uuid::Uuid::new_v4().to_string(),
            Fault::TemporaryNamespace => caps.schema = "OTHER_PAGE".into(), _ => {} } }
        Ok(caps)
    }
    async fn relational_metadata(&self, _: &JdbcObjectId) -> Result<JdbcMetadataSnapshot> {
        let mut metadata = self.metadata.clone();
        if self.metadata_calls.fetch_add(1, Ordering::SeqCst) > 0 && matches!(self.fault, Fault::PreviewMetadata) { metadata.revision = "b".repeat(64); }
        Ok(metadata)
    }
    async fn apply_row_plan(&self, plan: &JdbcRowPlan) -> Result<JdbcMutationOutcome> {
        self.apply_calls.fetch_add(1, Ordering::SeqCst);
        if matches!(self.fault, Fault::Transport) { return Err(anyhow!("reply lost after send")); }
        let mut outcome = JdbcMutationOutcome { operation_id: plan.operation_id.clone(), status: "applied".into(), affected_rows: Some("1".into()),
            matched_rows: Some("1".into()), verified: true, transaction: "committed".into(), connection_poisoned: false, mismatches: vec![],
            readback: overwrite(&baseline(), &plan.changes), generated_keys: vec![] };
        match self.fault {
            Fault::Unknown => { outcome.status = "unknown".into(); outcome.affected_rows = None; outcome.verified = false;
                outcome.transaction = "unknown".into(); outcome.connection_poisoned = true; outcome.readback.clear(); outcome.mismatches.push("commit ack missing".into()); }
            Fault::NullCount => outcome.affected_rows = None,
            Fault::TooMany => outcome.affected_rows = Some("2".into()),
            Fault::Unverified => outcome.verified = false,
            Fault::Poisoned => outcome.connection_poisoned = true,
            Fault::WrongReadback => outcome.readback[1].1 = DbValue::Text("trigger-modified".into()),
            Fault::BadGenerated => outcome.generated_keys = vec![("ID".into(), DbValue::Integer("999".into()))],
            Fault::KnownConflict => { outcome.status = "conflict".into(); outcome.affected_rows = Some("0".into()); outcome.matched_rows = Some("0".into());
                outcome.verified = false; outcome.transaction = "rolled_back".into(); outcome.readback.clear(); outcome.mismatches.push("CONFLICT: old value mismatch".into()); }
            _ => {}
        }
        Ok(outcome)
    }
}
async fn fake(fault: Fault) -> (HttpDb, Arc<AtomicUsize>) {
    let caps = caps(); let metadata = metadata(&caps); let apply = Arc::new(AtomicUsize::new(0));
    let request = ConnectionRequest { kind: DbKind::of("h2"), host: String::new(), port: 0, user: String::new(), password: String::new(),
        database: "identity-not-schema".into(), file: String::new(), params: String::new(), timeout_secs: 10 };
    (HttpDb::from_adapter(&request, Box::new(Fake { caps, metadata, fault, caps_calls: AtomicUsize::new(0),
        metadata_calls: AtomicUsize::new(0), apply_calls: apply.clone() })).await.unwrap(), apply)
}
#[tokio::test]
async fn preview_metadata_and_physical_session_races_never_register_or_send() {
    for fault in [Fault::PreviewMetadata, Fault::PreviewSession] {
        let (http, count) = fake(fault).await;
        assert_eq!(code(preview(&http, &draft(), "window1", "endpoint1".into()).await.unwrap_err()), "MUTATION_CONTEXT_CHANGED");
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
    let (http, count) = fake(Fault::TemporaryNamespace).await;
    let plan = preview(&http, &draft(), "window1", "endpoint1".into()).await.unwrap();
    assert_eq!(plan.jdbc.unwrap().metadata.object.schema, "PUBLIC"); assert_eq!(count.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn actual_saved_readonly_and_invalid_budget_refuse_before_adapter_apply() {
    let (http, count) = fake(Fault::None).await;
    let plan = preview(&http, &draft(), "window1", "endpoint1".into()).await.unwrap();
    validate_context(&http, &plan).await.unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(code(execute(&http, &id, &plan, true, 10).await.unwrap_err()), "READ_ONLY_VIOLATION");
    assert_eq!(code(execute(&http, &id, &plan, false, 0).await.unwrap_err()), "UNSUPPORTED_MUTATION");
    assert_eq!(code(execute(&http, &id, &plan, false, 301).await.unwrap_err()), "UNSUPPORTED_MUTATION");
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn post_send_transport_unknown_poison_null_counts_and_bad_evidence_never_become_zero_or_retry() {
    for fault in [Fault::Transport, Fault::Unknown, Fault::NullCount, Fault::TooMany, Fault::Unverified,
        Fault::Poisoned, Fault::WrongReadback, Fault::BadGenerated] {
        let (http, count) = fake(fault).await;
        let plan = preview(&http, &draft(), "window1", "endpoint1".into()).await.unwrap();
        validate_context(&http, &plan).await.unwrap(); let id = uuid::Uuid::new_v4().to_string();
        let error = AppError::from(execute(&http, &id, &plan, false, 10).await.unwrap_err());
        assert_eq!(error.code, "MUTATION_EFFECTS_UNKNOWN"); assert_eq!(error.operation_id.as_deref(), Some(id.as_str()));
        assert!(!error.retryable); assert_eq!(count.load(Ordering::SeqCst), 1, "no fallback execute API or automatic retry");
        assert!(serde_json::to_value(error).unwrap().get("affected").is_none(), "unknown cannot be forged into a zero-count receipt");
    }
}
#[tokio::test]
async fn acknowledged_commit_and_known_rollback_map_only_verified_evidence() {
    for fault in [Fault::None, Fault::KnownConflict] {
        let (http, count) = fake(fault).await; let plan = preview(&http, &draft(), "window1", "endpoint1".into()).await.unwrap();
        validate_context(&http, &plan).await.unwrap();
        let result = execute(&http, &uuid::Uuid::new_v4().to_string(), &plan, false, 10).await.unwrap();
        if matches!(fault, Fault::None) { assert_eq!(result.status, "applied"); assert!(result.verified); assert_eq!(result.affected, 1); assert_eq!(result.transaction, "committed"); }
        else { assert_eq!(result.status, "conflict"); assert!(!result.verified); assert_eq!(result.affected, 0); assert_eq!(result.transaction, "rolled_back"); }
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn mode_product_driver_and_application_kind_names_cannot_certify_writes() {
    for changed in ["mode", "product", "driver", "adapter"] {
        let mut caps = caps();
        match changed { "mode" => { caps.row_write = false; caps.metadata = false; caps.reason = Some("compatibility mode unverified".into()); },
            "product" => caps.product = "ordinary REST or unverified warehouse".into(), "driver" => caps.driver_version = "9.9.9".into(),
            "adapter" => caps.adapter_id = "elasticsearch".into(), _ => unreachable!() }
        assert_eq!(code(capabilities_valid(&caps).unwrap_err()), "UNSUPPORTED_MUTATION");
    }
}
