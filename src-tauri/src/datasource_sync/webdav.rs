//! 受限 WebDAV 客户端：只读发现、条件写、写后完整内容校验。
use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use quick_xml::{events::Event, Reader};
use reqwest::{
    header::{HeaderMap, HeaderName, HeaderValue},
    Client, Method, Response, StatusCode,
};
use url::Url;

use super::crypto;

pub(super) type Cooldown = Arc<Mutex<Option<Instant>>>;
const MAX_CONTROL_BYTES: usize = 64 * 1024;
const XML: &[u8] = b"<?xml version=\"1.0\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:resourcetype/></d:prop></d:propfind>";

pub(super) fn normalize_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(String::new());
    }
    if input.len() > 4096 || input.chars().any(char::is_control) {
        return Err("WebDAV URL 无效".into());
    }
    let mut url = Url::parse(input).map_err(|_| "WebDAV 地址必须是合法的 HTTP / HTTPS URL")?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || input
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("")
            .contains('@')
    {
        return Err("WebDAV URL 只允许 HTTP / HTTPS，不能含账号、密码、query 或 fragment".into());
    }
    let path = format!("{}/", url.path().trim_end_matches('/'));
    url.set_path(&path);
    Ok(url.to_string())
}

pub(super) fn parse_headers(text: &str) -> Result<HeaderMap, String> {
    if text.len() > 16 * 1024 || text.contains('\0') {
        return Err("额外请求头过大或包含无效字符".into());
    }
    let normalized = text.replace("\r\n", "\n");
    if normalized.contains('\r') {
        return Err("额外请求头包含非法 CR / LF".into());
    }
    let mut headers = HeaderMap::new();
    let mut seen = BTreeSet::new();
    for line in normalized
        .split('\n')
        .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
    {
        if line.starts_with([' ', '\t']) {
            return Err("额外请求头不允许折行或前导空白".into());
        }
        let (name, value) = line
            .split_once(':')
            .ok_or("额外请求头必须逐行使用 Name: value")?;
        let name = name.trim().to_ascii_lowercase();
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| "额外请求头名称无效")?;
        if !seen.insert(name.as_str().to_string()) {
            return Err("额外请求头名称重复（不区分大小写）".into());
        }
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "content-type"
                | "content-encoding"
                | "transfer-encoding"
                | "connection"
                | "accept-encoding"
                | "if-match"
                | "if-none-match"
                | "if-modified-since"
                | "if-unmodified-since"
                | "if-range"
                | "depth"
                | "destination"
                | "overwrite"
                | "range"
                | "te"
                | "trailer"
                | "expect"
                | "proxy-authorization"
                | "proxy-connection"
        ) {
            return Err("额外请求头不能覆盖 WebDAV 协议内置头".into());
        }
        let mut value =
            HeaderValue::from_str(value.trim()).map_err(|_| "额外请求头值无效或包含 CR / LF")?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    Ok(headers)
}

/// 与实际发出的 Authorization 完全一致：显式请求头优先，其他情况回退到 Basic。
/// 两处共同使用，避免同步身份与真实 HTTP 请求产生分歧。
fn effective_authorization(
    user: &str,
    password: &str,
    headers: &HeaderMap,
) -> Result<Option<HeaderValue>, String> {
    if let Some(auth) = headers.get("authorization") {
        return Ok(Some(auth.clone()));
    }
    if user.is_empty() && password.is_empty() {
        return Ok(None);
    }
    let mut auth = HeaderValue::from_str(&format!(
        "Basic {}",
        B64.encode(format!("{user}:{password}"))
    ))
    .map_err(|_| "WebDAV 认证字段无效")?;
    auth.set_sensitive(true);
    Ok(Some(auth))
}

/// 摘要只在本机加密状态中保存；认证头原文和 Basic 密码不进入预览、日志或基线。
/// URL、账号和实际有效的认证字段共同决定一个成功基线的身份。
pub(super) fn identity_fingerprint(
    url: &str,
    user: &str,
    password: &str,
    extra_headers: &str,
) -> Result<String, String> {
    let headers = parse_headers(extra_headers)?;
    let auth = effective_authorization(user, password, &headers)?;
    let material = serde_json::to_vec(&(
        "navkit-datasource-sync-auth-identity-v1",
        url,
        auth.as_ref().map(HeaderValue::as_bytes),
    ))
    .map_err(|_| "WebDAV 认证身份摘要生成失败")?;
    Ok(crypto::hash(&material))
}

pub(super) struct RemoteRead {
    pub bytes: Option<Vec<u8>>,
    pub etag: Option<String>,
    pub missing_dirs: Vec<Url>,
}

pub(super) struct Dav {
    client: Client,
    base: Url,
    headers: HeaderMap,
    cooldown: Cooldown,
    deadline: Instant,
}

impl Dav {
    #[cfg(test)]
    pub(super) fn set_total_timeout_for_test(&mut self, timeout: Duration) {
        self.deadline = Instant::now() + timeout;
    }
    pub fn new(
        url: &str,
        user: &str,
        password: &str,
        headers: &str,
        cooldown: Cooldown,
    ) -> Result<Self, String> {
        let normalized = normalize_url(url)?;
        if normalized.is_empty() {
            return Err("请先配置 WebDAV 地址".into());
        }
        let base = Url::parse(&normalized).map_err(|_| "WebDAV 地址无效")?;
        let mut headers = parse_headers(headers)?;
        if let Some(auth) = effective_authorization(user, password, &headers)? {
            headers.insert("authorization", auth);
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "初始化 WebDAV 客户端失败")?;
        Ok(Self {
            client,
            base,
            headers,
            cooldown,
            deadline: Instant::now() + Duration::from_secs(120),
        })
    }

    fn folder(&self) -> Url {
        self.base.join("navkit-sync/").expect("固定相对路径")
    }
    pub fn target(&self) -> Url {
        self.base
            .join("navkit-sync/datasources.enc")
            .expect("固定相对路径")
    }

    async fn request(
        &self,
        method: Method,
        url: Url,
        body: Option<&[u8]>,
        protocol: HeaderMap,
    ) -> Result<Response, String> {
        if let Some(until) = *self.cooldown.lock().map_err(|_| "WebDAV 冷却锁不可用")? {
            if until > Instant::now() {
                return Err(format!(
                    "WebDAV 正在等待 Retry-After，请 {} 秒后重试",
                    (until - Instant::now()).as_secs() + 1
                ));
            }
        }
        let mut url = url;
        for redirect in 0..=3 {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err("WebDAV 网络阶段超过总时限，请重新预览".into());
            }
            if url.origin() != self.base.origin()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err("WebDAV 重定向改变 origin 或包含认证参数，已拒绝发送凭据".into());
            }
            let mut req = self
                .client
                .request(method.clone(), url.clone())
                .headers(self.headers.clone())
                .headers(protocol.clone())
                .header("accept-encoding", "identity");
            req = req.timeout(remaining.min(Duration::from_secs(30)));
            if let Some(body) = body {
                req = req.body(body.to_vec());
            }
            let response = req.send().await.map_err(|e| {
                if e.is_timeout() {
                    "WebDAV 请求超时"
                } else {
                    "WebDAV 网络传输失败"
                }
                .to_string()
            })?;
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED {
                return Err("WebDAV 认证失败（HTTP 401）".into());
            }
            if status == StatusCode::FORBIDDEN {
                return Err("WebDAV 权限不足（HTTP 403），不能当作空目录".into());
            }
            if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE
            {
                let seconds = retry_after(response.headers());
                *self.cooldown.lock().map_err(|_| "WebDAV 冷却锁不可用")? =
                    Some(Instant::now() + Duration::from_secs(seconds));
                return Err(format!(
                    "WebDAV 服务器限流或繁忙（HTTP {}），请 {} 秒后重试",
                    status.as_u16(),
                    seconds
                ));
            }
            if status.is_redirection() {
                if !matches!(method.as_str(), "GET" | "PROPFIND") {
                    return Err("WebDAV 写入目标发生重定向，请修正地址并重新预览".into());
                }
                if redirect == 3 {
                    return Err("WebDAV 重定向次数过多".into());
                }
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or("WebDAV 重定向缺少有效地址")?;
                url = url.join(location).map_err(|_| "WebDAV 重定向地址无效")?;
                continue;
            }
            return Ok(response);
        }
        Err("WebDAV 重定向失败".into())
    }

    pub async fn read(&self) -> Result<RemoteRead, String> {
        let response = self
            .request(Method::GET, self.target(), None, HeaderMap::new())
            .await?;
        match response.status() {
            StatusCode::OK => {
                let etag = strong_etag(response.headers());
                let bytes = read_limited(response, crypto::MAX_REMOTE_BYTES).await?;
                Ok(RemoteRead {
                    bytes: Some(bytes),
                    etag,
                    missing_dirs: vec![],
                })
            }
            StatusCode::NOT_FOUND => {
                // 只接受经过父目录验证的 404；403、409、410 均不推断为空。
                let mut missing_dirs = vec![];
                let mut at = self.folder();
                loop {
                    match self.collection(&at).await? {
                        Collection::Exists => break,
                        Collection::Missing => {
                            if missing_dirs.len() >= 16 {
                                return Err(
                                    "WebDAV 缺失目录层级过深，请先创建基础目录再预览".into()
                                );
                            }
                            if at.path() == "/" {
                                return Err("WebDAV 服务器根目录不存在（HTTP 404）".into());
                            }
                            missing_dirs.push(at.clone());
                            let parent = at
                                .path()
                                .trim_end_matches('/')
                                .rsplit_once('/')
                                .map(|(p, _)| format!("{p}/"))
                                .unwrap_or_else(|| "/".into());
                            at.set_path(&parent);
                        }
                    }
                }
                missing_dirs.reverse();
                Ok(RemoteRead {
                    bytes: None,
                    etag: None,
                    missing_dirs,
                })
            }
            status => Err(format!(
                "读取 WebDAV 数据源快照失败（HTTP {}）",
                status.as_u16()
            )),
        }
    }

    pub async fn test(&self) -> Result<String, String> {
        let read = self.read().await?;
        if let Some(bytes) = &read.bytes {
            crypto::inspect_envelope(bytes)?;
            return Ok(if read.etag.is_some() {
                "WebDAV 只读测试成功，数据源快照可读"
            } else {
                "WebDAV 快照可读，但缺少强 ETag，无法安全更新现有快照"
            }
            .into());
        }
        if read
            .missing_dirs
            .iter()
            .any(|u| u.path() == self.base.path())
        {
            return Err("WebDAV 基础目录不存在（HTTP 404）；请先创建父目录或修正地址".into());
        }
        Ok("WebDAV 只读测试成功，父目录存在，远端数据源快照尚未创建".into())
    }

    async fn collection(&self, url: &Url) -> Result<Collection, String> {
        let mut headers = HeaderMap::new();
        headers.insert("depth", HeaderValue::from_static("0"));
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/xml; charset=utf-8"),
        );
        let response = self
            .request(
                Method::from_bytes(b"PROPFIND").expect("固定方法"),
                url.clone(),
                Some(XML),
                headers,
            )
            .await?;
        match response.status() {
            StatusCode::NOT_FOUND => Ok(Collection::Missing),
            StatusCode::MULTI_STATUS | StatusCode::OK => {
                let body = read_limited(response, MAX_CONTROL_BYTES).await?;
                parse_collection(&body, url)
            }
            status => Err(format!("WebDAV 目录验证失败（HTTP {}）", status.as_u16())),
        }
    }

    pub async fn put_verified(
        &self,
        observed: &RemoteRead,
        bytes: &[u8],
    ) -> Result<RemoteRead, String> {
        if bytes.len() > crypto::MAX_REMOTE_BYTES {
            return Err("WebDAV 上传文件过大".into());
        }
        if observed.bytes.is_some() && observed.etag.is_none() {
            return Err("远端未提供可靠的强 ETag，已拒绝覆盖；不能使用无条件 PUT".into());
        }
        for dir in &observed.missing_dirs {
            let response = self
                .request(
                    Method::from_bytes(b"MKCOL").expect("固定方法"),
                    dir.clone(),
                    None,
                    HeaderMap::new(),
                )
                .await?;
            match response.status().as_u16() {
                200 | 201 | 204 | 405 => {
                    if !matches!(self.collection(dir).await?, Collection::Exists) {
                        return Err("MKCOL 后目录验证失败，已停止写入".into());
                    }
                }
                status => {
                    return Err(format!(
                        "创建 WebDAV 同步目录失败（HTTP {status}），请重新预览"
                    ))
                }
            }
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            HeaderValue::from_static("application/octet-stream"),
        );
        if let Some(etag) = &observed.etag {
            headers.insert(
                "if-match",
                HeaderValue::from_str(etag).map_err(|_| "远端 ETag 无效")?,
            );
        } else {
            headers.insert("if-none-match", HeaderValue::from_static("*"));
        }
        let response = self
            .request(Method::PUT, self.target(), Some(bytes), headers)
            .await?;
        if response.status() == StatusCode::PRECONDITION_FAILED {
            return Err("远端已改变（HTTP 412），未覆盖；请重新预览".into());
        }
        if !matches!(response.status().as_u16(), 200 | 201 | 204) {
            return Err(format!(
                "WebDAV 条件写入失败（HTTP {}）",
                response.status().as_u16()
            ));
        }
        drop(response);
        let kept = self.read().await?;
        let data = kept
            .bytes
            .as_ref()
            .ok_or("写后远端快照不存在，未推进基线")?;
        if crypto::hash(data) != crypto::hash(bytes) {
            return Err("写后内容 SHA-256 校验失败，远端可能被截断或改变；未推进基线".into());
        }
        if kept.etag.is_none() {
            return Err("写后缺少强 ETag，无法保证后续条件写；未推进基线".into());
        }
        Ok(kept)
    }
}

pub(super) fn strong_etag(headers: &HeaderMap) -> Option<String> {
    if headers.get_all("etag").iter().count() != 1 {
        return None;
    }
    let etag = headers.get("etag")?.to_str().ok()?.trim();
    if etag.len() < 2
        || !etag.starts_with('"')
        || !etag.ends_with('"')
        || etag[1..etag.len() - 1]
            .bytes()
            .any(|b| b == b'"' || b < 0x21 || b > 0x7e)
    {
        return None;
    }
    Some(etag.to_string())
}

fn retry_after(headers: &HeaderMap) -> u64 {
    let value = headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    value
        .parse::<u64>()
        .ok()
        .or_else(|| {
            chrono::DateTime::parse_from_rfc2822(value)
                .ok()
                .map(|time| (time.timestamp() - chrono::Utc::now().timestamp()).max(1) as u64)
        })
        .unwrap_or(60)
        .clamp(1, 86400)
}

async fn read_limited(mut response: Response, limit: usize) -> Result<Vec<u8>, String> {
    let mime = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if mime.contains("text/html") || mime.contains("xhtml") {
        return Err("WebDAV 返回 HTML 登录页面，请检查 WebDAV 专用地址和认证方式".into());
    }
    if response.headers().get("content-encoding").is_some_and(|v| {
        !v.to_str()
            .unwrap_or("")
            .trim()
            .eq_ignore_ascii_case("identity")
    }) {
        return Err("WebDAV 未遵守 Accept-Encoding: identity，已拒绝使用可能改变的 ETag".into());
    }
    let expected = response.content_length();
    if expected.is_some_and(|n| n > limit as u64) {
        return Err("WebDAV 响应超过大小上限".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "WebDAV 响应被截断或传输失败")?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err("WebDAV 响应超过大小上限".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    if expected.is_some_and(|n| n != bytes.len() as u64) {
        return Err("WebDAV 响应长度不完整".into());
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)])
        .trim_start_matches('\u{feff}')
        .trim_start()
        .to_ascii_lowercase();
    let head = if head.starts_with("<?xml") {
        head.split_once("?>")
            .map(|(_, rest)| rest.trim_start())
            .unwrap_or(&head)
    } else {
        &head
    };
    if [
        "<!doctype html",
        "<html",
        "<head",
        "<body",
        "<form",
        "<script",
        "<meta",
        "<title",
    ]
    .iter()
    .any(|prefix| head.starts_with(prefix))
    {
        return Err("WebDAV 返回 HTML 登录页面，已拒绝把页面当作快照或目录".into());
    }
    Ok(bytes)
}

#[derive(Clone, Copy)]
enum Collection {
    Exists,
    Missing,
}

#[cfg(test)]
pub(super) fn validate_collection_for_test(bytes: &[u8], url: &Url) -> Result<(), String> {
    parse_collection(bytes, url).map(|_| ())
}

/// 只接受请求目标的 DAV response；propstat 404 不是目录 404。
fn parse_collection(bytes: &[u8], url: &Url) -> Result<Collection, String> {
    let mut reader = Reader::from_reader(bytes);
    let mut buffer = Vec::new();
    let mut depth = 0usize;
    let mut multistatus = false;
    let mut in_response = false;
    let mut in_propstat = false;
    let mut node = String::new();
    let mut href = String::new();
    let mut response_status = None;
    let mut prop_status = None;
    let mut prop_collection = false;
    let mut is_collection = false;
    let mut found = None;
    loop {
        match reader
            .read_event_into(&mut buffer)
            .map_err(|_| "WebDAV 目录 XML 损坏")?
        {
            Event::Start(e) => {
                depth += 1;
                node = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                match node.as_str() {
                    "multistatus" => multistatus = true,
                    "response" => {
                        in_response = true;
                        href.clear();
                        response_status = None;
                        is_collection = false;
                    }
                    "propstat" => {
                        in_propstat = true;
                        prop_status = None;
                        prop_collection = false;
                    }
                    "collection" if in_propstat => prop_collection = true,
                    _ => {}
                }
            }
            Event::Empty(e) if e.local_name().as_ref() == b"collection" && in_propstat => {
                prop_collection = true
            }
            Event::Text(e) if in_response => {
                let raw = String::from_utf8_lossy(e.as_ref());
                let text =
                    quick_xml::escape::unescape(&raw).map_err(|_| "WebDAV 目录 XML 文本无效")?;
                if node == "href" {
                    href.push_str(&text);
                }
                if node == "status" {
                    let status = text
                        .split_whitespace()
                        .nth(1)
                        .and_then(|n| n.parse::<u16>().ok());
                    if in_propstat {
                        prop_status = status;
                    } else {
                        response_status = status;
                    }
                }
            }
            Event::End(e) => {
                if depth == 0 {
                    return Err("WebDAV 目录 XML 结构无效".into());
                }
                depth -= 1;
                match e.local_name().as_ref() {
                    b"propstat" => {
                        if prop_status == Some(200) && prop_collection {
                            is_collection = true;
                        }
                        in_propstat = false;
                    }
                    b"response" => {
                        let target = url.join(href.trim()).map_err(|_| "WebDAV 目录 href 无效")?;
                        if target.origin() == url.origin()
                            && target.path().trim_end_matches('/')
                                == url.path().trim_end_matches('/')
                        {
                            if found.is_some() {
                                return Err("WebDAV 目录响应包含重复目标".into());
                            }
                            found = Some(match response_status {
                                Some(404) => Collection::Missing,
                                Some(401) => return Err("WebDAV 目录认证失败（HTTP 401）".into()),
                                Some(403) => return Err("WebDAV 目录权限不足（HTTP 403）".into()),
                                Some(n) if n != 200 => {
                                    return Err(format!("WebDAV 目录验证失败（HTTP {n}）"))
                                }
                                _ if is_collection => Collection::Exists,
                                _ => return Err("WebDAV 目标不是已验证的目录，已拒绝写入".into()),
                            });
                        }
                        in_response = false;
                    }
                    _ => {}
                }
                node.clear();
            }
            Event::DocType(_) => return Err("WebDAV 目录 XML 不允许 DTD".into()),
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }
    if depth != 0 || !multistatus {
        return Err("WebDAV 目录响应不是完整 DAV multistatus".into());
    }
    found.ok_or_else(|| "WebDAV 目录响应没有请求目标".into())
}
