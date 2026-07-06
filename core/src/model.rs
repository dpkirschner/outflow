use crate::money::Money;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccountKind {
    Checking,
    Savings,
    Credit,
    Other,
}

impl AccountKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AccountKind::Checking => "checking",
            AccountKind::Savings => "savings",
            AccountKind::Credit => "credit",
            AccountKind::Other => "other",
        }
    }

    pub fn from_str(s: &str) -> AccountKind {
        match s {
            "checking" => AccountKind::Checking,
            "savings" => AccountKind::Savings,
            "credit" => AccountKind::Credit,
            _ => AccountKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub org: String,
    pub name: String,
    pub kind: AccountKind,
    pub balance: Money,
    pub currency: String,
    pub last_synced: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CategorySource {
    SimpleFin,
    Rule,
    Llm,
    Manual,
}

impl CategorySource {
    pub fn as_str(self) -> &'static str {
        match self {
            CategorySource::SimpleFin => "simplefin",
            CategorySource::Rule => "rule",
            CategorySource::Llm => "llm",
            CategorySource::Manual => "manual",
        }
    }

    pub fn from_str(s: &str) -> Option<CategorySource> {
        match s {
            "simplefin" => Some(CategorySource::SimpleFin),
            "rule" => Some(CategorySource::Rule),
            "llm" => Some(CategorySource::Llm),
            "manual" => Some(CategorySource::Manual),
            _ => None,
        }
    }
}

/// A per-merchant override the user applies from the ledger slide-over. All three
/// are sticky, keyed on the normalized merchant so they apply to siblings:
/// `Committed` = a fixed obligation (kept out of the "hunt"); `Dismissed` = not a
/// real stream (its spend falls to Notable/Noise); `Kept` = force back into the
/// main streams list, overriding a committed-by-category classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mark {
    Committed,
    Dismissed,
    Kept,
}

impl Mark {
    pub fn as_str(self) -> &'static str {
        match self {
            Mark::Committed => "committed",
            Mark::Dismissed => "dismissed",
            Mark::Kept => "kept",
        }
    }

    pub fn from_str(s: &str) -> Option<Mark> {
        match s {
            "committed" => Some(Mark::Committed),
            "dismissed" => Some(Mark::Dismissed),
            "kept" => Some(Mark::Kept),
            _ => None,
        }
    }
}

/// How a transaction is treated by spend analytics. `Spending` is real
/// consumption and the default; `Transfer` (money moved between the user's own
/// accounts) and `CardPayment` (a checking→card payment whose underlying charges
/// are the real spend) are suppressed from all aggregations so they aren't
/// double-counted. See `query` and `subscriptions::detect`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TxnFlag {
    Spending,
    Transfer,
    CardPayment,
}

impl TxnFlag {
    pub fn as_str(self) -> &'static str {
        match self {
            TxnFlag::Spending => "spending",
            TxnFlag::Transfer => "transfer",
            TxnFlag::CardPayment => "card_payment",
        }
    }

    pub fn from_str(s: &str) -> Option<TxnFlag> {
        match s {
            "spending" => Some(TxnFlag::Spending),
            "transfer" => Some(TxnFlag::Transfer),
            "card_payment" => Some(TxnFlag::CardPayment),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transaction {
    pub id: String,
    pub account_id: String,
    pub posted: i64,
    /// When the purchase actually happened (SimpleFIN `transacted_at`), if the
    /// bank supplied it. `None` when only the posting date is known.
    pub transacted_at: Option<i64>,
    pub amount: Money,
    pub description: String,
    pub payee: Option<String>,
    pub category: Option<String>,
    pub category_source: Option<CategorySource>,
    pub pending: bool,
    pub flag: TxnFlag,
    pub raw: String,
}

impl Transaction {
    pub fn merchant(&self) -> &str {
        match &self.payee {
            Some(p) if !p.is_empty() => p,
            _ => &self.description,
        }
    }

    /// Behavioral basis date: when the purchase happened, falling back to the
    /// posting date. All month bucketing and cadence math keys off this so
    /// analytics reflect behavior, not bookkeeping.
    pub fn effective_date(&self) -> i64 {
        self.transacted_at.unwrap_or(self.posted)
    }
}
