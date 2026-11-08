//! Upkeep: what needs the owner right now, one source of truth for the
//! Settings badge, `GET /api/upkeep`, `outflow status` and the check-in's
//! attention items (GLOSSARY: Check-in).
//!
//! - **Connections** whose status isn't `ok` (login required, error).
//! - **Card-payment matches** waiting for review. A pair with a leg on a
//!   hidden account still suppresses but isn't offered for review (the match
//!   list drops it), so it isn't counted either.
//! - **Last synced**: the newest successful sync across connections
//!   (`plaid_items.last_synced`, written only when a sync batch commits).

use crate::model::MatchStatus;
use crate::store::Store;
use serde::Serialize;
use std::collections::HashSet;

/// A connection that needs the owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ConnectionIssue {
    pub item_id: String,
    pub institution: String,
    /// `login_required` or `error`.
    pub status: String,
}

/// What needs the owner now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Upkeep {
    /// Connections whose status isn't `ok`, oldest link first.
    pub connections: Vec<ConnectionIssue>,
    /// Card-payment matches waiting for review.
    pub proposed_matches: usize,
    /// Newest successful sync across connections, epoch seconds; None when
    /// nothing has synced (or nothing is linked).
    pub last_synced: Option<i64>,
    /// Things to do: each connection plus each match waiting.
    pub total: usize,
}

/// The upkeep summary.
pub fn summary(store: &Store) -> rusqlite::Result<Upkeep> {
    let items = store.plaid_items()?;
    let connections: Vec<ConnectionIssue> = items
        .iter()
        .filter(|it| it.status != "ok")
        .map(|it| ConnectionIssue {
            item_id: it.item_id.clone(),
            institution: it.institution.clone(),
            status: it.status.clone(),
        })
        .collect();
    let last_synced = items.iter().filter_map(|it| it.last_synced).max();

    let hidden: HashSet<String> = store.hidden_account_ids()?.into_iter().collect();
    let on_hidden = |id: &str| -> rusqlite::Result<bool> {
        Ok(store
            .transaction(id)?
            .is_some_and(|t| hidden.contains(&t.account_id)))
    };
    let mut proposed_matches = 0;
    for m in store.matches(Some(MatchStatus::Proposed))? {
        if hidden.is_empty() || !(on_hidden(&m.bank_txn_id)? || on_hidden(&m.card_txn_id)?) {
            proposed_matches += 1;
        }
    }

    Ok(Upkeep {
        total: connections.len() + proposed_matches,
        connections,
        proposed_matches,
        last_synced,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Account, AccountKind, MatchConfidence, Transaction, TxnFlag};
    use crate::money::Money;
    use crate::store::PlaidBatch;

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

    fn txn(id: &str, account: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: account.into(),
            posted: 1_800_000_000,
            transacted_at: None,
            amount: Money::from_cents(-100),
            description: "Demo".into(),
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

    fn propose(s: &Store, bank: &str, card: &str) {
        s.insert_match(
            bank,
            card,
            MatchStatus::Proposed,
            MatchConfidence::Medium,
            None,
            0,
        )
        .unwrap();
    }

    #[test]
    fn empty_store_needs_nothing() {
        let s = Store::open_in_memory().unwrap();
        let u = summary(&s).unwrap();
        assert_eq!(
            u,
            Upkeep {
                connections: vec![],
                proposed_matches: 0,
                last_synced: None,
                total: 0
            }
        );
    }

    #[test]
    fn counts_connections_and_visible_proposals() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[acct("a"), acct("b")]).unwrap();
        s.upsert_transactions(&[
            txn("t1", "a"),
            txn("t2", "a"),
            txn("t3", "b"),
            txn("t4", "a"),
        ])
        .unwrap();
        propose(&s, "t1", "t2");
        propose(&s, "t3", "t4"); // bank leg on b, hidden below
        s.set_account_hidden("b", true).unwrap();

        s.upsert_plaid_item("it1", "Demo Bank", 1).unwrap();
        s.upsert_plaid_item("it2", "Demo Union", 2).unwrap();
        s.upsert_plaid_item("it3", "Demo Broker", 3).unwrap();
        for (item, at) in [("it1", 1_800_000_500), ("it2", 1_800_000_900)] {
            s.apply_plaid_batch(
                &PlaidBatch {
                    item_id: item.into(),
                    accounts: vec![],
                    upserts: vec![],
                    removed_ids: vec![],
                    next_cursor: "c".into(),
                },
                at,
            )
            .unwrap();
        }
        s.set_plaid_item_status("it2", "login_required").unwrap();

        let u = summary(&s).unwrap();
        assert_eq!(
            u.connections,
            vec![ConnectionIssue {
                item_id: "it2".into(),
                institution: "Demo Union".into(),
                status: "login_required".into(),
            }]
        );
        assert_eq!(
            u.proposed_matches, 1,
            "the hidden-leg pair isn't reviewable"
        );
        assert_eq!(u.last_synced, Some(1_800_000_900));
        assert_eq!(u.total, 2);
    }
}
