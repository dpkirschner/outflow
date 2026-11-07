//! The recurring list (GLOSSARY: Recurring). One row per recurring merchant
//! over the 12 calendar months ending at a chosen month: how often, the
//! monthly cost, a six-month spark and a change marker. Detection is
//! `subscriptions::detect_rhythms`; the one per-merchant mark is
//! Ignored, which moves a merchant to a separate list.

use crate::model::Transaction;
use crate::query::month_index;
use crate::store::Store;
use crate::subscriptions::{detect_rhythms, median, StreamCadence, MONTH_DAYS};
use chrono::{Datelike, NaiveDate};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Calendar months the list looks back over, the chosen month included.
pub const WINDOW_MONTHS: i64 = 12;
/// Months in the spark, ending at the chosen month.
pub const SPARK_MONTHS: i64 = 6;
/// A merchant whose first charge in the window falls in the last this-many
/// months is New. Three, because a monthly charge needs three occurrences
/// before the detector sees it.
pub const NEW_MONTHS: i64 = 3;
/// Up / Down when the last completed month is more than this far from the
/// mean of the (up to three) completed months before it. Shared with the
/// "unusual" rule in GLOSSARY.
pub const CHANGE_PCT: f64 = 25.0;

/// How a recurring merchant's cost moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Change {
    /// First charge in the window falls in the last `NEW_MONTHS` months.
    New,
    /// Last completed month more than `CHANGE_PCT` above its usual.
    Up,
    /// Last completed month more than `CHANGE_PCT` below its usual.
    Down,
    Steady,
}

/// One recurring merchant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecurringMerchant {
    /// Normalized merchant (`normalize_payee`): the key for `merchant=` on
    /// `/transactions` and for the Ignored mark.
    pub merchant: String,
    /// The merchant as the bank writes it most often, for display.
    pub name: String,
    pub cadence: StreamCadence,
    /// Charges in the window.
    pub count: usize,
    /// Median charge, positive = money spent.
    pub median_cents: i64,
    /// Usual monthly cost: mean of the completed months since the first
    /// charge in the window (months with no charge count as $0). Positive.
    pub monthly_cents: i64,
    pub change: Change,
    /// Signed % of the last completed month against its usual; `None` with
    /// fewer than two completed months.
    pub change_pct: Option<f64>,
    /// Behavioral date (epoch seconds) of the first and last charge in the
    /// window.
    pub first_seen: i64,
    pub last_seen: i64,
    /// Spend per month, `SPARK_MONTHS` long, oldest first, ending at the
    /// chosen month (which may still be in progress). Positive.
    pub spark_cents: Vec<i64>,
    /// Most common category, if any charge has one.
    pub category: Option<String>,
    /// The account most charges land on.
    pub account_id: String,
}

/// The recurring list as of one month.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecurringView {
    /// The chosen month, `YYYY-MM`: the window's last month.
    pub month: String,
    /// Window bounds, `YYYY-MM-DD`, both inclusive: pass them as
    /// `since`/`until` to `/transactions` to see the rows behind a merchant.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub since: NaiveDate,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub until: NaiveDate,
    /// Whether the chosen month is still in progress.
    pub in_progress: bool,
    /// Sum of `monthly_cents` over `items`.
    pub monthly_total_cents: i64,
    /// Not ignored, by monthly cost (largest first).
    pub items: Vec<RecurringMerchant>,
    /// Ignored merchants that are still detected, same order.
    pub ignored: Vec<RecurringMerchant>,
}

fn month_of(idx: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(idx.div_euclid(12) as i32, idx.rem_euclid(12) as u32 + 1, 1)
        .expect("valid month")
}

fn index_of(d: NaiveDate) -> i64 {
    d.year() as i64 * 12 + d.month0() as i64
}

/// The recurring list for the 12 months ending with the month that contains
/// `through` (a local date). `now` decides which months are complete.
pub fn recurring(store: &Store, through: NaiveDate, now: i64) -> rusqlite::Result<RecurringView> {
    let all = store.all_transactions()?;
    let ignored = store.ignored_merchants()?;
    Ok(build(&all, &ignored, through, now))
}

fn build(
    all: &[Transaction],
    ignored: &HashSet<String>,
    through: NaiveDate,
    now: i64,
) -> RecurringView {
    let last = index_of(through);
    let first = last - (WINDOW_MONTHS - 1);
    let current = month_index(now);
    let earliest = all.iter().map(|t| month_index(t.effective_date())).min();
    let window: Vec<Transaction> = all
        .iter()
        .filter(|t| (first..=last).contains(&month_index(t.effective_date())))
        .cloned()
        .collect();

    let mut items = Vec::new();
    let mut ign = Vec::new();
    for g in detect_rhythms(&window) {
        let row = merchant_row(
            &g.merchant,
            &g.items,
            g.cadence,
            g.med_gap,
            last,
            current,
            earliest,
        );
        if ignored.contains(&g.merchant) {
            ign.push(row);
        } else {
            items.push(row);
        }
    }
    let order = |v: &mut Vec<RecurringMerchant>| {
        v.sort_by(|a, b| {
            b.monthly_cents
                .cmp(&a.monthly_cents)
                .then_with(|| a.merchant.cmp(&b.merchant))
        })
    };
    order(&mut items);
    order(&mut ign);

    let until = month_of(last + 1).pred_opt().expect("valid day");
    RecurringView {
        month: format!("{:04}-{:02}", until.year(), until.month()),
        since: month_of(first),
        until,
        in_progress: current <= last,
        monthly_total_cents: items.iter().map(|r| r.monthly_cents).sum(),
        items,
        ignored: ign,
    }
}

/// The most frequent value, ties going to the one seen last.
fn most_common<'a>(vals: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut counts: HashMap<&str, (usize, usize)> = HashMap::new();
    for (i, v) in vals.enumerate() {
        let e = counts.entry(v).or_insert((0, 0));
        e.0 += 1;
        e.1 = i;
    }
    counts.into_iter().max_by_key(|(_, c)| *c).map(|(v, _)| v)
}

fn merchant_row(
    key: &str,
    items: &[&Transaction],
    cadence: StreamCadence,
    med_gap: f64,
    last: i64,
    current: i64,
    earliest: Option<i64>,
) -> RecurringMerchant {
    let mut by_month: BTreeMap<i64, i64> = BTreeMap::new();
    for t in items {
        *by_month.entry(month_index(t.effective_date())).or_insert(0) += t.amount.cents().abs();
    }
    let spent = |m: i64| by_month.get(&m).copied().unwrap_or(0);
    let start = *by_month.keys().next().expect("at least one charge");

    // Completed months from the first charge on; gaps count as $0.
    let done_through = last.min(current - 1);
    let completed: Vec<i64> = (start..=done_through).map(spent).collect();

    let mut amounts: Vec<i64> = items.iter().map(|t| t.amount.cents().abs()).collect();
    amounts.sort_unstable();
    let median_cents = median(&amounts);

    let monthly_cents = if completed.is_empty() {
        // Nothing complete yet: median charge times how often it comes.
        (median_cents as f64 * (MONTH_DAYS / med_gap.max(0.5)).min(31.0)).round() as i64
    } else {
        completed.iter().sum::<i64>() / completed.len() as i64
    };

    let change_pct = match completed.split_last() {
        Some((&latest, prior)) if !prior.is_empty() => {
            let prior = &prior[prior.len().saturating_sub(3)..];
            let base = prior.iter().sum::<i64>() as f64 / prior.len() as f64;
            (base > 0.0).then(|| (latest as f64 - base) / base * 100.0)
        }
        _ => None,
    };
    // New only when there is history before it to be new against.
    let is_new = start > last - NEW_MONTHS && earliest.is_some_and(|e| e < start);
    let change = if is_new {
        Change::New
    } else {
        match change_pct {
            Some(p) if p > CHANGE_PCT => Change::Up,
            Some(p) if p < -CHANGE_PCT => Change::Down,
            _ => Change::Steady,
        }
    };

    RecurringMerchant {
        merchant: key.to_string(),
        name: most_common(items.iter().map(|t| t.merchant()))
            .unwrap_or(key)
            .to_string(),
        cadence,
        count: items.len(),
        median_cents,
        monthly_cents,
        change,
        change_pct,
        first_seen: items.first().map(|t| t.effective_date()).unwrap_or(0),
        last_seen: items.last().map(|t| t.effective_date()).unwrap_or(0),
        spark_cents: (last - SPARK_MONTHS + 1..=last).map(spent).collect(),
        category: most_common(items.iter().filter_map(|t| t.category.as_deref()))
            .map(str::to_string),
        account_id: most_common(items.iter().map(|t| t.account_id.as_str()))
            .unwrap_or_default()
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TxnFlag;
    use crate::money::Money;
    use chrono::{Local, TimeZone};

    /// Noon local time on a date, so month bucketing is unambiguous.
    fn at(y: i32, m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp()
    }

    fn charge(id: &str, when: i64, cents: i64, merchant: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "card".into(),
            posted: when,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: Some("Streaming".into()),
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            pfc_primary: None,
            pfc_detailed: None,
            source: crate::model::default_source(),
        }
    }

    /// One charge a month on the 10th, `months` = [(year, month, cents)].
    fn monthly(merchant: &str, months: &[(i32, u32, i64)]) -> Vec<Transaction> {
        months
            .iter()
            .enumerate()
            .map(|(i, (y, m, c))| charge(&format!("{merchant}{i}"), at(*y, *m, 10), -c, merchant))
            .collect()
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn steady_monthly_charge_reports_rate_window_and_spark() {
        let txns = monthly(
            "Streamflix",
            &[
                (2026, 5, 1599),
                (2026, 6, 1599),
                (2026, 7, 1599),
                (2026, 8, 1599),
                (2026, 9, 1599),
                (2026, 10, 1599),
            ],
        );
        // October chosen, viewed in November: October is complete.
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        assert_eq!(v.month, "2026-10");
        assert_eq!(v.since, day(2025, 11, 1));
        assert_eq!(v.until, day(2026, 10, 31));
        assert!(!v.in_progress);
        assert_eq!(v.items.len(), 1);
        let r = &v.items[0];
        assert_eq!(r.merchant, "streamflix");
        assert_eq!(r.name, "Streamflix");
        assert_eq!(r.cadence, StreamCadence::Monthly);
        assert_eq!(r.monthly_cents, 1599);
        assert_eq!(r.change, Change::Steady);
        assert_eq!(r.spark_cents, vec![1599; 6]);
        assert_eq!(r.account_id, "card");
        assert_eq!(r.category.as_deref(), Some("Streaming"));
        assert_eq!(v.monthly_total_cents, 1599);
    }

    #[test]
    fn in_progress_month_is_drawn_but_not_counted() {
        let txns = monthly(
            "Streamflix",
            &[
                (2026, 7, 1000),
                (2026, 8, 1000),
                (2026, 9, 1000),
                (2026, 10, 1000),
                (2026, 11, 9000),
            ],
        );
        let v = build(&txns, &HashSet::new(), day(2026, 11, 30), at(2026, 11, 20));
        assert!(v.in_progress);
        let r = &v.items[0];
        // November's $90 is in the spark, not in the rate or the change.
        assert_eq!(r.monthly_cents, 1000);
        assert_eq!(r.change, Change::Steady);
        assert_eq!(*r.spark_cents.last().unwrap(), 9000);
    }

    #[test]
    fn a_price_rise_over_25_percent_is_up_and_a_drop_is_down() {
        let mut txns = monthly(
            "Streamflix",
            &[
                (2026, 6, 1000),
                (2026, 7, 1000),
                (2026, 8, 1000),
                (2026, 9, 1000),
                (2026, 10, 1300),
            ],
        );
        txns.extend(monthly(
            "Gym Co",
            &[
                (2026, 6, 4000),
                (2026, 7, 4000),
                (2026, 8, 4000),
                (2026, 9, 4000),
                (2026, 10, 2000),
            ],
        ));
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        let find = |k: &str| v.items.iter().find(|r| r.merchant == k).unwrap();
        assert_eq!(find("streamflix").change, Change::Up);
        assert_eq!(find("streamflix").change_pct, Some(30.0));
        assert_eq!(find("gym co").change, Change::Down);
        assert_eq!(find("gym co").change_pct, Some(-50.0));
    }

    #[test]
    fn a_merchant_that_started_recently_is_new_only_against_older_history() {
        let mut txns = monthly(
            "Streamflix",
            &[(2026, 8, 999), (2026, 9, 999), (2026, 10, 999)],
        );
        let now = at(2026, 11, 7);
        // No older history: nothing to be new against.
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), now);
        assert_eq!(v.items[0].change, Change::Steady);
        // An older unrelated charge makes it New.
        txns.push(charge("old", at(2026, 1, 3), -500, "Corner Store"));
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), now);
        assert_eq!(v.items[0].change, Change::New);
    }

    #[test]
    fn window_is_twelve_months_ending_at_the_chosen_month() {
        let txns = monthly(
            "Streamflix",
            &[
                (2025, 1, 500),
                (2025, 2, 500),
                (2025, 3, 500),
                (2026, 9, 700),
                (2026, 10, 700),
            ],
        );
        // As of 2025-03 the 2025 charges are detected.
        let v = build(&txns, &HashSet::new(), day(2025, 3, 31), at(2026, 11, 7));
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].monthly_cents, 500);
        // As of 2026-10 only two charges are in the window: not recurring.
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        assert!(v.items.is_empty());
    }

    #[test]
    fn ignored_merchants_move_to_their_own_list() {
        let txns = monthly(
            "Streamflix",
            &[(2026, 8, 999), (2026, 9, 999), (2026, 10, 999)],
        );
        let ignored: HashSet<String> = ["streamflix".to_string()].into();
        let v = build(&txns, &ignored, day(2026, 10, 31), at(2026, 11, 7));
        assert!(v.items.is_empty());
        assert_eq!(v.ignored.len(), 1);
        assert_eq!(v.monthly_total_cents, 0);
    }

    #[test]
    fn moved_money_and_refunds_are_not_recurring() {
        let mut txns = monthly(
            "Savings Sweep",
            &[(2026, 8, 5000), (2026, 9, 5000), (2026, 10, 5000)],
        );
        for t in &mut txns {
            t.flag = TxnFlag::Save;
        }
        txns.extend(
            [(2026, 8), (2026, 9), (2026, 10)]
                .iter()
                .map(|(y, m)| charge(&format!("r{m}"), at(*y, *m, 10), 1500, "Refund Shop")),
        );
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        assert!(v.items.is_empty());
    }

    #[test]
    fn store_round_trip_through_ignored_mark() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&monthly(
            "Streamflix",
            &[(2026, 8, 999), (2026, 9, 999), (2026, 10, 999)],
        ))
        .unwrap();
        let through = day(2026, 10, 31);
        let now = at(2026, 11, 7);
        assert_eq!(recurring(&s, through, now).unwrap().items.len(), 1);
        s.set_merchant_ignored("streamflix", true).unwrap();
        let v = recurring(&s, through, now).unwrap();
        assert!(v.items.is_empty());
        assert_eq!(v.ignored[0].merchant, "streamflix");
        s.set_merchant_ignored("streamflix", false).unwrap();
        assert_eq!(recurring(&s, through, now).unwrap().items.len(), 1);
    }
}
