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
  a subscription nobody uses. Each one is a **Finding**. Not built yet.

## Flow lanes

- **In**: income-type inflows from outside your own accounts, such as
  payroll, interest and dividends. A refund is **not** In. Whether an
  inflow is In comes from the bank's own category and the account type,
  never from the category you gave it, so a missing rule can't move a
  paycheck into Out.
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
- **New / Up / Down**: the change marker. New means the first charge falls
  in the last three months, against older history. Up or Down means the
  last completed month is more than 25% away from the average of up to
  three months before it. Otherwise it's steady, and nothing is shown.
- **Ignored**: the one per-merchant mark. It moves the merchant off the
  recurring list into a separate ignored list, where it can be restored.

## Screens

- **Check-in**: the home screen. It summarizes a period (month by
  default, or week or quarter): net worth change, earned − spent, saved,
  and anything **unusual**.
- **Unusual**: one of: a new recurring merchant; a recurring merchant
  whose monthly cost is more than 25% away from its usual; a one-off
  charge that is large compared with your own spending (around your 95th
  percentile); or a connection or match review that needs attention.
  The thresholds are constants in `core`.
- **Holdings**: the Stock screen.
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
