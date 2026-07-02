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
      core/            domain logic, storage, detection (this is the real code)
      cli/             headless entrypoint            (planned)
      app/             Tauri desktop frontend         (planned)

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
- `subscriptions` recurring-charge detection over accumulated history

Not yet built: the SimpleFIN HTTP client (lands in the CLI, where network is
available), the model-backed categorizer fallback, analytics queries for the
dashboards, encryption-at-rest, keychain integration, CLI, GUI.

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
