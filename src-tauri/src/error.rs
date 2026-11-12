//! 统一错误类型：Tauri 命令必须返回可序列化的错误。

use serde::Serialize;

/// 应用错误。
#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub message: String,
    pub code: String,
    pub operation_id: Option<String>,
    pub retryable: bool,
}

impl AppError {
    pub fn new(msg: impl Into<String>) -> Self {
        AppError {
            message: msg.into(),
            code: "APPLICATION_ERROR".into(), operation_id: None, retryable: false,
        }
    }
    pub fn coded(code: &str, msg: impl Into<String>) -> Self {
        Self { code: code.into(), ..Self::new(msg) }
    }
    pub fn operation(mut self, id: &str) -> Self { self.operation_id = Some(id.into()); self }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}

impl From<anyhow::Error> for AppError {
    fn from(e: anyhow::Error) -> Self {
        if let Some(error) = e.downcast_ref::<AppError>() { return error.clone(); }
        if let Some(error) = e.downcast_ref::<sqlx::Error>() {
            let code = if matches!(error, sqlx::Error::PoolTimedOut) { "POOL_ACQUIRE_TIMEOUT" }
                else if error.as_database_error().and_then(|e| e.code()).is_some_and(|c| c.as_ref() == "5") { "SQLITE_BUSY_TIMEOUT" }
                else { "QUERY_FAILED" };
            return AppError::coded(code, error.to_string());
        }
        AppError::new(format!("{:#}", e))
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        let code = if matches!(e, sqlx::Error::PoolTimedOut) { "POOL_ACQUIRE_TIMEOUT" }
            else if e.as_database_error().and_then(|e| e.code()).is_some_and(|c| c.as_ref() == "5") { "SQLITE_BUSY_TIMEOUT" }
            else { "QUERY_FAILED" };
        AppError::coded(code, e.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::new(e.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        AppError::new(e.to_string())
    }
}

/// 命令返回类型。
pub type CmdResult<T> = Result<T, AppError>;
