use crate::model::*;
use crate::money::Money;
use rusqlite::{params, Connection};
use std::collections::HashSet;

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpsertResult {
    pub added: usize,
    pub updated: usize,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS accounts (
    id TEXT PRIMARY KEY,
    org TEXT NOT NULL,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    balance_cents INTEGER NOT NULL,
    currency TEXT NOT NULL,
    last_synced INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS transactions (
    id TEXT PRIMARY KEY,
    account_id TEXT NOT NULL,
    posted INTEGER NOT NULL,
    amount_cents INTEGER NOT NULL,
    description TEXT NOT NULL,
    payee TEXT,
    category TEXT,
    category_source TEXT,
    pending INTEGER NOT NULL,
    raw TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_txn_account ON transactions(account_id);
CREATE INDEX IF NOT EXISTS idx_txn_posted ON transactions(posted);
CREATE INDEX IF NOT EXISTS idx_txn_pending ON transactions(pending);
CREATE TABLE IF NOT EXISTS category_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    match_type TEXT NOT NULL,
    pattern TEXT NOT NULL,
    category TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS categories (
    name TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    started INTEGER NOT NULL,
    finished INTEGER,
    source TEXT NOT NULL,
    added INTEGER,
    updated INTEGER,
    note TEXT
);
";

impl Store {
    pub fn open(path: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    pub fn open_in_memory() -> rusqlite::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(SCHEMA)
    }

    pub fn upsert_accounts(&self, accounts: &[Account]) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for a in accounts {
            tx.execute(
                "INSERT INTO accounts (id, org, name, kind, balance_cents, currency, last_synced)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                    org=excluded.org, name=excluded.name, kind=excluded.kind,
                    balance_cents=excluded.balance_cents, currency=excluded.currency,
                    last_synced=excluded.last_synced",
                params![
                    a.id,
                    a.org,
                    a.name,
                    a.kind.as_str(),
                    a.balance.cents(),
                    a.currency,
                    a.last_synced
                ],
            )?;
        }
        tx.commit()
    }

    pub fn upsert_transactions(&self, txns: &[Transaction]) -> rusqlite::Result<UpsertResult> {
        let tx = self.conn.unchecked_transaction()?;

        let account_ids: HashSet<&str> = txns.iter().map(|t| t.account_id.as_str()).collect();
        for acct in &account_ids {
            tx.execute(
                "DELETE FROM transactions WHERE pending = 1 AND account_id = ?1",
                params![acct],
            )?;
        }

        let mut existing: HashSet<String> = HashSet::new();
        {
            let mut stmt = tx.prepare("SELECT id FROM transactions WHERE id = ?1")?;
            for t in txns.iter().filter(|t| !t.pending) {
                let found = stmt.exists(params![t.id])?;
                if found {
                    existing.insert(t.id.clone());
                }
            }
        }

        let mut added = 0usize;
        let mut updated = 0usize;

        for t in txns {
            let cat_src = t.category_source.map(|c| c.as_str());
            tx.execute(
                "INSERT INTO transactions
                    (id, account_id, posted, amount_cents, description, payee, category, category_source, pending, raw)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT(id) DO UPDATE SET
                    account_id=excluded.account_id, posted=excluded.posted,
                    amount_cents=excluded.amount_cents, description=excluded.description,
                    payee=excluded.payee, category=excluded.category,
                    category_source=excluded.category_source, pending=excluded.pending,
                    raw=excluded.raw",
                params![
                    t.id,
                    t.account_id,
                    t.posted,
                    t.amount.cents(),
                    t.description,
                    t.payee,
                    t.category,
                    cat_src,
                    t.pending as i64,
                    t.raw
                ],
            )?;
            if t.pending {
                continue;
            }
            if existing.contains(&t.id) {
                updated += 1;
            } else {
                added += 1;
            }
        }

        tx.commit()?;
        Ok(UpsertResult { added, updated })
    }

    pub fn all_transactions(&self) -> rusqlite::Result<Vec<Transaction>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, account_id, posted, amount_cents, description, payee, category, category_source, pending, raw
             FROM transactions ORDER BY posted ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Transaction {
                id: r.get(0)?,
                account_id: r.get(1)?,
                posted: r.get(2)?,
                amount: Money::from_cents(r.get(3)?),
                description: r.get(4)?,
                payee: r.get(5)?,
                category: r.get(6)?,
                category_source: {
                    let s: Option<String> = r.get(7)?;
                    s.and_then(|v| CategorySource::from_str(&v))
                },
                pending: {
                    let p: i64 = r.get(8)?;
                    p != 0
                },
                raw: r.get(9)?,
            })
        })?;
        rows.collect()
    }

    pub fn count_transactions(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn txn(id: &str, acct: &str, posted: i64, cents: i64, pending: bool) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: acct.into(),
            posted,
            amount: Money::from_cents(cents),
            description: "d".into(),
            payee: None,
            category: None,
            category_source: None,
            pending,
            raw: "{}".into(),
        }
    }

    #[test]
    fn counts_added_then_updated() {
        let s = Store::open_in_memory().unwrap();
        let r1 = s.upsert_transactions(&[txn("a", "acct1", 100, -500, false)]).unwrap();
        assert_eq!(r1.added, 1);
        assert_eq!(r1.updated, 0);
        let r2 = s.upsert_transactions(&[txn("a", "acct1", 100, -500, false)]).unwrap();
        assert_eq!(r2.added, 0);
        assert_eq!(r2.updated, 1);
        assert_eq!(s.count_transactions().unwrap(), 1);
    }

    #[test]
    fn pending_is_replaced_not_accumulated() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            txn("p1", "acct1", 100, -500, true),
            txn("posted1", "acct1", 90, -900, false),
        ])
        .unwrap();
        assert_eq!(s.count_transactions().unwrap(), 2);

        s.upsert_transactions(&[txn("posted1", "acct1", 90, -900, false)]).unwrap();
        let all = s.all_transactions().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "posted1");
    }

    #[test]
    fn pending_delete_is_scoped_to_synced_account() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[txn("p_a", "acctA", 100, -100, true)]).unwrap();
        s.upsert_transactions(&[txn("p_b", "acctB", 100, -100, true)]).unwrap();
        assert_eq!(s.count_transactions().unwrap(), 2);
    }
}
