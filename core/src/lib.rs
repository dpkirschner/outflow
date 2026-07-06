pub mod categorize;
pub mod ledger;
pub mod llm;
pub mod model;
pub mod money;
pub mod query;
pub mod source;
pub mod store;
pub mod subscriptions;

pub use categorize::{Categorizer, CategoryRule, MatchType, RuleSet};
pub use llm::{LlmCategorizer, LlmError, MerchantSample, Prompter, Suggestion};
pub use model::{Account, AccountKind, CategorySource, Mark, Transaction, TxnFlag};
pub use money::{Money, MoneyParseError};
pub use query::{
    monthly_flow, spend_by_category, top_merchants, CategorySpend, MerchantSpend, MonthlyFlow,
    TxnFilter,
};
pub use ledger::{
    ledger, Coverage, LedgerStats, LedgerView, LineItem, Source, SourceKind, Stream, TransferGroup,
};
pub use source::{parse_account_set, Fetched, SourceError, TransactionSource};
pub use store::{FlagRule, Store, UpsertResult};
pub use subscriptions::{
    detect, detect_rhythms, normalize_payee, Cadence, RhythmEntry, StreamCadence, Subscription,
    Trend,
};
