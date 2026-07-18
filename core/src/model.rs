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

/// Which upstream integration a row came from ("plaid" today; rows from the
/// retired SimpleFIN era carry "simplefin"). Ids are stored raw — provenance
/// lives here, not in id prefixes.
pub fn default_source() -> String {
    "plaid".to_string()
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
    #[serde(default = "default_source")]
    pub source: String,
    /// User-editable nickname. `None` = show the raw `name`. Set via
    /// `Store::set_account_nickname`; deliberately preserved across re-sync
    /// (omitted from the upsert conflict-update, like `Transaction::flag`).
    #[serde(default)]
    pub nickname: Option<String>,
}

/// Display label for an account chip: the user's nickname (when set) with the
/// auto last-4 mask preserved. `plaid::parse_accounts_get` bakes the mask onto
/// `name` as a trailing `" (1234)"`; when a nickname is set we strip that suffix
/// off `name` and re-attach it to the nickname. No nickname → the raw `name`.
/// Mirrored in the frontend as `accountDisplayLabel` (app/src/components/ledger/labels.ts).
pub fn account_display_label(name: &str, nickname: Option<&str>) -> String {
    match nickname.map(str::trim).filter(|s| !s.is_empty()) {
        None => name.to_string(),
        Some(nick) => match mask_suffix(name) {
            Some(mask) => format!("{nick} {mask}"),
            None => nick.to_string(),
        },
    }
}

/// The trailing `"(1234)"`-style suffix `plaid.rs` appends to an account name,
/// if present. Strips only the last parenthesized group.
fn mask_suffix(name: &str) -> Option<&str> {
    let name = name.trim_end();
    if !name.ends_with(')') {
        return None;
    }
    let open = name.rfind(" (")?;
    Some(name[open + 1..].trim_end())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CategorySource {
    Plaid,
    Rule,
    Llm,
    Manual,
}

impl CategorySource {
    pub fn as_str(self) -> &'static str {
        match self {
            CategorySource::Plaid => "plaid",
            CategorySource::Rule => "rule",
            CategorySource::Llm => "llm",
            CategorySource::Manual => "manual",
        }
    }

    pub fn from_str(s: &str) -> Option<CategorySource> {
        match s {
            "plaid" => Some(CategorySource::Plaid),
            "rule" => Some(CategorySource::Rule),
            "llm" => Some(CategorySource::Llm),
            "manual" => Some(CategorySource::Manual),
            // Unknown historical values (e.g. "simplefin") degrade to None.
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
    /// When the purchase actually happened (Plaid `authorized_date`), if the
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
    #[serde(default = "default_source")]
    pub source: String,
}

/// Non-secret metadata for a linked Plaid Item (one bank connection). The
/// access token is a secret and lives in a 0600 file outside the DB; the sync
/// cursor lives here because it must commit atomically with the transaction
/// batch it produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlaidItem {
    pub item_id: String,
    pub institution: String,
    pub cursor: Option<String>,
    /// "ok" | "login_required" | "error" — drives the re-link affordance.
    pub status: String,
    pub created: i64,
    pub last_synced: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchStatus {
    Proposed,
    Accepted,
    Rejected,
}

impl MatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchStatus::Proposed => "proposed",
            MatchStatus::Accepted => "accepted",
            MatchStatus::Rejected => "rejected",
        }
    }

    pub fn from_str(s: &str) -> Option<MatchStatus> {
        match s {
            "proposed" => Some(MatchStatus::Proposed),
            "accepted" => Some(MatchStatus::Accepted),
            "rejected" => Some(MatchStatus::Rejected),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MatchConfidence {
    High,
    Medium,
}

impl MatchConfidence {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchConfidence::High => "high",
            MatchConfidence::Medium => "medium",
        }
    }

    pub fn from_str(s: &str) -> Option<MatchConfidence> {
        match s {
            "high" => Some(MatchConfidence::High),
            "medium" => Some(MatchConfidence::Medium),
            _ => None,
        }
    }
}

/// A detected checking→card payment pair. `bank_txn_id` is the outflow leg on a
/// checking/savings account; `card_txn_id` the offsetting inflow on a credit
/// account. Accepting one flags both legs `CardPayment`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TxnMatch {
    pub id: i64,
    pub bank_txn_id: String,
    pub card_txn_id: String,
    pub status: MatchStatus,
    pub confidence: MatchConfidence,
    pub reason: Option<String>,
    pub created: i64,
}

/// One row of the sync history (`sync_log`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncEntry {
    pub id: i64,
    pub started: i64,
    pub finished: Option<i64>,
    pub source: String,
    pub added: Option<i64>,
    pub updated: Option<i64>,
    pub note: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::account_display_label as label;

    #[test]
    fn no_nickname_shows_raw_name() {
        assert_eq!(label("CREDIT CARD (4707)", None), "CREDIT CARD (4707)");
        assert_eq!(label("CREDIT CARD (4707)", Some("")), "CREDIT CARD (4707)");
        assert_eq!(label("CREDIT CARD (4707)", Some("   ")), "CREDIT CARD (4707)");
    }

    #[test]
    fn nickname_keeps_the_trailing_mask() {
        assert_eq!(label("CREDIT CARD (4707)", Some("Sapphire")), "Sapphire (4707)");
        assert_eq!(label("CREDIT CARD (4707)", Some("  Sapphire  ")), "Sapphire (4707)");
    }

    #[test]
    fn nickname_without_a_mask_shows_alone() {
        assert_eq!(label("Checking 0000", Some("Everyday")), "Everyday");
        assert_eq!(label("Plain Name", Some("Nick")), "Nick");
    }

    #[test]
    fn strips_only_the_last_paren_group() {
        assert_eq!(label("Joint (shared) (3333)", Some("Nick")), "Nick (3333)");
    }
}
