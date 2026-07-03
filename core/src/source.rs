use crate::model::{Account, AccountKind, Transaction};
use crate::money::Money;
use serde::{Deserialize, Serialize};

#[derive(Debug, PartialEq)]
pub struct Fetched {
    pub accounts: Vec<Account>,
    pub transactions: Vec<Transaction>,
    /// Non-fatal advisory messages the bridge returned alongside data
    /// (e.g. "Requested date range exceeds limit of 90 days and was capped.").
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub enum SourceError {
    Json(String),
    Remote(Vec<String>),
    Amount {
        account: String,
        field: &'static str,
        value: String,
    },
}

pub trait TransactionSource {
    fn fetch(&self) -> Result<Fetched, SourceError>;
}

#[derive(Deserialize)]
struct SfinAccountSet {
    #[serde(default)]
    errors: Vec<String>,
    #[serde(default)]
    accounts: Vec<SfinAccount>,
}

#[derive(Deserialize)]
struct SfinOrg {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    domain: Option<String>,
}

#[derive(Deserialize)]
struct SfinAccount {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    org: SfinOrg,
    #[serde(default)]
    currency: Option<String>,
    #[serde(default)]
    balance: Option<String>,
    #[serde(rename = "balance-date", default)]
    balance_date: Option<i64>,
    #[serde(default)]
    transactions: Vec<SfinTransaction>,
}

impl Default for SfinOrg {
    fn default() -> Self {
        SfinOrg {
            name: None,
            domain: None,
        }
    }
}

#[derive(Deserialize, Serialize)]
struct SfinTransaction {
    id: String,
    posted: i64,
    amount: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    payee: Option<String>,
    #[serde(default)]
    memo: Option<String>,
    #[serde(default)]
    pending: bool,
}

fn account_kind(name: &str) -> AccountKind {
    let n = name.to_lowercase();
    if n.contains("sav") {
        AccountKind::Savings
    } else if n.contains("credit") || n.contains("card") {
        AccountKind::Credit
    } else if n.contains("check") || n.contains("chq") || n.contains("chequ") {
        AccountKind::Checking
    } else {
        AccountKind::Other
    }
}

pub fn parse_account_set(json: &str) -> Result<Fetched, SourceError> {
    let set: SfinAccountSet =
        serde_json::from_str(json).map_err(|e| SourceError::Json(e.to_string()))?;
    // SimpleFIN returns advisory messages in `errors` even on success (the demo
    // caps the date range and says so). Treat `errors` as fatal only when the
    // bridge returned no accounts at all; otherwise surface them as warnings.
    if set.accounts.is_empty() && !set.errors.is_empty() {
        return Err(SourceError::Remote(set.errors));
    }

    let mut accounts = Vec::new();
    let mut transactions = Vec::new();

    for a in &set.accounts {
        let balance = match &a.balance {
            Some(b) => Money::from_decimal_str(b).map_err(|_| SourceError::Amount {
                account: a.id.clone(),
                field: "balance",
                value: b.clone(),
            })?,
            None => Money::from_cents(0),
        };
        let org = a
            .org
            .name
            .clone()
            .or_else(|| a.org.domain.clone())
            .unwrap_or_default();
        let name = a.name.clone().unwrap_or_default();
        let kind = account_kind(&name);

        accounts.push(Account {
            id: a.id.clone(),
            org,
            name,
            kind,
            balance,
            currency: a.currency.clone().unwrap_or_else(|| "USD".into()),
            last_synced: a.balance_date.unwrap_or(0),
        });

        for t in &a.transactions {
            let amount = Money::from_decimal_str(&t.amount).map_err(|_| SourceError::Amount {
                account: a.id.clone(),
                field: "amount",
                value: t.amount.clone(),
            })?;
            let payee = t.payee.clone().filter(|s| !s.is_empty());
            let description = t
                .description
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| t.memo.clone())
                .unwrap_or_default();
            let raw = serde_json::to_string(t).unwrap_or_default();

            transactions.push(Transaction {
                id: t.id.clone(),
                account_id: a.id.clone(),
                posted: t.posted,
                amount,
                description,
                payee,
                category: None,
                category_source: None,
                pending: t.pending,
                raw,
            });
        }
    }

    Ok(Fetched {
        accounts,
        transactions,
        warnings: set.errors,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO: &str = r#"
    {
      "errors": [],
      "accounts": [
        {
          "org": { "name": "Chase", "domain": "chase.com" },
          "id": "ACT-checking",
          "name": "Total Checking",
          "currency": "USD",
          "balance": "4210.55",
          "balance-date": 1710000000,
          "transactions": [
            { "id": "t1", "posted": 1709000000, "amount": "-33.72", "description": "STARBUCKS 812", "payee": "Starbucks" },
            { "id": "t2", "posted": 1709100000, "amount": "2500.00", "description": "ACME PAYROLL" },
            { "id": "t3", "posted": 1709200000, "amount": "-12.00", "memo": "netflix", "pending": true }
          ]
        },
        {
          "org": { "domain": "chase.com" },
          "id": "ACT-card",
          "name": "Freedom Credit Card",
          "balance": "-540.10",
          "transactions": []
        }
      ]
    }"#;

    #[test]
    fn parses_accounts_and_transactions() {
        let f = parse_account_set(DEMO).unwrap();
        assert_eq!(f.accounts.len(), 2);
        assert_eq!(f.transactions.len(), 3);
    }

    #[test]
    fn preserves_outflow_sign_and_cents() {
        let f = parse_account_set(DEMO).unwrap();
        let t1 = f.transactions.iter().find(|t| t.id == "t1").unwrap();
        assert_eq!(t1.amount.cents(), -3372);
        assert!(t1.amount.is_outflow());
        let t2 = f.transactions.iter().find(|t| t.id == "t2").unwrap();
        assert_eq!(t2.amount.cents(), 250000);
    }

    #[test]
    fn falls_back_to_memo_when_no_description() {
        let f = parse_account_set(DEMO).unwrap();
        let t3 = f.transactions.iter().find(|t| t.id == "t3").unwrap();
        assert_eq!(t3.description, "netflix");
        assert!(t3.pending);
        assert!(t3.payee.is_none());
    }

    #[test]
    fn infers_account_kind_from_name() {
        let f = parse_account_set(DEMO).unwrap();
        let checking = f.accounts.iter().find(|a| a.id == "ACT-checking").unwrap();
        let card = f.accounts.iter().find(|a| a.id == "ACT-card").unwrap();
        assert_eq!(checking.kind, AccountKind::Checking);
        assert_eq!(card.kind, AccountKind::Credit);
        assert_eq!(card.balance.cents(), -54010);
        assert_eq!(checking.org, "Chase");
        assert_eq!(card.org, "chase.com");
    }

    #[test]
    fn surfaces_remote_errors() {
        let json = r#"{ "errors": ["Connection to bank failed"], "accounts": [] }"#;
        assert_eq!(
            parse_account_set(json),
            Err(SourceError::Remote(vec!["Connection to bank failed".into()]))
        );
    }

    #[test]
    fn advisory_errors_with_accounts_are_warnings_not_failure() {
        // The live SimpleFIN demo returns a date-range warning in `errors`
        // alongside valid accounts; that must not fail the whole pull.
        let json = r#"{
            "errors": ["Requested date range exceeds limit of 90 days and was capped."],
            "accounts": [
                { "org": {}, "id": "A", "name": "Checking", "balance": "1.00",
                  "transactions": [ { "id": "x", "posted": 1, "amount": "-1.00" } ] }
            ]
        }"#;
        let f = parse_account_set(json).unwrap();
        assert_eq!(f.accounts.len(), 1);
        assert_eq!(f.transactions.len(), 1);
        assert_eq!(
            f.warnings,
            vec!["Requested date range exceeds limit of 90 days and was capped.".to_string()]
        );
    }

    #[test]
    fn rejects_unparseable_amount() {
        let json = r#"{ "errors": [], "accounts": [
            { "org": {}, "id": "A", "balance": "0.00", "transactions": [
                { "id": "x", "posted": 1, "amount": "not-a-number" }
            ] } ] }"#;
        match parse_account_set(json) {
            Err(SourceError::Amount { field, .. }) => assert_eq!(field, "amount"),
            other => panic!("expected amount error, got {:?}", other),
        }
    }
}
