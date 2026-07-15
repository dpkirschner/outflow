# Invariants

Non-negotiable rules the tests and correctness depend on. Violating these breaks
data integrity or the domain contract — treat them as load-bearing.

1. **Money is `i64` minor units (cents). Never floats.** Outflows negative,
   inflows positive. Parse via `Money::from_decimal_str`. Plaid's JSON doubles
   go through `serde_json::Number::to_string()` into that same decimal path —
   never f64 arithmetic. (`core/src/money.rs`, `core/src/plaid.rs`)

2. **Dedup key is the provider transaction id**, stored raw; provenance lives
   in the `source` column ("simplefin" | "plaid"), never in id prefixes.
   SimpleFIN: posted rows upsert by id, **pending rows are delete-and-replace
   per synced account** each pull. Plaid: **no pending sweep** —
   `store::apply_plaid_batch` applies upserts, explicit deletions (Plaid
   `removed` + superseded `pending_transaction_id`s), and the cursor advance in
   **one SQLite transaction**. (`store::upsert_transactions`,
   `store::apply_plaid_batch`)

3. **`core` has zero GUI/network deps by default.** Network, keychain, and
   encryption are cargo features. The full pipeline must stay runnable via
   `pull --from-file` with no features. Network code lives in `net`/`server`,
   never `core`.

4. **Merchant grouping is always `subscriptions::normalize_payee`.** Categorizer
   matching, merchant reports, subscription detection, transaction search, and
   the card-payment matcher's lexicon all use it. Do not add a second normalizer.

5. **Boundary parsing.** External JSON is deserialized then converted to domain
   types with explicit validation; malformed data returns `SourceError`, never
   leaks defaults. Plaid sign conventions flip at this boundary: amounts negate
   (Plaid positive = money out), credit balances negate (positive-owed →
   negative). (`source::parse_account_set`, `plaid::parse_sync_page`,
   `plaid::parse_accounts_get`)

6. **The ports stay swappable.** Sources follow the fetch-in-net / parse-in-core
   shape; `Categorizer`/`Prompter` are traits. Keep transport/IO out of the
   domain.

7. **Secrets never touch the DB or argv.** SimpleFIN access URL, Plaid client
   secret, Plaid access tokens, and the SQLCipher DB key come from env, a 0600
   file, or the keychain only. Plaid access tokens live in the 0600 tokens file
   keyed by item_id (`net::plaid_tokens`); the DB's `plaid_items` table holds
   only non-secret metadata (institution, cursor, status).

8. **The DB key is regenerated only when the keychain entry does not exist
   (`NoEntry`)** — never on a locked/denied read, or a new key would orphan the
   existing encrypted DB permanently. (`net::secrets::db_key_get_or_create`)

9. **Analytics key off `Transaction::effective_date()`, not `posted`.** That is
   `transacted_at` when the bank supplied it, else `posted` — the behavioral
   basis. Month bucketing and cadence detection use it; month buckets are in
   the machine's **local timezone** (`chrono::Local`), not UTC. Plaid calendar
   dates are anchored at **local noon** so they can never straddle a month
   boundary. (`query`, `subscriptions`, `plaid`)

10. **Non-`Spending` transactions are excluded from every aggregation** unless
    `TxnFilter.include_non_spending` is set. Suppression lives in
    `query::passes` so all aggregators inherit it and `monthly_flow` drops both
    legs. (`query`)

11. **User flag decisions survive re-sync.** The upsert conflict-update omits
    the `flag` column; `apply_flags` only ever assigns a rule's flag, never
    resets to `Spending`; Plaid `personal_finance_category` hints seed flags at
    parse time only (insert, not update). Card-payment suppression is only
    correct when the card account is ingested — warn via
    `Store::has_credit_account`. (`store`, `plaid`)

12. **Existing DBs migrate through `PRAGMA user_version`.** New columns on an
    existing table need a guarded `ALTER TABLE` in `store::run_migrations` and a
    `SCHEMA_VERSION` bump — `CREATE TABLE IF NOT EXISTS` alone never alters a
    live DB. Column lists (`TXN_COLUMNS`, `row_to_txn`, the upsert INSERT) are
    positional: **append only**. (`store::migrate`)

13. **The Plaid cursor only advances inside `apply_plaid_batch`'s transaction.**
    Never persist a cursor whose batch didn't commit — a crash between the two
    would silently skip transactions forever. Pagination-mutation errors restart
    from the stored cursor. (`store::apply_plaid_batch`, `server::sync`)

14. **A decided card-payment pair is never re-proposed.** `txn_matches` has a
    UNIQUE(bank, card) constraint and the detector receives every decided pair;
    rejecting is permanent unless the user re-accepts by hand. Auto-accept only
    on High confidence (payment signal AND unambiguous pairing).
    (`transfers::detect_card_payments`, `store::insert_match`)
