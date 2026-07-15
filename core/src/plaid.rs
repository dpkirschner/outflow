//! Pure Plaid → domain mapping. Mirrors `source::parse_account_set`: the HTTP
//! transport lives in `outflow-net`; this module only turns response JSON into
//! domain types with explicit validation (invariant #5). Sign convention: Plaid
//! reports outflows as POSITIVE amounts and credit balances as positive
//! amounts owed — both are negated here to match the domain (outflow < 0,
//! credit balance < 0).

use crate::model::{
    Account, AccountKind, CategorySource, Transaction, TxnFlag,
};
use crate::money::Money;
use crate::source::SourceError;
use chrono::{Local, NaiveDate, TimeZone};

/// One page of `/transactions/sync`. `pending_superseded` carries the
/// `pending_transaction_id` of every non-pending row — the pending predecessor
/// ids that must be deleted alongside Plaid's own `removed` list (a posted
/// transaction gets a NEW id, so the pending row would otherwise linger and
/// double-count).
#[derive(Debug, Clone, PartialEq)]
pub struct SyncPage {
    pub added: Vec<Transaction>,
    pub modified: Vec<Transaction>,
    pub removed_ids: Vec<String>,
    pub pending_superseded: Vec<String>,
    pub next_cursor: String,
    pub has_more: bool,
}

/// Map Plaid `type`/`subtype` to a domain kind. Real typing from the API —
/// unlike SimpleFIN, no name-substring guessing. `investment` stays `Other`
/// until brokerage support lands.
fn plaid_kind(kind: &str, subtype: Option<&str>) -> AccountKind {
    match kind {
        "credit" => AccountKind::Credit,
        "depository" => match subtype.unwrap_or("") {
            "checking" | "cash management" | "paypal" | "prepaid" => AccountKind::Checking,
            "savings" | "cd" | "money market" | "hsa" | "ebt" => AccountKind::Savings,
            _ => AccountKind::Checking,
        },
        _ => AccountKind::Other,
    }
}

/// Parse a Plaid decimal (JSON number) exactly. `serde_json::Number`'s Display
/// is the shortest round-trip representation, so a bank amount like `4.22`
/// prints back as `"4.22"` and goes through the same exact decimal path as
/// SimpleFIN amounts — never f64 multiplication.
fn parse_amount(
    n: &serde_json::Number,
    account: &str,
    field: &'static str,
) -> Result<Money, SourceError> {
    Money::from_decimal_str(&n.to_string()).map_err(|_| SourceError::Amount {
        account: account.to_string(),
        field,
        value: n.to_string(),
    })
}

/// Plaid dates are calendar strings (`YYYY-MM-DD`) with no time. Anchor them at
/// LOCAL noon so `ym_local` month bucketing can never straddle a boundary in
/// any nearby timezone interpretation.
fn date_to_epoch(s: &str) -> Result<i64, SourceError> {
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| SourceError::Json(format!("bad date {s:?}: {e}")))?;
    let noon = d
        .and_hms_opt(12, 0, 0)
        .ok_or_else(|| SourceError::Json(format!("bad date {s:?}")))?;
    Local
        .from_local_datetime(&noon)
        .earliest()
        .map(|dt| dt.timestamp())
        .ok_or_else(|| SourceError::Json(format!("unrepresentable local time for {s:?}")))
}

fn str_field<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(|x| x.as_str()).filter(|s| !s.is_empty())
}

fn require_str(v: &serde_json::Value, key: &str) -> Result<String, SourceError> {
    str_field(v, key)
        .map(|s| s.to_string())
        .ok_or_else(|| SourceError::Json(format!("missing field {key:?}")))
}

/// Seed the suppression flag from Plaid's classification. Seed-time only: the
/// upsert never overwrites `flag` on conflict, so user overrides always win.
fn flag_from_pfc(primary: Option<&str>, detailed: Option<&str>) -> TxnFlag {
    if detailed == Some("LOAN_PAYMENTS_CREDIT_CARD_PAYMENT") {
        return TxnFlag::CardPayment;
    }
    match primary {
        Some("TRANSFER_IN") | Some("TRANSFER_OUT") => TxnFlag::Transfer,
        _ => TxnFlag::Spending,
    }
}

/// Conservative mapping from Plaid's personal_finance_category into the
/// default vocabulary. Anything ambiguous maps to `None` and falls through to
/// rules / the LLM tail, which also always outrank a Plaid-sourced category
/// (they run on uncategorized rows only, and manual fixes overwrite).
fn category_from_pfc(primary: Option<&str>, detailed: Option<&str>) -> Option<&'static str> {
    match detailed {
        Some("FOOD_AND_DRINK_GROCERIES") => return Some("Groceries"),
        Some("FOOD_AND_DRINK_COFFEE") => return Some("Coffee"),
        Some("TRANSPORTATION_GAS") => return Some("Fuel"),
        Some("RENT_AND_UTILITIES_RENT") => return Some("Rent"),
        Some("ENTERTAINMENT_TV_AND_MOVIES") => return Some("Streaming"),
        _ => {}
    }
    match primary? {
        "FOOD_AND_DRINK" => Some("Dining"),
        "TRANSPORTATION" => Some("Transport"),
        "TRAVEL" => Some("Travel"),
        "ENTERTAINMENT" => Some("Entertainment"),
        "GENERAL_MERCHANDISE" => Some("Shopping"),
        "RENT_AND_UTILITIES" => Some("Utilities"),
        "MEDICAL" => Some("Health"),
        "PERSONAL_CARE" => Some("Personal Care"),
        "HOME_IMPROVEMENT" => Some("Home"),
        "BANK_FEES" => Some("Fees"),
        "INCOME" => Some("Income"),
        "TRANSFER_IN" | "TRANSFER_OUT" | "LOAN_PAYMENTS" => Some("Transfers"),
        _ => None,
    }
}

/// Parse an `/accounts/get` response. `institution` becomes `org` (Plaid's
/// response carries only an institution_id; the human name is captured from
/// Link metadata at exchange time). The account mask is appended to the name
/// so the ledger's card last-4 attribution keeps working.
pub fn parse_accounts_get(
    json: &str,
    institution: &str,
    synced_at: i64,
) -> Result<Vec<Account>, SourceError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| SourceError::Json(e.to_string()))?;
    let raw_accounts = v
        .get("accounts")
        .and_then(|a| a.as_array())
        .ok_or_else(|| SourceError::Json("missing accounts array".into()))?;

    let mut accounts = Vec::with_capacity(raw_accounts.len());
    for a in raw_accounts {
        let id = require_str(a, "account_id")?;
        let kind = plaid_kind(
            str_field(a, "type").unwrap_or(""),
            str_field(a, "subtype"),
        );
        let mut name = str_field(a, "name")
            .or_else(|| str_field(a, "official_name"))
            .unwrap_or("Account")
            .to_string();
        if let Some(mask) = str_field(a, "mask") {
            if !name.contains(mask) {
                name = format!("{name} ({mask})");
            }
        }
        let balances = a.get("balances");
        let mut balance = match balances.and_then(|b| b.get("current")).and_then(|c| c.as_number())
        {
            Some(n) => parse_amount(n, &id, "balance")?,
            None => Money::from_cents(0),
        };
        // Plaid credit balances are positive-owed; domain stores them negative.
        if kind == AccountKind::Credit {
            balance = Money::from_cents(-balance.cents());
        }
        let currency = balances
            .and_then(|b| str_field(b, "iso_currency_code"))
            .unwrap_or("USD")
            .to_string();

        accounts.push(Account {
            id,
            org: institution.to_string(),
            name,
            kind,
            balance,
            currency,
            last_synced: synced_at,
            source: "plaid".to_string(),
        });
    }
    Ok(accounts)
}

fn parse_txn(v: &serde_json::Value) -> Result<Transaction, SourceError> {
    let id = require_str(v, "transaction_id")?;
    let account_id = require_str(v, "account_id")?;
    let amount_num = v
        .get("amount")
        .and_then(|a| a.as_number())
        .ok_or_else(|| SourceError::Json(format!("txn {id}: missing amount")))?;
    // Plaid: positive = money out. Domain: outflow negative.
    let amount = Money::from_cents(-parse_amount(amount_num, &account_id, "amount")?.cents());

    let posted = date_to_epoch(&require_str(v, "date")?)?;
    let transacted_at = match str_field(v, "authorized_date") {
        Some(d) => Some(date_to_epoch(d)?),
        None => None,
    };

    let payee = str_field(v, "merchant_name").map(|s| s.to_string());
    let description = str_field(v, "name")
        .or(str_field(v, "merchant_name"))
        .unwrap_or_default()
        .to_string();

    let pfc = v.get("personal_finance_category");
    let primary = pfc.and_then(|p| str_field(p, "primary"));
    let detailed = pfc.and_then(|p| str_field(p, "detailed"));
    let category = category_from_pfc(primary, detailed).map(|c| c.to_string());
    let category_source = category.as_ref().map(|_| CategorySource::Plaid);

    Ok(Transaction {
        id,
        account_id,
        posted,
        transacted_at,
        amount,
        description,
        payee,
        category,
        category_source,
        pending: v.get("pending").and_then(|p| p.as_bool()).unwrap_or(false),
        flag: flag_from_pfc(primary, detailed),
        raw: v.to_string(),
        source: "plaid".to_string(),
    })
}

/// Parse one `/transactions/sync` page. The caller loops while `has_more`,
/// accumulating pages into a `store::PlaidBatch` so the whole run (and the
/// final cursor) commits atomically.
pub fn parse_sync_page(json: &str) -> Result<SyncPage, SourceError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| SourceError::Json(e.to_string()))?;

    let txn_list = |key: &str| -> Result<Vec<Transaction>, SourceError> {
        v.get(key)
            .and_then(|a| a.as_array())
            .map(|arr| arr.iter().map(parse_txn).collect())
            .unwrap_or_else(|| Ok(Vec::new()))
    };
    let added = txn_list("added")?;
    let modified = txn_list("modified")?;

    let removed_ids = v
        .get("removed")
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|r| {
                    // Plaid sends `{ "transaction_id": ... }` objects; tolerate
                    // bare strings too.
                    r.get("transaction_id")
                        .and_then(|x| x.as_str())
                        .or_else(|| r.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        })
        .unwrap_or_default();

    let pending_superseded = added
        .iter()
        .chain(modified.iter())
        .filter(|t| !t.pending)
        .filter_map(|t| {
            serde_json::from_str::<serde_json::Value>(&t.raw)
                .ok()
                .and_then(|raw| {
                    raw.get("pending_transaction_id")
                        .and_then(|p| p.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                })
        })
        .collect();

    Ok(SyncPage {
        added,
        modified,
        removed_ids,
        pending_superseded,
        next_cursor: require_str(&v, "next_cursor")?,
        has_more: v.get("has_more").and_then(|h| h.as_bool()).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNTS: &str = include_str!("../../examples/plaid-accounts-get.json");
    const SYNC: &str = include_str!("../../examples/plaid-sync-page.json");

    #[test]
    fn accounts_map_kind_sign_and_mask() {
        let accounts = parse_accounts_get(ACCOUNTS, "Chase", 1000).unwrap();
        assert_eq!(accounts.len(), 3);

        let checking = &accounts[0];
        assert_eq!(checking.kind, AccountKind::Checking);
        assert_eq!(checking.balance.cents(), 110_24); // depository stays positive
        assert_eq!(checking.name, "Plaid Checking (0000)");
        assert_eq!(checking.org, "Chase");
        assert_eq!(checking.source, "plaid");
        assert_eq!(checking.last_synced, 1000);

        let savings = &accounts[1];
        assert_eq!(savings.kind, AccountKind::Savings);

        let card = &accounts[2];
        assert_eq!(card.kind, AccountKind::Credit);
        // 410.00 owed → stored negative.
        assert_eq!(card.balance.cents(), -410_00);
    }

    #[test]
    fn sync_page_flips_sign_and_keeps_cents_exact() {
        let page = parse_sync_page(SYNC).unwrap();
        let coffee = page.added.iter().find(|t| t.id == "txn-coffee").unwrap();
        // Plaid 4.22 (money out) → -422 cents, exactly.
        assert_eq!(coffee.amount.cents(), -422);
        assert_eq!(coffee.payee.as_deref(), Some("Starbucks"));
        assert_eq!(coffee.source, "plaid");
        assert!(!coffee.pending);

        let payroll = page.added.iter().find(|t| t.id == "txn-payroll").unwrap();
        // Plaid negative = money in → positive inflow.
        assert_eq!(payroll.amount.cents(), 1234_56);
    }

    #[test]
    fn sync_page_seeds_flags_and_categories() {
        let page = parse_sync_page(SYNC).unwrap();
        let ccpay = page.added.iter().find(|t| t.id == "txn-ccpay").unwrap();
        assert_eq!(ccpay.flag, TxnFlag::CardPayment);
        assert_eq!(ccpay.category.as_deref(), Some("Transfers"));
        assert_eq!(ccpay.category_source, Some(CategorySource::Plaid));

        let coffee = page.added.iter().find(|t| t.id == "txn-coffee").unwrap();
        assert_eq!(coffee.flag, TxnFlag::Spending);
        assert_eq!(coffee.category.as_deref(), Some("Coffee"));

        let mystery = page.added.iter().find(|t| t.id == "txn-mystery").unwrap();
        assert_eq!(mystery.category, None);
        assert_eq!(mystery.category_source, None);
    }

    #[test]
    fn sync_page_collects_removed_and_superseded_pending() {
        let page = parse_sync_page(SYNC).unwrap();
        assert_eq!(page.removed_ids, vec!["txn-gone".to_string()]);
        // txn-posted supersedes its pending predecessor.
        assert_eq!(page.pending_superseded, vec!["txn-pending-old".to_string()]);
        assert_eq!(page.next_cursor, "cursor-2");
        assert!(!page.has_more);
    }

    #[test]
    fn dates_bucket_on_local_noon() {
        let page = parse_sync_page(SYNC).unwrap();
        let coffee = page.added.iter().find(|t| t.id == "txn-coffee").unwrap();
        // authorized_date is a day before date.
        assert!(coffee.transacted_at.unwrap() < coffee.posted);
        assert_eq!(coffee.posted - coffee.transacted_at.unwrap(), 86_400);
        // Noon anchoring: the local-time hour is 12.
        use chrono::Timelike;
        let dt = Local.timestamp_opt(coffee.posted, 0).single().unwrap();
        assert_eq!(dt.time().hour(), 12);
    }

    #[test]
    fn raw_preserves_full_plaid_payload() {
        let page = parse_sync_page(SYNC).unwrap();
        let ccpay = page.added.iter().find(|t| t.id == "txn-ccpay").unwrap();
        let raw: serde_json::Value = serde_json::from_str(&ccpay.raw).unwrap();
        assert_eq!(
            raw["personal_finance_category"]["detailed"],
            "LOAN_PAYMENTS_CREDIT_CARD_PAYMENT"
        );
    }

    #[test]
    fn bad_amount_reports_account_and_value() {
        let json = r#"{"added":[{"transaction_id":"t","account_id":"a","amount":"nope","date":"2026-01-02"}],"next_cursor":"c","has_more":false}"#;
        match parse_sync_page(json) {
            Err(SourceError::Json(_)) => {} // string amount → missing number
            other => panic!("expected Json error, got {other:?}"),
        }
    }

    #[test]
    fn missing_cursor_is_an_error() {
        assert!(matches!(
            parse_sync_page(r#"{"added":[],"has_more":false}"#),
            Err(SourceError::Json(_))
        ));
    }
}
