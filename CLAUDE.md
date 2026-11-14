# CLAUDE.md

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
  Dates everywhere go through `core::period` (`--period` or
  `--since`/`--until`, never both).
- HTTP API: `docs/API.md`.
- Server env vars and secrets files: the config table in
  `docs/ARCHITECTURE.md`; launchd setup in `docs/DEPLOYMENT.md`. Setting
  either API token makes `/api/*` deny-by-default.

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
- **`server/`** — axum + tokio, `Arc<Mutex<Store>>` via `spawn_blocking`;
  thin handlers over core (`routes.rs`, `plaid_routes.rs`), the sync engine
  (`sync.rs`), and the SPA from `app/dist`. Frontend in `app/src/` (React +
  Vite + TS).

**Styling:** plain CSS in one file, `app/src/ledger.css` (tokens are the
`--lg-*` properties on `:root`). Class prefixes by feature: `ci-` Check-in,
`fl-` Flow (`fl-rc-` recurring), `hd-` Holdings, `fm-` Find money, `lg-` shell
and shared, `cx-` Connections, `st-` Settings, `mr-` match review. Trace
`className` → rule with `rtk grep` in `ledger.css`.

**Ports:** fetch JSON in `net`, parse pure in `core` (`plaid::parse_*`);
`categorize::Categorizer` / `llm::Prompter` (rules today, LLM tail).

Analysis (`query`, `recurring`) loads all transactions and
**aggregates in memory in Rust**, not SQL. The durable SQLite archive grows
past the providers' pull windows — it's the permanent history.

## Releases and branches

Claude drives releases: every deploy is a release (`vYYYY.MM.N`, notes in
`CHANGELOG.md`), cut from `main` after a local merge of a whole feature
branch (no PRs). Follow the `/release` skill for the order and the one
question to ask; depth and rollback in `docs/RELEASING.md`. Never move or
delete a pushed tag; fix forward with the next N.

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

- **The wire format is snake_case** (serde field names straight over HTTP);
  `app/src/api.ts` is the single client and sends Rust field names.
- **`app/src/types.ts` is generated** from the Rust DTOs: change the struct,
  then `make types`. `make check` fails when it or the API contract snapshot
  is stale.
- **`None` filter must mean "everything"** (`TxnFilter::all()`). A derived
  `Default` would set `include_pending = false` and silently drop pending rows.
- Also in GOTCHAS: launchd has no keychain or shell env, plaintext and
  encrypted DBs aren't interchangeable, `/tmp` gets purged, OAuth banks need
  the exact redirect URI.
