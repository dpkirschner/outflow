//! Shared server state: the Store behind a mutex (rusqlite `Connection` is
//! `Send` but not `Sync` — same posture as the old Tauri backend), bridged
//! onto the async runtime with `spawn_blocking`.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use outflow_core::Store;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Config {
    pub db_path: String,
    pub listen: String,
    pub web_dir: String,
    /// 0600 JSON file mapping plaid item_id → access token.
    pub plaid_tokens_file: String,
    /// HTTPS redirect URI registered in the Plaid dashboard (OAuth banks).
    pub oauth_redirect: Option<String>,
    /// Optional bearer token required on /api/* (tailnet is the main boundary).
    pub api_token: Option<String>,
    pub sync_interval_secs: u64,
}

impl Config {
    pub fn from_env() -> Config {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let data_dir = env("OUTFLOW_DATA_DIR").unwrap_or_else(|| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
            format!("{home}/Library/Application Support/outflow")
        });
        Config {
            db_path: env("OUTFLOW_DB").unwrap_or_else(|| format!("{data_dir}/outflow.db")),
            listen: env("OUTFLOW_LISTEN").unwrap_or_else(|| "127.0.0.1:8080".into()),
            web_dir: env("OUTFLOW_WEB_DIR").unwrap_or_else(|| "app/dist".into()),
            plaid_tokens_file: env("OUTFLOW_PLAID_TOKENS_FILE")
                .unwrap_or_else(|| format!("{data_dir}/plaid-tokens.json")),
            oauth_redirect: env("OUTFLOW_OAUTH_REDIRECT"),
            api_token: env("OUTFLOW_API_TOKEN"),
            sync_interval_secs: env("OUTFLOW_SYNC_INTERVAL_SECS")
                .and_then(|v| v.parse().ok())
                .unwrap_or(21_600), // 6h
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Mutex<Store>>,
    /// Serializes sync runs (background interval vs manual pull).
    pub sync_lock: Arc<tokio::sync::Mutex<()>>,
    pub cfg: Arc<Config>,
}

/// API errors surface as `{ "error": "..." }`. 4xx/5xx split is coarse: bad
/// input errors are produced by handlers directly; everything from the domain
/// layer is a 500.
pub struct ApiError(pub StatusCode, pub String);

impl ApiError {
    pub fn internal(msg: impl std::fmt::Display) -> ApiError {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, msg.to_string())
    }

    pub fn bad_request(msg: impl std::fmt::Display) -> ApiError {
        ApiError(StatusCode::BAD_REQUEST, msg.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

pub type ApiResult<T> = Result<Json<T>, ApiError>;

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Run a closure against the Store on the blocking pool. Domain errors map to
/// 500s; handlers needing finer control call the store variantly and map
/// themselves.
pub async fn with_store<T, F>(state: &AppState, f: F) -> Result<T, ApiError>
where
    F: FnOnce(&Store) -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || {
        let guard = store.lock().map_err(|e| format!("store lock poisoned: {e}"))?;
        f(&guard)
    })
    .await
    .map_err(|e| ApiError::internal(format!("blocking task: {e}")))?
    .map_err(ApiError::internal)
}
