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

use crate::balances::{local_day, parse_decimal, Group, Totals, UNINVESTED_CASH_ID};
use crate::model::{
    account_display_label, Account, AccountKind, BalanceSnapshot, HoldingSnapshot, InvestmentTxn,
    Security,
};
use crate::money::Money;
use chrono::NaiveDate;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

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
            item_id: None,
            kind_from_bank: None,
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

// ---------------------------------------------------------------------------
// Positions over a period (ho-2mj.4.2)
// ---------------------------------------------------------------------------

/// How position rows are keyed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PositionLens {
    /// One row per position per account.
    #[default]
    Account,
    /// The same security combined across accounts: keyed on ticker, else
    /// security id (so un-tickered instruments like warrants stay apart).
    Position,
}

/// One position over a period. Values come from holdings snapshots, priced
/// as of `priced_at`. Position gains do NOT sum to the account's gain: a
/// reinvested dividend's buy counts as a contribution here, while at account
/// level it nets to zero (and positions and balance are priced at different
/// moments).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PositionChange {
    /// The account ids behind this row (one, unless combined).
    pub account_ids: Vec<String>,
    /// Display label(s), comma-joined when combined.
    pub account: String,
    pub security_id: String,
    pub ticker: Option<String>,
    /// Ticker, else security name, else id.
    pub label: String,
    pub name: String,
    pub is_cash: bool,
    /// Exact decimal text; "0" when the position wasn't held.
    pub start_quantity: String,
    pub end_quantity: String,
    /// End snapshot price (exact decimal text); None once sold or on
    /// snapshots without a price.
    pub price: Option<String>,
    /// Epoch seconds of the end snapshot the value is priced at.
    pub priced_at: Option<i64>,
    /// The institution's own price date, when the end snapshot is the
    /// current holdings sync (a frozen price shows here). Oldest when
    /// combined.
    pub price_as_of: Option<String>,
    pub start_value: Money,
    pub end_value: Money,
    /// Buys - sells + shares transferred in (at snapshot price). None when a
    /// transfer has no recorded price.
    pub contributions: Option<Money>,
    /// Gain net of contributions = end - start - contributions.
    pub gain: Option<Money>,
    /// Gain over start value plus net contributions (when positive), in
    /// basis points.
    pub gain_bp: Option<i64>,
    pub gain_reason: Option<GainReason>,
    /// Start and end are the same snapshot.
    pub change_reason: Option<ChangeReason>,
    /// The account's start is its first holdings snapshot inside the period.
    pub partial_start: bool,
    /// Held at the start or traded in the period, gone by the end.
    pub sold: bool,
    /// End snapshot basis; None when the broker gives none (or any
    /// combined part lacks one).
    pub cost_basis: Option<Money>,
    /// `end_value - cost_basis`.
    pub unrealized_gain: Option<Money>,
    pub unrealized_bp: Option<i64>,
}

type SyncedPriceDate<'a> = (Option<i64>, Option<&'a str>);

/// Every position over `[since, until)` on `accounts` (the visible set),
/// filtered, largest end value first. `current` is the live holdings table
/// with sync times (`Store::current_holdings_synced`), used only for the
/// institution's price date.
#[allow(clippy::too_many_arguments)]
pub fn position_change(
    accounts: &[Account],
    securities: &[Security],
    holdings: &[HoldingSnapshot],
    balances: &[BalanceSnapshot],
    current: &[(crate::model::Holding, Option<i64>)],
    activity: &[InvestmentTxn],
    since: Option<i64>,
    until: Option<i64>,
    filter: &crate::balances::PositionFilter,
    lens: PositionLens,
) -> Vec<PositionChange> {
    let since = since.unwrap_or(i64::MIN);
    let until = until.unwrap_or(i64::MAX);
    let secs: HashMap<&str, &Security> = securities
        .iter()
        .map(|s| (s.security_id.as_str(), s))
        .collect();
    let live = live_activity(activity);
    // (account, security) -> (sync time, institution price date).
    let as_of: HashMap<(&str, &str), SyncedPriceDate> = current
        .iter()
        .map(|(h, synced)| {
            (
                (h.account_id.as_str(), h.security_id.as_str()),
                (*synced, h.price_as_of.as_deref()),
            )
        })
        .collect();

    let mut rows = Vec::new();
    for a in accounts {
        let mut mine: Vec<&HoldingSnapshot> = holdings
            .iter()
            .filter(|h| h.account_id == a.id && h.taken_at < until)
            .collect();
        mine.sort_by_key(|h| h.taken_at);
        let times: BTreeSet<i64> = mine.iter().map(|h| h.taken_at).collect();
        let (Some(&first), Some(&end_t)) = (times.first(), times.last()) else {
            continue;
        };
        let before = times.range(..=since).next_back().copied();
        let partial = before.is_none();
        let start_t = before.unwrap_or(first);
        let set_at = |t: i64| -> BTreeMap<&str, &HoldingSnapshot> {
            mine.iter()
                .filter(|h| h.taken_at == t)
                .map(|h| (h.security_id.as_str(), *h))
                .collect()
        };
        let (start, end) = (set_at(start_t), set_at(end_t));
        let one = start_t == end_t;
        let window: Vec<(&InvestmentTxn, NaiveDate)> = match (local_day(start_t), local_day(end_t))
        {
            (Some(sd), Some(ed)) if !one => live
                .iter()
                .filter(|t| t.account_id == a.id)
                .filter_map(|t| {
                    let d = NaiveDate::parse_from_str(&t.date, "%Y-%m-%d").ok()?;
                    (sd < d && d <= ed).then_some((*t, d))
                })
                .collect(),
            _ => Vec::new(),
        };
        let ids: BTreeSet<&str> = start
            .keys()
            .chain(end.keys())
            .copied()
            .chain(window.iter().filter_map(|(t, _)| t.security_id.as_deref()))
            .collect();
        let label = account_display_label(&a.name, a.nickname.as_deref());

        for id in ids {
            let sec = secs.get(id).copied();
            let ticker = sec.and_then(|s| s.ticker.clone());
            if !filter.keeps(&a.id, &label, id, ticker.as_deref()) {
                continue;
            }
            let (s0, e0) = (start.get(id).copied(), end.get(id).copied());
            let start_value = s0.map_or(0, |h| h.value.cents());
            let end_value = e0.map_or(0, |h| h.value.cents());
            let is_cash = sec.is_some_and(|s| s.is_cash);
            let rows_for: Vec<&(&InvestmentTxn, NaiveDate)> = window
                .iter()
                .filter(|(t, _)| t.security_id.as_deref() == Some(id))
                .collect();

            let mut contributions = None;
            let mut gain = None;
            let mut gain_reason = None;
            let change_reason = one.then_some(ChangeReason::OneSnapshot);
            if is_cash {
                gain_reason = Some(GainReason::Cash);
            } else if !one {
                let flows: Option<i64> = rows_for
                    .iter()
                    .map(|(t, d)| position_flow(t, *d, &mine))
                    .sum();
                let q = |h: Option<&HoldingSnapshot>| h.map_or(Some(0), |h| qty_units(&h.quantity));
                let moved: Option<i128> =
                    rows_for.iter().map(|(t, _)| qty_units(&t.quantity)).sum();
                let explained = match (q(s0), q(e0), moved) {
                    (Some(a), Some(b), Some(m)) => (b - a - m).abs() <= QTY_TOLERANCE,
                    _ => false,
                };
                contributions = flows;
                match flows {
                    Some(f) if explained => gain = Some(end_value - start_value - f),
                    _ => gain_reason = Some(GainReason::ContributionsUnknown),
                }
            }
            let basis = e0.and_then(|h| h.cost_basis).map(|b| b.cents());
            let (sold, price, priced_at) = match e0 {
                Some(h) => (false, h.price.clone(), Some(h.taken_at)),
                None => (true, None, None),
            };
            let price_as_of = match as_of.get(&(a.id.as_str(), id)) {
                Some((Some(synced), d)) if Some(*synced) == priced_at => d.map(String::from),
                _ => None,
            };
            rows.push(finish(PositionChange {
                account_ids: vec![a.id.clone()],
                account: label.clone(),
                security_id: id.to_string(),
                label: sec
                    .and_then(|s| s.ticker.clone())
                    .or_else(|| sec.map(|s| s.name.clone()))
                    .unwrap_or_else(|| id.to_string()),
                ticker,
                name: sec.map(|s| s.name.clone()).unwrap_or_default(),
                is_cash,
                start_quantity: s0.map_or_else(|| "0".into(), |h| h.quantity.clone()),
                end_quantity: e0.map_or_else(|| "0".into(), |h| h.quantity.clone()),
                price,
                priced_at,
                price_as_of,
                start_value: Money::from_cents(start_value),
                end_value: Money::from_cents(end_value),
                contributions: contributions.map(Money::from_cents),
                gain: gain.map(Money::from_cents),
                gain_bp: None,
                gain_reason,
                change_reason,
                partial_start: partial,
                sold,
                cost_basis: basis.map(Money::from_cents),
                unrealized_gain: None,
                unrealized_bp: None,
            }));
        }
        if a.kind == AccountKind::Investment
            && filter.keeps(&a.id, &label, UNINVESTED_CASH_ID, None)
        {
            if let Some(r) = uninvested_row(a, &label, balances, &start, &end, start_t, end_t) {
                rows.push(finish(PositionChange {
                    partial_start: partial,
                    change_reason: one.then_some(ChangeReason::OneSnapshot),
                    ..r
                }));
            }
        }
    }
    if lens == PositionLens::Position {
        rows = combine(rows);
    }
    rows.sort_by(|x, y| {
        (
            y.end_value.cents(),
            y.start_value.cents(),
            &x.label,
            &x.account,
        )
            .cmp(&(
                x.end_value.cents(),
                x.start_value.cents(),
                &y.label,
                &y.account,
            ))
    });
    rows
}

/// The synthetic `cash:uninvested` row for one investment account: its
/// balance snapshot less its holdings at the start and end snapshot times,
/// as `account_cash` does, so the positions sum to the account's value.
/// None when the account had no uninvested cash at either end.
fn uninvested_row(
    a: &Account,
    label: &str,
    balances: &[BalanceSnapshot],
    start: &BTreeMap<&str, &HoldingSnapshot>,
    end: &BTreeMap<&str, &HoldingSnapshot>,
    start_t: i64,
    end_t: i64,
) -> Option<PositionChange> {
    let at = |t: i64, held: &BTreeMap<&str, &HoldingSnapshot>| -> Option<i64> {
        let bal = balances
            .iter()
            .filter(|b| b.account_id == a.id && b.taken_at <= t)
            .max_by_key(|b| b.taken_at)?;
        let held: i64 = held.values().map(|h| h.value.cents()).sum();
        Some((bal.balance.cents() - held).max(0))
    };
    let (s, e) = (at(start_t, start).unwrap_or(0), at(end_t, end)?);
    if s == 0 && e == 0 {
        return None;
    }
    let dollars = |c: i64| format!("{}.{:02}", c / 100, c % 100);
    Some(PositionChange {
        account_ids: vec![a.id.clone()],
        account: label.to_string(),
        security_id: UNINVESTED_CASH_ID.into(),
        ticker: None,
        label: "Cash".into(),
        name: "Uninvested cash".into(),
        is_cash: true,
        start_quantity: dollars(s),
        end_quantity: dollars(e),
        price: Some("1".into()),
        priced_at: Some(end_t),
        price_as_of: None,
        start_value: Money::from_cents(s),
        end_value: Money::from_cents(e),
        contributions: None,
        gain: None,
        gain_bp: None,
        gain_reason: Some(GainReason::Cash),
        change_reason: None,
        partial_start: false,
        sold: false,
        cost_basis: None,
        unrealized_gain: None,
        unrealized_bp: None,
    })
}

/// A position's money in (positive) or out from one activity row: buys and
/// sells at their cash cost, share transfers at the first recorded price on
/// or after the date (rule D). Dividends, fees and the rest: 0. None when a
/// transfer can't be valued.
fn position_flow(t: &InvestmentTxn, date: NaiveDate, mine: &[&HoldingSnapshot]) -> Option<i64> {
    let cash = t.amount.cents();
    match t.txn_type.as_str() {
        "buy" | "sell" => Some(-cash),
        "transfer" if cash != 0 => Some(-cash),
        "transfer" => match contribution(t, date, mine) {
            Contribution::Valued(v) => Some(v),
            Contribution::None => Some(0),
            Contribution::Unvalued => None,
        },
        _ => Some(0),
    }
}

/// Fill the derived fields: gain %, unrealized gain.
fn finish(mut r: PositionChange) -> PositionChange {
    let start = r.start_value.cents();
    r.gain_bp = r.gain.and_then(|g| {
        let base = start + r.contributions.map_or(0, |c| c.cents().max(0));
        (base > 0).then(|| ((g.cents() as i128 * 10_000) / base as i128) as i64)
    });
    r.unrealized_gain = r
        .cost_basis
        .map(|b| Money::from_cents(r.end_value.cents() - b.cents()));
    r.unrealized_bp = match (r.unrealized_gain, r.cost_basis) {
        (Some(g), Some(b)) if b.cents() > 0 => {
            Some(((g.cents() as i128 * 10_000) / b.cents() as i128) as i64)
        }
        _ => None,
    };
    r
}

/// 1e-8 share units back to plain decimal text, trailing zeros trimmed.
fn format_units(u: i128) -> String {
    let sign = if u < 0 { "-" } else { "" };
    let (int, frac) = (u.abs() / QTY_SCALE, u.abs() % QTY_SCALE);
    let frac = format!("{frac:08}");
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    }
}

/// The By position lens: one row per ticker (else security id) across
/// accounts. Sums are only given when every part has them.
fn combine(rows: Vec<PositionChange>) -> Vec<PositionChange> {
    let mut groups: BTreeMap<String, Vec<PositionChange>> = BTreeMap::new();
    for r in rows {
        let key = r
            .ticker
            .clone()
            .unwrap_or_else(|| format!("id:{}", r.security_id));
        groups.entry(key).or_default().push(r);
    }
    groups
        .into_values()
        .map(|mut parts| {
            if parts.len() == 1 {
                return parts.pop().unwrap();
            }
            let sum = |f: fn(&PositionChange) -> Money| -> Money {
                Money::from_cents(parts.iter().map(|p| f(p).cents()).sum())
            };
            let all = |f: fn(&PositionChange) -> Option<Money>| -> Option<Money> {
                parts
                    .iter()
                    .map(|p| f(p).map(|m| m.cents()))
                    .sum::<Option<i64>>()
                    .map(Money::from_cents)
            };
            let units = |f: fn(&PositionChange) -> &String| -> String {
                parts
                    .iter()
                    .map(|p| qty_units(f(p)))
                    .sum::<Option<i128>>()
                    .map_or_else(|| "?".into(), format_units)
            };
            let latest = parts
                .iter()
                .filter(|p| p.price.is_some())
                .max_by_key(|p| p.priced_at);
            let held: Vec<&PositionChange> = parts.iter().filter(|p| !p.sold).collect();
            let basis = if held.is_empty() {
                None
            } else {
                held.iter()
                    .map(|p| p.cost_basis.map(|m| m.cents()))
                    .sum::<Option<i64>>()
                    .map(Money::from_cents)
            };
            let gain = all(|p| p.gain);
            let first = &parts[0];
            finish(PositionChange {
                account_ids: parts.iter().flat_map(|p| p.account_ids.clone()).collect(),
                account: parts
                    .iter()
                    .map(|p| p.account.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                security_id: first.security_id.clone(),
                ticker: first.ticker.clone(),
                label: first.label.clone(),
                name: first.name.clone(),
                is_cash: first.is_cash,
                start_quantity: units(|p| &p.start_quantity),
                end_quantity: units(|p| &p.end_quantity),
                price: latest.and_then(|p| p.price.clone()),
                priced_at: parts.iter().filter_map(|p| p.priced_at).max(),
                price_as_of: parts.iter().filter_map(|p| p.price_as_of.clone()).min(),
                start_value: sum(|p| p.start_value),
                end_value: sum(|p| p.end_value),
                contributions: all(|p| p.contributions),
                gain,
                gain_bp: None,
                gain_reason: if gain.is_some() {
                    None
                } else {
                    parts.iter().find_map(|p| p.gain_reason)
                },
                change_reason: if parts.iter().all(|p| p.change_reason.is_some()) {
                    first.change_reason
                } else {
                    None
                },
                partial_start: parts.iter().any(|p| p.partial_start),
                sold: parts.iter().all(|p| p.sold),
                cost_basis: basis,
                unrealized_gain: None,
                unrealized_bp: None,
            })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
mod position_tests {
    use super::*;
    use crate::balances::PositionFilter;
    use crate::model::{AccountKind, Holding};
    use chrono::{Local, TimeZone};

    fn acct(id: &str) -> Account {
        Account {
            id: id.into(),
            org: "Broker".into(),
            name: format!("{id} (0000)"),
            kind: AccountKind::Investment,
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

    fn at(m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, m, d, 12, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn nov() -> (Option<i64>, Option<i64>) {
        let mid = |m, d| {
            Local
                .with_ymd_and_hms(2026, m, d, 0, 0, 0)
                .earliest()
                .unwrap()
                .timestamp()
        };
        (Some(mid(11, 1)), Some(mid(12, 1)))
    }

    fn pos(
        ts: i64,
        a: &str,
        sec: &str,
        qty: &str,
        price: &str,
        cents: i64,
        basis: Option<i64>,
    ) -> HoldingSnapshot {
        HoldingSnapshot {
            taken_at: ts,
            account_id: a.into(),
            security_id: sec.into(),
            quantity: qty.into(),
            price: Some(price.into()),
            value: Money::from_cents(cents),
            cost_basis: basis.map(Money::from_cents),
        }
    }

    fn sec(id: &str, ticker: Option<&str>, is_cash: bool) -> Security {
        Security {
            security_id: id.into(),
            ticker: ticker.map(Into::into),
            name: format!("{id} name"),
            sec_type: "etf".into(),
            is_cash,
            cusip: None,
            isin: None,
            close_price: None,
            close_price_as_of: None,
        }
    }

    fn secs() -> Vec<Security> {
        vec![
            sec("vti-a", Some("VTI"), false),
            sec("vti-b", Some("VTI"), false), // same ticker, another institution's id
            sec("abc", Some("ABC"), false),
            sec("wts", None, false), // warrants: no ticker
            sec("mmf", Some("MMF"), true),
        ]
    }

    #[allow(clippy::too_many_arguments)]
    fn txn(
        id: &str,
        a: &str,
        sec: &str,
        date: &str,
        ty: &str,
        sub: &str,
        qty: &str,
        cents: i64,
    ) -> InvestmentTxn {
        InvestmentTxn {
            id: id.into(),
            account_id: a.into(),
            security_id: Some(sec.into()),
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

    fn run(
        h: &[HoldingSnapshot],
        act: &[InvestmentTxn],
        lens: PositionLens,
    ) -> Vec<PositionChange> {
        let (since, until) = nov();
        position_change(
            &[acct("brk"), acct("ira")],
            &secs(),
            h,
            &[],
            &[],
            act,
            since,
            until,
            &PositionFilter::default(),
            lens,
        )
    }

    fn row<'a>(rows: &'a [PositionChange], a: &str, sec: &str) -> &'a PositionChange {
        rows.iter()
            .find(|r| r.account_ids == [a] && r.security_id == sec)
            .unwrap()
    }

    fn c(m: Option<Money>) -> Option<i64> {
        m.map(|m| m.cents())
    }

    #[test]
    fn steady_position_gain_and_unrealized() {
        let h = [
            pos(
                at(10, 31),
                "brk",
                "vti-a",
                "10",
                "250",
                2_500_00,
                Some(2_000_00),
            ),
            pos(
                at(11, 30),
                "brk",
                "vti-a",
                "10",
                "262",
                2_620_00,
                Some(2_000_00),
            ),
        ];
        let rows = run(&h, &[], PositionLens::Account);
        let r = row(&rows, "brk", "vti-a");
        assert_eq!(c(r.gain), Some(120_00));
        assert_eq!(r.gain_bp, Some(480)); // 120 / 2,500
        assert_eq!(c(r.contributions), Some(0));
        assert_eq!(c(r.unrealized_gain), Some(620_00));
        assert_eq!(r.unrealized_bp, Some(3_100));
        assert_eq!(r.price.as_deref(), Some("262"));
        assert_eq!(r.priced_at, Some(at(11, 30)));
        assert!(!r.sold && !r.partial_start);
    }

    /// Bought mid-period: starts at $0, the buy is a contribution.
    #[test]
    fn mid_period_buy_starts_at_zero() {
        let h = [
            pos(at(10, 31), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(11, 30), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(11, 30), "brk", "abc", "20", "11", 220_00, Some(200_00)),
        ];
        let act = [txn(
            "b1",
            "brk",
            "abc",
            "2026-11-10",
            "buy",
            "buy",
            "20",
            -200_00,
        )];
        let rows = run(&h, &act, PositionLens::Account);
        let r = row(&rows, "brk", "abc");
        assert_eq!(r.start_value.cents(), 0);
        assert_eq!(r.start_quantity, "0");
        assert_eq!(c(r.contributions), Some(200_00));
        assert_eq!(c(r.gain), Some(20_00));
        assert_eq!(r.gain_bp, Some(1_000)); // 20 / (0 + 200)
    }

    /// Sold in the period: stays in the list, gain from the sale proceeds.
    #[test]
    fn sold_position_stays() {
        let h = [
            pos(at(10, 31), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(10, 31), "brk", "abc", "20", "10", 200_00, Some(150_00)),
            pos(at(11, 30), "brk", "vti-a", "10", "250", 2_500_00, None),
        ];
        let act = [txn(
            "s1",
            "brk",
            "abc",
            "2026-11-12",
            "sell",
            "sell",
            "-20",
            230_00,
        )];
        let rows = run(&h, &act, PositionLens::Account);
        let r = row(&rows, "brk", "abc");
        assert!(r.sold);
        assert_eq!(r.end_value.cents(), 0);
        assert_eq!(c(r.contributions), Some(-230_00));
        assert_eq!(c(r.gain), Some(30_00));
        assert_eq!(r.price, None);
        assert_eq!(r.cost_basis, None);
    }

    /// Shares that appear with no activity: unknown, never estimated.
    #[test]
    fn unexplained_quantity_is_unknown() {
        let h = [
            pos(at(10, 31), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(11, 30), "brk", "vti-a", "14", "250", 3_500_00, None),
        ];
        let rows = run(&h, &[], PositionLens::Account);
        let r = row(&rows, "brk", "vti-a");
        assert_eq!(r.gain, None);
        assert_eq!(r.gain_reason, Some(GainReason::ContributionsUnknown));
    }

    /// Reinvested dividend: the buy is a contribution to the position, so
    /// the dividend itself isn't gain here (it is at account level).
    #[test]
    fn reinvestment_buy_is_a_position_contribution() {
        let h = [
            pos(at(10, 31), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(11, 30), "brk", "vti-a", "10.2", "250", 2_550_00, None),
        ];
        let act = [
            txn(
                "d1",
                "brk",
                "vti-a",
                "2026-11-10",
                "cash",
                "dividend",
                "0",
                50_00,
            ),
            txn(
                "r1",
                "brk",
                "vti-a",
                "2026-11-10",
                "buy",
                "dividend reinvestment",
                "0.2",
                -50_00,
            ),
        ];
        let rows = run(&h, &act, PositionLens::Account);
        assert_eq!(c(row(&rows, "brk", "vti-a").gain), Some(0));
    }

    /// Shares transferred in count at the recorded snapshot price (rule D).
    #[test]
    fn share_transfer_in_is_a_contribution() {
        let h = [
            pos(at(10, 31), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(11, 6), "brk", "abc", "120", "31.37", 3_764_40, None),
            pos(at(11, 6), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(11, 30), "brk", "abc", "120", "32.10", 3_852_00, None),
            pos(at(11, 30), "brk", "vti-a", "10", "250", 2_500_00, None),
        ];
        let act = [txn(
            "t1",
            "brk",
            "abc",
            "2026-11-05",
            "transfer",
            "transfer",
            "120",
            0,
        )];
        let rows = run(&h, &act, PositionLens::Account);
        let r = row(&rows, "brk", "abc");
        assert_eq!(c(r.contributions), Some(3_764_40));
        assert_eq!(c(r.gain), Some(87_60));
    }

    /// The same ticker in two accounts (different security ids) combines;
    /// warrants without a ticker stay their own row, even at $0; a missing
    /// basis on one side leaves the combined basis unknown.
    #[test]
    fn by_position_combines_tickers_across_accounts() {
        let h = [
            pos(
                at(10, 31),
                "brk",
                "vti-a",
                "10",
                "250",
                2_500_00,
                Some(2_000_00),
            ),
            pos(at(10, 31), "ira", "vti-b", "4", "250", 1_000_00, None),
            pos(at(10, 31), "ira", "wts", "100", "0", 0, None),
            pos(
                at(11, 30),
                "brk",
                "vti-a",
                "10",
                "260",
                2_600_00,
                Some(2_000_00),
            ),
            pos(at(11, 30), "ira", "vti-b", "4.5", "260", 1_170_00, None),
            pos(at(11, 30), "ira", "wts", "100", "0", 0, None),
        ];
        let act = [txn(
            "b1",
            "ira",
            "vti-b",
            "2026-11-03",
            "buy",
            "buy",
            "0.5",
            -125_00,
        )];
        let rows = run(&h, &act, PositionLens::Position);
        let vti = rows
            .iter()
            .find(|r| r.ticker.as_deref() == Some("VTI"))
            .unwrap();
        assert_eq!(vti.account_ids, ["brk", "ira"]);
        assert_eq!(vti.account, "brk (0000), ira (0000)");
        assert_eq!(vti.end_quantity, "14.5");
        assert_eq!(vti.start_quantity, "14");
        assert_eq!(vti.end_value.cents(), 3_770_00);
        assert_eq!(c(vti.contributions), Some(125_00));
        assert_eq!(c(vti.gain), Some(100_00 + 45_00));
        assert_eq!(vti.cost_basis, None, "ira's VTI has no basis");
        assert_eq!(vti.unrealized_gain, None);
        let wts = rows.iter().find(|r| r.security_id == "wts").unwrap();
        assert_eq!(wts.end_value.cents(), 0);
        assert_eq!(wts.label, "wts name");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "VTI"); // largest first
    }

    /// Money-market funds are cash, listed but with no gain.
    #[test]
    fn cash_security_has_no_gain() {
        let h = [
            pos(at(10, 31), "brk", "mmf", "100", "1", 100_00, None),
            pos(at(11, 30), "brk", "mmf", "180", "1", 180_00, None),
        ];
        let rows = run(&h, &[], PositionLens::Account);
        let r = row(&rows, "brk", "mmf");
        assert!(r.is_cash);
        assert_eq!(r.gain_reason, Some(GainReason::Cash));
    }

    /// One holdings snapshot: no change; a first snapshot inside the period
    /// is a partial start.
    #[test]
    fn one_snapshot_and_partial_start() {
        let h = [pos(at(11, 5), "brk", "vti-a", "10", "250", 2_500_00, None)];
        let rows = run(&h, &[], PositionLens::Account);
        let r = row(&rows, "brk", "vti-a");
        assert_eq!(r.change_reason, Some(ChangeReason::OneSnapshot));
        assert_eq!(r.gain, None);
        assert!(r.partial_start);
    }

    /// Filters and the institution's price date (frozen-price custodians).
    #[test]
    fn filter_and_price_as_of() {
        let h = [
            pos(at(10, 31), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(11, 30), "brk", "vti-a", "10", "250", 2_500_00, None),
            pos(at(10, 31), "ira", "abc", "1", "10", 10_00, None),
            pos(at(11, 30), "ira", "abc", "1", "10", 10_00, None),
        ];
        let current = [(
            Holding {
                account_id: "brk".into(),
                security_id: "vti-a".into(),
                quantity: "10".into(),
                price: "250".into(),
                value: Money::from_cents(2_500_00),
                cost_basis: None,
                price_as_of: Some("2026-09-30".into()),
                currency: None,
            },
            Some(at(11, 30)),
        )];
        let (since, until) = nov();
        let f = PositionFilter {
            account: None,
            ticker: Some("vti".into()),
        };
        let rows = position_change(
            &[acct("brk"), acct("ira")],
            &secs(),
            &h,
            &[],
            &current,
            &[],
            since,
            until,
            &f,
            PositionLens::Account,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].price_as_of.as_deref(), Some("2026-09-30"));
    }
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
mod uninvested_tests {
    use super::*;
    use crate::balances::PositionFilter;
    use chrono::{Local, TimeZone};

    fn at(m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, m, d, 12, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }
    fn mid(m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(2026, m, d, 0, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }
    fn acct(id: &str, kind: AccountKind) -> Account {
        Account {
            id: id.into(),
            org: "Broker".into(),
            name: id.into(),
            kind,
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
    fn pos(ts: i64, a: &str, cents: i64) -> HoldingSnapshot {
        HoldingSnapshot {
            taken_at: ts,
            account_id: a.into(),
            security_id: "vti".into(),
            quantity: "10".into(),
            value: Money::from_cents(cents),
            cost_basis: None,
            price: Some("1".into()),
        }
    }
    fn bal(ts: i64, a: &str, cents: i64) -> BalanceSnapshot {
        BalanceSnapshot {
            taken_at: ts,
            account_id: a.into(),
            balance: Money::from_cents(cents),
            cash: Money::from_cents(0),
        }
    }
    fn vti() -> Security {
        Security {
            security_id: "vti".into(),
            ticker: Some("VTI".into()),
            name: "Total Market".into(),
            sec_type: "etf".into(),
            is_cash: false,
            cusip: None,
            isin: None,
            close_price: None,
            close_price_as_of: None,
        }
    }

    #[test]
    fn positions_plus_uninvested_cash_sum_to_the_balance() {
        let h = [
            pos(at(10, 31), "brk", 1_000_00),
            pos(at(11, 30), "brk", 1_100_00),
        ];
        let b = [
            bal(at(10, 31), "brk", 1_250_00),
            bal(at(11, 30), "brk", 1_400_00),
            bal(at(11, 30), "chk", 9_00),
        ];
        let rows = position_change(
            &[
                acct("brk", AccountKind::Investment),
                acct("chk", AccountKind::Checking),
            ],
            &[vti()],
            &h,
            &b,
            &[],
            &[],
            Some(mid(11, 1)),
            Some(mid(12, 1)),
            &PositionFilter::default(),
            PositionLens::Account,
        );
        assert_eq!(rows.len(), 2, "VTI + one cash row, nothing for checking");
        let cash = rows.iter().find(|r| r.is_cash).unwrap();
        assert_eq!(cash.security_id, UNINVESTED_CASH_ID);
        assert_eq!(cash.start_value.cents(), 250_00);
        assert_eq!(cash.end_value.cents(), 300_00);
        assert_eq!(cash.end_quantity, "300.00");
        assert_eq!(cash.gain_reason, Some(GainReason::Cash));
        assert_eq!(cash.gain, None);
        let total: i64 = rows.iter().map(|r| r.end_value.cents()).sum();
        assert_eq!(total, 1_400_00);
    }

    #[test]
    fn fully_invested_account_has_no_cash_row() {
        let h = [
            pos(at(10, 31), "brk", 1_000_00),
            pos(at(11, 30), "brk", 1_100_00),
        ];
        let b = [
            bal(at(10, 31), "brk", 1_000_00),
            bal(at(11, 30), "brk", 1_100_00),
        ];
        let rows = position_change(
            &[acct("brk", AccountKind::Investment)],
            &[vti()],
            &h,
            &b,
            &[],
            &[],
            Some(mid(11, 1)),
            Some(mid(12, 1)),
            &PositionFilter::default(),
            PositionLens::Account,
        );
        assert!(rows.iter().all(|r| !r.is_cash));
    }
}
