//! 通用 JDBC sidecar：用 Java 11+ 的 source-file launcher 加载用户提供的 .jar 驱动。
//!
//! 与 `h2_db` 同构：私有进程 + stdin/stdout 上的 tab/base64 协议，因此密码不会出现在
//! 进程命令行里。一个 sidecar 承载一条连接，Drop 时终止进程并清理临时源码目录。
use std::{
    path::{Path, PathBuf},
    process::Stdio,
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
        if self.source_dir.is_absolute()
            && self.source_dir.parent().and_then(Path::file_name).and_then(|s| s.to_str()) == Some("jdbc-sidecar")
            && self.source_dir.file_name().and_then(|s| s.to_str()).is_some_and(|s| uuid::Uuid::parse_str(s).is_ok()) {
            let _ = std::fs::remove_dir_all(&self.source_dir);
        }
    }
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
            .stderr(Stdio::inherit())
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
        let mut db = Self {
            process: Mutex::new(Process {
                child,
                input: Some(input),
                output,
                source_dir: dir,
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
}
