use crate::model::Transaction;
use serde::Serialize;
use std::collections::HashMap;

const DAY: i64 = 86400;
const MIN_OCCURRENCES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Cadence {
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Subscription {
    pub payee: String,
    pub cadence: Cadence,
    pub typical_amount_cents: i64,
    pub occurrences: usize,
    pub first_seen: i64,
    pub last_seen: i64,
    pub total_cents: i64,
}

fn is_processor_prefix(head: &str) -> bool {
    matches!(head.trim(), "sq" | "tst" | "sp" | "pp" | "py" | "pypl" | "paypal")
}

pub fn normalize_payee(s: &str) -> String {
    let lower = s.to_lowercase();
    let base = match lower.find('*') {
        Some(i) if is_processor_prefix(&lower[..i]) => &lower[i + 1..],
        Some(i) => &lower[..i],
        None => &lower[..],
    };
    let mut out = String::new();
    for tok in base.split(|c: char| !c.is_alphanumeric()) {
        if tok.is_empty() {
            continue;
        }
        let digits = tok.chars().filter(|c| c.is_ascii_digit()).count();
        if digits * 2 >= tok.len() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(tok);
    }
    out.trim().to_string()
}

fn median(sorted: &[i64]) -> i64 {
    sorted[sorted.len() / 2]
}

pub fn detect(txns: &[Transaction]) -> Vec<Subscription> {
    let mut groups: HashMap<String, Vec<&Transaction>> = HashMap::new();
    for t in txns.iter().filter(|t| t.amount.is_outflow()) {
        let key = normalize_payee(t.merchant());
        if key.is_empty() {
            continue;
        }
        groups.entry(key).or_default().push(t);
    }

    let mut out = Vec::new();

    for (payee, mut items) in groups {
        if items.len() < MIN_OCCURRENCES {
            continue;
        }
        items.sort_by_key(|t| t.posted);

        let mut amounts: Vec<i64> = items.iter().map(|t| t.amount.cents().abs()).collect();
        amounts.sort_unstable();
        let med = median(&amounts);
        let tol = std::cmp::max(100, med / 20);
        let stable = amounts
            .iter()
            .filter(|a| (**a - med).abs() <= tol)
            .count();
        if stable * 3 < amounts.len() * 2 {
            continue;
        }

        let mut monthly = 0usize;
        let mut yearly = 0usize;
        let mut gaps = 0usize;
        for w in items.windows(2) {
            let days = (w[1].posted - w[0].posted) / DAY;
            gaps += 1;
            if (26..=35).contains(&days) {
                monthly += 1;
            } else if (350..=380).contains(&days) {
                yearly += 1;
            }
        }
        if gaps == 0 {
            continue;
        }

        let cadence = if monthly * 2 >= gaps && monthly >= yearly {
            Cadence::Monthly
        } else if yearly * 2 >= gaps {
            Cadence::Yearly
        } else {
            continue;
        };

        let first_seen = items.first().unwrap().posted;
        let last_seen = items.last().unwrap().posted;
        let total_cents = amounts.iter().sum();

        out.push(Subscription {
            payee,
            cadence,
            typical_amount_cents: med,
            occurrences: items.len(),
            first_seen,
            last_seen,
            total_cents,
        });
    }

    out.sort_by(|a, b| b.total_cents.cmp(&a.total_cents));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Money;

    fn charge(id: &str, day_offset: i64, cents: i64, merchant: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct".into(),
            posted: day_offset * DAY,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            raw: "{}".into(),
        }
    }

    #[test]
    fn detects_monthly_subscription() {
        let txns = vec![
            charge("1", 0, -1599, "Netflix"),
            charge("2", 30, -1599, "Netflix"),
            charge("3", 60, -1599, "Netflix"),
            charge("4", 91, -1599, "Netflix"),
        ];
        let subs = detect(&txns);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].cadence, Cadence::Monthly);
        assert_eq!(subs[0].typical_amount_cents, 1599);
        assert_eq!(subs[0].occurrences, 4);
    }

    #[test]
    fn ignores_irregular_purchases() {
        let txns = vec![
            charge("1", 0, -1200, "Corner Store"),
            charge("2", 3, -4500, "Corner Store"),
            charge("3", 47, -800, "Corner Store"),
        ];
        let subs = detect(&txns);
        assert!(subs.is_empty());
    }

    #[test]
    fn ignores_inflows() {
        let txns = vec![
            charge("1", 0, 200000, "Payroll"),
            charge("2", 30, 200000, "Payroll"),
            charge("3", 60, 200000, "Payroll"),
        ];
        assert!(detect(&txns).is_empty());
    }

    #[test]
    fn normalizes_merchant_noise() {
        assert_eq!(normalize_payee("SQ *BLUE BOTTLE 4471"), "blue bottle");
        assert_eq!(normalize_payee("AMZN Mktp US*2X4K9"), "amzn mktp us");
        assert_eq!(normalize_payee("NETFLIX.COM"), "netflix com");
    }

    #[test]
    fn groups_across_merchant_noise() {
        let txns = vec![
            charge("1", 0, -1599, "SQ *SPOTIFY 001"),
            charge("2", 30, -1599, "SQ *SPOTIFY 002"),
            charge("3", 60, -1599, "SQ *SPOTIFY 003"),
        ];
        let subs = detect(&txns);
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].payee, "spotify");
    }
}
