//! The rhythm ledger view model — the single source of truth for the ledger
//! screen. `ledger()` partitions a window of transactions **once** into the
//! screen's zones (streams / committed / notable / transfers / noise) so nothing
//! double-counts, and attaches per-stream sources. Everything downstream (Tauri
//! commands, the React screen) is a thin render over `LedgerView`.

use crate::model::{Account, AccountKind, Mark, Transaction, TxnFlag};
use crate::store::Store;
use crate::subscriptions::{detect_rhythms, normalize_payee, rhythm_for_items, RhythmEntry};
use serde::Serialize;
use std::collections::HashMap;

/// One-off spend at or above this is "Notable" rather than "Noise" (flat start;
/// spec revisits relative-to-median later).
const NOTABLE_THRESHOLD_CENTS: i64 = 40_000; // $400
const NOTABLE_TOP_N: usize = 8;

/// Categories that make a recurring stream a fixed obligation ("Committed") — kept
/// out of the hunt. Matched case-insensitively against a stream's dominant
/// category. Editable; the slide-over's manual "Mark as Committed" covers the rest.
const COMMITTED_CATEGORIES: &[&str] = &[
    "rent",
    "housing",
    "home",
    "mortgage",
    "insurance",
    "loan",
    "loans",
    "debt",
];

/// Which account(s) a stream's charges land on. `label` = the full account name
/// (e.g. "VentureOne (3419)"); `kind` = the account type, which the UI colors by
/// (cash vs credit vs investment); `pct` = share of occurrences on this account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    pub label: String,
    pub kind: AccountKind,
    pub pct: u8,
}

/// A recurring stream row: the detected rhythm plus its sources and the dominant
/// category (used for Committed classification and display).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stream {
    #[serde(flatten)]
    pub rhythm: RhythmEntry,
    pub sources: Vec<Source>,
    pub category: Option<String>,
}

/// A single transaction line item — used for both Notable one-offs and the
/// expandable Noise list, so the user can always see the rows behind a zone.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LineItem {
    pub id: String,
    pub merchant: String,
    /// Normalized merchant (`normalize_payee`) — the key the frontend groups
    /// noise rows by and addresses promote/preview with, since it must not run
    /// the normalizer itself (invariant #4).
    pub merchant_key: String,
    pub date: i64,
    pub amount_cents: i64,
    pub category: Option<String>,
    pub source: Source,
}

/// A suppressed group (Transfer or CardPayment), shown collapsed and excluded
/// from every total.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransferGroup {
    pub merchant: String,
    pub flag: TxnFlag,
    pub total_cents: i64,
    pub count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct LedgerStats {
    pub recurring_monthly_cents: i64,
    pub stream_count: usize,
    pub committed_monthly_cents: i64,
    pub noise_total_cents: i64,
    pub noise_count: usize,
}

/// How much data sits behind the current view — so the UI can distinguish "the
/// window is set to 12mo" from "you only have 3 months of history."
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Coverage {
    /// Earliest behavioral date across the WHOLE database (not just the window).
    /// `None` when there are no transactions.
    pub earliest: Option<i64>,
    /// Count of outflow transactions within the window — the spend the screen
    /// analyzes.
    pub window_txn_count: usize,
    /// True when the window reaches back to (or before) the earliest data — i.e.
    /// it's showing everything, not clipping. Every window ≥ your history reads
    /// the same until the archive accumulates past the window.
    pub covers_all: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LedgerView {
    pub coverage: Coverage,
    pub stats: LedgerStats,
    pub streams: Vec<Stream>,
    pub committed: Vec<Stream>,
    pub notable: Vec<LineItem>,
    /// The individual sub-threshold one-offs behind the Noise summary, newest
    /// first — so the fold can list them, not just the total.
    pub noise_items: Vec<LineItem>,
    pub transfers: Vec<TransferGroup>,
}

/// Label + kind for an account (pct filled in per stream). The label is the full
/// account name for every kind; the UI colors the chip by `kind`.
fn account_source(acct: &Account) -> (String, AccountKind) {
    (acct.name.clone(), acct.kind)
}

/// The dominant non-null category among a group of transactions, if any.
fn dominant_category(items: &[&Transaction]) -> Option<String> {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for t in items {
        if let Some(c) = &t.category {
            *counts.entry(c.as_str()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(c, _)| c.to_string())
}

/// Per-account source distribution for a stream's occurrences, sorted by share.
fn stream_sources(items: &[&Transaction], accounts: &HashMap<String, Account>) -> Vec<Source> {
    let total = items.len().max(1);
    let mut by_acct: HashMap<&str, usize> = HashMap::new();
    for t in items {
        *by_acct.entry(t.account_id.as_str()).or_insert(0) += 1;
    }
    let mut out: Vec<Source> = by_acct
        .into_iter()
        .map(|(id, n)| {
            let (label, kind) = accounts
                .get(id)
                .map(account_source)
                .unwrap_or_else(|| (id.to_string(), AccountKind::Other));
            Source {
                label,
                kind,
                pct: ((n * 100 + total / 2) / total) as u8,
            }
        })
        .collect();
    out.sort_by(|a, b| b.pct.cmp(&a.pct));
    out
}

/// Build a line item (with its account source) for a single transaction.
fn line_item(t: &Transaction, accounts: &HashMap<String, Account>) -> LineItem {
    let (label, kind) = accounts
        .get(&t.account_id)
        .map(account_source)
        .unwrap_or_else(|| (t.account_id.clone(), AccountKind::Other));
    LineItem {
        id: t.id.clone(),
        merchant: t.merchant().to_string(),
        merchant_key: normalize_payee(t.merchant()),
        date: t.effective_date(),
        amount_cents: t.amount.cents().abs(),
        category: t.category.clone(),
        source: Source { label, kind, pct: 100 },
    }
}

fn is_committed(category: &Option<String>, mark: Option<Mark>) -> bool {
    match mark {
        Some(Mark::Committed) => true,
        Some(Mark::Kept) => false, // force into the main streams list
        Some(Mark::Dismissed) => false, // handled earlier (dropped from streams)
        None => match category {
            Some(c) => {
                let lc = c.to_lowercase();
                COMMITTED_CATEGORIES.iter().any(|k| lc == *k)
            }
            None => false,
        },
    }
}

/// Assemble the whole ledger view for a window. `since` bounds transactions on the
/// behavioral date (inclusive; `None` = all history); `now` fixes the in-progress
/// month for the rate/trend math.
pub fn ledger(store: &Store, since: Option<i64>, now: i64) -> rusqlite::Result<LedgerView> {
    let accounts: HashMap<String, Account> = store
        .accounts()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();

    let all = store.all_transactions()?;
    // Earliest date across the whole archive (independent of the window), so the
    // UI can report how far back the data goes.
    let earliest = all.iter().map(|t| t.effective_date()).min();
    let windowed: Vec<Transaction> = all
        .into_iter()
        .filter(|t| since.map_or(true, |s| t.effective_date() >= s))
        .collect();
    let coverage = Coverage {
        earliest,
        window_txn_count: windowed.iter().filter(|t| t.amount.is_outflow()).count(),
        covers_all: match since {
            None => true,
            Some(s) => earliest.map_or(true, |e| e >= s),
        },
    };

    let marks = store.merchant_marks()?;

    // Group Spending outflows by normalized merchant — the same grouping the
    // detector uses — so we can attach sources/category per stream.
    let mut groups: HashMap<String, Vec<&Transaction>> = HashMap::new();
    for t in windowed
        .iter()
        .filter(|t| t.amount.is_outflow() && t.flag == TxnFlag::Spending)
    {
        let key = normalize_payee(t.merchant());
        if key.is_empty() {
            continue;
        }
        groups.entry(key).or_default().push(t);
    }

    let entries: Vec<RhythmEntry> = detect_rhythms(&windowed, now);

    let mut streams: Vec<Stream> = Vec::new();
    let mut committed: Vec<Stream> = Vec::new();
    // Merchants that are "accounted for" by a stream — excluded from notable/noise.
    // A Dismissed stream is NOT accounted (its spend falls back to notable/noise).
    let mut accounted: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in entries {
        let key = entry.merchant.clone();
        let mark = marks.get(&key).copied();
        if mark == Some(Mark::Dismissed) {
            continue; // repartitioned into notable/noise below
        }
        accounted.insert(key.clone());
        let items = groups.get(&key).cloned().unwrap_or_default();
        let category = dominant_category(&items);
        let sources = stream_sources(&items, &accounts);
        let stream = Stream {
            rhythm: entry,
            sources,
            category: category.clone(),
        };
        if is_committed(&category, mark) {
            committed.push(stream);
        } else {
            streams.push(stream);
        }
    }

    // Force-promoted merchants: a `Kept` mark the auto-detector didn't already
    // account for (too few occurrences, or irregular spacing). Synthesize a
    // stream from its occurrences so it leaves Noise. Runs BEFORE the
    // notable/noise partition so `accounted` suppresses its rows there.
    for (key, mark) in &marks {
        if *mark != Mark::Kept || accounted.contains(key) {
            continue;
        }
        let items = groups.get(key).cloned().unwrap_or_default();
        if let Some(rhythm) = rhythm_for_items(key.clone(), &items, now) {
            accounted.insert(key.clone());
            let category = dominant_category(&items);
            let sources = stream_sources(&items, &accounts);
            streams.push(Stream { rhythm, sources, category });
        }
    }

    // Non-stream spending outflows → Notable (≥ threshold) or Noise (below).
    let mut notable: Vec<LineItem> = Vec::new();
    let mut noise_items: Vec<LineItem> = Vec::new();
    let mut noise_total = 0i64;
    for t in windowed
        .iter()
        .filter(|t| t.amount.is_outflow() && t.flag == TxnFlag::Spending)
    {
        let key = normalize_payee(t.merchant());
        if accounted.contains(&key) {
            continue;
        }
        let amt = t.amount.cents().abs();
        let item = line_item(t, &accounts);
        if amt >= NOTABLE_THRESHOLD_CENTS {
            notable.push(item);
        } else {
            noise_total += amt;
            noise_items.push(item);
        }
    }
    notable.sort_by(|a, b| b.amount_cents.cmp(&a.amount_cents));
    notable.truncate(NOTABLE_TOP_N);
    noise_items.sort_by(|a, b| b.date.cmp(&a.date)); // newest first
    let noise_count = noise_items.len();

    // Transfers / card payments — grouped, excluded from every total.
    let mut xfer: HashMap<(String, &'static str), (TxnFlag, i64, usize)> = HashMap::new();
    for t in windowed.iter().filter(|t| t.flag != TxnFlag::Spending) {
        let key = (normalize_payee(t.merchant()), t.flag.as_str());
        let e = xfer.entry(key).or_insert((t.flag, 0, 0));
        e.1 += t.amount.cents().abs();
        e.2 += 1;
    }
    let mut transfers: Vec<TransferGroup> = xfer
        .into_iter()
        .map(|((merchant, _), (flag, total_cents, count))| TransferGroup {
            merchant,
            flag,
            total_cents,
            count,
        })
        .collect();
    transfers.sort_by(|a, b| b.total_cents.cmp(&a.total_cents));

    let stats = LedgerStats {
        recurring_monthly_cents: streams.iter().map(|s| s.rhythm.monthly_estimate_cents).sum(),
        stream_count: streams.len(),
        committed_monthly_cents: committed.iter().map(|s| s.rhythm.monthly_estimate_cents).sum(),
        noise_total_cents: noise_total,
        noise_count,
    };

    Ok(LedgerView {
        coverage,
        stats,
        streams,
        committed,
        notable,
        noise_items,
        transfers,
    })
}

/// Synthesize the `Stream` a merchant WOULD become if force-promoted, without
/// touching any mark — powers the Noise popover's cadence/rate preview. Uses the
/// same Spending-only grouping and normalized key as `ledger()`, so the preview
/// matches what promoting actually produces. `None` when the merchant has no
/// spending occurrences in the window.
pub fn stream_preview(
    store: &Store,
    merchant_key: &str,
    since: Option<i64>,
    now: i64,
) -> rusqlite::Result<Option<Stream>> {
    let accounts: HashMap<String, Account> = store
        .accounts()?
        .into_iter()
        .map(|a| (a.id.clone(), a))
        .collect();
    let all = store.all_transactions()?;
    let items: Vec<&Transaction> = all
        .iter()
        .filter(|t| {
            t.amount.is_outflow()
                && t.flag == TxnFlag::Spending
                && normalize_payee(t.merchant()) == merchant_key
                && since.map_or(true, |s| t.effective_date() >= s)
        })
        .collect();
    Ok(rhythm_for_items(merchant_key.to_string(), &items, now).map(|rhythm| {
        let category = dominant_category(&items);
        let sources = stream_sources(&items, &accounts);
        Stream {
            rhythm,
            sources,
            category,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AccountKind;
    use crate::money::Money;

    const BASE: i64 = 1_610_712_000; // 2021-01-15 12:00 UTC
    const DAY: i64 = 86_400;
    const NOW: i64 = BASE + 400 * DAY;

    fn acct(id: &str, name: &str, kind: AccountKind) -> Account {
        Account {
            id: id.into(),
            org: "Bank".into(),
            name: name.into(),
            kind,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: crate::model::default_source(),
        }
    }

    fn tx(id: &str, acct: &str, day: i64, cents: i64, merchant: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: acct.into(),
            posted: BASE + day * DAY,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            source: crate::model::default_source(),
        }
    }

    #[test]
    fn partitions_streams_notable_and_noise_without_double_count() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "SimpleFIN Checking", AccountKind::Checking)])
            .unwrap();
        let mut txns = Vec::new();
        // A daily stream (grocery), 20 days.
        for d in 0..20 {
            txns.push(tx(&format!("g{d}"), "chk", d, -(2000 + d * 10), "Grocery Store"));
        }
        // A big one-off ($900) → Notable.
        txns.push(tx("big", "chk", 5, -90000, "Tree Removal Co"));
        // Two small scattered one-offs → Noise.
        txns.push(tx("n1", "chk", 2, -1200, "Random Cafe"));
        txns.push(tx("n2", "chk", 9, -800, "Corner Shop"));
        s.upsert_transactions(&txns).unwrap();

        let v = ledger(&s, None, NOW).unwrap();
        assert_eq!(v.streams.len(), 1, "grocery is the only stream");
        assert_eq!(v.streams[0].rhythm.merchant, "grocery store");
        assert_eq!(v.streams[0].sources.len(), 1);
        assert_eq!(v.streams[0].sources[0].kind, AccountKind::Checking);
        assert_eq!(v.notable.len(), 1);
        assert_eq!(v.notable[0].amount_cents, 90000);
        assert_eq!(v.stats.noise_count, 2);
        assert_eq!(v.stats.noise_total_cents, 2000);
        // The grocery charges are NOT also in noise/notable.
        assert!(v.stats.recurring_monthly_cents > 0);
    }

    #[test]
    fn committed_by_category_and_mark_leaves_the_streams_list() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        // Two monthly streams.
        let mut txns = Vec::new();
        for (i, d) in [0i64, 31, 59, 90].iter().enumerate() {
            txns.push(tx(&format!("m{i}"), "chk", *d, -220000, "Chase Mortgage"));
            txns.push(tx(&format!("g{i}"), "chk", *d, -4500, "Planet Fitness"));
        }
        s.upsert_transactions(&txns).unwrap();
        // Mortgage → Committed by category.
        for t in s.all_transactions().unwrap() {
            if t.merchant() == "Chase Mortgage" {
                s.set_category(&t.id, "Rent", crate::model::CategorySource::Manual).unwrap();
            }
        }
        let v = ledger(&s, None, NOW).unwrap();
        assert_eq!(v.committed.len(), 1);
        assert_eq!(v.committed[0].rhythm.merchant, "chase mortgage");
        assert_eq!(v.streams.len(), 1);
        assert_eq!(v.streams[0].rhythm.merchant, "planet fitness");

        // Manually mark Planet Fitness Committed too.
        s.set_merchant_mark("planet fitness", Mark::Committed).unwrap();
        let v2 = ledger(&s, None, NOW).unwrap();
        assert_eq!(v2.streams.len(), 0);
        assert_eq!(v2.committed.len(), 2);

        // "Return to the hunt": a Kept mark forces the category-committed mortgage
        // back into the main streams list, overriding its Rent category.
        s.set_merchant_mark("chase mortgage", Mark::Kept).unwrap();
        let v3 = ledger(&s, None, NOW).unwrap();
        assert!(v3.streams.iter().any(|s| s.rhythm.merchant == "chase mortgage"));
        assert!(v3.committed.iter().all(|s| s.rhythm.merchant != "chase mortgage"));
    }

    #[test]
    fn dismissed_stream_falls_to_noise_or_notable() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        let mut txns = Vec::new();
        for d in 0..20 {
            txns.push(tx(&format!("c{d}"), "chk", d, -500, "Blue Bottle"));
        }
        s.upsert_transactions(&txns).unwrap();
        assert_eq!(ledger(&s, None, NOW).unwrap().streams.len(), 1);

        s.set_merchant_mark("blue bottle", Mark::Dismissed).unwrap();
        let v = ledger(&s, None, NOW).unwrap();
        assert_eq!(v.streams.len(), 0, "dismissed → not a stream");
        // Each $5 charge is below the notable threshold → noise.
        assert_eq!(v.stats.noise_count, 20);
        assert!(v.notable.is_empty());
        // The individual rows are still visible in the noise list.
        assert_eq!(v.noise_items.len(), 20);
        assert!(v.noise_items.iter().all(|i| i.merchant == "Blue Bottle"));
    }

    #[test]
    fn stream_serializes_flat_for_the_ts_contract() {
        // The TS `Stream` interface expects the RhythmEntry fields flattened
        // alongside `sources`/`category`. Guard the serde(flatten) boundary.
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        let txns: Vec<_> = (0..20)
            .map(|d| tx(&format!("c{d}"), "chk", d, -500, "Blue Bottle"))
            .collect();
        s.upsert_transactions(&txns).unwrap();
        let v = ledger(&s, None, NOW).unwrap();
        let json = serde_json::to_value(&v.streams[0]).unwrap();
        let obj = json.as_object().unwrap();
        for key in [
            "merchant",
            "cadence",
            "occurrence_count",
            "median_amount_cents",
            "monthly_estimate_cents",
            "trend_pct",
            "trend",
            "spark_cents",
            "sources",
            "category",
        ] {
            assert!(obj.contains_key(key), "Stream JSON missing top-level `{key}`");
        }
        assert_eq!(obj["cadence"], serde_json::json!("Daily"));
    }

    #[test]
    fn coverage_distinguishes_window_from_available_data() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        // Data spans day 0 to day 100.
        s.upsert_transactions(&[
            tx("a", "chk", 0, -1000, "Old Shop"),
            tx("b", "chk", 100, -1000, "New Shop"),
        ])
        .unwrap();

        // No window → covers everything, both txns.
        let all = ledger(&s, None, NOW).unwrap();
        assert!(all.coverage.covers_all);
        assert_eq!(all.coverage.window_txn_count, 2);
        assert_eq!(all.coverage.earliest, Some(BASE));

        // A `since` after the earliest txn → the window is clipping.
        let since = BASE + 50 * DAY;
        let clipped = ledger(&s, Some(since), NOW).unwrap();
        assert!(!clipped.coverage.covers_all, "window starts after earliest data");
        assert_eq!(clipped.coverage.window_txn_count, 1);
        // earliest still reports the true start of the archive, not the window.
        assert_eq!(clipped.coverage.earliest, Some(BASE));
    }

    #[test]
    fn transfers_are_grouped_and_excluded() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        let mut a = tx("t1", "chk", 1, -50000, "Move to Savings");
        a.flag = TxnFlag::Transfer;
        let mut b = tx("t2", "chk", 2, -30000, "Move to Savings");
        b.flag = TxnFlag::Transfer;
        s.upsert_transactions(&[a, b, tx("s1", "chk", 3, -2000, "Coffee")]).unwrap();
        let v = ledger(&s, None, NOW).unwrap();
        assert_eq!(v.transfers.len(), 1);
        assert_eq!(v.transfers[0].total_cents, 80000);
        assert_eq!(v.transfers[0].count, 2);
        // The transfer isn't in noise (only the $20 coffee is).
        assert_eq!(v.stats.noise_total_cents, 2000);
    }

    #[test]
    fn line_item_carries_normalized_merchant_key() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        s.upsert_transactions(&[tx("n1", "chk", 2, -1200, "SQ *RANDOM CAFE 991")]).unwrap();
        let v = ledger(&s, None, NOW).unwrap();
        assert_eq!(v.noise_items.len(), 1);
        assert_eq!(v.noise_items[0].merchant, "SQ *RANDOM CAFE 991"); // raw, for display
        assert_eq!(v.noise_items[0].merchant_key, "random cafe"); // normalized, for promote
    }

    #[test]
    fn kept_promotes_an_undetected_merchant_out_of_noise() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        // Erratic spacing: the detector never makes this a stream.
        let txns = vec![
            tx("c1", "chk", 0, -1200, "Corner Store"),
            tx("c2", "chk", 3, -4500, "Corner Store"),
            tx("c3", "chk", 47, -800, "Corner Store"),
        ];
        s.upsert_transactions(&txns).unwrap();
        let before = ledger(&s, None, NOW).unwrap();
        assert!(before.streams.is_empty());
        assert_eq!(before.stats.noise_count, 3);

        // Force-promote: a Kept mark synthesizes an Irregular stream.
        s.set_merchant_mark("corner store", Mark::Kept).unwrap();
        let after = ledger(&s, None, NOW).unwrap();
        assert_eq!(after.streams.len(), 1);
        assert_eq!(after.streams[0].rhythm.merchant, "corner store");
        assert_eq!(after.streams[0].rhythm.cadence, crate::StreamCadence::Irregular);
        // Its rows are pulled out of noise — no double-count.
        assert_eq!(after.stats.noise_count, 0);
        assert!(after.noise_items.is_empty());
    }

    #[test]
    fn dismissed_then_kept_round_trips_a_real_rhythm() {
        // Uber-shaped: a genuine FewPerMonth rhythm, dismissed to noise, then promoted back.
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        let days = [0i64, 17, 30, 47, 60, 77];
        let txns: Vec<_> = days
            .iter()
            .enumerate()
            .map(|(i, d)| tx(&format!("u{i}"), "chk", *d, -600, "Uber"))
            .collect();
        s.upsert_transactions(&txns).unwrap();
        assert_eq!(ledger(&s, None, NOW).unwrap().streams.len(), 1, "detected as a stream");

        s.set_merchant_mark("uber", Mark::Dismissed).unwrap();
        let dismissed = ledger(&s, None, NOW).unwrap();
        assert!(dismissed.streams.is_empty());
        assert_eq!(dismissed.stats.noise_count, 6);

        s.set_merchant_mark("uber", Mark::Kept).unwrap();
        let promoted = ledger(&s, None, NOW).unwrap();
        assert_eq!(promoted.streams.len(), 1);
        assert_eq!(promoted.streams[0].rhythm.merchant, "uber");
        // Regular spacing → the real cadence survives, not Irregular.
        assert_eq!(promoted.streams[0].rhythm.cadence, crate::StreamCadence::FewPerMonth);
        assert_eq!(promoted.stats.noise_count, 0);
    }

    #[test]
    fn stream_preview_matches_what_promotion_produces() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        let txns = vec![
            tx("c1", "chk", 0, -1200, "Corner Store"),
            tx("c2", "chk", 3, -4500, "Corner Store"),
            tx("c3", "chk", 47, -800, "Corner Store"),
        ];
        s.upsert_transactions(&txns).unwrap();

        let preview = stream_preview(&s, "corner store", None, NOW).unwrap().unwrap();
        assert_eq!(preview.rhythm.merchant, "corner store");
        assert_eq!(preview.rhythm.occurrence_count, 3);
        assert_eq!(preview.rhythm.cadence, crate::StreamCadence::Irregular);

        // Preview is read-only: no stream materializes until the mark is set.
        assert!(ledger(&s, None, NOW).unwrap().streams.is_empty());

        // And it equals the real stream once promoted.
        s.set_merchant_mark("corner store", Mark::Kept).unwrap();
        let real = &ledger(&s, None, NOW).unwrap().streams[0].rhythm;
        assert_eq!(real.cadence, preview.rhythm.cadence);
        assert_eq!(real.occurrence_count, preview.rhythm.occurrence_count);
        assert_eq!(real.monthly_estimate_cents, preview.rhythm.monthly_estimate_cents);
    }

    #[test]
    fn stream_preview_none_for_unknown_merchant() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("chk", "Checking", AccountKind::Checking)]).unwrap();
        s.upsert_transactions(&[tx("c1", "chk", 0, -1200, "Corner Store")]).unwrap();
        assert!(stream_preview(&s, "nonexistent", None, NOW).unwrap().is_none());
    }
}
