pub mod balances;
pub mod categorize;
pub mod ledger;
pub mod llm;
pub mod model;
pub mod money;
pub mod perks;
pub mod plaid;
pub mod query;
pub mod store;
pub mod subscriptions;
pub mod transfers;

pub use balances::{
    account_cash, history as balance_history, position_history, positions,
    summarize as summarize_balances, AccountBalance, BalancesView, DayTotals, Group, GroupBalance,
    HoldingRow, PositionFilter, PositionPoint, PositionRow, Totals,
};
pub use categorize::{Categorizer, CategoryRule, MatchType, RuleSet};
pub use ledger::{
    ledger, stream_preview, Coverage, LedgerStats, LedgerView, LineItem, Source, Stream,
    TransferGroup,
};
pub use llm::{LlmCategorizer, LlmError, MerchantSample, Prompter, Suggestion};
pub use model::{
    account_display_label, Account, AccountKind, BalanceSnapshot, CategorySource, Holding,
    HoldingSnapshot, Mark, MatchConfidence, MatchStatus, PlaidItem, Security, SyncEntry,
    Transaction, TxnFlag, TxnMatch,
};
pub use money::{Money, MoneyParseError};
pub use perks::{
    evaluate as evaluate_perks, load_offers, parse_offers, Cap, CardOffers, CardStatus, Finding,
    FindingKind, Offer, PerkOptions, PerksError, PerksReport, Reward,
};
pub use plaid::{
    item_has_investments_consent, parse_accounts_get, parse_fixture, parse_holdings_get,
    parse_sync_page, Fixture, SourceError, SyncPage,
};
pub use query::{
    monthly_flow, search_transactions, spend_by_category, top_merchants, CategorySpend,
    MerchantSpend, MonthlyFlow, SortKey, TxnFilter, TxnPage, TxnQuery,
};
pub use store::{FlagRule, PlaidBatch, Store, UpsertResult};
pub use subscriptions::{
    detect, detect_rhythms, normalize_payee, Cadence, RhythmEntry, StreamCadence, Subscription,
    Trend,
};
pub use transfers::{detect_card_payments, MatchOptions, ProposedMatch};
