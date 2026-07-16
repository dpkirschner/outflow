# outflow

A **single-user spending analyzer** for one person's own bank data. A server
(target: an always-on mac-mini, exposed over Tailscale) pulls transactions from
[**Plaid**](https://plaid.com/) (checking/savings/credit cards) into a local
SQLite database, categorizes them, auto-detects checking→card payments so
spending isn't double-counted, and serves a web UI + JSON API to answer "where
did my money go?", "how much did I spend at that coffee shop?", and "what
subscriptions am I paying for?".

Analysis, not budgeting — no limits or targets, just visibility. Your data
lives in a local SQLite file and only ever leaves your machine to talk to Plaid
(and, if you enable it, an LLM endpoint you configure for categorizing merchant
names).

> Personal project. It targets macOS, ships as build-from-source, and is tuned
> for one person analyzing their own accounts.

## Highlights

- **Local-first & private** — transactions live in a local SQLite file that is
  the permanent archive; optional at-rest encryption via SQLCipher. Secrets
  (Plaid keys, access tokens, DB key) live in env vars or 0600 files, never in
  the database or in argv. No telemetry.
- **One core, two front-ends** — a pure Rust domain `core`, an axum **server**
  that serves a React SPA + JSON API, and a headless **CLI** (direct-DB or
  HTTP-client mode against the server). Both front-ends call the same core, so
  they run identical domain logic.
- **The rhythm ledger** — the primary view: recurring streams (with cadence,
  trend, and which card/account they hit), committed vs. discretionary, notable
  one-offs, and a noise fold — all with card-payments and transfers suppressed.
- **Card-payment matching** — pairs a checking→card payment with the card's
  balance payment so the same money isn't counted as both an outflow and the
  card's underlying charges. High-confidence pairs auto-match; ambiguous ones
  queue for review.
- **Categorization** — a deterministic rule engine that *learns rules* from your
  inline corrections, with an optional LLM pass for merchants rules can't place.
- **Durable history** — Plaid serves a limited window per pull (up to ~24 months
  on first sync); the local database accumulates indefinitely, so trends and
  subscription detection sharpen over time.

## How it works

```
your banks ──Plaid /transactions/sync──▶ server ──▶ local SQLite ──▶ categorize + match ──▶ web UI / JSON API / CLI
```

Built as a Cargo workspace around ports-and-adapters: a pure `core` domain crate
with zero GUI/network deps by default, networked adapters in `net`, and two thin
front-ends (`server`, `cli`). `app/` is a React + Vite + TS SPA (not a cargo
member), built to `app/dist` and served by the server. The transaction source
is a fetch-in-`net` / parse-in-`core` pipeline and the categorizer is a trait,
so the source and rules→LLM tail are swappable without touching the domain.

Deeper reference lives in [`docs/`](docs/):
[architecture](docs/ARCHITECTURE.md) ·
[data model](docs/DATA_MODEL.md) ·
[invariants](docs/INVARIANTS.md) ·
[development](docs/DEVELOPMENT.md) ·
[gotchas](docs/GOTCHAS.md) ·
[deployment](docs/DEPLOYMENT.md).

## Getting started

### Prerequisites

- A current stable Rust toolchain (via [rustup](https://rustup.rs/))
- Node.js + npm (for the web client)
- A [Plaid](https://dashboard.plaid.com/) account for live data. Sandbox works
  immediately (`user_good` / `pass_good`); production needs a Trial-plan request.

### Offline, zero-credential pipeline (the fastest look)

With no feature flags, the full pipeline runs against a bundled fixture:

```
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/plaid-fixture.json
cargo run -p outflow-cli -- --db "$HOME/outflow.db" categorize
cargo run -p outflow-cli -- --db "$HOME/outflow.db" report --by category
cargo run -p outflow-cli -- --db "$HOME/outflow.db" report --by merchant --top 10
cargo run -p outflow-cli -- --db "$HOME/outflow.db" subs
```

The database path also reads from `OUTFLOW_DB`.

### The server + web client

```
cd app && npm install && npm run build
OUTFLOW_DB="$HOME/outflow-dev.db" cargo run -p outflow-server
```

Then open the printed URL. For live Plaid sandbox, set `OUTFLOW_PLAID_CLIENT_ID`,
`OUTFLOW_PLAID_SECRET`, `OUTFLOW_PLAID_ENV=sandbox`, go to the **Connections**
tab, and link an institution with `user_good` / `pass_good`. A `Makefile` wraps
the common commands — run `make help`.

### CLI feature flags

```
cargo build --release -p outflow-cli --features "net,client,encryption"
```

| Feature | Enables |
|---|---|
| `net` | the optional LLM categorizer (`categorize --llm`) |
| `client` | HTTP mode against a running server (`--server URL` / `OUTFLOW_SERVER`) |
| `encryption` | open the SQLite database with SQLCipher (`OUTFLOW_DB_KEY[_FILE]`) |

With no features, the full pipeline still runs offline via `pull --from-file`.

## Deployment

The production posture is `outflow-server` as a launchd daemon on an always-on
mac-mini, bound to loopback, with `tailscale serve` terminating HTTPS on the
tailnet — every device on the tailnet gets the web UI (prompting once for an
API token), and agents/scripts use the CLI in `--server` mode against the same
URL with a read-only token. Full recipe (Plaid dashboard, tailscale, launchd,
0600 secret files) in [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md).

## Known limitations

- Card-payment auto-matching pairs equal amounts within 5 days; partial/split
  payments need a manual flag.
- Own-account transfers auto-suppress only when Plaid classifies them; anything
  else is a manual "exclude".
- Annual subscriptions are only detectable once the database holds over a year
  of history.

## License

[PolyForm Noncommercial 1.0.0](LICENSE). The source is public and you're welcome
to use, modify, and share it for **noncommercial** purposes — personal use, study,
hobby projects, nonprofits. **Commercial use requires permission** from the
copyright holder. (This is a source-available license, not an OSI "open source"
one.)
