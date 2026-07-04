use crate::model::{Transaction, TxnFlag};
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
    // Spending only: a recurring transfer or card payment is not a subscription.
    for t in txns
        .iter()
        .filter(|t| t.amount.is_outflow() && t.flag == TxnFlag::Spending)
    {
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

/// Direction of a merchant's amount over time, for the rhythm roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Trend {
    Rising,
    Falling,
    Steady,
}

/// A recurring merchant whose amount varies (unlike a fixed `Subscription`). The
/// data contract for the rhythm-roster screen — it renders exactly these fields.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RhythmEntry {
    pub merchant: String,
    pub cadence: Cadence,
    pub occurrence_count: usize,
    pub median_amount_cents: i64,
    pub amount_min_cents: i64,
    pub amount_max_cents: i64,
    /// Amount normalized to a per-month figure (median for monthly, median/12 for
    /// yearly), so entries of different cadences compare on one axis.
    pub monthly_estimate_cents: i64,
    pub last_seen: i64,
    pub trend: Trend,
}

/// Classify a regular cadence from consecutive gaps (in days). `None` if the
/// spacing is neither predominantly monthly nor yearly. Duplicated from `detect`
/// deliberately so `detect`'s fixed-amount logic stays untouched.
fn classify_cadence(sorted_dates: &[i64]) -> Option<Cadence> {
    let mut monthly = 0usize;
    let mut yearly = 0usize;
    let mut gaps = 0usize;
    for w in sorted_dates.windows(2) {
        let days = (w[1] - w[0]) / DAY;
        gaps += 1;
        if (26..=35).contains(&days) {
            monthly += 1;
        } else if (350..=380).contains(&days) {
            yearly += 1;
        }
    }
    if gaps == 0 {
        return None;
    }
    if monthly * 2 >= gaps && monthly >= yearly {
        Some(Cadence::Monthly)
    } else if yearly * 2 >= gaps {
        Some(Cadence::Yearly)
    } else {
        None
    }
}

/// Compare the median of the earlier half of amounts to the later half.
fn trend_of(sorted_by_date: &[i64]) -> Trend {
    let n = sorted_by_date.len();
    let half = n / 2;
    if half == 0 {
        return Trend::Steady;
    }
    let mut first: Vec<i64> = sorted_by_date[..half].to_vec();
    let mut second: Vec<i64> = sorted_by_date[n - half..].to_vec();
    first.sort_unstable();
    second.sort_unstable();
    let fm = median(&first) as f64;
    let sm = median(&second) as f64;
    if sm > fm * 1.1 {
        Trend::Rising
    } else if sm < fm * 0.9 {
        Trend::Falling
    } else {
        Trend::Steady
    }
}

/// Detect merchants that recur on a regular cadence, WITHOUT the fixed-amount
/// constraint of `detect`. Groups Spending outflows by normalized merchant, keys
/// cadence off the behavioral date, and reports the amount range/median and
/// trend. Powers the rhythm roster.
pub fn detect_rhythms(txns: &[Transaction]) -> Vec<RhythmEntry> {
    let mut groups: HashMap<String, Vec<&Transaction>> = HashMap::new();
    for t in txns
        .iter()
        .filter(|t| t.amount.is_outflow() && t.flag == TxnFlag::Spending)
    {
        let key = normalize_payee(t.merchant());
        if key.is_empty() {
            continue;
        }
        groups.entry(key).or_default().push(t);
    }

    let mut out = Vec::new();

    for (merchant, mut items) in groups {
        if items.len() < MIN_OCCURRENCES {
            continue;
        }
        items.sort_by_key(|t| t.effective_date());

        let dates: Vec<i64> = items.iter().map(|t| t.effective_date()).collect();
        let cadence = match classify_cadence(&dates) {
            Some(c) => c,
            None => continue,
        };

        let amounts_by_date: Vec<i64> = items.iter().map(|t| t.amount.cents().abs()).collect();
        let amount_min = *amounts_by_date.iter().min().unwrap();
        let amount_max = *amounts_by_date.iter().max().unwrap();
        let mut sorted = amounts_by_date.clone();
        sorted.sort_unstable();
        let med = median(&sorted);
        let monthly_estimate = match cadence {
            Cadence::Monthly => med,
            Cadence::Yearly => med / 12,
        };
        let trend = trend_of(&amounts_by_date);
        let last_seen = *dates.last().unwrap();

        out.push(RhythmEntry {
            merchant,
            cadence,
            occurrence_count: items.len(),
            median_amount_cents: med,
            amount_min_cents: amount_min,
            amount_max_cents: amount_max,
            monthly_estimate_cents: monthly_estimate,
            last_seen,
            trend,
        });
    }

    out.sort_by(|a, b| b.monthly_estimate_cents.cmp(&a.monthly_estimate_cents));
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
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
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

    #[test]
    fn detect_ignores_flagged_recurring_payment() {
        let mut txns = vec![
            charge("1", 0, -100000, "Amex Payment"),
            charge("2", 30, -100000, "Amex Payment"),
            charge("3", 60, -100000, "Amex Payment"),
        ];
        // A regular, fixed-amount card payment would otherwise look exactly like a
        // subscription; the flag keeps it out.
        for t in &mut txns {
            t.flag = TxnFlag::CardPayment;
        }
        assert!(detect(&txns).is_empty());
    }

    #[test]
    fn rhythm_detects_variable_amount_recurring_merchant() {
        let txns = vec![
            charge("1", 0, -1000, "PG&E"),
            charge("2", 30, -1200, "PG&E"),
            charge("3", 60, -1500, "PG&E"),
            charge("4", 91, -1800, "PG&E"),
        ];
        // The fixed-amount detector rejects this (amounts vary too much)...
        assert!(detect(&txns).is_empty());
        // ...but the rhythm detector surfaces it.
        let r = detect_rhythms(&txns);
        assert_eq!(r.len(), 1);
        let e = &r[0];
        assert_eq!(e.merchant, "pg e");
        assert_eq!(e.cadence, Cadence::Monthly);
        assert_eq!(e.occurrence_count, 4);
        assert_eq!(e.amount_min_cents, 1000);
        assert_eq!(e.amount_max_cents, 1800);
        assert_eq!(e.median_amount_cents, 1500);
        assert_eq!(e.monthly_estimate_cents, 1500);
        assert_eq!(e.trend, Trend::Rising);
    }

    #[test]
    fn rhythm_yearly_estimate_is_prorated_and_ignores_non_spending() {
        let mut txns = vec![
            charge("1", 0, -12000, "Insurance Co"),
            charge("2", 365, -12000, "Insurance Co"),
            charge("3", 730, -12000, "Insurance Co"),
        ];
        let yearly = detect_rhythms(&txns);
        assert_eq!(yearly.len(), 1);
        assert_eq!(yearly[0].cadence, Cadence::Yearly);
        assert_eq!(yearly[0].monthly_estimate_cents, 1000); // 12000 / 12

        // Flagging them out removes the entry entirely.
        for t in &mut txns {
            t.flag = TxnFlag::Transfer;
        }
        assert!(detect_rhythms(&txns).is_empty());
    }
}
