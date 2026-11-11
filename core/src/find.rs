//! Find money: one [`Finding`] type for everything that would save or earn
//! money, from several sources: card offers ([`crate::perks`]), card
//! choice ([`crate::earn`]) and subscriptions ([`crate::recurring`]) (ADR
//! 0001, amendment 2026-11-09).
//!
//! Every estimate is in dollars per year, read off the 12 calendar months
//! ending with the chosen month (the same window as the recurring list).
//! Findings whose [`Finding::est_value_cents_per_year`] is under
//! [`MIN_EST_CENTS_PER_YEAR`] are dropped.
//!
//! A Finding marked Dismissed moves to [`FindReport::dismissed`] and comes
//! back on its own only when its estimate more than doubles the one it had
//! when dismissed ([`apply_dismissed`]). A subscription or price rise whose
//! merchant is marked Reviewed moves to [`FindReport::reviewed`] for a year,
//! or until its price rises past the one reviewed ([`apply_reviewed`]).

use crate::earn::{self, EarnEvidence};
use crate::model::{Account, Transaction};
use crate::period::{Period, PeriodKind};
use crate::perks::{self, CardOffers, CardStatus, Offer, PerksError};
use crate::recurring::{self, PriceRise, STEADY_PCT};
use crate::store::{DismissedMark, ReviewedMark, Store};
use crate::subscriptions::{normalize_payee, StreamCadence};
use chrono::{Datelike, Months, NaiveDate};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Findings whose value (cash plus points at the card's earn-table
/// `cents_per_point`, one cent when its file has no `[earn]` table) is under
/// this many cents per year are dropped. Expiring offers included.
pub const MIN_EST_CENTS_PER_YEAR: i64 = 2_500;

/// How many calendar months the estimates read.
pub const WINDOW_MONTHS: u32 = 12;

/// A Reviewed mark hides its merchant's Finding for this many days.
pub const REVIEWED_DAYS: i64 = 365;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// A card offer you haven't added, at a merchant you already pay.
    AddOffer,
    /// An added offer whose merchant was charged to a different card.
    WrongCard,
    /// An added offer about to expire before it has paid out.
    Expiring,
    /// A category charged to a card that earns less on it than another
    /// card you hold.
    CardChoice,
    /// A steady recurring merchant (GLOSSARY: Steady) to review. outflow
    /// can't see usage, so it never says unused.
    Subscription,
    /// A steady recurring merchant whose price just went up.
    PriceRise,
}

impl FindingKind {
    /// The wire spelling, also the first part of a [`Finding::key`].
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::AddOffer => "add_offer",
            FindingKind::WrongCard => "wrong_card",
            FindingKind::Expiring => "expiring",
            FindingKind::CardChoice => "card_choice",
            FindingKind::Subscription => "subscription",
            FindingKind::PriceRise => "price_rise",
        }
    }
}

/// One thing that would save or earn money.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Finding {
    pub kind: FindingKind,
    /// Stable across requests and syncs: `kind:card:subject`, e.g.
    /// `add_offer:4242:fubo`. Card choice and subscription kinds leave the
    /// card out (`card_choice::food-and-drink-groceries`,
    /// `subscription::streamflix`), so a mark survives a new best card or a
    /// merchant moving cards. A price rise adds the day it began
    /// (`price_rise::streamflix:2026-10-10`), so a later rise is new.
    /// Dismissed keys off it.
    pub key: String,
    /// What it's about, for display: the offer's or subscription's
    /// merchant, or the category for card choice.
    pub subject: String,
    /// The card the action is on, by its last digits (for a subscription,
    /// the card most charges land on, when its name ends in one).
    pub card_mask: Option<String>,
    /// Estimated cash per year, in cents. Card choice compares cards in
    /// different currencies, so its whole estimate is here, points already
    /// valued at each card's `cents_per_point`.
    pub est_cents_per_year: i64,
    /// Estimated points per year, kept apart from cash (offers only).
    pub est_points_per_year: i64,
    /// Cash plus points valued at the card's `cents_per_point` (one cent
    /// without an earn table). The floor and the ranking read this.
    pub est_value_cents_per_year: i64,
    /// The transactions behind the estimate.
    pub txn_ids: Vec<String>,
    /// The one thing to do about it.
    pub action: String,
    pub evidence: Evidence,
}

/// The source-specific detail behind a Finding.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Evidence {
    Offer(OfferEvidence),
    Earn(EarnEvidence),
    Subscription(SubscriptionEvidence),
}

/// Behind a subscription or price rise Finding: the recurring merchant.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct SubscriptionEvidence {
    /// Normalized merchant (`RecurringMerchant::merchant`): what the
    /// Reviewed and Ignored marks key on.
    pub merchant: String,
    /// Display name.
    pub name: String,
    /// Monthly or yearly.
    pub cadence: StreamCadence,
    /// The charge now: the steady charge, or the new price after a rise.
    pub charge_cents: i64,
    /// Charges a year at this cadence (12 or 1); the estimate is the charge
    /// (or the rise) times this.
    pub per_year: i64,
    pub price_rise: Option<PriceRise>,
    /// Behavioral date of the last charge in the window, epoch seconds.
    pub last_charge: i64,
    /// The account most charges land on.
    pub account_id: String,
    /// When the merchant was marked Reviewed; set only on
    /// `FindReport.reviewed`.
    pub reviewed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct OfferEvidence {
    pub issuer: String,
    pub offer: Offer,
    /// Whole days until expiry (0 on the last day).
    pub days_left: i64,
    /// Charges behind the estimate: window history for `AddOffer` and
    /// `Expiring`, the off-card charges for `WrongCard`.
    pub charges: usize,
    pub last_charge: Option<i64>,
    pub typical_charge_cents: Option<i64>,
    /// Distinct raw merchant strings the patterns caught, for checking them.
    pub matched_merchants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FindReport {
    /// The months the estimates read.
    pub window: Period,
    /// Every offers file's card, whether it tied to an account, and whether
    /// it has an earn table.
    pub cards: Vec<CardStatus>,
    /// Sources that couldn't run, in words (e.g. no offers configured).
    pub notes: Vec<String>,
    /// Open Findings, highest estimate first.
    pub findings: Vec<Finding>,
    /// Findings marked Dismissed, in the same order. Restoring one clears
    /// the mark.
    pub dismissed: Vec<Finding>,
    /// Subscription and price rise Findings whose merchant is marked
    /// Reviewed, in the same order. Unreviewing one clears the mark.
    pub reviewed: Vec<Finding>,
}

#[derive(Debug, Clone, Copy)]
pub struct FindOptions {
    /// Flag added offers expiring within this many days.
    pub expiring_days: i64,
}

impl Default for FindOptions {
    fn default() -> Self {
        FindOptions { expiring_days: 14 }
    }
}

#[derive(Debug)]
pub enum FindError {
    Store(rusqlite::Error),
    Offers(PerksError),
    /// Dismiss named a key that isn't among this report's Findings.
    NoSuchFinding(String),
    /// Review named a merchant with no subscription or price rise Finding in
    /// this report.
    NoSuchSubscription(String),
}

impl std::fmt::Display for FindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FindError::Store(e) => write!(f, "{e}"),
            FindError::Offers(e) => write!(f, "{e}"),
            FindError::NoSuchFinding(k) => write!(f, "no Finding with key {k}"),
            FindError::NoSuchSubscription(m) => {
                write!(f, "no subscription or price rise Finding for merchant {m}")
            }
        }
    }
}

impl std::error::Error for FindError {}

/// The 12 calendar months ending with the month that holds `through`.
pub fn window(through: NaiveDate) -> Period {
    let last = through.with_day(1).expect("day 1 exists");
    let first = last - Months::new(WINDOW_MONTHS - 1);
    let end = last + Months::new(1);
    Period {
        kind: PeriodKind::Custom,
        start: first,
        end,
    }
}

/// Every Finding, from transactions already loaded. `offers` is `None` when
/// no offers file is configured; `ignored` is the Ignored merchants, which
/// give no subscription Findings.
pub fn evaluate(
    offers: Option<&[CardOffers]>,
    accounts: &[Account],
    txns: &[Transaction],
    ignored: &HashSet<String>,
    through: NaiveDate,
    now: i64,
    opts: &FindOptions,
) -> FindReport {
    let window = window(through);
    let mut notes = Vec::new();
    let (cards, mut findings) = match offers {
        Some(cards) => {
            let (status, mut found) =
                perks::findings(cards, accounts, txns, window.bounds(), now, opts);
            let (choice, choice_notes) = earn::findings(cards, accounts, txns, window.bounds());
            found.extend(choice);
            notes.extend(choice_notes);
            (status, found)
        }
        None => {
            notes.push("card offers not configured: set OUTFLOW_PERKS".to_string());
            (Vec::new(), Vec::new())
        }
    };
    let view = recurring::build(txns, ignored, through, now);
    findings.extend(recurring::findings(&view, accounts, txns, window.bounds()));
    findings.retain(|f| f.est_value_cents_per_year >= MIN_EST_CENTS_PER_YEAR);
    findings.sort_by(|a, b| {
        b.est_value_cents_per_year
            .cmp(&a.est_value_cents_per_year)
            .then(b.est_cents_per_year.cmp(&a.est_cents_per_year))
            .then(b.est_points_per_year.cmp(&a.est_points_per_year))
            .then(days_left(a).cmp(&days_left(b)))
            .then(a.key.cmp(&b.key))
    });
    FindReport {
        window,
        cards,
        notes,
        findings,
        dismissed: Vec::new(),
        reviewed: Vec::new(),
    }
}

/// Move every subscription or price rise Finding whose merchant carries a
/// live Reviewed mark to `report.reviewed`. A mark lapses `REVIEWED_DAYS`
/// after it was set, or once the merchant's charge is more than `STEADY_PCT`
/// above the one reviewed (it went up since), so a price rise after a review
/// always shows. Stale marks stay stored and do nothing.
pub fn apply_reviewed(report: &mut FindReport, marks: &HashMap<String, ReviewedMark>, now: i64) {
    let (mut reviewed, open): (Vec<Finding>, Vec<Finding>) = std::mem::take(&mut report.findings)
        .into_iter()
        .partition(|f| live_review(f, marks, now).is_some());
    for f in &mut reviewed {
        let at = live_review(f, marks, now).map(|m| m.marked_at);
        if let Evidence::Subscription(e) = &mut f.evidence {
            e.reviewed_at = at;
        }
    }
    report.findings = open;
    report.reviewed = reviewed;
}

fn live_review<'a>(
    f: &Finding,
    marks: &'a HashMap<String, ReviewedMark>,
    now: i64,
) -> Option<&'a ReviewedMark> {
    let Evidence::Subscription(e) = &f.evidence else {
        return None;
    };
    marks.get(&e.merchant).filter(|m| {
        now < m.marked_at + REVIEWED_DAYS * 86_400
            && e.charge_cents as f64 <= m.charge_cents as f64 * (1.0 + STEADY_PCT / 100.0)
    })
}

/// Move every Finding with a Dismissed mark to `report.dismissed`, unless
/// its value now more than doubles the one it had when dismissed. Both are
/// valued like the floor, the mark's points at the Finding's `cents_per_point`
/// today, so a change in `cents_per_point` alone doesn't bring it back.
pub fn apply_dismissed(report: &mut FindReport, marks: &HashMap<String, DismissedMark>) {
    let (dismissed, open) = std::mem::take(&mut report.findings)
        .into_iter()
        .partition(|f| {
            marks
                .get(&f.key)
                .is_some_and(|m| f.est_value_cents_per_year <= 2 * value_then(f, m))
        });
    report.findings = open;
    report.dismissed = dismissed;
}

/// A Dismissed mark's estimate valued at the Finding's `cents_per_point`
/// now (its points value over its points; one cent when it has none).
fn value_then(f: &Finding, m: &DismissedMark) -> i64 {
    let points = match f.est_points_per_year {
        0 => i128::from(m.est_points_per_year),
        p => {
            i128::from(m.est_points_per_year)
                * i128::from(f.est_value_cents_per_year - f.est_cents_per_year)
                / i128::from(p)
        }
    };
    m.est_cents_per_year + points as i64
}

fn days_left(f: &Finding) -> i64 {
    match &f.evidence {
        Evidence::Offer(o) => o.days_left,
        Evidence::Earn(_) | Evidence::Subscription(_) => i64::MAX,
    }
}

/// Load the offers (a file or a folder; `None` = not configured) and the
/// archive, then [`evaluate`]. The one entry point for the server and CLI.
pub fn find(
    store: &Store,
    offers_path: Option<&Path>,
    through: NaiveDate,
    now: i64,
    opts: &FindOptions,
) -> Result<FindReport, FindError> {
    let offers = offers_path
        .map(perks::load_offers)
        .transpose()
        .map_err(FindError::Offers)?;
    let accounts = store.accounts().map_err(FindError::Store)?;
    let txns = store.all_transactions().map_err(FindError::Store)?;
    let ignored = store.ignored_merchants().map_err(FindError::Store)?;
    report_with(
        store,
        offers.as_deref(),
        &accounts,
        &txns,
        &ignored,
        through,
        now,
        opts,
    )
    .map_err(FindError::Store)
}

/// [`evaluate`] over an archive the caller already loaded, with the stored
/// Reviewed and Dismissed marks applied: what [`find`] returns, for callers
/// (check-in) that hold the transactions anyway.
#[allow(clippy::too_many_arguments)]
pub fn report_with(
    store: &Store,
    offers: Option<&[CardOffers]>,
    accounts: &[Account],
    txns: &[Transaction],
    ignored: &HashSet<String>,
    through: NaiveDate,
    now: i64,
    opts: &FindOptions,
) -> rusqlite::Result<FindReport> {
    let marks = store.dismissed_findings()?;
    let reviewed = store.reviewed_merchants()?;
    let mut report = evaluate(offers, accounts, txns, ignored, through, now, opts);
    apply_reviewed(&mut report, &reviewed, now);
    apply_dismissed(&mut report, &marks);
    Ok(report)
}

/// Dismiss the Finding with `key` (it must be in the report for these
/// params, open or dismissed), or restore it, then return the new report.
/// Dismissing stores the estimate it has now; dismissing an already
/// dismissed Finding keeps the first estimate, so the bar doesn't creep.
/// Restoring a key with no mark is a no-op.
#[allow(clippy::too_many_arguments)]
pub fn set_dismissed(
    store: &Store,
    offers_path: Option<&Path>,
    through: NaiveDate,
    now: i64,
    opts: &FindOptions,
    key: &str,
    dismissed: bool,
) -> Result<FindReport, FindError> {
    if dismissed {
        let report = find(store, offers_path, through, now, opts)?;
        if !report.dismissed.iter().any(|f| f.key == key) {
            let f = report
                .findings
                .iter()
                .find(|f| f.key == key)
                .ok_or_else(|| FindError::NoSuchFinding(key.to_string()))?;
            let mark = DismissedMark {
                est_cents_per_year: f.est_cents_per_year,
                est_points_per_year: f.est_points_per_year,
                marked_at: now,
            };
            store
                .set_finding_dismissed(key, Some(mark))
                .map_err(FindError::Store)?;
        }
    } else {
        store
            .set_finding_dismissed(key, None)
            .map_err(FindError::Store)?;
    }
    find(store, offers_path, through, now, opts)
}

/// Mark the merchant (any spelling; it is normalized) Reviewed, or clear the
/// mark, then return the new report. Marking needs a subscription or price
/// rise Finding for it in the report for these params (open, reviewed or
/// dismissed) and stores that Finding's charge and `now`; marking again
/// refreshes both (you reviewed it again). Clearing a merchant with no mark
/// is a no-op.
#[allow(clippy::too_many_arguments)]
pub fn set_reviewed(
    store: &Store,
    offers_path: Option<&Path>,
    through: NaiveDate,
    now: i64,
    opts: &FindOptions,
    merchant: &str,
    reviewed: bool,
) -> Result<FindReport, FindError> {
    let m = normalize_payee(merchant);
    if reviewed {
        let report = find(store, offers_path, through, now, opts)?;
        let charge_cents = report
            .findings
            .iter()
            .chain(&report.reviewed)
            .chain(&report.dismissed)
            .find_map(|f| match &f.evidence {
                Evidence::Subscription(e) if e.merchant == m => Some(e.charge_cents),
                _ => None,
            })
            .ok_or_else(|| FindError::NoSuchSubscription(m.clone()))?;
        let mark = ReviewedMark {
            charge_cents,
            marked_at: now,
        };
        store
            .set_merchant_reviewed(&m, Some(mark))
            .map_err(FindError::Store)?;
    } else {
        store
            .set_merchant_reviewed(&m, None)
            .map_err(FindError::Store)?;
    }
    find(store, offers_path, through, now, opts)
}

/// `kind:card:subject`, the subject normalized so a respelled merchant name
/// in the offers file keeps its key.
pub(crate) fn key(kind: FindingKind, card: &str, subject: &str) -> String {
    let slug = crate::subscriptions::normalize_payee(subject).replace(' ', "-");
    format!("{}:{card}:{slug}", kind.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn window_is_twelve_whole_months_ending_with_through() {
        let w = window(d(2026, 11, 9));
        assert_eq!((w.start, w.end), (d(2025, 12, 1), d(2026, 12, 1)));
        let w = window(d(2026, 1, 31));
        assert_eq!((w.start, w.end), (d(2025, 2, 1), d(2026, 2, 1)));
    }

    #[test]
    fn keys_are_stable_across_spelling() {
        assert_eq!(
            key(FindingKind::AddOffer, "4242", "Fubo TV"),
            key(FindingKind::AddOffer, "4242", "FUBO  tv")
        );
        assert_eq!(
            key(FindingKind::WrongCard, "4242", "Fubo TV"),
            "wrong_card:4242:fubo-tv"
        );
    }

    fn finding(key: &str, cents: i64, points: i64) -> Finding {
        Finding {
            kind: FindingKind::AddOffer,
            key: key.into(),
            subject: key.into(),
            card_mask: None,
            est_cents_per_year: cents,
            est_points_per_year: points,
            est_value_cents_per_year: cents + points,
            txn_ids: vec![],
            action: String::new(),
            evidence: Evidence::Offer(OfferEvidence {
                issuer: String::new(),
                offer: Offer {
                    merchant: key.into(),
                    reward: crate::perks::Reward::Back(crate::money::Money::from_cents(cents)),
                    min: None,
                    uses: None,
                    cap: crate::perks::Cap::None,
                    single: false,
                    per_year: false,
                    added: false,
                    expires: 0,
                    patterns: vec![],
                    note: None,
                },
                days_left: 30,
                charges: 0,
                last_charge: None,
                typical_charge_cents: None,
                matched_merchants: vec![],
            }),
        }
    }

    fn mark(cents: i64, points: i64) -> DismissedMark {
        DismissedMark {
            est_cents_per_year: cents,
            est_points_per_year: points,
            marked_at: 0,
        }
    }

    #[test]
    fn dismissed_stays_down_until_its_estimate_more_than_doubles() {
        let mut r = evaluate(
            None,
            &[],
            &[],
            &HashSet::new(),
            d(2026, 11, 9),
            0,
            &FindOptions::default(),
        );
        // Points at 2 cents: value is cash plus twice the points.
        let at_2c = |key: &str, cents: i64, points: i64| Finding {
            est_value_cents_per_year: cents + 2 * points,
            ..finding(key, cents, points)
        };
        r.findings = vec![
            finding("doubled", 6_000, 0),
            finding("exactly-double", 6_000, 0),
            finding("points", 0, 9_000),
            at_2c("points-at-2c", 0, 4_000),
            at_2c("cash-then-points", 0, 4_000),
            finding("open", 5_000, 0),
        ];
        let marks = HashMap::from([
            ("doubled".to_string(), mark(2_999, 0)),
            ("exactly-double".to_string(), mark(3_000, 0)),
            ("points".to_string(), mark(0, 4_500)),
            // 2,000 points then, 4,000 now, both at 2c: exactly double.
            ("points-at-2c".to_string(), mark(0, 2_000)),
            // $30 cash then, $80 of points now: more than double in value,
            // though 4,000 points counted as cents would not be.
            ("cash-then-points".to_string(), mark(3_000, 0)),
            ("stale".to_string(), mark(100, 0)),
        ]);
        apply_dismissed(&mut r, &marks);
        let keys = |v: &[Finding]| v.iter().map(|f| f.key.clone()).collect::<Vec<_>>();
        assert_eq!(
            keys(&r.findings),
            vec!["doubled", "cash-then-points", "open"]
        );
        assert_eq!(
            keys(&r.dismissed),
            vec!["exactly-double", "points", "points-at-2c"]
        );
    }

    #[test]
    fn unconfigured_offers_is_a_note_not_an_error() {
        let r = evaluate(
            None,
            &[],
            &[],
            &HashSet::new(),
            d(2026, 11, 9),
            0,
            &FindOptions::default(),
        );
        assert!(r.findings.is_empty());
        assert!(r.notes[0].contains("OUTFLOW_PERKS"));
    }

    // ── subscriptions and Reviewed (ho-2mj.17) ──────────────────────────

    const DAY: i64 = 86_400;

    fn noon(y: i32, m: u32, day: u32) -> i64 {
        use chrono::TimeZone;
        chrono::Local
            .with_ymd_and_hms(y, m, day, 12, 0, 0)
            .single()
            .unwrap()
            .timestamp()
    }

    /// One charge a month on the 10th, July to October 2026.
    fn sub(merchant: &str, cents: [i64; 4]) -> Vec<Transaction> {
        cents
            .iter()
            .enumerate()
            .map(|(i, c)| Transaction {
                id: format!("{merchant}-{i}"),
                account_id: "card".into(),
                posted: noon(2026, 7 + i as u32, 10),
                transacted_at: None,
                amount: crate::money::Money::from_cents(-c),
                description: merchant.into(),
                payee: Some(merchant.into()),
                category: None,
                category_source: None,
                pending: false,
                flag: crate::model::TxnFlag::Spending,
                raw: "{}".into(),
                pfc_primary: None,
                pfc_detailed: None,
                source: crate::model::default_source(),
            })
            .collect()
    }

    fn eval(txns: &[Transaction], ignored: &HashSet<String>) -> FindReport {
        evaluate(
            None,
            &[],
            txns,
            ignored,
            d(2026, 10, 31),
            noon(2026, 11, 7),
            &FindOptions::default(),
        )
    }

    #[test]
    fn subscriptions_rank_by_yearly_cost_over_the_floor_without_offers() {
        let mut txns = sub("Streamflix", [1599; 4]);
        txns.extend(sub("Demo Gym", [4000; 4]));
        // $1.99 a month is $23.88 a year: under the floor.
        txns.extend(sub("Tiny App", [199; 4]));
        let r = eval(&txns, &HashSet::new());
        let keys: Vec<&str> = r.findings.iter().map(|f| f.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["subscription::demo-gym", "subscription::streamflix"]
        );
        assert!(r.notes[0].contains("OUTFLOW_PERKS"));
        let ignored: HashSet<String> = ["demo gym".to_string()].into();
        assert_eq!(eval(&txns, &ignored).findings.len(), 1);
    }

    fn review(charge: i64, at: i64) -> ReviewedMark {
        ReviewedMark {
            charge_cents: charge,
            marked_at: at,
        }
    }

    #[test]
    fn reviewed_hides_for_a_year_or_until_the_price_rises() {
        let mut txns = sub("Streamflix", [1599; 4]);
        txns.extend(sub("Demo Gym", [4000; 4]));
        txns.extend(sub("Sample Music", [1099, 1099, 1099, 1499]));
        txns.extend(sub("Sample News", [1000; 4]));
        let now = noon(2026, 11, 7);
        let marks = HashMap::from([
            ("streamflix".to_string(), review(1599, now - 10 * DAY)),
            // Reviewed over a year ago: lapsed.
            (
                "demo gym".to_string(),
                review(4000, now - REVIEWED_DAYS * DAY),
            ),
            // Reviewed at the old price, then it rose: the rise shows.
            ("sample music".to_string(), review(1099, now - 100 * DAY)),
            // Reviewed at the new price (as a rise): stays hidden.
            ("sample news".to_string(), review(990, now - 5 * DAY)),
        ]);
        let mut r = eval(&txns, &HashSet::new());
        apply_reviewed(&mut r, &marks, now);
        let keys = |v: &[Finding]| v.iter().map(|f| f.key.clone()).collect::<Vec<_>>();
        assert_eq!(
            keys(&r.findings),
            vec![
                "subscription::demo-gym",
                "price_rise::sample-music:2026-10-10"
            ]
        );
        assert_eq!(
            keys(&r.reviewed),
            vec!["subscription::streamflix", "subscription::sample-news"]
        );
        let Evidence::Subscription(e) = &r.reviewed[0].evidence else {
            panic!("subscription evidence")
        };
        assert_eq!(e.reviewed_at, Some(now - 10 * DAY));
    }

    #[test]
    fn set_reviewed_round_trips_through_the_store() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&sub("Streamflix", [1599; 4]))
            .unwrap();
        let (through, now, o) = (d(2026, 10, 31), noon(2026, 11, 7), FindOptions::default());
        let r = set_reviewed(&s, None, through, now, &o, "STREAMFLIX", true).unwrap();
        assert!(r.findings.is_empty());
        assert_eq!(r.reviewed[0].key, "subscription::streamflix");
        assert_eq!(
            s.reviewed_merchants().unwrap()["streamflix"],
            review(1599, now)
        );
        let r = set_reviewed(&s, None, through, now, &o, "streamflix", false).unwrap();
        assert_eq!(r.findings[0].key, "subscription::streamflix");
        assert!(r.reviewed.is_empty());
        assert!(matches!(
            set_reviewed(&s, None, through, now, &o, "nobody", true),
            Err(FindError::NoSuchSubscription(m)) if m == "nobody"
        ));
        // Clearing a merchant with no mark is a no-op.
        assert!(set_reviewed(&s, None, through, now, &o, "nobody", false).is_ok());
    }
}
