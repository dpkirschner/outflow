//! JSON API routes — a 1:1 port of the former Tauri command layer, so the web
//! client runs identical domain logic to the old desktop app. Wire format is
//! the serde shape of the core types: snake_case fields, enum variant names as
//! strings, Money as a bare number of cents.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use outflow_core::{
    activity, balance_history, flow_lanes, position_history, positions, spend_by_category,
    summarize_balances, today_local, top_merchants, Account, AccountKind, ActivityFilter,
    ActivityRow, BalancesView, CategorySource, CategorySpend, DayTotals, FlowMonth, MerchantSpend,
    Period, PositionFilter, PositionPoint, PositionRow, RecurringView, SyncEntry, TxnFilter,
    TxnFlag,
};
use serde::{Deserialize, Serialize};

use crate::state::{now_secs, with_store, ApiError, ApiResult, AppState};

/// The perks lookback (`3mo`/`6mo`/`12mo`/`all`) → a `since` bound (epoch
/// secs), with coarse ~30.5-day months.
const MONTH_SECS: i64 = 2_635_200;
pub fn window_since(window: &str, now: i64) -> Option<i64> {
    match window {
        "all" => None,
        "3mo" => Some(now - 3 * MONTH_SECS),
        "12mo" => Some(now - 12 * MONTH_SECS),
        _ => Some(now - 6 * MONTH_SECS), // default 6mo
    }
}

/// `None` for a missing or blank query value.
fn blank(o: Option<&str>) -> Option<&str> {
    o.map(str::trim).filter(|s| !s.is_empty())
}

/// A bare integer (optional leading `-`): the legacy epoch-seconds form.
fn is_epoch(s: &str) -> bool {
    let digits = s.strip_prefix('-').unwrap_or(s);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// The shared date range on the read endpoints, as epoch seconds
/// `[since, until)`. Two forms:
///
/// - `period=2026-11` (any [`Period::parse`] spelling) or
///   `since=YYYY-MM-DD&until=YYYY-MM-DD` with `until` inclusive, resolved by
///   [`Period::resolve`] to local-midnight bounds. Neither means all history.
/// - Legacy: `since` / `until` as bare epoch seconds (since inclusive, until
///   exclusive), passed through unchanged. Both given values must then be
///   integers, and `period` must be absent.
fn resolve_range(
    period: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<(Option<i64>, Option<i64>), ApiError> {
    let (period, since, until) = (blank(period), blank(since), blank(until));
    if since.is_some_and(is_epoch) || until.is_some_and(is_epoch) {
        if period.is_some() {
            return Err(ApiError::bad_request(outflow_core::PeriodError::Ambiguous));
        }
        let epoch = |o: Option<&str>| -> Result<Option<i64>, ApiError> {
            o.map(|s| {
                s.parse::<i64>()
                    .ok()
                    .filter(|_| is_epoch(s))
                    .ok_or_else(|| {
                        ApiError::bad_request(format!(
                            "bad bound {s:?}: give since and until both as epoch seconds \
                         or both as YYYY-MM-DD"
                        ))
                    })
            })
            .transpose()
        };
        return Ok((epoch(since)?, epoch(until)?));
    }
    Ok(match resolve_period(period, since, until)? {
        Some(p) => {
            let (start, end) = p.bounds();
            (Some(start), Some(end))
        }
        None => (None, None),
    })
}

/// [`Period::resolve`] against today, with errors as 400s.
fn resolve_period(
    period: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
) -> Result<Option<Period>, ApiError> {
    Period::resolve(period, since, until, today_local()).map_err(ApiError::bad_request)
}

/// Date/pending filter from query params: `period` or `since`/`until` (see
/// [`resolve_range`]). A missing param set means "everything" (include
/// pending) — mirrors the old `to_filter` guard against the derived-Default
/// trap.
#[derive(Debug, Default, Deserialize)]
pub struct FilterParams {
    pub period: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
    /// Include pending rows (default true).
    pub pending: Option<bool>,
    /// Include Transfer/CardPayment rows in aggregations (default false).
    pub transfers: Option<bool>,
}

impl FilterParams {
    pub fn to_filter(&self) -> Result<TxnFilter, ApiError> {
        let (since, until) = resolve_range(
            self.period.as_deref(),
            self.since.as_deref(),
            self.until.as_deref(),
        )?;
        Ok(TxnFilter {
            since,
            until,
            include_pending: self.pending.unwrap_or(true),
            include_non_spending: self.transfers.unwrap_or(false),
        })
    }
}

/// `POST /api/categorize`: rows the rules categorized, and how many are
/// still uncategorized (the LLM tail).
#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CategorizeResult {
    pub rule: usize,
    pub remaining: usize,
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
    /// `period` or `since`/`until` (see [`resolve_range`]); omit for the full
    /// history.
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
}

#[derive(Deserialize)]
struct HoldingsParams {
    /// Account id or (substring of) its display label.
    account: Option<String>,
    /// Ticker or security id.
    ticker: Option<String>,
    /// `period` or `since`/`until` (see [`resolve_range`]);
    /// `/holdings/history` only.
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
}

impl HoldingsParams {
    fn filter(&self) -> PositionFilter {
        PositionFilter {
            account: self.account.clone().filter(|s| !s.is_empty()),
            ticker: self.ticker.clone().filter(|s| !s.is_empty()),
        }
    }
}

/// Every current position on visible accounts, flattened with security
/// detail (prices + as-of dates, cost basis, unit cost, gain), largest first.
async fn holdings(
    State(state): State<AppState>,
    Query(p): Query<HoldingsParams>,
) -> ApiResult<Vec<PositionRow>> {
    with_store(&state, move |s| {
        let accounts = s.accounts().map_err(|e| e.to_string())?;
        let securities = s.securities().map_err(|e| e.to_string())?;
        let holdings = s.current_holdings_synced().map_err(|e| e.to_string())?;
        Ok(positions(&accounts, &securities, &holdings, &p.filter()))
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct ActivityParams {
    account: Option<String>,
    ticker: Option<String>,
    /// Plaid type or subtype (buy, sell, dividend, contribution, fee, ...).
    #[serde(rename = "type")]
    kind: Option<String>,
    /// `period`, or inclusive `YYYY-MM-DD` `since`/`until` bounds.
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
}

/// Archived investment transactions on visible accounts, newest first.
async fn investment_activity(
    State(state): State<AppState>,
    Query(p): Query<ActivityParams>,
) -> ApiResult<Vec<ActivityRow>> {
    let nonempty = |o: Option<String>| o.filter(|s| !s.is_empty());
    let range = resolve_period(p.period.as_deref(), p.since.as_deref(), p.until.as_deref())?;
    let f = ActivityFilter {
        account: nonempty(p.account),
        ticker: nonempty(p.ticker),
        kind: nonempty(p.kind),
        since: range.map(|r| r.start.format("%Y-%m-%d").to_string()),
        until: range.map(|r| r.last_day().format("%Y-%m-%d").to_string()),
    };
    with_store(&state, move |s| {
        let accounts = s.accounts().map_err(|e| e.to_string())?;
        let securities = s.securities().map_err(|e| e.to_string())?;
        let txns = s.investment_txns().map_err(|e| e.to_string())?;
        Ok(activity(&txns, &accounts, &securities, &f))
    })
    .await
    .map(Json)
}

/// Per-sync position history from `holding_snapshots`, oldest first.
async fn holdings_history(
    State(state): State<AppState>,
    Query(p): Query<HoldingsParams>,
) -> ApiResult<Vec<PositionPoint>> {
    let (since, until) =
        resolve_range(p.period.as_deref(), p.since.as_deref(), p.until.as_deref())?;
    with_store(&state, move |s| {
        let accounts = s.accounts().map_err(|e| e.to_string())?;
        let securities = s.securities().map_err(|e| e.to_string())?;
        let snaps = s
            .holding_snapshots(since, until)
            .map_err(|e| e.to_string())?;
        Ok(position_history(
            &snaps,
            &accounts,
            &securities,
            &p.filter(),
        ))
    })
    .await
    .map(Json)
}

/// Daily net worth / cash / invested / credit from the per-sync snapshots.
async fn balances_history(
    State(state): State<AppState>,
    Query(p): Query<HistoryParams>,
) -> ApiResult<Vec<DayTotals>> {
    let (since, until) =
        resolve_range(p.period.as_deref(), p.since.as_deref(), p.until.as_deref())?;
    with_store(&state, move |s| {
        let kinds: std::collections::HashMap<String, AccountKind> = s
            .accounts()
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|a| (a.id, a.kind))
            .collect();
        let snaps = s
            .balance_snapshots(since, until)
            .map_err(|e| e.to_string())?;
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
// JSON). Inline fields deserialize directly. Keep the five names in sync with
// FilterParams; `to_filter()` below reuses its single default-guard.
#[derive(Debug, Default, Deserialize)]
struct TxnQueryParams {
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
    pending: Option<bool>,
    transfers: Option<bool>,
    q: Option<String>,
    account: Option<String>,
    category: Option<String>,
    source: Option<String>,
    /// serde variant name, e.g. "Transfer" — implies include_non_spending.
    flag: Option<TxnFlag>,
    /// Flow lane, e.g. "card_payment": exactly the rows behind that figure.
    /// Parsed in the handler (JSON 400 on a bad value, like `sort`).
    lane: Option<String>,
    /// Normalized merchant, e.g. a recurring merchant's key.
    merchant: Option<String>,
    /// true = only rows with no category.
    uncategorized: Option<bool>,
    /// "out" | "in": the amount's sign.
    direction: Option<String>,
    min_cents: Option<i64>,
    max_cents: Option<i64>,
    sort: Option<String>,
    dir: Option<String>, // "asc" | "desc" (default desc)
    offset: Option<usize>,
    limit: Option<usize>,
}

impl TxnQueryParams {
    fn to_filter(&self) -> Result<TxnFilter, ApiError> {
        FilterParams {
            period: self.period.clone(),
            since: self.since.clone(),
            until: self.until.clone(),
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
    let lane = match p.lane.as_deref() {
        None => None,
        Some(s) => Some(
            outflow_core::Lane::from_str(s)
                .ok_or_else(|| ApiError::bad_request(format!("bad lane {s:?}")))?,
        ),
    };
    let direction = match p.direction.as_deref() {
        None => None,
        Some(s) => Some(
            outflow_core::Direction::from_str(s)
                .ok_or_else(|| ApiError::bad_request(format!("bad direction {s:?}")))?,
        ),
    };
    let mut filter = p.to_filter()?;
    // Filtering BY a suppressed flag only makes sense with those rows visible.
    if p.flag.is_some_and(TxnFlag::is_moved) {
        filter.include_non_spending = true;
    }
    let q = outflow_core::TxnQuery {
        filter,
        text: p.q.filter(|s| !s.trim().is_empty()),
        account_id: p.account,
        category: p.category,
        source: p.source,
        flag: p.flag,
        lane,
        merchant: p.merchant.filter(|s| !s.trim().is_empty()),
        uncategorized: p.uncategorized.unwrap_or(false),
        direction,
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
    let f = params.to_filter()?;
    with_store(&state, move |s| {
        spend_by_category(s, &f).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

// Inlined filter fields, not flatten — see the note on TxnQueryParams.
#[derive(Deserialize)]
struct MerchantParams {
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
    pending: Option<bool>,
    transfers: Option<bool>,
    limit: Option<usize>,
}

async fn merchants(
    State(state): State<AppState>,
    Query(params): Query<MerchantParams>,
) -> ApiResult<Vec<MerchantSpend>> {
    let f = FilterParams {
        period: params.period,
        since: params.since,
        until: params.until,
        pending: params.pending,
        transfers: params.transfers,
    }
    .to_filter()?;
    let limit = params.limit.unwrap_or(15);
    with_store(&state, move |s| {
        top_merchants(s, &f, limit).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

/// Flow lanes per month: In / Out (refunds netted) / Moved by group, plus
/// earned − spent and saved. Date range + pending from the query.
async fn flow(
    State(state): State<AppState>,
    Query(params): Query<FilterParams>,
) -> ApiResult<Vec<FlowMonth>> {
    let f = params.to_filter()?;
    with_store(&state, move |s| {
        flow_lanes(s, &f).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct RecurringParams {
    period: Option<String>,
    since: Option<String>,
    until: Option<String>,
}

/// The recurring list over the 12 months ending with the period's last month
/// (default: this month). `period=2026-10` is the usual spelling.
async fn recurring(
    State(state): State<AppState>,
    Query(p): Query<RecurringParams>,
) -> ApiResult<RecurringView> {
    let today = today_local();
    let through = Period::resolve(
        blank(p.period.as_deref()),
        blank(p.since.as_deref()),
        blank(p.until.as_deref()),
        today,
    )
    .map_err(ApiError::bad_request)?
    .map_or(today, |period| period.last_day());
    let now = now_secs();
    with_store(&state, move |s| {
        outflow_core::recurring(s, through, now).map_err(|e| e.to_string())
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct IgnoredBody {
    /// Normalized merchant, as `RecurringMerchant::merchant`.
    merchant: String,
    ignored: bool,
}

/// Set or clear the Ignored mark on a recurring merchant.
async fn set_recurring_ignored(
    State(state): State<AppState>,
    Json(body): Json<IgnoredBody>,
) -> ApiResult<()> {
    if body.merchant.trim().is_empty() {
        return Err(ApiError::bad_request("merchant is empty"));
    }
    with_store(&state, move |s| {
        s.set_merchant_ignored(body.merchant.trim(), body.ignored)
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
    /// Serde variant name: "Spending" | "Transfer" | "CardPayment" | "Invest"
    /// | "Save" | "LoanPaydown".
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

/// Manual "pull now": runs the same Plaid sync as the background interval
/// (`sync::sync_all`, every linked item).
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
        .route("/holdings", get(holdings))
        .route("/holdings/history", get(holdings_history))
        .route("/activity", get(investment_activity))
        .route("/transactions", get(transactions))
        .route("/spend/categories", get(spend_categories))
        .route("/merchants", get(merchants))
        .route("/flow", get(flow))
        .route("/recurring", get(recurring))
        .route("/recurring/ignored", post(set_recurring_ignored))
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
        .route("/pull", post(pull))
        .route("/reset_data", post(reset_data))
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use outflow_core::Transaction;
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
            pfc_primary: None,
            pfc_detailed: None,
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
    async fn activity_lists_and_filters_investment_txns() {
        use outflow_core::{AccountKind, InvestmentTxn, Security};
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_accounts(&[Account {
                id: "brk".into(),
                org: "Broker".into(),
                name: "Brokerage (0000)".into(),
                kind: AccountKind::Investment,
                balance: Money::from_cents(0),
                currency: "USD".into(),
                last_synced: 0,
                source: "plaid".into(),
                nickname: None,
                hidden: false,
            }])
            .unwrap();
        let sec = Security {
            security_id: "vti".into(),
            ticker: Some("VTI".into()),
            name: "Total Market".into(),
            sec_type: "etf".into(),
            is_cash: false,
            cusip: None,
            isin: None,
            close_price: None,
            close_price_as_of: None,
        };
        let t = |id: &str, date: &str, ty: &str, sub: &str, cents: i64| InvestmentTxn {
            id: id.into(),
            account_id: "brk".into(),
            security_id: Some("vti".into()),
            date: date.into(),
            name: sub.to_uppercase(),
            txn_type: ty.into(),
            subtype: sub.into(),
            quantity: "1".into(),
            price: "300".into(),
            amount: Money::from_cents(cents),
            fees: None,
            currency: Some("USD".into()),
            cancel_txn_id: None,
        };
        store
            .upsert_investment_txns(
                &[sec],
                &[
                    t("i1", "2026-09-01", "buy", "buy", -300_00),
                    t("i2", "2026-10-01", "cash", "dividend", 4_20),
                ],
                1,
            )
            .unwrap();
        let app = app_with_store(store);

        let (status, v) = get_json(app.clone(), "/activity").await;
        assert_eq!(status, StatusCode::OK);
        let rows = v.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["id"], "i2"); // newest first
        assert_eq!(rows[0]["type"], "cash");
        assert_eq!(rows[0]["amount"], 4_20);
        assert_eq!(rows[1]["label"], "VTI");
        let (_, v) = get_json(app.clone(), "/activity?type=dividend").await;
        assert_eq!(v.as_array().unwrap().len(), 1);
        let (_, v) = get_json(app, "/activity?since=2026-09-02&until=").await;
        assert_eq!(v.as_array().unwrap().len(), 1);
    }

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
            acct("hsa", AccountKind::Investment, 3_210_00),
        ];
        store.upsert_accounts(&accounts).unwrap();
        let secs = [
            Security {
                security_id: "vmfxx".into(),
                ticker: Some("VMFXX".into()),
                name: "Money Market".into(),
                sec_type: "mutual fund".into(),
                is_cash: true,
                cusip: None,
                isin: None,
                close_price: None,
                close_price_as_of: None,
            },
            Security {
                security_id: "vti".into(),
                ticker: Some("VTI".into()),
                name: "Total Market".into(),
                sec_type: "etf".into(),
                is_cash: false,
                cusip: Some("922908769".into()),
                isin: None,
                close_price: Some("301.25".into()),
                close_price_as_of: Some("2026-10-21".into()),
            },
        ];
        let pos = |sec: &str, cents: i64| Holding {
            account_id: "brk".into(),
            security_id: sec.into(),
            quantity: "1".into(),
            price: "1".into(),
            value: Money::from_cents(cents),
            cost_basis: None,
            price_as_of: None,
            currency: None,
        };
        let vti = Holding {
            quantity: "33".into(),
            price: "300".into(),
            price_as_of: Some("2026-10-21".into()),
            cost_basis: Some(Money::from_cents(6_600_00)),
            ..pos("vti", 9_900_00)
        };
        let holds = [pos("vmfxx", 100_00), vti];
        store
            .record_item_snapshot(&accounts, Some((&secs, &holds)), 1_000)
            .unwrap();
        store
            .record_item_snapshot(&accounts, None, 1_000 + 2 * 86_400)
            .unwrap();
        let app = app_with_store(store);

        let (status, v) = get_json(app.clone(), "/balances").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["totals"]["net_worth"], 14_910_00);
        // The breakdown: brokerage/HSA cash sits under invested, as uninvested.
        assert_eq!(v["totals"]["cash"], 2_000_00);
        assert_eq!(v["totals"]["invested"], 13_210_00);
        assert_eq!(v["totals"]["uninvested"], 100_00 + 3_210_00);
        assert_eq!(v["totals"]["credit"], -300_00);
        assert_eq!(v["totals"]["loans"], 0);
        assert_eq!(v["totals"]["other"], 0);
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

        let (status, h) = get_json(app.clone(), "/balances/history").await;
        assert_eq!(status, StatusCode::OK);
        let days = h.as_array().unwrap();
        assert_eq!(days.len(), 3); // two sync days + the carried-forward gap
        assert_eq!(days[2]["net_worth"], 14_910_00);

        // /holdings: flat, largest first, with price dates and unit cost.
        let (status, h) = get_json(app.clone(), "/holdings").await;
        assert_eq!(status, StatusCode::OK);
        let rows = h.as_array().unwrap();
        assert_eq!(rows.len(), 3); // VTI, VMFXX, and the HSA's uninvested cash
        let vti = &rows[0];
        assert_eq!(vti["label"], "VTI");
        assert_eq!(vti["account_id"], "brk");
        assert_eq!(vti["quantity"], "33");
        assert_eq!(vti["price_as_of"], "2026-10-21");
        assert_eq!(vti["close_price"], "301.25");
        assert_eq!(vti["cusip"], "922908769");
        assert_eq!(vti["cost_basis"], 6_600_00);
        assert_eq!(vti["unit_cost"], "200.0000");
        assert_eq!(vti["gain"], 3_300_00);
        assert_eq!(vti["gain_bp"], 5_000);
        assert_eq!(vti["synced_at"], 1_000);
        let (_, h) = get_json(app.clone(), "/holdings?ticker=vmfxx").await;
        assert_eq!(h.as_array().unwrap().len(), 1);
        // An all-cash account still shows up, as one synthetic cash row.
        let (_, h) = get_json(app.clone(), "/holdings?account=hsa").await;
        let hsa = h.as_array().unwrap();
        assert_eq!(hsa.len(), 1);
        assert_eq!(hsa[0]["security_id"], "cash:uninvested");
        assert_eq!(hsa[0]["is_cash"], true);
        assert_eq!(hsa[0]["value"], 3_210_00);

        // /holdings/history: one point per position per holdings sync.
        let (status, h) = get_json(app, "/holdings/history?ticker=VTI").await;
        assert_eq!(status, StatusCode::OK);
        let pts = h.as_array().unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0]["price"], "300");
        assert_eq!(pts[0]["value"], 9_900_00);
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

    // Lane drill-down: the lane reads moved rows without transfers=true,
    // direction picks a side, uncategorized picks rows with no category.
    #[tokio::test]
    async fn transactions_filter_by_lane_direction_and_uncategorized() {
        use outflow_core::model::{Account, AccountKind};
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_accounts(&[Account {
                id: "acct-1".into(),
                org: "Example Bank".into(),
                name: "Checking".into(),
                kind: AccountKind::Checking,
                balance: Money::from_cents(0),
                currency: "USD".into(),
                last_synced: 0,
                source: "plaid".into(),
                nickname: None,
                hidden: false,
            }])
            .unwrap();
        let mut out = txn("sav-out", -5000, "To Savings");
        out.flag = TxnFlag::Save;
        let mut back = txn("sav-in", 2000, "From Savings");
        back.flag = TxnFlag::Save;
        let mut pay = txn("pay", 90000, "Payroll");
        pay.category = None;
        pay.pfc_primary = Some("INCOME".into());
        store
            .upsert_transactions(&[txn("cafe", -1500, "Blue Bottle"), out, back, pay])
            .unwrap();
        let app = app_with_store(store);
        let ids = txn_ids(app.clone(), "/transactions?lane=save").await;
        assert_eq!(ids, ["sav-in", "sav-out"]);
        let ids = txn_ids(app.clone(), "/transactions?lane=save&direction=out").await;
        assert_eq!(ids, ["sav-out"]);
        let ids = txn_ids(app.clone(), "/transactions?lane=in&uncategorized=true").await;
        assert_eq!(ids, ["pay"]);
        let (status, body) = get_json(app, "/transactions?lane=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body["error"].as_str().unwrap().contains("bogus"));
    }

    /// /recurring lists a monthly merchant as of a chosen month, Ignore moves
    /// it to `ignored`, and merchant= on /transactions shows its rows.
    #[tokio::test]
    async fn recurring_lists_ignores_and_drills_into_a_merchant() {
        use chrono::{Local, TimeZone};
        let at = |m: u32| {
            Local
                .with_ymd_and_hms(2026, m, 10, 12, 0, 0)
                .single()
                .unwrap()
                .timestamp()
        };
        let mut txns: Vec<Transaction> = (8..=10)
            .map(|m| {
                let mut t = txn(&format!("sf{m}"), -1599, "STREAMFLIX.COM 1234");
                t.posted = at(m);
                t.transacted_at = Some(at(m));
                t
            })
            .collect();
        txns.push(txn("cafe", -1500, "Blue Bottle"));
        let store = Store::open_in_memory().unwrap();
        store.upsert_transactions(&txns).unwrap();
        let app = app_with_store(store);

        let (status, body) = get_json(app.clone(), "/recurring?period=2026-10").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["month"], "2026-10");
        assert_eq!(body["since"], "2025-11-01");
        assert_eq!(body["until"], "2026-10-31");
        assert_eq!(body["items"][0]["merchant"], "streamflix com");
        assert_eq!(body["items"][0]["monthly_cents"], 1599);

        let q = "/transactions?merchant=streamflix%20com&since=2025-11-01&until=2026-10-31";
        assert_eq!(txn_ids(app.clone(), q).await, ["sf10", "sf8", "sf9"]);

        let ignore = serde_json::json!({ "merchant": "streamflix com", "ignored": true });
        assert_eq!(
            post_status(app.clone(), "/recurring/ignored", ignore).await,
            StatusCode::OK
        );
        let (_, body) = get_json(app.clone(), "/recurring?period=2026-10").await;
        assert_eq!(body["items"].as_array().unwrap().len(), 0);
        assert_eq!(body["ignored"][0]["merchant"], "streamflix com");

        let (status, _) = get_json(app, "/recurring?period=bogus").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
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

    /// Local noon on `day` (YYYY-MM-DD), as epoch seconds.
    fn noon(day: &str) -> i64 {
        use chrono::TimeZone;
        let d = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
        chrono::Local
            .from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
            .single()
            .unwrap()
            .timestamp()
    }

    fn txn_on(id: &str, day: &str) -> Transaction {
        let mut t = txn(id, -1000, "Corner Cafe");
        t.posted = noon(day);
        t.transacted_at = Some(t.posted);
        t
    }

    async fn txn_ids(app: Router, uri: &str) -> Vec<String> {
        let (status, body) = get_json(app, uri).await;
        assert_eq!(status, StatusCode::OK, "{uri}: {body}");
        let mut ids: Vec<String> = body["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["id"].as_str().unwrap().to_owned())
            .collect();
        ids.sort();
        ids
    }

    fn period_app() -> Router {
        app_with_txns(&[
            txn_on("sep30", "2026-09-30"),
            txn_on("oct01", "2026-10-01"),
            txn_on("oct15", "2026-10-15"),
            txn_on("oct31", "2026-10-31"),
            txn_on("nov01", "2026-11-01"),
        ])
    }

    #[tokio::test]
    async fn period_picks_one_month() {
        let ids = txn_ids(period_app(), "/transactions?period=2026-10").await;
        assert_eq!(ids, ["oct01", "oct15", "oct31"]);
        // Same resolver on the aggregates.
        let (status, v) = get_json(period_app(), "/spend/categories?period=2026-10").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v[0]["total_cents"].as_i64().unwrap().abs(), 3000);
        let (status, _) = get_json(period_app(), "/merchants?period=2026-Q4").await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = get_json(period_app(), "/flow?period=last-month").await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn since_until_dates_include_the_until_day() {
        let ids = txn_ids(
            period_app(),
            "/transactions?since=2026-09-30&until=2026-10-15",
        )
        .await;
        assert_eq!(ids, ["oct01", "oct15", "sep30"]);
    }

    #[tokio::test]
    async fn legacy_epoch_since_until_unchanged() {
        // until is exclusive in epoch mode: a row exactly at `until` drops.
        let uri = format!(
            "/transactions?since={}&until={}",
            noon("2026-10-01"),
            noon("2026-10-31")
        );
        let ids = txn_ids(period_app(), &uri).await;
        assert_eq!(ids, ["oct01", "oct15"]);
        // since alone stays open-ended.
        let uri = format!("/transactions?since={}", noon("2026-10-31"));
        assert_eq!(txn_ids(period_app(), &uri).await, ["nov01", "oct31"]);
    }

    #[tokio::test]
    async fn bad_range_params_are_400s() {
        let cases = [
            ("/transactions?period=2026-10&since=2026-10-01", "not both"),
            ("/transactions?period=2026-10&since=1790000000", "not both"),
            ("/transactions?period=Octember", "bad period"),
            ("/flow?since=1790000000&until=2026-10-01", "epoch seconds"),
            (
                "/merchants?since=2026-10-01&until=1790000000",
                "epoch seconds",
            ),
            (
                "/spend/categories?since=2026-10-31&until=2026-10-01",
                "after",
            ),
            ("/activity?period=2026-13", "bad period"),
            ("/balances/history?since=10/01/2026", "bad date"),
        ];
        for (uri, needle) in cases {
            let (status, body) = get_json(period_app(), uri).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
            let msg = body["error"].as_str().unwrap();
            assert!(msg.contains(needle), "{uri}: {msg}");
        }
    }

    #[tokio::test]
    async fn balance_and_holding_history_honor_until() {
        use outflow_core::{AccountKind, Holding, Security};
        let store = Store::open_in_memory().unwrap();
        let accounts = [Account {
            id: "brk".into(),
            org: "Broker".into(),
            name: "Brokerage (0000)".into(),
            kind: AccountKind::Investment,
            balance: Money::from_cents(1_000_00),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
        }];
        store.upsert_accounts(&accounts).unwrap();
        let secs = [Security {
            security_id: "vti".into(),
            ticker: Some("VTI".into()),
            name: "Total Market".into(),
            sec_type: "etf".into(),
            is_cash: false,
            cusip: None,
            isin: None,
            close_price: None,
            close_price_as_of: None,
        }];
        let holds = [Holding {
            account_id: "brk".into(),
            security_id: "vti".into(),
            quantity: "1".into(),
            price: "1000".into(),
            value: Money::from_cents(1_000_00),
            cost_basis: None,
            price_as_of: None,
            currency: None,
        }];
        for day in ["2026-10-01", "2026-10-05"] {
            store
                .record_item_snapshot(&accounts, Some((&secs, &holds)), noon(day))
                .unwrap();
        }
        let app = app_with_store(store);

        let (_, all) = get_json(app.clone(), "/balances/history").await;
        assert!(all.as_array().unwrap().len() > 1);
        let (status, h) = get_json(app.clone(), "/balances/history?until=2026-10-02").await;
        assert_eq!(status, StatusCode::OK, "{h}");
        assert_eq!(h.as_array().unwrap().len(), 1);
        let (_, h) = get_json(app.clone(), "/balances/history?period=2026-W41").await;
        assert_eq!(h.as_array().unwrap().len(), 1); // Oct 5-11: the later sync only

        let (_, h) = get_json(app.clone(), "/holdings/history").await;
        assert_eq!(h.as_array().unwrap().len(), 2);
        let uri = format!("/holdings/history?until={}", noon("2026-10-05"));
        let (status, h) = get_json(app, &uri).await;
        assert_eq!(status, StatusCode::OK, "{h}");
        assert_eq!(h.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn activity_takes_a_period() {
        use outflow_core::{AccountKind, InvestmentTxn};
        let store = Store::open_in_memory().unwrap();
        store
            .upsert_accounts(&[Account {
                id: "brk".into(),
                org: "Broker".into(),
                name: "Brokerage (0000)".into(),
                kind: AccountKind::Investment,
                balance: Money::from_cents(0),
                currency: "USD".into(),
                last_synced: 0,
                source: "plaid".into(),
                nickname: None,
                hidden: false,
            }])
            .unwrap();
        let t = |id: &str, date: &str| InvestmentTxn {
            id: id.into(),
            account_id: "brk".into(),
            security_id: None,
            date: date.into(),
            name: "CONTRIBUTION".into(),
            txn_type: "cash".into(),
            subtype: "contribution".into(),
            quantity: "0".into(),
            price: "0".into(),
            amount: Money::from_cents(100_00),
            fees: None,
            currency: Some("USD".into()),
            cancel_txn_id: None,
        };
        store
            .upsert_investment_txns(
                &[],
                &[
                    t("a", "2026-09-30"),
                    t("b", "2026-10-01"),
                    t("c", "2026-10-31"),
                    t("d", "2026-11-01"),
                ],
                1,
            )
            .unwrap();
        let app = app_with_store(store);
        let ids = |v: serde_json::Value| -> Vec<String> {
            let mut ids: Vec<String> = v
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["id"].as_str().unwrap().to_owned())
                .collect();
            ids.sort();
            ids
        };
        let (status, v) = get_json(app.clone(), "/activity?period=2026-10").await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(ids(v), ["b", "c"]);
        let (_, v) = get_json(app, "/activity?since=2026-09-30&until=2026-10-01").await;
        assert_eq!(ids(v), ["a", "b"]);
    }
}
