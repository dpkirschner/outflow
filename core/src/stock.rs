//! Stock change over a period: net worth change, per-account change, gain
//! net of contributions, and the hero split Markets + Added. Pure — the store
//! hands in the visible accounts and every snapshot and activity row.
//!
//! Measurement rules (ho-2mj.4.4; also docs/DATA_MODEL.md):
//!
//! - **Snapshots per account.** Start = the last balance snapshot at or
//!   before the period start, else the first inside it (`partial_start`).
//!   End = the last snapshot inside the period. One snapshot: no change.
//! - **A. Values come from balance snapshots**, the source net worth and the
//!   breakdown use, so every figure sums. Positions only test measurability.
//! - **B. No positions = cash.** An investment account holding nothing but
//!   cash has no market: its whole change goes to Added (`GainReason::Cash`).
//! - **C. Measurable** only when every non-cash position's quantity change
//!   between the start and end holdings is explained by activity rows, within
//!   `QTY_TOLERANCE`. Otherwise `GainReason::ContributionsUnknown` — never an
//!   estimate.
//! - **D. Share transfers** (type transfer, quantity != 0, no cash) are
//!   contributions valued at quantity x the recorded snapshot price on or
//!   after the transfer date.
//! - **E. Contributions** = external cash in/out: contribution, deposit,
//!   withdrawal, transfer with cash. A `buy/contribution` (payroll buying
//!   shares directly) counts its full cost. Dividends, reinvestment, interest
//!   and fees are performance. Cancelled rows are skipped.
//!
//! Activity counts when its date is after the start snapshot's local day, up
//! to and including the end snapshot's day.

use crate::balances::{local_day, parse_decimal, Group, Totals};
use crate::model::{
    account_display_label, Account, BalanceSnapshot, HoldingSnapshot, InvestmentTxn, Security,
};
use crate::money::Money;
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Share quantities are compared as integers in 1e-8 share units.
const QTY_SCALE: i128 = 100_000_000;
/// A quantity gap up to 0.001 share is rounding, not an unexplained trade.
pub const QTY_TOLERANCE: i128 = 100_000;

/// Why a change is missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChangeReason {
    /// No snapshot at all up to the period end.
    NoSnapshots,
    /// Start and end are the same snapshot.
    OneSnapshot,
}

/// Why an investment account has no gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum GainReason {
    /// No positions: cash has no market; the change is all Added.
    Cash,
    /// Positions moved in ways the activity doesn't explain, or a share
    /// transfer has no recorded price.
    ContributionsUnknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AccountChange {
    pub account_id: String,
    /// Nickname-aware display label.
    pub account: String,
    pub group: Group,
    /// Balance at the start snapshot (counted in `NetWorthChange::start`).
    pub start: Money,
    /// Balance at the end snapshot (counted in `NetWorthChange::end`).
    pub end: Money,
    /// Epoch seconds of the start / end snapshots.
    pub start_at: i64,
    pub end_at: i64,
    /// Start is the first snapshot inside the period, not one before it.
    pub partial_start: bool,
    /// `end - start`; None with `change_reason` when start is end.
    pub change: Option<Money>,
    pub change_reason: Option<ChangeReason>,
    /// Investment accounts: net external money in (withdrawals negative),
    /// None when a share transfer has no recorded price.
    pub contributions: Option<Money>,
    /// Investment accounts: gain net of contributions = change -
    /// contributions. None with `gain_reason`.
    pub gain: Option<Money>,
    pub gain_reason: Option<GainReason>,
}

/// An investment account left out of Markets; its change lands in Added.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ExcludedAccount {
    pub account_id: String,
    pub account: String,
    pub change: Money,
}

/// One day of the Invested vs contributed chart.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ContributedPoint {
    /// Local `YYYY-MM-DD`.
    pub day: String,
    /// Investment account balances that day.
    pub invested: Money,
    /// Invested on the first day plus net contributions through this day.
    /// The gap to `invested` is market gain.
    pub contributed: Money,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct NetWorthChange {
    /// The breakdown at each account's start / end snapshot.
    pub start: Totals,
    pub end: Totals,
    /// Earliest start snapshot / latest end snapshot, epoch seconds.
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    /// Some account starts at its first snapshot inside the period.
    pub partial_start: bool,
    /// `end.net_worth - start.net_worth`; None with `reason`.
    pub change: Option<Money>,
    /// Change as a share of the starting net worth, basis points; None when
    /// that is not positive.
    pub change_bp: Option<i64>,
    pub reason: Option<ChangeReason>,
    /// Sum of measurable investment accounts' gain.
    pub markets: Option<Money>,
    /// `change - markets`, exactly: saved from income minus spending, plus
    /// every unmeasured account's change.
    pub added: Option<Money>,
    /// Investment accounts whose gain is unknown, named.
    pub excluded: Vec<ExcludedAccount>,
    /// True when `excluded` is non-empty: Added then includes their market
    /// move, and the hero must say so.
    pub added_includes_unmeasured: bool,
    /// Accounts with at least one snapshot, in input order.
    pub accounts: Vec<AccountChange>,
    pub contributed: Vec<ContributedPoint>,
    /// False when some contribution couldn't be valued or some account is
    /// excluded (its unrecorded contributions are missing from the line).
    pub contributed_complete: bool,
}

/// Change over `[since, until)` (epoch seconds; None = open). `accounts` is
/// the visible set; snapshots and activity for other accounts are ignored.
pub fn net_worth_change(
    accounts: &[Account],
    securities: &[Security],
    balances: &[BalanceSnapshot],
    holdings: &[HoldingSnapshot],
    activity: &[InvestmentTxn],
    since: Option<i64>,
    until: Option<i64>,
) -> NetWorthChange {
    let since = since.unwrap_or(i64::MIN);
    let until = until.unwrap_or(i64::MAX);
    let cash_secs: HashSet<&str> = securities
        .iter()
        .filter(|s| s.is_cash)
        .map(|s| s.security_id.as_str())
        .collect();
    let live = live_activity(activity);

    let mut out = NetWorthChange {
        start: Totals::default(),
        end: Totals::default(),
        start_at: None,
        end_at: None,
        partial_start: false,
        change: None,
        change_bp: None,
        reason: None,
        markets: None,
        added: None,
        excluded: Vec::new(),
        added_includes_unmeasured: false,
        accounts: Vec::new(),
        contributed: Vec::new(),
        contributed_complete: true,
    };
    let mut any_change = false;
    let mut markets = 0i64;
    // Snapshots that feed the chart, and dated contributions for it.
    let mut series_snaps: Vec<BalanceSnapshot> = Vec::new();
    let mut series_kinds = std::collections::HashMap::new();
    let mut dated: Vec<(NaiveDate, i64)> = Vec::new();

    for a in accounts {
        let mut snaps: Vec<&BalanceSnapshot> = balances
            .iter()
            .filter(|s| s.account_id == a.id && s.taken_at < until)
            .collect();
        snaps.sort_by_key(|s| s.taken_at);
        let Some(&end) = snaps.last() else { continue };
        let before = snaps.iter().rev().find(|s| s.taken_at <= since).copied();
        let partial = before.is_none();
        let start = before.unwrap_or(snaps[0]);
        let group = Group::of(a.kind);
        out.start.add(group, start.balance, start.cash);
        out.end.add(group, end.balance, end.cash);
        out.start_at = Some(
            out.start_at
                .map_or(start.taken_at, |t| t.min(start.taken_at)),
        );
        out.end_at = Some(out.end_at.map_or(end.taken_at, |t| t.max(end.taken_at)));
        out.partial_start |= partial;

        let label = account_display_label(&a.name, a.nickname.as_deref());
        let mut row = AccountChange {
            account_id: a.id.clone(),
            account: label.clone(),
            group,
            start: start.balance,
            end: end.balance,
            start_at: start.taken_at,
            end_at: end.taken_at,
            partial_start: partial,
            change: None,
            change_reason: None,
            contributions: None,
            gain: None,
            gain_reason: None,
        };
        if start.taken_at == end.taken_at {
            row.change_reason = Some(ChangeReason::OneSnapshot);
        } else {
            any_change = true;
            let change = end.balance.cents() - start.balance.cents();
            row.change = Some(Money::from_cents(change));
            if group == Group::Investments {
                let m = measure(
                    &a.id,
                    start.taken_at,
                    end.taken_at,
                    holdings,
                    &cash_secs,
                    &live,
                );
                row.contributions = m.contributions.map(Money::from_cents);
                if m.dated.len() < m.rows {
                    out.contributed_complete = false;
                }
                dated.extend(m.dated);
                match m.outcome {
                    Ok(()) => match m.contributions {
                        Some(c) => {
                            row.gain = Some(Money::from_cents(change - c));
                            markets += change - c;
                        }
                        None => row.gain_reason = Some(GainReason::ContributionsUnknown),
                    },
                    Err(r) => row.gain_reason = Some(r),
                }
                if row.gain_reason == Some(GainReason::ContributionsUnknown) {
                    out.excluded.push(ExcludedAccount {
                        account_id: a.id.clone(),
                        account: label,
                        change: Money::from_cents(change),
                    });
                }
            }
        }
        series_kinds.insert(a.id.clone(), a.kind);
        series_snaps.extend(
            snaps
                .iter()
                .filter(|s| s.taken_at >= start.taken_at)
                .map(|s| (*s).clone()),
        );
        out.accounts.push(row);
    }

    if out.accounts.is_empty() {
        out.reason = Some(ChangeReason::NoSnapshots);
        out.contributed_complete = false;
        return out;
    }
    if !any_change {
        out.reason = Some(ChangeReason::OneSnapshot);
        return out;
    }
    let change = out.end.net_worth.cents() - out.start.net_worth.cents();
    out.change = Some(Money::from_cents(change));
    let base = out.start.net_worth.cents();
    out.change_bp = (base > 0).then(|| ((change as i128 * 10_000) / base as i128) as i64);
    out.markets = Some(Money::from_cents(markets));
    out.added = Some(Money::from_cents(change - markets));
    out.added_includes_unmeasured = !out.excluded.is_empty();
    if out.added_includes_unmeasured {
        out.contributed_complete = false;
    }

    let days = crate::balances::history(&series_snaps, &series_kinds);
    if let Some(first) = days.first() {
        let base = first.invested.cents();
        dated.sort();
        let mut i = 0;
        let mut sum = 0i64;
        for d in &days {
            let day = NaiveDate::parse_from_str(&d.day, "%Y-%m-%d").ok();
            while i < dated.len() && day.is_some_and(|day| dated[i].0 <= day) {
                sum += dated[i].1;
                i += 1;
            }
            out.contributed.push(ContributedPoint {
                day: d.day.clone(),
                invested: d.invested,
                contributed: Money::from_cents(base + sum),
            });
        }
    }
    out
}

/// Activity that happened: drops `cancel` rows and the rows they cancel.
fn live_activity(activity: &[InvestmentTxn]) -> Vec<&InvestmentTxn> {
    let cancelled: HashSet<&str> = activity
        .iter()
        .filter_map(|t| t.cancel_txn_id.as_deref())
        .collect();
    activity
        .iter()
        .filter(|t| t.txn_type != "cancel" && !cancelled.contains(t.id.as_str()))
        .collect()
}

struct Measured {
    /// Net contributions in the window; None when one couldn't be valued.
    contributions: Option<i64>,
    /// Each valued contribution with its date (for the chart).
    dated: Vec<(NaiveDate, i64)>,
    /// How many contribution rows the window has (valued or not).
    rows: usize,
    /// Ok = measurable (rule C holds and not cash).
    outcome: Result<(), GainReason>,
}

/// Non-cash positions (security -> 1e-8 share units) in this account's
/// holdings snapshot at or before `ts`. None when there is none; Some(None)
/// inside when a quantity doesn't parse.
fn positions_at<'a>(
    mine: &[&'a HoldingSnapshot],
    ts: i64,
    cash_secs: &HashSet<&str>,
) -> Option<Option<BTreeMap<&'a str, i128>>> {
    let at = mine.iter().map(|h| h.taken_at).filter(|t| *t <= ts).max()?;
    let mut m = BTreeMap::new();
    for h in mine.iter().filter(|h| h.taken_at == at) {
        if cash_secs.contains(h.security_id.as_str()) {
            continue;
        }
        let Some(q) = qty_units(&h.quantity) else {
            return Some(None);
        };
        m.insert(h.security_id.as_str(), q);
    }
    Some(Some(m))
}

fn measure(
    account_id: &str,
    start_at: i64,
    end_at: i64,
    holdings: &[HoldingSnapshot],
    cash_secs: &HashSet<&str>,
    live: &[&InvestmentTxn],
) -> Measured {
    let mut mine: Vec<&HoldingSnapshot> = holdings
        .iter()
        .filter(|h| h.account_id == account_id)
        .collect();
    mine.sort_by_key(|h| h.taken_at);
    let (Some(sd), Some(ed)) = (local_day(start_at), local_day(end_at)) else {
        return Measured {
            contributions: None,
            dated: Vec::new(),
            rows: 0,
            outcome: Err(GainReason::ContributionsUnknown),
        };
    };
    let window: Vec<(&InvestmentTxn, NaiveDate)> = live
        .iter()
        .filter(|t| t.account_id == account_id)
        .filter_map(|t| {
            let d = NaiveDate::parse_from_str(&t.date, "%Y-%m-%d").ok()?;
            (sd < d && d <= ed).then_some((*t, d))
        })
        .collect();

    // Contributions (rules D, E).
    let mut total = Some(0i64);
    let mut dated = Vec::new();
    let mut rows = 0;
    for (t, d) in &window {
        match contribution(t, *d, &mine) {
            Contribution::None => {}
            Contribution::Valued(c) => {
                rows += 1;
                total = total.map(|x| x + c);
                dated.push((*d, c));
            }
            Contribution::Unvalued => {
                rows += 1;
                total = None;
            }
        }
    }

    let outcome = (|| {
        let start = positions_at(&mine, start_at, cash_secs);
        let end = positions_at(&mine, end_at, cash_secs);
        let held = |p: &Option<Option<BTreeMap<&str, i128>>>| {
            p.as_ref()
                .is_some_and(|m| m.as_ref().is_none_or(|m| !m.is_empty()))
        };
        if !held(&start) && !held(&end) {
            return Err(GainReason::Cash); // rule B
        }
        let unknown = Err(GainReason::ContributionsUnknown);
        let (Some(Some(start)), Some(Some(end))) = (start, end) else {
            return unknown;
        };
        // Rule C: every non-cash position's change is explained by activity.
        let mut moved: BTreeMap<&str, i128> = BTreeMap::new();
        for (t, _) in &window {
            let Some(sec) = t.security_id.as_deref() else {
                continue;
            };
            if cash_secs.contains(sec) {
                continue;
            }
            let Some(q) = qty_units(&t.quantity) else {
                return unknown;
            };
            *moved.entry(sec).or_default() += q;
        }
        let secs: BTreeSet<&str> = start
            .keys()
            .chain(end.keys())
            .chain(moved.keys())
            .copied()
            .collect();
        for s in secs {
            let gap =
                end.get(s).unwrap_or(&0) - start.get(s).unwrap_or(&0) - moved.get(s).unwrap_or(&0);
            if gap.abs() > QTY_TOLERANCE {
                return unknown;
            }
        }
        Ok(())
    })();
    Measured {
        contributions: total,
        dated,
        rows,
        outcome,
    }
}

enum Contribution {
    /// Not external money (trade, dividend, fee, interest...).
    None,
    Valued(i64),
    /// A share transfer with no recorded price on or after its date.
    Unvalued,
}

fn contribution(t: &InvestmentTxn, date: NaiveDate, mine: &[&HoldingSnapshot]) -> Contribution {
    let cash = t.amount.cents();
    if t.txn_type == "transfer" || t.subtype == "transfer" {
        if cash != 0 {
            return Contribution::Valued(cash);
        }
        let Some(q) = qty_units(&t.quantity).filter(|q| *q != 0) else {
            return Contribution::None;
        };
        // Rule D: the first recorded price on or after the transfer date.
        let price = t.security_id.as_deref().and_then(|sec| {
            mine.iter()
                .filter(|h| h.security_id == sec)
                .filter(|h| local_day(h.taken_at).is_some_and(|d| d >= date))
                .find_map(|h| h.price.as_deref())
        });
        return match price.and_then(|p| value_cents(q, p)) {
            Some(v) => Contribution::Valued(v),
            None => Contribution::Unvalued,
        };
    }
    match (t.txn_type.as_str(), t.subtype.as_str()) {
        ("buy", "contribution") => Contribution::Valued(cash.abs()),
        (_, "contribution" | "deposit" | "withdrawal") => Contribution::Valued(cash),
        _ => Contribution::None,
    }
}

/// `n / d` rounded half away from zero (`d > 0`).
fn round_div(n: i128, d: i128) -> i128 {
    let q = (n.abs() + d / 2) / d;
    if n < 0 {
        -q
    } else {
        q
    }
}

/// Decimal share text -> 1e-8 share units, exact (rounded past 8 places).
fn qty_units(s: &str) -> Option<i128> {
    let (n, scale) = parse_decimal(s)?;
    if scale <= 8 {
        n.checked_mul(10i128.pow(8 - scale))
    } else {
        Some(round_div(n, 10i128.checked_pow(scale - 8)?))
    }
}

/// Quantity (1e-8 units) x decimal price -> cents, rounded half away from
/// zero; None on unparseable price or overflow.
fn value_cents(qty: i128, price: &str) -> Option<i64> {
    let (p, scale) = parse_decimal(price)?;
    let num = qty.checked_mul(p)?.checked_mul(100)?;
    let den = QTY_SCALE.checked_mul(10i128.checked_pow(scale)?)?;
    i64::try_from(round_div(num, den)).ok()
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
mod tests {
    use super::*;
    use crate::model::AccountKind;
    use chrono::{Local, TimeZone};

    fn acct(id: &str, kind: AccountKind) -> Account {
        Account {
            id: id.into(),
            org: "Bank".into(),
            name: format!("{id} (0000)"),
            kind,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
        }
    }

    fn at(m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, m, d, 12, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn midnight(m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, m, d, 0, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }

    /// November 2026.
    fn nov() -> (Option<i64>, Option<i64>) {
        (Some(midnight(11, 1)), Some(midnight(12, 1)))
    }

    fn bal(ts: i64, id: &str, cents: i64, cash: i64) -> BalanceSnapshot {
        BalanceSnapshot {
            taken_at: ts,
            account_id: id.into(),
            balance: Money::from_cents(cents),
            cash: Money::from_cents(cash),
        }
    }

    fn pos(ts: i64, id: &str, sec: &str, qty: &str, price: &str) -> HoldingSnapshot {
        HoldingSnapshot {
            taken_at: ts,
            account_id: id.into(),
            security_id: sec.into(),
            quantity: qty.into(),
            price: Some(price.into()),
            value: Money::from_cents(0),
            cost_basis: None,
        }
    }

    fn sec(id: &str, is_cash: bool) -> Security {
        Security {
            security_id: id.into(),
            ticker: Some(id.to_uppercase()),
            name: id.into(),
            sec_type: if is_cash { "cash" } else { "etf" }.into(),
            is_cash,
            cusip: None,
            isin: None,
            close_price: None,
            close_price_as_of: None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn txn(
        id: &str,
        acct: &str,
        sec: Option<&str>,
        date: &str,
        ty: &str,
        sub: &str,
        qty: &str,
        cents: i64,
    ) -> InvestmentTxn {
        InvestmentTxn {
            id: id.into(),
            account_id: acct.into(),
            security_id: sec.map(Into::into),
            date: date.into(),
            name: id.into(),
            txn_type: ty.into(),
            subtype: sub.into(),
            quantity: qty.into(),
            price: "0".into(),
            amount: Money::from_cents(cents),
            fees: None,
            currency: None,
            cancel_txn_id: None,
        }
    }

    fn secs() -> Vec<Security> {
        vec![sec("vti", false), sec("abc", false), sec("vmfxx", true)]
    }

    fn find<'a>(c: &'a NetWorthChange, id: &str) -> &'a AccountChange {
        c.accounts.iter().find(|a| a.account_id == id).unwrap()
    }

    fn cents(m: Option<Money>) -> Option<i64> {
        m.map(|m| m.cents())
    }

    /// Checking, a measurable brokerage (dividend reinvested), a cash-only
    /// HSA and a card: markets + added == change, exactly.
    #[test]
    fn markets_plus_added_is_change() {
        let accounts = [
            acct("chk", AccountKind::Checking),
            acct("brk", AccountKind::Investment),
            acct("hsa", AccountKind::Investment),
            acct("card", AccountKind::Credit),
        ];
        let (s, e) = (at(10, 31), at(11, 30));
        let balances = [
            bal(s, "chk", 5_000_00, 5_000_00),
            bal(s, "brk", 10_000_00, 0),
            bal(s, "hsa", 3_000_00, 3_000_00),
            bal(s, "card", -400_00, 0),
            bal(e, "chk", 5_600_00, 5_600_00),
            bal(e, "brk", 10_550_00, 0),
            bal(e, "hsa", 3_200_00, 3_200_00),
            bal(e, "card", -700_00, 0),
        ];
        let holdings = [
            pos(s, "brk", "vti", "40", "250"),
            pos(e, "brk", "vti", "40.2", "262"),
        ];
        let activity = [
            // Dividend paid in cash, then reinvested: internal, nets to 0.
            txn(
                "d1",
                "brk",
                Some("vti"),
                "2026-11-10",
                "cash",
                "dividend",
                "0",
                50_00,
            ),
            txn(
                "r1",
                "brk",
                Some("vti"),
                "2026-11-10",
                "buy",
                "dividend reinvestment",
                "0.2",
                -50_00,
            ),
            // $200 deposited: a contribution, not gain.
            txn(
                "c1",
                "brk",
                None,
                "2026-11-12",
                "cash",
                "deposit",
                "0",
                200_00,
            ),
        ];
        let (since, until) = nov();
        let c = net_worth_change(
            &accounts,
            &secs(),
            &balances,
            &holdings,
            &activity,
            since,
            until,
        );
        assert_eq!(cents(c.change), Some(18_650_00 - 17_600_00));
        assert_eq!(c.start.net_worth.cents(), 17_600_00);
        assert_eq!(c.end.net_worth.cents(), 18_650_00);
        assert_eq!(c.reason, None);
        assert!(!c.partial_start);
        let brk = find(&c, "brk");
        assert_eq!(cents(brk.contributions), Some(200_00));
        assert_eq!(cents(brk.gain), Some(550_00 - 200_00));
        assert_eq!(find(&c, "hsa").gain_reason, Some(GainReason::Cash));
        assert_eq!(find(&c, "chk").gain, None);
        assert_eq!(find(&c, "chk").gain_reason, None);
        assert_eq!(cents(c.markets), Some(350_00));
        assert_eq!(
            c.markets.unwrap().cents() + c.added.unwrap().cents(),
            c.change.unwrap().cents()
        );
        assert!(c.excluded.is_empty());
        assert!(!c.added_includes_unmeasured);
        // Breakdown still sums at both ends.
        for t in [c.start, c.end] {
            let sum = t.cash.cents()
                + t.invested.cents()
                + t.credit.cents()
                + t.loans.cents()
                + t.other.cents();
            assert_eq!(sum, t.net_worth.cents());
        }
        assert_eq!(c.change_bp, Some(1_050_00 * 10_000 / 17_600_00));
    }

    /// A lot-merge artifact or unreported payroll buys: shares appear with no
    /// activity. Gain unknown, account excluded and named, its change in Added.
    #[test]
    fn quantity_jump_without_activity_is_excluded() {
        let accounts = [acct("401k", AccountKind::Investment)];
        let (s, e) = (at(10, 31), at(11, 30));
        let balances = [bal(s, "401k", 1_000_00, 0), bal(e, "401k", 4_000_00, 0)];
        let holdings = [
            pos(s, "401k", "vti", "4", "250"),
            pos(e, "401k", "vti", "16", "250"),
        ];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &secs(), &balances, &holdings, &[], since, until);
        let k = find(&c, "401k");
        assert_eq!(k.gain, None);
        assert_eq!(k.gain_reason, Some(GainReason::ContributionsUnknown));
        assert_eq!(c.excluded.len(), 1);
        assert_eq!(c.excluded[0].account, "401k (0000)");
        assert_eq!(c.excluded[0].change.cents(), 3_000_00);
        assert!(c.added_includes_unmeasured);
        assert_eq!(cents(c.markets), Some(0));
        assert_eq!(cents(c.added), Some(3_000_00));
        assert!(!c.contributed_complete);
    }

    /// Rounding within 0.001 share is not a jump.
    #[test]
    fn small_quantity_rounding_is_tolerated() {
        let accounts = [acct("brk", AccountKind::Investment)];
        let (s, e) = (at(10, 31), at(11, 30));
        let balances = [bal(s, "brk", 1_000_00, 0), bal(e, "brk", 1_010_00, 0)];
        let holdings = [
            pos(s, "brk", "vti", "4.0000", "250"),
            pos(e, "brk", "vti", "4.1234", "250"),
        ];
        let activity = [
            txn(
                "r1",
                "brk",
                Some("vti"),
                "2026-11-03",
                "buy",
                "buy",
                "0.06172",
                -15_00,
            ),
            txn(
                "r2",
                "brk",
                Some("vti"),
                "2026-11-04",
                "buy",
                "buy",
                "0.06172",
                -15_00,
            ),
        ];
        let (since, until) = nov();
        let c = net_worth_change(
            &accounts,
            &secs(),
            &balances,
            &holdings,
            &activity,
            since,
            until,
        );
        assert_eq!(cents(find(&c, "brk").gain), Some(10_00));
    }

    /// 120 shares moved in from another brokerage: a contribution valued at
    /// the first snapshot price on or after the transfer date (rule D).
    #[test]
    fn in_kind_share_transfer_is_a_contribution_at_snapshot_price() {
        let accounts = [acct("brk", AccountKind::Investment)];
        let (s, mid, e) = (at(10, 31), at(11, 6), at(11, 30));
        let balances = [
            bal(s, "brk", 1_000_00, 0),
            bal(mid, "brk", 4_800_00, 0),
            bal(e, "brk", 4_900_00, 0),
        ];
        let holdings = [
            pos(s, "brk", "vti", "4", "250"),
            pos(mid, "brk", "vti", "4", "250"),
            pos(mid, "brk", "abc", "120", "31.37"),
            pos(e, "brk", "vti", "4", "250"),
            pos(e, "brk", "abc", "120", "32.10"),
        ];
        let activity = [txn(
            "t1",
            "brk",
            Some("abc"),
            "2026-11-05",
            "transfer",
            "transfer",
            "120",
            0,
        )];
        let (since, until) = nov();
        let c = net_worth_change(
            &accounts,
            &secs(),
            &balances,
            &holdings,
            &activity,
            since,
            until,
        );
        let brk = find(&c, "brk");
        // 120 x 31.37 = 3,764.40.
        assert_eq!(cents(brk.contributions), Some(3_764_40));
        assert_eq!(cents(brk.gain), Some(3_900_00 - 3_764_40));
    }

    /// A share transfer with no recorded price afterwards: unknown, never
    /// estimated.
    #[test]
    fn share_transfer_without_a_price_is_unknown() {
        let accounts = [acct("brk", AccountKind::Investment)];
        let (s, e) = (at(10, 31), at(11, 30));
        let balances = [bal(s, "brk", 1_000_00, 0), bal(e, "brk", 2_000_00, 0)];
        let holdings = [
            pos(s, "brk", "vti", "4", "250"),
            pos(e, "brk", "vti", "8", "250"),
        ];
        let activity = [txn(
            "t1",
            "brk",
            Some("vti"),
            "2026-11-30",
            "transfer",
            "transfer",
            "4",
            0,
        )];
        let mut h = holdings.to_vec();
        h[1].price = None;
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &secs(), &balances, &h, &activity, since, until);
        assert_eq!(
            find(&c, "brk").gain_reason,
            Some(GainReason::ContributionsUnknown)
        );
    }

    /// A custodian whose holdings price never moves while the balance does. Value
    /// comes from the balance (rule A), so the market move is gain.
    #[test]
    fn stale_holdings_price_still_measures_from_balance() {
        let accounts = [acct("emp", AccountKind::Investment)];
        let (s, e) = (at(10, 31), at(11, 30));
        let balances = [bal(s, "emp", 50_000_00, 0), bal(e, "emp", 51_250_00, 0)];
        let holdings = [
            pos(s, "emp", "fund", "1000", "50"),
            pos(e, "emp", "fund", "1000", "50"),
        ];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &secs(), &balances, &holdings, &[], since, until);
        assert_eq!(cents(find(&c, "emp").gain), Some(1_250_00));
        assert_eq!(cents(c.markets), Some(1_250_00));
        assert_eq!(cents(c.added), Some(0));
    }

    /// A money-market sweep moves with every deposit and isn't a position:
    /// only non-cash securities are checked, and an account holding only
    /// cash securities is cash.
    #[test]
    fn cash_securities_are_cash_not_positions() {
        let accounts = [acct("ira", AccountKind::Investment)];
        let (s, e) = (at(10, 31), at(11, 30));
        let balances = [
            bal(s, "ira", 2_000_00, 2_000_00),
            bal(e, "ira", 2_500_00, 2_500_00),
        ];
        let holdings = [
            pos(s, "ira", "vmfxx", "2000", "1"),
            pos(e, "ira", "vmfxx", "2500", "1"),
        ];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &secs(), &balances, &holdings, &[], since, until);
        assert_eq!(find(&c, "ira").gain_reason, Some(GainReason::Cash));
        assert_eq!(cents(c.added), Some(500_00));
    }

    /// Payroll buying shares directly counts its full cost; a cancelled row
    /// and its cancel are skipped.
    #[test]
    fn payroll_buy_counts_and_cancelled_rows_skip() {
        let accounts = [acct("401k", AccountKind::Investment)];
        let (s, e) = (at(10, 31), at(11, 30));
        let balances = [bal(s, "401k", 1_000_00, 0), bal(e, "401k", 1_560_00, 0)];
        let holdings = [
            pos(s, "401k", "vti", "4", "250"),
            pos(e, "401k", "vti", "6", "260"),
        ];
        let mut cancel = txn(
            "x2",
            "401k",
            None,
            "2026-11-20",
            "cancel",
            "cancel",
            "0",
            -999_00,
        );
        cancel.cancel_txn_id = Some("x1".into());
        let activity = [
            txn(
                "p1",
                "401k",
                Some("vti"),
                "2026-11-15",
                "buy",
                "contribution",
                "2",
                -500_00,
            ),
            txn(
                "x1",
                "401k",
                None,
                "2026-11-19",
                "cash",
                "deposit",
                "0",
                999_00,
            ),
            cancel,
        ];
        let (since, until) = nov();
        let c = net_worth_change(
            &accounts,
            &secs(),
            &balances,
            &holdings,
            &activity,
            since,
            until,
        );
        let k = find(&c, "401k");
        assert_eq!(cents(k.contributions), Some(500_00));
        assert_eq!(cents(k.gain), Some(60_00));
    }

    /// Activity on the start snapshot's day is already in the start.
    #[test]
    fn activity_on_the_start_day_is_outside_the_window() {
        let accounts = [acct("brk", AccountKind::Investment)];
        let (s, e) = (at(11, 2), at(11, 30));
        let balances = [bal(s, "brk", 1_000_00, 0), bal(e, "brk", 1_000_00, 0)];
        let holdings = [
            pos(s, "brk", "vti", "4", "250"),
            pos(e, "brk", "vti", "4", "250"),
        ];
        let activity = [txn(
            "b0",
            "brk",
            Some("vti"),
            "2026-11-02",
            "buy",
            "buy",
            "1",
            -250_00,
        )];
        let c = net_worth_change(
            &accounts,
            &secs(),
            &balances,
            &holdings,
            &activity,
            Some(midnight(11, 1)),
            Some(midnight(12, 1)),
        );
        assert!(find(&c, "brk").gain.is_some());
    }

    /// No snapshot before the period: start is the first inside, flagged.
    #[test]
    fn partial_start_is_flagged() {
        let accounts = [acct("chk", AccountKind::Checking)];
        let balances = [
            bal(at(11, 5), "chk", 100_00, 100_00),
            bal(at(11, 20), "chk", 250_00, 250_00),
        ];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &[], &balances, &[], &[], since, until);
        assert!(c.partial_start);
        assert!(find(&c, "chk").partial_start);
        assert_eq!(cents(c.change), Some(150_00));
        assert_eq!(c.start_at, Some(at(11, 5)));
    }

    /// One snapshot: no change, with the reason; none at all likewise.
    #[test]
    fn one_snapshot_gives_no_change() {
        let accounts = [acct("chk", AccountKind::Checking)];
        let balances = [bal(at(11, 5), "chk", 100_00, 100_00)];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &[], &balances, &[], &[], since, until);
        assert_eq!(c.change, None);
        assert_eq!(c.reason, Some(ChangeReason::OneSnapshot));
        assert_eq!(c.markets, None);
        assert_eq!(c.added, None);
        assert_eq!(
            find(&c, "chk").change_reason,
            Some(ChangeReason::OneSnapshot)
        );
        assert_eq!(c.end.net_worth.cents(), 100_00);

        let none = net_worth_change(&accounts, &[], &[], &[], &[], since, until);
        assert_eq!(none.reason, Some(ChangeReason::NoSnapshots));
        assert!(none.accounts.is_empty());
    }

    /// An account linked mid-period with one snapshot counts on both ends
    /// (change 0), so it never reads as a gain; others still change.
    #[test]
    fn single_snapshot_account_adds_nothing() {
        let accounts = [
            acct("chk", AccountKind::Checking),
            acct("new", AccountKind::Savings),
        ];
        let balances = [
            bal(at(10, 31), "chk", 100_00, 100_00),
            bal(at(11, 30), "chk", 300_00, 300_00),
            bal(at(11, 30), "new", 9_000_00, 9_000_00),
        ];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &[], &balances, &[], &[], since, until);
        assert_eq!(cents(c.change), Some(200_00));
        assert_eq!(find(&c, "new").change, None);
        assert!(c.partial_start);
    }

    /// Snapshots of accounts outside the visible set are ignored.
    #[test]
    fn hidden_accounts_stay_out() {
        let accounts = [acct("chk", AccountKind::Checking)];
        let balances = [
            bal(at(10, 31), "chk", 100_00, 100_00),
            bal(at(11, 30), "chk", 300_00, 300_00),
            bal(at(10, 31), "hidden", 0, 0),
            bal(at(11, 30), "hidden", 50_000_00, 0),
        ];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &[], &balances, &[], &[], since, until);
        assert_eq!(cents(c.change), Some(200_00));
        assert_eq!(c.accounts.len(), 1);
    }

    /// The chart series: invested per day, and invested-at-start plus
    /// cumulative contributions.
    #[test]
    fn contributed_series_accumulates_contributions() {
        let accounts = [
            acct("brk", AccountKind::Investment),
            acct("chk", AccountKind::Checking),
        ];
        let balances = [
            bal(at(10, 31), "brk", 1_000_00, 1_000_00),
            bal(at(10, 31), "chk", 77_00, 77_00),
            bal(at(11, 2), "brk", 1_300_00, 1_300_00),
        ];
        let activity = [txn(
            "c1",
            "brk",
            None,
            "2026-11-01",
            "cash",
            "contribution",
            "0",
            250_00,
        )];
        let (since, until) = nov();
        let c = net_worth_change(&accounts, &secs(), &balances, &[], &activity, since, until);
        let pts: Vec<(&str, i64, i64)> = c
            .contributed
            .iter()
            .map(|p| (p.day.as_str(), p.invested.cents(), p.contributed.cents()))
            .collect();
        assert_eq!(
            pts,
            [
                ("2026-10-31", 1_000_00, 1_000_00),
                ("2026-11-01", 1_000_00, 1_250_00),
                ("2026-11-02", 1_300_00, 1_250_00),
            ]
        );
        assert!(c.contributed_complete);
    }

    #[test]
    fn quantities_and_values_use_exact_integers() {
        assert_eq!(qty_units("1"), Some(QTY_SCALE));
        assert_eq!(qty_units("-0.5"), Some(-QTY_SCALE / 2));
        assert_eq!(qty_units("0.123456789"), Some(12_345_679)); // half away from zero
        assert_eq!(qty_units("lots"), None);
        assert_eq!(
            value_cents(qty_units("120").unwrap(), "31.37"),
            Some(3_764_40)
        );
        assert_eq!(
            value_cents(qty_units("0.333").unwrap(), "10.005"),
            Some(3_33)
        );
        assert_eq!(value_cents(qty_units("-2").unwrap(), "1.005"), Some(-2_01));
    }
}
