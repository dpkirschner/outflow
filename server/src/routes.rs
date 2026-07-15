//! JSON API routes — a 1:1 port of the former Tauri command layer, so the web
//! client runs identical domain logic to the old desktop app. Wire format is
//! the serde shape of the core types: snake_case fields, enum variant names as
//! strings, Money as a bare number of cents.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use outflow_core::{
    detect, monthly_flow, parse_fixture, spend_by_category, top_merchants, Account,
    CategorySource, CategorySpend, LedgerView, Mark, MerchantSpend, MonthlyFlow, Subscription,
    SyncEntry, Transaction, TxnFilter, TxnFlag,
};
use serde::{Deserialize, Serialize};

use crate::state::{now_secs, with_store, ApiError, ApiResult, AppState};

/// Rolling-window control → a `since` bound (epoch secs). ~30.5-day months is a
/// coarse filter; the ledger's month/rate math is exact via `chrono::Local`.
const MONTH_SECS: i64 = 2_635_200;
pub fn window_since(window: &str, now: i64) -> Option<i64> {
    match window {
        "all" => None,
        "3mo" => Some(now - 3 * MONTH_SECS),
        "12mo" => Some(now - 12 * MONTH_SECS),
        _ => Some(now - 6 * MONTH_SECS), // default 6mo
    }
}

/// Date/pending filter from query params. Epoch **seconds**. A missing param
/// set means "everything" (include pending) — mirrors the old `to_filter`
/// guard against the derived-Default trap.
#[derive(Debug, Default, Deserialize)]
pub struct FilterParams {
    pub since: Option<i64>,
    pub until: Option<i64>,
    /// Include pending rows (default true).
    pub pending: Option<bool>,
    /// Include Transfer/CardPayment rows in aggregations (default false).
    pub transfers: Option<bool>,
}

impl FilterParams {
    pub fn to_filter(&self) -> TxnFilter {
        TxnFilter {
            since: self.since,
            until: self.until,
            include_pending: self.pending.unwrap_or(true),
            include_non_spending: self.transfers.unwrap_or(false),
        }
    }
}

#[derive(Serialize)]
pub struct CategorizeResult {
    rule: usize,
    remaining: usize,
}

#[derive(Serialize)]
pub struct PullResult {
    pub added: usize,
    pub updated: usize,
    pub accounts: usize,
    pub warnings: Vec<String>,
}

async fn accounts(State(state): State<AppState>) -> ApiResult<Vec<Account>> {
    with_store(&state, |s| s.accounts().map_err(|e| e.to_string()))
        .await
        .map(Json)
}

/// Full search/sort/filter over the transaction list (the Outflows screen and
/// the CLI `txns` subcommand). Semantics live in `core::query::search_transactions`.
// NOTE: filter fields are inlined, NOT `#[serde(flatten)] FilterParams`. axum's
// Query uses serde_urlencoded, whose flatten support buffers every value as a
// string and then can't coerce it back to i64/bool — a flattened `since=...`
// makes the whole query fail to deserialize (a 400 the SPA sees as broken
// JSON). Inline fields deserialize directly. Keep the four names in sync with
// FilterParams; `to_filter()` below reuses its single default-guard.
#[derive(Debug, Default, Deserialize)]
struct TxnQueryParams {
    since: Option<i64>,
    until: Option<i64>,
    pending: Option<bool>,
    transfers: Option<bool>,
    q: Option<String>,
    account: Option<String>,
    category: Option<String>,
    source: Option<String>,
    /// serde variant name, e.g. "Transfer" — implies include_non_spending.
    flag: Option<TxnFlag>,
    min_cents: Option<i64>,
    max_cents: Option<i64>,
    sort: Option<String>,
    dir: Option<String>, // "asc" | "desc" (default desc)
    offset: Option<usize>,
    limit: Option<usize>,
}

impl TxnQueryParams {
    fn to_filter(&self) -> TxnFilter {
        FilterParams {
            since: self.since,
            until: self.until,
            pending: self.pending,
            transfers: self.transfers,
        }
        .to_filter()
    }
}

async fn transactions(
    State(state): State<AppState>,
    Query(p): Query<TxnQueryParams>,
) -> ApiResult<outflow_core::TxnPage> {
    let sort = match p.sort.as_deref() {
        None => outflow_core::SortKey::Date,
        Some(s) => outflow_core::SortKey::from_str(s)
            .ok_or_else(|| ApiError::bad_request(format!("bad sort key {s:?}")))?,
    };
    let mut filter = p.to_filter();
    // Filtering BY a suppressed flag only makes sense with those rows visible.
    if matches!(p.flag, Some(TxnFlag::Transfer) | Some(TxnFlag::CardPayment)) {
        filter.include_non_spending = true;
    }
    let q = outflow_core::TxnQuery {
        filter,
        text: p.q.filter(|s| !s.trim().is_empty()),
        account_id: p.account,
        category: p.category,
        source: p.source,
        flag: p.flag,
        min_cents: p.min_cents,
        max_cents: p.max_cents,
        sort,
        descending: p.dir.as_deref() != Some("asc"),
        offset: p.offset.unwrap_or(0),
        limit: p.limit.unwrap_or(100).min(1000),
    };
    with_store(&state, move |s| {
        outflow_core::search_transactions(s, &q).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

async fn categorize(State(state): State<AppState>) -> ApiResult<CategorizeResult> {
    with_store(&state, |s| {
        let rules = s.ruleset().map_err(|e| e.to_string())?;
        let rule = s
            .categorize_uncategorized(&rules, CategorySource::Rule)
            .map_err(|e| e.to_string())?;
        let remaining = s.uncategorized().map_err(|e| e.to_string())?.len();
        Ok(CategorizeResult { rule, remaining })
    })
    .await
    .map(Json)
}

async fn spend_categories(
    State(state): State<AppState>,
    Query(params): Query<FilterParams>,
) -> ApiResult<Vec<CategorySpend>> {
    let f = params.to_filter();
    with_store(&state, move |s| spend_by_category(s, &f).map_err(|e| e.to_string()))
        .await
        .map(Json)
}

// Inlined filter fields, not flatten — see the note on TxnQueryParams.
#[derive(Deserialize)]
struct MerchantParams {
    since: Option<i64>,
    until: Option<i64>,
    pending: Option<bool>,
    transfers: Option<bool>,
    limit: Option<usize>,
}

async fn merchants(
    State(state): State<AppState>,
    Query(params): Query<MerchantParams>,
) -> ApiResult<Vec<MerchantSpend>> {
    let f = FilterParams {
        since: params.since,
        until: params.until,
        pending: params.pending,
        transfers: params.transfers,
    }
    .to_filter();
    let limit = params.limit.unwrap_or(15);
    with_store(&state, move |s| top_merchants(s, &f, limit).map_err(|e| e.to_string()))
        .await
        .map(Json)
}

async fn flow(
    State(state): State<AppState>,
    Query(params): Query<FilterParams>,
) -> ApiResult<Vec<MonthlyFlow>> {
    let f = params.to_filter();
    with_store(&state, move |s| monthly_flow(s, &f).map_err(|e| e.to_string()))
        .await
        .map(Json)
}

async fn subscriptions(State(state): State<AppState>) -> ApiResult<Vec<Subscription>> {
    with_store(&state, |s| {
        let txns = s.all_transactions().map_err(|e| e.to_string())?;
        Ok(detect(&txns))
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct WindowParam {
    window: Option<String>,
}

/// The whole rhythm-ledger view for a rolling window (`3mo`/`6mo`/`12mo`/`all`).
async fn ledger(
    State(state): State<AppState>,
    Query(p): Query<WindowParam>,
) -> ApiResult<LedgerView> {
    let now = now_secs();
    let since = window_since(p.window.as_deref().unwrap_or("6mo"), now);
    with_store(&state, move |s| {
        outflow_core::ledger(s, since, now).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct StreamParams {
    merchant: String,
    window: Option<String>,
}

async fn stream_occurrences(
    State(state): State<AppState>,
    Query(p): Query<StreamParams>,
) -> ApiResult<Vec<Transaction>> {
    let since = window_since(p.window.as_deref().unwrap_or("6mo"), now_secs());
    with_store(&state, move |s| {
        s.stream_occurrences(&p.merchant, since).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

/// The `Stream` a noise merchant WOULD become if promoted — cadence, rate, and
/// rhythm strip — without committing the mark. Powers the Noise popover preview.
/// `null` when the merchant has no spending occurrences in the window.
async fn stream_preview(
    State(state): State<AppState>,
    Query(p): Query<StreamParams>,
) -> ApiResult<Option<outflow_core::Stream>> {
    let now = now_secs();
    let since = window_since(p.window.as_deref().unwrap_or("6mo"), now);
    with_store(&state, move |s| {
        outflow_core::stream_preview(s, &p.merchant, since, now).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct MarkBody {
    merchant: String,
    mark: Mark,
}

async fn mark_stream(
    State(state): State<AppState>,
    Json(body): Json<MarkBody>,
) -> ApiResult<()> {
    with_store(&state, move |s| {
        s.set_merchant_mark(&body.merchant, body.mark).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct MerchantBody {
    merchant: String,
}

async fn clear_stream_mark(
    State(state): State<AppState>,
    Json(body): Json<MerchantBody>,
) -> ApiResult<()> {
    with_store(&state, move |s| {
        s.clear_merchant_mark(&body.merchant).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

async fn categories(State(state): State<AppState>) -> ApiResult<Vec<String>> {
    with_store(&state, |s| s.categories().map_err(|e| e.to_string()))
        .await
        .map(Json)
}

#[derive(Deserialize)]
struct CategoryBody {
    category: String,
    #[serde(default)]
    learn: bool,
}

async fn set_category(
    State(state): State<AppState>,
    Path(txn_id): Path<String>,
    Json(body): Json<CategoryBody>,
) -> ApiResult<Option<i64>> {
    with_store(&state, move |s| {
        s.set_manual_category(&txn_id, &body.category, body.learn)
            .map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct FlagBody {
    /// Serde variant name: "Spending" | "Transfer" | "CardPayment".
    flag: TxnFlag,
    #[serde(default)]
    learn: bool,
}

async fn set_flag(
    State(state): State<AppState>,
    Path(txn_id): Path<String>,
    Json(body): Json<FlagBody>,
) -> ApiResult<Option<i64>> {
    with_store(&state, move |s| {
        s.set_flag(&txn_id, body.flag, body.learn).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

async fn apply_flags(State(state): State<AppState>) -> ApiResult<usize> {
    with_store(&state, |s| s.apply_flags().map_err(|e| e.to_string()))
        .await
        .map(Json)
}

async fn has_credit_account(State(state): State<AppState>) -> ApiResult<bool> {
    with_store(&state, |s| s.has_credit_account().map_err(|e| e.to_string()))
        .await
        .map(Json)
}

async fn reset_data(State(state): State<AppState>) -> ApiResult<()> {
    with_store(&state, |s| s.reset_data().map_err(|e| e.to_string()))
        .await
        .map(Json)
}

#[derive(Deserialize)]
struct LimitParam {
    limit: Option<usize>,
}

async fn sync_log(
    State(state): State<AppState>,
    Query(p): Query<LimitParam>,
) -> ApiResult<Vec<SyncEntry>> {
    let limit = p.limit.unwrap_or(50);
    with_store(&state, move |s| s.sync_log(limit).map_err(|e| e.to_string()))
        .await
        .map(Json)
}

#[derive(Deserialize)]
struct PullFromFileBody {
    path: String,
}

/// Dev/offline pull: ingest a Plaid fixture envelope from a local file (the
/// `--from-file` path) — accounts + one sync page, no credentials needed.
async fn pull_from_file(
    State(state): State<AppState>,
    Json(body): Json<PullFromFileBody>,
) -> ApiResult<PullResult> {
    let json = std::fs::read_to_string(&body.path)
        .map_err(|err| ApiError::bad_request(format!("read {}: {err}", body.path)))?;
    let started = now_secs();
    let fixture = parse_fixture(&json, started)
        .map_err(|err| ApiError::bad_request(format!("parse: {err:?}")))?;
    with_store(&state, move |s| {
        let log_id = s.log_sync_start("plaid:file", started).map_err(|e| e.to_string())?;
        s.upsert_accounts(&fixture.accounts).map_err(|e| e.to_string())?;
        let mut txns = fixture.sync.added;
        txns.extend(fixture.sync.modified);
        let r = s.upsert_transactions(&txns).map_err(|e| e.to_string())?;
        let mut removed = fixture.sync.removed_ids;
        removed.extend(fixture.sync.pending_superseded);
        s.delete_transactions(&removed).map_err(|e| e.to_string())?;
        s.log_sync_finish(log_id, now_secs(), r.added, r.updated, None)
            .map_err(|e| e.to_string())?;
        Ok(PullResult {
            added: r.added,
            updated: r.updated,
            accounts: fixture.accounts.len(),
            warnings: Vec::new(),
        })
    })
    .await
    .map(Json)
}

/// Manual "pull now": SimpleFIN (if configured) — Plaid items join in the sync
/// engine (Phase 5 wiring lives in `sync.rs`; this route delegates there).
async fn pull(State(state): State<AppState>) -> ApiResult<crate::sync::SyncReport> {
    crate::sync::sync_all(&state).await.map(Json)
}

/// LLM pass over the merchants the rule pass couldn't match. Needs
/// `ANTHROPIC_API_KEY`. Returns the count newly categorized.
async fn categorize_llm(State(state): State<AppState>) -> ApiResult<usize> {
    use outflow_core::{normalize_payee, LlmCategorizer, MerchantSample};
    use outflow_net::AnthropicPrompter;
    use std::collections::HashMap;

    with_store(&state, |store| {
        let uncategorized = store.uncategorized().map_err(|e| e.to_string())?;
        if uncategorized.is_empty() {
            return Ok(0);
        }
        // One decision per distinct merchant; keep a representative amount.
        let mut samples: Vec<MerchantSample> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        for t in &uncategorized {
            let key = normalize_payee(t.merchant());
            if key.is_empty() || seen.insert(key.clone(), ()).is_some() {
                continue;
            }
            samples.push(MerchantSample {
                merchant: key,
                amount_cents: t.amount.cents(),
            });
        }
        let categories = store.categories().map_err(|e| e.to_string())?;
        if categories.is_empty() {
            return Err("no categories in vocabulary; nothing to constrain the LLM to".into());
        }
        let categorizer = LlmCategorizer::new(AnthropicPrompter::from_env()?, categories);
        let suggestions = categorizer
            .suggest(&samples)
            .map_err(|err| format!("llm: {err:?}"))?;
        let by_merchant: HashMap<String, String> = suggestions
            .into_iter()
            .map(|s| (s.merchant, s.category))
            .collect();
        let mut applied = 0usize;
        for t in &uncategorized {
            let key = normalize_payee(t.merchant());
            if let Some(cat) = by_merchant.get(&key) {
                store
                    .set_category(&t.id, cat, CategorySource::Llm)
                    .map_err(|e| e.to_string())?;
                applied += 1;
            }
        }
        Ok(applied)
    })
    .await
    .map(Json)
}

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/accounts", get(accounts))
        .route("/transactions", get(transactions))
        .route("/spend/categories", get(spend_categories))
        .route("/merchants", get(merchants))
        .route("/flow", get(flow))
        .route("/subscriptions", get(subscriptions))
        .route("/ledger", get(ledger))
        .route("/stream_occurrences", get(stream_occurrences))
        .route("/stream_preview", get(stream_preview))
        .route("/categories", get(categories))
        .route("/has_credit_account", get(has_credit_account))
        .route("/sync_log", get(sync_log))
        .route("/categorize", post(categorize))
        .route("/categorize_llm", post(categorize_llm))
        .route("/txn/{txn_id}/category", post(set_category))
        .route("/txn/{txn_id}/flag", post(set_flag))
        .route("/apply_flags", post(apply_flags))
        .route("/mark_stream", post(mark_stream))
        .route("/clear_stream_mark", post(clear_stream_mark))
        .route("/pull", post(pull))
        .route("/pull_from_file", post(pull_from_file))
        .route("/reset_data", post(reset_data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use outflow_core::{Money, Store};
    use std::sync::{Arc, Mutex};
    use tower::ServiceExt; // for `oneshot`

    fn txn(id: &str, cents: i64, payee: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct-1".into(),
            posted: 1_720_000_000,
            transacted_at: Some(1_720_000_000),
            amount: Money::from_cents(cents),
            description: payee.into(),
            payee: Some(payee.into()),
            category: Some("Dining".into()),
            category_source: Some(CategorySource::Rule),
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            source: "plaid".into(),
        }
    }

    fn app_with_txns(txns: &[Transaction]) -> Router {
        let store = Store::open_in_memory().expect("in-memory store");
        store.upsert_transactions(txns).expect("seed txns");
        let cfg = crate::state::Config {
            db_path: ":memory:".into(),
            listen: "127.0.0.1:0".into(),
            web_dir: "app/dist".into(),
            plaid_tokens_file: "/dev/null".into(),
            oauth_redirect: None,
            api_token: None,
            sync_interval_secs: 21_600,
        };
        let state = AppState {
            store: Arc::new(Mutex::new(store)),
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            cfg: Arc::new(cfg),
        };
        api_router().with_state(state)
    }

    async fn get_json(app: Router, uri: &str) -> (StatusCode, serde_json::Value) {
        let res = app
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = if bytes.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                panic!("body was not JSON ({e}): {:?}", String::from_utf8_lossy(&bytes))
            })
        };
        (status, body)
    }

    // Regression: the Outflows tab query carries flattened FilterParams fields
    // (since/until, i64) alongside sort/dir/offset/limit. axum's serde_urlencoded
    // Query rejected these with a 400 plaintext "Failed to deserialize query
    // string" body; axum_extra's serde_html_form Query must accept them.
    #[tokio::test]
    async fn transactions_accepts_flattened_filter_params() {
        let app = app_with_txns(&[txn("t1", -1500, "Blue Bottle"), txn("t2", -900, "Sightglass")]);
        let (status, body) = get_json(
            app,
            "/transactions?since=1719810000&until=1722488400&sort=date&dir=desc&offset=0&limit=50",
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(body["total"], 2);
        assert_eq!(body["items"].as_array().unwrap().len(), 2);
    }

    // The category filter (also on the Outflows tab) rides on the same handler.
    #[tokio::test]
    async fn transactions_filters_by_category() {
        let mut other = txn("t3", -2000, "Whole Foods");
        other.category = Some("Groceries".into());
        let app = app_with_txns(&[txn("t1", -1500, "Blue Bottle"), other]);
        let (status, body) = get_json(app, "/transactions?category=Dining&limit=50").await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(body["total"], 1);
        assert_eq!(body["items"][0]["payee"], "Blue Bottle");
    }

    // The /merchants handler flattens FilterParams too — same fix, same guard.
    #[tokio::test]
    async fn merchants_accepts_flattened_filter_params() {
        let app = app_with_txns(&[txn("t1", -1500, "Blue Bottle"), txn("t2", -900, "Sightglass")]);
        let (status, body) =
            get_json(app, "/merchants?since=1719810000&until=1722488400&limit=5").await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(body.is_array());
    }

    // A bad sort key is still a clean 400 with a JSON error body (handler-level
    // validation), not an extractor rejection.
    #[tokio::test]
    async fn transactions_bad_sort_key_is_json_400() {
        let app = app_with_txns(&[txn("t1", -1500, "Blue Bottle")]);
        let (status, body) = get_json(app, "/transactions?sort=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("bogus"));
    }

    // The Noise popover preview: /stream_preview synthesizes a Stream for a
    // merchant keyed by its normalized name, with no min-occurrence gate.
    #[tokio::test]
    async fn stream_preview_returns_a_stream_for_a_noise_merchant() {
        let app = app_with_txns(&[
            txn("u1", -600, "Uber"),
            txn("u2", -700, "Uber"),
            txn("u3", -650, "Uber"),
        ]);
        let (status, body) = get_json(app, "/stream_preview?merchant=uber&window=all").await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert_eq!(body["merchant"], "uber");
        assert_eq!(body["occurrence_count"], 3);
        assert!(body["cadence"].is_string());
    }

    // Unknown merchant → null (JSON `null`, a 200), not an error.
    #[tokio::test]
    async fn stream_preview_null_for_unknown_merchant() {
        let app = app_with_txns(&[txn("u1", -600, "Uber")]);
        let (status, body) = get_json(app, "/stream_preview?merchant=nobody&window=all").await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_null());
    }
}
