# Changelog

Every deploy of outflow is a release, versioned `vYYYY.MM.N` (N counts
from 0 each month). Each section is drafted from commit subjects by
git-cliff (`cliff.toml`) through `scripts/release.py`; big releases open
with a short summary. See `docs/RELEASING.md`.

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
