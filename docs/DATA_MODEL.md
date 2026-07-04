# Data model

## Money

**Always `i64` minor units (cents). Never floats.** Sign convention: **outflows
are negative**, inflows positive. Parse decimal strings with
`Money::from_decimal_str` (rounds the 3rd decimal). `Money` is a serde-transparent
newtype, so it serializes as a bare JSON **number** — the frontend receives every
amount as integer cents and formats with `formatCents`. See `core/src/money.rs`.

## Domain types (`core/src/model.rs`)

```rust
Account { id, org, name, kind: AccountKind, balance: Money, currency, last_synced }
AccountKind = Checking | Savings | Credit | Other   // name heuristic; SimpleFIN has no type field

Transaction {
    id,               // SimpleFIN transaction id — the dedup key
    account_id,
    posted: i64,      // epoch seconds
    amount: Money,    // outflow negative
    description,
    payee: Option<String>,
    category: Option<String>,
    category_source: Option<CategorySource>,   // SimpleFin | Rule | Llm | Manual
    pending: bool,
    raw: String,      // original JSON, retained
}
```

`Transaction::merchant()` returns `payee` if non-empty, else `description`. All
enums have `as_str`/`from_str` for string round-tripping in SQLite.

## SQLite schema (`core/src/store.rs`)

| Table | Role | Key notes |
|---|---|---|
| `accounts` | latest account snapshot | upsert by `id` |
| `transactions` | the durable archive | grows past SimpleFIN's ~90-day window; this is the permanent history |
| `category_rules` | learned/manual rules | `match_type` (exact/contains), `pattern` (lowercased), `category` |
| `categories` | the vocabulary | seeded with a default set on first open; constrains the LLM's output |
| `sync_log` | pull history | started/finished/source/added/updated/note |

Indexes on `transactions(account_id)`, `(posted)`, `(pending)`.

## Write invariants (these govern every upsert)

1. **Dedup key = the SimpleFIN transaction id.** Posted transactions upsert by
   id. **Pending transactions are delete-and-replace per synced account each
   pull** — `upsert_transactions` first deletes `pending = 1` rows for every
   account in the batch, then inserts. This avoids a pending charge and its later
   posted version double-counting. `UpsertResult` counts only posted adds/updates.

2. **One merchant normalizer everywhere: `subscriptions::normalize_payee`.** It
   lowercases, strips payment-processor prefixes (`SQ *`, `TST*`, `PYPL*`, …) and
   drops mostly-numeric noise tokens. Categorizer matching, merchant reports, and
   subscription detection all key off it, so a merchant groups identically across
   the whole app. **Never introduce a second normalizer.**

3. **Boundary parsing.** External JSON is deserialized then converted to domain
   types with explicit validation; malformed data returns `SourceError`, never
   leaks defaults (`source::parse_account_set`).

## Read / analysis layer

`query` and `subscriptions` load all transactions and aggregate in memory over a
`TxnFilter { since: Option, until: Option, include_pending: bool }` — **since is
inclusive, until is exclusive**.

- `spend_by_category` / `top_merchants` — sum of **outflows only** (`is_outflow`),
  grouped; merchants grouped by `normalize_payee`. Uncategorized bucketed under
  `None`.
- `monthly_flow` — inflow/outflow/net per month, **bucketed in UTC**.
- `subscriptions::detect` — groups outflows by normalized payee; a subscription
  needs **≥3 occurrences**, a **stable amount** (within `max($1, 5%)` of the
  median), and a **regular cadence** (monthly 26–35 days, or yearly 350–380).
  Sorted by total spend descending.

## Categorization precedence

Rule matching (`categorize::RuleSet`): **exact match beats contains; among
contains matches, the longest pattern wins.** Matches on the normalized merchant.
`set_manual_category(id, cat, learn=true)` writes an **exact** rule on the
normalized merchant, so correcting one transaction categorizes its siblings on the
next `categorize` pass. Sources are stamped: `Rule`, `Llm`, `Manual`.

## Serialization boundary (Rust → TS)

- serde serializes struct fields as **snake_case** and enums as their **variant
  names** (`"Rule"`, `"Monthly"`). `types.ts` mirrors this exactly.
- `Money` and all `*_cents: i64` fields cross as JS **numbers** (cents).
- Query/subscription result types (`CategorySpend`, `MerchantSpend`,
  `MonthlyFlow`, `Subscription`, `Cadence`) derive `Serialize` specifically so the
  Tauri commands can return them.
