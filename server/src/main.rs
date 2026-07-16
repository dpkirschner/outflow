//! outflow server: JSON API + web client for a headless always-on box (the
//! mac-mini), fronted by `tailscale serve` for HTTPS on the tailnet. Binds
//! loopback by default — Tailscale is the network boundary; an optional bearer
//! token adds an app-level check.
//!
//! Headless posture: all config and secrets resolve from env / 0600 files —
//! never the keychain (locked without a GUI session) and never the DB.

mod match_routes;
mod plaid_routes;
mod routes;
mod state;
mod sync;

use axum::extract::{Request, State};
use axum::http::{header, Method, StatusCode};
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

/// Compare without an early exit on the first differing byte, so a token can't
/// be recovered byte-by-byte from response timing. Length is not secret (both
/// tokens are fixed-width random hex), so returning early on a length mismatch
/// is fine.
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Optional bearer check on /api/*, in two tiers:
///
/// - `OUTFLOW_API_TOKEN`    — full access.
/// - `OUTFLOW_API_TOKEN_RO` — GET only; 403 on any mutation.
///
/// With **neither** set this layer passes everything through — the tailnet is
/// the boundary, which is the zero-config single-user default. Setting either
/// one flips the whole /api surface to deny-by-default.
///
/// The read-only tier keys off the HTTP method rather than a route allowlist:
/// every read in this API is a GET and every mutation is a POST/DELETE, so a
/// newly added mutating route is denied to the RO token the day it lands, with
/// no list for anyone to remember to update.
async fn require_bearer(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let (full, ro) = (&state.cfg.api_token, &state.cfg.api_token_ro);
    if full.is_none() && ro.is_none() {
        return Ok(next.run(req).await);
    }

    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    if full.as_deref().is_some_and(|t| ct_eq(presented, t)) {
        return Ok(next.run(req).await);
    }
    if ro.as_deref().is_some_and(|t| ct_eq(presented, t)) {
        if req.method() == Method::GET {
            return Ok(next.run(req).await);
        }
        return Err(StatusCode::FORBIDDEN);
    }
    Err(StatusCode::UNAUTHORIZED)
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
        .merge(match_routes::router())
        // Unknown /api paths must 404 as JSON — never fall through to the SPA
        // shell (a 200 HTML page masquerading as an API response).
        .fallback(|| async {
            (
                StatusCode::NOT_FOUND,
                axum::Json(serde_json::json!({ "error": "no such API route" })),
            )
        })
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request as HttpRequest;
    use axum::routing::{delete, get, post};
    use tower::ServiceExt; // for `oneshot`

    const FULL: &str = "fulltoken0000000000000000000000f";
    const RO: &str = "readonlytoken00000000000000000ro";

    /// A stand-in for the real /api surface: one GET, one POST, one DELETE, all
    /// behind the same layer main() applies.
    fn app(full: Option<&str>, ro: Option<&str>) -> Router {
        let store = Store::open_in_memory().expect("in-memory store");
        let cfg = Config {
            db_path: ":memory:".into(),
            listen: "127.0.0.1:0".into(),
            web_dir: "app/dist".into(),
            plaid_tokens_file: "/dev/null".into(),
            oauth_redirect: None,
            api_token: full.map(Into::into),
            api_token_ro: ro.map(Into::into),
            sync_interval_secs: 21_600,
        };
        let state = AppState {
            store: Arc::new(Mutex::new(store)),
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            cfg: Arc::new(cfg),
        };
        Router::new()
            .route("/read", get(|| async { "ok" }))
            .route("/write", post(|| async { "ok" }))
            .route("/remove", delete(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, require_bearer))
    }

    async fn status(app: Router, method: Method, uri: &str, token: Option<&str>) -> StatusCode {
        let mut b = HttpRequest::builder().method(method).uri(uri);
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        app.oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// Both tiers configured — the production shape.
    async fn tiered(method: Method, uri: &str, token: &str) -> StatusCode {
        status(app(Some(FULL), Some(RO)), method, uri, Some(token)).await
    }

    // With no tokens configured the layer is a no-op: the tailnet is the
    // boundary. This is the zero-config default and the dev posture.
    #[tokio::test]
    async fn no_tokens_configured_passes_everything() {
        assert_eq!(
            status(app(None, None), Method::GET, "/read", None).await,
            StatusCode::OK
        );
        assert_eq!(
            status(app(None, None), Method::POST, "/write", None).await,
            StatusCode::OK
        );
    }

    // Setting any token flips the whole surface to deny-by-default.
    #[tokio::test]
    async fn full_token_required_once_configured() {
        assert_eq!(
            status(app(Some(FULL), None), Method::GET, "/read", None).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(app(Some(FULL), None), Method::GET, "/read", Some("wrong")).await,
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status(app(Some(FULL), None), Method::GET, "/read", Some(FULL)).await,
            StatusCode::OK
        );
        assert_eq!(
            status(app(Some(FULL), None), Method::POST, "/write", Some(FULL)).await,
            StatusCode::OK
        );
    }

    // The point of the RO tier: reads pass, mutations are refused. 403 (not
    // 401) so the caller can tell "your token is wrong" from "your token is
    // real but may not do this".
    #[tokio::test]
    async fn ro_token_reads_but_cannot_mutate() {
        assert_eq!(tiered(Method::GET, "/read", RO).await, StatusCode::OK);
        assert_eq!(
            tiered(Method::POST, "/write", RO).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            tiered(Method::DELETE, "/remove", RO).await,
            StatusCode::FORBIDDEN
        );
    }

    // Both tiers coexist: the full token is unaffected by the RO token existing.
    #[tokio::test]
    async fn full_token_still_mutates_alongside_ro() {
        assert_eq!(tiered(Method::POST, "/write", FULL).await, StatusCode::OK);
        assert_eq!(
            tiered(Method::DELETE, "/remove", FULL).await,
            StatusCode::OK
        );
        assert_eq!(
            tiered(Method::GET, "/read", "nope").await,
            StatusCode::UNAUTHORIZED
        );
    }

    // An RO token alone is a valid config: a read-only server for everyone.
    #[tokio::test]
    async fn ro_token_alone_denies_writes_to_all() {
        assert_eq!(
            status(app(None, Some(RO)), Method::GET, "/read", Some(RO)).await,
            StatusCode::OK
        );
        assert_eq!(
            status(app(None, Some(RO)), Method::POST, "/write", Some(RO)).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status(app(None, Some(RO)), Method::GET, "/read", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn ct_eq_matches_only_identical_strings() {
        assert!(ct_eq(FULL, FULL));
        assert!(!ct_eq(FULL, RO));
        assert!(!ct_eq("abc", "abcd")); // length mismatch
        assert!(!ct_eq("abc", "abd")); // last byte differs
        assert!(ct_eq("", ""));
    }
}
