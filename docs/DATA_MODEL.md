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
    posted: i64,      // epoch seconds — when the bank booked it
    transacted_at: Option<i64>,  // epoch seconds — when the purchase happened (SimpleFIN), if given
    amount: Money,    // outflow negative
    description,
    payee: Option<String>,
    category: Option<String>,
    category_source: Option<CategorySource>,   // SimpleFin | Rule | Llm | Manual
    pending: bool,
    flag: TxnFlag,    // Spending (default) | Transfer | CardPayment
    raw: String,      // original JSON, retained
}
TxnFlag = Spending | Transfer | CardPayment   // suppression axis; see below
```

`Transaction::merchant()` returns `payee` if non-empty, else `description`.
`Transaction::effective_date()` returns `transacted_at` if present, else `posted`
— the **behavioral basis date** all month bucketing and cadence math keys off, so
analytics reflect behavior, not bookkeeping. All enums have `as_str`/`from_str`
for string round-tripping in SQLite.

## SQLite schema (`core/src/store.rs`)

| Table | Role | Key notes |
|---|---|---|
| `accounts` | latest account snapshot | upsert by `id` |
| `transactions` | the durable archive | grows past SimpleFIN's ~90-day window; this is the permanent history |
| `category_rules` | learned/manual rules | `match_type` (exact/contains), `pattern` (lowercased), `category` |
| `flag_rules` | learned/manual suppression rules | same shape as `category_rules` but carries a `flag` — an independent axis from categories |
| `categories` | the vocabulary | seeded with a default set on first open; constrains the LLM's output |
| `sync_log` | pull history | started/finished/source/added/updated/note |

Indexes on `transactions(account_id)`, `(posted)`, `(pending)`.

`transactions` gained `transacted_at INTEGER` (nullable) and
`flag TEXT NOT NULL DEFAULT 'spending'`. Schema changes to existing tables run
through a **`PRAGMA user_version` migration runner** in `store::migrate` — fresh
DBs get the columns from the `CREATE TABLE`, existing DBs get idempotent
`ALTER TABLE ADD COLUMN`s (guarded against re-adding). Bump `SCHEMA_VERSION` and
add a guarded block there for any future column add.

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
`TxnFilter { since, until, include_pending, include_non_spending }` — **since is
inclusive, until is exclusive**, both compared against `effective_date()`. Every
aggregation **excludes non-`Spending` transactions** (transfers, card payments)
unless `include_non_spending` is set — that suppression lives in `passes()`, so
all three aggregators inherit it. The transaction **list** deliberately keeps
non-Spending rows visible so the user can reclassify them.

- `spend_by_category` / `top_merchants` — sum of **outflows only** (`is_outflow`),
  grouped; merchants grouped by `normalize_payee`. Uncategorized bucketed under
  `None`.
- `monthly_flow` — inflow/outflow/net per month, **bucketed on `effective_date()`
  in the machine's local timezone** (via `chrono::Local`, DST-correct). Both legs
  drop non-Spending, so a transfer-in isn't income and a card-payment-out isn't
  spend.
- `subscriptions::detect` — groups **Spending** outflows by normalized payee; a
  subscription needs **≥3 occurrences**, a **stable amount** (within `max($1, 5%)`
  of the median), and a **regular cadence** (monthly 26–35 days, or yearly
  350–380). Sorted by total spend descending.
- `subscriptions::detect_rhythms` — the same grouping and cadence test **without**
  the stable-amount constraint (cadence keyed off `effective_date()`). Returns a
  `RhythmEntry { merchant, cadence, occurrence_count, median_amount_cents,
  amount_min_cents, amount_max_cents, monthly_estimate_cents, last_seen, trend }`
  per recurring variable-amount merchant. `monthly_estimate` = median (monthly) or
  median/12 (yearly); `trend` compares the earlier vs later half of amounts. This
  is the rhythm-roster row contract.

## Suppression flag (transfers & card payments)

`TxnFlag` is an axis independent of category. `Transfer` (money between the user's
own accounts) and `CardPayment` (a checking→card payment) are excluded from all
spend analytics so they aren't counted as consumption. `set_flag(id, flag,
learn=true)` writes an **exact** `flag_rule` on the normalized merchant (mirroring
`set_manual_category`); `apply_flags()` re-applies rules to siblings and **never
resets a row to `Spending`**. A re-pull preserves a manual flag (the upsert's
conflict-update omits the `flag` column). **Card-payment suppression only nets
correctly when the card account is also ingested** — otherwise the payment is
suppressed with no offsetting charges. `Store::has_credit_account()` backs the
app's non-blocking warning for that case.

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
  `MonthlyFlow`, `Subscription`, `Cadence`, `RhythmEntry`, `Trend`) derive
  `Serialize` specifically so the Tauri commands can return them.
- `TxnFlag` / `Trend` cross as their variant names (`"Transfer"`, `"Rising"`).
  `TxnFlag` also derives `Deserialize`, so `set_flag` accepts the variant name
  directly as its argument.
