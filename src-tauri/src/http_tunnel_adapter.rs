//! Certified SQL-over-HTTPS is explicitly separate from ordinary HTTP adapters.
use std::sync::Arc;
use anyhow::{bail, Result};
use async_trait::async_trait;
use crate::{http_db::{HttpAdapter, HttpColumn, HttpDb, HttpRows}, http_tunnel::{TunnelConfig, TunnelRuntime}, http_tunnel_protocol as wire, models::ConnectionRequest};

pub(crate) struct HttpTunnelAdapter { runtime: Arc<TunnelRuntime>, read_only: bool }
impl HttpTunnelAdapter {
    pub(crate) async fn connect(req: &ConnectionRequest, config: TunnelConfig, read_only: bool) -> Result<HttpDb> {
        let runtime = TunnelRuntime::connect(config).await?;
        // Opening verifies remote session identity, target generation and permissions.
        if let Err(error) = runtime.session("", read_only).await { runtime.shutdown().await; return Err(error); }
        HttpDb::from_adapter(req, Box::new(Self { runtime, read_only })).await
    }
    async fn catalog(&self, namespace: &str) -> Result<Vec<wire::CatalogTable>> {
        self.runtime.session(namespace, true).await?.catalog().await
    }
    async fn columns(&self, namespace: &str, name: &str) -> Result<crate::http_db::relational::JdbcMetadataSnapshot> {
        let session = self.runtime.session(namespace, true).await?;
        session.metadata(wire::ObjectId { catalog: session.catalog.clone(), schema: session.binding.namespace.clone(), name: name.to_owned() }).await
    }
}
fn identifier(value: &str) -> Result<String> {
    if value.is_empty() || value.len() > 200 || value.chars().any(char::is_control) { bail!("HTTP_TUNNEL_OBJECT_INVALID"); }
    Ok(format!("\"{}\"", value.replace('"', "\"\"")))
}
#[async_trait]
impl HttpAdapter for HttpTunnelAdapter {
    fn shares_session_namespace(&self) -> bool { true }
    fn tunnel_runtime(&self) -> Option<Arc<TunnelRuntime>> { Some(self.runtime.clone()) }
    async fn shutdown(&self) { self.runtime.shutdown().await; }
    async fn ping(&self) -> Result<String> {
        Ok(format!("H2 HTTPS SQL tunnel v1 ({})", self.runtime.capabilities.implementation))
    }
    async fn containers(&self) -> Result<Vec<(String, String)>> {
        Ok(self.runtime.namespaces().into_iter().map(|name| (name, "已授权的远端命名空间".into())).collect())
    }
    async fn objects(&self, container: &str) -> Result<Vec<String>> {
        Ok(self.catalog(container).await?.into_iter().map(|table| table.name).collect())
    }
    async fn object_types(&self, container: &str) -> Result<Option<Vec<(String, String)>>> {
        Ok(Some(self.catalog(container).await?.into_iter().map(|table| (table.name, table.kind)).collect()))
    }
    async fn table_columns(&self, container: &str, object: &str) -> Result<Option<Vec<HttpColumn>>> {
        let metadata = self.columns(container, object).await?;
        if metadata.completeness.values().any(|value| value != "known") { bail!("METADATA_UNKNOWN: 远端列目录不完整"); }
        Ok(Some(metadata.columns.iter().map(|column| HttpColumn {
            name: column.name.clone(), data_type: column.type_name.clone(), nullable: column.nullable.unwrap_or(true), primary_key: metadata.primary_key.contains(&column.name),
        }).collect()))
    }
    async fn rows(&self, container: &str, object: &str, offset: u64, limit: u64) -> Result<HttpRows> {
        if limit == 0 || limit > wire::MAX_ROWS { bail!("HTTP_TUNNEL_ROW_LIMIT"); }
        // Catalog authorization precedes browsing; views with unknown effects stay explicit.
        let catalog = self.catalog(container).await?;
        if !catalog.iter().any(|table| table.name == object && table.kind == "table") { bail!("HTTP_TUNNEL_BROWSE_UNPROVEN: 只能结构化浏览已核对的实体表"); }
        let session = self.runtime.session(container, true).await?;
        let sql = format!("SELECT * FROM {}.{} LIMIT {} OFFSET {}", identifier(&session.binding.namespace)?, identifier(object)?, limit, offset);
        let mut result = session.execute(sql, Vec::new(), 60_000).await?;
        if result.sets.len() != 1 { bail!("HTTP_TUNNEL_RESULT_INVALID"); }
        let set = result.sets.remove(0);
        Ok(HttpRows { columns: set.columns.into_iter().map(|column| column.name).collect(), rows: set.rows })
    }
    async fn run(&self, _container: &str, _object: &str, _sql: &str) -> Result<HttpRows> {
        let _ = self.read_only;
        bail!("HTTP_TUNNEL_CONTROLLED_EXECUTION_REQUIRED: 自由SQL必须通过C1授权、独立取消和已验证的远端回执")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn table_identifiers_preserve_case_and_quote_without_sql_injection() {
        assert_eq!(identifier("T\"中").unwrap(), "\"T\"\"中\"");
        assert!(identifier("").is_err()); assert!(identifier("t\n").is_err());
    }
}
