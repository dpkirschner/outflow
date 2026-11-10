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
| `GET /accounts` | `include_hidden` (default `false`) | `Account[]` — latest snapshot per account incl. `balance` (cents; credit balances negated), `hidden`, `kind_from_bank` (the bank's type when the owner changed `kind`, else `null`) and `item_id` (the connection it last synced through, `null` before its first v14 sync or from a fixture). Hidden accounts are omitted unless `include_hidden=true` (the Connections screen) |
| `GET /transactions` | see **Transaction query** below | `TxnPage { items: TxnRow[], total, total_cents }` — paginated. `TxnRow` = every `Transaction` field (flattened) plus `display_name` (`display_merchant`: processor prefix, trailing store number and `.COM` cut, all-caps title-cased; display only) |
| `GET /spend/categories` | `period` / `since` / `until`, `pending`, `transfers` | `CategorySpend[]` — outflow sum per category (uncategorized under `null`) |
| `GET /merchants` | `period` / `since` / `until`, `pending`, `transfers`, `limit` (default 15) | `MerchantSpend[]` — outflow sum per normalized merchant |
| `GET /flow` | `period` / `since` / `until`, `pending` | `FlowMonth[]` — Flow lanes per month (local-time buckets): `in_cents`, `out_cents` (refunds netted), `refunds_cents`, `moved` (`card_payment_cents` + `card_payment_bank_cents` / `card_payment_card_cents` + `invest`/`save`/`loan_paydown`/`between` as `{out_cents, in_cents}` from checking + cards), `in_uncategorized_cents`, `earned_minus_spent_cents`, `saved_cents`, `gap_cents` (earned − spent − saved: cash left in checking) |
| `GET /flow/in-sources` | `period` / `since` / `until`, `pending` | `InSource[]` — the In lane by payee (`normalize_payee`), largest first: `{merchant, name, total_cents, count, uncategorized_cents}`; `merchant` is `merchant=` on `/transactions` (empty = no payee). Totals sum to `/flow` `in_cents` |
| `GET /flow/series` | the `/transactions` filters (`period` / `since` / `until`, `pending`, `lane`, `category`, `merchant`, `uncategorized`, `direction`, `account`, `q`, …; sort and paging ignored) | `SeriesMonth[]` — `{year, month, cents}`: the rows `/transactions` would list, signed sum per local month, oldest first; months with no rows left out. Flow's month strip plots a category, merchant or uncategorized pick with it |
| `GET /recurring` | `period` / `since` / `until` (optional; the list ends with that range's last month, default this month), `subscriptions` (optional bool: the Subscriptions filter) | `RecurringView { month, since, until, in_progress, monthly_total_cents, items, ignored }` — recurring merchants over the 12 calendar months ending with that month (`since`/`until` = the window, `YYYY-MM-DD` inclusive). Each `RecurringMerchant` = `{merchant (normalized key), name (display name), cadence (…Monthly\|EveryFewMonths\|Yearly), gap_days, count, median_cents, monthly_cents (usual: mean of completed months since the first charge, gaps as $0; positive = spent), change: New\|Up\|Down\|Late\|Steady, change_pct, change_base_cents (what change_pct compares with), late_since, year_cents, prior_year_cents (null until 2 years of archive), recent_charges: [{at, cents}] (slower merchants: last 6 charges), first_seen, last_seen, spark_cents (6 months ending at that month), category, account_id, steady_cents, price_rise}`. `steady_cents` (Steady: monthly or yearly, last 3 charges within 5% of their median) is that median, else null; the Subscriptions filter is `steady_cents != null`: `subscriptions=true` keeps only those in `items`, and `monthly_total_cents` is then their sum; `ignored` is never filtered. `price_rise: {from_cents, to_cents, since} | null`: a monthly or yearly merchant whose latest charges (fewer than 3, within 5% of the latest) sit more than 5% above a steady run before them; `since` is the first charge at the new price. New = first charge in the last 3 months with older history; Late = no charge for 1.5× the usual gap (at least 31 days); Up/Down = last completed month more than 25% from the mean of up to 3 before it, or for EveryFewMonths/Yearly the latest charge against the median of up to 4 before it. `ignored` holds Ignored merchants still detected. The rows behind a merchant: `/transactions?merchant=<key>&lane=out&direction=out&since=…&until=…` |
| `GET /categories` | — | `string[]` — the category vocabulary |
| `GET /has_credit_account` | — | `bool` — whether a credit account is ingested (card-payment suppression is only correct when true) |
| `GET /sync_log` | `limit` (default 50) | `SyncEntry[]` — most recent sync runs |
| `GET /reset/preview` | — | `ResetPreview { transactions, oldest_day, refetch_days, refetch_from, lost_for_good, manual_categories, decided_matches, accounts, nicknamed_accounts, hidden_accounts }` — what `POST /reset_data` would delete (counts include hidden accounts). `refetch_from` (`YYYY-MM-DD`, local) = today − `refetch_days` (90: Plaid's default `days_requested`, which our link token leaves unset); `lost_for_good` = transactions posted before it. Conservative: an item may re-serve more. `oldest_day` is `null` with no transactions. Nicknames are cleared, hidden accounts come back visible, one-off manual categories and decided match pairs are gone |
| `GET /upkeep` | — | `Upkeep { connections: [{item_id, institution, status}], proposed_matches, last_synced, total }` — what needs the owner now: connections whose status isn't `ok`, card-payment matches waiting for review (pairs with a leg on a hidden account aren't counted, as `/matches` doesn't show them), the newest successful sync across connections (epoch secs, `null` if none), `total` = connections + matches. The check-in's `connection`/`match_review` items and `outflow status` read the same summary (`core::upkeep`) |
| `GET /version` | — | `BuildInfo { version }` — the release tag (`vYYYY.MM.N`) this server was built from, `null` for a build that isn't a release (`scripts/deploy.sh build` sets it; see RELEASING.md). `outflow status` reports it as `version` |
| `GET /balances` | — | `BalancesView { totals: {net_worth, cash, invested, uninvested, credit, loans, other}, groups: [{group: cash\|investments\|credit\|loans\|other, total, accounts: AccountBalance[]}] }`. `totals` is the signed breakdown: `cash + invested + credit + loans + other == net_worth` exactly, each equal to its group's total; `cash` = checking + savings, `invested` includes brokerage cash, `uninvested` = that cash (a part of `invested`); credit and loans negative when owed. `AccountBalance` = `{ account, group, cash, invested, cash_bp, idle_cash, holdings: HoldingRow[] }` (largest position first; `quantity`/`price` are decimal strings, `*_bp` basis points). Visible accounts only. `idle_cash` = investment account with ≥ $1,000 and ≥ 5% cash |
| `GET /balances/history` | `period` / `since` / `until` (optional) | `DayTotals[]` — `{day: "YYYY-MM-DD", net_worth, cash, invested, uninvested, credit, loans, other}` (the same breakdown) per local day from the per-sync snapshots, each account's last value carried forward |
| `GET /checkin` | `period` (a week, month or quarter spelling; default this month) | `CheckIn { period, label, in_progress, net_worth: {start_cents, end_cents, start_at, end_at, change_cents, change_bp, reason, markets_cents, added_cents, excluded, partial_start}, flow: {in_cents, out_cents, earned_minus_spent_cents, saved_cents, gap_cents}, usual: {periods, in_cents, out_cents, earned_minus_spent_cents, saved_cents, net_worth_periods, net_worth_change_cents}, unusual: Unusual[], not_checked: [{check: recurring_changes\|new_recurring\|large_charges\|card_offers, reason: week\|in_progress\|short_history\|range\|offers_file}] }`. `Unusual` = `{kind: connection\|match_review\|new_recurring\|recurring_up\|recurring_down\|large_charge\|late\|finding, label, cents, usual_cents, change_pct, merchant, txn_id, account_id, day, month, item_id, status, count, gap_days, per_charge, impact_cents, finding_kind, finding_key, action, minor}` (`per_charge`: up/down judged charge to charge, `cents` = the charge); set fields depend on kind; `usual_cents` is the recurring change's base (mean of up to 3 months before) or the large-charge threshold. `finding` is an open Finding from `GET /find` that is new this period (a wrong-card charge or a price rise in it, an offer expiring soon, a card choice whose first charge is in it); `finding_kind`/`finding_key`/`action` are the Finding's `kind`/`key`/`action`, `cents` its `est_value_cents_per_year`. A `recurring_up` row whose merchant has an open `price_rise` Finding stands in for it and carries its `finding_key`. Offers come from the server's `OUTFLOW_PERKS`; a bad file adds `{check: card_offers, reason: offers_file}` instead of failing. Usual = median of the 12 periods before (needs 3). Rules in DATA_MODEL "Check-in" |
| `GET /checkin/categories` | `period` (as `/checkin`) | `CategoriesVsUsual { period, label, periods, to_date, items: [{category, total_cents, count, usual_cents, change: up\|down\|null}] }`: Out per category (largest first, totals as `/spend/categories`), each against its usual (median of the same earlier periods as `/checkin`, a period with none counting as $0). `change` marks a category at 2x or half its usual and $50 or more away; never on uncategorized spend or while the period is in progress. Feeds the Flow Out lane's margin notes |
| `GET /net-worth/change` | `period` / `since` / `until` (optional; none = all history) | `NetWorthChange { start, end (Totals breakdowns), start_at, end_at, partial_start, change, change_bp, reason: no_snapshots\|one_snapshot, markets, added, excluded: [{account_id, account, change}], added_includes_unmeasured, accounts: AccountChange[], contributed: [{day, invested, contributed}], contributed_complete }`. `markets + added == change` exactly; Markets = gain net of contributions summed over measurable investment accounts. `AccountChange` = `{account_id, account, group, start, end, start_at, end_at, partial_start, change, change_reason, contributions, gain, gain_reason: cash\|contributions_unknown}`. Rules in DATA_MODEL "Net worth change". Visible accounts only |
| `GET /holdings` | `account` (id or label substring), `ticker` (ticker or security_id), both optional, case-insensitive | `PositionRow[]` — every current position on visible accounts, largest first: `{account_id, account, security_id, ticker, label, name, sec_type, is_cash, cusip, isin, currency, quantity, price, price_as_of, close_price, close_price_as_of, value, cost_basis, unit_cost, gain, gain_bp, share_bp, synced_at}`. `quantity`/`price`/`close_price`/`unit_cost` are decimal strings (`unit_cost` = basis ÷ qty, 4 places); `price` is the institution's, `close_price` Plaid's end-of-day (neither is a live quote); dates `YYYY-MM-DD`; `synced_at` = epoch secs of the sync that last refreshed the position (stale if a holdings fetch has been failing). Detail fields are `null` until the first holdings sync on v8. Each investment account whose balance its positions don't cover also gets one synthetic row: `security_id` `cash:uninvested`, `is_cash: true`, `quantity` = dollars at `price` `1` |
| `GET /activity` | `account` (id or label substring), `ticker` (ticker or security_id), `type` (Plaid type or subtype: `buy`, `sell`, `dividend`, `contribution`, `fee`, …), `period` / `since` / `until` — all optional | `ActivityRow[]` — archived investment transactions on visible accounts, newest first: `{id, date, account_id, account, security_id, ticker, label, name, type, subtype, quantity, price, amount, fees, currency, cancel_txn_id}`. `amount` is the cash effect on the account (negative = cash out, e.g. a buy; positive = cash in, e.g. a dividend); `quantity`/`price` decimal strings |
| `GET /holdings/history` | `period` / `since` / `until`, `account`, `ticker` — all optional | `PositionPoint[]` — `{taken_at, day, account_id, account, security_id, label, quantity, price, value, cost_basis, gain}` per position per holdings sync, oldest first (`price` null on pre-v8 snapshots) |
| `GET /holdings/change` | `period` / `since` / `until` (none = all history), `account`, `ticker`, `by` = `account` (default) \| `position` | `PositionChange[]` — gain over the period per position, largest end value first: `{account_ids, account, security_id, ticker, label, name, is_cash, start_quantity, end_quantity, price, priced_at, price_as_of, start_value, end_value, contributions, gain, gain_bp, gain_reason: cash\|contributions_unknown, change_reason: one_snapshot, partial_start, sold, cost_basis, unrealized_gain, unrealized_bp}`. Values from holdings snapshots (`priced_at`); `price_as_of` = the institution's own price date when the end snapshot is the current sync. A position bought mid-period starts at $0; a sold one stays (`sold`). Each investment account with uninvested cash at either end gets a synthetic `cash:uninvested` row (`is_cash`, `gain_reason: cash`), so its rows sum to the account's balance. `by=position` combines a ticker (else security id) across accounts; sums are null when any part lacks them. Position gains do not sum to `/net-worth/change` account gains |
| `GET /find` | `period` / `since` / `until` (picks the last month of the window; default this month), `expiring_days` (default 14) | `FindReport { window: Period, cards: CardStatus[], notes: string[], findings: Finding[], dismissed: Finding[], reviewed: Finding[] }` — every Finding, estimated over the 12 calendar months ending with the period's last month. `Finding = {kind: add_offer\|wrong_card\|expiring\|card_choice\|subscription\|price_rise, key, subject, card_mask, est_cents_per_year, est_points_per_year, est_value_cents_per_year, txn_ids, action, evidence}`; `key` (`kind:card:subject`) is stable; `evidence` is tagged by `source` (`offer`: issuer, offer, days_left, charges, last_charge, typical_charge_cents, matched_merchants; `earn`: `category` (the Plaid category the rates were read for, or `default`), `cards: CardEarn[]` best first, each `{issuer, mask, spend_cents, charges, rate, cents_per_point, return_bp, cap_left_cents, earned_cents}`, where `rate` and `cents_per_point` are decimal strings and `cents_per_point: null` is cash back; `subscription`: merchant (normalized), name, cadence, charge_cents (the steady charge, or the new price), per_year (12 or 1), price_rise, last_charge, account_id, reviewed_at (set only in `reviewed`)). `est_value_cents_per_year` is cash plus points at the card's earn-table `cents_per_point` (1¢ without one); Findings under $25/yr of it are dropped, and they rank by it, then cash, then points, then soonest expiry. `card_choice` (key `card_choice:<best card>:<category>`) is the window's Out in one category, split across the cards with earn tables in the best order their caps allow, minus what it earned where it was charged; its whole estimate is cash (points already valued), `est_points_per_year` is 0, and `card_mask` is the card to use. Card offers and earn tables come from the `OUTFLOW_PERKS` file(s), re-read per request; unset → no offer or card choice Findings and a line in `notes`; cards left out of card choice (no `[earn]` table, no tied account) are named in `notes` and `CardStatus.earn`; a bad file is a `500` naming it. `subscription` (key `subscription::<merchant>`) is each steady recurring merchant (see `GET /recurring`) to review, estimated at a year of its charge; `price_rise` (key `price_rise::<merchant>`) is one whose price just rose, estimated at a year of the rise, and replaces its `subscription` Finding while it lasts. Both are cash, come without offers, skip Ignored and Late merchants, and never claim a subscription is unused. A subscription or price rise whose merchant is marked Reviewed is in `reviewed` for 365 days, or until its charge is more than 5% above the one reviewed. A Finding marked Dismissed is in `dismissed`, not `findings`, until its estimate (cash + points) more than doubles the one it had when dismissed |
| `POST /find/dismissed` | the `GET /find` params; body `{key, dismissed: bool}` | `FindReport` after the change. `dismissed: true` marks the Finding with that `key` (open or dismissed in the report for these params, else `404`) and stores its estimate now; dismissing it again keeps the first estimate. `false` clears the mark (a no-op for an unmarked key) |
| `POST /find/reviewed` | the `GET /find` params; body `{merchant, reviewed: bool}` | `FindReport` after the change. `merchant` is normalized (any spelling works). `reviewed: true` marks it Reviewed (it needs a `subscription` or `price_rise` Finding in the report for these params, open, reviewed or dismissed, else `404`) and stores that Finding's `charge_cents` and the time; marking again refreshes both. `false` clears the mark (a no-op for an unmarked merchant). `400` on an empty merchant |

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
| `source` | string | provenance (`plaid`; `simplefin` on rows from the retired integration) |
| `flag` | enum | `Spending`\|`Transfer`\|`CardPayment`\|`Invest`\|`Save`\|`LoanPaydown` — filtering by a Moved flag auto-includes those rows |
| `merchant` | string | normalized merchant key (`normalize_payee`), e.g. a `RecurringMerchant.merchant` |
| `ids` | string | comma-separated transaction ids, e.g. a Finding's `txn_ids` (the Find money screen's drill-in); only those rows |
| `lane` | string | `in`\|`out`\|`card_payment`\|`invest`\|`save`\|`loan_paydown`\|`between` — exactly the rows behind that `/flow` figure (signed sum = the figure); reads Moved rows without `transfers`; a bad value is a `400` |
| `uncategorized` | bool | only rows with no category (e.g. `lane=in&uncategorized=true`, or Out's `null` category) |
| `direction` | string | `out` (negative) \| `in` (positive); on `lane=card_payment`, `out` = bank side, `in` = card side; a bad value is a `400` |
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
| `POST /txn/{txn_id}/category` | `{ category, learn? }` | set a manual category; `learn:true` points the merchant's exact rule at it (re-points the existing exact rule on the normalized merchant, else adds one) so uncategorized siblings follow on the next categorize pass; nothing else is recategorized now → `CategoryEdit { rule_id, undo: { category, prev_category, prev_source, rule: { id, prev_category } \| null } }` (`rule.prev_category` null = the edit created the rule) |
| `POST /txn/{txn_id}/category/undo` | the `undo` object from that response, as is | reverse exactly that edit → `null`: the row's previous category and source come back, a rule the edit created is deleted, a rule it re-pointed is pointed back. `409` (nothing changed) when the row's category isn't the edit's manual one any more or the rule no longer says it (e.g. a later edit re-pointed it: undo newest first); `404` unknown transaction. Rows a categorize pass filled from the learned rule in between keep their category |
| `POST /txn/{txn_id}/flag` | `{ flag, learn? }` | set the flag (`Spending`, or a Moved group: `Transfer`\|`CardPayment`\|`Invest`\|`Save`\|`LoanPaydown`) |
| `POST /account/{account_id}/nickname` | `{ nickname }` | set/clear account nickname (empty clears) |
| `POST /txn/{txn_id}/group` | `{ flag }` (`Invest`\|`Save`\|`LoanPaydown`\|`Transfer`) | put a moved row in a Moved group and teach its merchant → `GroupEdit { changed, rule_id, undo }`: the merchant's exact flag rule is re-pointed or added and its other rows get now what the next `apply_flags` would give them (`Transfer` never pulls a row out of Invest/Save/LoanPaydown). `400` for another flag, `404` unknown transaction |
| `POST /txn/{txn_id}/group/undo` | the `undo` object from that response, as is | reverse that edit → rows restored: each row it changed gets its previous flag back if it still has the edit's; a rule it created is deleted, one it re-pointed is pointed back, if it still says the edit's flag |
| `POST /account/{account_id}/kind` | `{ kind }` (`Checking`\|`Savings`\|`Investment`\|`Other`\|`Credit`\|`Loan`, or `null` = the bank's) | set the account's type over the bank's; survives re-sync. A savings, investment or loan account is the far side of a move, so Flow doesn't count it again. `400` when it would turn an asset into a debt or back, `404` unknown account |
| `POST /account/{account_id}/hidden` | `{ hidden }` | hide/un-hide an account: its rows leave every list, aggregate, dropdown and match review (data keeps syncing). `404` for an unknown id |
| `POST /apply_flags` | — | re-apply flag rules → count (`number`) |
| `POST /recurring/ignored` | `{ merchant, ignored }` | set (`true`) or clear the Ignored mark on a normalized merchant; ignored merchants move to `RecurringView.ignored`. `400` on an empty merchant |
| `POST /pull` | — | trigger a live Plaid sync now → `SyncReport` (a pull with no linked items returns a clean empty report without touching credentials) |
| `POST /reset_data` | — | clear transactions/accounts/sync_log/matches and reset Plaid cursors — **destructive**, back up the DB first (cannot recover rows older than Plaid's window; `GET /reset/preview` counts them) |

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
| `POST /matches/{id}/accept` | `id` | accept a proposed pair (flags both legs `CardPayment`, retires other proposals sharing a leg, records what it changed). `404` unknown id |
| `POST /matches/{id}/reject` | `id` | reject a pair (permanent unless re-accepted by hand). On an accepted pair it first undoes the accept as `unaccept` does, but the pair is never proposed again. `404` unknown id |
| `POST /matches/{id}/unaccept` | `id` | undo an accept → `null`: the pair goes back to `proposed` (review) and each leg gets back the flag it had before the accept unless another accepted pair still uses that leg, and the proposals the accept retired return to review. Pairs accepted before schema v14 have no record: legs go back to the flag Plaid's category gives and retired proposals stay rejected. `404` unknown id, `409` when the pair isn't accepted |

## Source of truth

Routes are registered in `server/src/routes.rs::api_router` (+ `plaid_routes.rs`
and `match_routes.rs`). Response types are the core domain types — see
[DATA_MODEL.md](DATA_MODEL.md) for `Account`, `Transaction`, `TxnPage`,
`RecurringView`, `FlowMonth`, `SyncEntry`, and the enums. The
exact JSON shapes are in `app/src/types.ts` (generated) and
`server/contract/responses.json` (one example of each). If this table and the
code ever disagree, the code wins — update this doc.
