//! The sync engine: pulls every configured source into the store. Runs on the
//! blocking pool (ureq + rusqlite are synchronous); the async layer only
//! schedules it. One run at a time — the background interval and the manual
//! `/api/pull` share a try-lock so they can't interleave.
//!
//! Per Plaid item: cursor-loop `/transactions/sync`, accumulate every page
//! into one `PlaidBatch`, apply atomically (data + deletions + cursor in a
//! single SQLite transaction). One item failing never blocks the others —
//! `ITEM_LOGIN_REQUIRED` marks the item for re-link and moves on.

use axum::http::StatusCode;
use outflow_core::store::PlaidBatch;
use outflow_core::{
    item_has_investments_consent, item_is_investments_only, parse_accounts_get, parse_holdings_get,
    parse_investment_txns_get, parse_sync_page, PlaidItem, Store,
};
use outflow_net::{plaid, plaid_tokens, PlaidConfig};
use serde::Serialize;
use std::sync::{Arc, Mutex};

use crate::state::{now_secs, ApiError, AppState, Config};

/// One source's outcome within a sync run.
#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct LegReport {
    /// `"plaid:<institution>"`.
    pub source: String,
    pub added: usize,
    pub updated: usize,
    /// The leg failed; its item's status becomes `login_required` or
    /// `error`. Other legs still ran.
    pub error: Option<String>,
    /// Non-fatal problem (e.g. holdings fetch failed): transactions synced
    /// and the item stays `ok`, but something was left stale.
    /// Omitted from the JSON when there is none.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub warning: Option<String>,
}

/// What one `POST /api/pull` (or background sync) did.
#[derive(Serialize, Debug, Clone)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SyncReport {
    pub legs: Vec<LegReport>,
    /// Flag rules applied + rule categorization run after ingest.
    pub flags_applied: usize,
    pub categorized: usize,
    /// Card-payment pairs auto-accepted (high confidence) / queued for review.
    pub matches_auto: usize,
    pub matches_proposed: usize,
}

/// Run every source once. Errors per leg are captured in the report, not
/// propagated — a dead bank connection must not hide the others' data.
pub async fn sync_all(state: &AppState) -> Result<SyncReport, ApiError> {
    let _guard = state
        .sync_lock
        .try_lock()
        .map_err(|_| ApiError(StatusCode::CONFLICT, "a sync is already running".into()))?;
    let store = state.store.clone();
    let cfg = state.cfg.clone();
    tokio::task::spawn_blocking(move || sync_all_blocking(&store, &cfg))
        .await
        .map_err(|e| ApiError::internal(format!("sync task: {e}")))
}

fn sync_all_blocking(store: &Arc<Mutex<Store>>, cfg: &Config) -> SyncReport {
    let mut legs = Vec::new();

    // Plaid legs — one per linked item.
    let items = lock(store, |s| s.plaid_items().map_err(|e| e.to_string())).unwrap_or_default();
    if !items.is_empty() {
        match PlaidConfig::from_env() {
            Ok(pcfg) => {
                for item in items {
                    legs.push(sync_plaid_item(store, cfg, &pcfg, &item));
                }
            }
            Err(e) => legs.push(LegReport {
                source: "plaid".into(),
                added: 0,
                updated: 0,
                error: Some(e),
                warning: None,
            }),
        }
    }

    // Post-ingest passes: learned flag rules, rule categorization over
    // whatever is still uncategorized, then card-payment pair detection.
    let flags_applied = lock(store, |s| s.apply_flags().map_err(|e| e.to_string())).unwrap_or(0);
    let categorized = lock(store, |s| {
        let rules = s.ruleset().map_err(|e| e.to_string())?;
        s.categorize_uncategorized(&rules, outflow_core::CategorySource::Rule)
            .map_err(|e| e.to_string())
    })
    .unwrap_or(0);
    let (matches_auto, matches_proposed) = lock(store, detect_and_record_matches).unwrap_or((0, 0));

    SyncReport {
        legs,
        flags_applied,
        categorized,
        matches_auto,
        matches_proposed,
    }
}

/// Run the card-payment detector over the whole archive. High-confidence pairs
/// auto-accept (flag both legs immediately); the rest queue as proposals for
/// the Review screen. Previously decided pairs never re-enter.
fn detect_and_record_matches(s: &Store) -> Result<(usize, usize), String> {
    use outflow_core::{detect_card_payments, MatchConfidence, MatchOptions, MatchStatus};

    // Hidden accounts included: a hidden checking account can still pay a
    // visible card, and that card's payment leg must be suppressed.
    let accounts = s.all_accounts().map_err(|e| e.to_string())?;
    let txns = s
        .all_transactions_including_hidden()
        .map_err(|e| e.to_string())?;
    let decided = s.decided_pairs().map_err(|e| e.to_string())?;
    let proposals = detect_card_payments(&accounts, &txns, &decided, &MatchOptions::default());

    let mut auto = 0usize;
    let mut queued = 0usize;
    for p in proposals {
        match p.confidence {
            MatchConfidence::High => {
                // Recorded as a proposal, then accepted the one way every
                // accept goes, so unaccept can restore it exactly.
                let id = s
                    .insert_match(
                        &p.bank_txn_id,
                        &p.card_txn_id,
                        MatchStatus::Proposed,
                        p.confidence,
                        Some(&p.reason),
                        now_secs(),
                    )
                    .map_err(|e| e.to_string())?;
                if let Some(id) = id {
                    s.accept_match(id).map_err(|e| e.to_string())?;
                    auto += 1;
                }
            }
            MatchConfidence::Medium => {
                let inserted = s
                    .insert_match(
                        &p.bank_txn_id,
                        &p.card_txn_id,
                        MatchStatus::Proposed,
                        p.confidence,
                        Some(&p.reason),
                        now_secs(),
                    )
                    .map_err(|e| e.to_string())?;
                if inserted.is_some() {
                    queued += 1;
                }
            }
        }
    }
    Ok((auto, queued))
}

/// Lock the store for one operation; network calls happen outside the lock so
/// API reads stay responsive during a sync.
fn lock<T>(
    store: &Arc<Mutex<Store>>,
    f: impl FnOnce(&Store) -> Result<T, String>,
) -> Result<T, String> {
    let guard = store
        .lock()
        .map_err(|e| format!("store lock poisoned: {e}"))?;
    f(&guard)
}

pub(crate) fn sync_plaid_item(
    store: &Arc<Mutex<Store>>,
    cfg: &Config,
    pcfg: &PlaidConfig,
    item: &PlaidItem,
) -> LegReport {
    let source = format!("plaid:{}", item.institution);
    let mut report = LegReport {
        source: source.clone(),
        added: 0,
        updated: 0,
        error: None,
        warning: None,
    };
    let log_id = lock(store, |s| {
        s.log_sync_start(&source, now_secs())
            .map_err(|e| e.to_string())
    })
    .ok();

    match run_plaid_item(store, cfg, pcfg, item) {
        Ok((r, warning)) => {
            report.added = r.added;
            report.updated = r.updated;
            report.warning = warning;
        }
        Err(e) => {
            if e.contains("ITEM_LOGIN_REQUIRED") {
                let _ = lock(store, |s| {
                    s.set_plaid_item_status(&item.item_id, "login_required")
                        .map_err(|e2| e2.to_string())
                });
            } else {
                let _ = lock(store, |s| {
                    s.set_plaid_item_status(&item.item_id, "error")
                        .map_err(|e2| e2.to_string())
                });
            }
            report.error = Some(e);
        }
    }
    if let Some(id) = log_id {
        let _ = lock(store, |s| {
            s.log_sync_finish(
                id,
                now_secs(),
                report.added,
                report.updated,
                report.error.as_deref(),
            )
            .map_err(|e| e.to_string())
        });
    }
    report
}

/// One full item sync: accounts + the complete `/transactions/sync` cursor
/// loop, committed as a single `PlaidBatch`, then the holdings/balance
/// snapshot. On the pagination-mutation error the whole loop restarts from the
/// item's stored cursor (which only advances on commit, so a restart is always
/// safe). The `Option<String>` is a non-fatal warning from the snapshot step.
pub fn run_plaid_item(
    store: &Arc<Mutex<Store>>,
    cfg: &Config,
    pcfg: &PlaidConfig,
    item: &PlaidItem,
) -> Result<(outflow_core::UpsertResult, Option<String>), String> {
    let token = plaid_tokens::token_for(&cfg.plaid_tokens_file, &item.item_id)?;
    let synced_at = now_secs();

    let accounts_json = plaid::accounts_get(pcfg, &token)?;
    let mut accounts = parse_accounts_get(&accounts_json, &item.institution, synced_at)
        .map_err(|e| format!("parse accounts: {e:?}"))?;
    // Which connection each account came through: Settings lists accounts
    // under it.
    for a in &mut accounts {
        a.item_id = Some(item.item_id.clone());
    }
    // Consent is display-only metadata (drives "Allow holdings" in the UI), so
    // a malformed item block is logged rather than failing the sync.
    let consent = match item_has_investments_consent(&accounts_json) {
        Ok(granted) => {
            let _ = lock(store, |s| {
                s.set_plaid_item_investments_consent(&item.item_id, granted)
                    .map_err(|e| e.to_string())
            });
            granted
        }
        Err(e) => {
            tracing::warn!(item_id = %item.item_id, error = ?e, "unreadable item consent");
            item.investments_consent
        }
    };

    // An investments-only item has no Transactions product: /transactions/sync
    // would fail and take the holdings with it. Commit just its accounts (an
    // empty cursor leaves the stored one alone). Unreadable → normal path.
    if item_is_investments_only(&accounts_json).unwrap_or(false) {
        let batch = PlaidBatch {
            item_id: item.item_id.clone(),
            accounts: accounts.clone(),
            upserts: Vec::new(),
            removed_ids: Vec::new(),
            next_cursor: String::new(),
        };
        let result = lock(store, |s| {
            s.apply_plaid_batch(&batch, synced_at)
                .map_err(|e| e.to_string())
        })?;
        let warning = after_commit(store, pcfg, &token, item, consent, &accounts, synced_at);
        return Ok((result, warning));
    }

    const MAX_RESTARTS: usize = 3;
    let mut restarts = 0;
    'restart: loop {
        let mut batch = PlaidBatch {
            item_id: item.item_id.clone(),
            accounts: accounts.clone(),
            upserts: Vec::new(),
            removed_ids: Vec::new(),
            next_cursor: String::new(),
        };
        let mut cursor = item.cursor.clone();
        loop {
            let page_json = match plaid::transactions_sync_page(pcfg, &token, cursor.as_deref()) {
                Ok(j) => j,
                Err(e) if e.contains("TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION") => {
                    restarts += 1;
                    if restarts > MAX_RESTARTS {
                        return Err(format!("sync kept mutating during pagination: {e}"));
                    }
                    continue 'restart;
                }
                Err(e) => return Err(e),
            };
            let page = parse_sync_page(&page_json).map_err(|e| format!("parse sync: {e:?}"))?;
            batch.upserts.extend(page.added);
            batch.upserts.extend(page.modified);
            batch.removed_ids.extend(page.removed_ids);
            batch.removed_ids.extend(page.pending_superseded);
            batch.next_cursor = page.next_cursor.clone();
            if !page.has_more {
                break;
            }
            if page.next_cursor.is_empty() {
                // has_more with no cursor would re-request page one forever.
                return Err("plaid sync: has_more with an empty next_cursor".into());
            }
            cursor = Some(page.next_cursor);
        }
        let result = lock(store, |s| {
            s.apply_plaid_batch(&batch, synced_at)
                .map_err(|e| e.to_string())
        })?;
        let warning = after_commit(store, pcfg, &token, item, consent, &accounts, synced_at);
        return Ok((result, warning));
    }
}

/// The post-commit steps every item gets: the holdings/balance snapshot,
/// then investment transactions. Both are non-fatal; their warnings join.
fn after_commit(
    store: &Arc<Mutex<Store>>,
    pcfg: &PlaidConfig,
    token: &str,
    item: &PlaidItem,
    consent: bool,
    accounts: &[outflow_core::Account],
    synced_at: i64,
) -> Option<String> {
    let snap = snapshot_item(store, pcfg, token, item, consent, accounts, synced_at);
    let itx = sync_investment_txns(store, pcfg, token, item, consent, accounts, synced_at);
    match (snap, itx) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}

/// Fetch holdings (when the item has Investments consent and any investment
/// account) and record this sync point. Never fails the item: transactions
/// are already committed, so a holdings error keeps the last-known positions,
/// leaves status `ok`, and comes back as a warning. Lost consent clears the
/// flag so Connections offers "Allow holdings" again.
fn snapshot_item(
    store: &Arc<Mutex<Store>>,
    pcfg: &PlaidConfig,
    token: &str,
    item: &PlaidItem,
    consent: bool,
    accounts: &[outflow_core::Account],
    synced_at: i64,
) -> Option<String> {
    let mut warning = None;
    let wants_holdings = consent
        && accounts
            .iter()
            .any(|a| a.kind == outflow_core::AccountKind::Investment);
    let holdings = if wants_holdings {
        let fetched = plaid::investments_holdings_get(pcfg, token)
            .and_then(|j| parse_holdings_get(&j).map_err(|e| format!("parse holdings: {e:?}")));
        match fetched {
            Ok(h) => Some(h),
            Err(e) => {
                if e.contains("ADDITIONAL_CONSENT_REQUIRED") {
                    let _ = lock(store, |s| {
                        s.set_plaid_item_investments_consent(&item.item_id, false)
                            .map_err(|e| e.to_string())
                    });
                }
                tracing::warn!(item_id = %item.item_id, error = %e, "holdings fetch failed; keeping last-known");
                warning = Some(format!("holdings not refreshed: {e}"));
                None
            }
        }
    } else {
        None
    };
    let recorded = lock(store, |s| {
        s.record_item_snapshot(
            accounts,
            holdings
                .as_ref()
                .map(|(sec, h)| (sec.as_slice(), h.as_slice())),
            synced_at,
        )
        .map_err(|e| e.to_string())
    });
    if let Err(e) = recorded {
        tracing::warn!(item_id = %item.item_id, error = %e, "balance snapshot failed");
        warning = Some(format!("snapshot not recorded: {e}"));
    }
    warning
}

/// Days re-fetched before the newest stored investment transaction, so late
/// postings and corrections inside that window are picked up (rows upsert by
/// id, so the overlap is harmless).
const INVEST_TXN_OVERLAP_DAYS: u64 = 30;
/// Plaid serves investment transactions up to 24 months back.
const INVEST_TXN_MAX_DAYS: u64 = 730;
/// Stop paging past this many rows in one sync (a guard, not a real limit).
const INVEST_TXN_MAX_ROWS: usize = 50_000;

/// The `start_date` for an item's fetch: 24 months back on the first sync,
/// else the newest stored date minus the overlap (never earlier than 24
/// months).
fn invest_txn_window_start(today: chrono::NaiveDate, latest: Option<&str>) -> chrono::NaiveDate {
    let floor = today - chrono::Days::new(INVEST_TXN_MAX_DAYS);
    latest
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        .map(|d| (d - chrono::Days::new(INVEST_TXN_OVERLAP_DAYS)).max(floor))
        .unwrap_or(floor)
}

/// Fetch and archive investment transactions for an item with Investments
/// consent and an investment account. Like holdings it never fails the item:
/// transactions are already committed, so a problem comes back as a warning.
fn sync_investment_txns(
    store: &Arc<Mutex<Store>>,
    pcfg: &PlaidConfig,
    token: &str,
    item: &PlaidItem,
    consent: bool,
    accounts: &[outflow_core::Account],
    synced_at: i64,
) -> Option<String> {
    let invest: Vec<&str> = accounts
        .iter()
        .filter(|a| a.kind == outflow_core::AccountKind::Investment)
        .map(|a| a.id.as_str())
        .collect();
    if !consent || invest.is_empty() {
        return None;
    }
    let result = (|| -> Result<usize, String> {
        let latest = lock(store, |s| {
            s.latest_investment_txn_date(&invest)
                .map_err(|e| e.to_string())
        })?;
        let today = chrono::Local::now().date_naive();
        let start = invest_txn_window_start(today, latest.as_deref())
            .format("%Y-%m-%d")
            .to_string();
        let end = today.format("%Y-%m-%d").to_string();
        let (mut secs, mut txns) = (Vec::new(), Vec::new());
        loop {
            let json = plaid::investments_transactions_get(pcfg, token, &start, &end, txns.len())?;
            let page = parse_investment_txns_get(&json)
                .map_err(|e| format!("parse investment transactions: {e:?}"))?;
            let got = page.txns.len();
            secs.extend(page.securities);
            txns.extend(page.txns);
            if got == 0 || txns.len() >= page.total || txns.len() >= INVEST_TXN_MAX_ROWS {
                break;
            }
        }
        lock(store, |s| {
            s.upsert_investment_txns(&secs, &txns, synced_at)
                .map_err(|e| e.to_string())
        })
    })();
    match result {
        Ok(added) => {
            if added > 0 {
                tracing::info!(item_id = %item.item_id, added, "investment transactions synced");
            }
            None
        }
        Err(e) => {
            tracing::warn!(item_id = %item.item_id, error = %e, "investment transactions not synced");
            Some(format!("investment transactions not synced: {e}"))
        }
    }
}

#[cfg(test)]
mod invest_window_tests {
    use super::*;
    use chrono::NaiveDate;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn window_is_24_months_first_then_overlaps_the_newest_row() {
        let today = d("2026-11-04");
        assert_eq!(invest_txn_window_start(today, None), d("2024-11-04"));
        assert_eq!(
            invest_txn_window_start(today, Some("2026-10-30")),
            d("2026-09-30")
        );
        // Never earlier than Plaid's 24-month floor, and junk falls back to it.
        assert_eq!(
            invest_txn_window_start(today, Some("2020-01-01")),
            d("2024-11-04")
        );
        assert_eq!(invest_txn_window_start(today, Some("bad")), d("2024-11-04"));
    }
}
