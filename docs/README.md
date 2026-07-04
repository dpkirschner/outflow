# outflow — docs

Durable reference for the codebase, meant to get a new contributor (human or
agent) oriented fast without re-deriving everything. These files track **stable**
knowledge — architecture, data model, invariants, commands, gotchas. Volatile
per-session status and the running build spec live in the untracked `agent.md`
at the repo root.

## What this project is

A local, single-user spending analyzer for one person's own bank data. Pulls
transactions via SimpleFIN, stores them in local SQLite, categorizes them, and
answers "where did my money go?", "how much at merchant X?", and "what
subscriptions am I paying for?". Analysis only — no budgets or targets. Ships as
a headless CLI and a double-click macOS `.app`, both over one shared domain core.

## Map

| Doc | Read it for |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | Crate layout, ports-and-adapters, layering, data flow, config resolution, feature flags |
| [DATA_MODEL.md](DATA_MODEL.md) | Money convention, domain types, SQLite schema, the write invariants |
| [INVARIANTS.md](INVARIANTS.md) | The non-negotiables tests and correctness depend on |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Build/test/run/bundle commands, feature matrix, dev-vs-prod, agent constraints |
| [GOTCHAS.md](GOTCHAS.md) | Hard-won lessons: macOS/keychain/Gatekeeper, encryption switching, data contamination |

## One-paragraph architecture

Ports-and-adapters over a pure domain `core`, in one Cargo workspace. `core` has
zero GUI/network deps by default. The two volatile externals — the transaction
**source** and the **categorizer** — are traits, so they can be swapped
(SimpleFIN→Plaid, rules→LLM) without touching the domain. Networked adapters live
in `net`. Two front-ends, `cli` and `app/src-tauri`, depend on `core` directly
and share `net`, so they run identical logic. See ARCHITECTURE.md.
