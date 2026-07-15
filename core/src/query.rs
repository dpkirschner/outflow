use crate::model::{Transaction, TxnFlag};
use crate::store::Store;
use crate::subscriptions::normalize_payee;
use chrono::{Datelike, Local, TimeZone};
use serde::Serialize;
use std::collections::HashMap;

pub struct TxnFilter {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub include_pending: bool,
    /// When false (the default), aggregations drop `Transfer`/`CardPayment`
    /// transactions so money moving between the user's own accounts isn't
    /// counted as spending. Set true for a "show transfers" view.
    pub include_non_spending: bool,
}

impl TxnFilter {
    pub fn all() -> Self {
        TxnFilter {
            since: None,
            until: None,
            include_pending: true,
            include_non_spending: false,
        }
    }

    pub fn range(since: i64, until: i64) -> Self {
        TxnFilter {
            since: Some(since),
            until: Some(until),
            include_pending: true,
            include_non_spending: false,
        }
    }
}

fn passes(t: &Transaction, f: &TxnFilter) -> bool {
    if !f.include_pending && t.pending {
        return false;
    }
    if !f.include_non_spending && t.flag != TxnFlag::Spending {
        return false;
    }
    // Filter on the behavioral date so a transaction lands in the same range it
    // is bucketed under. since inclusive, until exclusive.
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

/// (year, month) of an epoch-seconds instant in the machine's local timezone.
/// Local (not UTC) so a late-night purchase buckets into the day/month the user
/// actually made it, and DST-correct because each instant gets its own offset.
/// Shared with the rhythm detector and ledger so all month math agrees.
pub(crate) fn ym_local(secs: i64) -> (i64, u32) {
    match Local.timestamp_opt(secs, 0).single() {
        Some(dt) => (dt.year() as i64, dt.month()),
        None => (1970, 1),
    }
}

/// A monotonic month index (year*12 + month-1) in local time, so months can be
/// compared, diffed, and used as map keys. Shared with the detector/ledger.
pub(crate) fn month_index(secs: i64) -> i64 {
    let (y, m) = ym_local(secs);
    y * 12 + (m as i64 - 1)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategorySpend {
    pub category: Option<String>,
    pub total_cents: i64,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MerchantSpend {
    pub merchant: String,
    pub total_cents: i64,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonthlyFlow {
    pub year: i64,
    pub month: u32,
    pub inflow_cents: i64,
    pub outflow_cents: i64,
    pub net_cents: i64,
}

pub fn spend_by_category(store: &Store, f: &TxnFilter) -> rusqlite::Result<Vec<CategorySpend>> {
    let txns = store.all_transactions()?;
    let mut map: HashMap<Option<String>, (i64, usize)> = HashMap::new();
    for t in &txns {
        if !passes(t, f) || !t.amount.is_outflow() {
            continue;
        }
        let e = map.entry(t.category.clone()).or_insert((0, 0));
        e.0 += t.amount.cents().abs();
        e.1 += 1;
    }
    let mut out: Vec<CategorySpend> = map
        .into_iter()
        .map(|(category, (total_cents, count))| CategorySpend {
            category,
            total_cents,
            count,
        })
        .collect();
    out.sort_by(|a, b| b.total_cents.cmp(&a.total_cents));
    Ok(out)
}

pub fn top_merchants(
    store: &Store,
    f: &TxnFilter,
    limit: usize,
) -> rusqlite::Result<Vec<MerchantSpend>> {
    let txns = store.all_transactions()?;
    let mut map: HashMap<String, (i64, usize)> = HashMap::new();
    for t in &txns {
        if !passes(t, f) || !t.amount.is_outflow() {
            continue;
        }
        let key = normalize_payee(t.merchant());
        if key.is_empty() {
            continue;
        }
        let e = map.entry(key).or_insert((0, 0));
        e.0 += t.amount.cents().abs();
        e.1 += 1;
    }
    let mut out: Vec<MerchantSpend> = map
        .into_iter()
        .map(|(merchant, (total_cents, count))| MerchantSpend {
            merchant,
            total_cents,
            count,
        })
        .collect();
    out.sort_by(|a, b| b.total_cents.cmp(&a.total_cents));
    out.truncate(limit);
    Ok(out)
}

pub fn monthly_flow(store: &Store, f: &TxnFilter) -> rusqlite::Result<Vec<MonthlyFlow>> {
    let txns = store.all_transactions()?;
    let mut map: HashMap<(i64, u32), (i64, i64)> = HashMap::new();
    for t in &txns {
        if !passes(t, f) {
            continue;
        }
        let key = ym_local(t.effective_date());
        let e = map.entry(key).or_insert((0, 0));
        if t.amount.is_outflow() {
            e.1 += t.amount.cents().abs();
        } else {
            e.0 += t.amount.cents();
        }
    }
    let mut out: Vec<MonthlyFlow> = map
        .into_iter()
        .map(|((year, month), (inflow_cents, outflow_cents))| MonthlyFlow {
            year,
            month,
            inflow_cents,
            outflow_cents,
            net_cents: inflow_cents - outflow_cents,
        })
        .collect();
    out.sort_by(|a, b| (a.year, a.month).cmp(&(b.year, b.month)));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Transaction, TxnFlag};
    use crate::money::Money;

    // Mid-month, noon UTC: bucketing is unambiguous in any timezone from UTC-12
    // to UTC+14, so these local-time tests stay deterministic on any machine.
    const JAN15: i64 = 1610712000; // 2021-01-15 12:00:00 UTC
    const FEB15: i64 = 1613390400; // 2021-02-15 12:00:00 UTC

    fn t(id: &str, posted: i64, cents: i64, merchant: &str, cat: Option<&str>, pending: bool) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct".into(),
            posted,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: cat.map(|s| s.into()),
            category_source: None,
            pending,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            source: crate::model::default_source(),
        }
    }

    fn seed() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t("1", JAN15, -5000, "Whole Foods", Some("Groceries"), false),
            t("2", JAN15, -1500, "Netflix", Some("Streaming"), false),
            t("3", JAN15, -2000, "SQ *SPOTIFY 01", None, false),
            t("4", FEB15, -3000, "Whole Foods", Some("Groceries"), false),
            t("5", FEB15, 250000, "Payroll", None, false),
            t("6", FEB15, -900, "SQ *SPOTIFY 02", None, true),
        ])
        .unwrap();
        s
    }

    #[test]
    fn category_spend_sums_outflows_and_buckets_uncategorized() {
        let s = seed();
        let rows = spend_by_category(&s, &TxnFilter::all()).unwrap();
        let groceries = rows.iter().find(|r| r.category.as_deref() == Some("Groceries")).unwrap();
        assert_eq!(groceries.total_cents, 8000);
        assert_eq!(groceries.count, 2);
        let uncat = rows.iter().find(|r| r.category.is_none()).unwrap();
        assert_eq!(uncat.total_cents, 2900);
        assert_eq!(rows[0].category.as_deref(), Some("Groceries"));
    }

    #[test]
    fn category_spend_ignores_inflows() {
        let s = seed();
        let rows = spend_by_category(&s, &TxnFilter::all()).unwrap();
        assert!(rows.iter().all(|r| r.total_cents > 0));
        let total: i64 = rows.iter().map(|r| r.total_cents).sum();
        assert_eq!(total, 5000 + 1500 + 2000 + 3000 + 900);
    }

    #[test]
    fn top_merchants_groups_normalized_and_limits() {
        let s = seed();
        let rows = top_merchants(&s, &TxnFilter::all(), 2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].merchant, "whole foods");
        assert_eq!(rows[0].total_cents, 8000);
        let spotify = top_merchants(&s, &TxnFilter::all(), 10)
            .unwrap()
            .into_iter()
            .find(|r| r.merchant == "spotify")
            .unwrap();
        assert_eq!(spotify.count, 2);
        assert_eq!(spotify.total_cents, 2900);
    }

    #[test]
    fn monthly_flow_splits_in_and_out() {
        let s = seed();
        let rows = monthly_flow(&s, &TxnFilter::all()).unwrap();
        let jan = rows.iter().find(|r| (r.year, r.month) == (2021, 1)).unwrap();
        assert_eq!(jan.outflow_cents, 8500);
        assert_eq!(jan.inflow_cents, 0);
        assert_eq!(jan.net_cents, -8500);
        let feb = rows.iter().find(|r| (r.year, r.month) == (2021, 2)).unwrap();
        assert_eq!(feb.inflow_cents, 250000);
        assert_eq!(feb.outflow_cents, 3900);
        assert_eq!(feb.net_cents, 246100);
    }

    #[test]
    fn excluding_pending_drops_provisional_rows() {
        let s = seed();
        let mut f = TxnFilter::all();
        f.include_pending = false;
        let spotify = top_merchants(&s, &f, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.merchant == "spotify")
            .unwrap();
        assert_eq!(spotify.count, 1);
        assert_eq!(spotify.total_cents, 2000);
    }

    #[test]
    fn range_filter_bounds_are_since_inclusive_until_exclusive() {
        let s = seed();
        let f = TxnFilter::range(1609459200, 1612137600);
        let rows = monthly_flow(&s, &f).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].year, rows[0].month), (2021, 1));
    }

    #[test]
    fn non_spending_excluded_by_default_and_readmitted_by_toggle() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t("spend", JAN15, -2000, "Whole Foods", Some("Groceries"), false),
            t("xfer", JAN15, -50000, "Move to Savings", None, false),
            t("pay", JAN15, -100000, "Amex Payment", None, false),
        ])
        .unwrap();
        s.set_flag("xfer", TxnFlag::Transfer, false).unwrap();
        s.set_flag("pay", TxnFlag::CardPayment, false).unwrap();

        let total: i64 = spend_by_category(&s, &TxnFilter::all())
            .unwrap()
            .iter()
            .map(|c| c.total_cents)
            .sum();
        assert_eq!(total, 2000, "transfer + card payment must be suppressed");

        let merch = top_merchants(&s, &TxnFilter::all(), 10).unwrap();
        assert!(merch.iter().all(|m| m.merchant == "whole foods"));

        let mut f = TxnFilter::all();
        f.include_non_spending = true;
        let total_all: i64 = spend_by_category(&s, &f)
            .unwrap()
            .iter()
            .map(|c| c.total_cents)
            .sum();
        assert_eq!(total_all, 2000 + 50000 + 100000);
    }

    #[test]
    fn monthly_flow_excludes_transfers_on_both_legs() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t("in", JAN15, 30000, "Transfer from Savings", None, false),
            t("out", JAN15, -30000, "Transfer to Savings", None, false),
            t("spend", JAN15, -2000, "Coffee", Some("Coffee"), false),
        ])
        .unwrap();
        s.set_flag("in", TxnFlag::Transfer, false).unwrap();
        s.set_flag("out", TxnFlag::Transfer, false).unwrap();

        let rows = monthly_flow(&s, &TxnFilter::all()).unwrap();
        let jan = rows.iter().find(|r| (r.year, r.month) == (2021, 1)).unwrap();
        assert_eq!(jan.inflow_cents, 0, "transfer-in is not income");
        assert_eq!(jan.outflow_cents, 2000, "transfer-out is not spend");
        assert_eq!(jan.net_cents, -2000);
    }

    #[test]
    fn buckets_on_transacted_at_not_posted() {
        // Behaved in January, posted in February → must bucket into January.
        let s = Store::open_in_memory().unwrap();
        let mut tx = t("late", FEB15, -1000, "Corner Store", Some("Shopping"), false);
        tx.transacted_at = Some(JAN15);
        s.upsert_transactions(&[tx]).unwrap();
        let rows = monthly_flow(&s, &TxnFilter::all()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].year, rows[0].month), (2021, 1));
    }
}
