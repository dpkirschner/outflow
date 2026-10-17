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
  defined in [DATA_MODEL.md](DATA_MODEL.md); `app/src/types.ts` mirrors them.
- **Timestamps** in query params and bodies are **epoch seconds** (`i64`).
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
| `GET /accounts` | — | `Account[]` — latest snapshot per account incl. `balance` (cents; credit balances negated) |
| `GET /transactions` | see **Transaction query** below | `TxnPage { items: Transaction[], total, total_cents }` — paginated |
| `GET /spend/categories` | `since`, `until`, `pending`, `transfers` | `CategorySpend[]` — outflow sum per category (uncategorized under `null`) |
| `GET /merchants` | `since`, `until`, `pending`, `transfers`, `limit` (default 15) | `MerchantSpend[]` — outflow sum per normalized merchant |
| `GET /flow` | `since`, `until`, `pending`, `transfers` | `MonthlyFlow[]` — inflow/outflow/net per month (local-time buckets; both legs drop non-Spending) |
| `GET /subscriptions` | — | `Subscription[]` — fixed-amount recurring (≥3 occurrences) |
| `GET /ledger` | `window` = `3mo`\|`6mo`(default)\|`12mo`\|`all` | `LedgerView` — the rhythm ledger (streams / committed / notable / transfers / noise) |
| `GET /stream_occurrences` | `merchant` (required, normalized name), `window` | `Transaction[]` — the raw rows behind one stream |
| `GET /stream_preview` | `merchant` (required), `window` | `Stream \| null` — what a noise merchant would become if promoted (`null` if no spending in window) |
| `GET /categories` | — | `string[]` — the category vocabulary |
| `GET /has_credit_account` | — | `bool` — whether a credit account is ingested (card-payment suppression is only correct when true) |
| `GET /sync_log` | `limit` (default 50) | `SyncEntry[]` — most recent sync runs |
| `GET /perks` | `lookback` = `3mo`\|`6mo`\|`12mo`(default)\|`all`, `expiring_days` (default 14) | `PerksReport { cards: CardStatus[], findings: Finding[] }` — card offers from the `OUTFLOW_PERKS` file(s) matched against spending; `kind` = `add_offer`\|`wrong_card`\|`expiring`, sorted by `est_cash_cents` desc. `400` if `OUTFLOW_PERKS` is unset; the file is re-read per request |

### Transaction query (`GET /transactions`)

All optional; a missing filter set means **everything** (pending included).

| Param | Type | Meaning |
|---|---|---|
| `since` / `until` | epoch secs | window on `effective_date()`; `since` inclusive, `until` exclusive |
| `pending` | bool (default `true`) | include pending rows |
| `transfers` | bool (default `false`) | include Transfer/CardPayment rows |
| `q` | string | case-insensitive text over payee/description/category/normalized merchant |
| `account` | string | filter by `account_id` |
| `category` | string | filter by category name |
| `source` | string | provenance (`plaid`) |
| `flag` | enum | `Spending`\|`Transfer`\|`CardPayment` — filtering by a suppressed flag auto-includes those rows |
| `min_cents` / `max_cents` | i64 | amount band (cents) |
| `sort` | string | `date`(default)\|`amount`\|`merchant`\|`category`; a bad key is a `400` |
| `dir` | string | `asc`\|`desc`(default) |
| `offset` / `limit` | usize | pagination; `limit` default 100, **capped at 1000** |

`TxnPage.total` / `total_cents` cover **all** matches, not just the returned
page — page through with `offset` until you've read `total` items.

```bash
# Most recent 50 Starbucks transactions since June, as an agent:
curl -s -H "Authorization: Bearer $RO_TOKEN" \
  "http://127.0.0.1:8080/api/transactions?q=starbucks&since=1717200000&limit=50&sort=date&dir=desc" | jq
```

## Mutation endpoints (POST/DELETE — full token required)

An RO token gets **403** on all of these.

| Method · Path | Body | Effect / returns |
|---|---|---|
| `POST /categorize` | — | run the rule categorizer over uncategorized rows → `{ rule, remaining }` |
| `POST /categorize_llm` | — | LLM pass over merchants rules couldn't place (needs `ANTHROPIC_API_KEY` on the server) → count (`number`) |
| `POST /txn/{txn_id}/category` | `{ category, learn? }` | set a manual category; `learn:true` writes an exact rule so siblings follow |
| `POST /txn/{txn_id}/flag` | `{ flag, learn? }` | set suppression flag (`Spending`\|`Transfer`\|`CardPayment`) |
| `POST /account/{account_id}/nickname` | `{ nickname }` | set/clear account nickname (empty clears) |
| `POST /apply_flags` | — | re-apply flag rules → count (`number`) |
| `POST /mark_stream` | `{ merchant, mark }` | ledger mark (`Committed`\|`Dismissed`\|`Kept`) |
| `POST /clear_stream_mark` | `{ merchant }` | clear a ledger mark |
| `POST /pull` | — | trigger a live Plaid sync now → `SyncReport` (a pull with no linked items returns a clean empty report without touching credentials) |
| `POST /pull_from_file` | `{ path }` | ingest a local Plaid fixture (offline/dev) → `PullResult { added, updated, accounts, warnings }` |
| `POST /reset_data` | — | clear transactions/accounts/sync_log/matches and reset Plaid cursors — **destructive**, back up the DB first (cannot recover rows older than Plaid's window) |

### Plaid Link lifecycle (`server/src/plaid_routes.rs`)

| Method · Path | Body | Returns |
|---|---|---|
| `POST /plaid/link_token` | `{ item_id? }` (present = update/reconnect mode) | Plaid `link_token` payload |
| `POST /plaid/exchange` | `{ public_token }` | exchanges + persists the access token (0600 file) → `{ item_id, institution, ... }` |
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
`LedgerView`, `Subscription`, `MonthlyFlow`, `SyncEntry`, and the enums. If this
table and the code ever disagree, the code wins — update this doc.
