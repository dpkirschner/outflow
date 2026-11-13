//! Find money: every Finding (core::find). The offers file(s) named by
//! `OUTFLOW_PERKS` are re-read on every request: they're tiny and hand-edited
//! (flip `added = true`), so edits apply without a restart.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use outflow_core::{
    find, finding, search_transactions, set_dismissed, set_reviewed, today_local, FindError,
    FindOptions, FindReport, Period, TxnPage,
};
use serde::Deserialize;
use std::path::PathBuf;

use crate::routes::{blank, ids_query};
use crate::state::{now_secs, with_store, ApiError, ApiResult, AppState};

#[derive(Deserialize)]
struct FindParams {
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
    /// Flag added offers expiring within this many days (default 14).
    expiring_days: Option<i64>,
}

impl FindParams {
    /// The last day the estimates read, and the options.
    fn resolve(&self) -> Result<(chrono::NaiveDate, FindOptions), ApiError> {
        let today = today_local();
        let through = Period::resolve(
            blank(self.period.as_deref()),
            blank(self.since.as_deref()),
            blank(self.until.as_deref()),
            today,
        )
        .map_err(ApiError::bad_request)?
        .map_or(today, |period| period.last_day());
        let opts = FindOptions {
            expiring_days: self.expiring_days.unwrap_or(14).max(0),
        };
        Ok((through, opts))
    }
}

/// Every Finding, estimated over the 12 months ending with the period's last
/// month (default: this month).
async fn find_money(
    State(state): State<AppState>,
    Query(p): Query<FindParams>,
) -> ApiResult<FindReport> {
    let (through, opts) = p.resolve()?;
    let path: Option<PathBuf> = state.cfg.perks_path.clone().map(PathBuf::from);
    let now = now_secs();
    with_store(&state, move |s| {
        find(s, path.as_deref(), through, now, &opts).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct ChargesParams {
    /// `Finding::key`.
    key: String,
    sort: Option<String>,
    dir: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

/// The charges behind one Finding (its `txn_ids`), as a `/transactions`
/// page. The Finding is looked up by key in the report for the `GET /find`
/// params (open, dismissed or reviewed); 404 when it isn't there.
async fn finding_charges(
    State(state): State<AppState>,
    Query(p): Query<FindParams>,
    Query(c): Query<ChargesParams>,
) -> ApiResult<TxnPage> {
    let key = c.key.trim().to_string();
    if key.is_empty() {
        return Err(ApiError::bad_request("key is empty"));
    }
    let (through, opts) = p.resolve()?;
    let path: Option<PathBuf> = state.cfg.perks_path.clone().map(PathBuf::from);
    let now = now_secs();
    let res = with_store(&state, move |s| {
        Ok(finding(s, path.as_deref(), through, now, &opts, &key))
    })
    .await?;
    let f = match res {
        Ok(f) => f,
        Err(e @ FindError::NoSuchFinding(_)) => {
            return Err(ApiError(StatusCode::NOT_FOUND, e.to_string()))
        }
        Err(e) => return Err(ApiError::internal(e)),
    };
    let q = ids_query(f.txn_ids, c.sort, c.dir, c.offset, c.limit)?;
    with_store(&state, move |s| {
        search_transactions(s, &q).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct DismissedBody {
    /// `Finding::key`.
    key: String,
    dismissed: bool,
}

/// Dismiss a Finding (it must be in the report for these params) or restore
/// it; returns the report after the change. 404 for a key not in the report.
async fn set_finding_dismissed(
    State(state): State<AppState>,
    Query(p): Query<FindParams>,
    Json(body): Json<DismissedBody>,
) -> ApiResult<FindReport> {
    let key = body.key.trim().to_string();
    if key.is_empty() {
        return Err(ApiError::bad_request("key is empty"));
    }
    let (through, opts) = p.resolve()?;
    let path: Option<PathBuf> = state.cfg.perks_path.clone().map(PathBuf::from);
    let now = now_secs();
    let res = with_store(&state, move |s| {
        Ok(set_dismissed(
            s,
            path.as_deref(),
            through,
            now,
            &opts,
            &key,
            body.dismissed,
        ))
    })
    .await?;
    match res {
        Ok(report) => Ok(Json(report)),
        Err(e @ FindError::NoSuchFinding(_)) => Err(ApiError(StatusCode::NOT_FOUND, e.to_string())),
        Err(e) => Err(ApiError::internal(e)),
    }
}

#[derive(Deserialize)]
struct ReviewedBody {
    /// Merchant, any spelling (normalized like `RecurringMerchant::merchant`).
    merchant: String,
    reviewed: bool,
}

/// Mark a merchant Reviewed (it needs a subscription or price rise Finding
/// in the report for these params) or clear the mark; returns the report
/// after the change. 404 for a merchant with no such Finding.
async fn set_merchant_reviewed(
    State(state): State<AppState>,
    Query(p): Query<FindParams>,
    Json(body): Json<ReviewedBody>,
) -> ApiResult<FindReport> {
    let merchant = body.merchant.trim().to_string();
    if merchant.is_empty() {
        return Err(ApiError::bad_request("merchant is empty"));
    }
    let (through, opts) = p.resolve()?;
    let path: Option<PathBuf> = state.cfg.perks_path.clone().map(PathBuf::from);
    let now = now_secs();
    let res = with_store(&state, move |s| {
        Ok(set_reviewed(
            s,
            path.as_deref(),
            through,
            now,
            &opts,
            &merchant,
            body.reviewed,
        ))
    })
    .await?;
    match res {
        Ok(report) => Ok(Json(report)),
        Err(e @ FindError::NoSuchSubscription(_)) => {
            Err(ApiError(StatusCode::NOT_FOUND, e.to_string()))
        }
        Err(e) => Err(ApiError::internal(e)),
    }
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/find", get(find_money))
        .route("/find/charges", get(finding_charges))
        .route("/find/dismissed", post(set_finding_dismissed))
        .route("/find/reviewed", post(set_merchant_reviewed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Config;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use chrono::{Datelike, TimeZone};
    use outflow_core::{Account, AccountKind, Money, Store, Transaction, TxnFlag};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt;

    fn app(perks_path: Option<String>) -> Router {
        app_with(perks_path, false)
    }

    fn app_with(perks_path: Option<String>, with_subscription: bool) -> Router {
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
        // A steady monthly subscription: 4 charges, a month apart, ending
        // last month so none is late.
        let this_month = today_local().with_day(1).unwrap();
        let subs: Vec<Transaction> = (1..=4u32)
            .map(|i| {
                let day = (this_month - chrono::Months::new(i)).with_day(10).unwrap();
                Transaction {
                    id: format!("s{i}"),
                    account_id: "visa".into(),
                    posted: chrono::Local
                        .from_local_datetime(&day.and_hms_opt(12, 0, 0).unwrap())
                        .earliest()
                        .unwrap()
                        .timestamp(),
                    transacted_at: None,
                    amount: Money::from_cents(-1599),
                    description: "STREAMFLIX".into(),
                    payee: Some("Streamflix".into()),
                    category: None,
                    category_source: None,
                    pending: false,
                    flag: TxnFlag::Spending,
                    raw: "{}".into(),
                    pfc_primary: None,
                    pfc_detailed: None,
                    source: "plaid".into(),
                }
            })
            .collect();
        if with_subscription {
            store.upsert_transactions(&subs).unwrap();
        }
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

    async fn post(
        app: Router,
        uri: &str,
        body: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let req = Request::post(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or_default())
    }

    /// Dismiss moves the Finding to `dismissed` and survives a fresh GET;
    /// restore brings it back; an unknown key is a 404.
    #[tokio::test]
    async fn dismisses_and_restores_a_finding() {
        let path =
            std::env::temp_dir().join(format!("outflow-dismiss-{}.toml", std::process::id()));
        std::fs::write(
            &path,
            r#"[card]
issuer = "Demo Issuer"
mask = "4242"
captured = 2026-01-01
offers = [
  { merchant = "Dropbox", back = "30.00", expires = 2099-12-31, match = ["dropbox"] },
]
"#,
        )
        .unwrap();
        let app = app(Some(path.display().to_string()));
        let key = "add_offer:4242:dropbox";
        let dismiss = |on: bool| serde_json::json!({ "key": key, "dismissed": on });

        let (status, body) = post(app.clone(), "/find/dismissed", dismiss(true)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["findings"], serde_json::json!([]));
        assert_eq!(body["dismissed"][0]["key"], key);
        let (_, body) = get(app.clone(), "/find").await;
        assert_eq!(body["dismissed"][0]["key"], key);

        let (status, body) = post(app.clone(), "/find/dismissed", dismiss(false)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["findings"][0]["key"], key);
        assert_eq!(body["dismissed"], serde_json::json!([]));

        let unknown = serde_json::json!({ "key": "add_offer:4242:nope", "dismissed": true });
        let (status, body) = post(app.clone(), "/find/dismissed", unknown).await;
        std::fs::remove_file(&path).ok();
        assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    }

    /// A steady monthly charge is a subscription Finding with no offers
    /// file; Reviewed moves it to `reviewed` and back; an unknown merchant
    /// is a 404.
    #[tokio::test]
    async fn reviews_and_unreviews_a_subscription() {
        let app = app_with(None, true);
        let (_, body) = get(app.clone(), "/find").await;
        let f = &body["findings"][0];
        assert_eq!(f["kind"], "subscription", "{body}");
        assert_eq!(f["key"], "subscription::streamflix");
        assert_eq!(f["evidence"]["source"], "subscription");
        assert_eq!(f["est_cents_per_year"], 12 * 1599);

        let review = |m: &str, on: bool| serde_json::json!({ "merchant": m, "reviewed": on });
        let (status, body) = post(app.clone(), "/find/reviewed", review("STREAMFLIX", true)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["reviewed"][0]["key"], "subscription::streamflix");
        assert!(body["reviewed"][0]["evidence"]["reviewed_at"].is_i64());
        let (_, body) = get(app.clone(), "/find").await;
        assert_eq!(body["reviewed"][0]["key"], "subscription::streamflix");

        let (status, body) = post(app.clone(), "/find/reviewed", review("streamflix", false)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["findings"][0]["key"], "subscription::streamflix");
        assert_eq!(body["reviewed"], serde_json::json!([]));

        let (status, _) = post(app.clone(), "/find/reviewed", review("nobody", true)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = post(app, "/find/reviewed", review(" ", true)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    /// The charges behind a Finding resolve by key, sorted and paged like
    /// `/transactions`, also once it's Reviewed; an unknown key is a 404.
    #[tokio::test]
    async fn lists_a_findings_charges_by_key() {
        let app = app_with(None, true);
        let (_, body) = get(app.clone(), "/find").await;
        let ids = body["findings"][0]["txn_ids"].as_array().unwrap().clone();
        let uri = "/find/charges?key=subscription::streamflix&dir=desc&limit=2";
        let (status, page) = get(app.clone(), uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["total"], ids.len());
        assert_eq!(page["items"].as_array().unwrap().len(), 2);
        assert_eq!(page["total_cents"], -1599 * ids.len() as i64);
        assert_eq!(page["items"][0]["id"], "s1", "newest first");

        let review = serde_json::json!({ "merchant": "streamflix", "reviewed": true });
        post(app.clone(), "/find/reviewed", review).await;
        let (status, page) = get(app.clone(), uri).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        assert_eq!(page["total"], ids.len());

        let (status, _) = get(app.clone(), "/find/charges?key=subscription::nobody").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = get(app, "/find/charges?key=%20").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
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
