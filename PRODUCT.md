# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

One person, viewing their own money. They open outflow for a periodic
check-in (monthly by default) and to watch net worth and individual
assets over time. The check-in usually happens at a desktop browser;
the phone is for an occasional glance.

The second, equal client is an agent the person talks the data over
with. It reads the same numbers through the read-only API token or the
CLI. One-off lookups ("how much did I spend at merchant X?") mostly go
to the agent rather than the screen.

## Product Purpose

Show where the money is and where it's going. Three pillars (see
`GLOSSARY.md`):

- **Stock**: balances, holdings and how they performed (gain net of
  contributions, unrealized gain per position, net worth change).
- **Flow**: money in, out and moved between own accounts, plus the
  merchants bought from on a cadence.
- **Find money**: card offers, card choice, and subscriptions worth
  reviewing, each a Finding with an estimate per year.

Success: a check-in answers "what changed, and is anything unusual?"
in a few minutes, and every number on screen can also be asked of the
agent.

## Positioning

Self-hosted and single-user. The data lives in a local SQLite archive
that grows past the bank aggregator's pull windows, so history is
permanent. Analysis only: no budgets, limits or targets. Norms the app
derives itself (usual monthly cost, unusual charges, idle cash) are
fine. Transfers between own accounts are shown as their own lane, never
hidden and never counted as spending or income. Moved is classified
from Plaid's category and the user's flag rules; only card payments are
paired with their other leg (general two-sided transfer matching is out
of scope). Performance is gain net of contributions, not a
time-weighted return.

## Operating Context

- Served by an always-on home server over a private network, opened in
  a browser. Data arrives from Plaid on a background sync (every few
  hours) or a manual pull.
- Screens: Check-in (home), Holdings, Flow (with the Recurring list),
  Find money, Settings.
- Find money lists every open Finding, highest estimate a year first,
  over the 12 months ending with the selected month (stepped like
  Check-in's weeks). Each row has its kind, the estimate (points beside
  the cash value they're counted at), core's one suggested action, the
  evidence in words (offer terms and days left, each card's rate and
  spend for card choice, a subscription's charge or price rise), and
  the charges behind it, opened in place. Dismiss moves a Finding to a
  folded dismissed list with Restore; a subscription or price rise can
  also be marked Reviewed, into a folded reviewed list with Unreview.
  Both marks have Undo on the toast. Under the list, each card from the
  offers files is said in words (offers added, no earn table, not tied
  to an account), or that no offers are set up. A Check-in row for a
  Finding (or a recurring up row that stands in for a price rise) opens
  Find money on that Finding.
- Find money sees charges, not usage, so it never calls a subscription
  unused; it lists steady ones to review. Offers files are kept by hand
  (`OUTFLOW_PERKS`), with no write endpoint, so the read-only token
  stays read-only.
- Upkeep (bank connections, card-payment match review, categorize,
  reset) lives in Settings, away from the analysis screens.

## Capabilities and Constraints

- Money is integer cents end to end; amounts arrive at the browser as
  numbers of cents. Outflows are negative.
- Figures that answer a question you'd ask the agent are computed in the
  Rust core and exposed through the API and CLI. Chart scaffolding
  (axes, scaling, sort order) may stay in the browser.
- Frontend: React + Vite + TypeScript in `app/`, plain CSS in
  `app/src/theme.css` and `app/src/ledger.css`. No CSS framework.
- Vocabulary is fixed by `GLOSSARY.md` (In / Out / Moved, Recurring
  merchant, Ignored, Unusual, Gain net of contributions, Idle cash, Finding).
  Name things the way it does.

## Brand Commitments

None binding. The name, the lowercase wordmark and the tagline are all
open.

## Evidence on Hand

- Synthetic sample data only: `examples/plaid-fixture.json`. Real
  account data never appears in this repo, its screenshots or its docs.
  Any sample or demo data is invented and labelled synthetic.

## Product Principles

1. **Answers over dashboards.** Each screen answers a question the
   person actually asks during a check-in. Nothing is on screen just
   because the data exists.
2. **Same numbers for the person and the agent.** The screen never shows
   a figure the API can't produce.
3. **Show, don't judge.** No budgets or targets. Point out what's unusual
   relative to the person's own history and leave the decision to them.
4. **Nothing hidden, nothing double-counted.** Transfers and card
   payments stay visible in their own lane; they never inflate spending.
5. **Upkeep stays out of the way.** Destructive or maintenance actions
   never sit on an analysis screen.

## Accessibility & Inclusion

General good practice: WCAG AA contrast, and money in vs out is never
signalled by colour alone (sign, label or position carries it too).
