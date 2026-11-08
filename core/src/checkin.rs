//! Check-in: one period (week, month or quarter) summarized for the home
//! screen and the agent. Net worth change, earned − spent alongside saved,
//! what's usual for each, and what's unusual (GLOSSARY: Check-in, Unusual;
//! ADR 0001). Every figure comes from the same core functions Flow and
//! Holdings use, so the numbers agree across screens.
//!
//! Rules (decided 2026-11-07, ho-2mj.5):
//!
//! - **Usual** = the median of up to `USUAL_PERIODS` periods of the same kind
//!   just before this one, counting only periods that start on or after the
//!   first transaction (or, for net worth, that have a change). Needs
//!   `USUAL_MIN_PERIODS`, else None.
//! - **Recurring changes** (new merchant, monthly cost up or down by more than
//!   `recurring::CHANGE_PCT`) come from the recurring list. Up/Down is judged
//!   only on completed months: a month in progress gets new merchants only,
//!   a quarter gets each of its completed months, a week gets none.
//! - **Large charges**: an Out charge from a merchant that isn't recurring, at
//!   or above the `LARGE_CHARGE_PERCENTILE`th percentile of single Out charges
//!   in the `LARGE_CHARGE_LOOKBACK_MONTHS` months before the period. Needs
//!   `LARGE_CHARGE_MIN_HISTORY` charges; at most `LARGE_CHARGE_MAX_ITEMS`,
//!   largest first.
//! - **Attention**: a connection not "ok", card-payment matches waiting for
//!   review (`upkeep::summary`). These are about now, so they show for any
//!   period.
//!
//! A check that can't run is listed in `not_checked` with its reason, so the
//! screen says why instead of showing nothing.

use crate::model::Transaction;
use crate::period::{Period, PeriodKind};
use crate::query::{account_kinds, flow_over, lane_of, Lane};
use crate::recurring::{self, Change, RecurringView};
use crate::stock::{net_worth_change, ChangeReason, NetWorthChange};
use crate::store::Store;
use crate::subscriptions::{display_merchant, normalize_payee};
use crate::upkeep;
use chrono::{Datelike, Local, Months, NaiveDate, TimeZone};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};

/// How many earlier periods the usual is taken over.
pub const USUAL_PERIODS: usize = 12;
/// Fewer earlier periods than this: no usual.
pub const USUAL_MIN_PERIODS: usize = 3;
/// A charge at or above this percentile of your own charges is large.
pub const LARGE_CHARGE_PERCENTILE: u32 = 95;
/// The charges the percentile is taken over: this many months before the
/// period.
pub const LARGE_CHARGE_LOOKBACK_MONTHS: u32 = 12;
/// Fewer charges than this in the lookback: no large-charge check.
pub const LARGE_CHARGE_MIN_HISTORY: usize = 30;
/// At most this many large charges are listed.
pub const LARGE_CHARGE_MAX_ITEMS: usize = 5;

/// One period's check-in.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CheckIn {
    pub period: Period,
    /// The period's spelling (`2026-11`, `2026-W45`, `2026-Q4`).
    pub label: String,
    /// The period reaches today, so its figures are "so far".
    pub in_progress: bool,
    pub net_worth: NetWorthSummary,
    pub flow: FlowSummary,
    pub usual: Usual,
    /// Largest concern first: attention, then recurring changes, then large
    /// charges.
    pub unusual: Vec<Unusual>,
    /// Checks that didn't run for this period, and why.
    pub not_checked: Vec<NotChecked>,
}

/// The Holdings hero figures for the period (`/net-worth/change` has the
/// rest).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetWorthSummary {
    pub start_cents: Option<i64>,
    pub end_cents: Option<i64>,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    pub change_cents: Option<i64>,
    pub change_bp: Option<i64>,
    pub reason: Option<ChangeReason>,
    /// Markets + Added == change, exactly.
    pub markets_cents: Option<i64>,
    pub added_cents: Option<i64>,
    /// Investment accounts whose gain is unknown: their whole change is in
    /// Added.
    pub excluded: Vec<String>,
    pub partial_start: bool,
}

/// The Flow figures for the period (`/flow` has the lanes).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FlowSummary {
    pub in_cents: i64,
    /// Spending net of refunds, positive = spent.
    pub out_cents: i64,
    pub earned_minus_spent_cents: i64,
    pub saved_cents: i64,
    /// Earned − spent minus saved: cash left in checking.
    pub gap_cents: i64,
}

/// Each figure's median over the earlier periods; None without enough.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Usual {
    /// How many earlier periods the Flow medians cover.
    pub periods: usize,
    pub in_cents: Option<i64>,
    pub out_cents: Option<i64>,
    pub earned_minus_spent_cents: Option<i64>,
    pub saved_cents: Option<i64>,
    /// How many earlier periods had a net worth change.
    pub net_worth_periods: usize,
    pub net_worth_change_cents: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum UnusualKind {
    /// A connection whose status isn't ok (login required, error).
    Connection,
    /// Card-payment matches waiting for review.
    MatchReview,
    /// A recurring merchant whose first charge falls in the period.
    NewRecurring,
    /// A recurring merchant's month more than 25% above its usual.
    RecurringUp,
    RecurringDown,
    /// A single charge at or above your own 95th percentile.
    LargeCharge,
}

/// One unusual item. Which fields are set depends on `kind`; each item
/// carries what an agent or the screen needs to show the rows behind it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Unusual {
    pub kind: UnusualKind,
    /// Merchant display name (`display_merchant`), institution, or a short
    /// phrase.
    pub label: String,
    /// The charge (large charge), or the month's spend (recurring), positive.
    pub cents: Option<i64>,
    /// What it's compared with: the merchant's usual monthly cost, or the
    /// large-charge threshold.
    pub usual_cents: Option<i64>,
    /// Signed % against the usual (recurring up/down).
    pub change_pct: Option<f64>,
    /// `normalize_payee` key: `merchant=` on `/transactions`.
    pub merchant: Option<String>,
    pub txn_id: Option<String>,
    pub account_id: Option<String>,
    /// Day of the charge or first charge, `YYYY-MM-DD`.
    pub day: Option<String>,
    /// The month a recurring change was judged on, `YYYY-MM`.
    pub month: Option<String>,
    /// Connection: Plaid item and its status.
    pub item_id: Option<String>,
    pub status: Option<String>,
    /// Match review: how many wait.
    pub count: Option<usize>,
}

impl Unusual {
    fn new(kind: UnusualKind, label: impl Into<String>) -> Unusual {
        Unusual {
            kind,
            label: label.into(),
            cents: None,
            usual_cents: None,
            change_pct: None,
            merchant: None,
            txn_id: None,
            account_id: None,
            day: None,
            month: None,
            item_id: None,
            status: None,
            count: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Check {
    /// Recurring monthly cost up / down.
    RecurringChanges,
    /// New recurring merchants.
    NewRecurring,
    LargeCharges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum NotCheckedReason {
    /// Monthly cost isn't a weekly idea.
    Week,
    /// No completed month in the period yet.
    InProgress,
    /// Too few charges before the period to know what's large.
    ShortHistory,
    /// A custom or yearly range: check-in covers weeks, months, quarters.
    Range,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NotChecked {
    pub check: Check,
    pub reason: NotCheckedReason,
}

/// The check-in for `period`. `today` decides what's in progress; `now`
/// (epoch seconds) which months the recurring list treats as complete.
pub fn checkin(
    store: &Store,
    period: Period,
    today: NaiveDate,
    now: i64,
) -> rusqlite::Result<CheckIn> {
    let txns = store.all_transactions()?;
    let kind = account_kinds(store)?;
    let accounts = store.accounts()?;
    let securities = store.securities()?;
    let balances = store.balance_snapshots(None, None)?;
    let holdings = store.holding_snapshots(None, None)?;
    let activity = store.investment_txns()?;
    let ignored = store.ignored_merchants()?;

    let nw = |p: &Period| {
        let (since, until) = p.bounds();
        net_worth_change(
            &accounts,
            &securities,
            &balances,
            &holdings,
            &activity,
            Some(since),
            Some(until),
        )
    };
    let (since, until) = period.bounds();
    let flow = flow_over(&txns, &kind, since, until);
    let in_progress = period.end > today;

    // Usual: the earlier periods of the same kind.
    let first_day = txns
        .iter()
        .map(|t| t.effective_date())
        .min()
        .and_then(local_day);
    let mut earlier = Vec::new();
    let mut p = period;
    for _ in 0..USUAL_PERIODS {
        p = p.previous();
        earlier.push(p);
    }
    let covered: Vec<&Period> = earlier
        .iter()
        .filter(|p| first_day.is_some_and(|d| p.start >= d))
        .collect();
    let flows: Vec<_> = covered
        .iter()
        .map(|p| {
            let (s, u) = p.bounds();
            flow_over(&txns, &kind, s, u)
        })
        .collect();
    let nw_changes: Vec<i64> = earlier
        .iter()
        .filter_map(|p| nw(p).change.map(|m| m.cents()))
        .collect();
    let med = |v: Vec<i64>| (v.len() >= USUAL_MIN_PERIODS).then(|| median(v));
    let usual = Usual {
        periods: flows.len(),
        in_cents: med(flows.iter().map(|f| f.in_cents).collect()),
        out_cents: med(flows.iter().map(|f| f.out_cents).collect()),
        earned_minus_spent_cents: med(flows.iter().map(|f| f.earned_minus_spent_cents).collect()),
        saved_cents: med(flows.iter().map(|f| f.saved_cents).collect()),
        net_worth_periods: nw_changes.len(),
        net_worth_change_cents: med(nw_changes),
    };

    let mut unusual = attention(store)?;
    let mut not_checked = Vec::new();

    // Recurring: per month of the period that the rules allow.
    let months = months_of(&period);
    let calendar = matches!(
        period.kind,
        PeriodKind::Month | PeriodKind::Quarter | PeriodKind::Week
    );
    let mut recurring_merchants: HashSet<String> = HashSet::new();
    let view_at = |m: NaiveDate| recurring::build(&txns, &ignored, m, now);
    // The list as of the period's last month: who is recurring at all.
    let last_view = view_at(period.last_day().min(today.max(period.start)));
    for r in last_view.items.iter().chain(&last_view.ignored) {
        recurring_merchants.insert(r.merchant.clone());
    }
    match period.kind {
        PeriodKind::Week => {
            not_checked.push(NotChecked {
                check: Check::NewRecurring,
                reason: NotCheckedReason::Week,
            });
            not_checked.push(NotChecked {
                check: Check::RecurringChanges,
                reason: NotCheckedReason::Week,
            });
        }
        PeriodKind::Month | PeriodKind::Quarter => {
            unusual.extend(new_recurring(&last_view, since, until));
            let done: Vec<NaiveDate> = months
                .iter()
                .copied()
                .filter(|m| month_end(*m) <= today)
                .collect();
            if done.is_empty() {
                not_checked.push(NotChecked {
                    check: Check::RecurringChanges,
                    reason: NotCheckedReason::InProgress,
                });
            }
            // Latest month wins when a merchant is flagged in several.
            let mut changed: BTreeMap<String, Unusual> = BTreeMap::new();
            for m in done {
                for u in recurring_changes(&view_at(m), m) {
                    changed.insert(u.merchant.clone().unwrap_or_default(), u);
                }
            }
            let mut changed: Vec<Unusual> = changed.into_values().collect();
            changed.sort_by(|a, b| {
                let gap = |u: &Unusual| (u.cents.unwrap_or(0) - u.usual_cents.unwrap_or(0)).abs();
                gap(b).cmp(&gap(a))
            });
            unusual.extend(changed);
        }
        PeriodKind::Year | PeriodKind::Custom => {
            for check in [Check::NewRecurring, Check::RecurringChanges] {
                not_checked.push(NotChecked {
                    check,
                    reason: NotCheckedReason::Range,
                });
            }
        }
    }

    // Large charges.
    if calendar {
        let back = period
            .start
            .checked_sub_months(Months::new(LARGE_CHARGE_LOOKBACK_MONTHS))
            .expect("valid date");
        let (h_since, h_until) = (day_start(back), since);
        let charge = |t: &Transaction| -> Option<i64> {
            let c = t.amount.cents();
            (c < 0 && lane_of(t, kind(&t.account_id)) == Some(Lane::Out)).then_some(-c)
        };
        let mut history: Vec<i64> = txns
            .iter()
            .filter(|t| (h_since..h_until).contains(&t.effective_date()))
            .filter_map(charge)
            .collect();
        if history.len() < LARGE_CHARGE_MIN_HISTORY {
            not_checked.push(NotChecked {
                check: Check::LargeCharges,
                reason: NotCheckedReason::ShortHistory,
            });
        } else {
            let threshold = percentile(&mut history, LARGE_CHARGE_PERCENTILE);
            let mut large: Vec<(&Transaction, i64)> = txns
                .iter()
                .filter(|t| (since..until).contains(&t.effective_date()))
                .filter_map(|t| charge(t).map(|c| (t, c)))
                .filter(|(t, c)| {
                    *c >= threshold && !recurring_merchants.contains(&normalize_payee(t.merchant()))
                })
                .collect();
            large.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.id.cmp(&b.0.id)));
            unusual.extend(
                large
                    .into_iter()
                    .take(LARGE_CHARGE_MAX_ITEMS)
                    .map(|(t, c)| {
                        let mut u =
                            Unusual::new(UnusualKind::LargeCharge, display_merchant(t.merchant()));
                        u.cents = Some(c);
                        u.usual_cents = Some(threshold);
                        u.merchant = Some(normalize_payee(t.merchant()));
                        u.txn_id = Some(t.id.clone());
                        u.account_id = Some(t.account_id.clone());
                        u.day = local_day(t.effective_date()).map(|d| d.to_string());
                        u
                    }),
            );
        }
    } else {
        not_checked.push(NotChecked {
            check: Check::LargeCharges,
            reason: NotCheckedReason::Range,
        });
    }

    let change = nw(&period);
    Ok(CheckIn {
        label: period.to_string(),
        period,
        in_progress,
        net_worth: summary(&change),
        flow: FlowSummary {
            in_cents: flow.in_cents,
            out_cents: flow.out_cents,
            earned_minus_spent_cents: flow.earned_minus_spent_cents,
            saved_cents: flow.saved_cents,
            gap_cents: flow.gap_cents,
        },
        usual,
        unusual,
        not_checked,
    })
}

fn summary(c: &NetWorthChange) -> NetWorthSummary {
    let has = c.start_at.is_some();
    NetWorthSummary {
        start_cents: has.then_some(c.start.net_worth.cents()),
        end_cents: c.end_at.map(|_| c.end.net_worth.cents()),
        start_at: c.start_at,
        end_at: c.end_at,
        change_cents: c.change.map(|m| m.cents()),
        change_bp: c.change_bp,
        reason: c.reason,
        markets_cents: c.markets.map(|m| m.cents()),
        added_cents: c.added.map(|m| m.cents()),
        excluded: c.excluded.iter().map(|x| x.account.clone()).collect(),
        partial_start: c.partial_start,
    }
}

/// Connections and match reviews that need the owner now: the upkeep
/// summary (`upkeep::summary`), so the numbers match Settings and `status`.
fn attention(store: &Store) -> rusqlite::Result<Vec<Unusual>> {
    let up = upkeep::summary(store)?;
    let mut out: Vec<Unusual> = up
        .connections
        .into_iter()
        .map(|c| {
            let mut u = Unusual::new(UnusualKind::Connection, c.institution);
            u.item_id = Some(c.item_id);
            u.status = Some(c.status);
            u
        })
        .collect();
    if up.proposed_matches > 0 {
        let mut u = Unusual::new(UnusualKind::MatchReview, "Card payments to review");
        u.count = Some(up.proposed_matches);
        out.push(u);
    }
    Ok(out)
}

/// Recurring merchants whose first charge falls in `[since, until)`.
fn new_recurring(view: &RecurringView, since: i64, until: i64) -> Vec<Unusual> {
    let mut out: Vec<Unusual> = view
        .items
        .iter()
        .filter(|r| r.change == Change::New && (since..until).contains(&r.first_seen))
        .map(|r| {
            let mut u = Unusual::new(UnusualKind::NewRecurring, r.name.clone());
            u.cents = Some(r.median_cents);
            u.merchant = Some(r.merchant.clone());
            u.account_id = Some(r.account_id.clone());
            u.day = local_day(r.first_seen).map(|d| d.to_string());
            u
        })
        .collect();
    out.sort_by_key(|u| std::cmp::Reverse(u.cents));
    out
}

/// Up / Down merchants in a view whose last month is complete (`month`).
fn recurring_changes(view: &RecurringView, month: NaiveDate) -> Vec<Unusual> {
    view.items
        .iter()
        .filter_map(|r| {
            let kind = match r.change {
                Change::Up => UnusualKind::RecurringUp,
                Change::Down => UnusualKind::RecurringDown,
                _ => return None,
            };
            let mut u = Unusual::new(kind, r.name.clone());
            u.cents = r.spark_cents.last().copied();
            u.usual_cents = Some(r.monthly_cents);
            u.change_pct = r.change_pct;
            u.merchant = Some(r.merchant.clone());
            u.account_id = Some(r.account_id.clone());
            u.month = Some(format!("{:04}-{:02}", month.year(), month.month()));
            Some(u)
        })
        .collect()
}

/// First day of each month the period touches.
fn months_of(p: &Period) -> Vec<NaiveDate> {
    let mut m = p.start.with_day(1).expect("day 1");
    let mut out = Vec::new();
    while m < p.end {
        out.push(m);
        m = m + Months::new(1);
    }
    out
}

/// The day after the month that starts at `m`.
fn month_end(m: NaiveDate) -> NaiveDate {
    m + Months::new(1)
}

fn local_day(ts: i64) -> Option<NaiveDate> {
    Local
        .timestamp_opt(ts, 0)
        .earliest()
        .map(|d| d.date_naive())
}

fn day_start(d: NaiveDate) -> i64 {
    Period::containing(PeriodKind::Custom, d).bounds().0
}

/// Median; the mean of the two middle values (rounded toward −∞) when even.
fn median(mut v: Vec<i64>) -> i64 {
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]).div_euclid(2)
    }
}

/// Nearest-rank percentile of non-empty `v`.
fn percentile(v: &mut [i64], pct: u32) -> i64 {
    v.sort_unstable();
    let n = v.len();
    let rank = (n * pct as usize).div_ceil(100).max(1);
    v[rank - 1]
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
mod tests {
    use super::*;
    use crate::model::{Account, AccountKind, MatchConfidence, MatchStatus, Transaction, TxnFlag};
    use crate::money::Money;

    fn noon(y: i32, m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn txn(id: &str, ts: i64, cents: i64, payee: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "chk".into(),
            posted: ts,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: payee.into(),
            payee: Some(payee.into()),
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            pfc_primary: None,
            pfc_detailed: None,
            source: crate::model::default_source(),
        }
    }

    fn chk() -> Account {
        Account {
            id: "chk".into(),
            org: "Bank".into(),
            name: "Checking (0000)".into(),
            kind: AccountKind::Checking,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
            item_id: None,
            kind_from_bank: None,
        }
    }

    /// A shop name that normalizes to itself and never repeats.
    fn shop(i: usize) -> String {
        let a = (b'a' + (i / 26) as u8) as char;
        let b = (b'a' + (i % 26) as u8) as char;
        format!("Shop {a}{b}x")
    }

    /// Nov 2025 to Oct 2026, synthetic: payroll, a weekly one-off shop, a
    /// monthly streaming charge that jumps in Oct, a weekly club that starts
    /// in Oct, and one large furniture charge in Oct.
    fn seed() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[chk()]).unwrap();
        let mut t = Vec::new();
        let mut n = 0;
        let mut day = NaiveDate::from_ymd_opt(2025, 11, 3).unwrap();
        while day < NaiveDate::from_ymd_opt(2026, 11, 1).unwrap() {
            let ts = noon(day.year(), day.month(), day.day());
            let cents = if day.month() == 10 && day.year() == 2026 {
                -50_00
            } else {
                -(40_00 + (n as i64 % 6) * 9_00)
            };
            t.push(txn(&format!("s{n}"), ts, cents, &shop(n)));
            n += 1;
            day = day + chrono::Days::new(7);
        }
        for i in 0..12u32 {
            let (y, m) = if i < 2 { (2025, 11 + i) } else { (2026, i - 1) };
            t.push(txn(&format!("p{i}"), noon(y, m, 1), 3000_00, "Payroll"));
            let fee = if (y, m) == (2026, 10) { -22_99 } else { -15_49 };
            t.push(txn(&format!("n{i}"), noon(y, m, 9), fee, "Streamflix"));
        }
        for (i, d) in [2, 9, 16, 23, 30].iter().enumerate() {
            t.push(txn(
                &format!("c{i}"),
                noon(2026, 10, *d),
                -6_00,
                "Coffee Club",
            ));
        }
        t.push(txn("big", noon(2026, 10, 12), -1200_00, "Furniture Barn"));
        s.upsert_transactions(&t).unwrap();
        s
    }

    fn kinds(c: &CheckIn) -> Vec<UnusualKind> {
        c.unusual.iter().map(|u| u.kind).collect()
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()
    }

    fn run(s: &Store, p: &str) -> CheckIn {
        checkin(
            s,
            Period::parse(p, today()).unwrap(),
            today(),
            noon(2026, 11, 7),
        )
        .unwrap()
    }

    #[test]
    fn completed_month_flags_up_new_and_large() {
        let s = seed();
        let c = run(&s, "2026-10");
        assert!(!c.in_progress);
        assert_eq!(c.label, "2026-10");
        assert_eq!(c.flow.in_cents, 3000_00);
        let shops = 4 * 50_00; // Oct 5, 12, 19, 26
        assert_eq!(c.flow.out_cents, shops + 22_99 + 5 * 6_00 + 1200_00);
        assert_eq!(
            c.flow.earned_minus_spent_cents,
            c.flow.in_cents - c.flow.out_cents
        );
        assert!(c.not_checked.is_empty(), "{:?}", c.not_checked);
        assert!(c.usual.periods >= USUAL_MIN_PERIODS);
        assert_eq!(c.usual.in_cents, Some(3000_00));

        let up = c
            .unusual
            .iter()
            .find(|u| u.kind == UnusualKind::RecurringUp)
            .unwrap();
        assert_eq!(up.merchant.as_deref(), Some("streamflix"));
        assert_eq!(up.month.as_deref(), Some("2026-10"));
        assert_eq!(up.cents, Some(22_99));
        assert!(up.change_pct.unwrap() > 25.0);

        let new = c
            .unusual
            .iter()
            .find(|u| u.kind == UnusualKind::NewRecurring)
            .unwrap();
        assert_eq!(new.merchant.as_deref(), Some("coffee club"));
        assert_eq!(new.day.as_deref(), Some("2026-10-02"));

        let large: Vec<&Unusual> = c
            .unusual
            .iter()
            .filter(|u| u.kind == UnusualKind::LargeCharge)
            .collect();
        assert_eq!(large.len(), 1, "only the furniture charge clears p95");
        assert_eq!(large[0].txn_id.as_deref(), Some("big"));
        assert_eq!(large[0].cents, Some(1200_00));
        assert!(large[0].usual_cents.unwrap() < 1200_00);
    }

    #[test]
    fn attention_comes_first_for_any_period() {
        let s = seed();
        s.upsert_plaid_item("it1", "Demo Bank", 0).unwrap();
        s.set_plaid_item_status("it1", "login_required").unwrap();
        s.insert_match(
            "s1",
            "s2",
            MatchStatus::Proposed,
            MatchConfidence::Medium,
            None,
            0,
        )
        .unwrap();
        let c = run(&s, "2026-W40");
        assert_eq!(
            &kinds(&c)[..2],
            [UnusualKind::Connection, UnusualKind::MatchReview]
        );
        assert_eq!(c.unusual[0].status.as_deref(), Some("login_required"));
        assert_eq!(c.unusual[1].count, Some(1));
    }

    #[test]
    fn month_in_progress_skips_recurring_changes() {
        let s = seed();
        let c = run(&s, "2026-11");
        assert!(c.in_progress);
        assert!(c.not_checked.contains(&NotChecked {
            check: Check::RecurringChanges,
            reason: NotCheckedReason::InProgress,
        }));
        assert!(!kinds(&c).contains(&UnusualKind::RecurringUp));
    }

    #[test]
    fn week_checks_only_large_charges_and_attention() {
        let s = seed();
        let c = run(&s, "2026-W41"); // Oct 5 - 11
        for check in [Check::NewRecurring, Check::RecurringChanges] {
            assert!(c.not_checked.contains(&NotChecked {
                check,
                reason: NotCheckedReason::Week,
            }));
        }
        assert!(kinds(&c).is_empty());
        let w = run(&s, "2026-W42"); // Oct 12 - 18: the furniture
        assert_eq!(kinds(&w), [UnusualKind::LargeCharge]);
    }

    #[test]
    fn quarter_judges_its_completed_months() {
        let s = seed();
        let c = run(&s, "2026-Q4");
        assert!(c.in_progress);
        assert!(
            c.not_checked.is_empty(),
            "October is complete: {:?}",
            c.not_checked
        );
        assert!(kinds(&c).contains(&UnusualKind::RecurringUp));
    }

    #[test]
    fn short_history_says_so() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[chk()]).unwrap();
        s.upsert_transactions(&[txn("a", noon(2026, 10, 3), -900_00, "Furniture Barn")])
            .unwrap();
        let c = run(&s, "2026-10");
        assert!(c.not_checked.contains(&NotChecked {
            check: Check::LargeCharges,
            reason: NotCheckedReason::ShortHistory,
        }));
        assert_eq!(c.usual.periods, 0);
        assert_eq!(c.usual.out_cents, None);
    }

    #[test]
    fn median_and_percentile() {
        assert_eq!(median(vec![3, 1, 2]), 2);
        assert_eq!(median(vec![4, 1, 3, 2]), 2);
        assert_eq!(median(vec![-3, -1]), -2);
        let mut v: Vec<i64> = (1..=100).collect();
        assert_eq!(percentile(&mut v, 95), 95);
        assert_eq!(percentile(&mut [7], 95), 7);
    }
}
