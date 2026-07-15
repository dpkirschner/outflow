# outflow — docs

Durable reference for the codebase, meant to get a new contributor (human or
agent) oriented fast without re-deriving everything. These files track **stable**
knowledge — architecture, data model, invariants, commands, gotchas. Volatile
per-session status and the running build spec live in the untracked `agent.md`
at the repo root.

## What this project is

A single-user spending analyzer for one person's own bank data. A server
(target: an always-on mac-mini on a tailnet) pulls transactions from **Plaid**
(checking/savings/credit cards) and **SimpleFIN** into local SQLite,
categorizes them, auto-detects checking→card payments so nothing double-counts,
and answers "where did my money go?", "how much at merchant X?", and "what
subscriptions am I paying for?". Analysis only — no budgets or targets. Ships
as a web app served by the server plus a headless CLI (direct-DB or HTTP client
mode), all over one shared domain core.

## Map

| Doc | Read it for |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crate layout, ports-and-adapters, layering, data flow, config resolution, feature flags |
| [DATA_MODEL.md](DATA_MODEL.md) | Money convention, domain types, SQLite schema, the write invariants |
| [INVARIANTS.md](INVARIANTS.md) | The non-negotiables tests and correctness depend on |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Build/test/run commands, feature matrix, dev-vs-prod |
| [DEPLOYMENT.md](DEPLOYMENT.md) | mac-mini setup: Plaid dashboard, tailscale serve, launchd, secrets files |
| [GOTCHAS.md](GOTCHAS.md) | Hard-won lessons: OAuth redirects, headless secrets, encryption switching, data contamination |

## One-paragraph architecture

Ports-and-adapters over a pure domain `core`, in one Cargo workspace. `core`
has zero GUI/network deps by default. Transaction sources follow one shape —
transport in `net`, pure JSON→domain parsing in `core` (`parse_account_set` for
SimpleFIN, `plaid::parse_sync_page`/`parse_accounts_get` for Plaid) — and the
categorizer is a trait (rules today, LLM tail). Two front-ends, the axum
`server` (which also serves the React SPA) and the `cli`, depend on `core`
directly and share `net`, so they run identical logic. See ARCHITECTURE.md.
