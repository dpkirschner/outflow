use crate::categorize::{Categorizer, CategoryRule, MatchType, RuleSet};
use crate::model::*;
use crate::money::Money;
use crate::subscriptions::normalize_payee;
use rusqlite::{params, Connection};
use std::collections::HashSet;

fn row_to_txn(r: &rusqlite::Row) -> rusqlite::Result<Transaction> {
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
}

const TXN_COLUMNS: &str =
    "id, account_id, posted, amount_cents, description, payee, category, category_source, pending, raw";

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpsertResult {
    pub added: usize,
    pub updated: usize,
}

/// Default category vocabulary, seeded on first open and used to constrain the
/// LLM categorizer's output. Users can add to it via `add_category`.
const DEFAULT_CATEGORIES: &[&str] = &[
    "Groceries",
    "Dining",
    "Coffee",
    "Transport",
    "Fuel",
    "Shopping",
    "Entertainment",
    "Streaming",
    "Subscriptions",
    "Utilities",
    "Rent",
    "Insurance",
    "Health",
    "Fitness",
    "Travel",
    "Income",
    "Transfers",
    "Fees",
    "Education",
    "Personal Care",
    "Home",
    "Gifts",
    "Taxes",
    "Other",
];

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

    #[cfg(feature = "encryption")]
    pub fn open_encrypted(path: &str, key: &str) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "key", key)?;
        let s = Store { conn };
        s.migrate()?;
        Ok(s)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        self.conn.execute_batch(SCHEMA)?;
        self.seed_default_categories()
    }

    /// Populate the category vocabulary with a sensible default set on first
    /// open. Only seeds when the table is empty, so user edits (adds/removes)
    /// are never clobbered on subsequent opens.
    fn seed_default_categories(&self) -> rusqlite::Result<()> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM categories", [], |r| r.get(0))?;
        if count > 0 {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        for name in DEFAULT_CATEGORIES {
            tx.execute(
                "INSERT OR IGNORE INTO categories (name) VALUES (?1)",
                params![name],
            )?;
        }
        tx.commit()
    }

    pub fn add_category(&self, name: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO categories (name) VALUES (?1)",
            params![name],
        )?;
        Ok(())
    }

    pub fn categories(&self) -> rusqlite::Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM categories ORDER BY name")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        rows.collect()
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

    /// Wipe pulled data — transactions, accounts, and the sync log — for a clean
    /// re-pull, while keeping learned category rules and the vocabulary.
    pub fn reset_data(&self) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM transactions", [])?;
        tx.execute("DELETE FROM accounts", [])?;
        tx.execute("DELETE FROM sync_log", [])?;
        tx.commit()
    }

    pub fn accounts(&self) -> rusqlite::Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, org, name, kind, balance_cents, currency, last_synced
             FROM accounts ORDER BY name",
        )?;
        let rows = stmt.query_map([], |r| {
            let kind: String = r.get(3)?;
            Ok(Account {
                id: r.get(0)?,
                org: r.get(1)?,
                name: r.get(2)?,
                kind: AccountKind::from_str(&kind),
                balance: Money::from_cents(r.get(4)?),
                currency: r.get(5)?,
                last_synced: r.get(6)?,
            })
        })?;
        rows.collect()
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
        let sql = format!("SELECT {} FROM transactions ORDER BY posted ASC", TXN_COLUMNS);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_txn)?;
        rows.collect()
    }

    pub fn uncategorized(&self) -> rusqlite::Result<Vec<Transaction>> {
        let sql = format!(
            "SELECT {} FROM transactions WHERE category IS NULL ORDER BY posted ASC",
            TXN_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], row_to_txn)?;
        rows.collect()
    }

    pub fn transaction(&self, id: &str) -> rusqlite::Result<Option<Transaction>> {
        let sql = format!("SELECT {} FROM transactions WHERE id = ?1", TXN_COLUMNS);
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map(params![id], row_to_txn)?;
        match rows.next() {
            Some(r) => Ok(Some(r?)),
            None => Ok(None),
        }
    }

    pub fn count_transactions(&self) -> rusqlite::Result<i64> {
        self.conn
            .query_row("SELECT COUNT(*) FROM transactions", [], |r| r.get(0))
    }

    pub fn add_rule(
        &self,
        match_type: MatchType,
        pattern: &str,
        category: &str,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO category_rules (match_type, pattern, category) VALUES (?1, ?2, ?3)",
            params![match_type.as_str(), pattern.to_lowercase(), category],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn rules(&self) -> rusqlite::Result<Vec<CategoryRule>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, match_type, pattern, category FROM category_rules")?;
        let rows = stmt.query_map([], |r| {
            let mt: String = r.get(1)?;
            Ok(CategoryRule {
                id: r.get(0)?,
                match_type: MatchType::from_str(&mt).unwrap_or(MatchType::Contains),
                pattern: r.get(2)?,
                category: r.get(3)?,
            })
        })?;
        rows.collect()
    }

    pub fn ruleset(&self) -> rusqlite::Result<RuleSet> {
        Ok(RuleSet::new(self.rules()?))
    }

    pub fn set_category(
        &self,
        txn_id: &str,
        category: &str,
        source: CategorySource,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE transactions SET category = ?1, category_source = ?2 WHERE id = ?3",
            params![category, source.as_str(), txn_id],
        )?;
        Ok(())
    }

    pub fn categorize_uncategorized(
        &self,
        categorizer: &dyn Categorizer,
        source: CategorySource,
    ) -> rusqlite::Result<usize> {
        let pending = self.uncategorized()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        for t in &pending {
            if let Some(cat) = categorizer.categorize(t) {
                tx.execute(
                    "UPDATE transactions SET category = ?1, category_source = ?2 WHERE id = ?3",
                    params![cat, source.as_str(), t.id],
                )?;
                n += 1;
            }
        }
        tx.commit()?;
        Ok(n)
    }

    pub fn set_manual_category(
        &self,
        txn_id: &str,
        category: &str,
        learn: bool,
    ) -> rusqlite::Result<Option<i64>> {
        let existing = self.transaction(txn_id)?;
        self.set_category(txn_id, category, CategorySource::Manual)?;
        if learn {
            if let Some(t) = existing {
                let pattern = normalize_payee(t.merchant());
                if !pattern.is_empty() {
                    let id = self.add_rule(MatchType::Exact, &pattern, category)?;
                    return Ok(Some(id));
                }
            }
        }
        Ok(None)
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
    fn seeds_default_categories_and_respects_edits() {
        let s = Store::open_in_memory().unwrap();
        let seeded = s.categories().unwrap();
        assert!(seeded.contains(&"Groceries".to_string()));
        assert!(seeded.contains(&"Streaming".to_string()));

        // add_category is idempotent; a new name shows up sorted.
        s.add_category("Groceries").unwrap();
        s.add_category("Charity").unwrap();
        let after = s.categories().unwrap();
        assert_eq!(after.iter().filter(|c| *c == "Groceries").count(), 1);
        assert!(after.contains(&"Charity".to_string()));
    }

    #[test]
    fn reset_data_clears_txns_and_accounts_but_keeps_rules() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[txn("a", "acct1", 100, -500, false)]).unwrap();
        s.add_rule(MatchType::Exact, "netflix", "Streaming").unwrap();

        s.reset_data().unwrap();

        assert_eq!(s.count_transactions().unwrap(), 0);
        assert!(s.accounts().unwrap().is_empty());
        // Learned rules survive a data reset.
        assert_eq!(s.rules().unwrap().len(), 1);
    }

    #[test]
    fn accounts_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let a = Account {
            id: "acct1".into(),
            org: "Demo Bank".into(),
            name: "Checking".into(),
            kind: AccountKind::Checking,
            balance: Money::from_cents(123456),
            currency: "USD".into(),
            last_synced: 42,
        };
        s.upsert_accounts(&[a.clone()]).unwrap();
        let got = s.accounts().unwrap();
        assert_eq!(got, vec![a]);
    }

    #[test]
    fn pending_delete_is_scoped_to_synced_account() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[txn("p_a", "acctA", 100, -100, true)]).unwrap();
        s.upsert_transactions(&[txn("p_b", "acctB", 100, -100, true)]).unwrap();
        assert_eq!(s.count_transactions().unwrap(), 2);
    }
}

#[cfg(test)]
mod categorize_tests {
    use super::*;
    use crate::categorize::MatchType;

    fn t(id: &str, merchant: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct".into(),
            posted: 0,
            amount: Money::from_cents(-500),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            raw: "{}".into(),
        }
    }

    #[test]
    fn categorize_pass_applies_rules_and_counts() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[t("1", "Netflix"), t("2", "Whole Foods")]).unwrap();
        s.add_rule(MatchType::Exact, "netflix", "Streaming").unwrap();

        let n = s.categorize_uncategorized(&s.ruleset().unwrap(), CategorySource::Rule).unwrap();
        assert_eq!(n, 1);

        let all = s.all_transactions().unwrap();
        let netflix = all.iter().find(|x| x.id == "1").unwrap();
        let groceries = all.iter().find(|x| x.id == "2").unwrap();
        assert_eq!(netflix.category.as_deref(), Some("Streaming"));
        assert_eq!(netflix.category_source, Some(CategorySource::Rule));
        assert!(groceries.category.is_none());
    }

    #[test]
    fn manual_correction_learns_rule_that_catches_siblings() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t("1", "SQ *BLUE BOTTLE 4471"),
            t("2", "SQ *BLUE BOTTLE 9982"),
        ])
        .unwrap();

        let rule_id = s.set_manual_category("1", "Coffee", true).unwrap();
        assert!(rule_id.is_some());

        let n = s.categorize_uncategorized(&s.ruleset().unwrap(), CategorySource::Rule).unwrap();
        assert_eq!(n, 1);

        let sibling = s.transaction("2").unwrap().unwrap();
        assert_eq!(sibling.category.as_deref(), Some("Coffee"));
    }
}

#[cfg(all(test, feature = "encryption"))]
mod encryption_tests {
    use super::*;

    fn tmp_path(tag: &str) -> String {
        let mut p = std::env::temp_dir();
        // process id keeps the path unique across concurrent test binaries.
        p.push(format!("outflow-enc-{}-{}.db", tag, std::process::id()));
        p.to_string_lossy().into_owned()
    }

    fn txn(id: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct".into(),
            posted: 100,
            amount: Money::from_cents(-500),
            description: "d".into(),
            payee: None,
            category: None,
            category_source: None,
            pending: false,
            raw: "{}".into(),
        }
    }

    #[test]
    fn encrypts_and_requires_correct_key() {
        let path = tmp_path("roundtrip");
        let _ = std::fs::remove_file(&path);

        // Write with a key.
        {
            let s = Store::open_encrypted(&path, "correct horse").unwrap();
            s.upsert_transactions(&[txn("a")]).unwrap();
        }

        // Reopen with the same key -> data is readable.
        {
            let s = Store::open_encrypted(&path, "correct horse").unwrap();
            assert_eq!(s.count_transactions().unwrap(), 1);
        }

        // Wrong key must fail. This also proves SQLCipher is actually compiled
        // in: if the build silently fell back to plain SQLite, the key pragma
        // would be ignored and this would succeed.
        assert!(
            Store::open_encrypted(&path, "wrong key").is_err(),
            "wrong key must not open the encrypted db"
        );

        // A plaintext open must also fail on an encrypted file.
        assert!(
            Store::open(&path).is_err(),
            "plaintext open of an encrypted db must fail"
        );

        let _ = std::fs::remove_file(&path);
    }
}
