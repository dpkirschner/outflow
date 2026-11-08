//! What a reset (`Store::reset_data`) loses for good, so the Settings
//! confirm and an agent can name it before anything is deleted.
//!
//! Reset deletes every transaction, account, match and the sync log, keeps
//! rules, categories, Plaid items (cursors cleared), snapshots and
//! investment transactions. The next sync replays what Plaid still serves:
//! `/transactions/sync` from an empty cursor returns the item's history, and
//! we never set `transactions.days_requested` when creating a link token
//! (`net::plaid::link_token_body`), so Plaid's default of
//! `PLAID_REFETCH_DAYS` applies. Rows posted before `today − window` count
//! as lost for good. That is the conservative reading: an item linked long
//! ago may still re-serve more, never less.

use crate::model::{CategorySource, MatchStatus};
use crate::period::Period;
use crate::store::Store;
use chrono::{Days, Local, NaiveDate, TimeZone};
use serde::Serialize;

/// Days of transaction history Plaid serves for an item: its default
/// `transactions.days_requested`, which the link token leaves unset.
pub const PLAID_REFETCH_DAYS: u64 = 90;

/// What a reset deletes, and which of it the next sync can't bring back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ResetPreview {
    /// Every stored transaction, hidden accounts' included.
    pub transactions: usize,
    /// Local day of the oldest stored transaction (posted date),
    /// `YYYY-MM-DD`; None when there are none.
    pub oldest_day: Option<String>,
    /// How many days back Plaid re-serves (`PLAID_REFETCH_DAYS`).
    pub refetch_days: u64,
    /// The first local day the next sync re-fetches, `YYYY-MM-DD`
    /// (today − `refetch_days`).
    pub refetch_from: String,
    /// Transactions posted before `refetch_from`: gone for good.
    pub lost_for_good: usize,
    /// Transactions whose category the owner set by hand. Re-fetched rows
    /// come back without it (a learned rule re-applies on the next
    /// categorize; a one-off edit is gone).
    pub manual_categories: usize,
    /// Card-payment pairs the owner or the detector decided (accepted or
    /// rejected). Re-fetched pairs are detected again; high confidence
    /// re-accepts, the rest come back for review.
    pub decided_matches: usize,
    /// Every account, hidden ones included.
    pub accounts: usize,
    /// Accounts with a nickname: cleared.
    pub nicknamed_accounts: usize,
    /// Hidden accounts: they come back visible.
    pub hidden_accounts: usize,
}

/// The reset preview as of `today` (local).
pub fn preview(store: &Store, today: NaiveDate) -> rusqlite::Result<ResetPreview> {
    let txns = store.all_transactions_including_hidden()?;
    let accounts = store.all_accounts()?;
    let refetch_from = today - Days::new(PLAID_REFETCH_DAYS);
    let cutoff = Period::containing(crate::period::PeriodKind::Custom, refetch_from)
        .bounds()
        .0;
    let oldest_day = txns
        .iter()
        .map(|t| t.posted)
        .min()
        .and_then(|ts| Local.timestamp_opt(ts, 0).earliest())
        .map(|d| d.date_naive().to_string());
    let decided_matches = store
        .matches(None)?
        .iter()
        .filter(|m| m.status != MatchStatus::Proposed)
        .count();
    Ok(ResetPreview {
        transactions: txns.len(),
        oldest_day,
        refetch_days: PLAID_REFETCH_DAYS,
        refetch_from: refetch_from.to_string(),
        lost_for_good: txns.iter().filter(|t| t.posted < cutoff).count(),
        manual_categories: txns
            .iter()
            .filter(|t| t.category_source == Some(CategorySource::Manual))
            .count(),
        decided_matches,
        accounts: accounts.len(),
        nicknamed_accounts: accounts.iter().filter(|a| a.nickname.is_some()).count(),
        hidden_accounts: accounts.iter().filter(|a| a.hidden).count(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Account, AccountKind, MatchConfidence, Transaction, TxnFlag};
    use crate::money::Money;

    fn at(y: i32, m: u32, d: u32, h: u32) -> i64 {
        Local
            .with_ymd_and_hms(y, m, d, h, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }

    fn txn(id: &str, account: &str, posted: i64) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: account.into(),
            posted,
            transacted_at: None,
            amount: Money::from_cents(-500),
            description: "Demo Shop".into(),
            payee: None,
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            pfc_primary: None,
            pfc_detailed: None,
            source: "plaid".into(),
        }
    }

    fn acct(id: &str) -> Account {
        Account {
            id: id.into(),
            org: "Bank".into(),
            name: format!("{id} (0000)"),
            kind: AccountKind::Checking,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
            item_id: None,
        }
    }

    #[test]
    fn empty_store() {
        let s = Store::open_in_memory().unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 11, 7).unwrap();
        let p = preview(&s, today).unwrap();
        assert_eq!(p.transactions, 0);
        assert_eq!(p.oldest_day, None);
        assert_eq!(p.refetch_from, "2026-08-09");
        assert_eq!(p.lost_for_good, 0);
    }

    #[test]
    fn counts_what_is_lost_for_good() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("a"), acct("b"), acct("c")])
            .unwrap();
        s.set_account_nickname("a", Some("Bills")).unwrap();
        s.set_account_hidden("b", true).unwrap();
        // today 2026-11-07, window 90 days: re-fetch from 2026-08-09.
        let today = NaiveDate::from_ymd_opt(2026, 11, 7).unwrap();
        s.upsert_transactions(&[
            txn("old", "a", at(2026, 3, 2, 12)),
            txn("edge-before", "b", at(2026, 8, 8, 23)), // hidden account counts
            txn("edge-on", "a", at(2026, 8, 9, 0)),
            txn("new", "c", at(2026, 11, 1, 12)),
        ])
        .unwrap();
        s.set_manual_category("new", "Dining", false).unwrap();
        s.insert_match(
            "edge-on",
            "new",
            MatchStatus::Accepted,
            MatchConfidence::High,
            None,
            0,
        )
        .unwrap();
        s.insert_match(
            "old",
            "new",
            MatchStatus::Proposed,
            MatchConfidence::Medium,
            None,
            0,
        )
        .unwrap();

        let p = preview(&s, today).unwrap();
        assert_eq!(
            p,
            ResetPreview {
                transactions: 4,
                oldest_day: Some("2026-03-02".into()),
                refetch_days: 90,
                refetch_from: "2026-08-09".into(),
                lost_for_good: 2,
                manual_categories: 1,
                decided_matches: 1,
                accounts: 3,
                nicknamed_accounts: 1,
                hidden_accounts: 1,
            }
        );
    }
}
