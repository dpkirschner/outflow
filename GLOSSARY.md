# Glossary

Domain vocabulary for outflow. Name things the way this file does. The
product direction behind these terms is in
[docs/adr/0001-product-direction.md](docs/adr/0001-product-direction.md).

## Pillars

- **Stock**: what you have at a point in time. Accounts grouped by the
  form the money is in (cash, investments, credit, loans), positions
  inside investment accounts, and net worth. Answers "where is my
  money, and how has it performed?"
- **Flow**: money crossing an account boundary over a period. Split
  into the three lanes In, Out and Moved. Answers "when does money
  leave and enter?"
- **Find money**: things outflow can point out that would save or earn
  money, such as an unused card offer, a purchase on the wrong card, or
  a subscription worth reviewing. Each one is a **Finding**.

## Flow lanes

- **In**: income-type inflows from outside your own accounts, such as
  payroll, interest and dividends. A refund is **not** In. Whether an
  inflow is In comes from the bank's own category and the account type,
  never from the category you gave it, so a missing rule can't move a
  paycheck into Out.
  Flow lists In by **source**: the payee (employer, bank paying
  interest), since income mostly shares one category.
- **Out**: spending. Refunds and returns net against Out in their
  category instead of counting as In.
- **Moved**: money going between your own accounts. It is shown, never
  hidden, and never counted as spending or income. Every Moved
  transaction is in exactly one group:
  - **Card payment**: paying a credit card balance. It never counts as
    saving, because the card's purchases were already counted in Out.
  - **Invest**: to a brokerage, retirement account or HSA.
  - **Save**: to savings, a CD or a money market account.
  - **Loan paydown**: principal paid on a loan.
  - **Between own accounts**: checking to checking and anything not yet
    classified.
- **Earned − spent**: In minus Out for a period.
- **Saved**: Invest + Save + Loan paydown going out, minus money coming
  back from those accounts. It's shown next to earned − spent. The gap
  between the two is cash left sitting in checking.

## Stock terms

- **Net worth**: cash + invested − credit owed − loans, from the latest
  balance of each visible account.
- **Net worth change**: net worth at the end of a range minus the start,
  from per-sync balance snapshots.
- **Gain net of contributions**: value change of an account or position
  over a period, minus the money you put in and plus the money you took
  out. The main performance number. It answers "what did the market do
  for me?" in dollars.
- **Markets**: the part of net worth change the market made: gain net of
  contributions summed over the investment accounts that can be measured.
- **Added**: net worth change minus Markets. Money saved from income minus
  spending, plus the whole change of any investment account that can't be
  measured (it then says so). Contributions and loan paydown move money
  between your own accounts, so they are not part of the change.
- **Unrealized gain**: current value minus cost basis for a position.
- **Idle cash**: an investment account holding at least $1,000 and at
  least 5% of its value in cash.

## Recurring

- **Recurring merchant**: a merchant (keyed by
  `subscriptions::normalize_payee`) that you buy from on a detectable
  cadence: at least three charges, regularly spaced, within the 12 months
  ending with the month you're looking at. It shows how often, the usual
  monthly cost, and what changed. There is one detector
  (`subscriptions::detect_rhythms`). A fixed-amount subscription is just a
  recurring merchant with a steady amount.
- **Usual monthly cost**: the mean of a recurring merchant's completed
  months since its first charge in the window, with months that had no
  charge counted as $0.
- **Every few months**: a recurring merchant charged roughly every two to
  nine months (a bimonthly bill, a groomer). Months with no charge are
  normal for it, so it's compared **charge to charge**, never month to month.
- **New / Up / Down / Late**: the change marker. New means the first charge
  falls in the last three months, against older history. Up or Down means
  the last completed month is more than 25% away from the average of up to
  three months before it; for a merchant charged every few months, the
  latest charge is more than 25% away from its usual charge (the median of
  up to four before it). A month with no charge is never Down. Late means
  no charge for 1.5 times the usual gap (at least a month): skipped, moved
  or stopped. Otherwise it's steady, and nothing is shown.
- **Year on year**: what a recurring merchant cost over the last 12
  completed months against the 12 before, once the archive reaches back
  two years.
- **Ignored**: the one per-merchant mark. It moves the merchant off the
  recurring list into a separate ignored list, where it can be restored.

## Find money

- **Finding**: one thing that would save or earn money. It has a kind,
  an estimate in dollars per year, the transactions behind it, one
  suggested action, and a stable key. Findings under $25 a year are
  dropped. The kinds are:
  - **Add offer**: a card offer you haven't added, at a merchant you
    already pay.
  - **Wrong card**: an added offer whose merchant keeps getting charged
    to a different card.
  - **Expiring**: an added offer about to expire before it has paid out.
  - **Card choice**: a category charged to a card that earns less on it
    than another card you hold.
  - **Subscription**: a steady recurring merchant to review, valued at a
    year of its charge. outflow sees charges, not usage, so it never says
    unused.
  - **Price rise**: a steady merchant whose price just rose, valued at a
    year of the rise. It stands in for the merchant's subscription
    Finding while it lasts.
- **Offers file**: a hand-kept TOML file per card, listing its offers
  and its earn table. `OUTFLOW_PERKS` names the file or folder.
- **Earn table**: what a card pays per Plaid category (detailed category
  first, then primary, then `default`), with a cents-per-point value and
  optional yearly caps. A card without one is left out of card choice.
  Its cents-per-point value also prices that card's offer points; a
  card without one counts a point as one cent.
- **Steady**: a recurring merchant on a monthly or yearly cadence whose
  recent charges (the last three) are within 5% of their median. A steady
  merchant is what the Subscriptions filter shows. Its price rises when
  its latest charges sit more than 5% above a steady run before them; that
  lasts until three charges are at the new price.
- **Reviewed**: a mark on a steady merchant. It hides the subscription
  Finding for 12 months, or until the merchant goes Up: its charge is
  more than 5% above the one reviewed. A price rise after a review always
  shows. Separate from Ignored, which hides the merchant everywhere.
- **Dismissed**: the mark on a Finding. It moves the Finding to a list
  you can restore from. The Finding comes back on its own only if its
  estimate more than doubles.

## Screens

- **Check-in**: the home screen. It summarizes a period (month by
  default, or week or quarter): net worth change, earned − spent, saved,
  and anything **unusual**.
- **Unusual**: one of: a new recurring merchant; a recurring merchant
  whose monthly cost (or, every few months, its latest charge) is more
  than 25% away from its usual; a recurring merchant that went late; a one-off
  charge that is large compared with your own spending (around your 95th
  percentile); or a connection or match review that needs attention.
  The thresholds are constants in `core`. Items about a place you visit
  every few weeks, or changes under $20 a month, are **minor**: still
  listed, folded at the end.
- **Usual**: the middle value (median) of up to 12 periods of the same kind
  before this one. While a period is in progress, each earlier period is
  cut to the same day (**usual to date**), so Nov 1–8 reads against Oct
  1–8. The check-in compares its headline figure (earned − spent; spent,
  for a week) with its usual and, when it's notably worse, names the
  unusual items behind it. A big difference that's within how much those
  periods swing (a rent week) is said, but not called unusual. A month check-in also judges every finished
  month the same way, so the month strip can mark the ones well below
  their usual.
- **Category change**: a category's Out in a finished period at least
  twice its usual or at most half of it, and at least $50 away. Flow says
  it under the category ("about 3× usual · usually $60", "down 70%").
  Uncategorized spend is never judged; it swings with how much is
  categorized.
- **Holdings**: the Stock screen.
- **Find money**: every open Finding, ranked by estimated dollars per
  year, with a dismissed list.
- **Flow**: the In / Out / Moved screen, with the transaction table and
  the recurring list.
- **Settings**: upkeep. Bank connections, card-payment match review,
  categorize, reset.

## Plumbing terms

- **Card-payment match**: a checking outflow paired with the
  equal-magnitude credit-card inflow it paid. High-confidence pairs are
  auto-accepted; medium ones wait for review in Settings.
- **Flag**: the lane axis on a transaction, separate from its category.
  `Spending` is In / Out; every other value is a Moved group: `CardPayment`,
  `Invest`, `Save`, `LoanPaydown`, and `Transfer` (between own accounts, or
  moved with no finer group known yet).
