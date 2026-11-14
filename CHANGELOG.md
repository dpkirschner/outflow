# Changelog

Every deploy of outflow is a release, versioned `vYYYY.MM.N` (N counts
from 0 each month). Each section is drafted from commit subjects by
git-cliff (`cliff.toml`) through `scripts/release.py`; big releases open
with a short summary. See `docs/RELEASING.md`.

## v2026.11.4 (2026-11-13)

The Holdings and Check-in hero no longer lumps everything that isn't a
market move into one "Added" figure that read like deposits. Net worth
change now splits into Markets, Moved in (shares transferred between
accounts), Earned - spent (from Flow) and Other, so the hero never
claims money was added when it wasn't. The CLI's `--json` output now
follows the agent-wrapper conventions.

### Features

- holdings: Split the hero into Markets, Moved in, Earned - spent and Other; share transfers show as Moved in instead of a market loss (rule F dropped), and each amount stays on one line with its label and sign (ho-2mj.25) ([272ac80](https://github.com/dpkirschner/outflow/commit/272ac80), [b200fd9](https://github.com/dpkirschner/outflow/commit/b200fd9))
- cli: Agent-wrapper conventions: `--json` before or after the subcommand prints one `{ok, ...}` or `{ok: false, error: {code, message}}` object, with typed exit codes (2 usage, 3 not found, ...) (ho-8ti) ([4f1a09e](https://github.com/dpkirschner/outflow/commit/4f1a09e))

## v2026.11.3 (2026-11-12)

Find money arrives: a new screen (and `GET /find`, `outflow find`) that
lists card offers worth using, which card earns most on each kind of
spending, and subscriptions to review, each with Dismissed or Reviewed
marks that stick. New findings show up in Check-in. Flow gets margin
notes on unusual categories, an In lane by payee, a month strip that
plots the picked figure, and a Subscriptions filter on Recurring, which
now also catches yearly charges.

Schema bump (v17: finding and review marks). Rolling back past this
release needs `scripts/deploy.sh rollback --restore-db`. Card offers
need `OUTFLOW_PERKS`; without it Find money says so and lists the rest.

### Features

- find: Find money module, `GET /find` and `outflow find`; `/perks` is gone (ho-2mj.13, ho-2mj.22) ([cb7a5ce](https://github.com/dpkirschner/outflow/commit/cb7a5ce), [d4d0493](https://github.com/dpkirschner/outflow/commit/d4d0493))
- find: Dismissed mark on a Finding (ho-2mj.15) ([e005f36](https://github.com/dpkirschner/outflow/commit/e005f36))
- find: Card choice from per-card earn tables (ho-2mj.16) ([7613b76](https://github.com/dpkirschner/outflow/commit/7613b76))
- find: Subscription review list and Reviewed mark (ho-2mj.17) ([5766b2a](https://github.com/dpkirschner/outflow/commit/5766b2a))
- find: Optional offer start date (ho-2mj.26) ([f86c5da](https://github.com/dpkirschner/outflow/commit/f86c5da))
- find: `GET /find/charges` resolves Show the charges by key (ho-2mj.28) ([68fc764](https://github.com/dpkirschner/outflow/commit/68fc764))
- app: Find money screen (ho-2mj.19, ho-2mj.23) ([f0c1808](https://github.com/dpkirschner/outflow/commit/f0c1808), [e42a61e](https://github.com/dpkirschner/outflow/commit/e42a61e))
- checkin: New Findings as unusual items (ho-2mj.18) ([b0138d1](https://github.com/dpkirschner/outflow/commit/b0138d1))
- checkin: `expiring_days` on `/checkin` and `outflow checkin` (ho-2mj.27) ([bfa658b](https://github.com/dpkirschner/outflow/commit/bfa658b))
- flow: Margin notes on categories far from their usual (ho-2mj.9) ([04fae5b](https://github.com/dpkirschner/outflow/commit/04fae5b))
- flow: The In lane lists its sources by payee (ho-2mj.9) ([576a577](https://github.com/dpkirschner/outflow/commit/576a577))
- flow: The month strip plots the picked figure per month (ho-2mj.9) ([f222911](https://github.com/dpkirschner/outflow/commit/f222911))
- flow: Subscriptions filter on the Recurring list (ho-2mj.20) ([a5b3fc8](https://github.com/dpkirschner/outflow/commit/a5b3fc8))
- recurring: Detect yearly merchants from the archive (ho-2mj.21) ([d2be82d](https://github.com/dpkirschner/outflow/commit/d2be82d))

### Fixes

- deploy: Build with OUTFLOW_FEATURES= on macOS bash 3.2 ([0451b28](https://github.com/dpkirschner/outflow/commit/0451b28))

### Docs

- Find money in the glossary and PRODUCT.md; ADR 0001 retired now that the reshape is built (ho-2mj.12) ([08367ca](https://github.com/dpkirschner/outflow/commit/08367ca), [a57225b](https://github.com/dpkirschner/outflow/commit/a57225b))
- CLAUDE.md points at docs/ instead of repeating the CLI reference (ho-cf3) ([8ecedbd](https://github.com/dpkirschner/outflow/commit/8ecedbd))

## v2026.11.2 (2026-11-09)

### Features

- `outflow backup --to PATH`: a verified snapshot of the DB, still encrypted with the same key, safe while the server runs. The CLI also reads the DB key from `OUTFLOW_DB_KEY_FILE` now (ho-fm2.2) ([5c8b3b6](https://github.com/dpkirschner/outflow/commit/5c8b3b6))

### Docs

- Point DEVELOPMENT.md and the README at releases (ho-fpd.5) ([376216a](https://github.com/dpkirschner/outflow/commit/376216a))

## v2026.11.1 (2026-11-08)

Deploys now install a published release into its own folder and run it
from there, never from a working copy: `scripts/deploy.sh build`,
`install`, `rollback` and `status` (see `docs/DEPLOYMENT.md`). The
server reports which release it is at `GET /api/version`, in
`outflow status` and in `outflow --version`.

### Features

- Report the release a build came from (ho-fpd.4) ([c54810b](https://github.com/dpkirschner/outflow/commit/c54810b))
- Deploy releases by tag from their own folder (ho-fpd.4) ([f57b79c](https://github.com/dpkirschner/outflow/commit/f57b79c))

## v2026.11.0 (2026-11-08)

First tagged release: a baseline after the UI reshape merged into
`main`. Earlier history is in `git log`.

outflow is a single-user view of where one person's money is and where
it's going. A server pulls accounts, balances, holdings and transactions
from Plaid into local SQLite, categorizes them, and serves a web app and
a JSON API; the `outflow` CLI runs the same domain logic directly or
against the server.

What this release does:

- **Check-in** (home): one week, month or quarter. Net worth change,
  earned − spent and saved, each against its usual, plus what's unusual:
  new or changed recurring charges, large one-offs, connections and card
  payments that need a look.
- **Flow**: money in, out and moved by month, with exact drill-down to
  the transactions behind each figure, and a recurring list that knows
  each merchant's cadence (monthly, every few months, yearly).
- **Holdings**: positions per account with price, basis and gain, gain
  over a period with sold positions kept, and uninvested cash.
- **Balances and net worth**: cash + invested − credit − loans, with a
  daily history from per-sync snapshots and a split of each change into
  markets versus money added.
- **Upkeep**: card payments auto-matched to the bank side so spending
  isn't counted twice, with review, undo and per-account type overrides.
- **Repo tooling**: `make leakscan` and a pre-push/CI scan keep personal
  data out of this public repo; `make release` cuts releases like this
  one.
