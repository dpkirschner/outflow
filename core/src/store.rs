use crate::categorize::{Categorizer, CategoryRule, MatchType, RuleSet};
use crate::model::*;
use crate::money::Money;
use crate::subscriptions::normalize_payee;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

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
        transacted_at: r.get(10)?,
        flag: {
            // NOT NULL DEFAULT 'spending', so this is always present; unknown
            // values degrade to Spending rather than dropping the row.
            let f: String = r.get(11)?;
            TxnFlag::from_str(&f).unwrap_or(TxnFlag::Spending)
        },
        source: r.get(12)?,
    })
}

// Column order is load-bearing: it must stay in lockstep with the positional
// `row_to_txn` indices and the `upsert_transactions` INSERT. New columns are
// APPENDED, never inserted mid-list.
const TXN_COLUMNS: &str = "id, account_id, posted, amount_cents, description, payee, \
     category, category_source, pending, raw, transacted_at, flag, source";

pub struct Store {
    conn: Connection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpsertResult {
    pub added: usize,
    pub updated: usize,
}

/// One Plaid `/transactions/sync` run for a single item, applied atomically:
/// account upserts, transaction upserts (no pending sweep — Plaid signals
/// pending removal explicitly), deletions (`removed` + superseded pending
/// predecessors), and the cursor advance all commit in one SQLite transaction
/// so a crash can never persist the cursor without its data.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaidBatch {
    pub item_id: String,
    pub accounts: Vec<Account>,
    pub upserts: Vec<Transaction>,
    pub removed_ids: Vec<String>,
    pub next_cursor: String,
}

/// A learned/manual rule that assigns a `TxnFlag` to transactions whose
/// normalized merchant matches `pattern`. Parallel to `CategoryRule` — the same
/// mechanism on an independent axis (a merchant can be categorized AND flagged).
#[derive(Debug, Clone, PartialEq)]
pub struct FlagRule {
    pub id: i64,
    pub match_type: MatchType,
    pub pattern: String,
    pub flag: TxnFlag,
}

/// Match a normalized merchant against flag rules with the same precedence as
/// `RuleSet`: an exact match wins; otherwise the longest `contains` pattern wins.
fn match_flag<'a>(rules: &'a [FlagRule], merchant: &str) -> Option<&'a FlagRule> {
    let m = normalize_payee(merchant);
    if m.is_empty() {
        return None;
    }
    for r in rules {
        if r.match_type == MatchType::Exact && r.pattern == m {
            return Some(r);
        }
    }
    let mut best: Option<&FlagRule> = None;
    for r in rules {
        if r.match_type == MatchType::Contains && !r.pattern.is_empty() && m.contains(&r.pattern) {
            match best {
                Some(b) if b.pattern.len() >= r.pattern.len() => {}
                _ => best = Some(r),
            }
        }
    }
    best
}

/// Account upsert loop shared by `upsert_accounts` and `apply_plaid_batch`;
/// runs inside the caller's transaction.
fn insert_account_batch(conn: &Connection, accounts: &[Account]) -> rusqlite::Result<()> {
    for a in accounts {
        conn.execute(
            "INSERT INTO accounts (id, org, name, kind, balance_cents, currency, last_synced, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                org=excluded.org, name=excluded.name, kind=excluded.kind,
                balance_cents=excluded.balance_cents, currency=excluded.currency,
                last_synced=excluded.last_synced, source=excluded.source",
            params![
                a.id,
                a.org,
                a.name,
                a.kind.as_str(),
                a.balance.cents(),
                a.currency,
                a.last_synced,
                a.source
            ],
        )?;
    }
    Ok(())
}

/// Transaction upsert loop shared by `upsert_transactions` and
/// `apply_plaid_batch`; runs inside the caller's transaction. Counts pending
/// rows as neither added nor updated.
fn insert_txn_batch(conn: &Connection, txns: &[Transaction]) -> rusqlite::Result<UpsertResult> {
    let mut existing: HashSet<String> = HashSet::new();
    {
        let mut stmt = conn.prepare("SELECT id FROM transactions WHERE id = ?1")?;
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
        // `flag` is intentionally absent from the conflict-update: a re-pull
        // must not reset a manually-applied Transfer/CardPayment flag back to
        // 'spending'. `transacted_at` IS updated so a later pull can fill it
        // in if the bank starts supplying it.
        conn.execute(
            "INSERT INTO transactions
                (id, account_id, posted, amount_cents, description, payee, category, category_source, pending, raw, transacted_at, flag, source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(id) DO UPDATE SET
                account_id=excluded.account_id, posted=excluded.posted,
                amount_cents=excluded.amount_cents, description=excluded.description,
                payee=excluded.payee, category=excluded.category,
                category_source=excluded.category_source, pending=excluded.pending,
                raw=excluded.raw, transacted_at=excluded.transacted_at,
                source=excluded.source",
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
                t.raw,
                t.transacted_at,
                t.flag.as_str(),
                t.source
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

    Ok(UpsertResult { added, updated })
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
    last_synced INTEGER NOT NULL,
    source TEXT NOT NULL DEFAULT 'plaid'
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
    raw TEXT NOT NULL,
    transacted_at INTEGER,
    flag TEXT NOT NULL DEFAULT 'spending',
    source TEXT NOT NULL DEFAULT 'plaid'
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
CREATE TABLE IF NOT EXISTS flag_rules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    match_type TEXT NOT NULL,
    pattern TEXT NOT NULL,
    flag TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS categories (
    name TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS merchant_overrides (
    pattern TEXT PRIMARY KEY,
    mark TEXT NOT NULL
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
CREATE TABLE IF NOT EXISTS plaid_items (
    item_id TEXT PRIMARY KEY,
    institution TEXT NOT NULL,
    cursor TEXT,
    status TEXT NOT NULL DEFAULT 'ok',
    created INTEGER NOT NULL,
    last_synced INTEGER
);
CREATE TABLE IF NOT EXISTS txn_matches (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    bank_txn_id TEXT NOT NULL,
    card_txn_id TEXT NOT NULL,
    status TEXT NOT NULL,
    confidence TEXT NOT NULL,
    reason TEXT,
    created INTEGER NOT NULL,
    UNIQUE(bank_txn_id, card_txn_id)
);
";

/// Current schema version, tracked via `PRAGMA user_version`. Bump this and add a
/// guarded block in `run_migrations` when altering an existing table. New tables
/// (created via `CREATE TABLE IF NOT EXISTS` in `SCHEMA`) don't need an ALTER, but
/// bumping documents the change.
/// v1: transactions.transacted_at + transactions.flag. v2: merchant_overrides.
/// v3: accounts.source + transactions.source (+ plaid_items, txn_matches tables).
const SCHEMA_VERSION: i64 = 3;

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
        // Fresh DBs get every table/column from SCHEMA (all `IF NOT EXISTS`).
        self.conn.execute_batch(SCHEMA)?;
        // Existing DBs predate columns added later: `CREATE TABLE IF NOT EXISTS`
        // is a no-op once the table exists, so new columns must be ALTERed in.
        self.run_migrations()?;
        self.seed_default_categories()
    }

    /// Versioned schema upgrades for DBs created by an earlier build. Guarded by
    /// `PRAGMA user_version`; each column add is idempotent (skipped if the
    /// column already exists, e.g. on a fresh DB where SCHEMA created it).
    fn run_migrations(&self) -> rusqlite::Result<()> {
        let version: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            // v1: behavioral date + suppression flag on transactions.
            self.add_column_if_missing("transactions", "transacted_at", "INTEGER")?;
            self.add_column_if_missing(
                "transactions",
                "flag",
                "TEXT NOT NULL DEFAULT 'spending'",
            )?;
            // flag_rules is a new table → already created by SCHEMA, no ALTER.
        }
        if version < 3 {
            // v3: source provenance on pulled rows. Everything before v3 came
            // from the retired SimpleFIN integration — backfill stays
            // 'simplefin' so old archives keep truthful provenance.
            self.add_column_if_missing(
                "accounts",
                "source",
                "TEXT NOT NULL DEFAULT 'simplefin'",
            )?;
            self.add_column_if_missing(
                "transactions",
                "source",
                "TEXT NOT NULL DEFAULT 'simplefin'",
            )?;
            // plaid_items / txn_matches are new tables → created by SCHEMA.
        }
        self.conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(())
    }

    fn add_column_if_missing(
        &self,
        table: &str,
        column: &str,
        decl: &str,
    ) -> rusqlite::Result<()> {
        let mut stmt = self.conn.prepare(&format!("PRAGMA table_info({})", table))?;
        let present = stmt
            .query_map([], |r| r.get::<_, String>(1))?
            .filter_map(|c| c.ok())
            .any(|c| c == column);
        if !present {
            self.conn.execute(
                &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, decl),
                [],
            )?;
        }
        Ok(())
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
        insert_account_batch(&tx, accounts)?;
        tx.commit()
    }

    /// Wipe pulled data — transactions, accounts, the sync log, and detected
    /// payment matches — for a clean re-pull, while keeping learned category
    /// rules and the vocabulary. Plaid items survive (relinking is expensive)
    /// but their cursors reset so the next sync replays full history.
    pub fn reset_data(&self) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM transactions", [])?;
        tx.execute("DELETE FROM accounts", [])?;
        tx.execute("DELETE FROM sync_log", [])?;
        tx.execute("DELETE FROM txn_matches", [])?;
        tx.execute("UPDATE plaid_items SET cursor = NULL, last_synced = NULL", [])?;
        tx.commit()
    }

    pub fn accounts(&self) -> rusqlite::Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, org, name, kind, balance_cents, currency, last_synced, source
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
                source: r.get(7)?,
            })
        })?;
        rows.collect()
    }

    /// Plain id-keyed upsert (no pending lifecycle handling) — the offline
    /// fixture path and tests. Live Plaid syncs go through `apply_plaid_batch`,
    /// which also carries explicit deletions and the cursor advance.
    pub fn upsert_transactions(&self, txns: &[Transaction]) -> rusqlite::Result<UpsertResult> {
        let tx = self.conn.unchecked_transaction()?;
        let result = insert_txn_batch(&tx, txns)?;
        tx.commit()?;
        Ok(result)
    }

    /// Apply one Plaid sync run atomically. Unlike the SimpleFIN path there is
    /// no blanket pending sweep: Plaid reports pending-row lifecycle explicitly,
    /// so `removed_ids` (Plaid `removed` + pending predecessors superseded by
    /// posted rows) carries every deletion. The item's cursor advances in the
    /// same transaction — a crash can't persist one without the other.
    pub fn apply_plaid_batch(
        &self,
        batch: &PlaidBatch,
        synced_at: i64,
    ) -> rusqlite::Result<UpsertResult> {
        let tx = self.conn.unchecked_transaction()?;
        insert_account_batch(&tx, &batch.accounts)?;
        let result = insert_txn_batch(&tx, &batch.upserts)?;
        {
            let mut stmt = tx.prepare("DELETE FROM transactions WHERE id = ?1")?;
            for id in &batch.removed_ids {
                stmt.execute(params![id])?;
            }
        }
        tx.execute(
            "UPDATE plaid_items SET cursor = ?1, last_synced = ?2, status = 'ok'
             WHERE item_id = ?3",
            params![batch.next_cursor, synced_at, batch.item_id],
        )?;
        tx.commit()?;
        Ok(result)
    }

    pub fn delete_transactions(&self, ids: &[String]) -> rusqlite::Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        {
            let mut stmt = tx.prepare("DELETE FROM transactions WHERE id = ?1")?;
            for id in ids {
                n += stmt.execute(params![id])?;
            }
        }
        tx.commit()?;
        Ok(n)
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

    pub fn add_flag_rule(
        &self,
        match_type: MatchType,
        pattern: &str,
        flag: TxnFlag,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO flag_rules (match_type, pattern, flag) VALUES (?1, ?2, ?3)",
            params![match_type.as_str(), pattern.to_lowercase(), flag.as_str()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn flag_rules(&self) -> rusqlite::Result<Vec<FlagRule>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, match_type, pattern, flag FROM flag_rules")?;
        let rows = stmt.query_map([], |r| {
            let mt: String = r.get(1)?;
            let fl: String = r.get(3)?;
            Ok(FlagRule {
                id: r.get(0)?,
                match_type: MatchType::from_str(&mt).unwrap_or(MatchType::Contains),
                pattern: r.get(2)?,
                flag: TxnFlag::from_str(&fl).unwrap_or(TxnFlag::Spending),
            })
        })?;
        rows.collect()
    }

    /// Set a transaction's flag; when `learn`, also write an exact flag rule on
    /// its normalized merchant so siblings pick up the flag on the next
    /// `apply_flags` pass. Mirrors `set_manual_category`.
    pub fn set_flag(
        &self,
        txn_id: &str,
        flag: TxnFlag,
        learn: bool,
    ) -> rusqlite::Result<Option<i64>> {
        let existing = self.transaction(txn_id)?;
        self.conn.execute(
            "UPDATE transactions SET flag = ?1 WHERE id = ?2",
            params![flag.as_str(), txn_id],
        )?;
        if learn {
            if let Some(t) = existing {
                let pattern = normalize_payee(t.merchant());
                if !pattern.is_empty() {
                    let id = self.add_flag_rule(MatchType::Exact, &pattern, flag)?;
                    return Ok(Some(id));
                }
            }
        }
        Ok(None)
    }

    /// Apply every flag rule to matching transactions. Only touches rows whose
    /// current flag differs from the rule's, and never resets a row to
    /// `Spending` (rules only assign the flag they carry). Returns the count
    /// changed.
    pub fn apply_flags(&self) -> rusqlite::Result<usize> {
        let rules = self.flag_rules()?;
        if rules.is_empty() {
            return Ok(0);
        }
        let txns = self.all_transactions()?;
        let tx = self.conn.unchecked_transaction()?;
        let mut n = 0;
        for t in &txns {
            if let Some(r) = match_flag(&rules, t.merchant()) {
                if t.flag != r.flag {
                    tx.execute(
                        "UPDATE transactions SET flag = ?1 WHERE id = ?2",
                        params![r.flag.as_str(), t.id],
                    )?;
                    n += 1;
                }
            }
        }
        tx.commit()?;
        Ok(n)
    }

    /// Whether any ingested account is a credit/card account. Used to warn before
    /// suppressing a CardPayment with no offsetting charges present.
    pub fn has_credit_account(&self) -> rusqlite::Result<bool> {
        let n: i64 = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE kind = ?1)",
            params![AccountKind::Credit.as_str()],
            |r| r.get(0),
        )?;
        Ok(n != 0)
    }

    /// Set a per-merchant ledger override (Committed / Dismissed). `pattern` is a
    /// normalized merchant key; upserts so re-marking replaces.
    pub fn set_merchant_mark(&self, pattern: &str, mark: Mark) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO merchant_overrides (pattern, mark) VALUES (?1, ?2)
             ON CONFLICT(pattern) DO UPDATE SET mark = excluded.mark",
            params![pattern.to_lowercase(), mark.as_str()],
        )?;
        Ok(())
    }

    /// Remove a merchant's override, returning it to the normal streams list.
    pub fn clear_merchant_mark(&self, pattern: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM merchant_overrides WHERE pattern = ?1",
            params![pattern.to_lowercase()],
        )?;
        Ok(())
    }

    /// All merchant overrides, keyed by normalized merchant.
    pub fn merchant_marks(&self) -> rusqlite::Result<HashMap<String, Mark>> {
        let mut stmt = self.conn.prepare("SELECT pattern, mark FROM merchant_overrides")?;
        let rows = stmt.query_map([], |r| {
            let pattern: String = r.get(0)?;
            let mark: String = r.get(1)?;
            Ok((pattern, mark))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (pattern, mark) = row?;
            if let Some(m) = Mark::from_str(&mark) {
                out.insert(pattern, m);
            }
        }
        Ok(out)
    }

    /// The outflow transactions behind one stream — its normalized merchant, since
    /// `since` (inclusive, on the behavioral date) — newest first, for the
    /// slide-over. Includes flagged rows so their state is visible.
    pub fn stream_occurrences(
        &self,
        merchant_key: &str,
        since: Option<i64>,
    ) -> rusqlite::Result<Vec<Transaction>> {
        let mut txns: Vec<Transaction> = self
            .all_transactions()?
            .into_iter()
            .filter(|t| {
                t.amount.is_outflow()
                    && normalize_payee(t.merchant()) == merchant_key
                    && since.map_or(true, |s| t.effective_date() >= s)
            })
            .collect();
        txns.sort_by(|a, b| b.effective_date().cmp(&a.effective_date()));
        Ok(txns)
    }

    /// Register (or refresh) a linked Plaid item. Never touches the cursor —
    /// cursor advances happen only inside `apply_plaid_batch`.
    pub fn upsert_plaid_item(
        &self,
        item_id: &str,
        institution: &str,
        created: i64,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO plaid_items (item_id, institution, status, created)
             VALUES (?1, ?2, 'ok', ?3)
             ON CONFLICT(item_id) DO UPDATE SET
                institution=excluded.institution, status='ok'",
            params![item_id, institution, created],
        )?;
        Ok(())
    }

    pub fn plaid_items(&self) -> rusqlite::Result<Vec<PlaidItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT item_id, institution, cursor, status, created, last_synced
             FROM plaid_items ORDER BY created",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PlaidItem {
                item_id: r.get(0)?,
                institution: r.get(1)?,
                cursor: r.get(2)?,
                status: r.get(3)?,
                created: r.get(4)?,
                last_synced: r.get(5)?,
            })
        })?;
        rows.collect()
    }

    /// Mark an item's health ("ok" | "login_required" | "error") so the UI can
    /// surface a re-link affordance without failing the whole sync.
    pub fn set_plaid_item_status(&self, item_id: &str, status: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE plaid_items SET status = ?1 WHERE item_id = ?2",
            params![status, item_id],
        )?;
        Ok(())
    }

    /// Hard-reset an item's cursor so the next sync replays its full history.
    /// (Pagination-mutation restarts don't use this — they resume from the
    /// stored cursor, which only advances in `apply_plaid_batch`.)
    pub fn clear_plaid_cursor(&self, item_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE plaid_items SET cursor = NULL WHERE item_id = ?1",
            params![item_id],
        )?;
        Ok(())
    }

    /// Unlink an item's metadata. Historical transactions and accounts are kept
    /// — the archive is the point of this app.
    pub fn delete_plaid_item(&self, item_id: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM plaid_items WHERE item_id = ?1",
            params![item_id],
        )?;
        Ok(())
    }

    pub fn log_sync_start(&self, source: &str, started: i64) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO sync_log (started, source) VALUES (?1, ?2)",
            params![started, source],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn log_sync_finish(
        &self,
        id: i64,
        finished: i64,
        added: usize,
        updated: usize,
        note: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE sync_log SET finished = ?1, added = ?2, updated = ?3, note = ?4
             WHERE id = ?5",
            params![finished, added as i64, updated as i64, note, id],
        )?;
        Ok(())
    }

    /// Most recent sync runs, newest first.
    pub fn sync_log(&self, limit: usize) -> rusqlite::Result<Vec<SyncEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, started, finished, source, added, updated, note
             FROM sync_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |r| {
            Ok(SyncEntry {
                id: r.get(0)?,
                started: r.get(1)?,
                finished: r.get(2)?,
                source: r.get(3)?,
                added: r.get(4)?,
                updated: r.get(5)?,
                note: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Record a detected card-payment pair. The (bank, card) pair is UNIQUE —
    /// re-detection of an already-decided pair is a no-op that preserves the
    /// existing decision, so rejects are never re-proposed.
    pub fn insert_match(
        &self,
        bank_txn_id: &str,
        card_txn_id: &str,
        status: MatchStatus,
        confidence: MatchConfidence,
        reason: Option<&str>,
        created: i64,
    ) -> rusqlite::Result<Option<i64>> {
        let n = self.conn.execute(
            "INSERT OR IGNORE INTO txn_matches
                (bank_txn_id, card_txn_id, status, confidence, reason, created)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                bank_txn_id,
                card_txn_id,
                status.as_str(),
                confidence.as_str(),
                reason,
                created
            ],
        )?;
        if n == 0 {
            Ok(None)
        } else {
            Ok(Some(self.conn.last_insert_rowid()))
        }
    }

    pub fn matches(&self, status: Option<MatchStatus>) -> rusqlite::Result<Vec<TxnMatch>> {
        let sql = match status {
            Some(_) => {
                "SELECT id, bank_txn_id, card_txn_id, status, confidence, reason, created
                 FROM txn_matches WHERE status = ?1 ORDER BY created DESC"
            }
            None => {
                "SELECT id, bank_txn_id, card_txn_id, status, confidence, reason, created
                 FROM txn_matches ORDER BY created DESC"
            }
        };
        let mut stmt = self.conn.prepare(sql)?;
        let map = |r: &rusqlite::Row| -> rusqlite::Result<TxnMatch> {
            let st: String = r.get(3)?;
            let cf: String = r.get(4)?;
            Ok(TxnMatch {
                id: r.get(0)?,
                bank_txn_id: r.get(1)?,
                card_txn_id: r.get(2)?,
                status: MatchStatus::from_str(&st).unwrap_or(MatchStatus::Proposed),
                confidence: MatchConfidence::from_str(&cf).unwrap_or(MatchConfidence::Medium),
                reason: r.get(5)?,
                created: r.get(6)?,
            })
        };
        let rows = match status {
            Some(s) => stmt.query_map(params![s.as_str()], map)?.collect(),
            None => stmt.query_map([], map)?.collect(),
        };
        rows
    }

    pub fn match_by_id(&self, id: i64) -> rusqlite::Result<Option<TxnMatch>> {
        let all = self.matches(None)?;
        Ok(all.into_iter().find(|m| m.id == id))
    }

    pub fn set_match_status(&self, id: i64, status: MatchStatus) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE txn_matches SET status = ?1 WHERE id = ?2",
            params![status.as_str(), id],
        )?;
        Ok(())
    }

    /// After accepting a match, retire every other proposal that shares either
    /// leg — those pairings are stale once the legs are flagged. Returns the
    /// count retired.
    pub fn reject_conflicting_proposals(
        &self,
        accepted_id: i64,
        bank_txn_id: &str,
        card_txn_id: &str,
    ) -> rusqlite::Result<usize> {
        self.conn.execute(
            "UPDATE txn_matches SET status = 'rejected'
             WHERE status = 'proposed' AND id != ?1
               AND (bank_txn_id = ?2 OR card_txn_id = ?3)",
            params![accepted_id, bank_txn_id, card_txn_id],
        )
    }

    /// Every (bank, card) pair that already has a row, regardless of status —
    /// the detector must not re-propose any of these.
    pub fn decided_pairs(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT bank_txn_id, card_txn_id FROM txn_matches")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
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
            transacted_at: None,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            source: default_source(),
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
            source: default_source(),
        };
        s.upsert_accounts(&[a.clone()]).unwrap();
        let got = s.accounts().unwrap();
        assert_eq!(got, vec![a]);
    }

    #[test]
    fn migrate_stamps_schema_version() {
        let s = Store::open_in_memory().unwrap();
        let v: i64 = s.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn migrates_legacy_db_by_adding_columns() {
        // Emulate a DB created before this change: old transactions shape,
        // user_version 0, one existing row. Opening it through Store must add
        // the new columns in place without error and default the row's flag.
        let mut p = std::env::temp_dir();
        p.push(format!("outflow-migrate-{}.db", std::process::id()));
        let path = p.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&path);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE transactions (
                    id TEXT PRIMARY KEY, account_id TEXT NOT NULL, posted INTEGER NOT NULL,
                    amount_cents INTEGER NOT NULL, description TEXT NOT NULL, payee TEXT,
                    category TEXT, category_source TEXT, pending INTEGER NOT NULL, raw TEXT NOT NULL
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO transactions
                    (id, account_id, posted, amount_cents, description, payee, category, category_source, pending, raw)
                 VALUES ('old', 'acct', 100, -500, 'Legacy', NULL, NULL, NULL, 0, '{}')",
                [],
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let v: i64 = s.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let t = s.transaction("old").unwrap().unwrap();
        assert_eq!(t.transacted_at, None);
        assert_eq!(t.flag, TxnFlag::Spending);
        assert_eq!(t.effective_date(), 100);

        drop(s);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn migrates_v2_db_backfilling_source() {
        // A DB with the v2 shape (no source columns) must gain them on open,
        // with existing rows reading back as simplefin.
        let mut p = std::env::temp_dir();
        p.push(format!("outflow-migrate-v2-{}.db", std::process::id()));
        let path = p.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(&path);

        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE accounts (
                    id TEXT PRIMARY KEY, org TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL,
                    balance_cents INTEGER NOT NULL, currency TEXT NOT NULL, last_synced INTEGER NOT NULL
                );
                CREATE TABLE transactions (
                    id TEXT PRIMARY KEY, account_id TEXT NOT NULL, posted INTEGER NOT NULL,
                    amount_cents INTEGER NOT NULL, description TEXT NOT NULL, payee TEXT,
                    category TEXT, category_source TEXT, pending INTEGER NOT NULL, raw TEXT NOT NULL,
                    transacted_at INTEGER, flag TEXT NOT NULL DEFAULT 'spending'
                );
                PRAGMA user_version = 2;",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO accounts VALUES ('a1', 'Bank', 'Checking', 'checking', 100, 'USD', 5)",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO transactions
                    (id, account_id, posted, amount_cents, description, payee, category, category_source, pending, raw, transacted_at, flag)
                 VALUES ('t1', 'a1', 100, -500, 'Old', NULL, NULL, NULL, 0, '{}', NULL, 'spending')",
                [],
            )
            .unwrap();
        }

        let s = Store::open(&path).unwrap();
        let v: i64 = s.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        assert_eq!(s.transaction("t1").unwrap().unwrap().source, "simplefin");
        assert_eq!(s.accounts().unwrap()[0].source, "simplefin");

        drop(s);
        let _ = std::fs::remove_file(&path);
    }

    fn plaid_txn(id: &str, acct: &str, posted: i64, cents: i64, pending: bool) -> Transaction {
        let mut t = txn(id, acct, posted, cents, pending);
        t.source = "plaid".into();
        t
    }

    #[test]
    fn apply_plaid_batch_never_sweeps_pending_and_advances_cursor() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_plaid_item("item1", "Chase", 1000).unwrap();

        // First sync: one pending, one posted.
        let r1 = s
            .apply_plaid_batch(
                &PlaidBatch {
                    item_id: "item1".into(),
                    accounts: vec![],
                    upserts: vec![
                        plaid_txn("pend1", "acct1", 100, -500, true),
                        plaid_txn("post1", "acct1", 90, -900, false),
                    ],
                    removed_ids: vec![],
                    next_cursor: "cur1".into(),
                },
                2000,
            )
            .unwrap();
        assert_eq!(r1.added, 1); // pending counts as neither

        // Second sync touching the same account must NOT sweep the pending row
        // (that's SimpleFIN semantics only).
        s.apply_plaid_batch(
            &PlaidBatch {
                item_id: "item1".into(),
                accounts: vec![],
                upserts: vec![plaid_txn("post2", "acct1", 95, -700, false)],
                removed_ids: vec![],
                next_cursor: "cur2".into(),
            },
            3000,
        )
        .unwrap();
        assert_eq!(s.count_transactions().unwrap(), 3);

        // Third sync: pending posts under a new id; the predecessor id arrives
        // in removed_ids and is deleted in the same transaction.
        s.apply_plaid_batch(
            &PlaidBatch {
                item_id: "item1".into(),
                accounts: vec![],
                upserts: vec![plaid_txn("post3", "acct1", 100, -500, false)],
                removed_ids: vec!["pend1".into()],
                next_cursor: "cur3".into(),
            },
            4000,
        )
        .unwrap();
        assert_eq!(s.count_transactions().unwrap(), 3);
        assert!(s.transaction("pend1").unwrap().is_none());

        let items = s.plaid_items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].cursor.as_deref(), Some("cur3"));
        assert_eq!(items[0].last_synced, Some(4000));
        assert_eq!(items[0].status, "ok");
    }

    #[test]
    fn plaid_item_status_and_delete_keep_history() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_plaid_item("item1", "Amex", 10).unwrap();
        s.set_plaid_item_status("item1", "login_required").unwrap();
        assert_eq!(s.plaid_items().unwrap()[0].status, "login_required");
        // Relinking the same item resets status to ok.
        s.upsert_plaid_item("item1", "Amex", 10).unwrap();
        assert_eq!(s.plaid_items().unwrap()[0].status, "ok");

        s.apply_plaid_batch(
            &PlaidBatch {
                item_id: "item1".into(),
                accounts: vec![],
                upserts: vec![plaid_txn("t1", "acct1", 100, -500, false)],
                removed_ids: vec![],
                next_cursor: "c".into(),
            },
            20,
        )
        .unwrap();
        s.delete_plaid_item("item1").unwrap();
        assert!(s.plaid_items().unwrap().is_empty());
        // Transactions survive the unlink — the archive is permanent.
        assert_eq!(s.count_transactions().unwrap(), 1);
    }

    #[test]
    fn sync_log_round_trip() {
        let s = Store::open_in_memory().unwrap();
        let id = s.log_sync_start("plaid:item1", 100).unwrap();
        s.log_sync_finish(id, 110, 5, 2, Some("ok")).unwrap();
        let log = s.sync_log(10).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].source, "plaid:item1");
        assert_eq!(log[0].started, 100);
        assert_eq!(log[0].finished, Some(110));
        assert_eq!(log[0].added, Some(5));
        assert_eq!(log[0].updated, Some(2));
    }

    #[test]
    fn matches_dedupe_and_status_flow() {
        let s = Store::open_in_memory().unwrap();
        let id = s
            .insert_match("bank1", "card1", MatchStatus::Proposed, MatchConfidence::Medium, Some("same amount"), 100)
            .unwrap()
            .unwrap();
        // Re-detection of the same pair is a no-op that preserves the decision.
        assert!(s
            .insert_match("bank1", "card1", MatchStatus::Proposed, MatchConfidence::High, None, 200)
            .unwrap()
            .is_none());

        s.set_match_status(id, MatchStatus::Rejected).unwrap();
        assert!(s.matches(Some(MatchStatus::Proposed)).unwrap().is_empty());
        assert_eq!(s.matches(Some(MatchStatus::Rejected)).unwrap().len(), 1);
        assert_eq!(s.decided_pairs().unwrap(), vec![("bank1".to_string(), "card1".to_string())]);
        assert_eq!(s.match_by_id(id).unwrap().unwrap().status, MatchStatus::Rejected);
    }

    #[test]
    fn reset_data_clears_matches_and_plaid_cursors_but_keeps_items() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_plaid_item("item1", "Chase", 10).unwrap();
        s.apply_plaid_batch(
            &PlaidBatch {
                item_id: "item1".into(),
                accounts: vec![],
                upserts: vec![plaid_txn("t1", "acct1", 100, -500, false)],
                removed_ids: vec![],
                next_cursor: "cur".into(),
            },
            20,
        )
        .unwrap();
        s.insert_match("b", "c", MatchStatus::Proposed, MatchConfidence::High, None, 30)
            .unwrap();

        s.reset_data().unwrap();

        assert_eq!(s.count_transactions().unwrap(), 0);
        assert!(s.matches(None).unwrap().is_empty());
        let items = s.plaid_items().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].cursor, None);
    }

    #[test]
    fn set_flag_learns_rule_and_apply_flags_catches_siblings() {
        let s = Store::open_in_memory().unwrap();
        // Both share merchant() == "d" (payee None → description), so a learned
        // rule on one flags the other.
        s.upsert_transactions(&[
            txn("a", "acct1", 100, -500, false),
            txn("b", "acct1", 200, -600, false),
        ])
        .unwrap();

        let rule = s.set_flag("a", TxnFlag::Transfer, true).unwrap();
        assert!(rule.is_some());
        // Sibling stays Spending until an apply pass runs.
        assert_eq!(s.transaction("b").unwrap().unwrap().flag, TxnFlag::Spending);

        assert_eq!(s.apply_flags().unwrap(), 1);
        assert_eq!(s.transaction("b").unwrap().unwrap().flag, TxnFlag::Transfer);
        // Idempotent: nothing left to change.
        assert_eq!(s.apply_flags().unwrap(), 0);
    }

    #[test]
    fn flag_survives_repull() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[txn("a", "acct1", 100, -500, false)]).unwrap();
        s.set_flag("a", TxnFlag::CardPayment, false).unwrap();
        // A re-pull of the same id must not reset the manual flag to Spending.
        s.upsert_transactions(&[txn("a", "acct1", 100, -500, false)]).unwrap();
        assert_eq!(s.transaction("a").unwrap().unwrap().flag, TxnFlag::CardPayment);
    }

    #[test]
    fn merchant_marks_round_trip_and_clear() {
        let s = Store::open_in_memory().unwrap();
        s.set_merchant_mark("chase mortgage", Mark::Committed).unwrap();
        s.set_merchant_mark("random pop up", Mark::Dismissed).unwrap();
        let marks = s.merchant_marks().unwrap();
        assert_eq!(marks.get("chase mortgage"), Some(&Mark::Committed));
        assert_eq!(marks.get("random pop up"), Some(&Mark::Dismissed));
        // Re-marking replaces.
        s.set_merchant_mark("chase mortgage", Mark::Dismissed).unwrap();
        assert_eq!(s.merchant_marks().unwrap().get("chase mortgage"), Some(&Mark::Dismissed));
        s.clear_merchant_mark("chase mortgage").unwrap();
        assert!(s.merchant_marks().unwrap().get("chase mortgage").is_none());
    }

    #[test]
    fn stream_occurrences_filters_by_merchant_and_window() {
        let s = Store::open_in_memory().unwrap();
        // Two "blue bottle" (normalize to same key) + one other.
        let mut a = txn("a", "acct1", 1_000_000, -500, false);
        a.payee = Some("SQ *BLUE BOTTLE 1".into());
        let mut b = txn("b", "acct1", 2_000_000, -600, false);
        b.payee = Some("SQ *BLUE BOTTLE 2".into());
        let mut c = txn("c", "acct1", 2_000_000, -700, false);
        c.payee = Some("Whole Foods".into());
        s.upsert_transactions(&[a, b, c]).unwrap();

        let occ = s.stream_occurrences("blue bottle", None).unwrap();
        assert_eq!(occ.len(), 2);
        assert_eq!(occ[0].id, "b"); // newest first
        // Window bound drops the older one.
        let recent = s.stream_occurrences("blue bottle", Some(1_500_000)).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].id, "b");
    }

    #[test]
    fn has_credit_account_reflects_ingested_kinds() {
        let s = Store::open_in_memory().unwrap();
        assert!(!s.has_credit_account().unwrap());
        s.upsert_accounts(&[Account {
            id: "c".into(),
            org: "o".into(),
            name: "Visa".into(),
            kind: AccountKind::Credit,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: default_source(),
        }])
        .unwrap();
        assert!(s.has_credit_account().unwrap());
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
            transacted_at: None,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            source: default_source(),
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
            transacted_at: None,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            source: default_source(),
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
