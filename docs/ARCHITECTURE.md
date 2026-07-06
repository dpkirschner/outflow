# Architecture

## Shape: ports-and-adapters over a pure core

One Cargo workspace, four crates (`Cargo.toml` `members`):

```
core/  pure domain — money, model, store, source, categorize, query,
       subscriptions, llm. Zero GUI/network deps by default. Source of truth.
net/   networked adapters — secrets (keychain + access-URL resolution),
       simplefin (fetch/claim), anthropic (Prompter impl). Depends on core.
cli/   headless binary `outflow` — pull, categorize, report, subs, fix, claim.
app/   Tauri v2 desktop GUI: src-tauri (Rust backend) + a React/Vite/Recharts
       frontend (src/). src-tauri depends on core + net.
```

Both front-ends depend on `core` directly and share `net`, so **the CLI and GUI
run identical domain logic** — a feature exists once, in core, and both call it.

### The two ports (traits)

The volatile externals are traits so they can be swapped without touching the
domain (invariant: keep core swappable):

- `source::TransactionSource` — where transactions come from. Today SimpleFIN
  JSON via `parse_account_set`; a Plaid adapter would implement the same trait.
- `categorize::Categorizer` — assigns a category to a transaction. `RuleSet`
  (deterministic) implements it. The LLM tail uses a second port,
  `llm::Prompter` (pure prompt build + response validation in core; the HTTP
  call is `net::anthropic::AnthropicPrompter`), so `core` stays network-free.

## Layering inside `core`

```
money            i64-cents type, decimal parse, display
  └ model        Account, Transaction, enums (+ merchant())
      └ store    SQLite persistence (rusqlite); upsert rules; rule/category CRUD
          ├ query          spend_by_category, top_merchants, monthly_flow
          ├ subscriptions  normalize_payee (THE normalizer), detect(), detect_rhythms()
          ├ ledger         the rhythm-ledger view model (ledger() → LedgerView)
          ├ categorize     RuleSet, precedence
          └ llm            Prompter trait, prompt build, response validation
```

`ledger` sits on top of `subscriptions` + `query` + `store`: `ledger()` runs the
stream detector, attaches per-stream `Source`s, and partitions a window **once**
into the screen's zones (streams / committed / notable / transfers / noise) so
nothing double-counts. It is the single source of truth for the ledger screen.

Analysis (`query`, `subscriptions`) reads all transactions from the store and
**aggregates in memory in Rust**, not in SQL. Months are bucketed on each
transaction's behavioral date (`effective_date()`) in the machine's **local
timezone** — `core` depends on `chrono` (pure computation, no network, so
invariant #3 holds) purely for this DST-correct conversion. Aggregations exclude
non-`Spending` transactions (transfers, card payments) by default.

Schema evolution on existing DBs runs through a **`PRAGMA user_version` migration
runner** in `store::migrate` (`run_migrations` + `SCHEMA_VERSION`): fresh DBs get
every column from the `CREATE TABLE`s, existing DBs get guarded `ALTER TABLE ADD
COLUMN`s. This matters because a real encrypted production DB holds the user's
data — `CREATE TABLE IF NOT EXISTS` alone never alters it.

## Data flow

```
SimpleFIN JSON ──net::simplefin::fetch──▶ parse_account_set ──▶ Store.upsert_*
   (or --from-file / demo fixture)          (validate→domain)      (SQLite)
                                                                      │
   Store.categorize_uncategorized(RuleSet) ◀── rule pass ────────────┤
   LlmCategorizer(Prompter) over the tail  ◀── optional AI pass ─────┤
                                                                      ▼
   query::* / subscriptions::detect ──▶ CLI stdout  |  Tauri commands ──▶ React dashboards
```

## Front-end wiring

- **CLI** (`cli/src/main.rs`) — clap subcommands map 1:1 to core calls. Net paths
  (`claim`, live `pull`, `categorize --llm`) are `#[cfg(feature = "net")]` thin
  wrappers over `net`.
- **GUI backend** (`app/src-tauri/src/main.rs`) — a `Mutex<Store>` in Tauri
  managed state (rusqlite `Connection` is not `Sync`); each `#[tauri::command]`
  locks it and calls core. Ledger commands: `ledger(window)`,
  `stream_occurrences(merchant, window)`, `mark_stream`/`clear_stream_mark`;
  editing: `set_category`, `set_flag`, `apply_flags`, `has_credit_account`; plus
  `accounts`, `categorize`, `categorize_llm`, `pull_live`, `claim`, `reset_data`
  (and legacy `spend_categories`/`merchants`/`flow`/`subscriptions`, still
  registered). A window string (`3mo`/`6mo`/`12mo`/`all`) becomes a `since` bound.
- **GUI frontend** (`app/src/`) — the primary screen is the **rhythm ledger**
  (`App.tsx` + `components/ledger/*`, styled by `ledger.css`): an action bar
  (connect/pull/categorize/reset + rolling-window control), the streams list with
  hand-rolled flexbox rhythm strips (not Recharts) and By-size/By-change sort, the
  Notable/Committed/Transfers/Noise zones, and a right slide-over for per-stream
  drill-down and editing (reclassify, per-txn flag/categorize, card-payment
  warning). `api.ts` wraps `invoke()`; `types.ts` mirrors the serde DTOs. The old
  dashboard components remain in the tree but are no longer mounted. See
  DATA_MODEL.md for the serialization boundary.

## Config resolution (the productionization crux)

A Finder-launched `.app` inherits **no shell environment**, so config cannot come
from env vars. Each value has a production source and a dev/CLI env override:

| Value | Dev / CLI override | Production (`.app`) source | Code |
|---|---|---|---|
| DB path | `OUTFLOW_DB` | `app_data_dir()/outflow.db` (`~/Library/Application Support/com.outflow.app/`), created on first run | `resolve_db_path` (src-tauri) |
| DB key (SQLCipher) | `OUTFLOW_DB_KEY` | keychain `outflow/db-key`, 32 random bytes, generated once | `resolve_db_key` (src-tauri) → `net::secrets::db_key_get_or_create` |
| SimpleFIN access URL | `OUTFLOW_SFIN_URL`, or `OUTFLOW_SFIN_URL_FILE` (0600) | keychain `outflow/simplefin-access-url` (written by `claim`) | `net::secrets::access_url` |
| LLM endpoint/model/key | `OUTFLOW_LLM_URL`, `OUTFLOW_LLM_MODEL`, `ANTHROPIC_API_KEY` | *(not yet — needs an in-app Settings panel; see GOTCHAS/deferred)* | `net::anthropic` |

## Feature flags

Keep networked/heavy deps out of the default build; the full pipeline runs with
zero features via `pull --from-file`.

| Crate | Feature | Enables |
|---|---|---|
| core | `encryption` | rusqlite `bundled-sqlcipher-vendored-openssl` → `Store::open_encrypted` |
| net | `keychain` | `keyring` + `getrandom` (access-URL + DB-key keychain storage) |
| cli | `net` / `keychain` / `encryption` | `dep:outflow-net` / `net`+`outflow-net/keychain` / `outflow-core/encryption` |
| app/src-tauri | default `["net","keychain"]`; `encryption` opt-in | same; production bundle adds `encryption` via `npm run bundle` |

## Deferred / roadmap (not bugs)

- **Local-model LLM** — needs an in-app Settings panel (env is unavailable to a
  double-clicked app) and likely an OpenAI-shaped `Prompter` (the current adapter
  is Anthropic-Messages-only). The `core::llm` trait boundary makes this cheap.
- **Transfer / card-payment handling** — handled by the `TxnFlag` suppression axis
  (`Transfer`/`CardPayment` excluded from analytics) plus behavioral dating, which
  nets a card payment against its underlying charges with no explicit payment↔charge
  link. Still manual (flag + learn-a-rule); *automatic* both-leg matching is a
  future refinement, and card-payment suppression assumes the card account is
  ingested (the app warns otherwise).
- **Annual subscriptions** — undetectable until the DB holds >1 year.
- **Local-timezone month bucketing** — done: buckets use `chrono::Local` on
  `effective_date()`.
