//! Investment activity: the flat, filterable view over archived investment
//! transactions (buys, sells, dividends, contributions, fees) that `outflow
//! activity` and `/api/activity` serve. Pure — the store hands in rows.

use crate::model::{account_display_label, Account, InvestmentTxn, Security};
use crate::money::Money;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityRow {
    pub id: String,
    pub date: String,
    pub account_id: String,
    /// Nickname-aware display label.
    pub account: String,
    pub security_id: Option<String>,
    pub ticker: Option<String>,
    /// Ticker, else security name, else the transaction's own name.
    pub label: String,
    pub name: String,
    /// Plaid type (buy, sell, cash, fee, transfer, cancel) and subtype.
    #[serde(rename = "type")]
    pub txn_type: String,
    pub subtype: String,
    pub quantity: String,
    pub price: String,
    /// Cash effect on the account: negative = cash out (buy, fee),
    /// positive = cash in (sell, dividend, contribution of cash).
    pub amount: Money,
    pub fees: Option<Money>,
    pub currency: Option<String>,
    pub cancel_txn_id: Option<String>,
}

/// Which rows to keep. All optional and case-insensitive; dates are
/// inclusive `YYYY-MM-DD` bounds.
#[derive(Debug, Clone, Default)]
pub struct ActivityFilter {
    /// Account id, or substring of its display label.
    pub account: Option<String>,
    /// Ticker or security id, exact.
    pub ticker: Option<String>,
    /// Matches the Plaid type or subtype exactly, e.g. `buy`, `dividend`.
    pub kind: Option<String>,
    pub since: Option<String>,
    pub until: Option<String>,
}

/// Rows on `accounts` (pass the visible set), newest first, filtered.
pub fn activity(
    txns: &[InvestmentTxn],
    accounts: &[Account],
    securities: &[Security],
    f: &ActivityFilter,
) -> Vec<ActivityRow> {
    let secs: HashMap<&str, &Security> = securities
        .iter()
        .map(|s| (s.security_id.as_str(), s))
        .collect();
    let accts: HashMap<&str, &Account> = accounts.iter().map(|a| (a.id.as_str(), a)).collect();
    let eq = |a: &str, b: &str| a.eq_ignore_ascii_case(b);
    let mut rows: Vec<ActivityRow> = txns
        .iter()
        .filter_map(|t| {
            let a = accts.get(t.account_id.as_str())?;
            let account = account_display_label(&a.name, a.nickname.as_deref());
            let sec = t.security_id.as_deref().and_then(|id| secs.get(id));
            let ticker = sec.and_then(|s| s.ticker.clone());
            if let Some(q) = &f.account {
                let q = q.to_lowercase();
                if a.id.to_lowercase() != q && !account.to_lowercase().contains(&q) {
                    return None;
                }
            }
            if let Some(q) = &f.ticker {
                let by_id = t.security_id.as_deref().is_some_and(|id| eq(id, q));
                let by_ticker = ticker.as_deref().is_some_and(|tk| eq(tk, q));
                if !by_id && !by_ticker {
                    return None;
                }
            }
            if let Some(k) = &f.kind {
                if !eq(&t.txn_type, k) && !eq(&t.subtype, k) {
                    return None;
                }
            }
            if f.since.as_deref().is_some_and(|s| t.date.as_str() < s)
                || f.until.as_deref().is_some_and(|u| t.date.as_str() > u)
            {
                return None;
            }
            Some(ActivityRow {
                id: t.id.clone(),
                date: t.date.clone(),
                account_id: a.id.clone(),
                account,
                security_id: t.security_id.clone(),
                label: ticker
                    .clone()
                    .or_else(|| sec.map(|s| s.name.clone()))
                    .unwrap_or_else(|| t.name.clone()),
                ticker,
                name: t.name.clone(),
                txn_type: t.txn_type.clone(),
                subtype: t.subtype.clone(),
                quantity: t.quantity.clone(),
                price: t.price.clone(),
                amount: t.amount,
                fees: t.fees,
                currency: t.currency.clone(),
                cancel_txn_id: t.cancel_txn_id.clone(),
            })
        })
        .collect();
    rows.sort_by(|x, y| y.date.cmp(&x.date).then_with(|| x.id.cmp(&y.id)));
    rows
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)]
mod tests {
    use super::*;
    use crate::model::AccountKind;

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
        }
    }

    fn txn(id: &str, acct: &str, sec: Option<&str>, date: &str, sub: &str) -> InvestmentTxn {
        InvestmentTxn {
            id: id.into(),
            account_id: acct.into(),
            security_id: sec.map(Into::into),
            date: date.into(),
            name: format!("{sub} {id}"),
            txn_type: if sub == "dividend" { "cash" } else { "buy" }.into(),
            subtype: sub.into(),
            quantity: "1".into(),
            price: "10".into(),
            amount: Money::from_cents(-10_00),
            fees: None,
            currency: None,
            cancel_txn_id: None,
        }
    }

    #[test]
    fn filters_and_orders_newest_first() {
        let accounts = [acct("brk"), acct("ira")];
        let secs = [Security {
            security_id: "s_vti".into(),
            ticker: Some("VTI".into()),
            name: "Total Market".into(),
            sec_type: "etf".into(),
            is_cash: false,
            cusip: None,
            isin: None,
            close_price: None,
            close_price_as_of: None,
        }];
        let txns = [
            txn("a", "brk", Some("s_vti"), "2026-09-01", "buy"),
            txn("b", "brk", Some("s_vti"), "2026-10-01", "dividend"),
            txn("c", "ira", None, "2026-10-15", "contribution"),
            txn("d", "gone", None, "2026-10-20", "buy"), // account not visible
        ];
        let all = activity(&txns, &accounts, &secs, &ActivityFilter::default());
        assert_eq!(
            all.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            ["c", "b", "a"]
        );
        assert_eq!(all[1].label, "VTI");
        assert_eq!(all[0].label, "contribution c"); // no security → txn name

        let f = |f: ActivityFilter| {
            activity(&txns, &accounts, &secs, &f)
                .into_iter()
                .map(|r| r.id)
                .collect::<Vec<_>>()
        };
        let by_kind = ActivityFilter {
            kind: Some("Dividend".into()),
            ..Default::default()
        };
        assert_eq!(f(by_kind), ["b"]);
        let by_type = ActivityFilter {
            kind: Some("buy".into()),
            ..Default::default()
        };
        assert_eq!(f(by_type), ["c", "a"]); // type buy (c's type is buy too)
        let by_ticker = ActivityFilter {
            ticker: Some("vti".into()),
            ..Default::default()
        };
        assert_eq!(f(by_ticker), ["b", "a"]);
        let by_dates = ActivityFilter {
            since: Some("2026-10-01".into()),
            until: Some("2026-10-01".into()),
            ..Default::default()
        };
        assert_eq!(f(by_dates), ["b"]);
        let by_account = ActivityFilter {
            account: Some("IRA".into()),
            ..Default::default()
        };
        assert_eq!(f(by_account), ["c"]);
    }
}
