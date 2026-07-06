// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! outflow desktop backend. Thin Tauri command layer over `outflow-core`, so the
//! GUI and CLI share identical domain logic. This pass is dashboards-first: no
//! network — `pull` here loads a SimpleFIN JSON file, categorize runs the rule
//! pass only. Live SimpleFIN + LLM land in the deferred `outflow-net` pass.

use std::sync::Mutex;

use outflow_core::{
    detect, monthly_flow, parse_account_set, spend_by_category, top_merchants, Account,
    CategorySource, CategorySpend, LedgerView, Mark, MerchantSpend, MonthlyFlow, Store, Subscription,
    Transaction, TxnFilter, TxnFlag,
};
use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

/// Shared DB handle. `rusqlite::Connection` is not `Sync`, so serialize all
/// access behind a mutex — fine for a single-user desktop app.
struct AppState {
    store: Mutex<Store>,
}

/// `Result` alias: commands surface errors as plain strings to the frontend.
type CmdResult<T> = Result<T, String>;

fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Rolling-window control → a `since` bound (epoch secs). ~30.5-day months is a
/// coarse filter; the ledger's month/rate math is exact via `chrono::Local`.
const MONTH_SECS: i64 = 2_635_200;
fn window_since(window: &str, now: i64) -> Option<i64> {
    match window {
        "all" => None,
        "3mo" => Some(now - 3 * MONTH_SECS),
        "12mo" => Some(now - 12 * MONTH_SECS),
        _ => Some(now - 6 * MONTH_SECS), // default 6mo
    }
}

/// Filter payload from JS. Epoch **seconds**; the frontend computes them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterArg {
    since: Option<i64>,
    until: Option<i64>,
    #[serde(default = "default_true")]
    include_pending: bool,
    /// Off by default: charts suppress transfers/card payments. A "show
    /// transfers" toggle sends true.
    #[serde(default)]
    include_non_spending: bool,
}

fn default_true() -> bool {
    true
}

impl From<FilterArg> for TxnFilter {
    fn from(f: FilterArg) -> Self {
        TxnFilter {
            since: f.since,
            until: f.until,
            include_pending: f.include_pending,
            include_non_spending: f.include_non_spending,
        }
    }
}

/// A missing filter means "everything" (include pending). Note this is NOT
/// `FilterArg::default()`: a derived Default would set `include_pending=false`
/// and silently drop pending rows.
fn to_filter(filter: Option<FilterArg>) -> TxnFilter {
    filter.map(Into::into).unwrap_or_else(TxnFilter::all)
}

/// Filter for the transaction LIST. Mirrors the query-layer date/pending
/// semantics (filtering on the behavioral date), but deliberately does NOT drop
/// non-Spending rows — the list is where the user sees and reclassifies transfers
/// and card payments, so they must remain visible even though the charts hide
/// them. (`include_non_spending` therefore doesn't apply here.)
fn passes(t: &Transaction, f: &TxnFilter) -> bool {
    if !f.include_pending && t.pending {
        return false;
    }
    let d = t.effective_date();
    if let Some(s) = f.since {
        if d < s {
            return false;
        }
    }
    if let Some(u) = f.until {
        if d >= u {
            return false;
        }
    }
    true
}

#[derive(Serialize)]
struct CategorizeResult {
    rule: usize,
    remaining: usize,
}

#[derive(Serialize)]
struct PullResult {
    added: usize,
    updated: usize,
    accounts: usize,
    warnings: Vec<String>,
}

#[tauri::command]
fn accounts(state: State<AppState>) -> CmdResult<Vec<Account>> {
    let store = state.store.lock().map_err(e)?;
    store.accounts().map_err(e)
}

#[tauri::command]
fn transactions(state: State<AppState>, filter: Option<FilterArg>) -> CmdResult<Vec<Transaction>> {
    let store = state.store.lock().map_err(e)?;
    let f = to_filter(filter);
    let all = store.all_transactions().map_err(e)?;
    // Newest first for the list view.
    let mut out: Vec<Transaction> = all.into_iter().filter(|t| passes(t, &f)).collect();
    out.reverse();
    Ok(out)
}

#[tauri::command]
fn categorize(state: State<AppState>) -> CmdResult<CategorizeResult> {
    let store = state.store.lock().map_err(e)?;
    let rules = store.ruleset().map_err(e)?;
    let rule = store
        .categorize_uncategorized(&rules, CategorySource::Rule)
        .map_err(e)?;
    let remaining = store.uncategorized().map_err(e)?.len();
    Ok(CategorizeResult { rule, remaining })
}

#[tauri::command]
fn spend_categories(
    state: State<AppState>,
    filter: Option<FilterArg>,
) -> CmdResult<Vec<CategorySpend>> {
    let store = state.store.lock().map_err(e)?;
    let f = to_filter(filter);
    spend_by_category(&store, &f).map_err(e)
}

#[tauri::command]
fn merchants(
    state: State<AppState>,
    filter: Option<FilterArg>,
    limit: Option<usize>,
) -> CmdResult<Vec<MerchantSpend>> {
    let store = state.store.lock().map_err(e)?;
    let f = to_filter(filter);
    top_merchants(&store, &f, limit.unwrap_or(15)).map_err(e)
}

#[tauri::command]
fn flow(state: State<AppState>, filter: Option<FilterArg>) -> CmdResult<Vec<MonthlyFlow>> {
    let store = state.store.lock().map_err(e)?;
    let f = to_filter(filter);
    monthly_flow(&store, &f).map_err(e)
}

#[tauri::command]
fn subscriptions(state: State<AppState>) -> CmdResult<Vec<Subscription>> {
    let store = state.store.lock().map_err(e)?;
    let txns = store.all_transactions().map_err(e)?;
    Ok(detect(&txns))
}

/// The whole rhythm-ledger view for a rolling window (`3mo`/`6mo`/`12mo`/`all`).
/// Streams + zones + stats, partitioned once. The screen's primary query.
#[tauri::command]
fn ledger(state: State<AppState>, window: String) -> CmdResult<LedgerView> {
    let store = state.store.lock().map_err(e)?;
    let now = now_secs();
    outflow_core::ledger(&store, window_since(&window, now), now).map_err(e)
}

/// The occurrences behind one stream (its normalized merchant), for the slide-over.
#[tauri::command]
fn stream_occurrences(
    state: State<AppState>,
    merchant: String,
    window: String,
) -> CmdResult<Vec<Transaction>> {
    let store = state.store.lock().map_err(e)?;
    let since = window_since(&window, now_secs());
    store.stream_occurrences(&merchant, since).map_err(e)
}

/// Apply a per-merchant ledger override (`Committed` / `Dismissed`) from the
/// slide-over's subtractive reclassify.
#[tauri::command]
fn mark_stream(state: State<AppState>, merchant: String, mark: Mark) -> CmdResult<()> {
    let store = state.store.lock().map_err(e)?;
    store.set_merchant_mark(&merchant, mark).map_err(e)
}

/// Remove a merchant's override, returning it to the normal streams list.
#[tauri::command]
fn clear_stream_mark(state: State<AppState>, merchant: String) -> CmdResult<()> {
    let store = state.store.lock().map_err(e)?;
    store.clear_merchant_mark(&merchant).map_err(e)
}

/// Set a transaction's suppression flag (`Spending`/`Transfer`/`CardPayment`);
/// when `learn`, write a rule so siblings pick it up on the next `apply_flags`.
/// The frontend sends the serde variant name (e.g. "Transfer").
#[tauri::command]
fn set_flag(
    state: State<AppState>,
    txn_id: String,
    flag: TxnFlag,
    learn: bool,
) -> CmdResult<Option<i64>> {
    let store = state.store.lock().map_err(e)?;
    store.set_flag(&txn_id, flag, learn).map_err(e)
}

/// Re-apply all flag rules across the transactions. Returns the count changed.
#[tauri::command]
fn apply_flags(state: State<AppState>) -> CmdResult<usize> {
    let store = state.store.lock().map_err(e)?;
    store.apply_flags().map_err(e)
}

/// Whether any ingested account is a credit/card account. The frontend warns
/// before suppressing a CardPayment when this is false (no offsetting charges).
#[tauri::command]
fn has_credit_account(state: State<AppState>) -> CmdResult<bool> {
    let store = state.store.lock().map_err(e)?;
    store.has_credit_account().map_err(e)
}

#[tauri::command]
fn categories(state: State<AppState>) -> CmdResult<Vec<String>> {
    let store = state.store.lock().map_err(e)?;
    store.categories().map_err(e)
}

#[tauri::command]
fn set_category(
    state: State<AppState>,
    txn_id: String,
    category: String,
    learn: bool,
) -> CmdResult<Option<i64>> {
    let store = state.store.lock().map_err(e)?;
    store.set_manual_category(&txn_id, &category, learn).map_err(e)
}

/// Clear pulled data (transactions + accounts + sync log) for a clean re-pull,
/// keeping learned category rules. The safety valve for a contaminated DB.
#[tauri::command]
fn reset_data(state: State<AppState>) -> CmdResult<()> {
    let store = state.store.lock().map_err(e)?;
    store.reset_data().map_err(e)
}

/// Dev/offline pull: load a SimpleFIN JSON file (the `--from-file` path). Live
/// SimpleFIN over the network is the deferred `net` pass.
#[tauri::command]
fn pull_from_file(state: State<AppState>, path: String) -> CmdResult<PullResult> {
    let json = std::fs::read_to_string(&path).map_err(|err| format!("read {path}: {err}"))?;
    let fetched = parse_account_set(&json).map_err(|err| format!("parse: {err:?}"))?;
    let store = state.store.lock().map_err(e)?;
    store.upsert_accounts(&fetched.accounts).map_err(e)?;
    let r = store.upsert_transactions(&fetched.transactions).map_err(e)?;
    Ok(PullResult {
        added: r.added,
        updated: r.updated,
        accounts: fetched.accounts.len(),
        warnings: fetched.warnings,
    })
}

/// Live SimpleFIN pull: resolve the stored access URL, fetch, parse, upsert.
/// Needs the `net` feature and a prior `claim` (or `OUTFLOW_SFIN_URL`).
#[tauri::command]
fn pull_live(state: State<AppState>) -> CmdResult<PullResult> {
    #[cfg(feature = "net")]
    {
        let json = outflow_net::access_url().and_then(|u| outflow_net::fetch(&u))?;
        let fetched = parse_account_set(&json).map_err(|err| format!("parse: {err:?}"))?;
        let store = state.store.lock().map_err(e)?;
        store.upsert_accounts(&fetched.accounts).map_err(e)?;
        let r = store.upsert_transactions(&fetched.transactions).map_err(e)?;
        Ok(PullResult {
            added: r.added,
            updated: r.updated,
            accounts: fetched.accounts.len(),
            warnings: fetched.warnings,
        })
    }
    #[cfg(not(feature = "net"))]
    {
        let _ = state;
        Err("this build has no network support (rebuild with --features net)".into())
    }
}

/// Exchange a SimpleFIN setup token for a durable access URL and persist it
/// (keychain by default, or a 0600 file when `OUTFLOW_SFIN_URL_FILE` is set).
#[tauri::command]
fn claim(setup_token: String) -> CmdResult<String> {
    #[cfg(feature = "net")]
    {
        let url = outflow_net::claim_access_url(setup_token.trim())?;
        let placed = match outflow_net::persist_access_url(&url)? {
            outflow_net::Persisted::File(p) => format!("saved to {p}"),
            outflow_net::Persisted::Keychain => "saved to keychain".into(),
            outflow_net::Persisted::Ephemeral => {
                "obtained but not persisted (set OUTFLOW_SFIN_URL_FILE)".into()
            }
        };
        Ok(format!("Connected — access URL {placed}. Now hit Pull."))
    }
    #[cfg(not(feature = "net"))]
    {
        let _ = setup_token;
        Err("this build has no network support (rebuild with --features net)".into())
    }
}

/// LLM pass over the merchants the rule pass couldn't match. Needs the `net`
/// feature and `ANTHROPIC_API_KEY`. Returns the count newly categorized.
#[tauri::command]
fn categorize_llm(state: State<AppState>) -> CmdResult<usize> {
    #[cfg(feature = "net")]
    {
        use outflow_core::{normalize_payee, LlmCategorizer, MerchantSample};
        use outflow_net::AnthropicPrompter;
        use std::collections::HashMap;

        let store = state.store.lock().map_err(e)?;
        let uncategorized = store.uncategorized().map_err(e)?;
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
        let categories = store.categories().map_err(e)?;
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
                store.set_category(&t.id, cat, CategorySource::Llm).map_err(e)?;
                applied += 1;
            }
        }
        Ok(applied)
    }
    #[cfg(not(feature = "net"))]
    {
        let _ = state;
        Err("this build has no network support (rebuild with --features net)".into())
    }
}

fn open_store(app: &tauri::App) -> Result<Store, String> {
    let db = resolve_db_path(app)?;
    let store = open_db(&db)?;
    // Surface which DB we opened and how full it is — a fresh/empty file is the
    // usual reason "no data shows". Visible in the `tauri dev` terminal.
    let count = store.count_transactions().unwrap_or(-1);
    eprintln!("outflow: opened db {db} ({count} transactions)");
    Ok(store)
}

/// DB path: the `OUTFLOW_DB` override (dev/CLI), else the platform app-data dir.
/// A Finder-launched `.app` inherits no shell env, so the app-data dir is the
/// only path it can rely on. Created on first run.
fn resolve_db_path(app: &tauri::App) -> Result<String, String> {
    if let Ok(p) = std::env::var("OUTFLOW_DB") {
        if !p.is_empty() {
            return Ok(p);
        }
    }
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    Ok(dir.join("outflow.db").to_string_lossy().into_owned())
}

fn open_db(db: &str) -> Result<Store, String> {
    #[cfg(feature = "encryption")]
    {
        let key = resolve_db_key()?;
        Store::open_encrypted(db, &key).map_err(|err| format!("open encrypted db {db}: {err}"))
    }
    #[cfg(not(feature = "encryption"))]
    {
        Store::open(db).map_err(|err| format!("open db {db}: {err}"))
    }
}

/// SQLCipher key: the `OUTFLOW_DB_KEY` override (dev/CLI), else a keychain-managed
/// key generated on first run — transparent, passphrase-free encryption for the
/// double-click app. The key is a secret: env or keychain only, never argv/DB.
#[cfg(feature = "encryption")]
fn resolve_db_key() -> Result<String, String> {
    if let Ok(k) = std::env::var("OUTFLOW_DB_KEY") {
        if !k.is_empty() {
            return Ok(k);
        }
    }
    #[cfg(feature = "keychain")]
    {
        outflow_net::secrets::db_key_get_or_create()
    }
    #[cfg(not(feature = "keychain"))]
    {
        Err("encryption needs a key: set OUTFLOW_DB_KEY, or enable the keychain feature".into())
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let store = open_store(app)?;
            app.manage(AppState {
                store: Mutex::new(store),
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts,
            transactions,
            categorize,
            spend_categories,
            merchants,
            flow,
            subscriptions,
            ledger,
            stream_occurrences,
            mark_stream,
            clear_stream_mark,
            categories,
            set_category,
            set_flag,
            apply_flags,
            has_credit_account,
            pull_from_file,
            pull_live,
            claim,
            categorize_llm,
            reset_data,
        ])
        .run(tauri::generate_context!())
        .expect("error while running outflow");
}
