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
