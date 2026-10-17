//! JSON API routes — a 1:1 port of the former Tauri command layer, so the web
//! client runs identical domain logic to the old desktop app. Wire format is
//! the serde shape of the core types: snake_case fields, enum variant names as
//! strings, Money as a bare number of cents.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use outflow_core::{
    balance_history, detect, monthly_flow, parse_fixture, spend_by_category, summarize_balances,
    top_merchants, Account, AccountKind, BalancesView, CategorySource, CategorySpend, DayTotals,
    LedgerView, Mark, MerchantSpend, MonthlyFlow, Subscription, SyncEntry, Transaction, TxnFilter,
    TxnFlag,
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

#[derive(Deserialize)]
struct AccountsParams {
    /// Include hidden accounts (the Connections screen). Default false.
    include_hidden: Option<bool>,
}

async fn accounts(
    State(state): State<AppState>,
    Query(p): Query<AccountsParams>,
) -> ApiResult<Vec<Account>> {
    let all = p.include_hidden.unwrap_or(false);
    with_store(&state, move |s| {
        if all { s.all_accounts() } else { s.accounts() }.map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

/// Current state of every visible account: balances, cash vs invested,
/// holdings, idle-cash flags. Hidden accounts and their holdings drop out at
/// the store reads.
async fn balances(State(state): State<AppState>) -> ApiResult<BalancesView> {
    with_store(&state, |s| {
        let accounts = s.accounts().map_err(|e| e.to_string())?;
        let securities = s.securities().map_err(|e| e.to_string())?;
        let holdings = s.current_holdings().map_err(|e| e.to_string())?;
        Ok(summarize_balances(&accounts, &securities, &holdings))
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct HistoryParams {
    /// Epoch seconds; omit for the full history.
    since: Option<i64>,
}

/// Daily net worth / cash / invested / credit from the per-sync snapshots.
async fn balances_history(
    State(state): State<AppState>,
    Query(p): Query<HistoryParams>,
) -> ApiResult<Vec<DayTotals>> {
    with_store(&state, move |s| {
        let kinds: std::collections::HashMap<String, AccountKind> = s
            .accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|a| (a.id, a.kind))
            .collect();
        let snaps = s.balance_snapshots(p.since).map_err(|e| e.to_string())?;
        Ok(balance_history(&snaps, &kinds))
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct HiddenBody {
    hidden: bool,
}

/// Hide (or un-hide) an account everywhere: its rows drop out of every list,
/// aggregate and dropdown. Data keeps syncing; un-hiding restores it all.
async fn set_account_hidden(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Json(body): Json<HiddenBody>,
) -> ApiResult<()> {
    let found = with_store(&state, {
        let id = account_id.clone();
        move |s| {
            s.set_account_hidden(&id, body.hidden)
                .map_err(|e| e.to_string())
        }
    })
    .await?;
    if !found {
        return Err(ApiError(
            axum::http::StatusCode::NOT_FOUND,
            format!("no account {account_id}"),
        ));
    }
    Ok(Json(()))
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
    with_store(&state, move |s| {
        spend_by_category(s, &f).map_err(|e| e.to_string())
    })
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
    with_store(&state, move |s| {
        top_merchants(s, &f, limit).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

async fn flow(
    State(state): State<AppState>,
    Query(params): Query<FilterParams>,
) -> ApiResult<Vec<MonthlyFlow>> {
    let f = params.to_filter();
    with_store(&state, move |s| {
        monthly_flow(s, &f).map_err(|e| e.to_string())
    })
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
        s.stream_occurrences(&p.merchant, since)
            .map_err(|e| e.to_string())
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

async fn mark_stream(State(state): State<AppState>, Json(body): Json<MarkBody>) -> ApiResult<()> {
    with_store(&state, move |s| {
        s.set_merchant_mark(&body.merchant, body.mark)
            .map_err(|e| e.to_string())
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
        s.clear_merchant_mark(&body.merchant)
            .map_err(|e| e.to_string())
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
        s.set_flag(&txn_id, body.flag, body.learn)
            .map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct NicknameBody {
    /// Free-form nickname; empty/whitespace clears it (chip reverts to name).
    nickname: String,
}

async fn set_account_nickname(
    State(state): State<AppState>,
    Path(account_id): Path<String>,
    Json(body): Json<NicknameBody>,
) -> ApiResult<()> {
    with_store(&state, move |s| {
        let nn = body.nickname.trim();
        s.set_account_nickname(&account_id, (!nn.is_empty()).then_some(nn))
            .map_err(|e| e.to_string())
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
    with_store(&state, |s| {
        s.has_credit_account().map_err(|e| e.to_string())
    })
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
    with_store(&state, move |s| {
        s.sync_log(limit).map_err(|e| e.to_string())
    })
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
        let log_id = s
            .log_sync_start("plaid:file", started)
            .map_err(|e| e.to_string())?;
        s.upsert_accounts(&fixture.accounts)
            .map_err(|e| e.to_string())?;
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
        .route("/balances", get(balances))
        .route("/balances/history", get(balances_history))
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
        .route("/account/{account_id}/nickname", post(set_account_nickname))
        .route("/account/{account_id}/hidden", post(set_account_hidden))
        .route("/apply_flags", post(apply_flags))
        .route("/mark_stream", post(mark_stream))
        .route("/clear_stream_mark", post(clear_stream_mark))
        .route("/pull", post(pull))
        .route("/pull_from_file", post(pull_from_file))
        .route("/reset_data", post(reset_data))
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
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
        app_with_store(store)
    }

    fn app_with_store(store: Store) -> Router {
        let cfg = crate::state::Config {
            db_path: ":memory:".into(),
            listen: "127.0.0.1:0".into(),
            web_dir: "app/dist".into(),
            plaid_tokens_file: "/dev/null".into(),
            oauth_redirect: None,
            api_token: None,
            api_token_ro: None,
            sync_interval_secs: 21_600,
            perks_path: None,
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
                panic!(
                    "body was not JSON ({e}): {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, body)
    }

    /// Accounts acct-1 (visible) and acct-2 (to be hidden), one charge each,
    /// plus a proposed card-payment pair whose bank leg is on acct-2. Routes
    /// from both `api_router` and `match_routes` so match review is covered.
    fn app_with_two_accounts() -> Router {
        let store = Store::open_in_memory().expect("in-memory store");
        let acct = |id: &str| Account {
            id: id.into(),
            org: "Bank".into(),
            name: format!("{id} (0000)"),
            kind: outflow_core::AccountKind::Checking,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
        };
        store
            .upsert_accounts(&[acct("acct-1"), acct("acct-2")])
            .unwrap();
        let mut other = txn("t2", -900, "Sightglass");
        other.account_id = "acct-2".into();
        store
            .upsert_transactions(&[txn("t1", -1500, "Blue Bottle"), other])
            .unwrap();
        store
            .insert_match(
                "t2",
                "t1",
                outflow_core::MatchStatus::Proposed,
                outflow_core::MatchConfidence::Medium,
                None,
                0,
            )
            .unwrap();
        let cfg = crate::state::Config {
            db_path: ":memory:".into(),
            listen: "127.0.0.1:0".into(),
            web_dir: "app/dist".into(),
            plaid_tokens_file: "/dev/null".into(),
            oauth_redirect: None,
            api_token: None,
            api_token_ro: None,
            sync_interval_secs: 21_600,
            perks_path: None,
        };
        let state = AppState {
            store: Arc::new(Mutex::new(store)),
            sync_lock: Arc::new(tokio::sync::Mutex::new(())),
            cfg: Arc::new(cfg),
        };
        Router::new()
            .merge(api_router())
            .merge(crate::match_routes::router())
            .with_state(state)
    }

    async fn post_status(app: Router, uri: &str, body: serde_json::Value) -> StatusCode {
        app.oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap()
        .status()
    }

    /// /balances groups accounts, counts a money-market holding and an
    /// all-cash HSA as cash, flags idle cash; /balances/history has one point
    /// per day of snapshots.
    #[tokio::test]
    async fn balances_report_cash_holdings_and_history() {
        use outflow_core::{AccountKind, Holding, Security};
        let store = Store::open_in_memory().unwrap();
        let acct = |id: &str, kind: AccountKind, cents: i64| Account {
            id: id.into(),
            org: "Bank".into(),
            name: format!("{id} (0000)"),
            kind,
            balance: Money::from_cents(cents),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
        };
        let accounts = [
            acct("chk", AccountKind::Checking, 2_000_00),
            acct("card", AccountKind::Credit, -300_00),
            acct("brk", AccountKind::Investment, 10_000_00),
            acct("hsa", AccountKind::Investment, 4_478_25),
        ];
        store.upsert_accounts(&accounts).unwrap();
        let secs = [
            Security {
                security_id: "vmfxx".into(),
                ticker: Some("VMFXX".into()),
                name: "Money Market".into(),
                sec_type: "mutual fund".into(),
                is_cash: true,
            },
            Security {
                security_id: "vti".into(),
                ticker: Some("VTI".into()),
                name: "Total Market".into(),
                sec_type: "etf".into(),
                is_cash: false,
            },
        ];
        let pos = |sec: &str, cents: i64| Holding {
            account_id: "brk".into(),
            security_id: sec.into(),
            quantity: "1".into(),
            price: "1".into(),
            value: Money::from_cents(cents),
            cost_basis: None,
        };
        let holds = [pos("vmfxx", 100_00), pos("vti", 9_900_00)];
        store
            .record_item_snapshot(&accounts, Some((&secs, &holds)), 1_000)
            .unwrap();
        store
            .record_item_snapshot(&accounts, None, 1_000 + 2 * 86_400)
            .unwrap();
        let app = app_with_store(store);

        let (status, v) = get_json(app.clone(), "/balances").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["totals"]["net_worth"], 16_178_25);
        assert_eq!(v["totals"]["cash"], 2_000_00 + 100_00 + 4_478_25);
        assert_eq!(v["totals"]["credit"], -300_00);
        let groups: Vec<&str> = v["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["group"].as_str().unwrap())
            .collect();
        assert_eq!(groups, ["cash", "investments", "credit"]);
        let inv = &v["groups"][1]["accounts"];
        let by_id = |id: &str| {
            inv.as_array()
                .unwrap()
                .iter()
                .find(|a| a["account"]["id"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(by_id("hsa")["idle_cash"], true);
        assert_eq!(by_id("brk")["idle_cash"], false); // $100 is 1%
        assert_eq!(by_id("brk")["holdings"][0]["label"], "VTI");
        assert_eq!(by_id("brk")["holdings"][1]["is_cash"], true);

        let (status, h) = get_json(app, "/balances/history").await;
        assert_eq!(status, StatusCode::OK);
        let days = h.as_array().unwrap();
        assert_eq!(days.len(), 3); // two sync days + the carried-forward gap
        assert_eq!(days[2]["net_worth"], 16_178_25);
    }

    #[tokio::test]
    async fn hidden_account_disappears_everywhere_but_connections() {
        let app = app_with_two_accounts();
        let hide = serde_json::json!({ "hidden": true });
        assert_eq!(
            post_status(app.clone(), "/account/acct-2/hidden", hide.clone()).await,
            StatusCode::OK
        );

        // Dropdowns / balances read /accounts: hidden account gone.
        let (_, accts) = get_json(app.clone(), "/accounts").await;
        let ids: Vec<&str> = accts
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["acct-1"]);
        // Connections asks for everything and sees the flag.
        let (_, all) = get_json(app.clone(), "/accounts?include_hidden=true").await;
        let all = all.as_array().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(
            all.iter().find(|a| a["id"] == "acct-2").unwrap()["hidden"],
            true
        );

        // Transaction list, aggregates and match review drop its rows.
        let (_, page) = get_json(app.clone(), "/transactions").await;
        assert_eq!(page["total"], 1);
        assert_eq!(page["items"][0]["id"], "t1");
        let (_, spend) = get_json(app.clone(), "/spend/categories").await;
        let total: i64 = spend
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["total_cents"].as_i64().unwrap())
            .sum();
        assert_eq!(total.abs(), 1500);
        let (_, matches) = get_json(app.clone(), "/matches").await;
        assert_eq!(matches.as_array().unwrap().len(), 0);

        // Un-hiding brings it all back.
        let show = serde_json::json!({ "hidden": false });
        post_status(app.clone(), "/account/acct-2/hidden", show).await;
        let (_, page) = get_json(app.clone(), "/transactions").await;
        assert_eq!(page["total"], 2);
        let (_, matches) = get_json(app.clone(), "/matches").await;
        assert_eq!(matches.as_array().unwrap().len(), 1);

        // Unknown account is a 404, not a silent no-op.
        assert_eq!(
            post_status(app, "/account/nope/hidden", hide).await,
            StatusCode::NOT_FOUND
        );
    }

    // Regression: the Outflows tab query carries flattened FilterParams fields
    // (since/until, i64) alongside sort/dir/offset/limit. axum's serde_urlencoded
    // Query rejected these with a 400 plaintext "Failed to deserialize query
    // string" body; axum_extra's serde_html_form Query must accept them.
    #[tokio::test]
    async fn transactions_accepts_flattened_filter_params() {
        let app = app_with_txns(&[
            txn("t1", -1500, "Blue Bottle"),
            txn("t2", -900, "Sightglass"),
        ]);
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
        let app = app_with_txns(&[
            txn("t1", -1500, "Blue Bottle"),
            txn("t2", -900, "Sightglass"),
        ]);
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
