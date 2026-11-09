# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

outflow is a single-user view of where one person's money is and where it's
going: **Stock** (balances, holdings, performance), **Flow** (in / out / moved,
recurring purchases), and **Find money** (planned). Direction and screen plan:
`docs/adr/0001-product-direction.md`; vocabulary: `GLOSSARY.md`. Earlier
framing as a pure spending analyzer below is being reshaped toward that.
A server (target: an always-on mac-mini, exposed over Tailscale) pulls
transactions from **Plaid** (checking/savings/credit cards) into local SQLite,
categorizes them (rules + optional LLM), auto-detects
checking→card payments so spending isn't double-counted, and serves a web
client + JSON API for outflow tracking. Analysis only — no budgets.

Deeper reference lives in `docs/` (all tracked; read these for depth):
ARCHITECTURE, DATA_MODEL, INVARIANTS, GOTCHAS, DEVELOPMENT, DEPLOYMENT, RELEASING. An
untracked `agent.md` holds volatile session status/handoff — never `git add` it.

## Commands

```
cargo test -p outflow-core -p outflow-net   # pure-crate suites (make test)
cargo build                                  # whole workspace (no Tauri anymore)
```

CLI (`outflow` binary). Features: `net` (LLM categorizer), `client`
(HTTP mode against a running server), `encryption`. With no
features the full pipeline still runs via `--from-file`:

```
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/plaid-fixture.json
```

Subcommands: `pull --from-file P` (offline fixture; live syncs run on the
server), `categorize [--llm]`,
`report --by category|merchant|monthly [--top --posted-only]
[--period P | --since D --until D]`
(monthly = Flow lanes: in / out / earned − spent / saved / Moved groups),
`recurring [--period P --ignored]` (recurring merchants over the 12 months
ending with P's last month: cadence, usual monthly cost, new / late / up /
down >25%, charge to charge for every-few-months merchants, 12-month
year-on-year),
`fix <id> <category> [--no-learn]`,
`perks [--file F --lookback 3mo|6mo|12mo|all --expiring-days N]` (card offers
vs spending; file from `--file`/`OUTFLOW_PERKS`),
`balances [--history [--period P | --since D --until D]]`
(current state incl. holdings + idle cash, led by the signed breakdown
cash + invested − credit − loans = net worth, brokerage cash shown as
"incl. $X uninvested"; daily trend of the same breakdown from per-sync
snapshots),
`checkin [--period P]` (one week / month / quarter, default this month: net
worth change, earned − spent, saved, each with its usual, and what's
unusual: new or changed recurring merchants, large one-off charges,
connections and match reviews; checks that didn't run say why),
`change [--period P | --since D --until D]` (net worth change, default this
month: Markets = gain net of contributions on measurable investment accounts,
Added = the rest, unmeasured accounts named; per-account change + gain),
`holdings [--account A --ticker T]
[--history [--period P | --since D --until D]]` (flat per-position
rows: qty, price + as-of, close price, basis, unit cost, gain; an
investment account's uninvested cash is a synthetic `cash:uninvested`
row; with `--period`/`--since`/`--until` or `--by account|position`
(no `--history`): gain over the period per position, default this month,
unrealized gain + basis, sold positions kept, `--by position` combines a
ticker across accounts), `activity [--account --ticker --type] [--period P | --since D --until D]`
(investment transactions: buys, sells, dividends, contributions; amount = cash
effect), plus server/JSON mode (`--server URL`/`OUTFLOW_SERVER`, `--json`):
`txns [... --merchant KEY
--lane in|out|card-payment|invest|save|loan-paydown|between
--direction out|in --uncategorized] [--period P | --since D --until D]`
(`--lane` = exactly the rows behind a `report --by monthly` figure; on a
card payment `--direction out` = bank side, `in` = card side), `accounts`,
`matches`, `status` (what needs you: connections not ok, card payments
to review, last sync; then recent sync runs; `--json` =
`{upkeep, sync_log}` in both modes).
DB path from `--db` or `OUTFLOW_DB`.

One date group everywhere (`core::period`): `--period` takes `2026-11`,
`2026-W45`, `2026-Q4`, `2026`, or `this-`/`last-` `week|month|quarter|year`;
`--since`/`--until` are `YYYY-MM-DD`, both inclusive; not both forms at once.
Days are local time. Server mode sends the same spelling (`period=` or
`since=&until=`) to the API.

Server + web client:

```
cd app && npm install                       # first-time frontend deps (make deps)
cd app && npm run build                     # frontend → app/dist (make web)
cargo run -p outflow-server                 # serves app/dist + /api (make serve)
cd app && npm run dev                       # hot-reload UI, proxies /api → :8080 (make dev)
```

A tracked `Makefile` wraps these — `make help` lists all targets. The one
gotcha the raw commands hide: **`make run` rebuilds `app/dist` first, then
serves** (reach for it after a UI change); **`make serve` skips the rebuild**
and serves whatever's already in `app/dist` (backend-only iteration). Running
the bare server binary never rebuilds the frontend, so a stale `app/dist`
serves the old bundle. `make check` is the gate: fmt-check, clippy, tests (server included), the
types.ts/contract freshness check, `tsc`, and `make leakscan` (personal-data scan of all
history: `scripts/leakscan.py`, exceptions in `.leakscan-allow`, the private denylist via
`LEAKSCAN_DENYLIST_FILE`; CI runs the same scan in `.github/workflows/leakscan.yml`).

Server env: `OUTFLOW_DB`, `OUTFLOW_LISTEN` (default 127.0.0.1:8080),
`OUTFLOW_WEB_DIR` (default app/dist), `OUTFLOW_PLAID_CLIENT_ID`,
`OUTFLOW_PLAID_SECRET[_FILE]`, `OUTFLOW_PLAID_ENV=sandbox|production`,
`OUTFLOW_PLAID_TOKENS_FILE`, `OUTFLOW_OAUTH_REDIRECT` (the ts.net HTTPS URL +
`/oauth-return`, required for OAuth banks), `OUTFLOW_SYNC_INTERVAL_SECS`
(default 21600), `OUTFLOW_PERKS` (card-offers TOML file or dir), optional `OUTFLOW_API_TOKEN[_FILE]` (full access) and
`OUTFLOW_API_TOKEN_RO[_FILE]` (GET only, 403 on mutations — for agents/scripts),
and with `--features encryption`: `OUTFLOW_DB_KEY[_FILE]`. Setting **either**
token flips all of `/api/*` to deny-by-default; with neither, the API is open to
anything that can reach the port. Every `_FILE` is 0600-enforced and fatal if
unreadable (never fail-open) — use them under launchd, whose plists are
world-readable.

## Architecture

Ports-and-adapters over a pure core. One Cargo workspace, members
`core net cli server` (`app/` is an npm project, not a cargo member):

- **`core/`** — pure domain, zero GUI/network deps by default. `money` → `model`
  → `store` (SQLite via rusqlite) → `query` / `subscriptions` / `recurring` / `categorize` /
  `llm` / `plaid` (pure Plaid JSON→domain parser) / `transfers` (card-payment
  matcher). Source of truth; every front-end calls it directly so **CLI and web
  run identical domain logic**.
- **`net/`** — networked adapters (sync ureq): `plaid` (Link/exchange/sync
  transport), `plaid_tokens` (0600 token file), `secrets` (0600-file
  helpers), `anthropic` (`Prompter`).
- **`cli/`** — headless `outflow` binary; clap subcommands map to core calls
  (direct DB) or to the server API (`--server`, for agents/scripts).
- **`server/`** — axum + tokio. `Arc<Mutex<Store>>` bridged with
  `spawn_blocking` (rusqlite `Connection` isn't `Sync`); thin handlers over
  core (`routes.rs`), Plaid Link lifecycle (`plaid_routes.rs`), and the sync
  engine (`sync.rs`: per-item cursor loops, 6h background interval, sync_log).
  Serves the SPA from `app/dist` with an index.html fallback (OAuth resumption
  at `/oauth-return`). Frontend in `app/src/` (React + Vite + TS).

**Styling:** plain CSS in one file, `app/src/ledger.css` (tokens are the
`--lg-*` properties on `:root`; no Tailwind/styled-components/CSS-modules). Classes are namespaced by feature:
`ci-*` = Check-in (home), `fl-*` = Flow tab (`fl-rc-*` its recurring list), `hd-*` = Holdings tab, `lg-*` = shell and shared (incl. the `lg-strip` month strip), `cx-*` = Connections (Settings), `st-*` = Settings, `mr-*` =
match review. For styling/visual questions, trace
`className` → rule with `rtk grep` in `ledger.css` (graphify doesn't index CSS).

**Swappable ports:** the transaction source is a free-function pipeline —
fetch JSON in `net`, parse pure in `core`
(`plaid::parse_sync_page`/`parse_accounts_get`/`parse_fixture`) — plus
`categorize::Categorizer` / `llm::Prompter` (rules today, LLM tail).

Analysis (`query`, `recurring`) loads all transactions and
**aggregates in memory in Rust**, not SQL. The durable SQLite archive grows
past the providers' pull windows — it's the permanent history.

## Releases and branches (see docs/RELEASING.md)

The user relies on Claude to drive releases and repo mechanics. Every deploy
is a release, versioned `vYYYY.MM.N` (N from 0 each month); notes live in
`CHANGELOG.md`.

- Work on a feature branch; merge into `main` locally when the change is whole
  (no PRs). Release from `main` only.
- "Cut a release": `make release-draft`, tidy the section (a big release gets a
  short summary paragraph), `make release` (make check + leak scan + commit +
  tag). Then **show the user the notes and ask once**: that yes covers pushing,
  `git push --atomic origin main V`, which publishes (the tag runs
  `.github/workflows/release.yml`). Afterwards confirm the workflow went green.
- Before any deploy, cut the release first and deploy that tag.
- Never move or delete a pushed tag (a ruleset blocks it); fix forward with the
  next N. `main` blocks force-push; only for a history rewrite the user has
  approved does the ruleset get switched off, then back on.
- Pre-push leak scan: a block means fix the commits; a dollar-figure warning
  means show the user the figures and re-push with `LEAKSCAN_ACK=1` only after
  they say so. False shape-rule hits go in `.leakscan-allow`.

## Agent access

The web UI and an agent (RO token / CLI `--server`) are equal clients. If a
number answers a question you'd ask the agent, compute it in `core` and expose
it through the API and CLI; chart scaffolding (axes, scaling, sort order) may
stay in the browser. A habit, not an enforced rule.

## Load-bearing invariants (see docs/INVARIANTS.md)

Violating these breaks data integrity or the domain contract:

1. **Money is `i64` minor units (cents), never floats.** Outflows negative.
   Parse via `Money::from_decimal_str` — Plaid's JSON doubles go through
   `serde_json::Number::to_string()` into that same path, never f64 math.
   Serde-transparent newtype → crosses to JS as a bare **number** (cents).
2. **Dedup key is the provider transaction id** (+ a `source` column for
   provenance; ids stay raw). Live syncs commit through
   `store::apply_plaid_batch`: upserts, explicit deletions (`removed` +
   superseded `pending_transaction_id`s), and the cursor advance in ONE SQLite
   transaction. Never persist a Plaid cursor outside that path.
   `upsert_transactions` is the plain path for offline fixture ingest.
3. **`core` stays network/GUI-free by default.** Network and encryption
   are cargo features; the pipeline must stay runnable via `pull --from-file`
   with zero features. Network code lives in `net`.
4. **One merchant normalizer everywhere: `subscriptions::normalize_payee`.**
   Categorizer matching, merchant reports, subscription detection, and the
   card-payment matcher all key off it. Do not add a second normalizer.
5. **Boundary parsing.** External JSON deserializes then converts to domain
   types with explicit validation; malformed data returns `SourceError`, never
   leaks defaults (`plaid::parse_sync_page`, `plaid::parse_fixture`).
   Plaid sign conventions flip at this boundary: amounts negate (Plaid
   positive = money out), credit and loan balances negate (positive-owed →
   negative).
6. **Secrets never touch the DB or argv** — env or a 0600 file only (no
   keychain path; launchd has no unlocked GUI keychain). Plaid access tokens
   live in the 0600 tokens file keyed by item_id; only non-secret item metadata
   (institution, cursor, status) is in the DB. The SQLCipher DB key comes from
   `OUTFLOW_DB_KEY[_FILE]`; losing or changing it orphans the encrypted DB.
7. **User flag decisions survive re-sync.** The flag is Plaid's category
   (`pfc_primary`/`pfc_detailed` columns, mapped on read) unless
   `flag_overridden`; the upsert never touches an override. Read the PFC
   columns, never search `raw`.

## Traps that have already cost time (see docs/GOTCHAS.md)

- **The wire format is snake_case** (serde field names straight over HTTP).
  The old Tauri camelCase translation is gone; `app/src/api.ts` is the single
  client and sends Rust field names (`txn_id`, `public_token`).
- **`app/src/types.ts` is generated** from the Rust DTOs: change the struct,
  then `make types`. `make check` fails when it or the API contract snapshot
  is stale.
- **`None` filter must mean "everything"** (`TxnFilter::all()`). A derived
  `Default` would set `include_pending = false` and silently drop pending rows
  — `FilterParams::to_filter` defaults pending=true explicitly.
- **Headless server ≠ interactive session.** launchd services get no keychain
  and no shell env — configure via a plist env block + 0600 files
  (`OUTFLOW_PLAID_SECRET_FILE`, `OUTFLOW_DB_KEY_FILE`).
- **Plaintext and encrypted DBs are not interchangeable** — switching a path's
  mode requires deleting the file first.
- **Don't stage the DB or secrets in `/tmp`** — macOS purges it and the app
  silently recreates an empty DB. Use `$HOME/...` or the app-data dir.
- **OAuth banks need the exact registered redirect URI** — `tailscale serve`
  HTTPS + `/oauth-return`, matching the Plaid dashboard byte-for-byte, and the
  SPA fallback must serve it with a 200.

## Toolchain note

`cli/Cargo.toml` pins `clap = "=4.4.18"` only because it was first built on Rust
1.75; on current stable this can loosen to `clap = "4"`. Nothing else depends on it.
