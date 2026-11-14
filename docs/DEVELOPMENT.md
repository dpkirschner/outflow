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
server), `encryption`. Zero-feature builds still run the full
pipeline from a file:

```
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/plaid-fixture.json
```

```
cargo build -p outflow-cli --features "net,client,encryption"
```

Subcommands (`outflow <cmd> --help` is the authoritative list of flags):

| Command | What it gives |
|---|---|
| `pull --from-file P` | offline fixture ingest; live syncs are the server's job (`--server URL pull` triggers one) |
| `categorize [--llm]` | rule pass over uncategorized rows, then optionally the LLM tail |
| `fix <id> <category> [--no-learn \| --past]` | manual category; also points the merchant's rule at it unless `--no-learn`; `--past` gives the merchant's past rows it too (not those set by hand) |
| `report --by category\|merchant\|source\|monthly [--top N --posted-only]` | spending by category or merchant, In by source (payee), or Flow lanes per month (in / out / earned − spent / saved / Moved groups); no period = all history |
| `recurring [--ignored] [--subscriptions]` | recurring merchants over the 12 months ending with the period's last month (default this month): cadence, usual monthly cost, new / late / up / down >25%, year on year. `--subscriptions` keeps only Steady merchants and totals those (the ignored list stays whole) |
| `checkin [--offers F --expiring-days N]` | one week, month or quarter (default this month): net worth change, earned − spent, saved, each against its usual, and what's unusual, including Findings new this period (`find money  <subject>: <action> (about $N a year)  [key]`; offers from `--offers` / `OUTFLOW_PERKS`, a bad file skips them); `--categories` lists Out per category against each one's usual instead |
| `change` | net worth change (default this month) split into Markets + Moved in + Earned - spent + Other, per account |
| `balances [--history]` | current state led by cash + invested − credit − loans = net worth; `--history` = daily trend |
| `holdings [--account A --ticker T] [--history \| --by account\|position]` | current positions; with a period or `--by`, gain over the period (default this month); `--history` = per-sync snapshots |
| `activity [--account A --ticker T --type T]` | investment transactions; amount = cash effect |
| `txns [--search --category --account --merchant --ids --flag --lane --direction --uncategorized --posted-only --show-excluded --sort --asc --limit --offset --by-month]` | the transaction list; `--lane` = exactly the rows behind a `report --by monthly` figure; `--ids a,b` = only those rows (a Finding's `txn_ids`); `--by-month` = their sum per month instead (`/flow/series`) |
| `accounts [--all]` | one row per account (`--all` includes hidden) |
| `matches list\|accept\|reject\|unaccept` | card-payment match review |
| `find [--file F --expiring-days N] [--dismiss KEY \| --restore KEY \| --review MERCHANT \| --unreview MERCHANT]` | Find money: every Finding ranked by $/yr over the 12 months ending with the period's last month (default this month), then the dismissed and reviewed ones (card offers, card choice from the files' `[earn]` tables, steady subscriptions to review and price rises); also validates the offers files. `--dismiss`/`--restore` set or clear the Dismissed mark by the key printed under each Finding; `--review`/`--unreview` set or clear the Reviewed mark by the merchant printed under a subscription or price rise |
| `status [--limit N]` | what needs you (connections, matches to review, last sync), then sync runs |
| `backup --to PATH` | a verified snapshot of the DB, encrypted with the same key, safe while the server runs; local only (see DEPLOYMENT §8) |

Dates: every date-filtered command (`report`, `recurring`, `change`,
`balances --history`, `holdings`, `activity`, `txns`) takes `--period P`
(`2026-11`, `2026-W45`, `2026-Q4`, `2026`, or `this-`/`last-`
`week|month|quarter|year`) or `--since D` / `--until D` (`YYYY-MM-DD`, both
inclusive), not both. `checkin` takes `--period` only, a week, month or
quarter. Days are local time.

`holdings` (and `/api/holdings`) also emits one synthetic row per investment
account whose balance its positions don't cover: `security_id`
`cash:uninvested`, `is_cash: true`, so all-cash accounts (an HSA with no
positions) still appear. History (`--history`) has no such row.

Global: `--db`/`OUTFLOW_DB`, `--json` (machine-readable, below),
`--server URL`/`OUTFLOW_SERVER` (needs `--features client`; every command
becomes one API call; bearer auth via `OUTFLOW_API_TOKEN`; always JSON).
Global flags go before or after the subcommand. With `--features
encryption`, the DB key comes from `OUTFLOW_DB_KEY`, else
`OUTFLOW_DB_KEY_FILE` (0600).

### JSON output and exit codes

The CLI follows the conventions agent wrappers share, so a caller can branch
on the exit code alone. With `--json` (or `--server`) it prints exactly one
JSON object on stdout:

- **Success:** `{"ok":true, ...}`. An object payload's fields sit at the top
  level beside `ok`, in the API's serde shape. A list goes under `items`
  (`outflow accounts --json` → `{"ok":true,"items":[...]}`). Writes return
  what the API returns: `fix` its `CategoryEdit` (with `--past`, the `PastEdit` under `applied`), `categorize`
  `{rule, remaining, llm?}`, `matches accept|reject|unaccept` `{id}`.
- **Failure:** `{"ok":false,"error":{"code":N,"message":"..."}}`, where
  `code` is the exit code.

Without `--json`, output is text and errors go to stderr as `error: ...`.
Local and `--server` output are the same, so consumers don't care which mode
ran. The HTTP API itself stays bare (no envelope).

| Exit | Meaning | e.g. |
|---|---|---|
| 0 | done | |
| 1 | internal: a bug, or an error it can't classify | DB error |
| 2 | usage: bad arguments, or input it won't take | bad `--period`, unknown flag, a malformed `--from-file` |
| 3 | not found | `matches accept 99`, `fix` on an unknown txn, unknown Finding key |
| 4 | conflict: already done, or the state changed | `matches unaccept` on a match that isn't accepted |
| 5 | unreachable: a service is down, slow or rate-limited | server unreachable, HTTP 502/503/504/429, the LLM call |
| 6 | denied: credentials refused or unreadable | HTTP 401, unreadable `OUTFLOW_DB_KEY_FILE`, no `ANTHROPIC_API_KEY` |
| 7 | refused: a guard said no | HTTP 403: the read-only token on a write |

Agent usage over the tailnet:

```
OUTFLOW_SERVER=https://MINI.TAILNET.ts.net outflow txns --search coffee --period last-month --json
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
`OUTFLOW_PLAID_ENV=sandbox`, open Settings → Connections, link institution
`user_good`/`pass_good`. Production setup lives in DEPLOYMENT.md.

Makefile shortcuts (`make help` lists them all): `make test` / `make web` /
`make run` (rebuilds `app/dist`, then serves) / `make serve` (serves the
existing `app/dist`) / `make dev` / `make deps` / `make types` / `make check`
(the gate: fmt, clippy, tests, types.ts + contract freshness, `tsc`,
`make leakscan`) / `make release-draft` + `make release` and
`make deploy-build` + `make deploy` (see RELEASING.md). None of these touch
the deployed server: it runs a release from the data dir, not this checkout.

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
