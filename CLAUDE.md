# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

outflow is a single-user view of where one person's money is and where it's
going: **Stock** (balances, holdings, performance), **Flow** (in / out / moved,
recurring purchases), and **Find money** (card offers, card choice, subscriptions to review). Product and its limits:
`PRODUCT.md`; vocabulary: `GLOSSARY.md`.
A server (an always-on mac-mini over Tailscale) pulls accounts from **Plaid**
into local SQLite, categorizes transactions (rules + optional LLM), matches
checking→card payments so nothing double-counts, and serves a web client +
JSON API. Analysis only — no budgets.

Deeper reference lives in `docs/` (all tracked; read these for depth):
ARCHITECTURE, DATA_MODEL, INVARIANTS, GOTCHAS, DEVELOPMENT, DEPLOYMENT, RELEASING. An
untracked `agent.md` holds volatile session status/handoff — never `git add` it.

## Commands

```
make check        # the gate: fmt, clippy, tests, types/contract freshness, tsc, leakscan
make test         # cargo test -p outflow-core -p outflow-net
make run          # rebuild app/dist, then serve (after a UI change)
make serve        # serve the existing app/dist (backend-only iteration)
make dev          # hot-reload UI, proxies /api → :8080
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/plaid-fixture.json
```

The bare server binary never rebuilds the frontend, so a stale `app/dist`
serves the old bundle. `make help` lists every target.

Look these up instead of guessing (grep the heading, read that range):

- CLI subcommands, the shared date flags, `--server`/`--json`:
  `docs/DEVELOPMENT.md` § CLI (`outflow <cmd> --help` is authoritative).
  Dates everywhere go through `core::period`: `--period 2026-11 | 2026-W45 |
  2026-Q4 | 2026 | this-/last-week|month|quarter|year`, or `--since`/`--until`
  (inclusive, local days), never both.
- HTTP API: `docs/API.md`.
- Server env vars and secrets files: the config table in
  `docs/ARCHITECTURE.md`; launchd setup in `docs/DEPLOYMENT.md`. Setting
  either API token (`OUTFLOW_API_TOKEN[_FILE]`, read-only
  `OUTFLOW_API_TOKEN_RO[_FILE]`) flips `/api/*` to deny-by-default; every
  `_FILE` is 0600-enforced and fatal if unreadable.

## Architecture

Ports-and-adapters over a pure core. One Cargo workspace, members
`core net cli server` (`app/` is an npm project, not a cargo member):

- **`core/`** — pure domain, zero GUI/network deps by default: `money`,
  `model`, `store` (rusqlite), `query`, `recurring`, `categorize`, `plaid`
  (pure JSON→domain parser), `transfers` (card-payment matcher), `backup`,
  `secrets`, ... Source of truth: **CLI and web run identical domain logic**.
- **`net/`** — networked adapters (sync ureq): Plaid transport, the 0600
  tokens file, `anthropic` (`Prompter`).
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
`ci-*` = Check-in (home), `fl-*` = Flow tab (`fl-rc-*` its recurring list), `hd-*` = Holdings tab, `fm-*` = Find money, `lg-*` = shell and shared (incl. the `lg-strip` month strip), `cx-*` = Connections (Settings), `st-*` = Settings, `mr-*` =
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
- Releases and deploys follow the `/release` skill, which sets the order:
  `scripts/deploy.sh try` on the branch, merge, `make release-draft`, tidy,
  `make release`, **show the user the notes and ask once** (that yes covers
  `git push --atomic origin main V`), confirm the workflows went green,
  `scripts/deploy.sh build V`, hand the user the sudo steps (`env set` if the
  release needs a setting, then `install V`) as one script, then
  `scripts/deploy.sh verify` and report only what it printed.
- Rollback: `scripts/deploy.sh rollback [V] [--restore-db]`, plus
  `env restore` if the plist changed. launchd runs
  `<data dir>/releases/current`, never this checkout, so building here doesn't
  touch the live server (docs/DEPLOYMENT.md).
- Never move or delete a pushed tag (a ruleset blocks it); fix forward with the
  next N. `main` blocks force-push; only for a history rewrite the user has
  approved does the ruleset get switched off, then back on.
- Pre-push leak scan: see the shared CLAUDE.md (dollar figures need the
  user's OK before `LEAKSCAN_ACK=1`).

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
