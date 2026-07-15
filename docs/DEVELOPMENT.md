# Development

## Layout

```
core/  net/  cli/  server/     Rust crates (the whole cargo workspace)
app/                           npm project (NOT a cargo member)
  src/                         React + Vite + TS frontend
  dist/                        built SPA (served by outflow-server)
examples/plaid-fixture.json    offline ingest envelope for --from-file
examples/plaid-accounts-get.json / plaid-sync-page.json   Plaid parser fixtures
```

Give shell commands as **one single-line command per code block** — the user
copy-pastes, and wrapped/multi-line blocks break.

## Test / build

```
cargo test -p outflow-core -p outflow-net
```

```
cargo build
```

Bare `cargo build`/`cargo test` at the root work now (the Tauri crate and its
pre-built-frontend requirement are gone). The `-p` form stays the fast path.

## CLI

Features: `net` (LLM categorizer), `client` (HTTP mode against a running
server), `keychain`, `encryption`. Zero-feature builds still run the full
pipeline from a file:

```
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/plaid-fixture.json
```

```
cargo build -p outflow-cli --features "net,client,keychain,encryption"
```

Subcommands: `pull --from-file P` (offline fixture ingest; live syncs are the
server's job), `categorize [--llm]`,
`report --by category|merchant|monthly [--top --since --until --posted-only]`,
`subs`, `fix <id> <category> [--no-learn]`, `txns [--search --category
--account --source --flag --sort --asc --limit --offset --since --until
--posted-only --show-excluded]`, `accounts`, `matches list|accept <id>|reject
<id>`, `status [--limit]`.

Global: `--db`/`OUTFLOW_DB`, `--json` (machine-readable; same serde shapes as
the HTTP API), `--server URL`/`OUTFLOW_SERVER` (needs `--features client`;
every command becomes one API call and prints the server's JSON verbatim;
bearer auth via `OUTFLOW_API_TOKEN`).

Agent usage over the tailnet:

```
OUTFLOW_SERVER=https://mini.tailnet.ts.net outflow txns --search starbucks --since 2026-06-01 --json
```

## Server + web client

First-time frontend setup, then build the SPA:

```
cd app && npm install && npm run build
```

Run the server (serves `app/dist` + `/api`):

```
OUTFLOW_DB="$HOME/outflow-dev.db" cargo run -p outflow-server
```

Frontend hot-reload during UI work (vite on :1420 proxies `/api` → :8080; run
the server alongside):

```
cd app && npm run dev
```

The server logs `opened database` with the path and row count on startup — a
one-glance check of which DB it opened. An empty/fresh file is the usual
reason "no data shows".

Plaid against sandbox: set `OUTFLOW_PLAID_CLIENT_ID`, `OUTFLOW_PLAID_SECRET`,
`OUTFLOW_PLAID_ENV=sandbox`, open the Connections tab, link institution
`user_good`/`pass_good`. Production setup lives in DEPLOYMENT.md.

Makefile shortcuts: `make test` / `make web` / `make server` / `make run` /
`make dev` / `make deps`.

## Definition of done (project)

`cargo test` green; `cargo build` (workspace) clean; sandbox Plaid link →
transactions land with `source='plaid'`, correct signs and kinds, second sync
incremental; card-payment pairs auto-match and monthly outflows exclude them;
web UI works from another tailnet device; CLI (both modes) emits identical
JSON.

## Toolchain note

`cli/Cargo.toml` pins `clap = "=4.4.18"` only because it was first built on
Rust 1.75. On current stable, loosen to `clap = "4"`. Nothing else depends on
the pin.
