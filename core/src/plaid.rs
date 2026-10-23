//! Pure Plaid → domain mapping. Mirrors `source::parse_account_set`: the HTTP
//! transport lives in `outflow-net`; this module only turns response JSON into
//! domain types with explicit validation (invariant #5). Sign convention: Plaid
//! reports outflows as POSITIVE amounts and credit balances as positive
//! amounts owed — both are negated here to match the domain (outflow < 0,
//! credit balance < 0).

use crate::model::{Account, AccountKind, CategorySource, Holding, Security, Transaction, TxnFlag};
use crate::money::Money;
use chrono::{Local, NaiveDate, TimeZone};

/// Why external JSON couldn't become domain data. Malformed input surfaces
/// here explicitly — it never leaks defaults into the archive (invariant #5).
#[derive(Debug, Clone, PartialEq)]
pub enum SourceError {
    /// Structurally bad JSON or a missing/mistyped required field.
    Json(String),
    /// An amount that couldn't parse to exact cents.
    Amount {
        account: String,
        field: &'static str,
        value: String,
    },
}

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
    /// Empty when Plaid has no cursor to give yet (new item, initial pull not
    /// ready); `store::apply_plaid_batch` then keeps the stored cursor.
    pub next_cursor: String,
    pub has_more: bool,
}

/// Map Plaid `type`/`subtype` to a domain kind. Real typing from the API —
/// unlike SimpleFIN, no name-substring guessing. Loans and anything unknown
/// fall to `Other`.
fn plaid_kind(kind: &str, subtype: Option<&str>) -> AccountKind {
    match kind {
        "credit" => AccountKind::Credit,
        "investment" => AccountKind::Investment,
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
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
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
        let kind = plaid_kind(str_field(a, "type").unwrap_or(""), str_field(a, "subtype"));
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
        let mut balance = match balances
            .and_then(|b| b.get("current"))
            .and_then(|c| c.as_number())
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
            nickname: None,
            hidden: false,
        });
    }
    Ok(accounts)
}

/// Whether an `/accounts/get` response's `item` has Investments access:
/// listed in `consented_products` (granted, maybe not yet billed) or in
/// `products`/`billed_products` (already initialized). A missing `item` or
/// list means no; a list that isn't an array of strings is malformed.
pub fn item_has_investments_consent(json: &str) -> Result<bool, SourceError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| SourceError::Json(e.to_string()))?;
    let Some(item) = v.get("item") else {
        return Ok(false);
    };
    for key in ["consented_products", "products", "billed_products"] {
        let Some(list) = item.get(key) else { continue };
        let arr = list
            .as_array()
            .ok_or_else(|| SourceError::Json(format!("item.{key} is not an array")))?;
        for p in arr {
            match p.as_str() {
                Some("investments") => return Ok(true),
                Some(_) => {}
                None => {
                    return Err(SourceError::Json(format!(
                        "item.{key} has a non-string entry"
                    )))
                }
            }
        }
    }
    Ok(false)
}

/// An optional `YYYY-MM-DD` field: absent/null → None, anything else must be
/// a real calendar date (kept as that text).
fn opt_date(v: &serde_json::Value, key: &str) -> Result<Option<String>, SourceError> {
    match v.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(d) => {
            let s = d
                .as_str()
                .ok_or_else(|| SourceError::Json(format!("{key}: expected a date, got {d}")))?;
            NaiveDate::parse_from_str(s, "%Y-%m-%d")
                .map_err(|e| SourceError::Json(format!("{key}: bad date {s:?}: {e}")))?;
            Ok(Some(s.to_string()))
        }
    }
}

/// A Plaid decimal kept as exact text (share quantities, unit prices). Same
/// no-f64 rule as money: `Number::to_string` is the shortest round-trip form,
/// and it must look like a plain decimal (no exponent).
fn parse_decimal_text(v: &serde_json::Value, what: &str) -> Result<String, SourceError> {
    let n = v
        .as_number()
        .ok_or_else(|| SourceError::Json(format!("{what}: expected a number, got {v}")))?;
    let s = n.to_string();
    let digits = s.strip_prefix('-').unwrap_or(&s);
    let mut parts = digits.splitn(2, '.');
    let ok = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    if !ok(parts.next().unwrap_or("")) || !parts.next().is_none_or(ok) {
        return Err(SourceError::Json(format!(
            "{what}: not a plain decimal: {s}"
        )));
    }
    Ok(s)
}

/// Parse `/investments/holdings/get` into securities + current positions.
/// Strict at the boundary: every holding must reference a security in the
/// same response, and quantity/price/value must be numbers. `cost_basis` may
/// be null (cash sweeps, some warrants); `is_cash_equivalent` null = false.
pub fn parse_holdings_get(json: &str) -> Result<(Vec<Security>, Vec<Holding>), SourceError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| SourceError::Json(e.to_string()))?;
    let array = |key: &str| {
        v.get(key)
            .and_then(|a| a.as_array())
            .ok_or_else(|| SourceError::Json(format!("missing {key} array")))
    };

    let mut securities = Vec::new();
    for s in array("securities")? {
        let security_id = require_str(s, "security_id")?;
        let sec_type = str_field(s, "type").unwrap_or("unknown").to_string();
        let ticker = str_field(s, "ticker_symbol").map(str::to_string);
        let name = str_field(s, "name")
            .map(str::to_string)
            .or_else(|| ticker.clone())
            .unwrap_or_else(|| security_id.clone());
        let is_cash = s
            .get("is_cash_equivalent")
            .and_then(|b| b.as_bool())
            .unwrap_or(false)
            || sec_type == "cash";
        let close_price = match s.get("close_price") {
            None | Some(serde_json::Value::Null) => None,
            Some(p) => Some(parse_decimal_text(p, "security close_price")?),
        };
        securities.push(Security {
            security_id,
            ticker,
            name,
            sec_type,
            is_cash,
            cusip: str_field(s, "cusip").map(str::to_string),
            isin: str_field(s, "isin").map(str::to_string),
            close_price,
            close_price_as_of: opt_date(s, "close_price_as_of")?,
        });
    }

    let mut holdings = Vec::new();
    for h in array("holdings")? {
        let account_id = require_str(h, "account_id")?;
        let security_id = require_str(h, "security_id")?;
        if !securities.iter().any(|s| s.security_id == security_id) {
            return Err(SourceError::Json(format!(
                "holding in {account_id} references unknown security {security_id}"
            )));
        }
        let field = |key: &str| {
            h.get(key)
                .ok_or_else(|| SourceError::Json(format!("holding {security_id}: missing {key}")))
        };
        let quantity = parse_decimal_text(field("quantity")?, "holding quantity")?;
        let price = parse_decimal_text(field("institution_price")?, "holding price")?;
        let value = match field("institution_value")?.as_number() {
            Some(n) => parse_amount(n, &account_id, "institution_value")?,
            None => {
                return Err(SourceError::Json(format!(
                    "holding {security_id}: institution_value is not a number"
                )))
            }
        };
        let cost_basis = match h.get("cost_basis") {
            None | Some(serde_json::Value::Null) => None,
            Some(c) => match c.as_number() {
                Some(n) => Some(parse_amount(n, &account_id, "cost_basis")?),
                None => {
                    return Err(SourceError::Json(format!(
                        "holding {security_id}: cost_basis is not a number"
                    )))
                }
            },
        };
        let price_as_of = opt_date(h, "institution_price_as_of")?;
        let currency = str_field(h, "iso_currency_code")
            .or_else(|| str_field(h, "unofficial_currency_code"))
            .map(str::to_string);
        holdings.push(Holding {
            account_id,
            security_id,
            quantity,
            price,
            value,
            cost_basis,
            price_as_of,
            currency,
        });
    }
    Ok((securities, holdings))
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
        // Present-but-empty is legitimate: a freshly linked item whose initial
        // pull isn't ready (`transactions_update_status: NOT_READY`) answers
        // with `"next_cursor": ""`. Absent or non-string stays an error.
        next_cursor: match v.get("next_cursor") {
            Some(serde_json::Value::String(s)) => s.clone(),
            _ => return Err(SourceError::Json("missing field \"next_cursor\"".into())),
        },
        has_more: v.get("has_more").and_then(|h| h.as_bool()).unwrap_or(false),
    })
}

/// An offline ingest bundle: a Plaid `/accounts/get` response and one
/// `/transactions/sync` page wrapped in an envelope, so the full pipeline runs
/// with zero features and no credentials (`pull --from-file`) — the dev/test
/// path that used to ride on SimpleFIN fixtures.
#[derive(Debug, Clone, PartialEq)]
pub struct Fixture {
    pub accounts: Vec<Account>,
    pub sync: SyncPage,
}

/// Parse a fixture envelope: `{ "institution": "...", "accounts": <accounts/get
/// response>, "sync": <transactions/sync response> }`. `institution` defaults
/// to "Demo Bank"; `synced_at` stamps `Account::last_synced`.
pub fn parse_fixture(json: &str, synced_at: i64) -> Result<Fixture, SourceError> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| SourceError::Json(e.to_string()))?;
    let institution = str_field(&v, "institution")
        .unwrap_or("Demo Bank")
        .to_string();
    let accounts_raw = v
        .get("accounts")
        .ok_or_else(|| SourceError::Json("fixture: missing \"accounts\" object".into()))?
        .to_string();
    let sync_raw = v
        .get("sync")
        .ok_or_else(|| SourceError::Json("fixture: missing \"sync\" object".into()))?
        .to_string();
    Ok(Fixture {
        accounts: parse_accounts_get(&accounts_raw, &institution, synced_at)?,
        sync: parse_sync_page(&sync_raw)?,
    })
}

#[cfg(test)]
// Amounts are written dollars_cents (110_24 = $110.24) for readability.
#[allow(clippy::inconsistent_digit_grouping)]
mod tests {
    use super::*;

    const ACCOUNTS: &str = include_str!("../../examples/plaid-accounts-get.json");
    const SYNC: &str = include_str!("../../examples/plaid-sync-page.json");
    const FIXTURE: &str = include_str!("../../examples/plaid-fixture.json");

    #[test]
    fn fixture_envelope_parses_both_halves() {
        let f = parse_fixture(FIXTURE, 42).unwrap();
        assert_eq!(f.accounts.len(), 3);
        assert_eq!(f.accounts[0].org, "Demo Bank");
        assert_eq!(f.accounts[0].last_synced, 42);
        assert!(f.sync.added.iter().any(|t| t.id == "txn-coffee"));
        assert_eq!(f.sync.removed_ids, vec!["txn-gone".to_string()]);
    }

    #[test]
    fn fixture_missing_half_errors() {
        assert!(matches!(
            parse_fixture(r#"{"accounts":{"accounts":[]}}"#, 0),
            Err(SourceError::Json(_))
        ));
    }

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

    /// A freshly linked item before Plaid's initial pull finishes. Real shape:
    /// HTTP 200, every list empty, `next_cursor` present but "". This used to
    /// fail as "missing field next_cursor", breaking every new link.
    #[test]
    fn not_ready_page_with_empty_cursor_parses() {
        let json = r#"{"added":[],"modified":[],"removed":[],"accounts":[],
            "next_cursor":"","has_more":false,
            "transactions_update_status":"NOT_READY","request_id":"r1"}"#;
        let page = parse_sync_page(json).expect("NOT_READY page must parse");
        assert_eq!(page.next_cursor, "");
        assert!(!page.has_more);
        assert!(page.added.is_empty() && page.modified.is_empty());
        assert!(page.removed_ids.is_empty() && page.pending_superseded.is_empty());
    }

    // Shape taken from a real Vanguard /investments/holdings/get: a cash sweep
    // (VMFXX, is_cash_equivalent), a no-ticker warrant with null cost basis,
    // and a fractional-share ETF.
    const HOLDINGS: &str = r#"{
      "accounts": [],
      "securities": [
        {"security_id":"s_vmfxx","ticker_symbol":"VMFXX","name":"Vanguard Federal Money Market",
         "type":"mutual fund","is_cash_equivalent":true},
        {"security_id":"s_wts","ticker_symbol":null,"name":"Gamestop Corp Wts Exp 10/30/26",
         "type":"derivative","is_cash_equivalent":null},
        {"security_id":"s_vwo","ticker_symbol":"VWO","name":"Vanguard FTSE EM ETF",
         "type":"etf","is_cash_equivalent":false,"cusip":"922042858","isin":"US9220428588",
         "close_price":58.12,"close_price_as_of":"2026-10-21"},
        {"security_id":"s_usd","ticker_symbol":"CUR:USD","name":"US Dollar","type":"cash"}
      ],
      "holdings": [
        {"account_id":"brk","security_id":"s_vmfxx","quantity":1090.87,
         "institution_price":1,"institution_value":1090.87,"cost_basis":null},
        {"account_id":"brk","security_id":"s_wts","quantity":7,
         "institution_price":1.65,"institution_value":11.55,"cost_basis":null},
        {"account_id":"brk","security_id":"s_vwo","quantity":326.5299,
         "institution_price":58.0312,"institution_value":18948.53,"cost_basis":13058.07,
         "institution_price_as_of":"2026-10-21","iso_currency_code":"USD"}
      ]
    }"#;

    #[test]
    fn holdings_parse_exact_and_classify_cash() {
        let (secs, holds) = parse_holdings_get(HOLDINGS).unwrap();
        assert_eq!(secs.len(), 4);
        let sec = |id: &str| secs.iter().find(|s| s.security_id == id).unwrap();
        assert!(sec("s_vmfxx").is_cash);
        assert!(sec("s_usd").is_cash); // type "cash"
        assert!(!sec("s_wts").is_cash); // null is_cash_equivalent = false
        assert_eq!(sec("s_wts").ticker, None);
        assert_eq!(sec("s_wts").name, "Gamestop Corp Wts Exp 10/30/26");

        assert_eq!(holds.len(), 3);
        let vwo = holds.iter().find(|h| h.security_id == "s_vwo").unwrap();
        assert_eq!(vwo.quantity, "326.5299"); // exact text, not rounded
        assert_eq!(vwo.price, "58.0312");
        assert_eq!(vwo.value.cents(), 18_948_53);
        assert_eq!(vwo.cost_basis.map(|m| m.cents()), Some(13_058_07));
        assert_eq!(vwo.price_as_of.as_deref(), Some("2026-10-21"));
        assert_eq!(vwo.currency.as_deref(), Some("USD"));
        assert_eq!(sec("s_vwo").cusip.as_deref(), Some("922042858"));
        assert_eq!(sec("s_vwo").isin.as_deref(), Some("US9220428588"));
        assert_eq!(sec("s_vwo").close_price.as_deref(), Some("58.12"));
        assert_eq!(
            sec("s_vwo").close_price_as_of.as_deref(),
            Some("2026-10-21")
        );
        let wts = holds.iter().find(|h| h.security_id == "s_wts").unwrap();
        assert_eq!(wts.cost_basis, None);
        assert_eq!(wts.price_as_of, None);
        assert_eq!(sec("s_wts").close_price, None);
        assert_eq!(wts.quantity, "7");
    }

    #[test]
    fn holdings_reject_unknown_security_and_bad_numbers() {
        let unknown = r#"{"securities":[],"holdings":[{"account_id":"a","security_id":"x",
            "quantity":1,"institution_price":1,"institution_value":1}]}"#;
        assert!(parse_holdings_get(unknown).is_err());
        let bad_qty = r#"{"securities":[{"security_id":"x","type":"etf"}],"holdings":[
            {"account_id":"a","security_id":"x","quantity":"lots","institution_price":1,
             "institution_value":1}]}"#;
        assert!(parse_holdings_get(bad_qty).is_err());
        let no_value = r#"{"securities":[{"security_id":"x","type":"etf"}],"holdings":[
            {"account_id":"a","security_id":"x","quantity":1,"institution_price":1}]}"#;
        assert!(parse_holdings_get(no_value).is_err());
        assert!(parse_holdings_get(r#"{"holdings":[]}"#).is_err()); // no securities
        let bad_as_of = r#"{"securities":[{"security_id":"x","type":"etf"}],"holdings":[
            {"account_id":"a","security_id":"x","quantity":1,"institution_price":1,
             "institution_value":1,"institution_price_as_of":"10/21/2026"}]}"#;
        assert!(parse_holdings_get(bad_as_of).is_err());
        let bad_close = r#"{"securities":[{"security_id":"x","type":"etf","close_price":"n/a"}],
            "holdings":[]}"#;
        assert!(parse_holdings_get(bad_close).is_err());
    }

    #[test]
    fn investment_accounts_get_their_own_kind() {
        assert_eq!(
            plaid_kind("investment", Some("ira")),
            AccountKind::Investment
        );
        assert_eq!(
            plaid_kind("investment", Some("hsa")),
            AccountKind::Investment
        );
        assert_eq!(plaid_kind("loan", Some("mortgage")), AccountKind::Other);
    }

    #[test]
    fn investments_consent_reads_any_product_list() {
        let consented = r#"{"accounts":[],"item":{"products":["transactions"],
            "consented_products":["transactions","investments"]}}"#;
        assert!(item_has_investments_consent(consented).unwrap());
        let initialized = r#"{"accounts":[],"item":{"billed_products":["investments"]}}"#;
        assert!(item_has_investments_consent(initialized).unwrap());
        // The ADDITIONAL_CONSENT_REQUIRED shape: transactions only.
        let not_yet = r#"{"accounts":[],"item":{"products":["transactions"],
            "consented_products":["transactions"]}}"#;
        assert!(!item_has_investments_consent(not_yet).unwrap());
        assert!(!item_has_investments_consent(r#"{"accounts":[]}"#).unwrap());
    }

    #[test]
    fn malformed_product_list_is_an_error() {
        assert!(
            item_has_investments_consent(r#"{"item":{"consented_products":"investments"}}"#)
                .is_err()
        );
        assert!(item_has_investments_consent(r#"{"item":{"products":[7]}}"#).is_err());
    }

    #[test]
    fn non_string_cursor_is_an_error() {
        for cursor in ["null", "123", "[]"] {
            let json = format!(r#"{{"added":[],"next_cursor":{cursor},"has_more":false}}"#);
            assert!(
                matches!(parse_sync_page(&json), Err(SourceError::Json(_))),
                "next_cursor = {cursor} should be rejected"
            );
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
