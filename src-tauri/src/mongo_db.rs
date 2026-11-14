//! MongoDB 原生客户端：mongodb 官方 Rust 驱动。
//!
//! 树结构：连接 → 数据库 → 集合；「表数据」页展示文档（顶层字段平铺）；
//! 查询标签页输入 JSON 查询条件（如 `{"age": {"$gt": 18}}`）执行 find，
//! 空输入 / `{}` 返回全部文档，`count {…}` 形式返回计数。

use anyhow::{anyhow, Result};
use futures::StreamExt;
use mongodb::bson::{Bson, Document};
use mongodb::{Client, options::FindOptions};

use crate::models::{ConnectionRequest, DbKind};

/// 导出游标每批从服务端拉取的文档数（仅控制网络批量，不限制总行数）。
const EXPORT_BATCH_SIZE: u32 = 1000;

pub struct MongoDb {
    client: Client,
    /// 连接配置中选择的默认库（可为空）。
    pub database: String,
    pub kind: DbKind,
}

impl MongoDb {
    pub async fn connect(kind: DbKind, req: &ConnectionRequest) -> Result<Self> {
        let port = if req.port == 0 { 27017 } else { req.port };
        let params = crate::db::extra_params(&req.params);
        let get = |name: &str| {
            params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        };
        let srv = get("srv")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        let auth_source = get("auth_source").unwrap_or_else(|| "admin".into());

        let mut url = if srv {
            "mongodb+srv://".to_string()
        } else {
            "mongodb://".to_string()
        };
        if !req.user.is_empty() || !req.password.is_empty() {
            url.push_str(&urlencode(&req.user));
            if !req.password.is_empty() {
                url.push(':');
                url.push_str(&urlencode(&req.password));
            }
            url.push('@');
        }
        if srv {
            url.push_str(&req.host);
        } else {
            url.push_str(&format!("{}:{}", req.host, port));
        }
        url.push_str("/?");
        if !req.user.is_empty() {
            url.push_str(&format!("authSource={}&", urlencode(&auth_source)));
        }
        if get("ssl")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
        {
            url.push_str("tls=true&");
        }
        url.push_str("serverSelectionTimeoutMS=15000");

        let client = Client::with_uri_str(&url)
            .await
            .map_err(|e| anyhow!("MongoDB 连接串无效: {e}"))?;
        // 真实建连验证：buildInfo 无需认证即可执行
        let hello = client
            .database("admin")
            .run_command(mongodb::bson::doc! {"buildInfo": 1})
            .await
            .map_err(|e| anyhow!("无法连接到 MongoDB {}:{}：{}", req.host, port, e))?;
        let _ = hello;

        Ok(Self {
            client,
            database: req.database.clone(),
            kind,
        })
    }

    /// 列出数据库。
    pub async fn databases(&self) -> Result<Vec<(String, String)>> {
        let names = self
            .client
            .list_database_names()
            .await
            .map_err(|e| anyhow!("MongoDB 列数据库失败: {e}"))?;
        Ok(names.into_iter().map(|n| (n.clone(), n)).collect())
    }

    /// 列出某个库下的集合。
    pub async fn collections(&self, db: &str) -> Result<Vec<String>> {
        self.client
            .database(db)
            .list_collection_names()
            .await
            .map_err(|e| anyhow!("MongoDB 列集合失败: {e}"))
    }

    /// 查询文档（顶层字段平铺为文本）。返回 (列名, 行)。
    pub async fn find_docs(
        &self,
        db: &str,
        coll: &str,
        filter_json: &str,
        skip: u64,
        limit: u64,
    ) -> Result<(Vec<String>, Vec<Vec<Option<String>>>)> {
        let filter = parse_filter(filter_json)?;
        let opts = FindOptions::builder()
            .skip(skip)
            .limit(Some(limit as i64))
            .build();
        let mut cursor = self
            .client
            .database(db)
            .collection::<Document>(coll)
            .find(filter.unwrap_or_default())
            .with_options(opts)
            .await
            .map_err(|e| anyhow!("MongoDB 查询失败: {e}"))?;

        let mut docs: Vec<Document> = Vec::new();
        while let Some(d) = cursor.next().await {
            docs.push(d.map_err(|e| anyhow!("MongoDB 读取失败: {e}"))?);
        }
        // 列：_id 优先，其余按首次出现顺序
        let mut cols: Vec<String> = Vec::new();
        for d in &docs {
            for k in d.keys() {
                if !cols.contains(k) {
                    cols.push(k.clone());
                }
            }
        }
        if let Some(pos) = cols.iter().position(|c| c == "_id") {
            if pos != 0 {
                cols.swap(0, pos);
            }
        }
        let rows = docs
            .iter()
            .map(|d| {
                cols.iter()
                    .map(|c| match d.get(c) { Some(Bson::Null) | None => None, Some(value) => Some(bson_to_text(value)) })
                    .collect::<Vec<Option<String>>>()
            })
            .collect();
        Ok((cols, rows))
    }

    /// 流式导出用的单个 `find` 游标：服务端按 `batch_size` 分批拉取，
    /// 不使用 `skip` 分页（避免深分页的线性扫描开销），也不把整集合读入内存。
    ///
    /// - `filter_json`：与查询页同语义的 JSON 过滤条件；空 / `{}` 表示全部文档。
    /// - `limit = Some(0)`：不执行 `find`，返回 `Ok(None)`（导出 0 行）。
    /// - `limit = None`：不限行数。
    pub async fn find_export_cursor(
        &self,
        db: &str,
        coll: &str,
        filter_json: &str,
        limit: Option<u64>,
    ) -> Result<Option<mongodb::Cursor<Document>>> {
        if matches!(limit, Some(0)) {
            return Ok(None);
        }
        let filter = parse_filter(filter_json)?;
        let mut opts = FindOptions::default();
        opts.batch_size = Some(EXPORT_BATCH_SIZE);
        if let Some(n) = limit {
            opts.limit = Some(
                i64::try_from(n).map_err(|_| anyhow!("MongoDB 导出行数上限 {n} 超过 i64 范围"))?,
            );
        }
        let cursor = self
            .client
            .database(db)
            .collection::<Document>(coll)
            .find(filter.unwrap_or_default())
            .with_options(opts)
            .await
            .map_err(|e| anyhow!("MongoDB 导出查询失败: {e}"))?;
        Ok(Some(cursor))
    }

    /// 统计文档数。
    pub async fn count(&self, db: &str, coll: &str, filter_json: &str) -> Result<u64> {
        let filter = parse_filter(filter_json)?;
        self.client
            .database(db)
            .collection::<Document>(coll)
            .count_documents(filter.unwrap_or_default())
            .await
            .map_err(|e| anyhow!("MongoDB 计数失败: {e}"))
    }

    /// 服务器版本（buildInfo）。
    pub async fn version(&self) -> Result<String> {
        let info = self
            .client
            .database("admin")
            .run_command(mongodb::bson::doc! {"buildInfo": 1})
            .await
            .map_err(|e| anyhow!("MongoDB buildInfo 失败: {e}"))?;
        let v = info.get("version").and_then(bson_as_string).unwrap_or_default();
        Ok(format!("MongoDB {}", v))
    }
}

/// 解析查询条件 JSON；空 / `{}` 表示全部。
fn parse_filter(filter_json: &str) -> Result<Option<Document>> {
    let s = filter_json.trim();
    if s.is_empty() || s == "{}" {
        return Ok(None);
    }
    let v: serde_json::Value =
        serde_json::from_str(s).map_err(|e| anyhow!("查询条件不是合法 JSON: {e}"))?;
    let bson = json_to_bson(&v).ok_or_else(|| anyhow!("不支持的查询条件类型"))?;
    let doc: Document = mongodb::bson::from_bson(bson)
        .map_err(|e| anyhow!("查询条件转换失败: {e}"))?;
    Ok(Some(doc))
}

fn json_to_bson(v: &serde_json::Value) -> Option<Bson> {
    use serde_json::Value as J;
    Some(match v {
        J::Null => Bson::Null,
        J::Bool(b) => Bson::Boolean(*b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Bson::Int64(i)
            } else {
                Bson::Double(n.as_f64()?)
            }
        }
        J::String(s) => Bson::String(s.clone()),
        J::Array(items) => Bson::Array(items.iter().map(json_to_bson).collect::<Option<Vec<_>>>()?),
        J::Object(map) => Bson::Document(map.iter().map(|(k, v)| Some((k.clone(), json_to_bson(v)?))).collect::<Option<Document>>()?),
    })
}

fn bson_as_string(v: &Bson) -> Option<String> {
    Some(bson_to_text(v))
}

/// BSON 值 → 文本（字符串原样，复合类型用 relaxed extended JSON）。
pub fn bson_to_text(v: &Bson) -> String {
    match v {
        Bson::String(s) => s.clone(),
        Bson::Int32(i) => i.to_string(),
        Bson::Int64(i) => i.to_string(),
        Bson::Double(f) => crate::values::format_float(*f),
        Bson::Boolean(b) => b.to_string(),
        Bson::ObjectId(o) => o.to_string(),
        Bson::DateTime(d) => chrono::DateTime::from_timestamp_millis(d.timestamp_millis())
            .map(|t| {
                t.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string()
            })
            .unwrap_or_default(),
        Bson::Null | Bson::Undefined => String::new(),
        Bson::Timestamp(t) => format!("Timestamp({}, {})", t.time, t.increment),
        Bson::JavaScriptCode(c) => c.clone(),
        other => bson_owned_to_text(other.clone()),
    }
}

/// 拥有值版本：直接使用 extended JSON。
fn bson_owned_to_text(v: Bson) -> String {
    use mongodb::bson::Bson as B;
    fn json_text(v: serde_json::Value) -> String {
        match v {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        }
    }
    match v {
        B::Array(items) => format!(
            "[{}]",
            items
                .into_iter()
                .map(bson_owned_to_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => json_text(other.into_relaxed_extjson()),
    }
}

/// 最小百分号编码（连接串用户名 / 密码）。
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
