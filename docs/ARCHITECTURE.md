# Architecture

## Shape: ports-and-adapters over a pure core

One Cargo workspace, four crates (`Cargo.toml` `members`):

```
core/    pure domain — money, model, store, plaid, categorize, query,
         subscriptions, ledger, transfers, llm. Zero GUI/network deps by
         default. Source of truth.
net/     networked adapters (sync ureq) — plaid (Link/exchange/sync
         transport), plaid_tokens (0600 token file), secrets (0600-file
         helpers), anthropic (Prompter impl). Depends on core.
cli/     headless binary `outflow` — pull --from-file, categorize, report,
         subs, fix, txns, accounts, matches, status; direct-DB or HTTP client
         mode (--server) against a running server.
server/  axum + tokio HTTP server — JSON API + serves the built React SPA.
         The always-on deployment target (mac-mini behind tailscale serve).
app/     npm project (NOT a cargo member): React + Vite + TS frontend in src/,
         built to app/dist and served by `server`.
```

Both front-ends depend on `core` directly and share `net`, so **the CLI and the
web app run identical domain logic** — a feature exists once, in core, and both
call it.

### The ports

The volatile externals are seams so they can be swapped without touching the
domain:

- **The transaction source** — one shape: raw JSON is fetched in `net`
  (`plaid::transactions_sync_page`/`accounts_get`) and parsed pure in `core`
  (`plaid::parse_sync_page`/`parse_accounts_get`/`parse_fixture`), producing
  domain `Account`/`Transaction` values. A future source (e.g. Plaid
  Investments for brokerage) follows the same pattern.
- `categorize::Categorizer` — assigns a category to a transaction. `RuleSet`
  (deterministic) implements it. The LLM tail uses a second port,
  `llm::Prompter` (pure prompt build + response validation in core; the HTTP
  call is `net::anthropic::AnthropicPrompter`), so `core` stays network-free.

## Layering inside `core`

```
money            i64-cents type, decimal parse, display
  └ model        Account, Transaction, PlaidItem, TxnMatch, enums (+ merchant())
      └ store    SQLite persistence (rusqlite); upsert rules; plaid batches;
                 rule/category/match CRUD; sync_log
          ├ query          spend_by_category, top_merchants, monthly_flow,
          │                search_transactions (text/sort/filter/pagination)
          ├ subscriptions  normalize_payee (THE normalizer), detect(), detect_rhythms()
          ├ ledger         the rhythm-ledger view model (ledger() → LedgerView)
          ├ transfers      detect_card_payments() — checking↔card pair matcher
          ├ categorize     RuleSet, precedence
          ├ plaid          pure Plaid JSON → domain (sign flips, kind mapping,
          │                flag/category seeding from personal_finance_category)
          └ llm            Prompter trait, prompt build, response validation
```

Analysis (`query`, `subscriptions`, `ledger`) reads all transactions from the
store and **aggregates in memory in Rust**, not in SQL. Months are bucketed on
each transaction's behavioral date (`effective_date()`) in the machine's
**local timezone** — `core` depends on `chrono` (pure computation, no network)
for this DST-correct conversion. Aggregations exclude non-`Spending`
transactions (transfers, card payments) by default.

Schema evolution on existing DBs runs through a **`PRAGMA user_version`
migration runner** in `store::migrate` (`run_migrations` + `SCHEMA_VERSION`,
currently v3): fresh DBs get every column from the `CREATE TABLE`s, existing
DBs get guarded `ALTER TABLE ADD COLUMN`s.

## Data flow

```
Plaid /transactions/sync ──net::plaid──▶ core::plaid::parse_sync_page ─┐
  (cursor loop per item)                  (validate→domain, sign flip)  │ apply_plaid_batch
                                                                        ▼ (atomic incl. cursor)
fixture file (pull --from-file) ──▶ core::plaid::parse_fixture ──▶ Store.upsert_*
   (offline dev/test path)              (validate→domain)             (SQLite)
                                                                      │
   post-ingest: apply_flags ── categorize(RuleSet) ── transfers::detect_card_payments
     (high-confidence pairs auto-flag CardPayment; ambiguous queue for review)
                                                                      ▼
   query::* / ledger / subscriptions ──▶ server /api/* ──▶ React SPA
                                     └──▶ CLI stdout (tables or --json)
```

## Front-end wiring

- **Server** (`server/src/`) — `AppState { Arc<Mutex<Store>>, sync_lock, cfg }`
  (rusqlite `Connection` is not `Sync`; blocking work runs via
  `spawn_blocking`). `routes.rs` ports the former Tauri command set 1:1;
  `plaid_routes.rs` handles Link token/exchange/items; `match_routes.rs` the
  card-payment review; `sync.rs` is the engine — per-item Plaid cursor loops,
  post-ingest passes, `sync_log` writes, and a
  `tokio::time::interval` background task (default 6h). Static serving:
  `/assets` from `app/dist`, everything else falls back to `index.html` with a
  200 (`/oauth-return` must load the SPA for Plaid OAuth resumption).
- **CLI** (`cli/src/main.rs`) — clap subcommands map 1:1 to core calls. The
  LLM pass (`categorize --llm`) is `#[cfg(feature="net")]`;
  `--server URL` (feature `client`, `cli/src/remote.rs`) redirects every
  subcommand to the HTTP API and prints the server's JSON verbatim — identical
  shapes to local `--json`, so agent consumers don't care which mode ran.
- **Frontend** (`app/src/`) — four tabs in `App.tsx`: the **rhythm ledger**
  (primary, `components/ledger/*`), **Outflows** (month bars + searchable/
  sortable/filterable table, `components/Outflows.tsx`), **Review**
  (card-payment matches, `components/MatchReview.tsx`), and **Connections**
  (Plaid Link + item health + sync log, `components/Connections.tsx`).
  `api.ts` is the single HTTP client; `types.ts` mirrors the serde DTOs. See
  DATA_MODEL.md for the serialization boundary.

## Config resolution (headless server)

The server runs headless (launchd on a mac-mini): no shell env at login, no
unlockable GUI keychain. Everything resolves from a launchd env block + 0600
files. See DEPLOYMENT.md for the full recipe.

| Value | Env | Notes |
|---|---|---|
| DB path | `OUTFLOW_DB` (default `~/Library/Application Support/outflow/outflow.db`) | override dir with `OUTFLOW_DATA_DIR` |
| Listen addr | `OUTFLOW_LISTEN` (default `127.0.0.1:8080`) | fronted by `tailscale serve` for HTTPS |
| Web dir | `OUTFLOW_WEB_DIR` (default `app/dist`) | the built SPA |
| Plaid creds | `OUTFLOW_PLAID_CLIENT_ID`, `OUTFLOW_PLAID_SECRET` or `OUTFLOW_PLAID_SECRET_FILE` (0600) | `OUTFLOW_PLAID_ENV` = `sandbox` (default) \| `production` |
| Plaid access tokens | `OUTFLOW_PLAID_TOKENS_FILE` (default `<data-dir>/plaid-tokens.json`, 0600) | per-item map; never in the DB |
| OAuth redirect | `OUTFLOW_OAUTH_REDIRECT` | `https://<mini>.<tailnet>.ts.net/oauth-return`, must match the Plaid dashboard exactly |
| Sync cadence | `OUTFLOW_SYNC_INTERVAL_SECS` (default 21600) | background interval |
| API auth | `OUTFLOW_API_TOKEN` (optional bearer) | tailnet is the primary boundary |
| DB key (encryption builds) | `OUTFLOW_DB_KEY` / `OUTFLOW_DB_KEY_FILE` (0600) | env or 0600 file only — no keychain path |
| LLM | `ANTHROPIC_API_KEY`, `OUTFLOW_LLM_URL`, `OUTFLOW_LLM_MODEL` | `net::anthropic` |

## Feature flags

Keep networked/heavy deps out of the default build; the full pipeline runs with
zero features via `pull --from-file`.

| Crate | Feature | Enables |
|---|---|---|
| core | `encryption` | rusqlite `bundled-sqlcipher-vendored-openssl` → `Store::open_encrypted` |
| cli | `net` / `client` / `encryption` | LLM categorizer / HTTP mode against a server / SQLCipher |
| server | `encryption` | SQLCipher via `OUTFLOW_DB_KEY[_FILE]` |

`server` always has network (it is the network layer); it depends on `net`
unconditionally.

## Deferred / roadmap (not bugs)

- **Brokerage accounts (Vanguard/Fidelity)** — Plaid Investments product. The
  seam is ready: enable the product in the Plaid dashboard, add
  `investments/holdings/get` + `investments/transactions/get` to `net::plaid`,
  a `parse_investments` in `core::plaid`, and map `investment/*` account types
  to a new `AccountKind`. Nothing else changes.
- **Plaid webhooks** — skipped on purpose (server isn't internet-reachable);
  periodic polling covers a personal archive. Tailscale Funnel could expose a
  webhook path later.
- **Local-model LLM** — an OpenAI-shaped `Prompter` behind the same trait.
- **Annual subscriptions** — undetectable until the DB holds >1 year.
