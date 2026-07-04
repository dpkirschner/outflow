# Invariants

Non-negotiable rules the tests and correctness depend on. Violating these breaks
data integrity or the domain contract — treat them as load-bearing.

1. **Money is `i64` minor units (cents). Never floats.** Outflows negative,
   inflows positive. Parse via `Money::from_decimal_str`. (`core/src/money.rs`)

2. **Dedup key is the SimpleFIN transaction id.** Posted transactions upsert by
   id. Pending transactions are **delete-and-replace per synced account** each
   pull, to avoid pending→posted double-counting.
   (`store::upsert_transactions`)

3. **`core` has zero GUI/network deps by default.** Network, keychain, and
   encryption are cargo features. The full pipeline must stay runnable via
   `pull --from-file` with no features. Network code lives in `net`, never `core`.

4. **Merchant grouping is always `subscriptions::normalize_payee`.** Categorizer
   matching, merchant reports, and subscription detection all use it, so a
   merchant groups identically everywhere. Do not add a second normalizer.

5. **Boundary parsing.** External JSON is deserialized then converted to domain
   types with explicit validation; malformed data returns `SourceError`, never
   leaks defaults. (`source::parse_account_set`)

6. **The two ports stay swappable.** `TransactionSource` and
   `Categorizer`/`Prompter` are traits precisely so SimpleFIN→Plaid and rules→LLM
   swaps don't touch the domain. Keep transport/IO behind them.

7. **Secrets never touch the DB or argv.** The SimpleFIN access URL and the
   SQLCipher DB key come from env, a 0600 file, or the keychain only.

8. **The DB key is regenerated only when the keychain entry does not exist
   (`NoEntry`)** — never on a locked/denied read, or a new key would orphan the
   existing encrypted DB permanently. (`net::secrets::db_key_get_or_create`)

9. **Analytics key off `Transaction::effective_date()`, not `posted`.** That is
   `transacted_at` when the bank supplied it, else `posted` — the behavioral
   basis. Month bucketing and cadence detection use it; month buckets are in the
   machine's **local timezone** (`chrono::Local`), not UTC. (`query`, `subscriptions`)

10. **Non-`Spending` transactions are excluded from every aggregation** unless
    `TxnFilter.include_non_spending` is set. Suppression lives in `query::passes`
    so all aggregators inherit it and `monthly_flow` drops both legs. The
    transaction list is the intentional exception (it shows every row so the user
    can reclassify). (`query`)

11. **A re-pull never resets a manual `flag`.** `upsert_transactions`' conflict
    update omits the `flag` column; `apply_flags` only ever assigns a rule's flag,
    never resets to `Spending`. Card-payment suppression is only correct when the
    card account is ingested — enforce nothing, but warn
    (`Store::has_credit_account`). (`store`)

12. **Existing DBs migrate through `PRAGMA user_version`.** New columns on an
    existing table need a guarded `ALTER TABLE` in `store::run_migrations` and a
    `SCHEMA_VERSION` bump — `CREATE TABLE IF NOT EXISTS` alone never alters a live
    DB, and a real encrypted production DB holds the user's data. (`store::migrate`)
