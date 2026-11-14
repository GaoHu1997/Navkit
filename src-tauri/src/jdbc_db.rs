//! 通用 JDBC sidecar：用 Java 11+ 的 source-file launcher 加载用户提供的 .jar 驱动。
//!
//! 与 `h2_db` 同构：私有进程 + stdin/stdout 上的 tab/base64 协议，因此密码不会出现在
//! 进程命令行里。一个 sidecar 承载一条连接，Drop 时终止进程并清理临时源码目录。
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use tokio::{
    io::BufReader,
    process::{Child, ChildStdin, ChildStdout},
    sync::Mutex,
};

use crate::{
    http_db::{relational, HttpAdapter, HttpColumn, HttpRows},
    models::ConnectionRequest,
};

#[path = "jdbc_runtime_options.rs"]
mod runtime_options;

pub fn java_path() -> PathBuf {
    if let Some(java) = std::env::var_os("NAVKIT_JAVA") {
        return java.into();
    }
    // 一键安装的托管 JDK 优先于系统 Java：版本已知可满足 source launcher。
    if let Some(java) = crate::java_runtime::managed_java() {
        return java;
    }
    if let Some(home) = std::env::var_os("JAVA_HOME") {
        return PathBuf::from(home).join("bin").join(if cfg!(windows) {
            "java.exe"
        } else {
            "java"
        });
    }
    PathBuf::from("java")
}

fn bounded_java(java: &Path, args: &[&std::ffi::OsStr]) -> Result<std::process::Output> {
    let mut child = crate::background_process::command(java)
        .arg("-Dfile.encoding=UTF-8")
        .args(args)
        .env_remove("BIGQUERY_JDBC_LOG_LEVEL")
        .env_remove("BIGQUERY_JDBC_LOG_PATH")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("无法运行 Java；JDBC 需要 Java 11+，请配置 JAVA_HOME 或 NAVKIT_JAVA")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Java 运行时探针 15 秒超时，进程已终止");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 校验 JDBC 运行时：jar 存在且 Java 主版本 >= 11（source-file launcher 的最低要求）。
pub fn runtime_status(jar: &Path) -> Result<String> {
    if !jar.is_file() {
        bail!("JDBC 驱动 JAR 不存在：{}", jar.display());
    }
    let output = bounded_java(&java_path(), &[std::ffi::OsStr::new("-version")])?;
    if !output.status.success() {
        bail!("Java -version 失败，JDBC 需要可用的 Java 11+");
    }
    let version = String::from_utf8_lossy(&output.stderr).into_owned();
    let major = version
        .split('"')
        .nth(1)
        .and_then(|s| s.split('.').next())
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    if major < 11 {
        bail!("JDBC source launcher 需要 Java 11+：{}", version.trim());
    }
    Ok(version.trim().to_string())
}

/// Offline readiness, not database availability: run the real source launcher, initialise and
/// instantiate this exact JDBC driver and verify its SPI/URL/property contract with no network.
/// 成功结果经 probe_cache 缓存（键含 java/jar 路径+大小+mtime 与驱动类/URL）：
/// 驱动检测每次打开「新建连接」都会调用，缓存后不再重复启动 java。
pub fn driver_runtime_status(jar: &Path, driver_class: &str, probe_url: &str) -> Result<String> {
    driver_runtime_status_with_dependencies(jar, driver_class, probe_url, &[])
}

pub(crate) fn driver_runtime_status_with_dependencies(jar: &Path, driver_class: &str, probe_url: &str, dependencies: &[PathBuf]) -> Result<String> {
    if !jar.is_file() {
        bail!("JDBC 驱动 JAR 不存在：{}", jar.display());
    }
    if driver_class.trim().is_empty() || !probe_url.starts_with("jdbc:") {
        bail!("JDBC 驱动探针需要明确的产品驱动类与 JDBC URL");
    }
    let mut key = format!("jdbc:{driver_class}:{probe_url}");
    for dep in dependencies {
        let metadata = std::fs::metadata(dep).with_context(|| format!("JDBC 运行依赖不存在：{}", dep.display()))?;
        if !metadata.is_file() { bail!("JDBC 依赖不是普通文件"); }
        key.push_str(&format!("|{}:{}:{:?}", dep.display(), metadata.len(), metadata.modified()?));
    }
    let classpath = std::env::join_paths(std::iter::once(jar.to_path_buf()).chain(dependencies.iter().cloned()))
        .context("JDBC classpath 路径无效")?;
    crate::probe_cache::cached(&java_path(), jar, &key, || {
        driver_runtime_probe(jar, driver_class, probe_url, &classpath)
    })
}

fn driver_runtime_probe(jar: &Path, driver_class: &str, probe_url: &str, classpath: &std::ffi::OsStr) -> Result<String> {
    let java_version = runtime_status(jar)?;
    let options = runtime_options::for_driver(driver_class, &java_version)?;
    struct SourceDir(PathBuf);
    impl Drop for SourceDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir =
        SourceDir(std::env::temp_dir().join(format!("navkit-jdbc-probe-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&dir.0)?;
    let source = dir.0.join("NavkitJdbc.java");
    std::fs::write(&source, relational::source_with_support(include_str!("NavkitJdbc.java")))?;
    let mut args: Vec<&std::ffi::OsStr> = options.iter().map(std::ffi::OsStr::new).collect();
    args.extend([
        std::ffi::OsStr::new("--class-path"), classpath, source.as_os_str(),
        std::ffi::OsStr::new("--probe"), std::ffi::OsStr::new(driver_class), std::ffi::OsStr::new(probe_url),
    ]);
    let output = bounded_java(&java_path(), &args)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = format!("NAVKIT_JDBC_READY\t{driver_class}\t");
    if !output.status.success() || !stdout.lines().any(|line| line.starts_with(&expected)) {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(2000)
            .collect();
        bail!("JDBC 驱动实际加载失败（需 Java 11+、jdk.compiler 和完整兼容依赖；不降级 Java）：{detail}");
    }
    Ok(stdout.trim().to_string())
}

struct Process {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    source_dir: PathBuf,
    /// 异常退出的有界 stderr 尾部；正常运行时持续排空，避免管道填满阻塞 JVM。
    stderr_tail: Arc<StdMutex<VecDeque<u8>>>,
}
impl Drop for Process {
    fn drop(&mut self) {
        // EOF closes the real JDBC connection and flushes committed file data.
        self.input.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut exited = false;
        while std::time::Instant::now() < deadline {
            match self.child.try_wait() {
                Ok(Some(_)) => { exited = true; break; }
                Err(_) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        if !exited {
            let _ = self.child.start_kill();
            let deadline = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < deadline {
                if !matches!(self.child.try_wait(), Ok(None)) { break; }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        report_sidecar_exit("JDBC", &mut self.child, &self.stderr_tail);
        if self.source_dir.is_absolute()
            && self.source_dir.parent().and_then(Path::file_name).and_then(|s| s.to_str()) == Some("jdbc-sidecar")
            && self.source_dir.file_name().and_then(|s| s.to_str()).is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()) {
            let _ = std::fs::remove_dir_all(&self.source_dir);
        }
    }
}

/// 子进程异常退出必须在日志中留下退出码和有界 stderr 尾部，才能区分 OOM、
/// 驱动崩溃和协议错误；正常请求结束时也有排空任务，不会阻塞 JVM。
pub(crate) fn report_sidecar_exit(protocol: &str, child: &mut Child, tail: &Arc<StdMutex<VecDeque<u8>>>) {
    let status = child.try_wait().ok().flatten();
    let abnormal = status.as_ref().is_some_and(|status| !status.success());
    if !abnormal { return; }
    let bytes: Vec<u8> = tail.lock().map(|tail| tail.iter().copied().collect()).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let text: String = text.chars().rev().take(4000).collect::<String>().chars().rev().collect();
    crate::storage::log_line(
        "error",
        &format!("{protocol} sidecar 异常退出（{:?}）：{}", status, text.trim()),
    );
}

/// 持续排空 stderr，只保留末尾 16 KiB；必须有读者，否则 JVM 报错时管道填满会死锁。
pub(crate) fn drain_sidecar_stderr(stderr: tokio::process::ChildStderr, tail: Arc<StdMutex<VecDeque<u8>>>) {
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut stderr = stderr;
        let mut buffer = [0u8; 4096];
        loop {
            match stderr.read(&mut buffer).await {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if let Ok(mut tail) = tail.lock() {
                        tail.extend(&buffer[..count]);
                        while tail.len() > 16 * 1024 { tail.pop_front(); }
                    }
                }
            }
        }
    });
}

pub struct JdbcDb {
    process: Mutex<Process>,
    timeout: Duration,
    version: String,
    session_id: String,
}

fn enc(s: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(s)
}

/// Split an ordered SQL script without breaking quoted text, comments, or
/// routine bodies. JDBC drivers receive statements one at a time, so a naive
/// semicolon split would corrupt Oracle/Dameng PL/SQL blocks. Compound blocks
/// (CREATE PROCEDURE/FUNCTION/TRIGGER/PACKAGE/TYPE, DECLARE, BEGIN) keep their
/// inner semicolons until every BEGIN/IF/LOOP/CASE is closed.
fn split_ordered_sql(sql: &str) -> Result<Vec<String>> {
    if sql.lines().any(|line| line.trim_start().to_ascii_uppercase().starts_with("DELIMITER ")) {
        bail!("JDBC 通道不解析 MySQL DELIMITER 指令；MySQL 复合例程请使用 MySQL 原生通道");
    }
    #[derive(Clone)]
    enum Mode { Normal, Quote(char), Line, Block(usize), Dollar(String) }
    let chars: Vec<char> = sql.chars().collect();
    let mut statements = Vec::new();
    let mut current = String::new();
    let mut mode = Mode::Normal;
    let mut word = String::new();
    let mut words: Vec<String> = Vec::new();
    let mut previous = String::new();
    let mut compound = false;
    let mut declare_block = false;
    let mut depth = 0usize;
    let mut index = 0usize;
    let mut flush_word = |word: &mut String, words: &mut Vec<String>, previous: &mut String, compound: &mut bool, declare_block: &mut bool, depth: &mut usize| {
        if word.is_empty() { return; }
        let token = std::mem::take(word).to_ascii_uppercase();
        if words.len() < 32 { words.push(token.clone()); }
        let routine = ["PROCEDURE", "FUNCTION", "TRIGGER", "PACKAGE", "TYPE"].contains(&token.as_str());
        if words.first().is_some_and(|first| first == "CREATE") && routine { *compound = true; }
        if words.len() == 1 && token == "DECLARE" { *compound = true; *declare_block = true; *depth = 1; }
        if *compound {
            if token == "END" {
                *depth = depth.saturating_sub(1);
            } else if matches!(token.as_str(), "BEGIN" | "IF" | "LOOP" | "CASE") && previous != "END" {
                // DECLARE 已为匿名块的 BEGIN 预留一层，避免重复计数。
                if token == "BEGIN" && *declare_block { *declare_block = false; }
                else { *depth = depth.saturating_add(1); }
            }
        }
        if words.len() == 1 && token == "BEGIN" { *compound = true; *depth = (*depth).max(1); }
        *previous = token;
    };
    while index < chars.len() {
        let ch = chars[index];
        match mode.clone() {
            Mode::Line => {
                current.push(ch); index += 1;
                if ch == '\n' { mode = Mode::Normal; }
                continue;
            }
            Mode::Block(level) => {
                if ch == '*' && chars.get(index + 1) == Some(&'/') {
                    current.push('*'); current.push('/'); index += 2;
                    mode = if level <= 1 { Mode::Normal } else { Mode::Block(level - 1) };
                } else if ch == '/' && chars.get(index + 1) == Some(&'*') {
                    current.push('/'); current.push('*'); index += 2; mode = Mode::Block(level + 1);
                } else { current.push(ch); index += 1; }
                continue;
            }
            Mode::Quote(quote) => {
                current.push(ch); index += 1;
                if ch == quote {
                    if chars.get(index) == Some(&quote) { current.push(quote); index += 1; }
                    else { mode = Mode::Normal; }
                }
                continue;
            }
            Mode::Dollar(tag) => {
                let remaining: String = chars[index..].iter().collect();
                if remaining.starts_with(&tag) {
                    current.push_str(&tag); index += tag.chars().count(); mode = Mode::Normal;
                } else { current.push(ch); index += 1; }
                continue;
            }
            Mode::Normal => {}
        }
        if ch == '-' && chars.get(index + 1) == Some(&'-') { current.push('-'); current.push('-'); index += 2; mode = Mode::Line; continue; }
        if ch == '/' && chars.get(index + 1) == Some(&'*') { current.push('/'); current.push('*'); index += 2; mode = Mode::Block(1); continue; }
        if matches!(ch, '\'' | '"' | '`') {
            flush_word(&mut word, &mut words, &mut previous, &mut compound, &mut declare_block, &mut depth);
            current.push(ch); index += 1; mode = Mode::Quote(ch); continue;
        }
        if ch == '$' {
            // PostgreSQL dollar quoting; only an identifier tag (or $$) opens it.
            let rest: String = chars[index..].iter().collect();
            if let Some(end) = rest[1..].find('$').filter(|end| *end <= 64) {
                let tag = &rest[..=end + 1];
                if tag[1..tag.len() - 1].chars().all(|c| c.is_alphanumeric() || c == '_') {
                    flush_word(&mut word, &mut words, &mut previous, &mut compound, &mut declare_block, &mut depth);
                    current.push_str(tag); index += tag.chars().count(); mode = Mode::Dollar(tag.to_string()); continue;
                }
            }
        }
        if ch.is_alphanumeric() || ch == '_' || ch == '$' && !word.is_empty() {
            word.push(ch); current.push(ch); index += 1; continue;
        }
        flush_word(&mut word, &mut words, &mut previous, &mut compound, &mut declare_block, &mut depth);
        if ch == ';' && depth == 0 {
            if crate::db::is_executable(&current) { statements.push(current.trim().to_string()); }
            current.clear(); words.clear(); previous.clear(); compound = false; declare_block = false;
            index += 1; continue;
        }
        current.push(ch); index += 1;
    }
    flush_word(&mut word, &mut words, &mut previous, &mut compound, &mut declare_block, &mut depth);
    if crate::db::is_executable(&current) { statements.push(current.trim().to_string()); }
    if statements.is_empty() { bail!("SQL 脚本没有可执行语句"); }
    Ok(statements)
}

fn decode_response(line: &str) -> Result<HttpRows> {
    let value: serde_json::Value =
        serde_json::from_str(line).context("JDBC sidecar 返回无效 JSON")?;
    if let Some(error) = value.get("error") {
        bail!("JDBC: {}", error.as_str().unwrap_or("未知错误"));
    }
    Ok(HttpRows {
        columns: serde_json::from_value(
            value
                .get("columns")
                .cloned()
                .ok_or_else(|| anyhow!("JDBC 响应缺少 columns"))?,
        )?,
        rows: serde_json::from_value(
            value
                .get("rows")
                .cloned()
                .ok_or_else(|| anyhow!("JDBC 响应缺少 rows"))?,
        )?,
    })
}

impl JdbcDb {
    /// 用已解析的 jar / JDBC URL / 可选驱动类名建立一条真实 JDBC 连接。
    pub async fn connect(
        req: &ConnectionRequest,
        jar: &str,
        url: &str,
        driver_class: &str,
    ) -> Result<Self> {
        let jar = PathBuf::from(jar);
        let java_version = runtime_status(&jar)?;
        let options = runtime_options::for_driver(driver_class.trim(), &java_version)?;
        if !url.starts_with("jdbc:") {
            bail!("JDBC URL 必须以 jdbc: 开头");
        }
        let dir = crate::storage::data_dir()
            .join("jdbc-sidecar")
            .join(uuid::Uuid::new_v4().to_string());
        tokio::fs::create_dir_all(&dir).await?;
        let source = dir.join("NavkitJdbc.java");
        tokio::fs::write(&source, relational::source_with_support(include_str!("NavkitJdbc.java"))).await?;
        let classpath = crate::driver_manager::downloads::managed_jdbc::classpath_for(&jar)?;
        let mut command = crate::background_process::tokio_command(java_path());
        if driver_class.trim() == "com.google.cloud.spanner.jdbc.JdbcDriver" {
            for key in ["GOOGLE_APPLICATION_CREDENTIALS", "GOOGLE_CLOUD_PROJECT", "GCLOUD_PROJECT", "GOOGLE_CLOUD_QUOTA_PROJECT", "SPANNER_EMULATOR_HOST"] {
                command.env_remove(key);
            }
            for key in ["SPANNER_DISABLE_BUILTIN_METRICS", "SPANNER_DISABLE_DIRECT_ACCESS_GRPC_BUILTIN_METRICS", "SPANNER_DISABLE_GRPC_GCP_OTEL_METRICS"] {
                command.env(key, "true");
            }
        }
        let mut child = command
            .args(options)
            .arg("-Dfile.encoding=UTF-8")
            .arg("--class-path")
            .arg(classpath)
            .arg(&source)
            .env_remove("BIGQUERY_JDBC_LOG_LEVEL")
            .env_remove("BIGQUERY_JDBC_LOG_PATH")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .spawn()
            .context("无法启动 JDBC sidecar")?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("JDBC stdin 不可用"))?;
        let output = BufReader::new(
            child
                .stdout
                .take()
                .ok_or_else(|| anyhow!("JDBC stdout 不可用"))?,
        );
        let stderr_tail = Arc::new(StdMutex::new(VecDeque::new()));
        if let Some(stderr) = child.stderr.take() { drain_sidecar_stderr(stderr, stderr_tail.clone()); }
        let mut db = Self {
            process: Mutex::new(Process {
                child,
                input: Some(input),
                output,
                source_dir: dir,
                stderr_tail,
            }),
            timeout: Duration::from_secs(req.timeout_secs.clamp(1, 300)),
            version: String::new(),
            session_id: String::new(),
        };
        let response = db
            .request(format!(
                "connect\t{}\t{}\t{}\t{}\t{}\n",
                enc(url),
                enc(&req.user),
                enc(&req.password),
                enc(driver_class.trim()),
                db.timeout.as_secs()
            ))
            .await?;
        db.version = response
            .rows
            .first()
            .and_then(|row| row.get(1).cloned().flatten())
            .or_else(|| {
                response
                    .rows
                    .first()
                    .and_then(|row| row.first().cloned().flatten())
            })
            .unwrap_or_default();
        let capabilities: relational::JdbcCapabilities = db.request_v2("capabilities", serde_json::json!({})).await?;
        db.session_id = capabilities.session_id;
        Ok(db)
    }

    async fn request(&self, line: String) -> Result<HttpRows> {
        let mut process = self.process.lock().await;
        let Process { child, input, output, .. } = &mut *process;
        let response = relational::exchange_line(child, input, output, &line, self.timeout).await?;
        if let Err(error) = relational::validate_scalar(&response) {
            process.input.take();
            let _ = process.child.start_kill();
            return Err(error.context("JDBC scalar contract failed; physical session discarded; do not replay"));
        }
        decode_response(&response)
    }

    async fn request_v2<T: serde::de::DeserializeOwned>(&self, action: &str, payload: serde_json::Value) -> Result<T> {
        let (request_id, line) = relational::request(action, &self.session_id, payload.clone())?;
        let mut process = self.process.lock().await;
        let Process { child, input, output, .. } = &mut *process;
        let response = relational::exchange_line(child, input, output, &line, self.timeout).await?;
        let envelope = match relational::decode_envelope(&response, &request_id, &self.session_id) {
            Ok(value) => value,
            Err(error) => { process.input.take(); let _ = process.child.start_kill(); return Err(error); }
        };
        if let Err(error) = relational::validate_result(action, &payload, &envelope) {
            process.input.take(); let _ = process.child.start_kill();
            return Err(error.context("JDBC evidence contract failed; physical session discarded; outcome unknown, do not replay"));
        }
        let driver_error = envelope.get("error").is_some();
        if relational::poisoned(&envelope) { process.input.take(); let _ = process.child.start_kill(); }
        match relational::decode_result(envelope) {
            Ok(result) => Ok(result),
            Err(error) => {
                if !driver_error { process.input.take(); let _ = process.child.start_kill(); }
                Err(error)
            }
        }
    }

    pub async fn query_rows(&self, sql: &str) -> Result<HttpRows> {
        self.query_scoped("", sql).await
    }

    async fn query_scoped(&self, namespace: &str, sql: &str) -> Result<HttpRows> {
        let mut context=crate::sql_history::current_context();
        if !namespace.is_empty(){context.namespace=namespace.into();}
        let mut history=crate::sql_history::Guard::in_context(sql,Vec::new(),context);
        let result=self.request(format!("query\t{}\t{}\t{}\n", enc(sql), self.timeout.as_secs(), enc(namespace))).await;
        history.result(&result,|r|r.rows.len() as i64);
        result
    }

    async fn command(&self, line: String) -> Result<HttpRows> {
        self.request(line).await
    }
}

#[async_trait]
impl HttpAdapter for JdbcDb {
    fn shares_session_namespace(&self) -> bool { true }
    async fn relational_capabilities(&self) -> Result<relational::JdbcCapabilities> {
        self.request_v2("capabilities", serde_json::json!({})).await
    }
    async fn relational_metadata(&self, object: &relational::JdbcObjectId) -> Result<relational::JdbcMetadataSnapshot> {
        self.request_v2("metadata", serde_json::json!({"object": object})).await
    }
    async fn apply_row_plan(&self, plan: &relational::JdbcRowPlan) -> Result<relational::JdbcMutationOutcome> {
        if plan.read_only { bail!("READ_ONLY_VIOLATION: effective JDBC connection is read-only"); }
        if plan.session_id != self.session_id { bail!("STALE_CONTEXT: row plan belongs to another physical JDBC session"); }
        self.request_v2("row_plan", serde_json::to_value(plan)?).await
    }
    async fn ping(&self) -> Result<String> {
        self.command(format!("ping\t{}\n", self.timeout.as_secs())).await?;
        Ok(format!("JDBC {}", self.version))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        Ok(self
            .command("containers\n".into())
            .await?
            .rows
            .into_iter()
            .filter_map(|row| {
                let mut it = row.into_iter();
                let name = it.next().flatten()?;
                let kind = it.next().flatten().unwrap_or_else(|| "schema".into());
                Some((name, format!("JDBC {kind}")))
            })
            .collect())
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        Ok(self
            .command(format!("tables\t{}\n", enc(container)))
            .await?
            .rows
            .into_iter()
            .filter_map(|row| row.into_iter().next().flatten())
            .collect())
    }
    async fn object_types(&self, container: &str) -> Result<Option<Vec<(String, String)>>> {
        // sidecar tables 命令返回 [name, type]（旧版仅 name → 全部按表）。
        Ok(Some(self
            .command(format!("tables\t{}\n", enc(container)))
            .await?
            .rows
            .into_iter()
            .filter_map(|row| {
                let mut it = row.into_iter();
                let name = it.next().flatten()?;
                let ty = it.next().flatten().unwrap_or_else(|| "table".into());
                Some((name, if ty.eq_ignore_ascii_case("view") { "view".into() } else { "table".into() }))
            })
            .collect()))
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        if limit > 10000 || offset.checked_add(limit).map_or(true, |end| end > 1000000) {
            bail!("JDBC preview exceeds explicit paging safety bounds (offset+limit <= 1000000, limit <= 10000)");
        }
        self.command(format!(
            "rows\t{}\t{}\t{}\t{}\t{}\n",
            enc(container), enc(object), offset, limit, self.timeout.as_secs()
        ))
        .await
    }
    async fn run(&self, container: &str, _: &str, sql: &str) -> Result<HttpRows> {
        self.query_scoped(container, sql).await
    }
    fn supports_sql_batch(&self) -> bool { true }
    /// Execute an ordered script on one physical session. Statements are sent in
    /// source order; the first driver error stops the tail and is reported with
    /// the already-executed prefix. Nothing is retried or replayed.
    async fn run_sql_batch(&self, container: &str, sql: &str) -> Result<crate::models::MultiResult> {
        let statements = split_ordered_sql(sql)?;
        let mut context = crate::sql_history::current_context();
        if !container.is_empty() { context.namespace = container.into(); }
        let started = std::time::Instant::now();
        let mut sets = Vec::new();
        let mut messages = Vec::new();
        let mut process = self.process.lock().await;
        for (index, statement) in statements.iter().enumerate() {
            let trimmed = statement.trim();
            if !crate::db::is_executable(trimmed) { continue; }
            let mut history = crate::sql_history::Guard::in_context(trimmed, Vec::new(), context.clone());
            let Process { child, input, output, .. } = &mut *process;
            let line = format!("query\t{}\t{}\t{}\n", enc(trimmed), self.timeout.as_secs(), enc(container));
            let response = match relational::exchange_line(child, input, output, &line, self.timeout).await {
                Ok(response) => response,
                Err(error) => {
                    drop(history);
                    process.input.take(); let _ = process.child.start_kill();
                    return Err(error.context(format!("第 {} 条语句执行结果未知；物理会话已丢弃，禁止重放", index + 1)));
                }
            };
            if let Err(error) = relational::validate_scalar(&response) {
                drop(history);
                process.input.take(); let _ = process.child.start_kill();
                return Err(error.context(format!("第 {} 条语句回执不符合协议；物理会话已丢弃，禁止重放", index + 1)));
            }
            let rows = match decode_response(&response) {
                Ok(rows) => rows,
                Err(error) => {
                    if !sets.is_empty() { messages.push(format!("前 {} 条语句已按顺序执行；第 {} 条失败", sets.len(), index + 1)); }
                    history.result(&Err::<(), _>(anyhow!("{error}")), |_| 0);
                    return Err(error.context(format!("第 {} 条语句失败（共 {} 条）", index + 1, statements.len())));
                }
            };
            history.result(&Ok::<_, anyhow::Error>(()), |_| rows.rows.len() as i64);
            let total = rows.rows.len() as u64;
            sets.push(crate::models::ResultSet {
                columns: rows.columns.into_iter().map(|name| crate::models::ResultColumn {
                    name, data_type: "string".into(), nullable: true, primary_key: false, auto_increment: false,
                }).collect(),
                rows: rows.rows,
                affected: 0,
                elapsed_ms: 0,
                ok: true,
                message: String::new(),
                has_rows: true,
                primary_keys: Vec::new(),
                total,
            });
        }
        Ok(crate::models::MultiResult { sets, messages, elapsed_ms: started.elapsed().as_millis() as u64, spool_id: None })
    }
    async fn table_columns(
        &self,
        container: &str,
        object: &str,
    ) -> Result<Option<Vec<HttpColumn>>> {
        let rows = self
            .command(format!("columns\t{}\t{}\n", enc(container), enc(object)))
            .await?
            .rows;
        if rows.is_empty() {
            return Ok(None);
        }
        Ok(Some(
            rows.into_iter()
                .filter_map(|row| {
                    let mut it = row.into_iter();
                    let name = it.next().flatten()?;
                    let data_type = it.next().flatten().unwrap_or_default();
                    let nullable = it.next().flatten().unwrap_or_default() != "false";
                    let primary_key = it.next().flatten().unwrap_or_default() == "true";
                    Some(HttpColumn {
                        name,
                        data_type,
                        nullable,
                        primary_key,
                    })
                })
                .collect(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jdbc_error_is_not_success() {
        assert!(
            decode_response(r#"{"error":"SQLException [08001]: refused"}"#)
                .err()
                .unwrap()
                .to_string()
                .contains("08001")
        );
    }

    #[test]
    fn jdbc_null_and_unicode_preserved() {
        let r = decode_response(r#"{"columns":["中文"],"rows":[[null],["值;尾"]]}"#).unwrap();
        assert_eq!(r.rows[0][0], None);
        assert_eq!(r.rows[1][0].as_deref(), Some("值;尾"));
    }

    #[test]
    fn ordered_batch_preserves_literals_and_drops_comment_only_blocks() {
        let statements = split_ordered_sql("SELECT '中文;O''Reilly';\n-- only comment\nSELECT 2;").unwrap();
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0], "SELECT '中文;O''Reilly'");
        // 注释属于下一条语句的前导内容，保留原文交给驱动（不丢用户 SQL）。
        assert!(statements[1].ends_with("SELECT 2"), "{statements:?}");
        assert!(split_ordered_sql("-- nothing to run\n").is_err());
    }

    #[test]
    fn ordered_batch_keeps_dameng_and_oracle_compound_blocks_intact() {
        for sql in [
            "CREATE OR REPLACE PROCEDURE p AS BEGIN INSERT INTO t VALUES(1); COMMIT; END;",
            "CREATE OR REPLACE TRIGGER trg BEFORE INSERT ON t FOR EACH ROW BEGIN :NEW.id := 1; END;",
            "DECLARE x INT; BEGIN x := 1; END;",
            "BEGIN IF 1 = 1 THEN INSERT INTO t VALUES(1); END IF; END;",
        ] {
            let statements = split_ordered_sql(sql).unwrap();
            assert_eq!(statements.len(), 1, "块被错误切分: {sql} => {statements:?}");
            assert!(statements[0].ends_with("END"));
        }
    }

    #[test]
    fn ordered_batch_handles_dollar_quotes_and_rejects_mysql_delimiter() {
        let statements = split_ordered_sql("SELECT $tag$ a;b $tag$; SELECT 2;").unwrap();
        assert_eq!(statements.len(), 2);
        assert!(split_ordered_sql("DELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT 1; END$$").is_err());
    }
}
