# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

outflow is a local, single-user spending analyzer for one person's own bank data.
It pulls transactions via SimpleFIN into local SQLite, categorizes them (rules +
optional LLM), and reports cash flow / spend-by-category / merchants /
subscriptions. Analysis only — no budgets. macOS-targeted, build-from-source.

Deeper reference lives in `docs/` (all tracked; read these for depth):
ARCHITECTURE, DATA_MODEL, INVARIANTS, GOTCHAS, DEVELOPMENT. An untracked
`agent.md` holds volatile session status and demo credentials — never `git add` it.

## Commands

Test the pure crates by package — **never run bare `cargo test` at the root**; it
tries to compile `app/src-tauri`, whose `generate_context!` needs a pre-built
frontend (`app/dist`) and fails without a prior `vite build`:

```
cargo test -p outflow-core -p outflow-net
```

CLI (`outflow` binary). Features: `net` (live SimpleFIN + LLM), `keychain`,
`encryption`. With no features the full pipeline still runs via `--from-file`:

```
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/sample-accounts.json
```

Subcommands: `claim <token>`, `pull [--from-file P]`, `categorize [--llm]`,
`report --by category|merchant|monthly [--top --since --until --posted-only]`,
`subs`, `fix <id> <category> [--no-learn]`. DB path from `--db` or `OUTFLOW_DB`.

GUI dev (plaintext, `net`+`keychain` on; point `OUTFLOW_DB` at a populated DB):

```
cd app && npm install && OUTFLOW_DB="$HOME/outflow.db" npm run tauri dev
```

GUI production bundle (`= tauri build --features encryption`) → encrypted,
ad-hoc-signed `outflow.app` + `.dmg` under `app/src-tauri/target/release/bundle/`:

```
cd app && npm run bundle
```

## Architecture

Ports-and-adapters over a pure core. One Cargo workspace, members
`core net cli app/src-tauri` (`app/` itself is an npm project, not a cargo member):

- **`core/`** — pure domain, zero GUI/network deps by default. `money` → `model`
  → `store` (SQLite via rusqlite) → `query` / `subscriptions` / `categorize` /
  `llm`. Source of truth; both front-ends call it directly so **CLI and GUI run
  identical domain logic**.
- **`net/`** — networked adapters: `simplefin` (fetch/claim), `secrets`
  (keychain + access-URL/DB-key resolution), `anthropic` (`Prompter` impl).
- **`cli/`** — headless `outflow` binary; clap subcommands map 1:1 to core calls.
- **`app/src-tauri`** — Tauri v2 backend, a `Mutex<Store>` in managed state
  (rusqlite `Connection` isn't `Sync`); thin `#[tauri::command]`s over core.
  Frontend in `app/src/` (React + Vite + TS + Recharts).

**Two swappable ports (traits):** `source::TransactionSource` (SimpleFIN today,
Plaid later) and `categorize::Categorizer` / `llm::Prompter` (rules today, LLM
tail). Keep all transport/IO behind them; the HTTP call for the LLM lives in
`net::anthropic`, prompt-build + response validation stay pure in `core::llm`.

Analysis (`query`, `subscriptions`) loads all transactions and **aggregates in
memory in Rust**, not SQL. The durable SQLite archive grows past SimpleFIN's
~90-day pull window — it's the permanent history.

## Load-bearing invariants (see docs/INVARIANTS.md)

Violating these breaks data integrity or the domain contract:

1. **Money is `i64` minor units (cents), never floats.** Outflows negative.
   Parse via `Money::from_decimal_str`. It's a serde-transparent newtype → crosses
   to JS as a bare **number** (cents); format with `formatCents`, never expect an object.
2. **Dedup key is the SimpleFIN transaction id.** Posted transactions upsert by
   id; **pending are delete-and-replace per synced account each pull** (avoids
   pending→posted double-counting). See `store::upsert_transactions`.
3. **`core` stays network/GUI-free by default.** Network, keychain, encryption
   are cargo features; the pipeline must stay runnable via `pull --from-file`
   with zero features. Network code lives in `net`.
4. **One merchant normalizer everywhere: `subscriptions::normalize_payee`.**
   Categorizer matching, merchant reports, and subscription detection all key off
   it. Do not add a second normalizer.
5. **Boundary parsing.** External JSON deserializes then converts to domain types
   with explicit validation; malformed data returns `SourceError`, never leaks
   defaults (`source::parse_account_set`).
6. **Secrets never touch the DB or argv** — env, a 0600 file, or the keychain
   only. The DB key is regenerated **only** on keychain `NoEntry`, never on a
   locked/denied read (a new key permanently orphans an existing encrypted DB).

## Traps that have already cost time (see docs/GOTCHAS.md)

- **Tauri camelCase↔snake_case.** JS calls a Rust `txn_id` param as `{ txnId }`.
- **`None` filter must mean "everything"** (`TxnFilter::all()` / `to_filter`).
  `FilterArg`'s derived `Default` sets `include_pending = false` and silently
  drops pending rows — don't rely on it.
- **A Finder-launched `.app` inherits no shell env**, so `OUTFLOW_*` vars are all
  unset when double-clicked. Production config resolves from app-data dir +
  keychain (`resolve_db_path` / `resolve_db_key` in `app/src-tauri/src/main.rs`).
- **Plaintext and encrypted DBs are not interchangeable** — switching a path's
  mode requires deleting the file first.
- **Don't stage the DB or secrets in `/tmp`** — macOS purges it and the app
  silently recreates an empty DB. Use `$HOME/...` or the app-data dir.

## Toolchain note

`cli/Cargo.toml` pins `clap = "=4.4.18"` only because it was first built on Rust
1.75; on current stable this can loosen to `clap = "4"`. Nothing else depends on it.
