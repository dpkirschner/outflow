# outflow

A **local, single-user spending analyzer** for your own bank data. It pulls
transactions from your bank via [SimpleFIN](https://www.simplefin.org/), stores
them in a local SQLite database, categorizes them, and answers questions like
"where did my money go?", "how much did I spend at that coffee shop?", and "what
subscriptions am I paying for?".

Analysis, not budgeting — no limits or targets, just visibility. Everything runs
on your machine; your financial data never leaves it.

> Personal project. It targets macOS, ships as build-from-source (no notarized
> release), and is tuned for one person analyzing their own accounts.

## Highlights

- **Local-first & private** — data lives in a local SQLite file; the desktop app
  encrypts it at rest (SQLCipher) with a key held in your macOS keychain. Bank
  credentials are stored in the keychain, never in the database or in argv.
- **Two front-ends, one core** — a Tauri desktop app with a dark dashboard, and a
  headless CLI for cron/automation. Both run identical domain logic.
- **Dashboards** — monthly cash flow (in vs out + net), spend by category,
  merchant leaderboard, and detected subscriptions, plus a transaction list with
  inline re-categorization that *learns rules* from your corrections.
- **Categorization** — a deterministic rule engine, with an optional LLM pass for
  the merchants rules can't place (model endpoint is configurable).
- **Durable history** — SimpleFIN returns ~90 days per pull; the local database is
  the permanent archive and accumulates indefinitely, so trends and subscription
  detection sharpen over time.

## How it works

```
your bank ──SimpleFIN──▶ outflow (pull) ──▶ local SQLite ──▶ categorize ──▶ dashboards / reports
```

Built as a Cargo workspace around ports-and-adapters: a pure `core` domain crate
with zero GUI/network deps, networked adapters in `net`, and two thin front-ends
(`cli`, `app`). The transaction source and the categorizer are traits, so
SimpleFIN→Plaid and rules→LLM are swappable without touching the domain.

Deeper reference lives in [`docs/`](docs/):
[architecture](docs/ARCHITECTURE.md) ·
[data model](docs/DATA_MODEL.md) ·
[invariants](docs/INVARIANTS.md) ·
[development](docs/DEVELOPMENT.md) ·
[gotchas](docs/GOTCHAS.md).

## Getting started

### Prerequisites

- A current stable Rust toolchain (via [rustup](https://rustup.rs/))
- Node.js + npm (for the desktop app)
- A [SimpleFIN Bridge](https://bridge.simplefin.org/) account to connect your
  bank and generate a setup token (a small paid service). You can try everything
  first against SimpleFIN's public demo bridge.

### The desktop app

Build the macOS `.app` and install it:

```
cd app && npm install && npm run bundle
cp -R app/src-tauri/target/release/bundle/macos/outflow.app /Applications/
```

Open it from Launchpad/Applications. On first run it creates an encrypted
database under `~/Library/Application Support/com.outflow.app/`. Click **Connect**,
paste your SimpleFIN setup token, then **Pull** — your accounts and transactions
load, and every later launch refreshes in the background.

### The CLI (headless)

```
cargo run -p outflow-cli -- --db out.db pull --from-file examples/sample-accounts.json
cargo run -p outflow-cli -- --db out.db categorize
cargo run -p outflow-cli -- --db out.db report --by category
cargo run -p outflow-cli -- --db out.db report --by merchant --top 10
cargo run -p outflow-cli -- --db out.db report --by monthly --since 2024-01-01
cargo run -p outflow-cli -- --db out.db subs
```

The database path also reads from `OUTFLOW_DB`. Reports accept `--since` /
`--until` (YYYY-MM-DD, since-inclusive, until-exclusive) and `--posted-only`.

For live use, build with feature flags:

```
cargo build --release -p outflow-cli --features "net,keychain,encryption"
```

| Feature | Enables |
|---|---|
| `net` | live SimpleFIN `pull`/`claim` and the optional LLM categorizer |
| `keychain` | read/write the access URL (and DB key) via the OS keychain |
| `encryption` | open the SQLite database with SQLCipher (`OUTFLOW_DB_KEY`) |

With no features, the full pipeline still runs offline via `pull --from-file`.

## Status

Working: the domain core, the CLI, live SimpleFIN pull, rule + LLM
categorization, the Tauri desktop app with all dashboards and learn-on-correct,
and transparent at-rest encryption for the packaged app.

Roadmap: an in-app settings panel to point the categorizer at a **local** model;
transfer detection (so moving money between your own accounts isn't counted as
spending); local-timezone month bucketing.

## Known limitations

- With only a checking account and no transfer detection, transfers (to savings,
  card payments) read as outflow, so totals overstate consumption until card
  accounts and transfer matching are added.
- Annual subscriptions are only detectable once the database holds over a year of
  history.
- Monthly buckets are UTC.

## Privacy

outflow is local-first: it talks only to SimpleFIN (to fetch your data) and,
if you enable it, an LLM endpoint you configure (for categorizing merchant
names). There is no telemetry and no server. Your transactions stay in a local,
optionally-encrypted SQLite file.

## License

[PolyForm Noncommercial 1.0.0](LICENSE). The source is public and you're welcome
to use, modify, and share it for **noncommercial** purposes — personal use, study,
hobby projects, nonprofits. **Commercial use requires permission** from the
copyright holder. (This is a source-available license, not an OSI "open source"
one.)
