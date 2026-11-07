//! Test-only shared lifecycle/reporting for actual synchronization simulations.
//! All credentials/files are synthetic and owned by this test. Never read data/.
use anyhow::{bail, ensure, Context, Result};
use futures::FutureExt;
use navkit_lib::{db::{self, Db}, models::{ConnectionRequest, DbKind, Protocol}, sync::{SyncDiffRequest, SyncOptions}, transfer};
use serde_json::{json, Value};
use std::{future::Future, path::{Path, PathBuf}, process::{Child, Command, Stdio}, sync::Arc, time::{Duration, Instant}};

#[derive(Clone)]
pub struct Endpoint {
    pub db: Arc<Db>,
    pub kind: DbKind,
    pub namespace: String,
}

pub struct Report {
    pub path: PathBuf,
    pub doc: Value,
}
impl Report {
    pub fn new(protocol: &str, tag: &str) -> Result<Self> {
        let dir = std::env::var_os("NAVKIT_SYNC_SIM_REPORT_DIR").map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join(format!("navkit-sync-sim-reports-{}", uuid::Uuid::new_v4().simple())));
        ensure!(dir.is_absolute(), "NAVKIT_SYNC_SIM_REPORT_DIR must be absolute");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{protocol}-{tag}.json"));
        use sha2::Digest;
        let executable = std::env::current_exe()?;
        let executable_hash = format!("{:x}", sha2::Sha256::digest(std::fs::read(&executable)?));
        let report = Self { path, doc: json!({
            "schema_version": 1, "protocol": protocol, "tag": tag,
            "executed_binary": {"path": executable, "sha256": executable_hash},
            "evidence_type": "ACTUAL_NAVKIT_SYNC_WITH_STATEFUL_PROTOCOL_SIMULATION",
            "status": "RUNNING", "vendor_server_status": "NOT_TESTED", "vendor_real_pass": 0,
            "limitations": ["Not a vendor server or certification", "Finite documented SQL/wire subset with actual mutable backend state", "No user connection secrets or data/ files are used"],
            "cases": [], "cleanup": {"success": false, "status": "NOT_RUN"}
        }) };
        report.persist()?;
        println!("sync simulation checkpoint: {}", report.path.display());
        Ok(report)
    }
    pub fn persist(&self) -> Result<()> {
        std::fs::write(&self.path, serde_json::to_vec_pretty(&self.doc)?)?;
        Ok(())
    }
    pub fn fixture(&mut self, root: &Path) -> Result<()> {
        self.doc["fixture_root"] = json!(root);
        self.persist()
    }
    pub async fn case<F>(&mut self, name: &str, future: F) -> Result<bool>
    where F: Future<Output = Result<Value>> {
        let cases = self.doc["cases"].as_array_mut().context("case array")?;
        let index = cases.len();
        cases.push(json!({"name": name, "status": "RUNNING", "vendor_server_status": "NOT_TESTED"}));
        self.persist()?;
        let start = Instant::now();
        let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(Duration::from_secs(30), future)).catch_unwind().await;
        let (passed, evidence, error) = match outcome {
            Ok(Ok(Ok(evidence))) => (true, evidence, None),
            Ok(Ok(Err(error))) => (false, Value::Null, Some(format!("{error:#}"))),
            Ok(Err(_)) => (false, Value::Null, Some("30 second case deadline; an unfinished native operation is not a PASS".into())),
            Err(payload) => (false, Value::Null, Some(panic_text(payload))),
        };
        self.doc["cases"][index] = json!({"name": name, "status": if passed {"SIMULATED_SYNC_PASS"} else {"SIMULATED_SYNC_FAIL"}, "elapsed_ms": start.elapsed().as_millis(), "evidence": evidence, "error": error, "vendor_server_status": "NOT_TESTED"});
        self.persist()?;
        Ok(passed)
    }
    pub fn fatal(&mut self, name: &str, error: String) {
        if let Some(cases) = self.doc["cases"].as_array_mut() {
            cases.push(json!({"name": name, "status": "SIMULATED_SYNC_FAIL", "error": error}));
        }
    }
    pub fn finish(&mut self, cleanup: Value, wire: Value) -> Result<()> {
        self.doc["cleanup"] = cleanup;
        self.doc["wire"] = wire;
        let cases = self.doc["cases"].as_array().context("case array")?;
        let pass = cases.iter().filter(|c| c["status"] == "SIMULATED_SYNC_PASS").count();
        let fail = cases.len() - pass;
        let clean = self.doc["cleanup"]["success"] == true;
        self.doc["counts"] = json!({"pass": pass, "fail": fail, "vendor_real_pass": 0});
        self.doc["status"] = json!(if fail == 0 && clean {"SIMULATED_SYNC_PASS"} else {"SIMULATED_SYNC_FAIL"});
        self.persist()?;
        ensure!(fail == 0 && clean, "simulation failures={fail}, cleanup={clean}; report={}", self.path.display());
        Ok(())
    }
}

pub fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() { format!("panic: {s}") }
    else if let Some(s) = payload.downcast_ref::<&str>() { format!("panic: {s}") }
    else { "panic with non-string payload".into() }
}

struct Server {
    child: Child,
    directory: PathBuf,
    protocol: String,
}
impl Drop for Server {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) { let _ = self.child.kill(); }
        let _ = self.child.wait();
    }
}

pub struct World {
    pub root: PathBuf,
    connections: Vec<Arc<Db>>,
    servers: Vec<Server>,
}
impl World {
    pub fn new() -> Result<Self> {
        let root = std::env::temp_dir().join(format!("navkit-sync-sim-{}", uuid::Uuid::new_v4().simple()));
        ensure!(root.is_absolute(), "owned absolute fixture path required");
        std::fs::create_dir(&root)?;
        Ok(Self {root, connections: Vec::new(), servers: Vec::new()})
    }
    pub async fn connect(&mut self, req: ConnectionRequest, namespace: &str) -> Result<Endpoint> {
        let connection = Arc::new(db::open(&req).await?);
        if req.kind.protocol() == Protocol::Odbc { ensure!(matches!(connection.as_ref(), Db::Odbc(_)), "test-only ODBC request escaped to JDBC/native route"); }
        self.connections.push(connection.clone());
        Ok(Endpoint {db: connection, kind: req.kind, namespace: namespace.to_string()})
    }
    pub async fn sqlite(&mut self) -> Result<Endpoint> {
        let file = self.root.join(format!("sqlite-{}.db", uuid::Uuid::new_v4().simple()));
        let req = ConnectionRequest {kind: DbKind::of("sqlite"), host: String::new(), port: 0, user: String::new(), password: String::new(), database: String::new(), file: file.to_string_lossy().into(), params: String::new(), timeout_secs: 5};
        self.connect(req, "main").await
    }
    pub async fn start_server(&mut self, protocol: &str) -> Result<ConnectionRequest> {
        let token = uuid::Uuid::new_v4().simple().to_string();
        let directory = self.root.join(format!("navkit-sync-sim-{protocol}-{token}"));
        std::fs::create_dir(&directory)?;
        let user = format!("user_{token}");
        let password = format!("fixture-{token}");
        let database = format!("DB{token}");
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let script = manifest.join("tests/support/sync_simulated").join(if protocol == "tds" {"tds_sync_server.py"} else {"clickhouse_sync_server.py"});
        let mut command = Command::new(std::env::var_os("NAVKIT_PYTHON").unwrap_or_else(|| "python".into()));
        command.arg(&script).arg(&directory)
            .env("NAVKIT_TDS_USER", &user).env("NAVKIT_TDS_PASSWORD", &password).env("NAVKIT_TDS_DATABASE", &database)
            .env("NAVKIT_CH_USER", &user).env("NAVKIT_CH_PASSWORD", &password).env("NAVKIT_CH_DATABASE", &database)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::from(std::fs::File::create(directory.join("stderr.log"))?));
        if protocol == "tds" { command.arg(manifest.join("../tools/acceptance/simulation/native/tds_server.py")); }
        #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x08000000); }
        let child = command.spawn().with_context(|| format!("start owned {protocol} loopback fixture"))?;
        let mut server = Server {child, directory, protocol: protocol.into()};
        let deadline = Instant::now() + Duration::from_secs(20);
        let port = loop {
            if let Ok(bytes) = std::fs::read(server.directory.join("ready.json")) {
                if let Ok(ready) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some(port) = ready["port"].as_u64() { break u16::try_from(port)?; }
                }
            }
            if let Some(status) = server.child.try_wait()? {
                bail!("{protocol} fixture exited {status}: {}", std::fs::read_to_string(server.directory.join("stderr.log")).unwrap_or_default());
            }
            ensure!(Instant::now() < deadline, "{protocol} fixture readiness deadline");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        self.servers.push(server);
        Ok(ConnectionRequest {kind: DbKind::of(if protocol == "tds" {"sqlserver"} else {"clickhouse"}), host: "127.0.0.1".into(), port, user, password, database, file: String::new(), params: if protocol == "tds" {"encrypt=false".into()} else {String::new()}, timeout_secs: 5})
    }
    pub async fn odbc(&mut self, kind: &str) -> Result<Endpoint> {
        ensure!(cfg!(windows), "ODBC simulation requires actual Windows ODBC manager");
        let driver = PathBuf::from(std::env::var_os("NAVKIT_SIM_ODBC_DRIVER").context("set NAVKIT_SIM_ODBC_DRIVER to existing test-only facade DLL")?);
        let backend = PathBuf::from(std::env::var_os("NAVKIT_SIM_ODBC_BACKEND").context("set NAVKIT_SIM_ODBC_BACKEND to existing complete DuckDB DLL")?);
        ensure!(driver.is_absolute() && backend.is_absolute() && driver.is_file() && backend.is_file(), "absolute existing simulation DLLs required");
        let kind = DbKind::of(kind);
        ensure!(kind.meta().is_some() && kind.protocol() == Protocol::Odbc, "ODBC catalogue kind required");
        let file = self.root.join(format!("odbc-{}.duckdb", uuid::Uuid::new_v4().simple()));
        let password = "contract;secret}中文";
        let q = navkit_lib::escape_odbc_connection_value;
        let req = ConnectionRequest {kind: kind.clone(), host: String::new(), port: 0, user: "navkit_sim".into(), password: password.into(), database: file.to_string_lossy().into(), file: file.to_string_lossy().into(), params: format!("Driver={};Database={};NavkitSimKind={};UID=navkit_sim;PWD={};", q(&driver.to_string_lossy()), q(&file.to_string_lossy()), kind.id(), q(password)), timeout_secs: 5};
        let ep = self.connect(req, "main").await?;
        execute(&ep, "SET schema = 'main'").await?;
        Ok(ep)
    }
    pub fn hook(&self, table: &str, ordinal: u64, action: &str) -> Result<PathBuf> {
        let server = self.servers.iter().find(|s| s.protocol == "tds").context("TDS fixture")?;
        std::fs::write(server.directory.join(format!("hook-{table}.json")), serde_json::to_vec(&json!({"ordinal": ordinal, "action": action}))?)?;
        Ok(server.directory.clone())
    }
    pub async fn cleanup(&mut self) -> (Value, Value) {
        let mut errors = Vec::new();
        for connection in &self.connections {
            let close = async { match connection.as_ref() {Db::Sqlite(p) => p.close().await, Db::Mysql(p) => p.close().await, Db::Postgres(p) => p.close().await, _ => {}} };
            if tokio::time::timeout(Duration::from_secs(5), close).await.is_err() { errors.push("connection pool close deadline".to_string()); }
        }
        self.connections.clear();
        let mut wire = Vec::new();
        for server in &mut self.servers {
            let running = matches!(server.child.try_wait(), Ok(None));
            if running { if let Err(e) = server.child.kill() { errors.push(format!("kill owned child: {e}")); } }
            if let Err(e) = server.child.wait() { errors.push(format!("reap owned child: {e}")); }
            wire.push(json!({"protocol": server.protocol, "wire": std::fs::read_to_string(server.directory.join("wire.jsonl")).unwrap_or_default(), "stderr": std::fs::read_to_string(server.directory.join("stderr.log")).unwrap_or_default()}));
        }
        self.servers.clear();
        let actual = self.root.canonicalize();
        match actual {
            Ok(actual) if actual.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("navkit-sync-sim-")) => {
                let deadline = Instant::now() + Duration::from_secs(3);
                loop {
                    match std::fs::remove_dir_all(&actual) {
                        Ok(()) => break,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                        Err(e) if (e.kind() == std::io::ErrorKind::PermissionDenied || matches!(e.raw_os_error(), Some(32 | 33))) && Instant::now() < deadline => tokio::time::sleep(Duration::from_millis(25)).await,
                        Err(e) => { errors.push(format!("remove exact owned fixture: {e}")); break; }
                    }
                }
            }
            Ok(_) => errors.push("refusing cleanup outside exact owned fixture".into()),
            Err(e) => errors.push(format!("canonicalize owned fixture before deletion: {e}")),
        }
        (json!({"success": errors.is_empty() && !self.root.exists(), "root": self.root, "errors": errors, "scope": "explicit pool close; owned child kill/reap; only UUID fixture directory"}), json!(wire))
    }
}

pub async fn execute(ep: &Endpoint, sql: &str) -> Result<u64> {
    match ep.db.as_ref() {
        Db::Sqlite(p) => Ok(sqlx::query(sql).execute(p).await?.rows_affected()),
        Db::Mysql(p) => Ok(sqlx::query(sql).execute(p).await?.rows_affected()),
        Db::Postgres(p) => Ok(sqlx::query(sql).execute(p).await?.rows_affected()),
        other => other.generic_exec(sql).await,
    }
}
pub fn full(ep: &Endpoint, table: &str) -> String { db::qualified(&ep.kind, &ep.namespace, table) }
pub fn literal(ep: &Endpoint, text: &str) -> String {
    let quoted = db::quote_literal(text);
    if ep.kind.protocol() == Protocol::Mssql { format!("N{quoted}") } else { quoted }
}
pub fn table() -> String { format!("T{}", uuid::Uuid::new_v4().simple()).to_ascii_uppercase() }
pub fn options(source: &Endpoint, source_table: &str, target: &Endpoint, target_table: &str) -> SyncOptions {
    SyncOptions {task_id: Some(uuid::Uuid::new_v4().to_string()), source_connection: "owned-source".into(), source_schema: source.namespace.clone(), source_table: source_table.into(), target_connection: "owned-target".into(), target_schema: target.namespace.clone(), target_table: target_table.into(), mode: "both".into(), write_mode: "append".into(), sync_mode: "full".into(), incremental_update: false, create_target: true, columns: Vec::new(), where_clause: None}
}
pub fn diff_request(opts: &SyncOptions) -> SyncDiffRequest {
    SyncDiffRequest {source_connection: opts.source_connection.clone(), source_schema: opts.source_schema.clone(), source_table: opts.source_table.clone(), target_connection: opts.target_connection.clone(), target_schema: opts.target_schema.clone(), target_table: opts.target_table.clone(), where_clause: opts.where_clause.clone(), columns: opts.columns.clone()}
}
pub async fn read(ep: &Endpoint, table: &str) -> Result<Vec<Vec<Option<String>>>> {
    let (_, mut rows) = transfer::fetch_transfer_rows(ep.db.as_ref(), &ep.kind, &ep.namespace, table, None, None).await?;
    rows.sort();
    Ok(rows)
}
pub async fn seed(ep: &Endpoint, table: &str, nullable: bool) -> Result<Vec<Vec<Option<String>>>> {
    let full = full(ep, table);
    let null = if nullable {"NULL"} else {"'present'"};
    let ddl = match ep.kind.protocol() {
        Protocol::Mssql => format!("CREATE TABLE {full} ([ID] BIGINT NOT NULL PRIMARY KEY,[PAYLOAD] NVARCHAR(MAX) NOT NULL,[OPTIONAL] NVARCHAR(MAX))"),
        Protocol::Clickhouse => format!("CREATE TABLE {full} (`ID` Int64 NOT NULL,`PAYLOAD` String,`OPTIONAL` {}) ENGINE = MergeTree ORDER BY (ID)", if nullable {"Nullable(String)"} else {"String"}),
        _ => format!("CREATE TABLE {full} (\"ID\" INTEGER NOT NULL PRIMARY KEY,\"PAYLOAD\" VARCHAR(1000) NOT NULL,\"OPTIONAL\" VARCHAR(100))"),
    };
    execute(ep, &ddl).await?;
    let sentinel = format!("{}:中文;O'Reilly\\尾🙂", uuid::Uuid::new_v4());
    execute(ep, &format!("INSERT INTO {full} VALUES (1,{},{}),(2,{},'present')", literal(ep, &sentinel), null, literal(ep, "second"))).await?;
    read(ep, table).await
}
pub async fn task_cancel_at_hook(directory: &Path, table: &str, task: &str) -> Result<()> {
    let entered = directory.join(format!("entered-{table}"));
    let release = directory.join(format!("release-{table}"));
    let deadline = Instant::now() + Duration::from_secs(8);
    while !entered.is_file() {
        ensure!(Instant::now() < deadline, "deterministic INSERT hook was not reached");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let cancelled = transfer::cancel_task(task);
    // Always release a reached hook, even if cancellation unexpectedly fails.
    std::fs::write(release, "release")?;
    cancelled?;
    Ok(())
}
