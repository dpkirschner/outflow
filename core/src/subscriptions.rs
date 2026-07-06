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

/// How often a stream recurs, classified from the median inter-occurrence
/// interval. Distinct from `Cadence` (Monthly/Yearly, used by `detect`) — a
/// stream can be daily or weekly. The UI maps each variant to a badge label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StreamCadence {
    Daily,       // "~daily"
    FewPerWeek,  // "~3×/wk"
    Weekly,      // "~weekly"
    FewPerMonth, // "~4×/mo"
    Monthly,     // "monthly"
    Yearly,      // "yearly"
}

/// Direction of a stream's monthly-rate change, derived from `trend_pct` for
/// coloring (Rising = spending more).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Trend {
    Rising,
    Falling,
    Steady,
}

/// A recurring merchant surfaced as a time series (the rhythm-ledger row). The
/// data contract the stream row and slide-over render directly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RhythmEntry {
    pub merchant: String,
    pub cadence: StreamCadence,
    pub occurrence_count: usize,
    pub median_amount_cents: i64,
    pub amount_min_cents: i64,
    pub amount_max_cents: i64,
    /// Average monthly spend over completed months (the in-progress month is
    /// excluded so a partial month can't deflate the rate).
    pub monthly_estimate_cents: i64,
    pub last_seen: i64,
    /// Signed pct change of the monthly rate: last completed month vs the mean of
    /// the prior three completed months. 0.0 when there isn't enough history.
    pub trend_pct: f64,
    pub trend: Trend,
    /// Per-month spend for the rhythm strip, oldest→newest (up to the last 6
    /// months of the window). The final element is the in-progress month.
    pub spark_cents: Vec<i64>,
}

const MONTH_DAYS: f64 = 30.44;
/// Number of trailing months drawn in the rhythm strip.
const SPARK_MONTHS: usize = 6;

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

/// Per-completed-month spend and per-month series for a merchant's occurrences.
/// `now` fixes the in-progress month (its `month_index`) so a partial current
/// month is excluded from the rate/trend math but still drawn in the strip.
struct MonthlySpend {
    /// completed-month totals, oldest→newest (in-progress month excluded)
    completed: Vec<i64>,
    /// last `SPARK_MONTHS` months incl. in-progress, oldest→newest
    spark: Vec<i64>,
}

fn monthly_spend(items: &[&Transaction], now: i64) -> MonthlySpend {
    use crate::query::month_index;
    let current = month_index(now);
    let mut by_month: std::collections::BTreeMap<i64, i64> = std::collections::BTreeMap::new();
    for t in items {
        let mi = month_index(t.effective_date());
        *by_month.entry(mi).or_insert(0) += t.amount.cents().abs();
    }
    if by_month.is_empty() {
        return MonthlySpend { completed: vec![], spark: vec![] };
    }
    // Dense range from first to last observed month so gaps count as $0 months.
    let first = *by_month.keys().next().unwrap();
    let last = *by_month.keys().next_back().unwrap();
    let mut dense: Vec<(i64, i64)> = Vec::new();
    for mi in first..=last {
        dense.push((mi, *by_month.get(&mi).unwrap_or(&0)));
    }
    let completed = dense
        .iter()
        .filter(|(mi, _)| *mi < current)
        .map(|(_, c)| *c)
        .collect();
    let spark = dense
        .iter()
        .rev()
        .take(SPARK_MONTHS)
        .rev()
        .map(|(_, c)| *c)
        .collect();
    MonthlySpend { completed, spark }
}

/// Monthly rate (mean of completed months) and its signed trend pct (last
/// completed month vs the mean of the three before it).
fn rate_and_trend(completed: &[i64]) -> (i64, f64) {
    if completed.is_empty() {
        return (0, 0.0);
    }
    let rate = completed.iter().sum::<i64>() / completed.len() as i64;
    if completed.len() < 2 {
        return (rate, 0.0);
    }
    let last = *completed.last().unwrap() as f64;
    let prior = &completed[completed.len().saturating_sub(4)..completed.len() - 1];
    let base = prior.iter().sum::<i64>() as f64 / prior.len() as f64;
    let pct = if base > 0.0 { (last - base) / base * 100.0 } else { 0.0 };
    (rate, pct)
}

fn trend_dir(pct: f64) -> Trend {
    if pct > 10.0 {
        Trend::Rising
    } else if pct < -10.0 {
        Trend::Falling
    } else {
        Trend::Steady
    }
}

/// Detect merchants that recur on a regular cadence, WITHOUT the fixed-amount
/// constraint of `detect` — this is what surfaces variable streams (coffee,
/// groceries, utilities) that `detect` filters out. Groups Spending outflows by
/// normalized merchant, keys cadence off the behavioral date, and reports the
/// amount range/median, monthly rate, and trend (completed months only, per
/// `now`). Sorted by monthly estimate desc.
pub fn detect_rhythms(txns: &[Transaction], now: i64) -> Vec<RhythmEntry> {
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
        let (cadence, med_gap) = match classify_stream_cadence(&dates) {
            Some(c) => c,
            None => continue,
        };

        let mut amounts: Vec<i64> = items.iter().map(|t| t.amount.cents().abs()).collect();
        let amount_min = *amounts.iter().min().unwrap();
        let amount_max = *amounts.iter().max().unwrap();
        amounts.sort_unstable();
        let med_amount = median(&amounts);

        let ms = monthly_spend(&items, now);
        let (mut monthly_estimate, trend_pct) = rate_and_trend(&ms.completed);
        // Fallback when there isn't a completed month yet: median × frequency.
        if monthly_estimate == 0 {
            let per_month = (MONTH_DAYS / med_gap.max(0.5)).min(31.0);
            monthly_estimate = (med_amount as f64 * per_month).round() as i64;
        }

        out.push(RhythmEntry {
            merchant,
            cadence,
            occurrence_count: items.len(),
            median_amount_cents: med_amount,
            amount_min_cents: amount_min,
            amount_max_cents: amount_max,
            monthly_estimate_cents: monthly_estimate,
            last_seen: *dates.last().unwrap(),
            trend_pct,
            trend: trend_dir(trend_pct),
            spark_cents: ms.spark,
        });
    }

    out.sort_by(|a, b| b.monthly_estimate_cents.cmp(&a.monthly_estimate_cents));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Money;

    // Noon on 2021-01-15 UTC — mid-month so month bucketing is tz-unambiguous.
    const BASE: i64 = 1610712000;
    // A `now` far after all fixtures, so every fixture month counts as completed.
    const LATER: i64 = BASE + 400 * DAY;

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
    fn rhythm_detects_monthly_variable_merchant_with_trend() {
        // One charge per month, Jan–Apr, rising amounts.
        let txns = vec![
            charge("1", 0, -1000, "PG&E"),
            charge("2", 31, -1200, "PG&E"),
            charge("3", 59, -1500, "PG&E"),
            charge("4", 90, -1800, "PG&E"),
        ];
        // The fixed-amount detector rejects this (amounts vary too much)...
        assert!(detect(&txns).is_empty());
        // ...but the rhythm detector surfaces it.
        let r = detect_rhythms(&txns, LATER);
        assert_eq!(r.len(), 1);
        let e = &r[0];
        assert_eq!(e.merchant, "pg e");
        assert_eq!(e.cadence, StreamCadence::Monthly);
        assert_eq!(e.occurrence_count, 4);
        assert_eq!(e.amount_min_cents, 1000);
        assert_eq!(e.amount_max_cents, 1800);
        assert_eq!(e.median_amount_cents, 1500);
        // Rate = mean of the four completed months.
        assert_eq!(e.monthly_estimate_cents, (1000 + 1200 + 1500 + 1800) / 4);
        // Last month (1800) vs mean of prior three (1233) → rising.
        assert_eq!(e.trend, Trend::Rising);
        assert!(e.trend_pct > 0.0);
    }

    #[test]
    fn rhythm_surfaces_a_daily_variable_stream() {
        // The case the old detector could NOT see: ~daily, variable amount.
        // (Starbucks/grocery in the demo DB.) 20 consecutive days.
        let mut txns = Vec::new();
        for d in 0..20 {
            let cents = -(400 + (d % 5) * 30); // $4.00–$5.20, varying
            txns.push(charge(&format!("c{d}"), d, cents, "Starbucks"));
        }
        assert!(detect(&txns).is_empty(), "fixed-amount detector must not see it");
        let r = detect_rhythms(&txns, LATER);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].merchant, "starbucks");
        assert_eq!(r[0].cadence, StreamCadence::Daily);
        assert_eq!(r[0].occurrence_count, 20);
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
        let r = detect_rhythms(&txns, LATER);
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
        assert!(detect_rhythms(&txns, LATER).is_empty());
    }

    #[test]
    fn rhythm_ignores_non_spending() {
        let mut txns = vec![
            charge("1", 0, -5000, "Amex Payment"),
            charge("2", 30, -5200, "Amex Payment"),
            charge("3", 60, -4800, "Amex Payment"),
            charge("4", 90, -5100, "Amex Payment"),
        ];
        assert_eq!(detect_rhythms(&txns, LATER).len(), 1);
        for t in &mut txns {
            t.flag = TxnFlag::CardPayment;
        }
        assert!(detect_rhythms(&txns, LATER).is_empty());
    }
}
