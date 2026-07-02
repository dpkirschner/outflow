# outflow

A local, single-user spending analyzer. Pulls transactions from a bank via
SimpleFIN, stores them in a local SQLite database, categorizes them, and
answers questions like "where did my money go?", "how much did I spend at
Starbucks last month?", and "what subscriptions am I paying for?".

Analysis, not budgeting: there are no limits or targets, just visibility.

## Design

The codebase is a Cargo workspace built around ports and adapters. The `core`
crate holds all domain logic and is deployment-agnostic. Two thin frontends sit
on top of it, sharing identical logic:

- a desktop GUI (Tauri) for interactive use
- a CLI binary for headless use (cron, agent invocation)

The two volatile external dependencies — the transaction source and the
categorizer — are behind interfaces so they can be swapped (SimpleFIN today,
Plaid or additional card connectors later; a hosted model or a local one for
categorization).

## Layout

    outflow/
      core/            domain logic, storage, detection
      cli/             headless entrypoint (pull, categorize, report, subs, fix)
      app/             Tauri desktop frontend         (planned)
      examples/        a sample SimpleFIN response for --from-file

## Using the CLI

    cargo run -p outflow-cli -- --db out.db pull --from-file examples/sample-accounts.json
    cargo run -p outflow-cli -- --db out.db categorize
    cargo run -p outflow-cli -- --db out.db fix <txn_id> Streaming
    cargo run -p outflow-cli -- --db out.db report --by category
    cargo run -p outflow-cli -- --db out.db report --by merchant --top 10
    cargo run -p outflow-cli -- --db out.db report --by monthly --since 2024-01-01
    cargo run -p outflow-cli -- --db out.db subs

The database path also reads from `OUTFLOW_DB`. Reports accept `--since` /
`--until` (YYYY-MM-DD, since-inclusive, until-exclusive) and `--posted-only`.

### Feature flags

The dev build has no network, keychain, or encryption, so the whole pipeline is
exercised via `pull --from-file`. For real use on a Mac:

    cargo build --release --features "net,keychain,encryption"

- `net`        live `pull` fetches the SimpleFIN access URL's `/accounts`
- `keychain`   reads the access URL from the OS keychain (else `OUTFLOW_SFIN_URL`)
- `encryption` opens the SQLite database with SQLCipher

## Status

Implemented in `core`:

- `money`         integer-cents money type, string parsing, no floating point
- `model`         accounts, transactions, category source
- `store`         SQLite persistence; id-keyed upsert for posted transactions,
                  delete-and-replace for pending (avoids pending/posted
                  double-counting); rule storage and categorization passes
- `source`        SimpleFIN parser behind the `TransactionSource` port; boundary
                  parsing that normalizes sign, currency, and decimal amounts
- `categorize`    `Categorizer` port and a deterministic rule engine (exact and
                  contains matching, longest-match precedence); manual
                  corrections write rules that then catch sibling transactions
- `query`         analytics: spend by category, merchant leaderboard, monthly
                  inflow/outflow, with date-range and pending filters
- `subscriptions` recurring-charge detection over accumulated history

`cli` wires these into a headless tool. The live SimpleFIN HTTP call, keychain
read, and SQLCipher open are behind feature flags (above).

Not yet built: the model-backed categorizer fallback, the Tauri GUI.

Known limitations: with only a checking account and no transfer detection,
transfers (to savings, credit-card payments) count as outflow, so spending
totals overstate consumption until transfer classification and card accounts are
added. Monthly buckets are UTC.

## Building

Requires a current stable Rust toolchain (via rustup). Cargo isolates all
dependencies into `target/`; nothing is installed system-wide.

    cargo test        # run the domain test suite
    cargo build       # compile the workspace

## Notes on data

- Money is stored as integer minor units (cents); never floats.
- Transaction amount sign convention: outflows are negative.
- SimpleFIN provides roughly 90 days of history per pull. The local database is
  the durable archive and accumulates indefinitely, so trend and subscription
  detection improve over time. Annual subscriptions are only detectable once the
  database holds more than a year of history.
