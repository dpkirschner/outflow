//! outflow server: JSON API + web client for a headless always-on box (the
//! mac-mini), fronted by `tailscale serve` for HTTPS on the tailnet. Binds
//! loopback by default — Tailscale is the network boundary; an optional bearer
//! token adds an app-level check.
//!
//! Headless posture: all config and secrets resolve from env / 0600 files —
//! never the keychain (locked without a GUI session) and never the DB.

mod plaid_routes;
mod routes;
mod state;
mod sync;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use outflow_core::Store;
use std::sync::{Arc, Mutex};
use tower_http::services::ServeDir;

use state::{AppState, Config};

fn open_db(cfg: &Config) -> Result<Store, String> {
    if let Some(dir) = std::path::Path::new(&cfg.db_path).parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    #[cfg(feature = "encryption")]
    {
        let key = resolve_db_key()?;
        Store::open_encrypted(&cfg.db_path, &key)
            .map_err(|e| format!("open encrypted db {}: {e}", cfg.db_path))
    }
    #[cfg(not(feature = "encryption"))]
    {
        Store::open(&cfg.db_path).map_err(|e| format!("open db {}: {e}", cfg.db_path))
    }
}

/// SQLCipher key for the encrypted build: `OUTFLOW_DB_KEY` env, else
/// `OUTFLOW_DB_KEY_FILE` (0600). No keychain on a headless box.
#[cfg(feature = "encryption")]
fn resolve_db_key() -> Result<String, String> {
    if let Ok(k) = std::env::var("OUTFLOW_DB_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    let path = std::env::var("OUTFLOW_DB_KEY_FILE")
        .ok()
        .filter(|p| !p.is_empty())
        .ok_or("encryption needs a key: set OUTFLOW_DB_KEY or OUTFLOW_DB_KEY_FILE (0600 file)")?;
    outflow_net::secrets::read_secret_file(&path)
}

/// Optional bearer check on /api/*. When `OUTFLOW_API_TOKEN` is unset this
/// layer passes everything through — the tailnet is the boundary.
async fn require_bearer(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(expected) = &state.cfg.api_token {
        let ok = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(|t| t == expected)
            .unwrap_or(false);
        if !ok {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(req).await)
}

/// Serve the SPA shell for any non-API, non-asset path — client-side routes
/// like /oauth-return must load the app, with a 200.
async fn spa_index(State(state): State<AppState>) -> Response {
    let index = format!("{}/index.html", state.cfg.web_dir);
    match tokio::fs::read(&index).await {
        Ok(body) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], body).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("web client not built ({index}: {e}) — run `npm run build` in app/"),
        )
            .into_response(),
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "outflow_server=info,tower_http=info".into()),
        )
        .init();

    let cfg = Config::from_env();
    let store = match open_db(&cfg) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("outflow-server: {e}");
            std::process::exit(1);
        }
    };
    let count = store.count_transactions().unwrap_or(-1);
    tracing::info!(db = %cfg.db_path, transactions = count, "opened database");

    let state = AppState {
        store: Arc::new(Mutex::new(store)),
        sync_lock: Arc::new(tokio::sync::Mutex::new(())),
        cfg: Arc::new(cfg.clone()),
    };

    // Periodic background sync. The first tick fires after one interval — on
    // boot the archive is already at most an interval stale, and startup isn't
    // serialized behind bank APIs.
    {
        let state = state.clone();
        tokio::spawn(async move {
            let period = std::time::Duration::from_secs(state.cfg.sync_interval_secs.max(300));
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            loop {
                ticker.tick().await;
                match sync::sync_all(&state).await {
                    Ok(report) => tracing::info!(?report, "background sync"),
                    Err(e) => tracing::warn!(error = %e.1, "background sync skipped"),
                }
            }
        });
    }

    let api = routes::api_router()
        .merge(plaid_routes::router())
        .layer(middleware::from_fn_with_state(state.clone(), require_bearer));

    // SPA fallback to index.html (status 200) is load-bearing: /oauth-return
    // must serve the app so Plaid Link can resume after an OAuth bank redirect.
    let app = Router::new()
        .nest("/api", api)
        .nest_service("/assets", ServeDir::new(format!("{}/assets", cfg.web_dir)))
        .fallback(spa_index)
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(&cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("outflow-server: bind {}: {e}", cfg.listen);
            std::process::exit(1);
        }
    };
    tracing::info!(listen = %cfg.listen, web = %cfg.web_dir, "outflow server up");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .expect("server error");
}
