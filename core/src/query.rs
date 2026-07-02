use crate::model::Transaction;
use crate::store::Store;
use crate::subscriptions::normalize_payee;
use std::collections::HashMap;

pub struct TxnFilter {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub include_pending: bool,
}

impl TxnFilter {
    pub fn all() -> Self {
        TxnFilter {
            since: None,
            until: None,
            include_pending: true,
        }
    }

    pub fn range(since: i64, until: i64) -> Self {
        TxnFilter {
            since: Some(since),
            until: Some(until),
            include_pending: true,
        }
    }
}

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

fn ym_from_epoch(secs: i64) -> (i64, u32) {
    let days = secs.div_euclid(86400);
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32)
}

#[derive(Debug, Clone, PartialEq)]
pub struct CategorySpend {
    pub category: Option<String>,
    pub total_cents: i64,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MerchantSpend {
    pub merchant: String,
    pub total_cents: i64,
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq)]
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
        let key = ym_from_epoch(t.posted);
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
    use crate::model::Transaction;
    use crate::money::Money;

    fn t(id: &str, posted: i64, cents: i64, merchant: &str, cat: Option<&str>, pending: bool) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct".into(),
            posted,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: cat.map(|s| s.into()),
            category_source: None,
            pending,
            raw: "{}".into(),
        }
    }

    fn seed() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t("1", 1609459200, -5000, "Whole Foods", Some("Groceries"), false),
            t("2", 1609545600, -1500, "Netflix", Some("Streaming"), false),
            t("3", 1609632000, -2000, "SQ *SPOTIFY 01", None, false),
            t("4", 1612137600, -3000, "Whole Foods", Some("Groceries"), false),
            t("5", 1612224000, 250000, "Payroll", None, false),
            t("6", 1612310400, -900, "SQ *SPOTIFY 02", None, true),
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
}
