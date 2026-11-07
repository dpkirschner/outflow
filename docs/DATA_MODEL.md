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
          last_synced, source }        // source: provenance ("plaid")
AccountKind = Checking | Savings | Credit | Investment | Loan | Other
// Real Plaid type/subtype mapping — credit/* → Credit and loan/* → Loan (both
// balances negated: Plaid reports positive-owed), depository/checking → Checking,
// depository/savings|cd|money market → Savings, investment/* → Investment
// (IRA, 401k, brokerage), and every HSA (depository or investment) →
// Investment so its cash reads as idle. Unknown types → Other. Kind is
// rewritten on every sync, so a mapping change needs no data migration.

Security { security_id, ticker: Option<String>, name, sec_type,
           is_cash }   // is_cash = Plaid is_cash_equivalent || type "cash"
Holding { account_id, security_id, quantity: String, price: String,
          value: Money, cost_basis: Option<Money> }
          // quantity/price are EXACT decimal text (4+ share decimals, sub-cent
          // prices); only value/cost_basis are money
BalanceSnapshot { taken_at, account_id, balance: Money, cash: Money }
```

**The signed breakdown** (`balances::Totals`, also each `DayTotals` of the
trend). Every account adds its balance to exactly one term, by kind:

```
cash (Checking+Savings) + invested (Investment) + credit (Credit)
  + loans (Loan) + other (Other) == net_worth        // exact, to the cent
uninvested = account_cash summed over Investment accounts  // a PART of invested
```

Credit and loans are negative when owed (the UI shows "− Credit owed $410").
Each term equals the total of the balances group of the same name (`Group`:
cash, investments, credit, loans, other), so groups and breakdown agree.
Brokerage cash counts under invested, never under cash.

```rust

Transaction {
    id,               // provider transaction id — the dedup key (stored raw)
    account_id,
    posted: i64,      // epoch seconds — when the bank booked it
    transacted_at: Option<i64>,  // when the purchase happened, if known
                      // (Plaid authorized_date)
    amount: Money,    // outflow negative
    description,      // Plaid name
    payee: Option<String>,       // Plaid merchant_name
    category: Option<String>,
    category_source: Option<CategorySource>, // Plaid | Rule | Llm | Manual
    pending: bool,
    flag: TxnFlag,    // EFFECTIVE flag: the override if set, else Plaid's
                      // (see "Flags: Plaid's classification + overrides")
    raw: String,      // original provider JSON, retained (full Plaid payload)
    source: String,   // provenance ("plaid"; "simplefin" on retired-era rows)
    pfc_primary: Option<String>,   // Plaid personal_finance_category.primary
    pfc_detailed: Option<String>,  // ... .detailed; None off Plaid / when absent
}
TxnFlag = Spending | Transfer | CardPayment | Invest | Save | LoanPaydown
          // Spending = In/Out lanes; every other value is a Moved group.
          // Transfer = between own accounts (or moved, group unknown).

PlaidItem { item_id, institution, cursor: Option<String>, status, created,
            last_synced, investments_consent }   // NON-secret metadata; access token lives in the
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

## SQLite schema (`core/src/store.rs`, SCHEMA_VERSION = 12)

| Table | Role | Key notes |
|---|---|---|
| `accounts` | latest account snapshot | upsert by `id`; `source` column; user-owned `nickname` + `hidden` (never overwritten by a re-pull) |
| `transactions` | the durable archive | grows past the providers' pull windows; this is the permanent history; `source` column; `pfc_primary`/`pfc_detailed` (Plaid's category, refreshed on re-sync) and `flag_overridden` (0/1, see below) |
| `category_rules` | learned/manual rules | `match_type` (exact/contains), `pattern` (lowercased), `category` |
| `flag_rules` | learned/manual suppression rules | same shape but carries a `flag` — an independent axis |
| `categories` | the vocabulary | seeded on first open; constrains the LLM |
| `merchant_overrides` | the per-merchant Ignored mark | `pattern` = normalized merchant, `mark` = `ignored` (the only value since v13) |
| `sync_log` | sync history | one row per source leg per run |
| `plaid_items` | linked Plaid connections | item_id PK, institution, **cursor**, status (`ok`/`login_required`/`error`), created, last_synced, investments_consent (refreshed from `/accounts/get`'s `item` each sync) — no secrets |
| `securities` | instruments seen in holdings | security_id PK; ticker, name, type, is_cash, cusip, isin, close_price (decimal text) + close_price_as_of |
| `holdings` | **current** positions | PK(account_id, security_id); replaced per item each sync (sold positions drop); quantity/price as decimal text, price_as_of, currency, synced_at (the sync that last wrote the row) |
| `balance_snapshots` | **append-only** history | PK(taken_at, account_id); every account, every successful item sync; `cash_cents` from `balances::account_cash` |
| `holding_snapshots` | **append-only** history | PK(taken_at, account_id, security_id); one row per position per sync; quantity, value, cost basis, price (NULL before v8) |
| `investment_transactions` | **archive** of investment activity | id PK (Plaid `investment_transaction_id`); account_id, security_id (nullable), date (`YYYY-MM-DD`), name, type, subtype, quantity/price (decimal text), amount_cents (cash effect, domain sign: out < 0), fees_cents, currency, cancel_txn_id, synced_at. Upserted by id each sync; never deleted (`reset_data` keeps it) |
| `txn_matches` | card-payment pair decisions | UNIQUE(bank_txn_id, card_txn_id); status proposed/accepted/rejected; decided pairs are never re-proposed |

Indexes on `transactions(account_id)`, `(posted)`, `(pending)`.

Schema changes run through the `PRAGMA user_version` migration runner
(`store::migrate`): v1 `transacted_at`+`flag`, v2 `merchant_overrides`, v3
`source` on accounts+transactions (+ new tables), v4 `accounts.nickname`, v5
`accounts.hidden`, v6 `plaid_items.investments_consent`, v7 `securities` +
`holdings` + `balance_snapshots` + `holding_snapshots` (new tables), v8 holdings detail
(securities cusip/isin/close_price/close_price_as_of, holdings
price_as_of/currency/synced_at, holding_snapshots.price), v9
`investment_transactions` (new table), v10 data-only: loan payments still
`spending` re-flagged to `transfer` once (`reflag_loan_payments`), v11
data-only: `transfer` rows split into the Moved groups (`split_moved_groups`),
v12 `transactions.pfc_primary`/`pfc_detailed`/`flag_overridden`, backfilled
once from `raw` (`backfill_pfc_columns`; a row is marked overridden when its
stored flag differs from what its category gives, so no effective flag
changes), v13 data-only: `merchant_overrides` keeps one mark — the ledger's
`dismissed` became `ignored`, `committed`/`kept` rows were deleted. The v10
and v11 backfills still parse `raw`; nothing after v12 does.
Column lists are positional
(`TXN_COLUMNS` / `row_to_txn` / upsert INSERT) — **append only**.

## Write invariants (these govern every upsert)

1. **Dedup key = the provider transaction id** (+ `source` for provenance).
   Live syncs commit through `apply_plaid_batch`: Plaid reports pending
   lifecycle explicitly, so the batch carries explicit deletions (its `removed`
   list plus the `pending_transaction_id`s superseded by posted rows) and
   everything — account upserts, txn upserts, deletions, **cursor advance** —
   commits in one SQLite transaction. `upsert_transactions` is the plain
   id-keyed path for offline fixture ingest. `UpsertResult` counts only posted
   adds/updates.

2. **One merchant normalizer everywhere: `subscriptions::normalize_payee`.**
   Lowercases, strips processor prefixes (`SQ *`, `TST*`, `PYPL*`, …), drops
   mostly-numeric noise tokens. **Never introduce a second normalizer.**

3. **Boundary parsing.** External JSON → domain with explicit validation;
   malformed data returns `SourceError`, never leaks defaults. Plaid signs flip
   here (amounts negate; credit and loan balances negate). Plaid
   `personal_finance_category` seeds `flag` (LOAN_PAYMENTS_CREDIT_CARD_PAYMENT →
   CardPayment; *_INVESTMENT_AND_RETIREMENT_FUNDS → Invest; *_SAVINGS → Save;
   other LOAN_PAYMENTS → LoanPaydown, since paying down a loan counts as
   saving; other TRANSFER_IN/OUT → Transfer) and a conservative category
   (`CategorySource::Plaid`) at parse time. The category is also kept as the
   `pfc_primary`/`pfc_detailed` columns; see below for how the flag follows it.

## Flags: Plaid's classification + overrides

The flag Plaid's category gives (`plaid::flag_from_pfc` over the two PFC
columns) is worked out **on every read**, so changing that mapping (a new
Moved group, say) reclassifies history with no data migration. The stored
`flag` column counts only when `flag_overridden = 1`: a user's flag
(`Store::set_flag`), a flag rule (`apply_flags`) or an accepted card-payment
match. Undoing an accepted match drops the override (`Store::clear_flag`), so
each leg goes back to Plaid's flag. On re-sync the PFC columns refresh and an
un-overridden row follows them; an override is never touched. A row inserted
with a flag other than its category's (fixture or test rows) is stored as an
override, so what goes in reads back. For un-overridden rows the stored `flag`
just mirrors the last seed and is ignored on read.

Code reads Plaid's category from the columns, never by searching `raw`
(`transfers::has_payment_signal` reads `pfc_detailed`; a test fails on any
`.raw.contains(` in `core` production code).

## Periods (`core/src/period.rs`)

`Period { kind: Week|Month|Quarter|Year|Custom, start, end }` is a half-open
range of **local** calendar days `[start, end)`: ISO weeks (Monday first),
months, quarters, years, or a custom range. `Period::parse` reads one spelling
(`2026-11`, `2026-W45`, `2026-Q4`, `2026`, `this-`/`last-` + unit);
`Period::resolve` adds `since`/`until` (`YYYY-MM-DD`, both inclusive).
`bounds()` turns it into epoch seconds at local midnight, so DST weeks are an
hour short or long. API, CLI and future screens all filter through it; see
API.md **Dates**.

## Read / analysis layer

`query` and `recurring` load all transactions and aggregate in memory over
a `TxnFilter { since, until, include_pending, include_non_spending }` — since
inclusive, until exclusive, both against `effective_date()`. Every aggregation
**excludes non-`Spending` transactions** unless `include_non_spending` is set.

- `spend_by_category` — outflows grouped by category, **refunds netted** in
  their own category (so it sums to Out); uncategorized under `None`.
- `top_merchants` — sum of **outflows only**, grouped by `normalize_payee`.
- `lane_of(txn, account_kind) -> Option<Lane>` — the one lane classifier.
  `Lane` = `In`, `Out` (refunds included), `CardPayment`, `Invest`, `Save`,
  `LoanPaydown`, `Between`; `None` = counted nowhere (the far leg of a move,
  or a card-payment leg that is neither money out of checking nor money into
  a card). `flow_lanes` and the `lane` filter both call it, so every lane
  figure is the signed sum of its rows.
- `flow_lanes` — the Flow lanes per month (GLOSSARY), bucketed on
  `effective_date()` in local time:
  - **In**: positive Spending rows whose `pfc_primary` is `INCOME`, or
    `TRANSFER_IN` re-flagged as Spending by the user; with no PFC, a positive
    on any account but credit. The user's `category` never decides the lane.
    `in_uncategorized_cents` is the part of In with no category yet.
  - **Out**: Spending outflows minus **refunds** (every other positive
    Spending row: any other PFC, or no PFC on a credit account), netted in
    their category (uncategorized under `None`).
  - **Moved** by group, from the **spending-side** accounts (checking +
    credit) only: the far leg of a move on a savings/investment/loan account
    isn't summed, so a move counts once whether or not the far account is
    linked. Card payments are the larger of the bank and card sides per
    month (never double-counted, still counted when one leg is missing);
    both sides are reported (`card_payment_bank_cents`,
    `card_payment_card_cents`).
  - **Earned − spent** = In − Out; **gap** = earned − spent − saved; **Saved** = Invest + Save + LoanPaydown out
    of the spending side, minus what came back.
- `search_transactions(store, TxnQuery) -> TxnPage` — the Flow screen and
  CLI `txns` query: case-insensitive text over payee/description/category/
  normalized merchant, filters (account, category, source, flag, amount band,
  `merchant` = exact normalized key, `lane`, `uncategorized`, `direction` = out|in by sign; a lane reads Moved
  rows by itself; on a card payment, out = bank side, in = card side),
  stable sort (date/amount/merchant/category + direction, tie-broken by date
  then id), offset/limit pagination. `TxnPage.total_cents` sums ALL matches,
  not just the page.
- `subscriptions::detect_rhythms(txns)` — the one recurring detector: Spending
  outflows grouped by `normalize_payee`, at least 3 charges, spacing regular
  (most gaps within 0.5×–2× the median gap). Amounts may vary. Returns each
  merchant's charges and its cadence class (Daily…Yearly).
- `recurring(store, through, now) -> RecurringView` — the recurring list
  (GLOSSARY: Recurring) over the 12 calendar months ending with the month that
  contains `through`; `now` decides which months are complete. Per merchant:
  usual monthly cost = mean of the completed months since its first charge in
  the window (months with no charge count as $0; with none complete, median
  charge × frequency); a 6-month spark ending at that month; the change marker
  — **New** if the first charge falls in the last 3 months and the archive has
  older rows, else **Up**/**Down** when the last completed month is more than
  25% (`CHANGE_PCT`) from the mean of up to 3 completed months before it.
  Ignored merchants (`merchant_overrides`) are split into `ignored`. A yearly
  charge can't qualify inside 12 months. The `merchant` filter on
  `search_transactions` (exact normalized key) returns the rows behind a
  merchant.

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

## Categorization precedence

Rule matching (`categorize::RuleSet`): **exact beats contains; among contains,
longest pattern wins**, on the normalized merchant. `set_manual_category(id,
cat, learn=true)` writes an exact rule so siblings follow on the next pass.
Sources stamped: `Plaid | Rule | Llm | Manual`. Plaid-seeded
categories fill only otherwise-empty rows (the categorizer passes touch NULL
categories only; manual fixes overwrite anything).

## Serialization boundary (Rust → TS)

- serde serializes struct fields as **snake_case** and enums as their
  **variant names** (`"Rule"`, `"CardPayment"`). `app/src/types.ts` is
  generated from these structs (`server/src/ts_types.rs`), so it matches; the HTTP API carries these shapes verbatim (there is no
  camelCase translation layer — that died with Tauri).
- `Money` and all `*_cents: i64` fields cross as JS **numbers** (cents).
- CLI `--json` emits the same serde shapes, so local and `--server` output are
  interchangeable for consumers.
