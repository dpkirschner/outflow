# ADR 0001: Product direction (Stock, Flow, Find money)

- Status: accepted
- Date: 2026-11-06

## Context

outflow started as a spending analyzer ("where did my money go?") and
grew a net-worth side: balances, holdings, investment activity, and
loans and HSAs treated as saving. The UI grew one tab per feature. As a
result:

- Spending had two unrelated views: a merchant-rhythm "ledger" and a
  month/category "outflows" table. Each had its own window, filters and
  recurring detector.
- Upkeep actions, including the destructive reset, sat on the home
  screen. Match review had a top-level tab.
- Some figures existed only in the browser, so an agent reading the API
  couldn't answer questions about what the screen showed.

The product has two equal clients: a person looking at the web UI, and
an agent (through the read-only API token or the CLI) that the person
talks the data over with. The main use is a periodic check-in plus
watching net worth and individual assets. One-off lookups ("how much at
merchant X?") mostly go to the agent.

## Decision

**Three pillars:**

1. **Stock**: where the money is, in what form, and how it has
   performed.
2. **Flow**: when money enters and leaves, including repeat purchases
   and how they change over time.
3. **Find money**: card offers, card choice, unused subscriptions.
   Designed in its own session; see the 2026-11-09 amendment.

**No budgets.** No limits or targets. Norms the app derives itself
(usual monthly cost, unusual charges, idle cash) are fine.

**Five screens:**

| Screen | Purpose |
|---|---|
| Check-in (home) | Period summary: month by default, week or quarter. Net worth change, earned − spent alongside saved, and what's unusual |
| Holdings | Stock: accounts by form, gain net of contributions, unrealized gain per position, net worth change over a range |
| Flow | In / Out / Moved lanes, the transaction table, and the recurring list |
| Find money | Every open Finding, ranked by estimated dollars per year (2026-11-09 amendment) |
| Settings | Connections, match review, categorize, reset |

**Flow lanes.** Transfers are no longer hidden. They form a **Moved**
lane split into card payment, invest, save, loan paydown, and between
own accounts. Moved is classified from Plaid's detailed
personal-finance category plus user rules. General two-sided transfer
matching is out of scope; only card payments keep the existing pair
matcher. Refunds net against Out rather than counting as In.

**Performance** is gain net of contributions, with unrealized gain per
position. Time-weighted returns can come later if benchmark comparison
is wanted.

**Recurring** replaces the ledger: one list of recurring merchants
(cadence, monthly cost, trend), one detector (`detect_rhythms`), and one
mark (Ignored). The ledger zones, noise promotion, the Committed and
Kept marks, `subscriptions::detect`, `/subscriptions`, `/stream_preview`
and `/mark_stream` are deleted, not deprecated. There is a single user
and no compatibility to keep.

**Agent access is a habit, not a rule.** If a number answers a question
you'd ask the agent, compute it in `core` and expose it through the API
and CLI. Chart scaffolding (axes, scaling, sort order) can stay in the
browser. Nothing enforces this mechanically.

## Build order

Each step ships on its own:

1. Settings: move upkeep off the main screens.
2. Flow lanes in core, API and CLI.
3. Platform step (added 2026-11-07, see the amendment below).
4. Recurring view; delete the ledger.
5. Holdings performance plus a net-worth-change endpoint.
6. Check-in, built from 2–5.

## Amendment 2026-11-07: platform step before the screens

A technical review after steps 1 and 2 found that the architecture
holds, but the seams the UI and the agent share get dearer with every
screen. Steps 4–6 therefore wait on a short platform step (beads
`ho-2mj.8`, one child each). None of it changes product behaviour.

- **Generated types and a contract test.** Every figure touches six
  hand-mirrored places (core function, route, CLI printer, CLI remote
  dispatch, `api.ts`, `types.ts`) with nothing checking they agree.
  `types.ts` is generated from the serde structs and one test pins the
  wire shapes.
- **One `Period` type.** The API carries four date conventions
  (`window` strings with 30.5-day months, epoch-second `since`/`until`,
  `YYYY-MM-DD`, `lookback`). Check-in needs calendar week, month and
  quarter and Holdings needs a range, so `core` gets one calendar-aligned
  `Period` in local time, one query-param spelling, and every new
  endpoint takes it.
- **Plaid's category becomes columns.** `personal_finance_category`
  lives only inside the stored raw JSON; the matcher searches it as a
  substring and two migrations have re-parsed it. `pfc_primary` and
  `pfc_detailed` are backfilled once, and lane assignment becomes a pure
  function of columns, account kind and user flag rules. The user flag
  stays the override axis only.
- **In comes from provider data, not the Income category.** Category
  remains the user's vocabulary; a missing rule no longer moves a
  paycheck between In and Out.
- **URL routing** per screen, before five screens are built on lifted
  state. Whether a screen's period goes in the URL is still open.
- Docs drift from the review is fixed in the same step.

Deferred, not blocking: loading the archive once per request when
Check-in calls several aggregators, and typed domain errors instead of
every failure being a 500.

## Amendment 2026-11-09: Find money

A design session (bead `ho-2mj.6`) settled the third pillar. Before
it, card offers existed only as `perks`: an offers file kept by hand,
matched against spending, reachable through `/perks` and the CLI, and
not configured on the live server.

- **One Finding type.** A new `core` module, `find`, owns `Finding`:
  a kind, an estimate in dollars per year (annualized from the last 12
  months, with points kept alongside), the transactions behind it, one
  suggested action, and a stable key. `perks` becomes one source of
  Findings among several. Estimates under $25 a year are dropped (a
  constant in `core`).
- **Three kinds, built in this order:** card offers (add an offer,
  wrong card, expiring), then card choice, then subscriptions.
- **Card choice** comes from an earn table in each card's offers file,
  keyed on Plaid's category (`pfc_detailed` overrides `pfc_primary`,
  plus a `default`), with a cents-per-point value and optional yearly
  caps. Rotating quarterly categories are modelled only if a card you
  hold has them. For each category, the gap is the last 12 months of Out
  on each card times the best held card's rate minus the rate earned.
  Cards without an earn table are left out, and the app says so. Annual
  fees and sign-up bonuses are out of scope: they are about which card
  to hold, not where to put a charge.
- **Subscriptions.** outflow sees charges, not usage, so it never claims
  a subscription is unused. It lists **steady** recurring merchants
  (monthly or yearly cadence, recent charges within 5% of their median)
  for review, sorted by yearly cost, and flags price rises. A new mark,
  **Reviewed**, hides a merchant for 12 months or until it goes Up. This
  is new, not the deleted Kept mark coming back.
- **Dismissed** is the one mark on a Finding, keyed by kind, merchant or
  category, and card. It moves the Finding to a list you can restore
  from, and the Finding comes back on its own only if its estimate more
  than doubles.
- **Check-in** shows a Finding as one more kind of unusual item when it
  is new this period: it has a qualifying charge in the period (a
  wrong-card charge, a price rise), its offer started in the period, or
  its offer expires within `expiring_days`. This is derived from the
  evidence; nothing records when a Finding was first seen. The Find
  money screen holds the full list.
- **API and CLI.** `GET /find` and `outflow find` return every kind and
  take the standard `Period` plus `expiring_days`. `/perks` and the
  `perks` command are deleted, not deprecated.
- **Offers are kept by hand** in a folder of per-card TOML files in the
  data directory, named by `OUTFLOW_PERKS`. outflow gets no write
  endpoint for offers, so the read-only token stays read-only. Having an
  agent draft the files is an idea for later.
- A **Subscriptions** filter on the Flow screen's Recurring list reuses
  the steady test for a plain "show my subscriptions" view.

## Consequences

- The "where did my money go?" pitch widens to "where your money is and
  where it's going."
- The transfer flag needs a finer classification (the Moved groups).
  Step 2 widened `TxnFlag`; the platform step adds the Plaid category
  columns so later reclassifications need no data migration.
- Performance depends on history: snapshots start at the first holdings
  sync, and Plaid returns at most about 24 months of investment activity.
  Older cost-basis history needs a brokerage import (still on the
  roadmap).
- Known agent gaps get fixed along the way: net worth change over a
  range, `balances --history --since`, and CLI access to the recurring
  list.
