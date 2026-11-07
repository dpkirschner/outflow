//! The one merchant normalizer (`normalize_payee`, invariant #4) and the one
//! recurring detector (`detect_rhythms`, GLOSSARY: Recurring). The recurring
//! list built on it lives in `recurring`.

use crate::model::{Transaction, TxnFlag};
use serde::Serialize;
use std::collections::HashMap;

const DAY: i64 = 86400;
const MIN_OCCURRENCES: usize = 3;
pub(crate) const MONTH_DAYS: f64 = 30.44;

fn is_processor_prefix(head: &str) -> bool {
    matches!(
        head.trim(),
        "sq" | "tst" | "sp" | "pp" | "py" | "pypl" | "paypal"
    )
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

pub(crate) fn median(sorted: &[i64]) -> i64 {
    sorted[sorted.len() / 2]
}

/// How often a recurring merchant charges, classified from the median gap
/// between charges. The UI maps each variant to a label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum StreamCadence {
    Daily,       // "~daily"
    FewPerWeek,  // "~3×/wk"
    Weekly,      // "weekly"
    FewPerMonth, // "~4×/mo"
    Monthly,     // "monthly"
    Yearly,      // "yearly"
}

fn f64_median(sorted: &[f64]) -> f64 {
    sorted[sorted.len() / 2]
}

/// Classify a stream's cadence from consecutive gaps, and confirm the spacing is
/// *consistent* (not merely a loosened fixed-window match): a stream qualifies
/// only if a majority of gaps land within `[0.5×, 2×]` the median gap — tolerant
/// of a daily-coffee weekend skip, but rejecting an erratic scatter. Returns the
/// cadence label and the median gap in days.
fn classify_stream_cadence(sorted_dates: &[i64]) -> Option<(StreamCadence, f64)> {
    if sorted_dates.len() < 2 {
        return None;
    }
    let mut gaps: Vec<f64> = sorted_dates
        .windows(2)
        .map(|w| (w[1] - w[0]) as f64 / DAY as f64)
        .collect();
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = f64_median(&gaps);
    if med <= 0.0 {
        return None;
    }
    let within = gaps
        .iter()
        .filter(|g| **g >= med * 0.5 && **g <= med * 2.0)
        .count();
    // Majority of intervals cluster around the median → genuinely regular.
    if within * 5 < gaps.len() * 3 {
        return None;
    }
    let cadence = if med <= 1.5 {
        StreamCadence::Daily
    } else if med <= 3.5 {
        StreamCadence::FewPerWeek
    } else if med <= 10.0 {
        StreamCadence::Weekly
    } else if med <= 20.0 {
        StreamCadence::FewPerMonth
    } else if med <= 45.0 {
        StreamCadence::Monthly
    } else {
        StreamCadence::Yearly
    };
    Some((cadence, med))
}

/// A merchant that passed the detector: its Spending outflows sorted by
/// behavioral date, the cadence, and the median gap in days.
pub struct RhythmGroup<'a> {
    /// Normalized merchant (`normalize_payee`).
    pub merchant: String,
    pub items: Vec<&'a Transaction>,
    pub cadence: StreamCadence,
    pub med_gap: f64,
}

/// The recurring detector: group Spending outflows by normalized merchant and
/// keep merchants with at least `MIN_OCCURRENCES` charges spaced regularly
/// (amounts may vary, so groceries and utilities count, not just fixed
/// subscriptions). Unordered.
pub fn detect_rhythms(txns: &[Transaction]) -> Vec<RhythmGroup<'_>> {
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
        let Some((cadence, med_gap)) = classify_stream_cadence(&dates) else {
            continue;
        };
        out.push(RhythmGroup {
            merchant,
            items,
            cadence,
            med_gap,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Money;

    // Noon on 2021-01-15 UTC.
    const BASE: i64 = 1610712000;

    fn charge(id: &str, day_offset: i64, cents: i64, merchant: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct".into(),
            posted: BASE + day_offset * DAY,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            pfc_primary: None,
            pfc_detailed: None,
            source: crate::model::default_source(),
        }
    }

    #[test]
    fn normalizes_merchant_noise() {
        assert_eq!(normalize_payee("SQ *BLUE BOTTLE 4471"), "blue bottle");
        assert_eq!(normalize_payee("AMZN Mktp US*2X4K9"), "amzn mktp us");
        assert_eq!(normalize_payee("NETFLIX.COM"), "netflix com");
    }

    #[test]
    fn groups_across_merchant_noise_with_varying_amounts() {
        let txns = vec![
            charge("1", 0, -1000, "SQ *PG&E 001"),
            charge("2", 31, -1200, "SQ *PG&E 002"),
            charge("3", 59, -1500, "SQ *PG&E 003"),
        ];
        let r = detect_rhythms(&txns);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].merchant, "pg e");
        assert_eq!(r[0].cadence, StreamCadence::Monthly);
        assert_eq!(r[0].items.len(), 3);
    }

    #[test]
    fn surfaces_a_daily_variable_stream() {
        let txns: Vec<_> = (0..20)
            .map(|d| charge(&format!("c{d}"), d, -(400 + (d % 5) * 30), "Starbucks"))
            .collect();
        let r = detect_rhythms(&txns);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].cadence, StreamCadence::Daily);
    }

    #[test]
    fn rhythm_tolerates_irregular_gaps() {
        // Daily coffee that skips weekends: gaps mostly 1, occasionally 3. Still a
        // stream — consistency is tolerant, not exact-period.
        let offsets = [0, 1, 2, 5, 6, 7, 8, 9, 12, 13, 14, 15, 16];
        let txns: Vec<_> = offsets
            .iter()
            .enumerate()
            .map(|(i, o)| charge(&format!("c{i}"), *o, -500, "Blue Bottle"))
            .collect();
        let r = detect_rhythms(&txns);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].cadence, StreamCadence::Daily);
    }

    #[test]
    fn rhythm_rejects_erratic_scatter() {
        // Three purchases at wildly different spacings — not a rhythm.
        let txns = vec![
            charge("1", 0, -1200, "Corner Store"),
            charge("2", 3, -4500, "Corner Store"),
            charge("3", 47, -800, "Corner Store"),
        ];
        assert!(detect_rhythms(&txns).is_empty());
    }

    #[test]
    fn rhythm_ignores_non_spending() {
        let mut txns = vec![
            charge("1", 0, -5000, "Amex Payment"),
            charge("2", 30, -5200, "Amex Payment"),
            charge("3", 60, -4800, "Amex Payment"),
            charge("4", 90, -5100, "Amex Payment"),
        ];
        assert_eq!(detect_rhythms(&txns).len(), 1);
        for t in &mut txns {
            t.flag = TxnFlag::CardPayment;
        }
        assert!(detect_rhythms(&txns).is_empty());
    }

    #[test]
    fn ignores_inflows_and_moved_money() {
        let pay = vec![
            charge("1", 0, 200000, "Payroll"),
            charge("2", 30, 200000, "Payroll"),
            charge("3", 60, 200000, "Payroll"),
        ];
        assert!(detect_rhythms(&pay).is_empty());
        let mut card: Vec<_> = (0..4)
            .map(|i| charge(&format!("p{i}"), i * 30, -5000, "Amex Payment"))
            .collect();
        assert_eq!(detect_rhythms(&card).len(), 1);
        for t in &mut card {
            t.flag = TxnFlag::CardPayment;
        }
        assert!(detect_rhythms(&card).is_empty());
    }
}
