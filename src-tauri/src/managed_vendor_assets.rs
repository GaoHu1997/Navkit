//! Exact original public vendor assets, isolated from the general Maven/GitHub allowlist.
//! User-initiated downloads retain the original bytes/notices; nothing is bundled or relicensed.
use super::*;
use std::io::Write;

const GBASE_URL: &str = "https://g1.cdn.gbase.cn/products/30/v9w1iatC0C-EOMRQXuVe8/gbase-connector-java-9.5.0.10-build1.zip";
const INCEPTOR_URL: &str = "https://transwarp-ce-1253207870.cos.ap-shanghai.myqcloud.com/%E9%A9%B1%E5%8A%A8%E5%88%97%E8%A1%A8/0919/inceptor-driver-8.37.3.jar";
const QUARK_URL: &str = "https://transwarp-ce-1253207870.cos.ap-shanghai.myqcloud.com/%E9%A9%B1%E5%8A%A8%E5%88%97%E8%A1%A8/0919/quark-driver-8.37.3.jar";
const GBASE_ROOT: &str = "gbase-connector-java-9.5.0.10-build1/";
const GBASE_INNER: &str = "gbase-connector-java-9.5.0.10-build1/gbase-connector-java-9.5.0.10-build1-bin.jar";
const GBASE_README: &str = "gbase-connector-java-9.5.0.10-build1/readme.txt";
const READ_ME: DownloadSpec = spec(GBASE_URL, 351, "f605f9d63ec37bc4fe39a0b929754cdd4c140533ed471d5132ebada66afaa7c0");
fn archive_spec() -> DownloadSpec { spec(GBASE_URL, 813_043, "25b71bc345f1bd3a5503593b829c69167153df7a45c45c25487b5372e0fb9832") }

pub(super) fn vendor(kind: &str) -> Option<Vendor> {
    match kind {
        "gbase" => Some(Vendor {
            kind: "gbase", kinds: &["gbase"], version: "9.5.0.10-build1", class: "com.gbase.jdbc.Driver", prefix: "gbase",
            manifest_version: "9.5.0.10-build1", class_major: 49,
            probe_url: "jdbc:gbase://127.0.0.1:1/test?useSSL=false&connectTimeout=3000&socketTimeout=3000",
            // The URL identifies the original ZIP. The independent transport contract below
            // checks that ZIP before extracting this exact inner JAR; it is never fetched as a JAR.
            jar: spec(GBASE_URL, 846_594, "ac3d899f717c9fc6ae53277dd1d619884d619358ddfa9026db5add80e1d62ba8"),
            pom: None, license: None, entries: 298, expanded: 1_908_821, max_expanded: 8 * 1024 * 1024,
            limits: "官网 GBase 8a Connector/J 9.5.0.10-build1 原 ZIP、原 JAR 和 release readme 全部保留；需 Java 11+，不捆绑或推断第三方再分发许可。仅普通 TCP 用户名/密码路线；不会以 8s、8c 或 MySQL 客户端替代。TLS/证书/集群扩展请显式配置自定义 JDBC；厂商服务 SQL/认证仍须实库验收。",
        }),
        "inceptor" => Some(Vendor {
            kind: "inceptor", kinds: &["inceptor"], version: "8.37.3", class: "org.apache.hive.jdbc.HiveDriver", prefix: "inceptor2",
            manifest_version: "8.37.3", class_major: 51,
            probe_url: "jdbc:inceptor2://127.0.0.1:1/default;auth=noSasl",
            jar: spec(INCEPTOR_URL, 10_719_927, "983a7cb06bf82a3ba1c4b41f644969acecd623b0dee17fc59ca1663bf13c5f3d"),
            pom: None, license: None, entries: 6660, expanded: 23_473_446, max_expanded: 32 * 1024 * 1024,
            limits: "星环官网 legacy Inceptor JDBC 8.37.3 原包，保留包内原始许可/NOTICE，不捆绑或改写厂商授权；需 Java 11+。目前只支持字面普通 TCP auth=noSasl，用户名/密码走 stdin Properties。Kerberos、SASL、ZooKeeper、HTTP/TLS 与专有认证请显式配置完整自定义 JDBC；不是 Apache Hive 包，也不推断 ArgoDB 适用性。厂商服务器 SQL/认证仍须实库验收。",
        }),
        "argodb" => Some(Vendor {
            kind: "argodb", kinds: &["argodb"], version: "8.37.3", class: "io.transwarp.jdbc.QuarkDriver", prefix: "transwarp2",
            manifest_version: "8.37.3", class_major: 51,
            probe_url: "jdbc:transwarp2://127.0.0.1:1/default;auth=noSasl",
            jar: spec(QUARK_URL, 10_748_370, "3905fb22c0667dba25518a3174c999b09e2a4fe8f3ad5544fba557c2877da3c6"),
            pom: None, license: None, entries: 6450, expanded: 24_613_167, max_expanded: 32 * 1024 * 1024,
            limits: "星环官方 Quark JDBC 8.37.3 独立原包；ArgoDB 官方文档明确 Quark Gateway 是 SQL 入口。使用实际 io.transwarp.jdbc.QuarkDriver 与 jdbc:transwarp2，不沿用旧示例 HiveDriver，不以 Inceptor/Hive 包替代。Java 11+，仅普通 TCP auth=noSasl 路线；不把 noSasl 冒称 LDAP/NONE/Kerberos 已验证。TLS、LDAP/Kerberos/HTTP 扩展请显式绑定自定义 JDBC；保留原包许可，不捆绑或推断再分发授权。厂商实库 SQL/认证仍 UNVERIFIED。",
        }),
        _ => None,
    }
}
pub(super) fn extra_files(kind: &str) -> &'static [&'static str] {
    if kind == "gbase" { &["official.zip", "release-readme.txt"] } else { &[] }
}
fn admitted(spec: &DownloadSpec) -> bool {
    let gbase = archive_spec();
    let inceptor = vendor("inceptor").unwrap().jar;
    [gbase, inceptor, vendor("argodb").unwrap().jar].iter().any(|fixed| fixed.url == spec.url && fixed.size == spec.size && fixed.sha256 == spec.sha256)
}
// These small sources do not require ranges. Inceptor's actual fixed original has text/plain
// MIME; only this exact URL/size/digest can accept it, never a generic text response or redirect.
async fn fetch(spec: &DownloadSpec, path: &Path) -> Result<()> {
    if !admitted(spec) { bail!("未核验的厂商原始下载资产") }
    let url = reqwest::Url::parse(spec.url)?;
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() || url.port().is_some() { bail!("厂商下载 URL 无效") }
    let client = reqwest::Client::builder().user_agent("NavKit-DriverManager/1.0").no_gzip()
        .redirect(reqwest::redirect::Policy::none()).connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180)).build()?;
    let mut request = client.get(url).header(reqwest::header::ACCEPT_ENCODING, "identity").header(reqwest::header::ACCEPT, "application/octet-stream");
    if matches!(spec.url, INCEPTOR_URL | QUARK_URL) { request = request.header(reqwest::header::REFERER, "https://community.transwarp.cn/"); }
    let mut response = tokio::time::timeout(Duration::from_secs(40), request.send()).await.context("等待厂商驱动响应超时")?
        .map_err(reqwest::Error::without_url)?.error_for_status().map_err(reqwest::Error::without_url)?;
    if response.status() != reqwest::StatusCode::OK { bail!("厂商原件必须返回完整 HTTP 200，不接受重定向或分段") }
    if response.headers().contains_key(reqwest::header::CONTENT_RANGE)
        || response.headers().get(reqwest::header::CONTENT_ENCODING).is_some_and(|value| value.as_bytes() != b"identity")
        || response.content_length().is_some_and(|size| size != spec.size) { bail!("厂商驱动响应大小或编码不符") }
    let content_type = response.headers().get(reqwest::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if content_type.contains("html") || content_type.contains("xml") || content_type.contains("json")
        || (content_type.starts_with("text/") && !(matches!(spec.url, INCEPTOR_URL | QUARK_URL) && content_type.split(';').next() == Some("text/plain"))) { bail!("厂商下载返回错误网页而不是固定驱动原件") }
    let mut file = fs::OpenOptions::new().create_new(true).write(true).open(path)?;
    let mut received = 0u64;
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(15), response.chunk()).await.context("厂商驱动响应读取超时")?.map_err(reqwest::Error::without_url)? {
        received = received.checked_add(chunk.len() as u64).ok_or_else(|| anyhow!("厂商下载大小溢出"))?;
        if received > spec.size { bail!("厂商驱动超过固定大小") }
        file.write_all(&chunk)?;
    }
    file.flush()?; drop(file);
    check_asset(path, spec)
}
fn read_archive(archive: &Path, v: &Vendor) -> Result<(Vec<u8>, Vec<u8>)> {
    check_asset(archive, &archive_spec())?;
    let mut zip = zip::ZipArchive::new(fs::File::open(archive)?)?;
    if zip.len() != 3 { bail!("GBase 官方 ZIP 条目数不符") }
    let mut names = HashSet::new(); let mut driver = None; let mut readme = None;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_string();
        if entry.enclosed_name().is_none() || !safe_relative(name.trim_end_matches('/')) || !names.insert(name.clone())
            || entry.unix_mode().is_some_and(|mode| mode & 0o170000 == 0o120000) { bail!("GBase ZIP 含不安全或重复条目") }
        match name.as_str() {
            GBASE_ROOT if entry.is_dir() && entry.size() == 0 => {},
            GBASE_INNER | GBASE_README if !entry.is_dir() => {
                let size = if name == GBASE_INNER { v.jar.size } else { READ_ME.size };
                if entry.size() != size { bail!("GBase ZIP 组件大小不符") }
                let mut bytes = Vec::new(); entry.by_ref().take(size + 1).read_to_end(&mut bytes)?; // read EOF validates CRC
                if bytes.len() as u64 != size { bail!("GBase ZIP 组件被截断") }
                if name == GBASE_INNER { driver = Some(bytes); } else { readme = Some(bytes); }
            },
            _ => bail!("GBase ZIP 包含未核验组件"),
        }
    }
    Ok((driver.ok_or_else(|| anyhow!("GBase ZIP 缺 JAR"))?, readme.ok_or_else(|| anyhow!("GBase ZIP 缺原始 readme"))?))
}
pub(super) fn verify(dir: &Path, v: &Vendor) -> Result<()> {
    if v.kind == "gbase" {
        let (driver, readme) = read_archive(&dir.join("official.zip"), v)?;
        check_asset(&dir.join("release-readme.txt"), &READ_ME)?;
        if driver != fs::read(dir.join("driver.jar"))? || readme != fs::read(dir.join("release-readme.txt"))? { bail!("GBase 提取文件与官网原 ZIP 不一致") }
    }
    Ok(())
}
pub(super) async fn download(v: &Vendor, dir: &Path) -> Result<bool> {
    match v.kind {
        "gbase" => {
            fetch(&archive_spec(), &dir.join("official.zip")).await?;
            let (driver, readme) = read_archive(&dir.join("official.zip"), v)?;
            fs::write(dir.join("driver.jar"), driver)?; fs::write(dir.join("release-readme.txt"), readme)?;
            check_asset(&dir.join("driver.jar"), &v.jar)?; check_asset(&dir.join("release-readme.txt"), &READ_ME)?;
        },
        "inceptor" | "argodb" => fetch(&v.jar, &dir.join("driver.jar")).await?,
        _ => return Ok(false),
    }
    Ok(true)
}
pub(super) fn connection_url(req: &ConnectionRequest, kind: &str, pairs: &[(String, String)]) -> Result<String> {
    if kind == "argodb" && (!req.user.is_empty() || !req.password.is_empty()) { bail!("ArgoDB 托管 auth=noSasl 连接应清空用户名和密码；LDAP/NONE/Kerberos 请显式配置自定义 JDBC，不能静默丢弃凭据") }
    if matches!(tls_mode(pairs)?, Some(TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull)) { bail!("此厂商托管连接未核验 TLS，不会降级明文；请显式配置自定义 JDBC") }
    if pairs.iter().any(|(key, _)| !["ssl", "tls", "sslmode", "ssl-mode", "ssl_verify"].contains(&key.as_str())) { bail!("此厂商扩展认证或命名空间属性请显式配置完整自定义 JDBC") }
    let default = if kind == "gbase" { 5258 } else { 10000 };
    let database = if req.database.trim().is_empty() { if kind == "gbase" { "test" } else { "default" } } else { plain_name(req.database.trim(), "厂商数据库名")? };
    let authority = server_authority(req, default)?;
    if kind == "gbase" {
        Ok(format!("jdbc:gbase://{authority}/{database}?useSSL=false&connectTimeout={0}&socketTimeout={0}", req.timeout_secs.clamp(1, 300) * 1000))
    } else { let prefix=if kind=="argodb"{"transwarp2"}else{"inceptor2"}; Ok(format!("jdbc:{prefix}://{authority}/{database};auth=noSasl")) }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn public_vendor_sources_are_exact_assets_not_new_trusted_origins() {
        assert!(admitted(&archive_spec())); assert!(admitted(&vendor("inceptor").unwrap().jar));
        for spec in [spec(GBASE_URL, 813_042, archive_spec().sha256), spec(INCEPTOR_URL, 10_719_927, archive_spec().sha256), spec("https://g1.cdn.gbase.cn/other.jar", 813_043, archive_spec().sha256)] { assert!(!admitted(&spec)); }
        for url in [GBASE_URL, INCEPTOR_URL] { assert!(!super::super::super::trusted_url(&reqwest::Url::parse(url).unwrap()), "do not widen general downloader's allowlist"); }
        assert!(admitted(&vendor("argodb").unwrap().jar));
        assert_ne!(vendor("argodb").unwrap().jar.sha256, vendor("inceptor").unwrap().jar.sha256);
        assert_eq!(vendor("argodb").unwrap().class, "io.transwarp.jdbc.QuarkDriver");
        assert!(vendor("gbase8s").is_none());
    }
}
