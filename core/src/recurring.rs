//! The recurring list (GLOSSARY: Recurring). One row per recurring merchant
//! over the 12 calendar months ending at a chosen month: how often, the
//! monthly cost, a six-month spark and a change marker. Detection is
//! `subscriptions::detect_rhythms`; the one per-merchant mark is
//! Ignored, which moves a merchant to a separate list.
//!
//! [`steady_charge`] and [`price_rise`] read a merchant's charges for the
//! subscription Findings ([`findings`]) and the Flow Subscriptions filter.

use crate::find::{self, Evidence, Finding, FindingKind};
use crate::model::{mask_suffix, Account, Transaction};
use crate::query::month_index;
use crate::store::Store;
use crate::subscriptions::{
    detect_rhythms, display_merchant, median, normalize_payee, StreamCadence, MONTH_DAYS,
};
use chrono::{Datelike, Local, NaiveDate, TimeZone};
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
/// mean of the (up to three) completed months before it, or, for a merchant
/// slower than monthly, the latest charge from the median of the (up to
/// `CHARGE_BASE`) charges before it. Shared with the "unusual" rule in
/// GLOSSARY.
pub const CHANGE_PCT: f64 = 25.0;
/// A slower-than-monthly merchant's latest charge is compared with the
/// median of up to this many charges before it (at least 2).
pub const CHARGE_BASE: usize = 4;
/// Late: no charge for this many times the usual gap...
pub const LATE_FACTOR: f64 = 1.5;
/// ...and never sooner than this many days, so a skipped week of coffee
/// isn't late.
pub const LATE_MIN_DAYS: i64 = 31;
/// Charges in a slower merchant's spark.
pub const CHARGE_SPARK: usize = 6;

/// Steady: charges within this % of their median (GLOSSARY: Steady). Also
/// the size of move that makes a price rise.
pub const STEADY_PCT: f64 = 5.0;
/// How many of the latest charges the steady test reads (it needs at least
/// two). A price rise lasts until this many charges are at the new price,
/// when the merchant is steady again.
pub const STEADY_CHARGES: usize = 3;

const DAY: i64 = 86_400;

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
    /// No charge for `LATE_FACTOR` times the usual gap (at least
    /// `LATE_MIN_DAYS`): skipped, moved or stopped. Never "down 100%".
    Late,
    Steady,
}

/// One charge (a day's charges at a merchant, summed: a visit plus its tip
/// is one charge).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Charge {
    /// Behavioral date, epoch seconds (the day's first charge).
    pub at: i64,
    /// Positive = spent.
    pub cents: i64,
}

/// A steady subscription's price went up (GLOSSARY: Steady): the charges
/// before were steady at `from_cents` and the latest ones are more than
/// `STEADY_PCT` above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PriceRise {
    /// The steady charge before the rise (median of up to `STEADY_CHARGES`).
    pub from_cents: i64,
    /// The charge now (median of the charges at the new price).
    pub to_cents: i64,
    /// The first charge at the new price, epoch seconds.
    pub since: i64,
}

/// Monthly or yearly: the cadences a subscription has.
pub fn is_subscription_cadence(cadence: StreamCadence) -> bool {
    matches!(cadence, StreamCadence::Monthly | StreamCadence::Yearly)
}

fn within(cents: i64, base: i64) -> bool {
    ((cents - base).abs() as f64) <= base as f64 * STEADY_PCT / 100.0
}

/// The median of `charges` when there are at least two and every one is
/// within `STEADY_PCT` of it.
fn steady_median(charges: &[Charge]) -> Option<i64> {
    if charges.len() < 2 {
        return None;
    }
    let mut v: Vec<i64> = charges.iter().map(|c| c.cents).collect();
    v.sort_unstable();
    let m = median(&v);
    (m > 0 && v.iter().all(|c| within(*c, m))).then_some(m)
}

/// GLOSSARY: Steady. The usual charge of a merchant on a monthly or yearly
/// cadence whose last `STEADY_CHARGES` charges (at least two) are within
/// `STEADY_PCT` of their median; None otherwise. `charges` are one per day,
/// oldest first, as on [`RecurringMerchant`].
pub fn steady_charge(cadence: StreamCadence, charges: &[Charge]) -> Option<i64> {
    if !is_subscription_cadence(cadence) {
        return None;
    }
    steady_median(&charges[charges.len().saturating_sub(STEADY_CHARGES)..])
}

/// A monthly or yearly merchant whose latest charges (fewer than
/// `STEADY_CHARGES` of them, each within `STEADY_PCT` of the latest) sit
/// more than `STEADY_PCT` above a steady run before them. Once
/// `STEADY_CHARGES` charges are at the new price it is steady instead.
pub fn price_rise(cadence: StreamCadence, charges: &[Charge]) -> Option<PriceRise> {
    if !is_subscription_cadence(cadence) {
        return None;
    }
    let latest = charges.last()?;
    let run = charges
        .iter()
        .rev()
        .take_while(|c| within(c.cents, latest.cents))
        .count();
    if run >= STEADY_CHARGES {
        return None;
    }
    let (before, now) = charges.split_at(charges.len() - run);
    let from = steady_median(&before[before.len().saturating_sub(STEADY_CHARGES)..])?;
    let mut new: Vec<i64> = now.iter().map(|c| c.cents).collect();
    new.sort_unstable();
    let to = median(&new);
    ((to as f64) > from as f64 * (1.0 + STEADY_PCT / 100.0)).then_some(PriceRise {
        from_cents: from,
        to_cents: to,
        since: now[0].at,
    })
}

/// One recurring merchant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct RecurringMerchant {
    /// Normalized merchant (`normalize_payee`): the key for `merchant=` on
    /// `/transactions` and for the Ignored mark.
    pub merchant: String,
    /// Display name (`display_merchant`) of the spelling the bank uses most
    /// often: `SQ *BLUE BOTTLE 0042` shows as `Blue Bottle`.
    pub name: String,
    pub cadence: StreamCadence,
    /// Median days between charges ("every ~2 months" is drawn from it).
    pub gap_days: i64,
    /// Charges in the window.
    pub count: usize,
    /// Median charge, positive = money spent.
    pub median_cents: i64,
    /// Usual monthly cost: mean of the completed months since the first
    /// charge in the window (months with no charge count as $0). Positive.
    pub monthly_cents: i64,
    pub change: Change,
    /// Signed % against `change_base_cents`. Monthly or faster: the last
    /// completed month (None when it had no charge, or with fewer than two
    /// completed months). Slower (`cadence.is_slow()`): the latest charge.
    pub change_pct: Option<f64>,
    /// What `change_pct` compares with. Monthly or faster: the mean of up to
    /// 3 completed months before the last one. Slower: the median of up to
    /// `CHARGE_BASE` charges before the latest. Positive.
    pub change_base_cents: Option<i64>,
    /// When it became late (last charge + the allowance), epoch seconds;
    /// set only when late as of the view (the chosen month's end, or now).
    pub late_since: Option<i64>,
    /// Spent in the 12 completed months ending with the chosen month (or the
    /// month before it, while it's in progress). Positive.
    pub year_cents: i64,
    /// The same over the 12 months before those; None when the archive
    /// doesn't reach back that far.
    pub prior_year_cents: Option<i64>,
    /// Slower merchants only (else empty): the last `CHARGE_SPARK` charges
    /// up to the chosen month's end, oldest first.
    pub recent_charges: Vec<Charge>,
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
    /// GLOSSARY: Steady. The usual charge when steady ([`steady_charge`]),
    /// else None. The Subscriptions filter is `steady_cents != null`.
    pub steady_cents: Option<i64>,
    /// Set while a steady price has just gone up ([`price_rise`]).
    pub price_rise: Option<PriceRise>,
}

impl RecurringMerchant {
    /// GLOSSARY: Steady.
    pub fn is_steady(&self) -> bool {
        self.steady_cents.is_some()
    }
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

pub(crate) fn build(
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
    // The view is as of the chosen month's end, or now while it runs.
    let view_end = month_start(last + 1).min(now);

    let rhythms = detect_rhythms(&window);
    // Every charge each detected merchant ever made (the archive outgrows
    // the window): the year-on-year totals and the charge-to-charge rules
    // read these.
    let wanted: HashSet<&str> = rhythms.iter().map(|g| g.merchant.as_str()).collect();
    let mut history: HashMap<String, Vec<&Transaction>> = HashMap::new();
    for t in all
        .iter()
        .filter(|t| t.amount.is_outflow() && t.flag == crate::model::TxnFlag::Spending)
    {
        let key = normalize_payee(t.merchant());
        if wanted.contains(key.as_str()) {
            history.entry(key).or_default().push(t);
        }
    }

    let mut items = Vec::new();
    let mut ign = Vec::new();
    for g in rhythms {
        let charges = charges_of(history.get(&g.merchant).map(Vec::as_slice).unwrap_or(&[]));
        let row = merchant_row(
            &g.merchant,
            &g.items,
            g.cadence,
            g.med_gap,
            Span {
                last,
                current,
                earliest,
                view_end,
            },
            &charges,
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

/// Local midnight starting month `idx`, epoch seconds.
fn month_start(idx: i64) -> i64 {
    let d = month_of(idx).and_hms_opt(0, 0, 0).expect("midnight");
    Local
        .from_local_datetime(&d)
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or_else(|| d.and_utc().timestamp())
}

/// A merchant's charges, one per local day (summed), oldest first.
fn charges_of(txns: &[&Transaction]) -> Vec<Charge> {
    let mut by_day: BTreeMap<NaiveDate, Charge> = BTreeMap::new();
    for t in txns {
        let at = t.effective_date();
        let day = Local
            .timestamp_opt(at, 0)
            .single()
            .map(|d| d.date_naive())
            .unwrap_or_default();
        let c = by_day.entry(day).or_insert(Charge { at, cents: 0 });
        c.at = c.at.min(at);
        c.cents += t.amount.cents().abs();
    }
    by_day.into_values().collect()
}

/// Where the view sits: the chosen month, the month containing now, the
/// archive's first month, and the moment the view is as of.
struct Span {
    last: i64,
    current: i64,
    earliest: Option<i64>,
    view_end: i64,
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

fn pct(latest: i64, base: f64) -> Option<f64> {
    (base > 0.0).then(|| (latest as f64 - base) / base * 100.0)
}

fn merchant_row(
    key: &str,
    items: &[&Transaction],
    cadence: StreamCadence,
    med_gap: f64,
    span: Span,
    history: &[Charge],
) -> RecurringMerchant {
    let Span {
        last,
        current,
        earliest,
        view_end,
    } = span;
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

    // Charges up to the view's end (the archive may hold later ones).
    let seen: Vec<Charge> = history
        .iter()
        .copied()
        .filter(|c| month_index(c.at) <= last)
        .collect();

    let (change_pct, change_base_cents) = if cadence.is_slow() {
        // Charge to charge: some months have no charge by design.
        match seen.split_last() {
            Some((latest, before)) if before.len() >= 2 => {
                let mut base: Vec<i64> = before[before.len().saturating_sub(CHARGE_BASE)..]
                    .iter()
                    .map(|c| c.cents)
                    .collect();
                base.sort_unstable();
                let base = median(&base);
                (pct(latest.cents, base as f64), Some(base))
            }
            _ => (None, None),
        }
    } else {
        match completed.split_last() {
            // A month with no charge isn't a price change: it's late, or a
            // charge that moved across the month line.
            Some((&latest, prior)) if !prior.is_empty() && latest > 0 => {
                let prior = &prior[prior.len().saturating_sub(3)..];
                let base = prior.iter().sum::<i64>() as f64 / prior.len() as f64;
                (pct(latest, base), Some(base.round() as i64))
            }
            _ => (None, None),
        }
    };

    let last_seen = items.last().map(|t| t.effective_date()).unwrap_or(0);
    let allowance = ((med_gap * LATE_FACTOR).round() as i64).max(LATE_MIN_DAYS);
    let late_since = Some(last_seen + allowance * DAY).filter(|at| *at <= view_end);

    // New only when there is history before it to be new against.
    let is_new = start > last - NEW_MONTHS && earliest.is_some_and(|e| e < start);
    let change = if is_new {
        Change::New
    } else if late_since.is_some() {
        Change::Late
    } else {
        match change_pct {
            Some(p) if p > CHANGE_PCT => Change::Up,
            Some(p) if p < -CHANGE_PCT => Change::Down,
            _ => Change::Steady,
        }
    };

    // Year on year, over completed months.
    let year_end = done_through;
    let in_months = |from: i64, to: i64| -> i64 {
        history
            .iter()
            .filter(|c| (from..=to).contains(&month_index(c.at)))
            .map(|c| c.cents)
            .sum()
    };
    let year_cents = in_months(year_end - 11, year_end);
    let prior_year_cents = earliest
        .filter(|e| *e <= year_end - 23)
        .map(|_| in_months(year_end - 23, year_end - 12));

    RecurringMerchant {
        merchant: key.to_string(),
        name: display_merchant(most_common(items.iter().map(|t| t.merchant())).unwrap_or(key)),
        cadence,
        gap_days: med_gap.round() as i64,
        count: items.len(),
        median_cents,
        monthly_cents,
        change,
        change_pct,
        change_base_cents,
        late_since,
        year_cents,
        prior_year_cents,
        recent_charges: if cadence.is_slow() {
            seen[seen.len().saturating_sub(CHARGE_SPARK)..].to_vec()
        } else {
            Vec::new()
        },
        first_seen: items.first().map(|t| t.effective_date()).unwrap_or(0),
        last_seen,
        spark_cents: (last - SPARK_MONTHS + 1..=last).map(spent).collect(),
        category: most_common(items.iter().filter_map(|t| t.category.as_deref()))
            .map(str::to_string),
        account_id: most_common(items.iter().map(|t| t.account_id.as_str()))
            .unwrap_or_default()
            .to_string(),
        steady_cents: steady_charge(cadence, &seen),
        price_rise: price_rise(cadence, &seen),
    }
}

/// Charges a year at a subscription cadence.
fn per_year(cadence: StreamCadence) -> i64 {
    if cadence == StreamCadence::Yearly {
        1
    } else {
        12
    }
}

/// The subscription Findings (GLOSSARY: Finding): a price rise for each
/// merchant whose price just went up, else a subscription to review for
/// each steady one, valued at a year of the charge (the rise, for a price
/// rise). outflow can't see usage, so the action never says unused. Late
/// merchants (stopped, as far as the archive shows) and Ignored ones (not in
/// `view.items`) give none. `txns` is the archive the view was built from;
/// `window` (epoch seconds, end exclusive) bounds the `txn_ids`.
pub(crate) fn findings(
    view: &RecurringView,
    accounts: &[Account],
    txns: &[Transaction],
    window: (i64, i64),
) -> Vec<Finding> {
    let (from, to) = window;
    let mut out = Vec::new();
    for r in view.items.iter().filter(|r| r.change != Change::Late) {
        let (kind, charge_cents, est) = match (r.price_rise, r.steady_cents) {
            (Some(p), _) => (
                FindingKind::PriceRise,
                p.to_cents,
                (p.to_cents - p.from_cents) * per_year(r.cadence),
            ),
            (None, Some(c)) => (FindingKind::Subscription, c, c * per_year(r.cadence)),
            (None, None) => continue,
        };
        let since = match r.price_rise {
            Some(p) => p.since,
            None => from,
        };
        let txn_ids = txns
            .iter()
            .filter(|t| {
                t.amount.is_outflow()
                    && t.flag == crate::model::TxnFlag::Spending
                    && t.effective_date() >= since
                    && t.effective_date() < to
                    && normalize_payee(t.merchant()) == r.merchant
            })
            .map(|t| t.id.clone())
            .collect();
        let each = if r.cadence == StreamCadence::Yearly {
            "a year"
        } else {
            "a month"
        };
        let money = |c: i64| format!("${}", crate::money::Money::from_cents(c).to_display());
        let action = match r.price_rise {
            Some(p) => format!(
                "{} went up from {} to {} {each}: check the plan is still worth it",
                r.name,
                money(p.from_cents),
                money(p.to_cents)
            ),
            None => format!(
                "Review {} at {} {each}: still worth it?",
                r.name,
                money(charge_cents)
            ),
        };
        let card_mask = accounts
            .iter()
            .find(|a| a.id == r.account_id)
            .and_then(|a| mask_suffix(&a.name))
            .map(|m| m.trim_matches(['(', ')']).to_string());
        out.push(Finding {
            kind,
            key: find::key(kind, "", &r.merchant),
            subject: r.name.clone(),
            card_mask,
            est_cents_per_year: est,
            est_points_per_year: 0,
            est_value_cents_per_year: est,
            txn_ids,
            action,
            evidence: Evidence::Subscription(find::SubscriptionEvidence {
                merchant: r.merchant.clone(),
                name: r.name.clone(),
                cadence: r.cadence,
                charge_cents,
                per_year: per_year(r.cadence),
                price_rise: r.price_rise,
                last_charge: r.last_seen,
                account_id: r.account_id.clone(),
                reviewed_at: None,
            }),
        });
    }
    out
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
    fn name_is_the_display_name_key_stays_normalized() {
        let txns = monthly(
            "SQ *DEMO GYM 0042",
            &[(2026, 7, 4000), (2026, 8, 4000), (2026, 9, 4000)],
        );
        let v = build(&txns, &HashSet::new(), day(2026, 9, 30), at(2026, 11, 7));
        assert_eq!(v.items[0].merchant, "demo gym");
        assert_eq!(v.items[0].name, "Demo Gym");
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
        // The % is against the 3 months before, not the whole-window usual.
        assert_eq!(find("streamflix").change_base_cents, Some(1000));
        assert_eq!(find("streamflix").monthly_cents, 1060);
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

    // ── cadence-aware rules (ho-2mj.10) ─────────────────────────────────

    /// A groomer every two months on the 10th: [(year, month, cents)].
    fn groomer(visits: &[(i32, u32, i64)]) -> Vec<Transaction> {
        monthly("Sample Groomer", visits)
    }

    #[test]
    fn a_two_monthly_merchant_with_no_charge_this_month_is_not_down() {
        // Charged in Sep; none in Oct by design.
        let txns = groomer(&[
            (2026, 1, 8000),
            (2026, 3, 8000),
            (2026, 5, 8000),
            (2026, 7, 8000),
            (2026, 9, 8000),
        ]);
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        let r = &v.items[0];
        assert_eq!(r.cadence, StreamCadence::EveryFewMonths);
        assert_eq!(r.change, Change::Steady);
        assert_eq!(r.change_pct, Some(0.0));
        assert_eq!(r.late_since, None);
        assert!((59..=62).contains(&r.gap_days));
        assert_eq!(r.recent_charges.len(), 5);
        assert!(r.recent_charges.iter().all(|c| c.cents == 8000));
    }

    #[test]
    fn a_dearer_visit_is_up_against_the_usual_charge() {
        let txns = groomer(&[
            (2026, 1, 8000),
            (2026, 3, 8000),
            (2026, 5, 8200),
            (2026, 7, 7800),
            (2026, 9, 10_500),
        ]);
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        let r = &v.items[0];
        assert_eq!(r.change, Change::Up);
        // Median of the 4 charges before: 8000 and 8000, 7800, 8200 -> 8000.
        assert_eq!(r.change_base_cents, Some(8000));
        assert_eq!(r.change_pct, Some(31.25));
    }

    #[test]
    fn the_same_day_counts_as_one_charge() {
        let mut txns = groomer(&[
            (2026, 1, 8000),
            (2026, 3, 8000),
            (2026, 5, 8000),
            (2026, 7, 8000),
        ]);
        // A tip on the July visit.
        txns.push(charge("tip", at(2026, 7, 10) + 60, -1500, "Sample Groomer"));
        let v = build(&txns, &HashSet::new(), day(2026, 7, 31), at(2026, 8, 7));
        let r = &v.items[0];
        assert_eq!(r.recent_charges.last().unwrap().cents, 9500);
        assert_eq!(r.change, Change::Steady); // +18.75%
    }

    #[test]
    fn a_quiet_merchant_is_late_not_down_100() {
        // Two-monthly, last charge May 10: late after ~91 days (Aug 9).
        let txns = groomer(&[(2026, 1, 8000), (2026, 3, 8000), (2026, 5, 8000)]);
        let v = build(&txns, &HashSet::new(), day(2026, 7, 31), at(2026, 11, 7));
        assert_eq!(
            v.items[0].change,
            Change::Steady,
            "not late yet at July's end"
        );
        let v = build(&txns, &HashSet::new(), day(2026, 8, 31), at(2026, 11, 7));
        let r = &v.items[0];
        assert_eq!(r.change, Change::Late);
        let since = r.late_since.unwrap();
        assert!(since > at(2026, 8, 1) && since < at(2026, 8, 15));

        // A monthly subscription that stopped: late, and never "down 100%".
        let txns = monthly(
            "Streamflix",
            &[
                (2026, 4, 1599),
                (2026, 5, 1599),
                (2026, 6, 1599),
                (2026, 7, 1599),
                (2026, 8, 1599),
            ],
        );
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        let r = &v.items[0];
        assert_eq!(r.change, Change::Late);
        assert_eq!(r.change_pct, None);
    }

    #[test]
    fn a_skipped_week_of_a_weekly_habit_is_not_late() {
        let txns: Vec<_> = (0..12)
            .map(|w| {
                charge(
                    &format!("w{w}"),
                    at(2026, 8, 3) + w * 7 * DAY,
                    -1200,
                    "Sample Class",
                )
            })
            .collect();
        // Last class Oct 19; viewed Nov 7: 19 days, under a month.
        let v = build(&txns, &HashSet::new(), day(2026, 11, 7), at(2026, 11, 7));
        assert_eq!(v.items[0].late_since, None);
    }

    #[test]
    fn year_on_year_needs_two_years_of_archive() {
        let months: Vec<(i32, u32, i64)> = (0..26)
            .map(|i| {
                let idx = 2024 * 12 + 8 + i; // Sep 2024 ..
                (
                    idx / 12,
                    (idx % 12 + 1) as u32,
                    if i < 14 { 1000 } else { 1200 },
                )
            })
            .collect();
        let txns = monthly("Streamflix", &months);
        // October 2026 complete: last 12 = Nov 2025..Oct 2026, prior = the 12 before.
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        let r = &v.items[0];
        // Sep 2024..Oct 2025 at 1000, then 1200 from Nov 2025.
        assert_eq!(r.year_cents, 12 * 1200);
        assert_eq!(r.prior_year_cents, Some(12 * 1000));
        // An archive that starts later: no prior year.
        let short = monthly("Streamflix", &months[14..]);
        let v = build(&short, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        assert_eq!(v.items[0].prior_year_cents, None);
    }

    // ── steady and price rise (ho-2mj.17) ───────────────────────────────

    fn charges(cents: &[i64]) -> Vec<Charge> {
        cents
            .iter()
            .enumerate()
            .map(|(i, c)| Charge {
                at: i as i64 * 30 * DAY,
                cents: *c,
            })
            .collect()
    }

    #[test]
    fn steady_is_monthly_or_yearly_with_recent_charges_within_5_percent() {
        let m = StreamCadence::Monthly;
        assert_eq!(steady_charge(m, &charges(&[999, 999, 999])), Some(999));
        // Only the last three count: an old odd charge doesn't matter.
        assert_eq!(
            steady_charge(m, &charges(&[4000, 1000, 1040, 1000])),
            Some(1000)
        );
        // 1060 is 6% off the median 1000.
        assert_eq!(steady_charge(m, &charges(&[1000, 1000, 1060])), None);
        assert_eq!(steady_charge(m, &charges(&[1000])), None);
        assert_eq!(
            steady_charge(StreamCadence::Yearly, &charges(&[9900, 9900])),
            Some(9900)
        );
        // A groomer or a coffee habit is never a subscription.
        assert_eq!(
            steady_charge(StreamCadence::EveryFewMonths, &charges(&[8000; 4])),
            None
        );
        assert_eq!(
            steady_charge(StreamCadence::Weekly, &charges(&[500; 4])),
            None
        );
    }

    #[test]
    fn a_price_rise_lasts_until_the_new_price_is_steady() {
        let m = StreamCadence::Monthly;
        let rise = price_rise(m, &charges(&[1549, 1549, 1549, 1799])).unwrap();
        assert_eq!((rise.from_cents, rise.to_cents), (1549, 1799));
        assert_eq!(rise.since, 3 * 30 * DAY);
        let rise = price_rise(m, &charges(&[1549, 1549, 1549, 1799, 1799])).unwrap();
        assert_eq!(rise.since, 3 * 30 * DAY);
        // Three charges at the new price: steady again, no longer a rise.
        let three = charges(&[1549, 1549, 1549, 1799, 1799, 1799]);
        assert_eq!(price_rise(m, &three), None);
        assert_eq!(steady_charge(m, &three), Some(1799));
        // Under 5% is noise; a drop isn't a rise; unsteady before isn't one.
        assert_eq!(price_rise(m, &charges(&[1000, 1000, 1000, 1040])), None);
        assert_eq!(price_rise(m, &charges(&[1000, 1000, 1000, 800])), None);
        assert_eq!(price_rise(m, &charges(&[700, 1000, 1300, 2000])), None);
        assert_eq!(
            price_rise(StreamCadence::EveryFewMonths, &charges(&[8000, 8000, 9000])),
            None
        );
    }

    #[test]
    fn rows_carry_steady_and_price_rise() {
        let mut txns = monthly(
            "Streamflix",
            &[
                (2026, 7, 1549),
                (2026, 8, 1549),
                (2026, 9, 1549),
                (2026, 10, 1799),
            ],
        );
        txns.extend(monthly(
            "Demo Power",
            &[
                (2026, 7, 6000),
                (2026, 8, 9000),
                (2026, 9, 7000),
                (2026, 10, 8000),
            ],
        ));
        let v = build(&txns, &HashSet::new(), day(2026, 10, 31), at(2026, 11, 7));
        let row = |k: &str| v.items.iter().find(|r| r.merchant == k).unwrap();
        assert!(!row("streamflix").is_steady());
        assert_eq!(row("streamflix").price_rise.unwrap().to_cents, 1799);
        // A utility bill that moves around is recurring, not steady.
        assert!(!row("demo power").is_steady());
        assert_eq!(row("demo power").price_rise, None);
    }

    fn findings_of(txns: &[Transaction], ignored: &HashSet<String>) -> Vec<Finding> {
        let through = day(2026, 10, 31);
        let v = build(txns, ignored, through, at(2026, 11, 7));
        findings(&v, &[], txns, find::window(through).bounds())
    }

    #[test]
    fn steady_merchants_are_review_findings_and_rises_are_their_own() {
        let mut txns = monthly(
            "Streamflix",
            &[
                (2026, 7, 1599),
                (2026, 8, 1599),
                (2026, 9, 1599),
                (2026, 10, 1599),
            ],
        );
        txns.extend(monthly(
            "Sample Music",
            &[
                (2026, 7, 1099),
                (2026, 8, 1099),
                (2026, 9, 1099),
                (2026, 10, 1299),
            ],
        ));
        let f = findings_of(&txns, &HashSet::new());
        let by = |k: &str| f.iter().find(|f| f.key == k).unwrap();
        let sub = by("subscription::streamflix");
        assert_eq!(sub.kind, FindingKind::Subscription);
        assert_eq!(sub.est_cents_per_year, 12 * 1599);
        assert_eq!(sub.est_value_cents_per_year, 12 * 1599);
        assert_eq!(sub.txn_ids.len(), 4);
        assert!(!sub.action.contains("unused"), "{}", sub.action);
        let rise = by("price_rise::sample-music");
        assert_eq!(rise.kind, FindingKind::PriceRise);
        assert_eq!(rise.est_cents_per_year, 12 * 200);
        // Only the charge at the new price is behind the rise.
        assert_eq!(rise.txn_ids, vec!["Sample Music3".to_string()]);
        assert_eq!(f.len(), 2, "a rise is not also a review");
    }

    #[test]
    fn ignored_and_late_merchants_give_no_findings() {
        let txns = monthly(
            "Streamflix",
            &[
                (2026, 7, 1599),
                (2026, 8, 1599),
                (2026, 9, 1599),
                (2026, 10, 1599),
            ],
        );
        let ignored: HashSet<String> = ["streamflix".to_string()].into();
        assert!(findings_of(&txns, &ignored).is_empty());
        // Stopped in July: late by October's end.
        let late = monthly(
            "Streamflix",
            &[
                (2026, 4, 1599),
                (2026, 5, 1599),
                (2026, 6, 1599),
                (2026, 7, 1599),
            ],
        );
        assert!(findings_of(&late, &HashSet::new()).is_empty());
    }
}
