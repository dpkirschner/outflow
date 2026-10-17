//! Card offers: evaluate the offers file(s) named by `OUTFLOW_PERKS` against
//! the archive. The file is re-read on every request — it's tiny and
//! hand-edited (flip `added = true`), so edits apply without a restart.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use outflow_core::{evaluate_perks, load_offers, PerkOptions, PerksReport};
use serde::Deserialize;
use std::path::PathBuf;

use crate::routes::window_since;
use crate::state::{now_secs, with_store, ApiError, ApiResult, AppState};

#[derive(Deserialize)]
struct PerksParams {
    /// History window for "do you shop here": `3mo`/`6mo`/`12mo`/`all`
    /// (default 12mo).
    lookback: Option<String>,
    /// Warn about added offers expiring within this many days (default 14).
    expiring_days: Option<i64>,
}

async fn perks(
    State(state): State<AppState>,
    Query(p): Query<PerksParams>,
) -> ApiResult<PerksReport> {
    let path: PathBuf = state
        .cfg
        .perks_path
        .clone()
        .ok_or_else(|| ApiError::bad_request("card offers not configured: set OUTFLOW_PERKS"))?
        .into();
    let lookback = p.lookback.as_deref().unwrap_or("12mo");
    if !matches!(lookback, "3mo" | "6mo" | "12mo" | "all") {
        return Err(ApiError::bad_request(format!(
            "bad lookback {lookback:?} (3mo|6mo|12mo|all)"
        )));
    }
    let now = now_secs();
    let opts = PerkOptions {
        since: window_since(lookback, now),
        expiring_days: p.expiring_days.unwrap_or(14).max(0),
    };
    with_store(&state, move |s| {
        let cards = load_offers(&path).map_err(|e| e.to_string())?;
        let accounts = s.accounts().map_err(|e| e.to_string())?;
        let txns = s.all_transactions().map_err(|e| e.to_string())?;
        Ok(evaluate_perks(&cards, &accounts, &txns, now, &opts))
    })
    .await
    .map(Json)
}

pub fn router() -> Router<AppState> {
    Router::new().route("/perks", get(perks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Config;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use outflow_core::{Account, AccountKind, Money, Store, Transaction, TxnFlag};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn app(perks_path: Option<String>) -> Router {
        let store = Store::open_in_memory().expect("in-memory store");
        store
            .upsert_accounts(&[Account {
                id: "visa".into(),
                org: "Bank".into(),
                name: "Sapphire (4707)".into(),
                kind: AccountKind::Credit,
                balance: Money::from_cents(0),
                currency: "USD".into(),
                last_synced: 0,
                source: "plaid".into(),
                nickname: None,
            }])
            .unwrap();
        let now = now_secs();
        store
            .upsert_transactions(&[Transaction {
                id: "t1".into(),
                account_id: "visa".into(),
                posted: now - 86_400,
                transacted_at: None,
                amount: Money::from_cents(-1199),
                description: "DROPBOX".into(),
                payee: Some("Dropbox".into()),
                category: None,
                category_source: None,
                pending: false,
                flag: TxnFlag::Spending,
                raw: "{}".into(),
                source: "plaid".into(),
            }])
            .unwrap();
        let cfg = Config {
            db_path: ":memory:".into(),
            listen: "127.0.0.1:0".into(),
            web_dir: "app/dist".into(),
            plaid_tokens_file: "/dev/null".into(),
            oauth_redirect: None,
            api_token: None,
            api_token_ro: None,
            sync_interval_secs: 21_600,
            perks_path,
        };
        let state = AppState {
            store: Arc::new(Mutex::new(store)),
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            cfg: Arc::new(cfg),
        };
        router().with_state(state)
    }

    async fn get(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let res = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn unconfigured_is_a_400() {
        let (status, body) = get(app(None), "/perks").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("OUTFLOW_PERKS"));
    }

    #[tokio::test]
    async fn rejects_unknown_lookback() {
        let (status, _) = get(app(Some("/nonexistent".into())), "/perks?lookback=2wk").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn evaluates_the_offers_file() {
        let path = std::env::temp_dir().join(format!("outflow-perks-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"[card]
issuer = "American Express"
mask = "1006"
captured = 2026-01-01
offers = [
  { merchant = "Dropbox", pct = 40, max = "30.00", expires = 2099-12-31, match = ["dropbox"] },
]
"#,
        )
        .unwrap();
        let (status, body) = get(app(Some(path.display().to_string())), "/perks").await;
        std::fs::remove_file(&path).ok();
        assert_eq!(status, StatusCode::OK, "{body}");
        // No (1006) account seeded: reported, but AddOffer still works.
        assert!(body["cards"][0]["problem"]
            .as_str()
            .unwrap()
            .contains("(1006)"));
        let f = &body["findings"][0];
        assert_eq!(f["kind"], "add_offer");
        assert_eq!(
            f["offer"]["reward"],
            serde_json::json!({"kind": "pct", "value": 40})
        );
        assert_eq!(f["est_cash_cents"], 479);
        assert_eq!(f["matched_merchants"][0], "Dropbox");
    }

    #[tokio::test]
    async fn bad_offers_file_is_an_error() {
        let (status, body) = get(app(Some("/nonexistent/perks.toml".into())), "/perks").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("/nonexistent/perks.toml"));
    }
}
