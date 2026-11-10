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
//! - **Usual to date** (a period in progress): each earlier period cut to
//!   the same number of days, so Nov 1–8 is read against Oct 1–8, Sep 1–8…
//! - **Minor** items (decided 2026-11-08, ho-2mj.5.3): late or up / down for
//!   a merchant charged every `MINOR_GAP_DAYS` days or more often (a place
//!   you visit, not a bill), and up / down by less than
//!   `MINOR_CHANGE_CENTS`. Still reported, marked `minor`, listed last; the
//!   screen folds them into one row.
//! - **Versus usual**: the period's headline figure (earned − spent; spent
//!   on a week, where paydays make earned − spent swing) against its usual.
//!   Outside the usual when the difference is at least `VERSUS_BAND_PCT`% of
//!   usual spending and the figure falls outside the
//!   `VERSUS_SPREAD_PCT`–(100 − it) percentile range of the earlier periods
//!   (weekly spending swings a lot); when it's worse, the largest money items behind it are
//!   named (`VERSUS_MAX_CONTRIBUTORS`).
//! - **Month marks** (ho-bv6.31): a month check-in also judges every
//!   finished month with a usual by the versus rule, for the month strip.
//! - Order: attention, then other items by money (`impact_cents`), minor
//!   last. One row per merchant: a merchant late and changed in the same
//!   period shows once, as late, carrying the change.
//!
//! A check that can't run is listed in `not_checked` with its reason, so the
//! screen says why instead of showing nothing.

use crate::model::Transaction;
use crate::period::{Period, PeriodKind};
use crate::query::{account_kinds, category_spend, flow_over, lane_of, Lane, TxnFilter};
use crate::recurring::{self, Change, RecurringView};
use crate::stock::{net_worth_change, ChangeReason, NetWorthChange};
use crate::store::Store;
use crate::subscriptions::{display_merchant, normalize_payee};
use crate::upkeep;
use chrono::{Datelike, Days, Local, Months, NaiveDate, TimeZone};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap, HashSet};

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
/// A recurring merchant charged this often or more (usual gap in days) is a
/// place you visit: its late or up / down items are minor.
pub const MINOR_GAP_DAYS: i64 = 20;
/// An up / down smaller than this (against the usual) is minor.
pub const MINOR_CHANGE_CENTS: i64 = 20_00;
/// The headline figure is outside its usual when the difference is at least
/// this % of usual spending.
pub const VERSUS_BAND_PCT: i64 = 25;
/// ...and outside this low / high percentile of the earlier periods.
pub const VERSUS_SPREAD_PCT: u32 = 10;
/// At most this many items are named behind a worse-than-usual figure.
pub const VERSUS_MAX_CONTRIBUTORS: usize = 3;

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
    /// The headline figure against its usual; None without a usual.
    pub versus: Option<Versus>,
    /// Attention first, then by `impact_cents` (largest first), minor items
    /// last.
    pub unusual: Vec<Unusual>,
    /// Checks that didn't run for this period, and why.
    pub not_checked: Vec<NotChecked>,
    /// Month check-ins only: every finished month with a usual, judged the
    /// way `versus` judges a month, so the month strip can mark the ones
    /// that need a look. Oldest first; empty for weeks and quarters.
    pub months: Vec<MonthMark>,
}

/// One month on the strip against its usual (the `versus` rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MonthMark {
    /// `YYYY-MM`.
    pub month: String,
    pub outside: bool,
    pub worse: bool,
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
#[derive(Debug, Clone, PartialEq, Serialize)]
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
    /// Set while the period is in progress: the medians are of the earlier
    /// periods cut to the same length, through this day of this period
    /// (`YYYY-MM-DD`, today).
    pub to_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum VersusFigure {
    /// Months and quarters: `flow.earned_minus_spent_cents`.
    EarnedMinusSpent,
    /// Weeks: `flow.out_cents` (a payday week isn't unusual).
    Spent,
}

/// The headline figure against its usual.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Versus {
    pub figure: VersusFigure,
    pub cents: i64,
    pub usual_cents: i64,
    /// `cents - usual_cents`.
    pub diff_cents: i64,
    /// At least `VERSUS_BAND_PCT`% of usual spending away from the usual.
    /// Far but not `outside` = a big difference that's still within how
    /// much these periods swing (a rent week); say so, don't call it usual.
    pub far: bool,
    /// Far, and beyond the earlier periods' usual spread.
    pub outside: bool,
    /// Kept less, or spent more, than usual.
    pub worse: bool,
    /// Earned − spent only: `flow.in_cents` minus its usual (how much of the
    /// difference is income).
    pub in_diff_cents: Option<i64>,
    /// When outside and worse: the unusual items with the most money behind
    /// the difference (large charges, recurring costs up), largest first,
    /// each at least a tenth of the difference.
    pub contributors: Vec<Contributor>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Contributor {
    pub label: String,
    pub merchant: Option<String>,
    /// Its share, positive: the charge, or a cost's rise over its usual.
    pub cents: i64,
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
    /// A recurring merchant with no charge for 1.5x its usual gap (at least
    /// a month), as of the period's end: skipped, moved or stopped.
    Late,
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
    /// What it's compared with: for a recurring change, the mean of up to 3
    /// months before (`change_pct`'s base); for a large charge, the
    /// threshold; for a new merchant, unset.
    pub usual_cents: Option<i64>,
    /// Signed % against the usual (recurring up/down). On a late item: a
    /// change judged earlier in the period, in `month`, if there was one.
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
    /// Recurring: the merchant's usual days between charges.
    pub gap_days: Option<i64>,
    /// Recurring up / down judged charge to charge (a merchant slower than
    /// monthly): `cents` is the charge and `usual_cents` the usual charge,
    /// not monthly totals.
    pub per_charge: bool,
    /// Money at stake, positive, for ordering: the charge (large), the
    /// difference from the usual (up / down), the usual monthly cost (new,
    /// late). None for attention items.
    pub impact_cents: Option<i64>,
    /// A small or frequent-merchant item (see the module docs); the screen
    /// folds these into one row.
    pub minor: bool,
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
            gap_days: None,
            per_charge: false,
            impact_cents: None,
            minor: false,
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

    let nw_over = |since: i64, until: i64| {
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
    let nw = |p: &Period| {
        let (since, until) = p.bounds();
        nw_over(since, until)
    };
    let (since, until) = period.bounds();
    let flow = flow_over(&txns, &kind, since, until);
    let in_progress = period.end > today;
    // In progress: earlier periods are cut to the days elapsed so far.
    let elapsed = in_progress.then(|| {
        Days::new(((today - period.start).num_days() + 1).clamp(1, period.days() as i64) as u64)
    });
    let cut = |p: &Period| -> (i64, i64) {
        match elapsed {
            Some(n) => (day_start(p.start), day_start((p.start + n).min(p.end))),
            None => p.bounds(),
        }
    };

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
    let flows: Vec<_> = usual_windows(&period, today, first_day)
        .into_iter()
        .map(|(s, u)| flow_over(&txns, &kind, s, u))
        .collect();
    let nw_changes: Vec<i64> = earlier
        .iter()
        .filter_map(|p| {
            let (s, u) = cut(p);
            nw_over(s, u).change.map(|m| m.cents())
        })
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
        to_date: elapsed.map(|_| today.min(period.last_day()).to_string()),
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
            // Slower merchants: a charge in the period against the ones
            // before it, month in progress or not.
            for u in charge_changes(&last_view, since, until) {
                changed.insert(u.merchant.clone().unwrap_or_default(), u);
            }
            // One row per merchant: late wins (the latest state) and carries
            // the change judged earlier in the period.
            for mut l in late(&last_view, since, until) {
                if let Some(c) = l.merchant.as_ref().and_then(|m| changed.remove(m)) {
                    l.change_pct = c.change_pct;
                    l.month = c.month;
                }
                unusual.push(l);
            }
            unusual.extend(changed.into_values());
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
                        u.impact_cents = Some(c);
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

    // Late on a week: recurring merchants that went quiet during it (month
    // and quarter add theirs above, merged with the changes).
    if period.kind == PeriodKind::Week {
        unusual.extend(late(&last_view, since, until));
    }

    for u in &mut unusual {
        mark_minor(u);
    }
    // Attention first (already first, kept in order), then by money, minor
    // last. Stable, so equal items keep the order they were found in.
    unusual.sort_by_key(|u| {
        let attention = matches!(u.kind, UnusualKind::Connection | UnusualKind::MatchReview);
        (
            !attention,
            u.minor,
            std::cmp::Reverse(u.impact_cents.unwrap_or(0)),
        )
    });

    let flow = FlowSummary {
        in_cents: flow.in_cents,
        out_cents: flow.out_cents,
        earned_minus_spent_cents: flow.earned_minus_spent_cents,
        saved_cents: flow.saved_cents,
        gap_cents: flow.gap_cents,
    };
    let versus = versus(period.kind, &flow, &usual, &flows, &unusual);
    let months = if period.kind == PeriodKind::Month {
        month_marks(&txns, &kind, first_day, today)
    } else {
        Vec::new()
    };
    let change = nw(&period);
    Ok(CheckIn {
        label: period.to_string(),
        period,
        in_progress,
        net_worth: summary(&change),
        flow,
        usual,
        versus,
        unusual,
        not_checked,
        months,
    })
}

/// Every finished month since the first full one, judged against the
/// months before it exactly as a month check-in's `versus` would be.
fn month_marks(
    txns: &[Transaction],
    kind: &impl Fn(&str) -> crate::model::AccountKind,
    first_day: Option<NaiveDate>,
    today: NaiveDate,
) -> Vec<MonthMark> {
    let Some(first) = first_day else {
        return Vec::new();
    };
    // Months that start on or after the first transaction (the usual's
    // rule), up to the last finished one.
    let mut start = first.with_day(1).expect("day 1");
    if start < first {
        start = start + Months::new(1);
    }
    let this_month = today.with_day(1).expect("day 1");
    let mut all = Vec::new();
    let mut m = start;
    while m < this_month {
        let p = Period::containing(PeriodKind::Month, m);
        let (s, u) = p.bounds();
        all.push((p, flow_over(txns, kind, s, u)));
        m = m + Months::new(1);
    }
    let mut out = Vec::new();
    for (i, (p, f)) in all.iter().enumerate() {
        let earlier: Vec<_> = all[i.saturating_sub(USUAL_PERIODS)..i]
            .iter()
            .map(|(_, f)| f.clone())
            .collect();
        if earlier.len() < USUAL_MIN_PERIODS {
            continue;
        }
        let usual_em = median(earlier.iter().map(|e| e.earned_minus_spent_cents).collect());
        let spending = median(earlier.iter().map(|e| e.out_cents).collect());
        let mut past: Vec<i64> = earlier.iter().map(|e| e.earned_minus_spent_cents).collect();
        let (_, worse, _, outside) = judge(
            false,
            f.earned_minus_spent_cents,
            usual_em,
            spending,
            &mut past,
        );
        out.push(MonthMark {
            month: p.start.format("%Y-%m").to_string(),
            outside,
            worse,
        });
    }
    out
}

/// The versus rule on one figure: (difference, worse, outside). `past` is
/// the earlier periods' same figure; `spending` their usual spending.
fn judge(
    week: bool,
    cents: i64,
    usual_cents: i64,
    spending: i64,
    past: &mut [i64],
) -> (i64, bool, bool, bool) {
    let low = percentile(past, VERSUS_SPREAD_PCT);
    let high = percentile(past, 100 - VERSUS_SPREAD_PCT);
    let diff = cents - usual_cents;
    let worse = if week { diff > 0 } else { diff < 0 };
    let far = spending > 0 && diff.abs() * 100 >= VERSUS_BAND_PCT * spending;
    let outside = far && (cents < low || cents > high);
    (diff, worse, far, outside)
}

/// Marks small and frequent-merchant items minor (module docs).
fn mark_minor(u: &mut Unusual) {
    let frequent = u.gap_days.is_some_and(|g| g <= MINOR_GAP_DAYS);
    u.minor = match u.kind {
        UnusualKind::Late => frequent,
        UnusualKind::RecurringUp | UnusualKind::RecurringDown => {
            frequent || u.impact_cents.unwrap_or(0) < MINOR_CHANGE_CENTS
        }
        _ => false,
    };
}

/// The headline figure against its usual (module docs).
fn versus(
    kind: PeriodKind,
    f: &FlowSummary,
    usual: &Usual,
    earlier: &[crate::query::FlowMonth],
    unusual: &[Unusual],
) -> Option<Versus> {
    let spending = usual.out_cents?;
    let week = kind == PeriodKind::Week;
    let (figure, cents, usual_cents) = if week {
        (VersusFigure::Spent, f.out_cents, spending)
    } else {
        (
            VersusFigure::EarnedMinusSpent,
            f.earned_minus_spent_cents,
            usual.earned_minus_spent_cents?,
        )
    };
    let mut past: Vec<i64> = earlier
        .iter()
        .map(|e| {
            if week {
                e.out_cents
            } else {
                e.earned_minus_spent_cents
            }
        })
        .collect();
    let (diff, worse, far, outside) = judge(week, cents, usual_cents, spending, &mut past);
    let mut contributors: Vec<Contributor> = if outside && worse {
        unusual
            .iter()
            .filter_map(|u| {
                let cents = match u.kind {
                    UnusualKind::LargeCharge => u.cents?,
                    UnusualKind::RecurringUp => u.impact_cents?,
                    _ => return None,
                };
                Some(Contributor {
                    label: u.label.clone(),
                    merchant: u.merchant.clone(),
                    cents,
                })
            })
            .collect()
    } else {
        Vec::new()
    };
    // Named only when worth a tenth of the difference or more.
    contributors.retain(|c| c.cents * 10 >= diff.abs());
    contributors.sort_by_key(|c| std::cmp::Reverse(c.cents));
    contributors.truncate(VERSUS_MAX_CONTRIBUTORS);
    Some(Versus {
        figure,
        cents,
        usual_cents,
        diff_cents: diff,
        far,
        outside,
        worse,
        in_diff_cents: (figure == VersusFigure::EarnedMinusSpent)
            .then(|| usual.in_cents.map(|u| f.in_cents - u))
            .flatten(),
        contributors,
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
            u.impact_cents = Some(r.monthly_cents);
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
        .filter(|r| !r.cadence.is_slow())
        .filter_map(|r| {
            let kind = match r.change {
                Change::Up => UnusualKind::RecurringUp,
                Change::Down => UnusualKind::RecurringDown,
                _ => return None,
            };
            let mut u = Unusual::new(kind, r.name.clone());
            u.cents = r.spark_cents.last().copied();
            u.usual_cents = r.change_base_cents;
            u.change_pct = r.change_pct;
            u.merchant = Some(r.merchant.clone());
            u.account_id = Some(r.account_id.clone());
            u.month = Some(format!("{:04}-{:02}", month.year(), month.month()));
            u.gap_days = Some(r.gap_days);
            u.impact_cents = u.cents.zip(u.usual_cents).map(|(c, b)| (c - b).abs());
            Some(u)
        })
        .collect()
}

/// Up / Down for merchants slower than monthly whose latest charge falls
/// in `[since, until)`: that charge against the usual charge.
fn charge_changes(view: &RecurringView, since: i64, until: i64) -> Vec<Unusual> {
    view.items
        .iter()
        .filter(|r| r.cadence.is_slow())
        .filter_map(|r| {
            let kind = match r.change {
                Change::Up => UnusualKind::RecurringUp,
                Change::Down => UnusualKind::RecurringDown,
                _ => return None,
            };
            let latest = r.recent_charges.last()?;
            if !(since..until).contains(&latest.at) {
                return None;
            }
            let day = local_day(latest.at)?;
            let mut u = Unusual::new(kind, r.name.clone());
            u.cents = Some(latest.cents);
            u.usual_cents = r.change_base_cents;
            u.change_pct = r.change_pct;
            u.merchant = Some(r.merchant.clone());
            u.account_id = Some(r.account_id.clone());
            u.day = Some(day.to_string());
            u.month = Some(format!("{:04}-{:02}", day.year(), day.month()));
            u.gap_days = Some(r.gap_days);
            u.per_charge = true;
            u.impact_cents = u.usual_cents.map(|b| (latest.cents - b).abs());
            Some(u)
        })
        .collect()
}

/// Recurring merchants (not ignored) that became late in `[since, until)`,
/// longest-quiet first. `day` is the last charge.
fn late(view: &RecurringView, since: i64, until: i64) -> Vec<Unusual> {
    let mut out: Vec<(i64, Unusual)> = view
        .items
        .iter()
        .filter_map(|r| {
            let at = r.late_since.filter(|at| (since..until).contains(at))?;
            let mut u = Unusual::new(UnusualKind::Late, r.name.clone());
            u.cents = Some(r.median_cents);
            u.merchant = Some(r.merchant.clone());
            u.account_id = Some(r.account_id.clone());
            u.day = local_day(r.last_seen).map(|d| d.to_string());
            u.gap_days = Some(r.gap_days);
            u.impact_cents = Some(r.monthly_cents);
            Some((at, u))
        })
        .collect();
    out.sort_by_key(|(at, _)| *at);
    out.into_iter().map(|(_, u)| u).collect()
}

/// First day of each month the period touches.
/// The windows a usual is taken over: up to `USUAL_PERIODS` periods of the
/// same kind before `period` that start on or after `first_day`, each cut to
/// the days elapsed so far while `period` is in progress (usual to date).
fn usual_windows(
    period: &Period,
    today: NaiveDate,
    first_day: Option<NaiveDate>,
) -> Vec<(i64, i64)> {
    let elapsed = (period.end > today).then(|| {
        Days::new(((today - period.start).num_days() + 1).clamp(1, period.days() as i64) as u64)
    });
    let mut out = Vec::new();
    let mut p = *period;
    for _ in 0..USUAL_PERIODS {
        p = p.previous();
        if !first_day.is_some_and(|d| p.start >= d) {
            continue;
        }
        out.push(match elapsed {
            Some(n) => (day_start(p.start), day_start((p.start + n).min(p.end))),
            None => p.bounds(),
        });
    }
    out
}

/// A category's spend against its usual, for the Flow Out lane's margin
/// notes (GLOSSARY: Usual, Category change).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CategoryVsUsual {
    /// `None` = uncategorized.
    pub category: Option<String>,
    /// Spend net of refunds this period, positive = money spent.
    pub total_cents: i64,
    /// Number of purchases (refunds not counted).
    pub count: usize,
    /// Median of the earlier periods' spend in this category, a period with
    /// none counting as $0. None without `USUAL_MIN_PERIODS` periods.
    pub usual_cents: Option<i64>,
    /// Set when the spend is far from the usual (`CATEGORY_FAR_RATIO` and
    /// `CATEGORY_FAR_CENTS`) in a finished period. Never set while the period
    /// is in progress (one early trip reads as double a usual-to-date), nor
    /// on uncategorized spend, which swings with how much is categorized,
    /// not with what was bought.
    pub change: Option<CategoryChange>,
}

/// Which way a category is far from its usual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum CategoryChange {
    /// At least `CATEGORY_FAR_RATIO` times the usual.
    Up,
    /// At most 1 / `CATEGORY_FAR_RATIO` of the usual.
    Down,
}

/// Spend per category for one period, each against its usual.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CategoriesVsUsual {
    pub period: Period,
    /// The period's spelling (`2026-11`, `2026-W45`, `2026-Q4`).
    pub label: String,
    /// How many earlier periods the usual is taken over.
    pub periods: usize,
    /// While the period is in progress: the last day each earlier period
    /// was cut to, `YYYY-MM-DD` (usual to date).
    pub to_date: Option<String>,
    /// Largest spend first, like `/spend/categories`.
    pub items: Vec<CategoryVsUsual>,
}

/// A category is far from its usual at this many times it (or under one
/// over it)...
pub const CATEGORY_FAR_RATIO: i64 = 2;
/// ...and at least this far in dollars, so small categories stay quiet.
pub const CATEGORY_FAR_CENTS: i64 = 50_00;

/// Spend per category for `period` against each category's usual (the
/// median of the same earlier periods the check-in's usual reads). Pending
/// rows count, as on Flow; transfers and card payments don't.
pub fn categories_vs_usual(
    store: &Store,
    period: Period,
    today: NaiveDate,
) -> rusqlite::Result<CategoriesVsUsual> {
    let txns = store.all_transactions()?;
    let kind = account_kinds(store)?;
    let first_day = txns
        .iter()
        .map(|t| t.effective_date())
        .min()
        .and_then(local_day);
    let (since, until) = period.bounds();
    let now = category_spend(&txns, &kind, &TxnFilter::range(since, until));
    let windows = usual_windows(&period, today, first_day);
    let in_progress = period.end > today;
    let earlier: Vec<HashMap<Option<String>, i64>> = windows
        .iter()
        .map(|&(s, u)| {
            category_spend(&txns, &kind, &TxnFilter::range(s, u))
                .into_iter()
                .map(|c| (c.category, c.total_cents))
                .collect()
        })
        .collect();
    let items = now
        .into_iter()
        .map(|c| {
            let usual_cents = (earlier.len() >= USUAL_MIN_PERIODS).then(|| {
                median(
                    earlier
                        .iter()
                        .map(|m| m.get(&c.category).copied().unwrap_or(0))
                        .collect(),
                )
            });
            let change = match (usual_cents, &c.category) {
                (Some(u), Some(_)) if !in_progress => category_change(c.total_cents, u),
                _ => None,
            };
            CategoryVsUsual {
                category: c.category,
                total_cents: c.total_cents,
                count: c.count,
                usual_cents,
                change,
            }
        })
        .collect();
    Ok(CategoriesVsUsual {
        period,
        label: period.to_string(),
        periods: windows.len(),
        to_date: in_progress.then(|| today.min(period.last_day()).to_string()),
        items,
    })
}

fn category_change(cents: i64, usual: i64) -> Option<CategoryChange> {
    if (cents - usual).abs() < CATEGORY_FAR_CENTS {
        None
    } else if cents >= usual * CATEGORY_FAR_RATIO {
        Some(CategoryChange::Up)
    } else if cents * CATEGORY_FAR_RATIO <= usual {
        Some(CategoryChange::Down)
    } else {
        None
    }
}

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

    /// Apr–Sep 2026 steady (synthetic), then an October with one category
    /// up, one down, one new and large, one new and small.
    fn seed_categories() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[chk()]).unwrap();
        let mut t = Vec::new();
        let mut add = |m: u32, cents: i64, cat: Option<&str>| {
            let mut x = txn(&format!("t{}", t.len()), noon(2026, m, 1), cents, "Shop");
            x.category = cat.map(Into::into);
            t.push(x);
        };
        for m in 4..=9 {
            add(m, -300_00, Some("Groceries"));
            add(m, -100_00, Some("Home"));
            add(m, -200_00, Some("Dining"));
        }
        add(10, -310_00, Some("Groceries"));
        add(10, -250_00, Some("Home"));
        add(10, -60_00, Some("Dining"));
        add(10, -500_00, Some("Travel"));
        add(10, -40_00, Some("Gifts"));
        add(10, -400_00, None);
        s.upsert_transactions(&t).unwrap();
        s
    }

    #[test]
    fn categories_read_against_their_usual() {
        let s = seed_categories();
        let p = Period::parse("2026-10", today()).unwrap();
        let c = categories_vs_usual(&s, p, today()).unwrap();
        assert_eq!(c.periods, 6, "only months on or after the first row");
        assert_eq!(c.to_date, None);
        let get = |cat: Option<&str>| {
            c.items
                .iter()
                .find(|i| i.category.as_deref() == cat)
                .unwrap()
        };
        let row = |cat: &str| get(Some(cat));
        assert_eq!(row("Groceries").usual_cents, Some(300_00));
        assert_eq!(row("Groceries").change, None);
        assert_eq!(row("Home").change, Some(CategoryChange::Up));
        assert_eq!(row("Dining").change, Some(CategoryChange::Down));
        assert_eq!(row("Travel").usual_cents, Some(0));
        assert_eq!(row("Travel").change, Some(CategoryChange::Up));
        assert_eq!(row("Gifts").change, None, "under the dollar floor");
        assert_eq!(get(None).change, None, "uncategorized is never judged");
        assert_eq!(c.items[0].category.as_deref(), Some("Travel"));
        // Totals agree with /spend/categories.
        let (since, until) = p.bounds();
        let flat = crate::query::spend_by_category(&s, &TxnFilter::range(since, until)).unwrap();
        let sum = |v: Vec<i64>| v.into_iter().sum::<i64>();
        assert_eq!(
            sum(c.items.iter().map(|i| i.total_cents).collect()),
            sum(flat.iter().map(|i| i.total_cents).collect())
        );
    }

    #[test]
    fn categories_in_progress_get_a_usual_to_date_but_no_change() {
        let s = seed_categories();
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let p = Period::parse("2026-10", today).unwrap();
        let c = categories_vs_usual(&s, p, today).unwrap();
        assert_eq!(c.to_date.as_deref(), Some("2026-10-05"));
        let home = c
            .items
            .iter()
            .find(|i| i.category.as_deref() == Some("Home"))
            .unwrap();
        assert_eq!(
            home.usual_cents,
            Some(100_00),
            "rows on the 1st fall inside the cut"
        );
        assert!(c.items.iter().all(|i| i.change.is_none()));
    }

    #[test]
    fn categories_without_enough_history_have_no_usual() {
        let s = seed_categories();
        let p = Period::parse("2026-06", today()).unwrap();
        let c = categories_vs_usual(&s, p, today()).unwrap();
        assert_eq!(c.periods, 2);
        assert!(c
            .items
            .iter()
            .all(|i| i.usual_cents.is_none() && i.change.is_none()));
    }

    #[test]
    fn category_change_needs_ratio_and_dollars() {
        use CategoryChange::{Down, Up};
        assert_eq!(category_change(200_00, 100_00), Some(Up));
        assert_eq!(category_change(199_99, 100_00), None);
        assert_eq!(category_change(79_99, 30_00), None, "2.7x but under $50");
        assert_eq!(category_change(80_00, 30_00), Some(Up));
        assert_eq!(category_change(50_00, 100_00), Some(Down));
        assert_eq!(category_change(50_01, 100_00), None);
        assert_eq!(category_change(10_00, 40_00), None, "down 75% but only $30");
        assert_eq!(category_change(0, 0), None);
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
        // The usual shown beside the % is the base the % was taken against.
        let base = up.usual_cents.unwrap() as f64;
        let pct = (22_99.0 - base) / base * 100.0;
        assert!(
            (pct - up.change_pct.unwrap()).abs() < 0.1,
            "{pct} vs {:?}",
            up.change_pct
        );

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

    /// The seed plus two merchants slower than monthly: a groomer every two
    /// months whose August visit costs more, and a plumber who went quiet
    /// after May.
    fn seed_slow() -> Store {
        let s = seed();
        let mut t = Vec::new();
        for (i, (y, m, c)) in [
            (2025, 12, 80_00i64),
            (2026, 2, 80_00),
            (2026, 4, 80_00),
            (2026, 6, 80_00),
            (2026, 8, 105_00),
        ]
        .iter()
        .enumerate()
        {
            t.push(txn(
                &format!("g{i}"),
                noon(*y, *m, 10),
                -c,
                "Sample Groomer",
            ));
        }
        for (i, (y, m)) in [(2025, 11), (2026, 1), (2026, 3), (2026, 5)]
            .iter()
            .enumerate()
        {
            t.push(txn(
                &format!("pl{i}"),
                noon(*y, *m, 10),
                -90_00,
                "Sample Plumber",
            ));
        }
        s.upsert_transactions(&t).unwrap();
        s
    }

    fn find<'a>(c: &'a CheckIn, kind: UnusualKind, merchant: &str) -> Option<&'a Unusual> {
        c.unusual
            .iter()
            .find(|u| u.kind == kind && u.merchant.as_deref() == Some(merchant))
    }

    #[test]
    fn slower_merchants_are_judged_by_charge_and_go_late_once() {
        let s = seed_slow();

        let aug = run(&s, "2026-08");
        let up = find(&aug, UnusualKind::RecurringUp, "sample groomer").expect("groomer up");
        assert!(up.per_charge);
        assert_eq!(up.cents, Some(105_00));
        assert_eq!(up.usual_cents, Some(80_00));
        assert_eq!(up.day.as_deref(), Some("2026-08-10"));
        assert!(up.gap_days.unwrap() >= 55);
        let late = find(&aug, UnusualKind::Late, "sample plumber").expect("plumber late");
        assert_eq!(late.day.as_deref(), Some("2026-05-10"));
        assert_eq!(late.cents, Some(90_00));

        // A month without a visit is not "down", and late shows only once.
        for p in ["2026-09", "2026-10"] {
            let c = run(&s, p);
            assert!(
                find(&c, UnusualKind::RecurringDown, "sample groomer").is_none(),
                "{p}"
            );
            assert!(
                find(&c, UnusualKind::RecurringUp, "sample groomer").is_none(),
                "{p}"
            );
            assert!(
                find(&c, UnusualKind::Late, "sample plumber").is_none(),
                "{p}"
            );
            assert!(
                !c.unusual
                    .iter()
                    .any(|u| u.change_pct.is_some_and(|p| p <= -100.0)),
                "no down 100% in {p}"
            );
        }

        // The quarter holding both.
        let q3 = run(&s, "2026-Q3");
        assert!(find(&q3, UnusualKind::RecurringUp, "sample groomer").is_some());
        assert!(find(&q3, UnusualKind::Late, "sample plumber").is_some());
    }

    #[test]
    fn month_compares_with_usual_and_names_what_moved_it() {
        let s = seed();
        let c = run(&s, "2026-10");
        let v = c.versus.as_ref().expect("usual exists");
        assert_eq!(v.figure, VersusFigure::EarnedMinusSpent);
        assert_eq!(v.cents, c.flow.earned_minus_spent_cents);
        assert_eq!(Some(v.usual_cents), c.usual.earned_minus_spent_cents);
        assert_eq!(v.diff_cents, v.cents - v.usual_cents);
        assert!(v.outside && v.worse);
        assert_eq!(v.in_diff_cents, Some(0), "payroll as usual");
        assert_eq!(v.contributors[0].label, "Furniture Barn");
        assert_eq!(v.contributors[0].cents, 1200_00);
        assert!(v.contributors.len() <= VERSUS_MAX_CONTRIBUTORS);
    }

    #[test]
    fn a_rent_sized_week_is_far_but_not_outside() {
        // Weeks of $100 with a $300 rent week every fourth: $300 is far from
        // the usual $100, but within how these weeks swing.
        let mut past = vec![100, 100, 100, 300, 100, 100, 100, 300, 100, 100, 100, 300];
        let (diff, worse, far, outside) = judge(true, 300, 100, 100, &mut past);
        assert_eq!(diff, 200);
        assert!(worse && far && !outside);
        // Beyond the swing: outside too.
        let (_, _, far, outside) = judge(true, 900, 100, 100, &mut past);
        assert!(far && outside);
        // Close to usual: neither.
        let (_, _, far, outside) = judge(true, 110, 100, 100, &mut past);
        assert!(!far && !outside);
    }

    #[test]
    fn month_marks_match_each_months_own_versus() {
        let s = seed();
        let c = run(&s, "2026-10");
        assert!(!c.months.is_empty());
        for m in &c.months {
            let own = run(&s, &m.month);
            let v = own.versus.as_ref().expect("marked months have a usual");
            assert_eq!((m.outside, m.worse), (v.outside, v.worse), "{}", m.month);
        }
        let oct = c.months.iter().find(|m| m.month == "2026-10").unwrap();
        assert!(oct.outside && oct.worse);
        // Weeks and quarters carry no marks.
        assert!(run(&s, "2026-W42").months.is_empty());
        assert!(run(&s, "2026-Q3").months.is_empty());
    }

    #[test]
    fn unusual_is_ordered_by_money_with_minor_last() {
        let s = seed();
        let c = run(&s, "2026-10");
        assert_eq!(c.unusual[0].kind, UnusualKind::LargeCharge);
        let up = find(&c, UnusualKind::RecurringUp, "streamflix").unwrap();
        // $7.50 over its usual: minor, listed after everything else.
        assert!(up.minor);
        assert_eq!(up.impact_cents, Some(22_99 - up.usual_cents.unwrap()));
        let first_minor = c.unusual.iter().position(|u| u.minor).unwrap();
        assert!(c.unusual[first_minor..].iter().all(|u| u.minor));
        assert!(!c.unusual[0].minor);
        let money: Vec<i64> = c.unusual[..first_minor]
            .iter()
            .map(|u| u.impact_cents.unwrap_or(0))
            .collect();
        assert!(money.windows(2).all(|w| w[0] >= w[1]), "{money:?}");
    }

    #[test]
    fn week_compares_spending_not_paydays() {
        let s = seed();
        let w = run(&s, "2026-W42"); // the furniture week
        let v = w.versus.as_ref().unwrap();
        assert_eq!(v.figure, VersusFigure::Spent);
        assert_eq!(v.cents, w.flow.out_cents);
        assert!(v.worse && v.outside);
        assert_eq!(v.in_diff_cents, None);
        assert_eq!(v.contributors[0].cents, 1200_00);
        // A payday week with ordinary spending isn't outside.
        let pay = run(&s, "2026-W36"); // Aug 31 - Sep 6, payroll Sep 1
        assert!(pay.flow.in_cents > 0);
        assert!(!pay.versus.unwrap().outside);
    }

    #[test]
    fn month_in_progress_reads_against_usual_to_date() {
        let s = seed();
        let c = run(&s, "2026-11"); // through Nov 7
        assert_eq!(c.usual.to_date.as_deref(), Some("2026-11-07"));
        // Earlier months cut to their first 7 days: payroll (the 1st) and
        // one weekly shop, not Streamflix (the 9th).
        assert_eq!(c.usual.in_cents, Some(3000_00));
        assert!(
            c.usual.out_cents.unwrap() <= 85_00,
            "{:?}",
            c.usual.out_cents
        );
        assert_eq!(run(&s, "2026-10").usual.to_date, None);
    }

    #[test]
    fn late_and_changed_in_one_period_is_one_row() {
        let s = seed();
        let mut t = Vec::new();
        let mut m = NaiveDate::from_ymd_opt(2025, 11, 10).unwrap();
        let mut i = 0;
        while m <= NaiveDate::from_ymd_opt(2026, 7, 10).unwrap() {
            let c = if m.month() == 7 { 20_00 } else { 50_00 };
            t.push(txn(
                &format!("gy{i}"),
                noon(m.year(), m.month(), 10),
                -c,
                "Sample Gym",
            ));
            m = m + Months::new(1);
            i += 1;
        }
        s.upsert_transactions(&t).unwrap();
        let q3 = run(&s, "2026-Q3");
        let rows: Vec<&Unusual> = q3
            .unusual
            .iter()
            .filter(|u| u.merchant.as_deref() == Some("sample gym"))
            .collect();
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].kind, UnusualKind::Late);
        assert_eq!(rows[0].month.as_deref(), Some("2026-07"));
        assert!(rows[0].change_pct.unwrap() < -25.0);
        assert!(!rows[0].minor, "monthly, not a frequent visit");
    }

    #[test]
    fn frequent_merchants_go_minor() {
        let mut u = Unusual::new(UnusualKind::Late, "Corner Spot");
        u.gap_days = Some(19);
        mark_minor(&mut u);
        assert!(u.minor);
        u.gap_days = Some(30);
        mark_minor(&mut u);
        assert!(!u.minor);
        let mut d = Unusual::new(UnusualKind::RecurringDown, "Bill");
        d.gap_days = Some(30);
        d.impact_cents = Some(MINOR_CHANGE_CENTS);
        mark_minor(&mut d);
        assert!(!d.minor);
        d.impact_cents = Some(MINOR_CHANGE_CENTS - 1);
        mark_minor(&mut d);
        assert!(d.minor);
    }
}
