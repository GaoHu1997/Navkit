//! H2 JDBC sidecar using the Java 11 source-file launcher, not ODBC.
use std::{path::{Path, PathBuf}, process::Stdio, time::Duration};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use base64::Engine;
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, process::{Child, ChildStdin, ChildStdout}, sync::Mutex};
use crate::{http_db::{HttpAdapter, HttpRows}, models::ConnectionRequest};

pub fn java_path() -> PathBuf {
    if let Some(java) = std::env::var_os("NAVKIT_JAVA") { return java.into(); }
    if let Some(home) = std::env::var_os("JAVA_HOME") { return PathBuf::from(home).join("bin").join(if cfg!(windows) { "java.exe" } else { "java" }); }
    PathBuf::from("java")
}
pub fn runtime_status(jar: &Path) -> Result<String> { runtime_status_for(&java_path(), jar) }
fn bounded_java(java: &Path, args: &[&std::ffi::OsStr]) -> Result<std::process::Output> {
    let mut child = crate::background_process::command(java).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().context("无法运行Java；H2需要Java 11+，请配置JAVA_HOME或NAVKIT_JAVA")?;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop { if child.try_wait()?.is_some() { return Ok(child.wait_with_output()?); } if std::time::Instant::now() >= deadline { let _=child.kill(); let _=child.wait(); bail!("H2 Java运行时探针15秒超时，进程已终止"); } std::thread::sleep(Duration::from_millis(20)); }
}
fn runtime_status_for(java: &Path, jar: &Path) -> Result<String> {
    if !jar.is_file() { bail!("H2 JDBC jar不存在，请下载H2驱动或指定h2_jar参数：{}", jar.display()); }
    // 成功探针按 java/jar 的路径+大小+mtime 缓存：驱动检测（每次打开新建连接都会跑）
    // 不必反复启动 java；jar 或 Java 被替换时键失效自动重探。
    crate::probe_cache::cached(java, jar, "h2", || h2_runtime_probe(java, jar))
}
fn h2_runtime_probe(java: &Path, jar: &Path) -> Result<String> {
    let output = bounded_java(java, &[std::ffi::OsStr::new("-version")])?;
    if !output.status.success() { bail!("Java -version失败"); }
    let version = String::from_utf8_lossy(&output.stderr).into_owned();
    let major=version.split('"').nth(1).and_then(|s|s.split('.').next()).and_then(|s|s.parse::<u32>().ok()).unwrap_or(0);
    if major<11 { bail!("H2 source launcher需要Java 11+：{}",version.trim()); }
    // A source-launch + class-load probe verifies jdk.compiler, jar bytecode and dependencies.
    let dir=std::env::temp_dir().join(format!("navkit-h2-probe-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&dir)?;
    let source=dir.join("NavkitH2Probe.java");std::fs::write(&source,"class NavkitH2Probe { public static void main(String[] args) throws Exception { Class.forName(\"org.h2.Driver\"); System.out.println(\"NAVKIT_H2_READY\"); } }")?;
    let probe=bounded_java(java,&[std::ffi::OsStr::new("--class-path"),jar.as_os_str(),source.as_os_str()]);let _=std::fs::remove_dir_all(&dir);
    let probe=probe?;if !probe.status.success() || String::from_utf8_lossy(&probe.stdout).trim()!="NAVKIT_H2_READY" { bail!("H2 Java源码编译/驱动类加载失败（需完整Java11+且Jar字节码兼容）：{}",String::from_utf8_lossy(&probe.stderr).chars().take(1000).collect::<String>()); }
    Ok(format!("{}; H2 source-launch/Driver verified",version.trim()))
}
/// 默认托管 H2 JAR：推荐版本的版本化目录优先，其次旧布局 managed-drivers/h2/h2.jar。
pub fn default_jar() -> PathBuf {
    let versioned = crate::driver_manager::downloads::managed_h2_jar_for(crate::driver_manager::downloads::H2_RECOMMENDED);
    if versioned.exists() { return versioned; }
    let legacy = crate::storage::data_dir().join("managed-drivers").join("h2").join("h2.jar");
    if legacy.exists() { legacy } else { versioned }
}
struct Process { child: Child, input: Option<ChildStdin>, output: BufReader<ChildStdout>, source_dir: PathBuf }
impl Drop for Process {
    fn drop(&mut self) {
        // EOF lets JDBC Connection.close() flush committed MVStore/auth data. A hard kill
        // immediately after COMMIT can discard H2's delayed file writes.
        self.input.take();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut exited = false;
        while std::time::Instant::now() < deadline {
            match self.child.try_wait() { Ok(Some(_)) => { exited = true; break; }, Err(_) => break, Ok(None) => {} }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !exited {
            let _ = self.child.start_kill();
            let reap_deadline = std::time::Instant::now() + Duration::from_millis(500);
            while std::time::Instant::now() < reap_deadline {
                if !matches!(self.child.try_wait(), Ok(None)) { break; }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if self.source_dir.is_absolute() && self.source_dir.parent().and_then(Path::file_name).and_then(|s|s.to_str()) == Some("h2-sidecar")
            && self.source_dir.file_name().and_then(|s|s.to_str()).is_some_and(|s|uuid::Uuid::parse_str(s).is_ok()) {
            let _ = std::fs::remove_dir_all(&self.source_dir);
        }
    }
}
pub struct H2Db { process: Mutex<Process>, timeout: Duration, version: String }
fn enc(s: &str) -> String { base64::engine::general_purpose::STANDARD.encode(s) }
fn decode_response(line: &str) -> Result<HttpRows> {
    let value: serde_json::Value = serde_json::from_str(line).context("H2 sidecar返回无效JSON")?;
    if let Some(error) = value.get("error") { bail!("H2 JDBC: {}", error.as_str().unwrap_or("未知错误")); }
    Ok(HttpRows {
        columns: serde_json::from_value(value.get("columns").cloned().ok_or_else(|| anyhow!("H2响应缺少columns"))?)?,
        rows: serde_json::from_value(value.get("rows").cloned().ok_or_else(|| anyhow!("H2响应缺少rows"))?)?,
    })
}
impl H2Db {
    pub async fn connect(req: &ConnectionRequest) -> Result<Self> {
        // 托管驱动版本选择（@navkit-driver-version: 前缀）优先；剥离后剩余参数照常解析。
        let (selection, rest_params) = crate::driver_manager::split_version_selection(&req.params)?;
        if let Some(selection) = &selection {
            if selection.kind != "h2" { bail!("托管驱动版本选择属于数据源 {}，与当前 H2 连接不一致", selection.kind); }
        }
        let mut scoped = req.clone();
        scoped.params = rest_params;
        let p = crate::native_db::Params::new(&scoped);
        let jar = match &selection {
            Some(s) => crate::driver_manager::downloads::resolve_h2_jar(&s.version)?,
            None => p.get("h2_jar").map(PathBuf::from).or_else(|| std::env::var_os("NAVKIT_H2_JAR").map(PathBuf::from)).unwrap_or_else(default_jar),
        };
        let java = p.get("java").map(PathBuf::from).unwrap_or_else(java_path);
        runtime_status_for(&java, &jar)?;
        let dir = crate::storage::data_dir().join("h2-sidecar").join(uuid::Uuid::new_v4().to_string()); tokio::fs::create_dir_all(&dir).await?;
        let source = dir.join("NavkitH2.java"); tokio::fs::write(&source, include_str!("NavkitH2.java")).await?;
        let mut child = crate::background_process::tokio_command(java).arg("--class-path").arg(&jar).arg(&source)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).kill_on_drop(false).spawn().context("无法启动H2 JDBC sidecar")?;
        let input = child.stdin.take().ok_or_else(|| anyhow!("H2 stdin不可用"))?;
        let output = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("H2 stdout不可用"))?);
        let mut db = Self { process: Mutex::new(Process { child, input: Some(input), output, source_dir: dir }), timeout: Duration::from_secs(req.timeout_secs.max(1)), version: String::new() };
        let url = p.get("jdbc_url").map(str::to_owned).unwrap_or_else(|| {
            if req.database.starts_with("jdbc:h2:") { req.database.clone() }
            else if !req.file.is_empty() { format!("jdbc:h2:{}", req.file) }
            else if !req.host.is_empty() && req.port != 0 { format!("jdbc:h2:tcp://{}:{}/{}", req.host, req.port, req.database) }
            else { format!("jdbc:h2:mem:{};DB_CLOSE_DELAY=-1", if req.database.is_empty() { "navkit" } else { &req.database }) }
        });
        if !url.starts_with("jdbc:h2:") { bail!("jdbc_url必须使用jdbc:h2:"); }
        let response = db.request(format!("connect\t{}\t{}\t{}\n", enc(&url), enc(if req.user.is_empty() { "sa" } else { &req.user }), enc(&req.password))).await?;
        db.version = response.rows.first().and_then(|r| r.first()).cloned().flatten().unwrap_or_default();
        Ok(db)
    }
    async fn request(&self, line: String) -> Result<HttpRows> {
        let mut process = self.process.lock().await;
        let result = tokio::time::timeout(self.timeout, async {
            let input = process.input.as_mut().ok_or_else(|| anyhow!("H2 sidecar已关闭"))?;
            input.write_all(line.as_bytes()).await?; input.flush().await?;
            let mut response = String::new();
            if process.output.read_line(&mut response).await? == 0 { bail!("H2 sidecar已退出"); }
            decode_response(&response)
        }).await;
        match result { Ok(r) => r, Err(_) => { let _ = process.child.kill().await; bail!("H2 JDBC请求超时；sidecar已终止，请重新连接"); } }
    }
    async fn query(&self, sql: &str) -> Result<HttpRows> { self.query_scoped("", sql).await }
    async fn query_scoped(&self, schema: &str, sql: &str) -> Result<HttpRows> {
        // The process mutex covers both schema selection and SQL, so concurrent
        // namespace views cannot leak a previous command's session schema.
        self.request(format!("query\t{}\t{}\t{}\n", enc(sql), self.timeout.as_secs(), enc(schema))).await
    }
}
#[async_trait]
impl HttpAdapter for H2Db {
    async fn ping(&self) -> Result<String> { self.query("SELECT 1").await?; Ok(format!("H2 {}", self.version)) }
    async fn containers(&self) -> Result<Vec<(String,String)>> { Ok(self.query("SELECT SCHEMA_NAME FROM INFORMATION_SCHEMA.SCHEMATA ORDER BY SCHEMA_NAME").await?.rows.into_iter().filter_map(|r| r.into_iter().next().flatten()).map(|n| (n, "H2 schema".into())).collect()) }
    async fn objects(&self, container: &str) -> Result<Vec<String>> { Ok(self.query(&format!("SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA={} ORDER BY TABLE_NAME", crate::db::quote_literal(container))).await?.rows.into_iter().filter_map(|r| r.into_iter().next().flatten()).collect()) }
    async fn rows(&self, container: &str, object: &str, offset:u64, limit:u64) -> Result<HttpRows> { self.query(&format!("SELECT * FROM {}.{} OFFSET {} ROWS FETCH NEXT {} ROWS ONLY", crate::db::quote_ident(&crate::models::DbKind::of("h2"), container), crate::db::quote_ident(&crate::models::DbKind::of("h2"), object), offset, limit.min(10000))).await }
    async fn run(&self, container: &str, _: &str, sql: &str) -> Result<HttpRows> { self.query_scoped(container, sql).await }
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn h2_error_is_not_success() { assert!(decode_response(r#"{"error":"JdbcSQLSyntaxErrorException [42000]: bad"}"#).err().unwrap().to_string().contains("42000")); }
    #[test] fn h2_null_and_unicode_preserved() { let r = decode_response(r#"{"columns":["中文"],"rows":[[null],["值;尾"]]}"#).unwrap(); assert_eq!(r.rows[0][0],None); assert_eq!(r.rows[1][0].as_deref(),Some("值;尾")); }

    #[tokio::test]
    #[ignore = "real official H2 jar + Java; copied executable in navkit-jdbc-command-tests-* temp dir"]
    async fn real_hidden_h2_connection() {
        let portable = crate::storage::portable_root();
        assert!(portable.starts_with(std::env::temp_dir())
            && portable.file_name().unwrap().to_string_lossy().starts_with("navkit-jdbc-command-tests-"),
            "must run only a copied, isolated test executable");
        let jar = PathBuf::from(std::env::var_os("NAVKIT_H2_JAR").expect("real official H2 jar"));
        let req = ConnectionRequest {
            kind: crate::models::DbKind::of("h2"),
            host: String::new(), port: 0, user: "sa".into(), password: String::new(),
            database: format!("navkit_hidden_{}", uuid::Uuid::new_v4().simple()),
            file: String::new(),
            params: serde_json::json!({"h2_jar": jar.to_string_lossy()}).to_string(),
            timeout_secs: 10,
        };
        let crate::db::Db::Http(client) = crate::db::open(&req).await.unwrap() else {
            panic!("built-in H2 must use the Java adapter, not ODBC");
        };
        client.run("PUBLIC", "", "CREATE TABLE NAVKIT_HIDDEN(id INT PRIMARY KEY, payload VARCHAR(100), nil VARCHAR(10))").await.unwrap();
        client.run("PUBLIC", "", "INSERT INTO NAVKIT_HIDDEN VALUES(1,'后台;中文',NULL)").await.unwrap();
        let rows = client.run("PUBLIC", "", "SELECT payload,nil FROM NAVKIT_HIDDEN").await.unwrap();
        assert_eq!(rows.rows[0][0].as_deref(), Some("后台;中文"));
        assert_eq!(rows.rows[0][1], None);
        assert!(client.run("PUBLIC", "", "SELECT * FROM NAVKIT_MISSING_TABLE").await.is_err());
        assert_eq!(client.run("PUBLIC", "", "SELECT COUNT(*) FROM NAVKIT_HIDDEN").await.unwrap().rows[0][0].as_deref(), Some("1"));
        println!("Real H2 background readiness/connect/DDL/DML/unicode/NULL/error recovery PASS");
    }
}
