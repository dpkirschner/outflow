# Invariants

Non-negotiable rules the tests and correctness depend on. Violating these breaks
data integrity or the domain contract — treat them as load-bearing.

1. **Money is `i64` minor units (cents). Never floats.** Outflows negative,
   inflows positive. Parse via `Money::from_decimal_str`. Plaid's JSON doubles
   go through `serde_json::Number::to_string()` into that same decimal path —
   never f64 arithmetic. (`core/src/money.rs`, `core/src/plaid.rs`)

2. **Dedup key is the provider transaction id**, stored raw; provenance lives
   in the `source` column ("plaid"; "simplefin" on rows from the retired era),
   never in id prefixes. Live syncs go through `store::apply_plaid_batch`,
   which applies upserts, explicit deletions (Plaid `removed` + superseded
   `pending_transaction_id`s), and the cursor advance in **one SQLite
   transaction**. `upsert_transactions` is the plain id-keyed path for offline
   fixture ingest.

3. **`core` has zero GUI/network deps by default.** Network and encryption
   are cargo features. The full pipeline must stay runnable via
   `pull --from-file` with no features. Network code lives in `net`/`server`,
   never `core`.

4. **Merchant grouping is always `subscriptions::normalize_payee`.** Categorizer
   matching, merchant reports, subscription detection, transaction search, and
   the card-payment matcher's lexicon all use it. Do not add a second normalizer.

5. **Boundary parsing.** External JSON is deserialized then converted to domain
   types with explicit validation; malformed data returns `SourceError`, never
   leaks defaults. Plaid sign conventions flip at this boundary: amounts negate
   (Plaid positive = money out), credit balances negate (positive-owed →
   negative). (`plaid::parse_sync_page`, `plaid::parse_accounts_get`,
   `plaid::parse_fixture`)

6. **The ports stay swappable.** Sources follow the fetch-in-net / parse-in-core
   shape; `Categorizer`/`Prompter` are traits. Keep transport/IO out of the
   domain.

7. **Secrets never touch the DB or argv.** The Plaid client secret, Plaid
   access tokens, and the SQLCipher DB key come from env or a 0600 file only
   (there is no keychain path — launchd has no unlocked GUI keychain). Plaid
   access tokens live in the 0600 tokens file keyed by item_id
   (`net::plaid_tokens`); the DB's `plaid_items` table holds only non-secret
   metadata (institution, cursor, status).

8. **Analytics key off `Transaction::effective_date()`, not `posted`.** That is
   `transacted_at` when the bank supplied it, else `posted` — the behavioral
   basis. Month bucketing and cadence detection use it; month buckets are in
   the machine's **local timezone** (`chrono::Local`), not UTC. Plaid calendar
   dates are anchored at **local noon** so they can never straddle a month
   boundary. (`query`, `subscriptions`, `plaid`)

9. **Non-`Spending` transactions are excluded from every aggregation** unless
   `TxnFilter.include_non_spending` is set. Suppression lives in
   `query::passes` so all aggregators inherit it and `monthly_flow` drops both
   legs. (`query`)

10. **User edits survive re-sync.** The upsert conflict-updates omit any
    user-owned column: transactions omit `flag`, accounts omit `nickname`
    (and never write `hidden` at all — new accounts default visible). A
    re-pull carries neither (it binds `flag`'s parse-time seed / a NULL nickname
    on *insert* only), so a stored value is never clobbered on *conflict*.
    Likewise `apply_flags` only ever assigns a rule's flag, never resets to
    `Spending`; Plaid `personal_finance_category` hints seed flags at parse time
    only. Card-payment suppression is only correct when the card account is
    ingested — warn via `Store::has_credit_account`. (`store`, `plaid`)

11. **Existing DBs migrate through `PRAGMA user_version`.** New columns on an
    existing table need a guarded `ALTER TABLE` in `store::run_migrations` and a
    `SCHEMA_VERSION` bump — `CREATE TABLE IF NOT EXISTS` alone never alters a
    live DB. Column lists (`TXN_COLUMNS`, `row_to_txn`, the upsert INSERT) are
    positional: **append only**. (`store::migrate`)

12. **The Plaid cursor only advances inside `apply_plaid_batch`'s transaction.**
    Never persist a cursor whose batch didn't commit — a crash between the two
    would silently skip transactions forever. Pagination-mutation errors restart
    from the stored cursor. (`store::apply_plaid_batch`, `server::sync`)

13. **A decided card-payment pair is never re-proposed.** `txn_matches` has a
    UNIQUE(bank, card) constraint and the detector receives every decided pair;
    rejecting is permanent unless the user re-accepts by hand. Auto-accept only
    on High confidence (payment signal AND unambiguous pairing).
    (`transfers::detect_card_payments`, `store::insert_match`)

14. **Hidden accounts are filtered at the store, not per view.** `Store::accounts`
    and `Store::all_transactions` (and `uncategorized`) exclude hidden accounts,
    so every list, dropdown, balance and aggregate drops them at once — new
    features inherit it for free. Only the Connections screen
    (`all_accounts`, `?include_hidden=true`) and maintenance passes that must
    keep hidden data correct (`apply_flags`, card-payment matching via
    `all_transactions_including_hidden`) see them. Hidden data keeps syncing;
    un-hiding restores it. (`store`)

15. **Holdings are current state; snapshots are append-only history.**
    `Store::record_item_snapshot` runs once per successful item sync, in one
    transaction: it replaces the item's investment-account positions (when
    fresh holdings arrived) and appends a `balance_snapshots` row per account
    plus a `holding_snapshots` row per position. Nothing updates or deletes a
    snapshot (`reset_data` keeps them). A failed holdings fetch never fails
    the item or flips its status — last-known positions stay current and the
    leg reports a `warning`. Share quantities and unit prices stay exact
    decimal text; only `value`/`cost_basis` are `Money`. (`store`,
    `server/src/sync.rs::snapshot_item`)

16. **One cash rule: `balances::account_cash`.** Checking/savings are all cash;
    an investment account's cash is its cash-equivalent holdings (Plaid
    `is_cash_equivalent` or `type: cash`, e.g. VMFXX) plus any balance the
    holdings don't cover (so a holdings-less HSA is 100% cash); credit/other
    hold none. Snapshots, the Balances view and the CLI all call it — don't
    compute cash anywhere else. (`core/src/balances.rs`)
