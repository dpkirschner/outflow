# HTTP API

The server (`outflow-server`) mounts the whole JSON API under **`/api`**. Base
URL is `http://127.0.0.1:8080/api` locally, or
`https://<mini>.<tailnet>.ts.net/api` over the tailnet. Every front-end (the
React SPA, the CLI in `--server` mode) talks to this surface; `app/src/api.ts`
is the reference client.

Most agents do **not** need to call this directly — the `outflow` CLI built
with `--features client` wraps it and prints the same JSON (see
[DEVELOPMENT.md](DEVELOPMENT.md) and [DEPLOYMENT.md §7](DEPLOYMENT.md)). This
doc is for when you want the raw endpoints.

## Conventions

- **Wire format is serde verbatim**: struct fields are **snake_case**, enums are
  their **variant names** as strings (`"Rule"`, `"CardPayment"`, `"High"`), and
  `Money` / every `*_cents` field crosses as a bare JSON **number of cents**
  (never an object). There is no camelCase layer. Response DTO shapes are
  defined in [DATA_MODEL.md](DATA_MODEL.md). `app/src/types.ts` is
  **generated** from the Rust structs (`server/src/ts_types.rs`, ts-rs behind
  the `ts` feature); `server/contract/responses.json` snapshots one instance
  of every response DTO. `make types` regenerates both; `make check` fails
  when either is stale.
- **Dates: one convention.** Every read endpoint that filters by date takes
  the same three params, resolved by `core::period::Period` in local time:
  - `period` — a calendar period: `2026-11` (month), `2026-W45` (ISO week,
    Monday first), `2026-Q4`, `2026`, or relative `this-week` / `last-week` /
    `this-month` / `last-month` / `this-quarter` / `last-quarter` /
    `this-year` / `last-year`.
  - `since` / `until` — `YYYY-MM-DD`, **both inclusive**. A missing `since`
    means from the start of history; a missing `until` means through today.
  - `period` together with `since`/`until` is a `400`, as is a bad spelling.
    Neither means all of history.
  - **Legacy:** a bare integer `since`/`until` is read as epoch seconds
    (`since` inclusive, `until` exclusive), the form the SPA still sends. New
    endpoints don't accept it.
  - Exceptions: `/ledger` `window` and `/perks` `lookback` are rolling
    windows, not calendar periods (the ledger window goes with ho-2mj.3).
- **Timestamps** in response bodies are **epoch seconds** (`i64`) unless a
  field says `YYYY-MM-DD`.
- **Errors** return the matching 4xx/5xx status with a JSON body
  `{ "error": "<message>" }`. An unknown `/api/*` path is a JSON `404`
  `{ "error": "no such API route" }` (it never falls through to the SPA shell).
- **CORS / content type**: POST bodies are JSON; send `Content-Type:
  application/json`.

## Auth

Bearer token, enforced by a middleware over all of `/api/*`
(`server/src/main.rs::require_bearer`):

| Server config | Effect |
|---|---|
| neither `OUTFLOW_API_TOKEN` nor `OUTFLOW_API_TOKEN_RO` set | API is **open** — anything that can reach the port gets in (the zero-config single-user default; "the tailnet is the boundary") |
| `OUTFLOW_API_TOKEN` (full) | `Authorization: Bearer <token>` → all endpoints |
| `OUTFLOW_API_TOKEN_RO` (read-only) | `Authorization: Bearer <token>` → **GET only**; any POST/DELETE returns **403** |

Setting **either** token flips the whole surface to deny-by-default: missing or
wrong token → **401**. The read-only tier is **method-based**, not a route
allowlist — every read is a `GET` and every mutation is a `POST`/`DELETE`, so a
new mutating route is denied to the RO token automatically. **Give agents the
RO token.**

```bash
curl -s -H "Authorization: Bearer $RO_TOKEN" \
  "http://127.0.0.1:8080/api/accounts" | jq
```

## Read endpoints (GET — RO token sufficient)

| Method · Path | Query params | Returns |
|---|---|---|
| `GET /accounts` | `include_hidden` (default `false`) | `Account[]` — latest snapshot per account incl. `balance` (cents; credit balances negated) and `hidden`. Hidden accounts are omitted unless `include_hidden=true` (the Connections screen) |
| `GET /transactions` | see **Transaction query** below | `TxnPage { items: Transaction[], total, total_cents }` — paginated |
| `GET /spend/categories` | `period` / `since` / `until`, `pending`, `transfers` | `CategorySpend[]` — outflow sum per category (uncategorized under `null`) |
| `GET /merchants` | `period` / `since` / `until`, `pending`, `transfers`, `limit` (default 15) | `MerchantSpend[]` — outflow sum per normalized merchant |
| `GET /flow` | `period` / `since` / `until`, `pending` | `FlowMonth[]` — Flow lanes per month (local-time buckets): `in_cents`, `out_cents` (refunds netted), `refunds_cents`, `moved` (`card_payment_cents` + `invest`/`save`/`loan_paydown`/`between` as `{out_cents, in_cents}` from checking + cards), `earned_minus_spent_cents`, `saved_cents` |
| `GET /subscriptions` | — | `Subscription[]` — fixed-amount recurring (≥3 occurrences) |
| `GET /ledger` | `window` = `3mo`\|`6mo`(default)\|`12mo`\|`all` | `LedgerView` — the rhythm ledger (streams / committed / notable / transfers / noise) |
| `GET /stream_occurrences` | `merchant` (required, normalized name), `window` | `Transaction[]` — the raw rows behind one stream |
| `GET /stream_preview` | `merchant` (required), `window` | `Stream \| null` — what a noise merchant would become if promoted (`null` if no spending in window) |
| `GET /categories` | — | `string[]` — the category vocabulary |
| `GET /has_credit_account` | — | `bool` — whether a credit account is ingested (card-payment suppression is only correct when true) |
| `GET /sync_log` | `limit` (default 50) | `SyncEntry[]` — most recent sync runs |
| `GET /balances` | — | `BalancesView { totals: {net_worth, cash, invested, credit}, groups: [{group: cash\|investments\|credit\|other, total, accounts: AccountBalance[]}] }`. `AccountBalance` = `{ account, group, cash, invested, cash_bp, idle_cash, holdings: HoldingRow[] }` (largest position first; `quantity`/`price` are decimal strings, `*_bp` basis points). Visible accounts only. `idle_cash` = investment account with ≥ $1,000 and ≥ 5% cash |
| `GET /balances/history` | `period` / `since` / `until` (optional) | `DayTotals[]` — `{day: "YYYY-MM-DD", net_worth, cash, invested, credit}` per local day from the per-sync snapshots, each account's last value carried forward |
| `GET /holdings` | `account` (id or label substring), `ticker` (ticker or security_id), both optional, case-insensitive | `PositionRow[]` — every current position on visible accounts, largest first: `{account_id, account, security_id, ticker, label, name, sec_type, is_cash, cusip, isin, currency, quantity, price, price_as_of, close_price, close_price_as_of, value, cost_basis, unit_cost, gain, gain_bp, share_bp, synced_at}`. `quantity`/`price`/`close_price`/`unit_cost` are decimal strings (`unit_cost` = basis ÷ qty, 4 places); `price` is the institution's, `close_price` Plaid's end-of-day (neither is a live quote); dates `YYYY-MM-DD`; `synced_at` = epoch secs of the sync that last refreshed the position (stale if a holdings fetch has been failing). Detail fields are `null` until the first holdings sync on v8. Each investment account whose balance its positions don't cover also gets one synthetic row: `security_id` `cash:uninvested`, `is_cash: true`, `quantity` = dollars at `price` `1` |
| `GET /activity` | `account` (id or label substring), `ticker` (ticker or security_id), `type` (Plaid type or subtype: `buy`, `sell`, `dividend`, `contribution`, `fee`, …), `period` / `since` / `until` — all optional | `ActivityRow[]` — archived investment transactions on visible accounts, newest first: `{id, date, account_id, account, security_id, ticker, label, name, type, subtype, quantity, price, amount, fees, currency, cancel_txn_id}`. `amount` is the cash effect on the account (negative = cash out, e.g. a buy; positive = cash in, e.g. a dividend); `quantity`/`price` decimal strings |
| `GET /holdings/history` | `period` / `since` / `until`, `account`, `ticker` — all optional | `PositionPoint[]` — `{taken_at, day, account_id, account, security_id, label, quantity, price, value, cost_basis, gain}` per position per holdings sync, oldest first (`price` null on pre-v8 snapshots) |
| `GET /perks` | `lookback` = `3mo`\|`6mo`\|`12mo`(default)\|`all`, `expiring_days` (default 14) | `PerksReport { cards: CardStatus[], findings: Finding[] }` — card offers from the `OUTFLOW_PERKS` file(s) matched against spending; `kind` = `add_offer`\|`wrong_card`\|`expiring`, sorted by `est_cash_cents` desc. `400` if `OUTFLOW_PERKS` is unset; the file is re-read per request |

### Transaction query (`GET /transactions`)

All optional; a missing filter set means **everything** (pending included).

| Param | Type | Meaning |
|---|---|---|
| `period` / `since` / `until` | see **Dates** above | window on `effective_date()` |
| `pending` | bool (default `true`) | include pending rows |
| `transfers` | bool (default `false`) | include Moved (non-Spending) rows |
| `q` | string | case-insensitive text over payee/description/category/normalized merchant |
| `account` | string | filter by `account_id` |
| `category` | string | filter by category name |
| `source` | string | provenance (`plaid`) |
| `flag` | enum | `Spending`\|`Transfer`\|`CardPayment`\|`Invest`\|`Save`\|`LoanPaydown` — filtering by a Moved flag auto-includes those rows |
| `min_cents` / `max_cents` | i64 | amount band (cents) |
| `sort` | string | `date`(default)\|`amount`\|`merchant`\|`category`; a bad key is a `400` |
| `dir` | string | `asc`\|`desc`(default) |
| `offset` / `limit` | usize | pagination; `limit` default 100, **capped at 1000** |

`TxnPage.total` / `total_cents` cover **all** matches, not just the returned
page — page through with `offset` until you've read `total` items.

```bash
# Most recent 50 coffee transactions last month, as an agent:
curl -s -H "Authorization: Bearer $RO_TOKEN" \
  "http://127.0.0.1:8080/api/transactions?q=coffee&period=last-month&limit=50&sort=date&dir=desc" | jq
```

## Mutation endpoints (POST/DELETE — full token required)

An RO token gets **403** on all of these.

| Method · Path | Body | Effect / returns |
|---|---|---|
| `POST /categorize` | — | run the rule categorizer over uncategorized rows → `{ rule, remaining }` |
| `POST /categorize_llm` | — | LLM pass over merchants rules couldn't place (needs `ANTHROPIC_API_KEY` on the server) → count (`number`) |
| `POST /txn/{txn_id}/category` | `{ category, learn? }` | set a manual category; `learn:true` writes an exact rule so siblings follow |
| `POST /txn/{txn_id}/flag` | `{ flag, learn? }` | set the flag (`Spending`, or a Moved group: `Transfer`\|`CardPayment`\|`Invest`\|`Save`\|`LoanPaydown`) |
| `POST /account/{account_id}/nickname` | `{ nickname }` | set/clear account nickname (empty clears) |
| `POST /account/{account_id}/hidden` | `{ hidden }` | hide/un-hide an account: its rows leave every list, aggregate, dropdown and match review (data keeps syncing). `404` for an unknown id |
| `POST /apply_flags` | — | re-apply flag rules → count (`number`) |
| `POST /mark_stream` | `{ merchant, mark }` | ledger mark (`Committed`\|`Dismissed`\|`Kept`) |
| `POST /clear_stream_mark` | `{ merchant }` | clear a ledger mark |
| `POST /pull` | — | trigger a live Plaid sync now → `SyncReport` (a pull with no linked items returns a clean empty report without touching credentials) |
| `POST /reset_data` | — | clear transactions/accounts/sync_log/matches and reset Plaid cursors — **destructive**, back up the DB first (cannot recover rows older than Plaid's window) |

### Plaid Link lifecycle (`server/src/plaid_routes.rs`)

| Method · Path | Body | Returns |
|---|---|---|
| `POST /plaid/link_token` | `{ item_id?, consent_investments?, investments_only? }` (`item_id` present = update/reconnect mode; `consent_investments` also asks for Investments access; `investments_only`, new links only, makes Investments the required product instead of Transactions — for brokerage/retirement/HSA providers Link won't offer otherwise, whose items then skip `/transactions/sync`) | Plaid `link_token` payload |
| `POST /plaid/exchange` | `{ public_token }` | exchanges + persists the access token (0600 file) → `{ item_id, institution, ... }` |
| `POST /plaid/link_event` | `{ stage: "event"\|"exit", error_type?, error_code?, error_message?, display_message?, institution?, institution_id?, link_session_id?, request_id? }` | `null` — the SPA forwards Plaid Link errors here so they reach the server log (`plaid link error` warning); fields are length-capped and stripped of control characters |
| `GET /plaid/items` | — | `PlaidItem[]` — linked connections + health (status, cursor, last_synced; no secrets) |
| `DELETE /plaid/items/{item_id}` | — | unlink (best-effort at Plaid; local history is kept on purpose) |

### Card-payment match review (`server/src/match_routes.rs`)

| Method · Path | Query / Path | Returns |
|---|---|---|
| `GET /matches` | `?status=` = `proposed`\|`accepted`\|`rejected` (omit for all) | `MatchView[]` |
| `POST /matches/{id}/accept` | `id` | accept a proposed pair (flags both legs `CardPayment`) |
| `POST /matches/{id}/reject` | `id` | reject a proposed pair (permanent unless re-accepted by hand) |

## Source of truth

Routes are registered in `server/src/routes.rs::api_router` (+ `plaid_routes.rs`
and `match_routes.rs`). Response types are the core domain types — see
[DATA_MODEL.md](DATA_MODEL.md) for `Account`, `Transaction`, `TxnPage`,
`LedgerView`, `Subscription`, `FlowMonth`, `SyncEntry`, and the enums. The
exact JSON shapes are in `app/src/types.ts` (generated) and
`server/contract/responses.json` (one example of each). If this table and the
code ever disagree, the code wins — update this doc.
