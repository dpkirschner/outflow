# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

outflow is a single-user spending analyzer for one person's own bank data.
A server (target: an always-on mac-mini, exposed over Tailscale) pulls
transactions from **Plaid** (checking/savings/credit cards) and **SimpleFIN**
into local SQLite, categorizes them (rules + optional LLM), auto-detects
checking→card payments so spending isn't double-counted, and serves a web
client + JSON API for outflow tracking. Analysis only — no budgets.

Deeper reference lives in `docs/` (all tracked; read these for depth):
ARCHITECTURE, DATA_MODEL, INVARIANTS, GOTCHAS, DEVELOPMENT. An untracked
`agent.md` holds volatile session status and demo credentials — never `git add` it.

## Commands

```
cargo test -p outflow-core -p outflow-net   # pure-crate suites (make test)
cargo build                                  # whole workspace (no Tauri anymore)
```

CLI (`outflow` binary). Features: `net` (live SimpleFIN + LLM), `client`
(HTTP mode against a running server), `keychain`, `encryption`. With no
features the full pipeline still runs via `--from-file`:

```
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/sample-accounts.json
```

Subcommands: `claim <token>`, `pull [--from-file P]`, `categorize [--llm]`,
`report --by category|merchant|monthly [--top --since --until --posted-only]`,
`subs`, `fix <id> <category> [--no-learn]`, plus server/JSON mode (`--server
URL`/`OUTFLOW_SERVER`, `--json`): `txns`, `accounts`, `matches`, `status`.
DB path from `--db` or `OUTFLOW_DB`.

Server + web client:

```
cd app && npm install && npm run build      # frontend → app/dist (make web)
cargo run -p outflow-server                 # serves app/dist + /api (make run)
cd app && npm run dev                       # hot-reload UI, proxies /api → :8080 (make dev)
```

Server env: `OUTFLOW_DB`, `OUTFLOW_LISTEN` (default 127.0.0.1:8080),
`OUTFLOW_WEB_DIR` (default app/dist), `OUTFLOW_PLAID_CLIENT_ID`,
`OUTFLOW_PLAID_SECRET[_FILE]`, `OUTFLOW_PLAID_ENV=sandbox|production`,
`OUTFLOW_PLAID_TOKENS_FILE`, `OUTFLOW_OAUTH_REDIRECT` (the ts.net HTTPS URL +
`/oauth-return`, required for OAuth banks), `OUTFLOW_SYNC_INTERVAL_SECS`
(default 21600), optional `OUTFLOW_API_TOKEN`, and with `--features
encryption`: `OUTFLOW_DB_KEY[_FILE]`.

## Architecture

Ports-and-adapters over a pure core. One Cargo workspace, members
`core net cli server` (`app/` is an npm project, not a cargo member):

- **`core/`** — pure domain, zero GUI/network deps by default. `money` → `model`
  → `store` (SQLite via rusqlite) → `query` / `subscriptions` / `categorize` /
  `llm` / `plaid` (pure Plaid JSON→domain parser) / `transfers` (card-payment
  matcher). Source of truth; every front-end calls it directly so **CLI and web
  run identical domain logic**.
- **`net/`** — networked adapters (sync ureq): `simplefin` (fetch/claim),
  `plaid` (Link/exchange/sync transport), `plaid_tokens` (0600 token file),
  `secrets` (keychain + access-URL/DB-key resolution), `anthropic` (`Prompter`).
- **`cli/`** — headless `outflow` binary; clap subcommands map to core calls
  (direct DB) or to the server API (`--server`, for agents/scripts).
- **`server/`** — axum + tokio. `Arc<Mutex<Store>>` bridged with
  `spawn_blocking` (rusqlite `Connection` isn't `Sync`); thin handlers over
  core (`routes.rs`), Plaid Link lifecycle (`plaid_routes.rs`), and the sync
  engine (`sync.rs`: per-item cursor loops, 6h background interval, sync_log).
  Serves the SPA from `app/dist` with an index.html fallback (OAuth resumption
  at `/oauth-return`). Frontend in `app/src/` (React + Vite + TS).

**Swappable ports:** transaction sources are free-function pipelines behind one
shape — fetch JSON in `net`, parse pure in `core` (`source::parse_account_set`
for SimpleFIN, `plaid::parse_sync_page`/`parse_accounts_get` for Plaid) — plus
`categorize::Categorizer` / `llm::Prompter` (rules today, LLM tail).

Analysis (`query`, `subscriptions`, `ledger`) loads all transactions and
**aggregates in memory in Rust**, not SQL. The durable SQLite archive grows
past the providers' pull windows — it's the permanent history.

## Load-bearing invariants (see docs/INVARIANTS.md)

Violating these breaks data integrity or the domain contract:

1. **Money is `i64` minor units (cents), never floats.** Outflows negative.
   Parse via `Money::from_decimal_str` — Plaid's JSON doubles go through
   `serde_json::Number::to_string()` into that same path, never f64 math.
   Serde-transparent newtype → crosses to JS as a bare **number** (cents).
2. **Dedup key is the provider transaction id** (+ a `source` column for
   provenance; ids stay raw). SimpleFIN: posted upsert by id, **pending
   delete-and-replace per synced account each pull**. Plaid: **no pending
   sweep** — `store::apply_plaid_batch` applies upserts, explicit deletions
   (`removed` + superseded `pending_transaction_id`s), and the cursor advance
   in ONE SQLite transaction. Never persist a Plaid cursor outside that path.
3. **`core` stays network/GUI-free by default.** Network, keychain, encryption
   are cargo features; the pipeline must stay runnable via `pull --from-file`
   with zero features. Network code lives in `net`.
4. **One merchant normalizer everywhere: `subscriptions::normalize_payee`.**
   Categorizer matching, merchant reports, subscription detection, and the
   card-payment matcher all key off it. Do not add a second normalizer.
5. **Boundary parsing.** External JSON deserializes then converts to domain
   types with explicit validation; malformed data returns `SourceError`, never
   leaks defaults (`source::parse_account_set`, `plaid::parse_sync_page`).
   Plaid sign conventions flip at this boundary: amounts negate (Plaid
   positive = money out), credit balances negate (positive-owed → negative).
6. **Secrets never touch the DB or argv** — env, a 0600 file, or the keychain
   only. Plaid access tokens live in the 0600 tokens file keyed by item_id;
   only non-secret item metadata (institution, cursor, status) is in the DB.
   The DB key is regenerated **only** on keychain `NoEntry`, never on a
   locked/denied read (a new key permanently orphans an existing encrypted DB).
7. **User flag decisions survive re-sync.** `flag` is absent from the upsert
   conflict-update; Plaid PFC hints seed flags at parse time only.

## Traps that have already cost time (see docs/GOTCHAS.md)

- **The wire format is snake_case** (serde field names straight over HTTP).
  The old Tauri camelCase translation is gone; `app/src/api.ts` is the single
  client and sends Rust field names (`txn_id`, `setup_token`).
- **`None` filter must mean "everything"** (`TxnFilter::all()`). A derived
  `Default` would set `include_pending = false` and silently drop pending rows
  — `FilterParams::to_filter` defaults pending=true explicitly.
- **Headless server ≠ interactive session.** launchd services get no keychain
  and no shell env — configure via a plist env block + 0600 files
  (`OUTFLOW_PLAID_SECRET_FILE`, `OUTFLOW_SFIN_URL_FILE`, `OUTFLOW_DB_KEY_FILE`).
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
