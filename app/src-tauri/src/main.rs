// Prevents an extra console window on Windows in release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! outflow desktop backend. Thin Tauri command layer over `outflow-core`, so the
//! GUI and CLI share identical domain logic. This pass is dashboards-first: no
//! network — `pull` here loads a SimpleFIN JSON file, categorize runs the rule
//! pass only. Live SimpleFIN + LLM land in the deferred `outflow-net` pass.

use std::sync::Mutex;

use outflow_core::{
    detect, monthly_flow, parse_account_set, spend_by_category, top_merchants, Account,
    CategorySource, CategorySpend, MerchantSpend, MonthlyFlow, Store, Subscription, Transaction,
    TxnFilter,
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

/// Filter payload from JS. Epoch **seconds**; the frontend computes them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FilterArg {
    since: Option<i64>,
    until: Option<i64>,
    #[serde(default = "default_true")]
    include_pending: bool,
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
        }
    }
}

/// A missing filter means "everything" (include pending). Note this is NOT
/// `FilterArg::default()`: a derived Default would set `include_pending=false`
/// and silently drop pending rows.
fn to_filter(filter: Option<FilterArg>) -> TxnFilter {
    filter.map(Into::into).unwrap_or_else(TxnFilter::all)
}

/// Mirror of the query-layer `passes()`: keep the transaction list consistent
/// with the aggregates, which all apply the same filter semantics.
fn passes(t: &Transaction, f: &TxnFilter) -> bool {
    if !f.include_pending && t.pending {
        return false;
    }
    if let Some(s) = f.since {
        if t.posted < s {
            return false;
        }
    }
    if let Some(u) = f.until {
        if t.posted >= u {
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

fn open_store() -> Result<Store, String> {
    let db = std::env::var("OUTFLOW_DB").unwrap_or_else(|_| "outflow.db".into());
    let store = Store::open(&db).map_err(|err| format!("open db {db}: {err}"))?;
    // Surface which DB we opened and how full it is — a fresh/empty file is the
    // usual reason "no data shows". Visible in the `tauri dev` terminal.
    let count = store.count_transactions().unwrap_or(-1);
    eprintln!("outflow: opened db {db} ({count} transactions)");
    Ok(store)
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let store = open_store()?;
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
            categories,
            set_category,
            pull_from_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running outflow");
}
