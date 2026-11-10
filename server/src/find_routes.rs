//! Find money: every Finding (core::find). The offers file(s) named by
//! `OUTFLOW_PERKS` are re-read on every request: they're tiny and hand-edited
//! (flip `added = true`), so edits apply without a restart.

use axum::extract::{Query, State};
use axum::routing::get;
use axum::{Json, Router};
use outflow_core::{find, today_local, FindOptions, FindReport, Period};
use serde::Deserialize;
use std::path::PathBuf;

use crate::routes::blank;
use crate::state::{now_secs, with_store, ApiError, ApiResult, AppState};

#[derive(Deserialize)]
struct FindParams {
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
    /// Flag added offers expiring within this many days (default 14).
    expiring_days: Option<i64>,
}

/// Every Finding, estimated over the 12 months ending with the period's last
/// month (default: this month).
async fn find_money(
    State(state): State<AppState>,
    Query(p): Query<FindParams>,
) -> ApiResult<FindReport> {
    let today = today_local();
    let through = Period::resolve(
        blank(p.period.as_deref()),
        blank(p.since.as_deref()),
        blank(p.until.as_deref()),
        today,
    )
    .map_err(ApiError::bad_request)?
    .map_or(today, |period| period.last_day());
    let opts = FindOptions {
        expiring_days: p.expiring_days.unwrap_or(14).max(0),
    };
    let path: Option<PathBuf> = state.cfg.perks_path.clone().map(PathBuf::from);
    let now = now_secs();
    with_store(&state, move |s| {
        find(s, path.as_deref(), through, now, &opts).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

pub fn router() -> Router<AppState> {
    Router::new().route("/find", get(find_money))
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
                name: "Travel Card (5555)".into(),
                kind: AccountKind::Credit,
                balance: Money::from_cents(0),
                currency: "USD".into(),
                last_synced: 0,
                source: "plaid".into(),
                nickname: None,
                hidden: false,
                item_id: None,
                kind_from_bank: None,
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
                pfc_primary: None,
                pfc_detailed: None,
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
    async fn unconfigured_offers_is_a_note() {
        let (status, body) = get(app(None), "/find").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["notes"][0].as_str().unwrap().contains("OUTFLOW_PERKS"));
        assert_eq!(body["findings"], serde_json::json!([]));
    }

    #[tokio::test]
    async fn rejects_a_bad_period() {
        let (status, _) = get(app(None), "/find?period=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn evaluates_the_offers_file() {
        let path = std::env::temp_dir().join(format!("outflow-find-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"[card]
issuer = "American Express"
mask = "4242"
captured = 2026-01-01
offers = [
  { merchant = "Dropbox", back = "30.00", expires = 2099-12-31, match = ["dropbox"] },
]
"#,
        )
        .unwrap();
        let (status, body) = get(app(Some(path.display().to_string())), "/find").await;
        std::fs::remove_file(&path).ok();
        assert_eq!(status, StatusCode::OK, "{body}");
        // No (4242) account seeded: reported, but AddOffer still works.
        assert!(body["cards"][0]["problem"]
            .as_str()
            .unwrap()
            .contains("(4242)"));
        let f = &body["findings"][0];
        assert_eq!(f["kind"], "add_offer");
        assert_eq!(f["key"], "add_offer:4242:dropbox");
        assert_eq!(f["est_cents_per_year"], 3000);
        assert_eq!(f["txn_ids"], serde_json::json!(["t1"]));
        assert_eq!(f["evidence"]["source"], "offer");
        assert_eq!(
            f["evidence"]["offer"]["reward"],
            serde_json::json!({"kind": "back", "value": 3000})
        );
        assert_eq!(f["evidence"]["matched_merchants"][0], "Dropbox");
    }

    #[tokio::test]
    async fn bad_offers_file_is_an_error() {
        let (status, body) = get(app(Some("/nonexistent/perks.toml".into())), "/find").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("/nonexistent/perks.toml"));
    }
}
