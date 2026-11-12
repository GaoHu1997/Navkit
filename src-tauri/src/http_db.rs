//! HTTP REST 类数据源的通用适配层。
//!
//! 覆盖：Elasticsearch / Easysearch、Solr、CouchDB、Meilisearch、Qdrant、Milvus、
//! Weaviate、ChromaDB、Neo4j、TDengine、IoTDB、InfluxDB、VictoriaMetrics、Turso、
//! Cloudflare D1、Pulsar、RabbitMQ、Nacos、Consul。
//!
//! 每种数据源实现同一个 [`HttpAdapter`] trait（连通验证 / 容器 / 对象 / 行浏览 /
//! 自由执行），由 [`HttpDb`] 按目录 `catalog.http` 的适配器 id 分发。
//! 「容器」对应树中的数据库层，「对象」对应表层；均返回文本化结果，与
//! sqlx / ODBC 路径形态一致。

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;

use crate::models::{ConnectionRequest, DbKind};

/// 文本化结果集（列名 + 行），与 MssqlResult / OdbcResult 同形态。
pub struct HttpRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
}

/// 列元数据（顺序与结构树/表结构面板一致）。
#[derive(Debug, Clone)]
pub struct HttpColumn {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub primary_key: bool,
}

/// HTTP 适配器统一接口。
#[path = "jdbc_mutation.rs"]
pub mod relational;

#[async_trait]
pub trait HttpAdapter: Send + Sync {
    /// Only the certified SQL tunnel exposes owned cancellation/drain to C1.
    fn tunnel_runtime(&self) -> Option<std::sync::Arc<crate::http_tunnel::TunnelRuntime>> { None }
    async fn shutdown(&self) {}
    /// Stateful JDBC adapters select namespaces within one serialized live session.
    fn shares_session_namespace(&self) -> bool { false }
    /// PL-20: only a certified relational adapter can override these methods.
    /// Ordinary REST/MQ adapters MUST remain unsupported, not a SQL fallback.
    async fn relational_capabilities(&self) -> Result<relational::JdbcCapabilities> {
        bail!("UNSUPPORTED_CAPABILITY: this adapter has no certified relational mutation channel")
    }
    async fn relational_metadata(&self, _object: &relational::JdbcObjectId) -> Result<relational::JdbcMetadataSnapshot> {
        bail!("UNSUPPORTED_CAPABILITY: this adapter has no complete relational metadata snapshot")
    }
    async fn apply_row_plan(&self, _plan: &relational::JdbcRowPlan) -> Result<relational::JdbcMutationOutcome> {
        bail!("UNSUPPORTED_CAPABILITY: this adapter cannot execute an owned typed row plan")
    }
    /// 连通性验证，返回版本串。
    async fn ping(&self) -> Result<String>;
    /// 顶级容器（树：连接下的「数据库」层），返回 (名称, 描述)。
    async fn containers(&self) -> Result<Vec<(String, String)>>;
    /// 容器内的对象（「表」层）。
    async fn objects(&self, container: &str) -> Result<Vec<String>>;
    /// 可选：对象类型分类（(名称, "table" | "view")）。支持视图区分的关系型
    /// 适配器（H2 / JDBC 等）实现此方法；未实现返回 None，调用方按全部表处理。
    async fn object_types(&self, _container: &str) -> Result<Option<Vec<(String, String)>>> {
        Ok(None)
    }
    /// 浏览对象数据（分页）。
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows>;
    /// 查询标签页执行（自由文本）。
    async fn run(&self, container: &str, object: &str, sql: &str) -> Result<HttpRows>;
    /// 可选：表的真实列元数据。无元数据的适配器返回 None，由调用方用行样本推导。
    async fn table_columns(&self, _container: &str, _object: &str) -> Result<Option<Vec<HttpColumn>>> {
        Ok(None)
    }
}

/// 连接上下文：HTTP 客户端 + 认证 + 基地址。
pub struct Ctx {
    http: reqwest::Client,
    base: String,
    user: String,
    pass: String,
    params: Vec<(String, String)>,
    /// 附加默认头（IoTDB / Turso / D1 等自定义认证方式）。
    headers: reqwest::header::HeaderMap,
}

impl Ctx {
    fn param(&self, name: &str) -> Option<String> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }

    fn token(&self) -> Option<String> {
        self.param("token")
            .or_else(|| self.param("api_token"))
            .or_else(|| self.param("api_key"))
            .filter(|s| !s.is_empty())
    }

    /// 自定义认证头优先；只有显式 token 才用 Bearer，普通密码绝不覆盖 Basic。
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = if path.starts_with("https://") || path.starts_with("http://") {
            path.to_string()
        } else {
            format!("{}{}", self.base, path)
        };
        let req = self.http.request(method, &url).headers(self.headers.clone());
        if self.headers.contains_key(reqwest::header::AUTHORIZATION)
            || self.headers.contains_key("api-key")
            || self.headers.contains_key("x-consul-token")
        {
            req
        } else if let Some(token) = self.token() {
            req.bearer_auth(token)
        } else if !self.user.is_empty() || !self.pass.is_empty() {
            req.basic_auth(&self.user, Some(&self.pass))
        } else {
            req
        }
    }

    async fn response(resp: reqwest::Response) -> Result<(u16, Value)> {
        let status = resp.status().as_u16();
        let text = resp.text().await.context("读取 HTTP 响应失败")?;
        let json = serde_json::from_str(&text)
            .unwrap_or(Value::String(text.trim().to_string()));
        Ok((status, json))
    }

    /// TDengine 等接口要求裸 SQL，而不是 JSON 字符串。
    async fn post_text(&self, path: &str, body: &str) -> Result<(u16, Value)> {
        let resp = self.request(reqwest::Method::POST, path)
            .header(reqwest::header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(body.to_string()).send().await.context("HTTP SQL 请求失败")?;
        Self::response(resp).await
    }

    /// GET JSON；返回 (状态, JSON)。
    async fn get_json(&self, path: &str) -> Result<(u16, Value)> {
        self.send_json(reqwest::Method::GET, path, None).await
    }

    /// POST JSON 请求体。
    async fn post_json(&self, path: &str, body: Value) -> Result<(u16, Value)> {
        self.send_json(reqwest::Method::POST, path, Some(body)).await
    }

    async fn send_json(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(u16, Value)> {
        let mut req = self.request(method, path);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let resp = req.send().await.with_context(|| format!("请求 {} 失败", path))?;
        Self::response(resp).await
    }

    fn err(status: u16, body: &Value, action: &str) -> anyhow::Error {
        anyhow!("{} 失败（HTTP {}）：{}", action, status, jbody(body))
    }

    /// 校验 HTTP 2xx，否则抛错。
    fn check(status: u16, body: &Value, action: &str) -> Result<()> {
        if (200..300).contains(&status) {
            if body.as_str().is_some_and(|s| s.trim_start().starts_with('<')) {
                bail!("{} 返回网页而非数据源响应：{}", action, jbody(body));
            }
            if body.as_object().is_some_and(|o| o.is_empty())
                || body.get("error").is_some_and(|v| !v.is_null())
                || body.get("errors").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
                || body.get("status").is_some_and(|v| v.as_str() == Some("error") || v.get("error").is_some_and(|e| !e.is_null()))
                || body.get("code").is_some_and(|v| v.is_string()) {
                bail!("{} 返回业务错误或缺失结果结构：{}", action, jbody(body));
            }
            Ok(())
        } else {
            Err(Self::err(status, body, action))
        }
    }
}

/// 错误体摘要（截断）。
fn jbody(v: &Value) -> String {
    let s = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let mut chars = s.chars();
    let prefix: String = chars.by_ref().take(300).collect();
    if chars.next().is_some() { format!("{}…", prefix) } else { prefix }
}

/// JSON 值 → 文本。
pub fn jtext(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// 对象数组 → 统一列（并集）+ 行。
fn obj_cols_rows(items: &[Value]) -> HttpRows {
    let mut cols: Vec<String> = Vec::new();
    for it in items {
        if let Value::Object(map) = it {
            for k in map.keys() {
                if !cols.contains(k) {
                    cols.push(k.clone());
                }
            }
        }
    }
    let rows = items
        .iter()
        .map(|it| {
            cols.iter()
                .map(|c| it.get(c).and_then(jtext))
                .collect::<Vec<Option<String>>>()
        })
        .collect();
    HttpRows { columns: cols, rows }
}

/// JSON 数组元素取字段字符串列表。
fn str_list(v: &Value, path: &[&str]) -> Vec<String> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p).unwrap_or(&Value::Null);
    }
    cur.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    other => jtext(other),
                })
                .collect()
        })
        .unwrap_or_default()
}

// =============================================================================
// Elasticsearch / Easysearch
// =============================================================================

struct EsAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for EsAdapter {
    async fn ping(&self) -> Result<String> {
        let (s, j) = self.ctx.get_json("/").await?;
        Self::ctx_check(s, &j)?;
        let v = j
            .pointer("/version/number")
            .and_then(jtext)
            .unwrap_or_default();
        Ok(format!("Elasticsearch {}", v))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        Ok(vec![("cluster".into(), "集群".into())])
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        let (s, j) = self.ctx.get_json("/_cat/indices?format=json").await?;
        Self::ctx_check(s, &j)?;
        Ok(j.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("index").and_then(jtext))
                    .collect()
            })
            .unwrap_or_default())
    }
    async fn rows(
        &self,
        _container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let index = if object.is_empty() { "_all" } else { object };
        let body = serde_json::json!({"from": offset, "size": limit.min(1000), "query": {"match_all": {}}});
        let (s, j) = self
            .ctx
            .post_json(&format!("/{}/_search", urlencode(index)), body)
            .await?;
        Self::ctx_check(s, &j)?;
        Ok(hits_rows(&j))
    }
    async fn run(&self, _container: &str, object: &str, sql: &str) -> Result<HttpRows> {
        let index = if object.is_empty() { "_all" } else { object };
        let path = format!("/{}/_search", urlencode(index));
        let sql = sql.trim();
        if sql.starts_with('{') {
            let body: Value = serde_json::from_str(sql).map_err(|e| anyhow!("DSL 不是合法 JSON: {e}"))?;
            let (s, j) = self.ctx.post_json(&path, body).await?;
            Self::ctx_check(s, &j)?;
            Ok(hits_rows(&j))
        } else {
            let (s, j) = self
                .ctx
                .get_json(&format!("{}?q={}&size=100", path, urlencode(sql)))
                .await?;
            Self::ctx_check(s, &j)?;
            Ok(hits_rows(&j))
        }
    }
}

impl EsAdapter {
    fn ctx_check(s: u16, j: &Value) -> Result<()> {
        Ctx::check(s, j, "Elasticsearch 请求")
    }
}

/// Elasticsearch _search 命中结果 → 行（_id + _source 字段）。
fn hits_rows(j: &Value) -> HttpRows {
    let hits = j
        .pointer("/hits/hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut items = Vec::with_capacity(hits.len());
    for h in hits {
        let mut row = serde_json::Map::new();
        if let Some(id) = h.get("_id") {
            row.insert("_id".into(), id.clone());
        }
        if let Some(Value::Object(src)) = h.get("_source") {
            for (k, v) in src {
                row.insert(k.clone(), v.clone());
            }
        }
        items.push(Value::Object(row));
    }
    obj_cols_rows(&items)
}

// =============================================================================
// Apache Solr
// =============================================================================

struct SolrAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for SolrAdapter {
    async fn ping(&self) -> Result<String> {
        let (s, j) = self
            .ctx
            .get_json("/solr/admin/cores?action=STATUS&wt=json")
            .await?;
        Ctx::check(s, &j, "Solr 请求")?;
        let n = j
            .pointer("/status")
            .and_then(Value::as_object)
            .map(|m| m.len())
            .unwrap_or(0);
        Ok(format!("Apache Solr（{} 个 Core）", n))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self
            .ctx
            .get_json("/solr/admin/cores?action=STATUS&wt=json")
            .await?;
        Ctx::check(s, &j, "Solr 请求")?;
        Ok(j.pointer("/status")
            .and_then(Value::as_object)
            .map(|m| m.keys().map(|k| (k.clone(), k.clone())).collect())
            .unwrap_or_default())
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec!["docs".into()])
    }
    async fn rows(
        &self,
        container: &str,
        _object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let (s, j) = self
            .ctx
            .get_json(&format!(
                "/solr/{}/select?q=*:*&wt=json&rows={}&start={}",
                urlencode(container),
                limit.min(1000),
                offset
            ))
            .await?;
        Ctx::check(s, &j, "Solr 查询")?;
        let docs = j
            .pointer("/response/docs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(obj_cols_rows(&docs))
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        if sql.get(..6).is_some_and(|prefix| prefix.eq_ignore_ascii_case("select")) {
            let (s, j) = self
                .ctx
                .get_json(&format!(
                    "/solr/{}/sql?stmt={}&wt=json",
                    urlencode(container),
                    urlencode(sql)
                ))
                .await?;
            Ctx::check(s, &j, "Solr SQL（需启用 Streaming Expression）")?;
            let docs = j
                .pointer("/response/docs")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Ok(obj_cols_rows(&docs))
        } else {
            bail!("Solr 查询请输入 SELECT SQL；当前接口不执行其它命令");
        }
    }
}

// =============================================================================
// Apache CouchDB
// =============================================================================

struct CouchDbAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for CouchDbAdapter {
    async fn ping(&self) -> Result<String> {
        self.containers().await?;
        Ok("Apache CouchDB（已验证列库权限）".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self.ctx.get_json("/_all_dbs").await?;
        Ctx::check(s, &j, "CouchDB 列库")?;
        j.as_array().ok_or_else(|| anyhow!("CouchDB 列库响应不是数组"))?
            .iter().map(|v| v.as_str().map(|name| (name.to_string(), name.to_string())).ok_or_else(|| anyhow!("CouchDB 数据库名称不是字符串"))).collect()
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec!["documents".into()])
    }
    async fn rows(
        &self,
        container: &str,
        _object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let (s, j) = self
            .ctx
            .get_json(&format!(
                "/{}/_all_docs?include_docs=true&limit={}&skip={}",
                urlencode(container),
                limit.min(1000),
                offset
            ))
            .await?;
        Ctx::check(s, &j, "CouchDB 浏览文档")?;
        let mut items = Vec::new();
        for r in j
            .get("rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(Value::Object(doc)) = r.get("doc") {
                items.push(Value::Object(doc.clone()));
            }
        }
        Ok(obj_cols_rows(&items))
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        if !sql.starts_with('{') {
            bail!("CouchDB 查询请输入 Mango 选择器 JSON，例如 {{\"selector\": {{\"type\": \"user\"}}}}");
        }
        let parsed: Value =
            serde_json::from_str(sql).map_err(|e| anyhow!("Mango 不是合法 JSON: {e}"))?;
        let body = if parsed.get("selector").is_some() {
            parsed
        } else {
            serde_json::json!({"selector": parsed})
        };
        let (s, j) = self
            .ctx
            .post_json(&format!("/{}/_find", urlencode(container)), body)
            .await?;
        Ctx::check(s, &j, "CouchDB _find")?;
        let docs = j.get("docs").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(obj_cols_rows(&docs))
    }
}

// =============================================================================
// Meilisearch
// =============================================================================

struct MeiliAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for MeiliAdapter {
    async fn ping(&self) -> Result<String> {
        self.containers().await?;
        Ok("Meilisearch（已验证索引访问权限）".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self.ctx.get_json("/indexes?limit=100").await?;
        Ctx::check(s, &j, "Meilisearch 列索引")?;
        j.get("results").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Meilisearch 响应缺少 results 数组"))?
            .iter().map(|v| v.get("uid").and_then(Value::as_str).map(|name| (name.to_string(), name.to_string())).ok_or_else(|| anyhow!("Meilisearch 索引缺少 uid"))).collect()
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec!["documents".into()])
    }
    async fn rows(
        &self,
        container: &str,
        _object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let (s, j) = self
            .ctx
            .get_json(&format!(
                "/indexes/{}/documents?limit={}&offset={}",
                urlencode(container),
                limit.min(1000),
                offset
            ))
            .await?;
        Ctx::check(s, &j, "Meilisearch 浏览文档")?;
        let items = j.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(obj_cols_rows(&items))
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let (s, j) = if sql.trim().starts_with('{') {
            let body: Value = serde_json::from_str(sql).context("Meilisearch 搜索请求不是合法 JSON")?;
            self.ctx.post_json(&format!("/indexes/{}/search", urlencode(container)), body).await?
        } else {
            self.ctx.get_json(&format!("/indexes/{}/search?q={}&limit=100", urlencode(container), urlencode(sql.trim()))).await?
        };
        Ctx::check(s, &j, "Meilisearch 搜索")?;
        let items = j.get("hits").and_then(Value::as_array).ok_or_else(|| anyhow!("Meilisearch 搜索缺少 hits 数组"))?;
        Ok(obj_cols_rows(items))
    }
}

// =============================================================================
// Qdrant
// =============================================================================

struct QdrantAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for QdrantAdapter {
    async fn ping(&self) -> Result<String> {
        self.containers().await?;
        Ok("Qdrant（已验证集合访问权限）".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self.ctx.get_json("/collections").await?;
        Ctx::check(s, &j, "Qdrant 列集合")?;
        j.pointer("/result/collections").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Qdrant 响应缺少 result.collections"))?
            .iter().map(|v| v.get("name").and_then(Value::as_str).map(|name| (name.to_string(), name.to_string())).ok_or_else(|| anyhow!("Qdrant 集合缺少 name"))).collect()
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec!["points".into()])
    }
    async fn rows(
        &self,
        container: &str,
        _object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        self.scroll(container, &serde_json::json!({
            "limit": limit.min(100) as u64,
            "offset": if offset == 0 { Value::Null } else { Value::from(offset) },
            "with_payload": true
        }))
        .await
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        if sql.starts_with('{') {
            let mut body: Value =
                serde_json::from_str(sql).map_err(|e| anyhow!("请求体不是合法 JSON: {e}"))?;
            if body.get("with_payload").is_none() {
                body["with_payload"] = Value::Bool(true);
            }
            self.scroll(container, &body).await
        } else if sql.eq_ignore_ascii_case("INFO") {
            let (s, j) = self.ctx.get_json(&format!("/collections/{}", urlencode(container))).await?;
            Ctx::check(s, &j, "Qdrant 集合信息")?;
            let r = j.get("result").filter(|v| v.is_object()).ok_or_else(|| anyhow!("Qdrant 集合信息缺少 result"))?;
            Ok(obj_cols_rows(std::slice::from_ref(r)))
        } else { bail!("Qdrant 查询需要 JSON scroll 请求体或 INFO"); }
    }
}

impl QdrantAdapter {
    async fn scroll(&self, collection: &str, body: &Value) -> Result<HttpRows> {
        let (s, j) = self
            .ctx
            .post_json(&format!("/collections/{}/points/scroll", urlencode(collection)), body.clone())
            .await?;
        Ctx::check(s, &j, "Qdrant scroll")?;
        let points = j
            .pointer("/result/points")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut items = Vec::with_capacity(points.len());
        for p in points {
            let mut row = serde_json::Map::new();
            if let Some(id) = p.get("id") {
                row.insert("id".into(), id.clone());
            }
            if let Some(Value::Object(payload)) = p.get("payload") {
                for (k, v) in payload {
                    row.insert(k.clone(), v.clone());
                }
            }
            items.push(Value::Object(row));
        }
        Ok(obj_cols_rows(&items))
    }
}

// =============================================================================
// Milvus（RESTful v2）
// =============================================================================

fn check_milvus_code(j: &Value) -> Result<()> {
    match j.get("code").and_then(Value::as_i64) {
        Some(0) => Ok(()),
        Some(_) => bail!("Milvus API失败: {}", jbody(j)),
        None => bail!("Milvus 响应缺少业务 code: {}", jbody(j)),
    }
}

struct MilvusAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for MilvusAdapter {
    async fn ping(&self) -> Result<String> {
        self.containers().await?;
        Ok("Milvus（已验证数据库访问权限）".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self
            .ctx
            .post_json("/v2/vectordb/databases/list", serde_json::json!({}))
            .await?;
        Ctx::check(s, &j, "Milvus 列数据库")?;
        check_milvus_code(&j)?;
        let names = j.get("data").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Milvus 列数据库响应缺少 data 数组"))?;
        names.iter().map(|v| {
            let name = v.as_str().ok_or_else(|| anyhow!("Milvus 数据库名称不是字符串"))?.to_string();
            Ok((name.clone(), name))
        }).collect()
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let (s, j) = self
            .ctx
            .post_json(
                "/v2/vectordb/collections/list",
                serde_json::json!({"dbName": container}),
            )
            .await?;
        Ctx::check(s, &j, "Milvus 列集合")?;
        check_milvus_code(&j)?;
        j.get("data").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Milvus 列集合响应缺少 data 数组"))?
            .iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| anyhow!("Milvus 集合名称不是字符串"))).collect()
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        self.query(container, object, "", offset, limit).await
    }
    async fn run(&self, container: &str, object: &str, sql: &str) -> Result<HttpRows> {
        let text = sql.trim();
        if text.starts_with('{') {
            let value: Value = serde_json::from_str(text).context("Milvus 查询需合法 JSON")?;
            let collection = value.get("collectionName").and_then(Value::as_str)
                .filter(|name| !name.trim().is_empty()).ok_or_else(|| anyhow!("Milvus 查询需要 collectionName"))?;
            let filter = value.get("filter").map(|v| v.as_str().ok_or_else(|| anyhow!("Milvus filter 必须为字符串"))).transpose()?.unwrap_or("");
            return self.query(container, collection, filter,
                value.get("offset").and_then(Value::as_u64).unwrap_or(0),
                value.get("limit").and_then(Value::as_u64).unwrap_or(100)).await;
        }
        if object.is_empty() { bail!("Milvus 查询请使用含 collectionName 和 filter 的 JSON"); }
        self.query(container, object, text, 0, 100).await
    }
}

impl MilvusAdapter {
    async fn query(
        &self,
        db: &str,
        coll: &str,
        filter: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let body = serde_json::json!({
            "dbName": db,
            "collectionName": coll,
            "filter": filter,
            "offset": offset,
            "limit": limit.min(1000),
            "outputFields": ["*"]
        });
        let (s, j) = self
            .ctx
            .post_json("/v2/vectordb/entities/query", body)
            .await?;
        Ctx::check(s, &j, "Milvus 查询")?;
        check_milvus_code(&j)?;
        let items = j.get("data").and_then(Value::as_array).ok_or_else(|| anyhow!("Milvus 查询响应缺少 data 数组"))?;
        Ok(obj_cols_rows(items))
    }
}

// =============================================================================
// Weaviate
// =============================================================================

struct WeaviateAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for WeaviateAdapter {
    async fn ping(&self) -> Result<String> {
        self.containers().await?;
        Ok("Weaviate（已验证 schema 访问权限）".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self.ctx.get_json("/v1/schema").await?;
        Ctx::check(s, &j, "Weaviate 读取 schema")?;
        j.get("classes").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("Weaviate 响应缺少 classes 数组"))?
            .iter().map(|v| v.get("class").and_then(Value::as_str).map(|name| (name.to_string(), name.to_string())).ok_or_else(|| anyhow!("Weaviate class 缺少名称"))).collect()
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec!["objects".into()])
    }
    async fn rows(
        &self,
        container: &str,
        _object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        // /v1/objects 不支持 offset，取前 offset+limit 条后本地切片
        let fetch = (offset + limit).min(100);
        let (s, j) = self
            .ctx
            .get_json(&format!(
                "/v1/objects?class={}&limit={}&include=class",
                urlencode(container),
                fetch
            ))
            .await?;
        Ctx::check(s, &j, "Weaviate 浏览对象")?;
        let arr = j
            .get("objects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let slice: Vec<Value> = arr
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();
        let mut items = Vec::with_capacity(slice.len());
        for o in slice {
            let mut row = serde_json::Map::new();
            if let Some(id) = o.get("id") {
                row.insert("id".into(), id.clone());
            }
            if let Some(Value::Object(props)) = o.get("properties") {
                for (k, v) in props {
                    row.insert(k.clone(), v.clone());
                }
            }
            items.push(Value::Object(row));
        }
        Ok(obj_cols_rows(&items))
    }
    async fn run(&self, _container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        if sql.starts_with('{') {
            let body: Value =
                serde_json::from_str(sql).map_err(|e| anyhow!("GraphQL 不是合法 JSON: {e}"))?;
            let (s, j) = self.ctx.post_json("/v1/graphql", body).await?;
            Ctx::check(s, &j, "Weaviate GraphQL")?;
            let get = j.pointer("/data/Get").cloned().unwrap_or(Value::Null);
            let mut items: Vec<Value> = Vec::new();
            if let Value::Object(map) = &get {
                for (_, v) in map {
                    if let Value::Array(arr) = v {
                        items.extend(arr.iter().cloned());
                    }
                }
            }
            Ok(obj_cols_rows(&items))
        } else {
            let (s, j) = self
                .ctx
                .get_json(&format!(
                    "/v1/objects?class={}&limit=100&include=class",
                    urlencode(sql.trim())
                ))
                .await?;
            Ctx::check(s, &j, "Weaviate 浏览对象")?;
            let items = j.get("objects").and_then(Value::as_array).cloned().unwrap_or_default();
            Ok(obj_cols_rows(&items))
        }
    }
}

// =============================================================================
// ChromaDB（REST v1）
// =============================================================================

struct ChromaAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for ChromaAdapter {
    async fn ping(&self) -> Result<String> {
        let (s, j) = self.ctx.get_json("/api/v1/version").await?;
        Ctx::check(s, &j, "ChromaDB 请求")?;
        let v = match &j {
            Value::String(x) => x.clone(),
            other => jtext(other).unwrap_or_default(),
        };
        Ok(format!("ChromaDB {}", v))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self.ctx.get_json("/api/v1/collections").await?;
        Ctx::check(s, &j, "ChromaDB 列集合")?;
        Ok(j.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("name").and_then(jtext))
                    .map(|n| (n.clone(), n))
                    .collect()
            })
            .unwrap_or_default())
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec!["items".into()])
    }
    async fn rows(
        &self,
        container: &str,
        _object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let (s, info) = self
            .ctx
            .get_json(&format!("/api/v1/collections/{}", urlencode(container)))
            .await?;
        Ctx::check(s, &info, "ChromaDB 集合信息")?;
        let id = info.get("id").and_then(jtext).unwrap_or_default();
        let body = serde_json::json!({
            "limit": limit.min(1000),
            "offset": offset,
            "include": ["metadatas", "documents"]
        });
        let (s, j) = self
            .ctx
            .post_json(&format!("/api/v1/collections/{}/get", urlencode(&id)), body)
            .await?;
        Ctx::check(s, &j, "ChromaDB 浏览")?;
        let ids = str_list(&j, &["ids"]);
        let docs = j.get("documents").and_then(Value::as_array).cloned().unwrap_or_default();
        let metas = j.get("metadatas").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut items = Vec::with_capacity(ids.len());
        for (i, id) in ids.iter().enumerate() {
            let mut row = serde_json::Map::new();
            row.insert("id".into(), Value::String(id.clone()));
            if let Some(d) = docs.get(i) {
                row.insert("document".into(), d.clone());
            }
            if let Some(m) = metas.get(i) {
                row.insert("metadata".into(), m.clone());
            }
            items.push(Value::Object(row));
        }
        Ok(obj_cols_rows(&items))
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        if sql.starts_with('{') {
            let parsed: Value =
                serde_json::from_str(sql).map_err(|e| anyhow!("条件不是合法 JSON: {e}"))?;
            let where_clause = parsed.get("where").cloned().unwrap_or(parsed);
            let (s, info) = self
                .ctx
                .get_json(&format!("/api/v1/collections/{}", urlencode(container)))
                .await?;
            Ctx::check(s, &info, "ChromaDB 集合信息")?;
            let id = info.get("id").and_then(jtext).unwrap_or_default();
            let body = serde_json::json!({
                "where": where_clause,
                "limit": 100,
                "include": ["metadatas", "documents"]
            });
            let (s, j) = self
                .ctx
                .post_json(&format!("/api/v1/collections/{}/get", urlencode(&id)), body)
                .await?;
            Ctx::check(s, &j, "ChromaDB 查询")?;
            let ids = str_list(&j, &["ids"]);
            let docs = j.get("documents").and_then(Value::as_array).cloned().unwrap_or_default();
            let items = ids
                .iter()
                .enumerate()
                .map(|(i, id)| {
                    serde_json::json!({"id": id, "document": docs.get(i).cloned().unwrap_or(Value::Null)})
                })
                .collect::<Vec<_>>();
            Ok(obj_cols_rows(&items))
        } else {
            bail!("ChromaDB 查询需要含 where 条件的 JSON");
        }
    }
}

// =============================================================================
// Neo4j（HTTP 事务接口）
// =============================================================================

struct Neo4jAdapter {
    ctx: Ctx,
    db: String,
}

impl Neo4jAdapter {
    async fn cypher(&self, db: &str, statement: &str) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
        let db = if db.is_empty() { self.db.as_str() } else { db };
        let body = serde_json::json!({"statements": [{"statement": statement}]});
        let (s, j) = self
            .ctx
            .post_json(&format!("/db/{}/tx/commit", urlencode(db)), body)
            .await?;
        Ctx::check(s, &j, "Neo4j Cypher")?;
        if let Some(errs) = j.get("errors").and_then(Value::as_array) {
            if !errs.is_empty() {
                bail!("Neo4j 错误: {}", errs.first().map(jbody).unwrap_or_default());
            }
        }
        let results = j.get("results").and_then(Value::as_array).cloned().unwrap_or_default();
        let first = results.first().cloned().unwrap_or(Value::Null);
        let cols = str_list(&first, &["columns"]);
        let mut rows = Vec::new();
        for d in first
            .pointer("/data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(row) = d.get("row").and_then(Value::as_array) {
                rows.push(row.iter().map(jtext).collect::<Vec<Option<String>>>());
            }
        }
        Ok((cols, rows))
    }
}

#[async_trait]
impl HttpAdapter for Neo4jAdapter {
    async fn ping(&self) -> Result<String> {
        let (cols, rows) = self.cypher("", "RETURN 1").await?;
        if cols.is_empty() && rows.is_empty() {
            bail!("Neo4j 事务接口无响应");
        }
        Ok("Neo4j".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        match self.cypher("", "SHOW DATABASES").await {
            Ok((_, rows)) if !rows.is_empty() => Ok(rows
                .iter()
                .filter_map(|r| r.first().cloned().flatten())
                .map(|n| (n.clone(), n))
                .collect()),
            _ => Ok(vec![("neo4j".into(), "neo4j（默认库）".into())]),
        }
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let sql = format!(
            "MATCH (n) RETURN DISTINCT labels(n)[0] AS label, count(*) AS cnt ORDER BY label"
        );
        let (_, rows) = self.cypher(container, &sql).await?;
        Ok(rows
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .filter(|l| !l.is_empty())
            .collect())
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let sql = format!(
            "MATCH (n:`{}`) RETURN properties(n) AS props SKIP {} LIMIT {}",
            object, offset, limit
        );
        let (_, rows) = self.cypher(container, &sql).await?;
        let out = rows
            .into_iter()
            .map(|r| vec![r.last().cloned().flatten()])
            .collect();
        Ok(HttpRows {
            columns: vec!["(节点)".into()],
            rows: out,
        })
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let (cols, rows) = self.cypher(container, sql).await?;
        Ok(HttpRows { columns: cols, rows })
    }
}

// =============================================================================
// TDengine（REST，taosAdapter）
// =============================================================================

struct TdengineAdapter {
    ctx: Ctx,
}

impl TdengineAdapter {
    async fn sql(&self, db: &str, sql: &str) -> Result<HttpRows> {
        let path = if db.is_empty() {
            "/rest/sql".to_string()
        } else {
            format!("/rest/sql/{}", urlencode(db))
        };
        let (s, j) = self
            .ctx
            .post_text(&path, sql)
            .await?;
        Ctx::check(s, &j, "TDengine 请求")?;
        let code = j.get("code").and_then(Value::as_i64)
            .ok_or_else(|| anyhow!("TDengine 响应缺少 code: {}", jbody(&j)))?;
        if code != 0 {
            bail!("TDengine 错误 {}: {}", code, j.get("desc").and_then(jtext).unwrap_or_default());
        }
        let mut cols: Vec<String> = Vec::new();
        if let Some(meta) = j.get("column_meta").and_then(Value::as_array) {
            for c in meta {
                if let Some(name) = c.as_array().and_then(|a| a.first()).and_then(jtext) {
                    cols.push(name);
                }
            }
        }
        let mut rows = Vec::new();
        if let Some(data) = j.get("data").and_then(Value::as_array) {
            for r in data {
                if let Some(arr) = r.as_array() {
                    rows.push(arr.iter().map(jtext).collect::<Vec<Option<String>>>());
                }
            }
        }
        Ok(HttpRows { columns: cols, rows })
    }
}

#[async_trait]
impl HttpAdapter for TdengineAdapter {
    async fn ping(&self) -> Result<String> {
        let r = self.sql("", "SELECT SERVER_VERSION()").await?;
        let v = r
            .rows
            .first()
            .and_then(|r| r.first().cloned().flatten())
            .unwrap_or_default();
        Ok(format!("TDengine {}", v))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let r = self.sql("", "SHOW DATABASES").await?;
        Ok(r.rows
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .map(|n| (n.clone(), n))
            .collect())
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for q in [
            format!("SHOW {}.STABLES", container),
            format!("SHOW {}.TABLES", container),
        ] {
            let r = self.sql(container, &q).await?;
            for row in &r.rows {
                if let Some(name) = row.first().cloned().flatten() {
                    if !out.contains(&name) {
                        out.push(name);
                    }
                }
            }
        }
        Ok(out)
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let sql = if container.is_empty() {
            format!("SELECT * FROM {} LIMIT {} OFFSET {}", object, limit, offset)
        } else {
            format!(
                "SELECT * FROM {}.{} LIMIT {} OFFSET {}",
                container, object, limit, offset
            )
        };
        self.sql(container, &sql).await
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        self.sql(container, sql).await
    }
}

// =============================================================================
// Apache IoTDB（REST API v2）
// =============================================================================

struct IotdbAdapter {
    ctx: Ctx,
}

impl IotdbAdapter {
    async fn query(&self, sql: &str) -> Result<HttpRows> {
        let (s, j) = self
            .ctx
            .post_json("/rest/v2/query", serde_json::json!({"sql": sql}))
            .await?;
        Ctx::check(s, &j, "IoTDB 请求")?;
        check_iotdb_code(&j)?;
        let cols = if j.get("column_names").is_some() {
            str_list(&j, &["column_names"])
        } else {
            str_list(&j, &["expressions"])
        };
        if cols.is_empty() && j.get("values").is_none() {
            bail!("IoTDB 查询响应缺少结果结构: {}", jbody(&j));
        }
        let times = j.get("timestamps").or_else(|| j.get("times"))
            .and_then(Value::as_array);
        let cols_matrix: Vec<Vec<Value>> = j.get("values")
            .and_then(Value::as_array)
            .map(|a| a.iter().map(|c| c.as_array().cloned().unwrap_or_default()).collect())
            .unwrap_or_default();
        let count = cols_matrix.iter().map(Vec::len).max().unwrap_or(0)
            .max(times.map_or(0, Vec::len));
        let has_time = times.is_some();
        let mut columns = Vec::new();
        if has_time { columns.push("Time".into()); }
        columns.extend(cols);
        let rows = (0..count).map(|i| {
            let mut row = Vec::new();
            if let Some(times) = times { row.push(times.get(i).and_then(jtext)); }
            for col in &cols_matrix { row.push(col.get(i).and_then(jtext)); }
            row
        }).collect();
        Ok(HttpRows { columns, rows })
    }
}

#[async_trait]
impl HttpAdapter for IotdbAdapter {
    async fn ping(&self) -> Result<String> {
        self.query("SHOW DATABASES").await?;
        Ok("Apache IoTDB".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let r = self.query("SHOW DATABASES").await?;
        let index = r.columns.iter().position(|c| c.eq_ignore_ascii_case("database"))
            .unwrap_or(0);
        Ok(r.rows.iter().filter_map(|r| r.get(index).cloned().flatten())
            .map(|n| (n.clone(), n)).collect())
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let r = self
            .query(&format!("SHOW TIMESERIES {}.** LIMIT 500", container))
            .await?;
        let index = r.columns.iter().position(|c| c.eq_ignore_ascii_case("timeseries"))
            .ok_or_else(|| anyhow!("IoTDB SHOW TIMESERIES 响应缺少 Timeseries 列"))?;
        Ok(r.rows.iter().filter_map(|r| r.get(index).cloned().flatten()).collect())
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let sql = if object.is_empty() {
            format!("SELECT * FROM {} LIMIT {} OFFSET {}", container, limit.min(1000), offset)
        } else {
            let (device, measurement) = object.rsplit_once('.')
                .ok_or_else(|| anyhow!("IoTDB 时间序列必须包含设备路径"))?;
            format!("SELECT {} FROM {} LIMIT {} OFFSET {}", measurement, device, limit.min(1000), offset)
        };
        self.query(&sql).await
    }
    async fn run(&self, _container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let up = sql.trim().to_ascii_uppercase();
        if up.starts_with("INSERT")
            || up.starts_with("DELETE")
            || up.starts_with("CREATE")
            || up.starts_with("DROP")
            || up.starts_with("ALTER")
            || up.starts_with("SETTLE")
            || up.starts_with("FLUSH")
        {
            let (s, j) = self
                .ctx
                .post_json("/rest/v2/nonQuery", serde_json::json!({"sql": sql}))
                .await?;
            Ctx::check(s, &j, "IoTDB 执行")?;
            if j.get("code").is_none() { bail!("IoTDB 执行响应缺少 code"); }
            check_iotdb_code(&j)?;
            Ok(HttpRows {
                columns: vec!["result".into()],
                rows: vec![vec![jtext(&j)]],
            })
        } else {
            self.query(sql).await
        }
    }
}

// =============================================================================
// InfluxDB（InfluxQL /query 兼容接口）
// =============================================================================

struct InfluxAdapter {
    ctx: Ctx,
}

impl InfluxAdapter {
    async fn influxql(&self, db: &str, q: &str) -> Result<HttpRows> {
        let mut path = format!("/query?epoch=ms&q={}", urlencode(q));
        if !db.is_empty() {
            path.push_str(&format!("&db={}", urlencode(db)));
            if let Some(rp) = self.ctx.param("rp").filter(|rp| !rp.trim().is_empty()) {
                path.push_str(&format!("&rp={}", urlencode(&rp)));
            }
        }
        let (s, j) = self.ctx.get_json(&path).await?;
        Ctx::check(s, &j, "InfluxDB 查询")?;
        if let Some(err) = j.get("error").and_then(jtext) {
            bail!("InfluxDB 错误: {}", err);
        }
        let results = j.get("results").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("InfluxDB 响应缺少 results 数组"))?;
        for r in results {
            if let Some(err) = r.get("error").and_then(jtext) {
                bail!("InfluxDB 错误: {}", err);
            }
        }
        let series = results
            .first()
            .and_then(|r| r.get("series"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut rows_out: Vec<Vec<Option<String>>> = Vec::new();
        let mut columns: Vec<String> = Vec::new();
        for s in &series {
            let cols: Vec<String> = str_list(s, &["columns"]);
            if columns.is_empty() {
                columns = cols;
            }
            if let Some(values) = s.get("values").and_then(Value::as_array) {
                for v in values {
                    if let Some(arr) = v.as_array() {
                        rows_out.push(arr.iter().map(jtext).collect::<Vec<Option<String>>>());
                    }
                }
            }
        }
        Ok(HttpRows { columns, rows: rows_out })
    }
}

#[async_trait]
impl HttpAdapter for InfluxAdapter {
    async fn ping(&self) -> Result<String> {
        // /ping 是未认证健康检查，不能证明密码或查询权限正确。
        self.influxql("", "SHOW DATABASES").await?;
        let resp = self.ctx.request(reqwest::Method::GET, "/ping")
            .send().await.context("InfluxDB 请求失败")?;
        if !resp.status().is_success() {
            bail!("InfluxDB ping 失败（HTTP {}）", resp.status().as_u16());
        }
        let v = resp
            .headers()
            .get("X-Influxdb-Version")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .unwrap_or_default();
        Ok(format!("InfluxDB {}", v))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let r = self.influxql("", "SHOW DATABASES").await?;
        Ok(r.rows
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .map(|n| (n.clone(), n))
            .collect())
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let r = self.influxql(container, "SHOW MEASUREMENTS LIMIT 500").await?;
        Ok(r.rows
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .collect())
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let sql = format!(
            "SELECT * FROM \"{}\" LIMIT {} OFFSET {}",
            object.replace('"', "\"\""),
            limit,
            offset
        );
        self.influxql(container, &sql).await
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        self.influxql(container, sql).await
    }
}

// =============================================================================
// VictoriaMetrics
// =============================================================================

struct VmAdapter {
    ctx: Ctx,
}

impl VmAdapter {
    async fn instant(&self, query: &str) -> Result<HttpRows> {
        let (s, j) = self
            .ctx
            .get_json(&format!("/api/v1/query?query={}", urlencode(query)))
            .await?;
        Ctx::check(s, &j, "VictoriaMetrics 查询")?;
        let status = j.get("status").and_then(jtext).unwrap_or_default();
        if status != "success" {
            bail!("VictoriaMetrics 错误: {}", jbody(&j));
        }
        let data = j.get("data").and_then(Value::as_object)
            .ok_or_else(|| anyhow!("VictoriaMetrics 响应缺少 data 对象"))?;
        let result_type = data.get("resultType").and_then(Value::as_str)
            .ok_or_else(|| anyhow!("VictoriaMetrics 响应缺少 resultType"))?;
        let result = data.get("result").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("VictoriaMetrics 响应缺少 result 数组"))?;
        let columns = vec!["series".to_string(), "value".to_string(), "timestamp".to_string()];
        let sample = |value: &Value| -> Result<(Option<String>, Option<String>)> {
            let array = value.as_array().filter(|array| array.len() == 2)
                .ok_or_else(|| anyhow!("VictoriaMetrics sample 需要 timestamp/value 二元组"))?;
            if !array[0].is_number() || !array[1].is_string() {
                bail!("VictoriaMetrics sample timestamp/value 类型无效");
            }
            Ok((jtext(&array[1]), jtext(&array[0])))
        };
        let rows_out = match result_type {
            "scalar" | "string" => {
                let (value, timestamp) = sample(&Value::Array(result.clone()))?;
                vec![vec![None, value, timestamp]]
            },
            "vector" => result.iter().map(|item| {
                let metric = item.get("metric").filter(|value| value.is_object())
                    .ok_or_else(|| anyhow!("VictoriaMetrics vector 缺少 metric 对象"))?;
                let (value, timestamp) = sample(item.get("value")
                    .ok_or_else(|| anyhow!("VictoriaMetrics vector 缺少 value"))?)?;
                Ok(vec![Some(metric.to_string()), value, timestamp])
            }).collect::<Result<Vec<_>>>()?,
            other => bail!("VictoriaMetrics instant query 不支持 resultType={other}"),
        };
        Ok(HttpRows { columns, rows: rows_out })
    }
}

#[async_trait]
impl HttpAdapter for VmAdapter {
    async fn ping(&self) -> Result<String> {
        // /health 可匿名，不证明查询接口的认证与业务能力。
        self.instant("1").await?;
        Ok("VictoriaMetrics".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self
            .ctx
            .get_json("/api/v1/label/__name__/values")
            .await?;
        Ctx::check(s, &j, "VictoriaMetrics 列指标")?;
        if j.get("status").and_then(Value::as_str) != Some("success") {
            bail!("VictoriaMetrics 列指标错误: {}", jbody(&j));
        }
        let names = j.get("data").and_then(Value::as_array)
            .ok_or_else(|| anyhow!("VictoriaMetrics 列指标缺少 data 数组"))?;
        names.iter().take(1000).map(|name| {
            let name = name.as_str().ok_or_else(|| anyhow!("VictoriaMetrics 指标名必须为字符串"))?;
            Ok((name.to_string(), name.to_string()))
        }).collect()
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        Ok(vec!["samples".into()])
    }
    async fn rows(
        &self,
        container: &str,
        _object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let mut rows = self.instant(container).await?;
        rows.rows = rows.rows.into_iter().skip(usize::try_from(offset).unwrap_or(usize::MAX))
            .take(limit.min(1000) as usize).collect();
        Ok(rows)
    }
    async fn run(&self, _container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        self.instant(sql.trim()).await
    }
}

// =============================================================================
// Turso（sqld HTTP pipeline）
// =============================================================================

struct TursoAdapter {
    ctx: Ctx,
}

impl TursoAdapter {
    async fn exec(&self, sql: &str) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
        let body = serde_json::json!({
            "requests": [{"type": "execute", "stmt": {"sql": sql}}]
        });
        let (s, j) = self.ctx.post_json("/v2/pipeline", body).await?;
        Ctx::check(s, &j, "Turso 请求")?;
        let result = j
            .pointer("/results/0/response/result")
            .cloned()
            .ok_or_else(|| anyhow!("Turso 响应异常: {}", jbody(&j)))?;
        if let Some(err) = result.get("error") {
            bail!("Turso SQL 错误: {}", jbody(err));
        }
        let cols = result
            .pointer("/cols")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|c| c.get("name").and_then(jtext))
                    .collect()
            })
            .unwrap_or_default();
        let mut rows = Vec::new();
        for r in result
            .pointer("/rows")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            if let Some(arr) = r.as_array() {
                rows.push(
                    arr.iter()
                        .map(|v| v.get("value").and_then(jtext))
                        .collect::<Vec<Option<String>>>(),
                );
            }
        }
        Ok((cols, rows))
    }
}

#[async_trait]
impl HttpAdapter for TursoAdapter {
    async fn ping(&self) -> Result<String> {
        self.exec("SELECT 1").await?;
        Ok("Turso (libSQL)".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        Ok(vec![("default".into(), "default".into())])
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        let (cols, rows) = self
            .exec("SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .await?;
        let _ = cols;
        Ok(rows
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .collect())
    }
    async fn rows(
        &self,
        _container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let sql = format!(
            "SELECT * FROM \"{}\" LIMIT {} OFFSET {}",
            object.replace('"', "\"\""),
            limit,
            offset
        );
        let (cols, rows) = self.exec(&sql).await?;
        Ok(HttpRows { columns: cols, rows })
    }
    async fn run(&self, _container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let (cols, rows) = self.exec(sql).await?;
        Ok(HttpRows { columns: cols, rows })
    }
}

// =============================================================================
// Cloudflare D1
// =============================================================================

struct D1Adapter {
    ctx: Ctx,
    account_id: String,
    database_id: String,
    base_api: String,
}

impl D1Adapter {
    fn api(&self, path: &str) -> String {
        format!("{}/accounts/{}/d1/{}{}", self.base_api, self.account_id, self.database_id, path)
    }

    async fn sql(&self, statement: &str) -> Result<HttpRows> {
        let (s, j) = self
            .ctx
            .post_json(&self.api("/query"), serde_json::json!({"sql": statement}))
            .await?;
        Ctx::check(s, &j, "Cloudflare D1 请求")?;
        if j.get("success").and_then(Value::as_bool) == Some(false) {
            bail!("D1 错误: {}", jbody(&j));
        }
        let result = if j.get("result").and_then(Value::as_array).is_some() {
            j.get("result").and_then(Value::as_array).and_then(|a| a.first().cloned()).unwrap_or(Value::Null)
        } else {
            j.get("result").cloned().unwrap_or(Value::Null)
        };
        if let Some(err) = result.get("error") {
            bail!("D1 SQL 错误: {}", jbody(err));
        }
        let mut cols: Vec<String> = Vec::new();
        if let Some(meta) = result.pointer("/meta/columns").and_then(Value::as_array) {
            for c in meta {
                if let Some(n) = jtext(c) {
                    cols.push(n);
                }
            }
        }
        let items = result
            .get("results")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if cols.is_empty() {
            for it in &items {
                if let Value::Object(map) = it {
                    for k in map.keys() {
                        if !cols.contains(k) {
                            cols.push(k.clone());
                        }
                    }
                }
            }
        }
        let rows = items
            .iter()
            .map(|it| cols.iter().map(|c| it.get(c).and_then(jtext)).collect())
            .collect();
        Ok(HttpRows { columns: cols, rows })
    }
}

#[async_trait]
impl HttpAdapter for D1Adapter {
    async fn ping(&self) -> Result<String> {
        let (s, j) = self.ctx.get_json(&self.api("")).await?;
        Ctx::check(s, &j, "Cloudflare D1 请求")?;
        if j.get("success").and_then(Value::as_bool) != Some(true) {
            bail!("D1 错误: {}", jbody(&j));
        }
        let name = j
            .pointer("/result/name")
            .and_then(jtext)
            .unwrap_or_default();
        Ok(format!("Cloudflare D1（{}）", name))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        Ok(vec![("database".into(), "当前数据库".into())])
    }
    async fn objects(&self, _container: &str) -> Result<Vec<String>> {
        let r = self
            .sql("SELECT name FROM sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
            .await?;
        Ok(r.rows
            .iter()
            .filter_map(|r| r.first().cloned().flatten())
            .collect())
    }
    async fn rows(
        &self,
        _container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let sql = format!(
            "SELECT * FROM \"{}\" LIMIT {} OFFSET {}",
            object.replace('"', "\"\""),
            limit,
            offset
        );
        self.sql(&sql).await
    }
    async fn run(&self, _container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        self.sql(sql).await
    }
}

// =============================================================================
// Apache Pulsar（REST 管理接口）
// =============================================================================

struct PulsarAdapter {
    ctx: Ctx,
    /// 指定 tenant 时容器直接是 namespace。
    tenant: Option<String>,
}

#[async_trait]
impl HttpAdapter for PulsarAdapter {
    async fn ping(&self) -> Result<String> {
        let (s, j) = self.ctx.get_json("/admin/v2/tenants").await?;
        Ctx::check(s, &j, "Pulsar 管理接口")?;
        Ok("Apache Pulsar".into())
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        match &self.tenant {
            Some(t) => {
                let (s, j) = self
                    .ctx
                    .get_json(&format!("/admin/v2/namespaces/{}", urlencode(t)))
                    .await?;
                Ctx::check(s, &j, "Pulsar 列命名空间")?;
                Ok(str_list(&j, &[])
                    .into_iter()
                    .map(|n| (n.clone(), n))
                    .collect())
            }
            None => {
                let (s, j) = self.ctx.get_json("/admin/v2/tenants").await?;
                Ctx::check(s, &j, "Pulsar 列租户")?;
                Ok(str_list(&j, &[])
                    .into_iter()
                    .map(|n| (n.clone(), n))
                    .collect())
            }
        }
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        // 容器是租户 → 列命名空间；容器是命名空间 → 列主题
        if self.tenant.is_some() {
            self.topics(container).await
        } else {
            let (s, j) = self
                .ctx
                .get_json(&format!("/admin/v2/namespaces/{}", urlencode(container)))
                .await?;
            Ctx::check(s, &j, "Pulsar 列命名空间")?;
            Ok(str_list(&j, &[]))
        }
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        // 命名空间 → 主题列表；主题 → 统计信息
        let mut result = if object.is_empty() && !container.contains('/') {
            let ns = self.objects(container).await?;
            HttpRows {
                columns: vec!["namespace".into()],
                rows: ns.into_iter().map(|n| vec![Some(n)]).collect(),
            }
        } else {
            self.topic_stats(container, object).await?
        };
        result.rows = result.rows.into_iter().skip(usize::try_from(offset).unwrap_or(usize::MAX)).take(limit.min(10000) as usize).collect();
        Ok(result)
    }
    async fn run(&self, container: &str, object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        // 支持 "/" 开头的原始管理 API 路径
        if sql.starts_with('/') {
            let (s, j) = self.ctx.get_json(sql).await?;
            Ctx::check(s, &j, "Pulsar 管理 API")?;
            return Ok(obj_cols_rows(std::slice::from_ref(&j)));
        }
        bail!("Pulsar 查询需要以 / 开头的管理 API 路径");
    }
}

impl PulsarAdapter {
    async fn topics(&self, ns: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for prefix in ["persistent", "non-persistent"] {
            let (s, j) = self
                .ctx
                .get_json(&format!("/admin/v2/{}/{}", prefix, ns.split('/').map(urlencode).collect::<Vec<_>>().join("/")))
                .await?;
            Ctx::check(s, &j, "Pulsar 列主题")?;
            for t in str_list(&j, &[]) {
                if let Some(short) = t
                    .strip_prefix(&format!("{}://{}/", prefix, ns))
                {
                    out.push(format!("{}:{}", &short, prefix));
                }
            }
        }
        Ok(out)
    }

    async fn topic_stats(&self, ns: &str, topic: &str) -> Result<HttpRows> {
        let (domain, name) = match topic.split_once(':') {
            Some((n, d)) => (d, n),
            None => ("persistent", topic),
        };
        let (s, j) = self
            .ctx
            .get_json(&format!(
                "/admin/v2/{}/{}/{}/stats",
                domain,
                ns.split('/').map(urlencode).collect::<Vec<_>>().join("/"),
                urlencode(name)
            ))
            .await?;
        Ctx::check(s, &j, "Pulsar 主题统计")?;
        Ok(obj_cols_rows(std::slice::from_ref(&j)))
    }
}

// =============================================================================
// RabbitMQ（HTTP 管理接口）
// =============================================================================

struct RabbitMgmtAdapter {
    ctx: Ctx,
}

#[async_trait]
impl HttpAdapter for RabbitMgmtAdapter {
    async fn ping(&self) -> Result<String> {
        let (s, j) = self.ctx.get_json("/api/overview").await?;
        Ctx::check(s, &j, "RabbitMQ 管理接口")?;
        let v = j
            .get("rabbitmq_version")
            .and_then(jtext)
            .unwrap_or_default();
        Ok(format!("RabbitMQ {}", v))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let (s, j) = self.ctx.get_json("/api/vhosts").await?;
        Ctx::check(s, &j, "RabbitMQ 列 vhost")?;
        Ok(j.as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.get("name").and_then(jtext))
                    .map(|n| (n.clone(), n))
                    .collect()
            })
            .unwrap_or_default())
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        let (s, j) = self
            .ctx
            .get_json(&format!("/api/queues/{}", urlencode(container)))
            .await?;
        Ctx::check(s, &j, "RabbitMQ 列队列")?;
        for q in j.as_array().cloned().unwrap_or_default() {
            if let Some(n) = q.get("name").and_then(jtext) {
                out.push(format!("queue:{}", n));
            }
        }
        let (s, j) = self
            .ctx
            .get_json(&format!("/api/exchanges/{}", urlencode(container)))
            .await?;
        Ctx::check(s, &j, "RabbitMQ 列交换器")?;
        for e in j.as_array().cloned().unwrap_or_default() {
            let name = e.get("name").and_then(jtext).unwrap_or_default();
            if !name.is_empty() && e.get("internal").and_then(Value::as_bool) != Some(true) {
                out.push(format!("exchange:{}", name));
            }
        }
        Ok(out)
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let (kind, name) = match object.split_once(':') {
            Some((k, n)) => (k, n),
            None => ("queue", object),
        };
        let (s, j) = self
            .ctx
            .get_json(&format!(
                "/api/{}/{}/{}",
                if kind == "exchange" { "exchanges" } else { "queues" },
                urlencode(container),
                urlencode(name)
            ))
            .await?;
        Ctx::check(s, &j, "RabbitMQ 详情")?;
        // 竖排展示：metric / value
        let mut rows = Vec::new();
        if let Value::Object(map) = &j {
            for (k, v) in map {
                rows.push(vec![Some(k.clone()), jtext(v)]);
            }
        }
        Ok(HttpRows {
            columns: vec!["属性".into(), "值".into()],
            rows: rows.into_iter().skip(usize::try_from(offset).unwrap_or(usize::MAX)).take(limit.min(10000) as usize).collect(),
        })
    }
    async fn run(&self, _container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        let path = if sql.starts_with('/') {
            sql.to_string()
        } else {
            bail!("RabbitMQ 管理查询需要 /api/ 开头的 API 路径");
        };
        let (s, j) = self.ctx.get_json(&path).await?;
        Ctx::check(s, &j, "RabbitMQ 管理 API")?;
        Ok(obj_cols_rows(std::slice::from_ref(&j)))
    }
}

// =============================================================================
// Nacos（Open API）
// =============================================================================

struct NacosAdapter {
    ctx: Ctx,
    access_token: std::sync::Mutex<Option<String>>,
}

impl NacosAdapter {
    async fn login(&self) -> Result<Option<String>> {
        {
            let guard = self
                .access_token
                .lock()
                .map_err(|_| anyhow!("Nacos 锁不可用"))?;
            if let Some(t) = guard.clone() {
                return Ok(Some(t));
            }
        }
        if self.ctx.user.is_empty() {
            return Ok(None);
        }
        let url = format!(
            "{}/nacos/v1/auth/login",
            self.ctx.base
        );
        let resp = self
            .ctx
            .http
            .post(&url)
            .form(&[("username", &self.ctx.user), ("password", &self.ctx.pass)])
            .send()
            .await
            .context("Nacos 登录失败")?;
        let text = resp.text().await.unwrap_or_default();
        let j: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let token = j.get("accessToken").and_then(jtext);
        *self
            .access_token
            .lock()
            .map_err(|_| anyhow!("Nacos 锁不可用"))? = token.clone();
        Ok(token)
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let mut path = path.to_string();
        if let Some(t) = self.login().await? {
            let sep = if path.contains('?') { '&' } else { '?' };
            path.push_str(&format!("{}accessToken={}", sep, urlencode(&t)));
        }
        let (s, j) = self.ctx.get_json(&path).await?;
        Ctx::check(s, &j, "Nacos 请求")?;
        Ok(j)
    }
}

#[async_trait]
impl HttpAdapter for NacosAdapter {
    async fn ping(&self) -> Result<String> {
        let j = self.get("/nacos/v1/console/server/state").await?;
        let v = j.get("version").and_then(jtext).unwrap_or_default();
        Ok(format!("Nacos {}", v))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let j = self.get("/nacos/v1/console/namespaces").await?;
        let arr = j.get("data").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(arr
            .iter()
            .filter_map(|x| {
                let id = x.get("namespace").and_then(jtext)?;
                let show = x
                    .get("namespaceShowName")
                    .and_then(jtext)
                    .unwrap_or_else(|| id.clone());
                Some((id, show))
            })
            .collect())
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        // 服务列表
        let j = self
            .get(&format!(
                "/nacos/v1/ns/service/list?pageNo=1&pageSize=200&namespaceId={}",
                urlencode(container)
            ))
            .await?;
        for d in str_list(&j, &["doms"]) {
            out.push(format!("svc:{}", d));
        }
        // 配置列表
        let j = self
            .get(&format!(
                "/nacos/v1/cs/configs?search=blur&dataId=&group=&pageNo=1&pageSize=200&tenant={}",
                urlencode(container)
            ))
            .await?;
        for it in j
            .pointer("/pageItems")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let data_id = it.get("dataId").and_then(jtext).unwrap_or_default();
            let group = it.get("group").and_then(jtext).unwrap_or_default();
            out.push(format!("cfg:{}|{}", data_id, group));
        }
        Ok(out)
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let mut result = if let Some(svc) = object.strip_prefix("svc:") {
            let j = self
                .get(&format!(
                    "/nacos/v1/ns/instance/list?serviceName={}&namespaceId={}",
                    urlencode(svc),
                    urlencode(container)
                ))
                .await?;
            let hosts = j.get("hosts").and_then(Value::as_array).cloned().unwrap_or_default();
            Ok::<HttpRows, anyhow::Error>(obj_cols_rows(&hosts))
        } else if let Some(cfg) = object.strip_prefix("cfg:") {
            let (data_id, group) = cfg.split_once('|').unwrap_or((cfg, "DEFAULT_GROUP"));
            let j = self
                .get(&format!(
                    "/nacos/v1/cs/configs?dataId={}&group={}&tenant={}",
                    urlencode(data_id),
                    urlencode(group),
                    urlencode(container)
                ))
                .await?;
            let content = match &j {
                Value::String(s) => s.clone(),
                other => jtext(other).unwrap_or_default(),
            };
            Ok(HttpRows {
                columns: vec!["dataId".into(), "group".into(), "content".into()],
                rows: vec![vec![Some(data_id.into()), Some(group.into()), Some(content)]],
            })
        } else {
            bail!("Nacos 对象需以 svc: 或 cfg: 前缀标识");
        }?;
        result.rows = result.rows.into_iter().skip(usize::try_from(offset).unwrap_or(usize::MAX)).take(limit.min(10000) as usize).collect();
        Ok(result)
    }
    async fn run(&self, container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        if sql.starts_with("svc:") || sql.starts_with("cfg:") {
            return self.rows(container, sql, 0, 100).await;
        }
        let j = self.get(sql).await?;
        Ok(obj_cols_rows(std::slice::from_ref(&j)))
    }
}

// =============================================================================
// Consul（HTTP API）
// =============================================================================

struct ConsulAdapter {
    ctx: Ctx,
}

impl ConsulAdapter {
    async fn get(&self, path: &str) -> Result<Value> {
        let (status, j) = self.ctx.get_json(path).await?;
        Ctx::check(status, &j, "Consul 请求")?;
        Ok(j)
    }

    fn kv_rows(j: &Value, offset: u64, limit: u64) -> Result<HttpRows> {
        use base64::Engine;
        let entries = j.as_array().ok_or_else(|| anyhow!("Consul KV 响应不是数组"))?;
        let mut rows = Vec::new();
        for item in entries.iter().skip(usize::try_from(offset).unwrap_or(usize::MAX)).take(limit.min(10000) as usize) {
            let key = item.get("Key").and_then(Value::as_str).ok_or_else(|| anyhow!("Consul KV 响应缺少 Key"))?;
            let value = match item.get("Value") {
                Some(Value::Null) => None,
                Some(Value::String(encoded)) => {
                    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded)
                        .context("Consul KV Value 不是合法 base64")?;
                    Some(crate::native_db::text_bytes(&bytes))
                }
                _ => bail!("Consul KV 响应缺少合法 Value"),
            };
            rows.push(vec![Some(key.to_owned()), value, item.get("ModifyIndex").and_then(jtext)]);
        }
        Ok(HttpRows { columns: vec!["key".into(), "value".into(), "modify_index".into()], rows })
    }
}

#[async_trait]
impl HttpAdapter for ConsulAdapter {
    async fn ping(&self) -> Result<String> {
        let j = self.get("/v1/agent/self").await?;
        let v = j.pointer("/Config/Version").and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Consul 响应缺少 Config.Version"))?;
        Ok(format!("Consul {}", v))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        let j = self.get("/v1/catalog/datacenters").await?;
        let dcs = j.as_array().ok_or_else(|| anyhow!("Consul datacenters 响应不是数组"))?;
        dcs.iter().map(|v| {
            let name = v.as_str().ok_or_else(|| anyhow!("Consul datacenter 不是字符串"))?;
            Ok((name.to_owned(), name.to_owned()))
        }).collect()
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        let j = self
            .get(&format!("/v1/kv/?keys&dc={}", urlencode(container)))
            .await?;
        let mut out: Vec<String> = Vec::new();
        let keys = j.as_array().ok_or_else(|| anyhow!("Consul keys 响应不是数组"))?;
        for value in keys {
            let key = value.as_str().ok_or_else(|| anyhow!("Consul key 不是字符串"))?;
            // 只展示第一层：目录（dir:）与叶子（kv:）
            let first = key.trim_start_matches('/').split('/').next().unwrap_or("");
            if first.is_empty() {
                continue;
            }
            let has_slash = key.contains('/');
            let entry = if has_slash {
                format!("dir:{}", first)
            } else {
                format!("kv:{}", first)
            };
            if !out.contains(&entry) {
                out.push(entry);
            }
        }
        Ok(out)
    }
    async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        let (prefix, recurse) = match object.split_once(':') {
            Some(("dir", p)) => (p, true),
            Some(("kv", p)) => (p, false),
            _ => (object, false),
        };
        let j = self
            .get(&format!(
                "/v1/kv/{}?dc={}&recurse={}",
                urlencode(prefix),
                urlencode(container),
                recurse
            ))
            .await?;
        Self::kv_rows(&j, offset, limit)
    }
    async fn run(&self, _container: &str, _object: &str, sql: &str) -> Result<HttpRows> {
        let sql = sql.trim();
        if sql.starts_with('/') {
            let j = self.get(sql).await?;
            if sql.split('?').next().is_some_and(|path| path.starts_with("/v1/kv/")) {
                Self::kv_rows(&j, 0, 10000)
            } else if let Some(items) = j.as_array() {
                Ok(obj_cols_rows(items))
            } else {
                Ok(obj_cols_rows(std::slice::from_ref(&j)))
            }
        } else {
            self.rows("", &format!("kv:{}", sql), 0, 100).await
        }
    }
}

/// 最小百分号编码（查询参数）。
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// =============================================================================
// 入口：HttpDb
// =============================================================================

/// HTTP 类数据源的统一门面（挂在 Db::Http 上）。
pub struct HttpDb {
    adapter: std::sync::Arc<dyn HttpAdapter>,
    pub kind: DbKind,
    pub database: String,
    pub version: String,
}

impl HttpDb {
    pub(crate) async fn from_adapter(req: &ConnectionRequest, adapter: Box<dyn HttpAdapter>) -> Result<Self> {
        let version = adapter.ping().await?;
        // JDBC ConnectionRequest.database identifies the endpoint/database URL,
        // not the tree namespace. Empty namespace restores the initial session.
        let database = if req.kind.id() == "h2" || adapter.shares_session_namespace() { String::new() } else { req.database.clone() };
        Ok(Self { kind: req.kind.clone(), database, version, adapter: std::sync::Arc::from(adapter) })
    }
    pub(crate) fn tunnel_runtime(&self) -> Option<std::sync::Arc<crate::http_tunnel::TunnelRuntime>> { self.adapter.tunnel_runtime() }
    pub(crate) async fn shutdown(&self) { self.adapter.shutdown().await; }
    pub(crate) fn shares_session_namespace(&self) -> bool {
        self.kind.id() == "h2" || self.adapter.shares_session_namespace()
    }
    /// Share the existing serialized JDBC session; keep each view's namespace.
    pub(crate) fn with_namespace(&self, database: &str) -> Self {
        Self { adapter: self.adapter.clone(), kind: self.kind.clone(),
            database: database.into(), version: self.version.clone() }
    }
    /// 真实列元数据（若适配器提供）。
    pub(crate) async fn table_columns(&self, container: &str, object: &str) -> Result<Option<Vec<HttpColumn>>> {
        self.adapter.table_columns(container, object).await
    }
    /// Adapter-side C2 evidence; the caller still owns activity/read-only/plan admission.
    pub(crate) async fn relational_capabilities(&self) -> Result<relational::JdbcCapabilities> {
        self.adapter.relational_capabilities().await
    }
    pub(crate) async fn relational_metadata(&self, object: &relational::JdbcObjectId) -> Result<relational::JdbcMetadataSnapshot> {
        self.adapter.relational_metadata(object).await
    }
    pub(crate) async fn apply_row_plan(&self, plan: &relational::JdbcRowPlan) -> Result<relational::JdbcMutationOutcome> {
        if plan.read_only { bail!("READ_ONLY_VIOLATION: effective connection is read-only"); }
        self.adapter.apply_row_plan(plan).await
    }
    pub async fn connect(kind: DbKind, req: &ConnectionRequest) -> Result<Self> {
        let meta = kind
            .meta()
            .ok_or_else(|| anyhow!("未知数据源类型: {}", kind.id()))?;
        let adapter_id = meta.http;
        if adapter_id.is_empty() {
            bail!("{} 未配置 HTTP 适配器", meta.label);
        }
        let port = if req.port == 0 { meta.port } else { req.port };
        let params = crate::db::extra_params(&req.params);
        let lookup = params.clone();
        let get = move |name: &str| {
            lookup
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        };
        let ssl = get("ssl")
            .or_else(|| get("tls"))
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let verify = get("ssl_verify")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);
        let base = match get("url") {
            Some(u) => u.trim_end_matches('/').to_string(),
            None => format!(
                "{}://{}:{}",
                if ssl { "https" } else { "http" },
                req.host,
                port
            ),
        };
        let ctx = Ctx {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(req.timeout_secs.clamp(1, 300)))
                .danger_accept_invalid_certs(!verify)
                .build()?,
            base,
            user: req.user.clone(),
            pass: req.password.clone(),
            params,
            headers: reqwest::header::HeaderMap::new(),
        };
        let adapter: Box<dyn HttpAdapter> = match adapter_id {
            "elasticsearch" => Box::new(EsAdapter { ctx }),
            "solr" => Box::new(SolrAdapter { ctx }),
            "couchdb" => Box::new(CouchDbAdapter { ctx }),
            "meilisearch" => Box::new(MeiliAdapter { ctx: with_bearer(ctx) }),
            "qdrant" => Box::new(QdrantAdapter { ctx: with_token_header(ctx, "api-key", "") }),
            "milvus" => Box::new(MilvusAdapter { ctx: with_milvus_auth(ctx) }),
            "weaviate" => Box::new(WeaviateAdapter { ctx: with_bearer(ctx) }),
            "chroma" => Box::new(ChromaAdapter { ctx }),
            "neo4j" => Box::new(Neo4jAdapter {
                ctx,
                db: if req.database.is_empty() {
                    "neo4j".into()
                } else {
                    req.database.clone()
                },
            }),
            "tdengine" => Box::new(TdengineAdapter { ctx }),
            "iotdb" => Box::new(IotdbAdapter {
                ctx: with_header_auth(ctx),
            }),
            "influxdb" => Box::new(InfluxAdapter { ctx: if ctx.token().is_some() || ctx.user.is_empty() {
                with_token_header(ctx, "authorization", "Token ")
            } else { ctx } }),
            "victoriametrics" => Box::new(VmAdapter { ctx }),
            "turso" => Box::new(TursoAdapter {
                ctx: with_bearer(ctx),
            }),
            "d1" => {
                let account_id = get("account_id")
                    .ok_or_else(|| anyhow!("Cloudflare D1 需要参数 account_id"))?;
                let database_id = get("database_id")
                    .ok_or_else(|| anyhow!("Cloudflare D1 需要参数 database_id"))?;
                for (name, value) in [("account_id", &account_id), ("database_id", &database_id)] {
                    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
                        bail!("D1 {} 仅允许字母、数字、下划线和连字符", name);
                    }
                }
                let base_api = match get("url") {
                    None => "https://api.cloudflare.com/client/v4".to_string(),
                    Some(raw) => {
                        let url = reqwest::Url::parse(&raw).context("D1 url 无效")?;
                        let loopback = url.host_str().is_some_and(|host| host == "localhost" || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback()));
                        if !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some()
                            || !(url.scheme() == "https" || (url.scheme() == "http" && loopback)) {
                            bail!("D1 url 必须是无凭据、query、fragment 的 HTTPS 地址；HTTP 仅允许回环地址");
                        }
                        raw.trim_end_matches('/').to_string()
                    }
                };
                Box::new(D1Adapter {
                    base_api,
                    account_id,
                    database_id,
                    ctx: with_bearer(ctx),
                })
            }
            "pulsar" => Box::new(PulsarAdapter {
                ctx,
                tenant: get("tenant"),
            }),
            "rabbitmq" => Box::new(RabbitMgmtAdapter { ctx }),
            "nacos" => Box::new(NacosAdapter {
                ctx,
                access_token: std::sync::Mutex::new(None),
            }),
            "consul" => Box::new(ConsulAdapter { ctx: with_token_header(ctx, "x-consul-token", "") }),
            other => bail!("未知 HTTP 适配器: {}", other),
        };

        // 真实建连验证
        let version = adapter.ping().await?;
        Ok(Self {
            adapter: std::sync::Arc::from(adapter),
            kind,
            database: req.database.clone(),
            version,
        })
    }

    /// 顶级容器（树：数据库层）。
    pub async fn containers(&self) -> Result<Vec<(String, String)>> {
        self.adapter.containers().await
    }

    /// 容器内对象（树：表层）。
    pub async fn objects(&self, container: &str) -> Result<Vec<String>> {
        self.adapter.objects(container).await
    }

    /// 容器内对象（含类型分类）：适配器支持视图区分时按类型返回，否则全部按表。
    pub async fn objects_with_types(&self, container: &str) -> Result<Vec<(String, crate::models::ObjectType)>> {
        let Some(pairs) = self.adapter.object_types(container).await? else {
            return Ok(self.adapter.objects(container).await?
                .into_iter().map(|n| (n, crate::models::ObjectType::Table)).collect());
        };
        Ok(pairs.into_iter()
            .map(|(name, ty)| (name, if ty.eq_ignore_ascii_case("view") {
                crate::models::ObjectType::View
            } else {
                crate::models::ObjectType::Table
            }))
            .collect())
    }

    /// 浏览对象数据。
    pub async fn rows(
        &self,
        container: &str,
        object: &str,
        offset: u64,
        limit: u64,
    ) -> Result<HttpRows> {
        if limit == 0 && !self.shares_session_namespace() {
            return Ok(HttpRows { columns: Vec::new(), rows: Vec::new() });
        }
        self.adapter.rows(container, object, offset, limit).await
    }

    /// 查询标签页执行。
    pub async fn run(&self, container: &str, object: &str, sql: &str) -> Result<HttpRows> {
        self.adapter.run(container, object, sql).await
    }
}

fn check_iotdb_code(body: &Value) -> Result<()> {
    if let Some(code) = body.get("code").and_then(Value::as_i64) {
        if code != 200 { bail!("IoTDB 错误 {}: {}", code, jbody(body)); }
    }
    Ok(())
}

/// IoTDB REST v2 使用 Basic base64(user:password)，不是 x-iotdb-*。
fn with_header_auth(ctx: Ctx) -> Ctx { ctx }

fn with_token_header(mut ctx: Ctx, header: &'static str, prefix: &str) -> Ctx {
    let token = ctx.token().unwrap_or_else(|| ctx.pass.clone());
    if !token.is_empty() {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("{}{}", prefix, token)) {
            ctx.headers.insert(reqwest::header::HeaderName::from_static(header), value);
        }
    }
    ctx
}

fn with_milvus_auth(mut ctx: Ctx) -> Ctx {
    let token = ctx.token().unwrap_or_else(|| {
        if ctx.user.is_empty() { ctx.pass.clone() } else { format!("{}:{}", ctx.user, ctx.pass) }
    });
    if !token.is_empty() {
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token)) {
            ctx.headers.insert(reqwest::header::AUTHORIZATION, value);
        }
    }
    ctx
}

/// Turso / D1：Bearer Token（密码或 token / api_token 参数）。
fn with_bearer(mut ctx: Ctx) -> Ctx {
    use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
    let mut headers = HeaderMap::new();
    let token = ctx
        .param("token")
        .or_else(|| ctx.param("api_token"))
        .unwrap_or_else(|| ctx.pass.clone());
    if !token.is_empty() {
        if let Ok(v) = HeaderValue::from_str(&format!("Bearer {}", token)) {
            headers.insert(AUTHORIZATION, v);
        }
    }
    ctx.headers = headers;
    ctx
}

#[cfg(test)]
#[path = "http_db_tests.rs"]
mod tests;
