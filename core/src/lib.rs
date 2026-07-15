pub mod categorize;
pub mod ledger;
pub mod llm;
pub mod model;
pub mod money;
pub mod plaid;
pub mod query;
pub mod store;
pub mod subscriptions;
pub mod transfers;

pub use categorize::{Categorizer, CategoryRule, MatchType, RuleSet};
pub use llm::{LlmCategorizer, LlmError, MerchantSample, Prompter, Suggestion};
pub use model::{
    Account, AccountKind, CategorySource, Mark, MatchConfidence, MatchStatus, PlaidItem,
    SyncEntry, Transaction, TxnFlag, TxnMatch,
};
pub use money::{Money, MoneyParseError};
pub use plaid::{parse_accounts_get, parse_fixture, parse_sync_page, Fixture, SourceError, SyncPage};
pub use query::{
    monthly_flow, search_transactions, spend_by_category, top_merchants, CategorySpend,
    MerchantSpend, MonthlyFlow, SortKey, TxnFilter, TxnPage, TxnQuery,
};
pub use ledger::{
    ledger, stream_preview, Coverage, LedgerStats, LedgerView, LineItem, Source, SourceKind,
    Stream, TransferGroup,
};
pub use store::{FlagRule, PlaidBatch, Store, UpsertResult};
pub use subscriptions::{
    detect, detect_rhythms, normalize_payee, Cadence, RhythmEntry, StreamCadence, Subscription,
    Trend,
};
pub use transfers::{detect_card_payments, MatchOptions, ProposedMatch};
