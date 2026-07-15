# Data model

## Money

**Always `i64` minor units (cents). Never floats.** Sign convention: **outflows
are negative**, inflows positive. Parse decimal strings with
`Money::from_decimal_str` (rounds the 3rd decimal). Plaid amounts arrive as
JSON doubles — they go through `serde_json::Number::to_string()` (shortest
round-trip representation, exact for bank decimals) into the same parser, then
**negate** (Plaid positive = money out). `Money` is a serde-transparent
newtype, so it serializes as a bare JSON **number** — the frontend receives
every amount as integer cents. See `core/src/money.rs`, `core/src/plaid.rs`.

## Domain types (`core/src/model.rs`)

```rust
Account { id, org, name, kind: AccountKind, balance: Money, currency,
          last_synced, source }        // source: "simplefin" | "plaid"
AccountKind = Checking | Savings | Credit | Other
// SimpleFIN: name heuristic. Plaid: real type/subtype mapping — credit/* →
// Credit (balance negated: Plaid reports positive-owed), depository/checking →
// Checking, depository/savings|cd|money market|hsa → Savings.

Transaction {
    id,               // provider transaction id — the dedup key (stored raw)
    account_id,
    posted: i64,      // epoch seconds — when the bank booked it
    transacted_at: Option<i64>,  // when the purchase happened, if known
                      // (SimpleFIN transacted_at / Plaid authorized_date)
    amount: Money,    // outflow negative
    description,      // SimpleFIN description / Plaid name
    payee: Option<String>,       // SimpleFIN payee / Plaid merchant_name
    category: Option<String>,
    category_source: Option<CategorySource>, // SimpleFin | Plaid | Rule | Llm | Manual
    pending: bool,
    flag: TxnFlag,    // Spending (default) | Transfer | CardPayment
    raw: String,      // original provider JSON, retained (full Plaid payload)
    source: String,   // "simplefin" | "plaid" — provenance, never id prefixes
}
TxnFlag = Spending | Transfer | CardPayment   // suppression axis; see below

PlaidItem { item_id, institution, cursor: Option<String>, status, created,
            last_synced }   // NON-secret metadata; access token lives in the
                            // 0600 tokens file, never here
TxnMatch { id, bank_txn_id, card_txn_id, status: Proposed|Accepted|Rejected,
           confidence: High|Medium, reason, created }
SyncEntry { id, started, finished, source, added, updated, note }
```

`Transaction::merchant()` returns `payee` if non-empty, else `description`.
`Transaction::effective_date()` returns `transacted_at` if present, else
`posted` — the **behavioral basis date** all month bucketing and cadence math
keys off. Plaid calendar dates (`YYYY-MM-DD`) are anchored at **local noon** so
they never straddle a month boundary. All enums have `as_str`/`from_str` for
string round-tripping in SQLite.

## SQLite schema (`core/src/store.rs`, SCHEMA_VERSION = 3)

| Table | Role | Key notes |
|---|---|---|
| `accounts` | latest account snapshot | upsert by `id`; `source` column |
| `transactions` | the durable archive | grows past the providers' pull windows; this is the permanent history; `source` column |
| `category_rules` | learned/manual rules | `match_type` (exact/contains), `pattern` (lowercased), `category` |
| `flag_rules` | learned/manual suppression rules | same shape but carries a `flag` — an independent axis |
| `categories` | the vocabulary | seeded on first open; constrains the LLM |
| `merchant_overrides` | per-merchant ledger marks | `Committed` / `Dismissed` / `Kept` |
| `sync_log` | sync history | one row per source leg per run |
| `plaid_items` | linked Plaid connections | item_id PK, institution, **cursor**, status (`ok`/`login_required`/`error`), created, last_synced — no secrets |
| `txn_matches` | card-payment pair decisions | UNIQUE(bank_txn_id, card_txn_id); status proposed/accepted/rejected; decided pairs are never re-proposed |

Indexes on `transactions(account_id)`, `(posted)`, `(pending)`.

Schema changes run through the `PRAGMA user_version` migration runner
(`store::migrate`): v1 `transacted_at`+`flag`, v2 `merchant_overrides`, v3
`source` on accounts+transactions (+ new tables). Column lists are positional
(`TXN_COLUMNS` / `row_to_txn` / upsert INSERT) — **append only**.

## Write invariants (these govern every upsert)

1. **Dedup key = the provider transaction id** (+ `source` for provenance).
   SimpleFIN (`upsert_transactions`): pending rows are **delete-and-replace per
   synced account** each pull (the bridge re-sends the full pending set).
   Plaid (`apply_plaid_batch`): **no pending sweep** — Plaid reports lifecycle
   explicitly, so the batch carries explicit deletions (its `removed` list plus
   the `pending_transaction_id`s superseded by posted rows) and everything —
   account upserts, txn upserts, deletions, **cursor advance** — commits in one
   SQLite transaction. `UpsertResult` counts only posted adds/updates.

2. **One merchant normalizer everywhere: `subscriptions::normalize_payee`.**
   Lowercases, strips processor prefixes (`SQ *`, `TST*`, `PYPL*`, …), drops
   mostly-numeric noise tokens. **Never introduce a second normalizer.**

3. **Boundary parsing.** External JSON → domain with explicit validation;
   malformed data returns `SourceError`, never leaks defaults. Plaid signs flip
   here (amounts negate; credit balances negate). Plaid
   `personal_finance_category` seeds `flag` (LOAN_PAYMENTS_CREDIT_CARD_PAYMENT →
   CardPayment, TRANSFER_IN/OUT → Transfer) and a conservative category
   (`CategorySource::Plaid`) at parse time — seed-only, since the upsert never
   overwrites `flag` on conflict.

## Read / analysis layer

`query` and `subscriptions` load all transactions and aggregate in memory over
a `TxnFilter { since, until, include_pending, include_non_spending }` — since
inclusive, until exclusive, both against `effective_date()`. Every aggregation
**excludes non-`Spending` transactions** unless `include_non_spending` is set.

- `spend_by_category` / `top_merchants` — sum of **outflows only**, grouped;
  merchants by `normalize_payee`; uncategorized under `None`.
- `monthly_flow` — inflow/outflow/net per month, bucketed on `effective_date()`
  in local time. Both legs drop non-Spending, so a card payment is neither
  income (card side) nor spend (checking side).
- `search_transactions(store, TxnQuery) -> TxnPage` — the Outflows screen and
  CLI `txns` query: case-insensitive text over payee/description/category/
  normalized merchant, filters (account, category, source, flag, amount band),
  stable sort (date/amount/merchant/category + direction, tie-broken by date
  then id), offset/limit pagination. `TxnPage.total_cents` sums ALL matches,
  not just the page.
- `subscriptions::detect` — fixed-amount recurring (≥3 occurrences, stable
  amount, monthly/yearly cadence).
- `subscriptions::detect_rhythms(txns, now)` — variable-amount stream detector
  (cadence classes Daily…Yearly, completed-month monthly estimate, trend,
  sparkline). Powers the ledger streams.

## Card-payment matching (`core/src/transfers.rs`)

`detect_card_payments(accounts, txns, already_decided, opts)` pairs an outflow
on a Checking/Savings account with an equal-magnitude inflow on a Credit
account within ≤5 days (behavioral dates), skipping pending rows and decided
pairs. Confidence **High** = a payment signal (lexicon hit on the normalized
merchant — payment/pymt/autopay/… — or a Plaid card-payment/transfer
classification on either leg) AND an unambiguous 1:1 pairing; everything else
is **Medium**. The sync engine auto-accepts High (flags both legs
`CardPayment`, retires competing proposals) and queues Medium for the Review
screen. Accepting/rejecting is idempotent and survives re-sync (UNIQUE pair +
flag-preserving upsert). Undoing an accept restores both legs to `Spending`.

## Rhythm ledger (`core/src/ledger.rs`)

`ledger(store, since, now) -> LedgerView` partitions a window **once** into
streams / committed / notable / transfers / noise with per-stream `Source`
chips (card last-4 parsed from the account name vs ACH). Unchanged by the
Plaid migration except that Plaid account names carry their mask (e.g.
"Plaid Credit Card (3333)") so last-4 attribution works.

## Categorization precedence

Rule matching (`categorize::RuleSet`): **exact beats contains; among contains,
longest pattern wins**, on the normalized merchant. `set_manual_category(id,
cat, learn=true)` writes an exact rule so siblings follow on the next pass.
Sources stamped: `SimpleFin | Plaid | Rule | Llm | Manual`. Plaid-seeded
categories fill only otherwise-empty rows (the categorizer passes touch NULL
categories only; manual fixes overwrite anything).

## Serialization boundary (Rust → TS)

- serde serializes struct fields as **snake_case** and enums as their
  **variant names** (`"Rule"`, `"CardPayment"`). `app/src/types.ts` mirrors
  this exactly; the HTTP API carries these shapes verbatim (there is no
  camelCase translation layer — that died with Tauri).
- `Money` and all `*_cents: i64` fields cross as JS **numbers** (cents).
- CLI `--json` emits the same serde shapes, so local and `--server` output are
  interchangeable for consumers.
