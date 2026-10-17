//! Balances: the "current state of the world" view across every account, plus
//! the daily trend built from per-sync snapshots. Pure — the store hands in
//! accounts/securities/holdings/snapshots and this module does all the math.
//!
//! The cash rule lives here and only here (`account_cash`): checking/savings
//! are all cash; an investment account's cash is its cash-equivalent holdings
//! (money-market funds like VMFXX, `type: cash` sweeps) plus any balance the
//! holdings don't account for (uninvested cash — an HSA with no positions is
//! therefore 100% cash). Credit and other accounts hold no cash.

use crate::model::{Account, AccountKind, BalanceSnapshot, Holding, Security};
use crate::money::Money;
use chrono::{Days, Local, NaiveDate, TimeZone};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

/// An investment account is flagged as holding idle cash when its cash is at
/// least this much...
pub const IDLE_CASH_MIN_CENTS: i64 = 100_000; // $1,000
/// ...and at least this share of the account, in basis points (5%).
pub const IDLE_CASH_MIN_BP: i64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Group {
    Cash,
    Investments,
    Credit,
    Other,
}

impl Group {
    fn of(kind: AccountKind) -> Group {
        match kind {
            AccountKind::Checking | AccountKind::Savings => Group::Cash,
            AccountKind::Investment => Group::Investments,
            AccountKind::Credit => Group::Credit,
            AccountKind::Other => Group::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HoldingRow {
    pub security_id: String,
    /// Ticker when there is one, else the security name.
    pub label: String,
    pub name: String,
    pub sec_type: String,
    pub is_cash: bool,
    pub quantity: String,
    pub price: String,
    pub value: Money,
    pub cost_basis: Option<Money>,
    /// `value - cost_basis`; None when the basis is unknown.
    pub gain: Option<Money>,
    /// Share of the account balance, in basis points.
    pub share_bp: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountBalance {
    pub account: Account,
    pub group: Group,
    pub cash: Money,
    /// Balance minus cash for investment accounts; 0 elsewhere.
    pub invested: Money,
    /// Cash as a share of the balance, in basis points (0 when balance <= 0).
    pub cash_bp: i64,
    pub idle_cash: bool,
    /// Largest position first.
    pub holdings: Vec<HoldingRow>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GroupBalance {
    pub group: Group,
    pub total: Money,
    pub accounts: Vec<AccountBalance>,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct Totals {
    /// Sum of every balance (credit and loans count negative).
    pub net_worth: Money,
    pub cash: Money,
    pub invested: Money,
    /// Sum of credit balances (negative = owed).
    pub credit: Money,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BalancesView {
    pub totals: Totals,
    /// In display order: cash, investments, credit, other; empty groups omitted.
    pub groups: Vec<GroupBalance>,
}

/// The one cash rule. `holdings` are this account's positions.
pub fn account_cash(
    kind: AccountKind,
    balance: Money,
    holdings: &[&Holding],
    securities: &HashMap<&str, &Security>,
) -> Money {
    match kind {
        AccountKind::Checking | AccountKind::Savings => balance,
        AccountKind::Investment => {
            let held: i64 = holdings.iter().map(|h| h.value.cents()).sum();
            let cash_held: i64 = holdings
                .iter()
                .filter(|h| {
                    securities
                        .get(h.security_id.as_str())
                        .is_some_and(|s| s.is_cash)
                })
                .map(|h| h.value.cents())
                .sum();
            let uninvested = (balance.cents() - held).max(0);
            Money::from_cents(cash_held + uninvested)
        }
        AccountKind::Credit | AccountKind::Other => Money::from_cents(0),
    }
}

fn share_bp(part: i64, whole: i64) -> i64 {
    if whole <= 0 {
        0
    } else {
        // i128: a $1M balance in cents * 10_000 still fits i64, but be safe.
        ((part as i128 * 10_000) / whole as i128) as i64
    }
}

/// Snapshot the current state. `accounts` should already be the visible set;
/// holdings for accounts not in it are ignored.
pub fn summarize(
    accounts: &[Account],
    securities: &[Security],
    holdings: &[Holding],
) -> BalancesView {
    let secs: HashMap<&str, &Security> = securities
        .iter()
        .map(|s| (s.security_id.as_str(), s))
        .collect();
    let mut by_account: HashMap<&str, Vec<&Holding>> = HashMap::new();
    for h in holdings {
        by_account.entry(h.account_id.as_str()).or_default().push(h);
    }

    let mut totals = Totals::default();
    let mut groups: BTreeMap<u8, GroupBalance> = BTreeMap::new();
    for a in accounts {
        let held = by_account
            .get(a.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let cash = account_cash(a.kind, a.balance, held, &secs);
        let group = Group::of(a.kind);
        let invested = if group == Group::Investments {
            Money::from_cents(a.balance.cents() - cash.cents())
        } else {
            Money::from_cents(0)
        };
        let cash_bp = share_bp(cash.cents(), a.balance.cents());
        let idle_cash = group == Group::Investments
            && cash.cents() >= IDLE_CASH_MIN_CENTS
            && cash_bp >= IDLE_CASH_MIN_BP;

        let mut rows: Vec<HoldingRow> = held
            .iter()
            .map(|h| {
                let sec = secs.get(h.security_id.as_str());
                HoldingRow {
                    security_id: h.security_id.clone(),
                    label: sec
                        .and_then(|s| s.ticker.clone())
                        .or_else(|| sec.map(|s| s.name.clone()))
                        .unwrap_or_else(|| h.security_id.clone()),
                    name: sec.map(|s| s.name.clone()).unwrap_or_default(),
                    sec_type: sec.map(|s| s.sec_type.clone()).unwrap_or_default(),
                    is_cash: sec.is_some_and(|s| s.is_cash),
                    quantity: h.quantity.clone(),
                    price: h.price.clone(),
                    value: h.value,
                    cost_basis: h.cost_basis,
                    gain: h
                        .cost_basis
                        .map(|b| Money::from_cents(h.value.cents() - b.cents())),
                    share_bp: share_bp(h.value.cents(), a.balance.cents()),
                }
            })
            .collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.value.cents()));

        totals.net_worth = Money::from_cents(totals.net_worth.cents() + a.balance.cents());
        totals.cash = Money::from_cents(totals.cash.cents() + cash.cents());
        totals.invested = Money::from_cents(totals.invested.cents() + invested.cents());
        if group == Group::Credit {
            totals.credit = Money::from_cents(totals.credit.cents() + a.balance.cents());
        }

        let g = groups.entry(group as u8).or_insert_with(|| GroupBalance {
            group,
            total: Money::from_cents(0),
            accounts: Vec::new(),
        });
        g.total = Money::from_cents(g.total.cents() + a.balance.cents());
        g.accounts.push(AccountBalance {
            account: a.clone(),
            group,
            cash,
            invested,
            cash_bp,
            idle_cash,
            holdings: rows,
        });
    }
    let mut groups: Vec<GroupBalance> = groups.into_values().collect();
    for g in &mut groups {
        g.accounts
            .sort_by_key(|a| std::cmp::Reverse(a.account.balance.cents().abs()));
    }
    BalancesView { totals, groups }
}

/// One calendar day of the trend (local time).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DayTotals {
    /// `YYYY-MM-DD`.
    pub day: String,
    pub net_worth: Money,
    pub cash: Money,
    pub invested: Money,
    pub credit: Money,
}

fn local_day(ts: i64) -> Option<NaiveDate> {
    Local
        .timestamp_opt(ts, 0)
        .earliest()
        .map(|d| d.date_naive())
}

/// Daily totals from snapshots. Each day uses every account's latest
/// snapshot as of that day's end, so an account that didn't sync that day
/// carries its last value forward; days with no sync at all are filled too,
/// from the first snapshot day through the last. `kinds` decides which
/// accounts count (pass the visible set) and how each one is bucketed.
pub fn history(
    snapshots: &[BalanceSnapshot],
    kinds: &HashMap<String, AccountKind>,
) -> Vec<DayTotals> {
    let mut snaps: Vec<&BalanceSnapshot> = snapshots
        .iter()
        .filter(|s| kinds.contains_key(&s.account_id))
        .collect();
    snaps.sort_by_key(|s| s.taken_at);
    let (Some(first), Some(last)) = (
        snaps.first().and_then(|s| local_day(s.taken_at)),
        snaps.last().and_then(|s| local_day(s.taken_at)),
    ) else {
        return Vec::new();
    };

    let mut latest: HashMap<&str, &BalanceSnapshot> = HashMap::new();
    let mut out = Vec::new();
    let mut i = 0;
    let mut day = first;
    while day <= last {
        while i < snaps.len() && local_day(snaps[i].taken_at).is_some_and(|d| d <= day) {
            latest.insert(snaps[i].account_id.as_str(), snaps[i]);
            i += 1;
        }
        let mut t = Totals::default();
        for s in latest.values() {
            let kind = kinds[&s.account_id];
            t.net_worth = Money::from_cents(t.net_worth.cents() + s.balance.cents());
            t.cash = Money::from_cents(t.cash.cents() + s.cash.cents());
            match kind {
                AccountKind::Investment => {
                    t.invested =
                        Money::from_cents(t.invested.cents() + s.balance.cents() - s.cash.cents())
                }
                AccountKind::Credit => {
                    t.credit = Money::from_cents(t.credit.cents() + s.balance.cents())
                }
                _ => {}
            }
        }
        out.push(DayTotals {
            day: day.format("%Y-%m-%d").to_string(),
            net_worth: t.net_worth,
            cash: t.cash,
            invested: t.invested,
            credit: t.credit,
        });
        day = match day.checked_add_days(Days::new(1)) {
            Some(d) => d,
            None => break,
        };
    }
    out
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
mod tests {
    use super::*;

    fn acct(id: &str, kind: AccountKind, cents: i64) -> Account {
        Account {
            id: id.into(),
            org: "Bank".into(),
            name: format!("{id} (0000)"),
            kind,
            balance: Money::from_cents(cents),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
        }
    }

    fn sec(id: &str, ticker: Option<&str>, is_cash: bool) -> Security {
        Security {
            security_id: id.into(),
            ticker: ticker.map(Into::into),
            name: format!("{id} name"),
            sec_type: if is_cash { "mutual fund" } else { "etf" }.into(),
            is_cash,
        }
    }

    fn hold(acct: &str, sec: &str, cents: i64, basis: Option<i64>) -> Holding {
        Holding {
            account_id: acct.into(),
            security_id: sec.into(),
            quantity: "1".into(),
            price: "1".into(),
            value: Money::from_cents(cents),
            cost_basis: basis.map(Money::from_cents),
        }
    }

    fn find<'a>(v: &'a BalancesView, id: &str) -> &'a AccountBalance {
        v.groups
            .iter()
            .flat_map(|g| &g.accounts)
            .find(|a| a.account.id == id)
            .unwrap()
    }

    #[test]
    fn investment_account_with_no_holdings_is_all_cash() {
        // The Fidelity HSA case: $4,478.25, no positions.
        let v = summarize(&[acct("hsa", AccountKind::Investment, 4_478_25)], &[], &[]);
        let hsa = find(&v, "hsa");
        assert_eq!(hsa.cash.cents(), 4_478_25);
        assert_eq!(hsa.invested.cents(), 0);
        assert_eq!(hsa.cash_bp, 10_000);
        assert!(hsa.idle_cash);
    }

    #[test]
    fn money_market_counts_as_cash_and_residual_adds_uninvested() {
        let secs = [
            sec("vmfxx", Some("VMFXX"), true),
            sec("vti", Some("VTI"), false),
        ];
        // 100 cash fund + 800 VTI held, balance 1000 → 100 uninvested.
        let holds = [
            hold("brk", "vmfxx", 100_00, None),
            hold("brk", "vti", 800_00, Some(500_00)),
        ];
        let v = summarize(
            &[acct("brk", AccountKind::Investment, 1_000_00)],
            &secs,
            &holds,
        );
        let brk = find(&v, "brk");
        assert_eq!(brk.cash.cents(), 200_00);
        assert_eq!(brk.invested.cents(), 800_00);
        assert_eq!(brk.holdings[0].label, "VTI"); // largest first
        assert_eq!(brk.holdings[0].gain.map(|g| g.cents()), Some(300_00));
        assert_eq!(brk.holdings[1].gain, None);
        assert!(brk.holdings[1].is_cash);
    }

    #[test]
    fn stale_balance_below_holdings_never_makes_negative_cash() {
        let secs = [sec("vti", Some("VTI"), false)];
        let holds = [hold("brk", "vti", 1_050_00, None)];
        let v = summarize(
            &[acct("brk", AccountKind::Investment, 1_000_00)],
            &secs,
            &holds,
        );
        assert_eq!(find(&v, "brk").cash.cents(), 0);
    }

    #[test]
    fn idle_cash_needs_both_the_dollar_and_share_threshold() {
        let secs = [sec("vti", Some("VTI"), false)];
        // $999 uninvested on a $1,999 account: 50% but under $1,000.
        let small = summarize(
            &[acct("a", AccountKind::Investment, 1_999_00)],
            &secs,
            &[hold("a", "vti", 1_000_00, None)],
        );
        assert!(!find(&small, "a").idle_cash);
        // $1,000 cash on a $1M account: over $1,000 but 0.1%.
        let big = summarize(
            &[acct("b", AccountKind::Investment, 1_000_000_00)],
            &secs,
            &[hold("b", "vti", 999_000_00, None)],
        );
        assert!(!find(&big, "b").idle_cash);
        // Exactly at both edges: $1,000 and 5% of $20,000.
        let edge = summarize(
            &[acct("c", AccountKind::Investment, 20_000_00)],
            &secs,
            &[hold("c", "vti", 19_000_00, None)],
        );
        assert!(find(&edge, "c").idle_cash);
        // Checking is cash but never "idle" — it's supposed to be cash.
        let chk = summarize(&[acct("d", AccountKind::Checking, 50_000_00)], &[], &[]);
        assert!(!find(&chk, "d").idle_cash);
    }

    #[test]
    fn totals_and_groups() {
        let accts = [
            acct("chk", AccountKind::Checking, 2_000_00),
            acct("card", AccountKind::Credit, -500_00),
            acct("ira", AccountKind::Investment, 10_000_00),
        ];
        let secs = [sec("vti", Some("VTI"), false)];
        let v = summarize(&accts, &secs, &[hold("ira", "vti", 9_000_00, None)]);
        assert_eq!(v.totals.net_worth.cents(), 11_500_00);
        assert_eq!(v.totals.cash.cents(), 3_000_00);
        assert_eq!(v.totals.invested.cents(), 9_000_00);
        assert_eq!(v.totals.credit.cents(), -500_00);
        let order: Vec<Group> = v.groups.iter().map(|g| g.group).collect();
        assert_eq!(order, [Group::Cash, Group::Investments, Group::Credit]);
    }

    fn snap(ts: i64, id: &str, bal: i64, cash: i64) -> BalanceSnapshot {
        BalanceSnapshot {
            taken_at: ts,
            account_id: id.into(),
            balance: Money::from_cents(bal),
            cash: Money::from_cents(cash),
        }
    }

    fn noon(y: i32, m: u32, d: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }

    #[test]
    fn history_takes_last_of_day_and_carries_forward() {
        let kinds: HashMap<String, AccountKind> = [
            ("chk".to_string(), AccountKind::Checking),
            ("ira".to_string(), AccountKind::Investment),
        ]
        .into();
        let snaps = [
            snap(noon(2026, 10, 1), "chk", 100_00, 100_00),
            snap(noon(2026, 10, 1) + 3600, "chk", 150_00, 150_00), // later same day wins
            snap(noon(2026, 10, 1), "ira", 1_000_00, 10_00),
            // Oct 2: nothing. Oct 3: only checking moves.
            snap(noon(2026, 10, 3), "chk", 200_00, 200_00),
            snap(noon(2026, 10, 3), "gone", 9_999_00, 0), // not in kinds → ignored
        ];
        let h = history(&snaps, &kinds);
        let days: Vec<&str> = h.iter().map(|d| d.day.as_str()).collect();
        assert_eq!(days, ["2026-10-01", "2026-10-02", "2026-10-03"]);
        assert_eq!(h[0].net_worth.cents(), 1_150_00);
        assert_eq!(h[0].invested.cents(), 990_00);
        assert_eq!(
            h[1],
            DayTotals {
                day: "2026-10-02".into(),
                ..h[0].clone()
            }
        );
        assert_eq!(h[2].net_worth.cents(), 1_200_00);
        assert_eq!(h[2].cash.cents(), 210_00);
        assert!(history(&[], &kinds).is_empty());
    }
}
