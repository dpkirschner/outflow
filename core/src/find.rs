//! Find money: one [`Finding`] type for everything that would save or earn
//! money, from several sources. Today the only source is card offers
//! ([`crate::perks`]); card choice and subscriptions come next (ADR 0001,
//! amendment 2026-11-09).
//!
//! Every estimate is in dollars per year, read off the 12 calendar months
//! ending with the chosen month (the same window as the recurring list).
//! Findings under [`MIN_EST_CENTS_PER_YEAR`] are dropped.

use crate::model::{Account, Transaction};
use crate::period::{Period, PeriodKind};
use crate::perks::{self, CardOffers, CardStatus, Offer, PerksError};
use crate::store::Store;
use chrono::{Datelike, Months, NaiveDate};
use serde::Serialize;
use std::path::Path;

/// Findings estimated under this (cents per year) are dropped. Until earn
/// tables give points a value (ho-2mj.16), a point counts as one cent for
/// this floor only; points are never converted anywhere else.
pub const MIN_EST_CENTS_PER_YEAR: i64 = 2_500;

/// How many calendar months the estimates read.
pub const WINDOW_MONTHS: u32 = 12;

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
}

impl FindingKind {
    /// The wire spelling, also the first part of a [`Finding::key`].
    pub fn as_str(self) -> &'static str {
        match self {
            FindingKind::AddOffer => "add_offer",
            FindingKind::WrongCard => "wrong_card",
            FindingKind::Expiring => "expiring",
        }
    }
}

/// One thing that would save or earn money.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Finding {
    pub kind: FindingKind,
    /// Stable across requests and syncs: `kind:card:subject`, e.g.
    /// `add_offer:4242:fubo`. Marks (Dismissed) key off it.
    pub key: String,
    /// What it's about, for display: the offer's merchant.
    pub subject: String,
    /// The card the action is on, by its last digits.
    pub card_mask: Option<String>,
    /// Estimated cash per year, in cents.
    pub est_cents_per_year: i64,
    /// Estimated points per year, kept apart from cash.
    pub est_points_per_year: i64,
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
    /// Every offers file's card, and whether it tied to an account.
    pub cards: Vec<CardStatus>,
    /// Sources that couldn't run, in words (e.g. no offers configured).
    pub notes: Vec<String>,
    /// Highest estimate first.
    pub findings: Vec<Finding>,
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
}

impl std::fmt::Display for FindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FindError::Store(e) => write!(f, "{e}"),
            FindError::Offers(e) => write!(f, "{e}"),
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
/// no offers file is configured.
pub fn evaluate(
    offers: Option<&[CardOffers]>,
    accounts: &[Account],
    txns: &[Transaction],
    through: NaiveDate,
    now: i64,
    opts: &FindOptions,
) -> FindReport {
    let window = window(through);
    let mut notes = Vec::new();
    let (cards, mut findings) = match offers {
        Some(cards) => perks::findings(cards, accounts, txns, window.bounds(), now, opts),
        None => {
            notes.push("card offers not configured: set OUTFLOW_PERKS".to_string());
            (Vec::new(), Vec::new())
        }
    };
    findings.retain(|f| f.est_cents_per_year + f.est_points_per_year >= MIN_EST_CENTS_PER_YEAR);
    findings.sort_by(|a, b| {
        b.est_cents_per_year
            .cmp(&a.est_cents_per_year)
            .then(b.est_points_per_year.cmp(&a.est_points_per_year))
            .then(days_left(a).cmp(&days_left(b)))
            .then(a.key.cmp(&b.key))
    });
    FindReport {
        window,
        cards,
        notes,
        findings,
    }
}

fn days_left(f: &Finding) -> i64 {
    match &f.evidence {
        Evidence::Offer(o) => o.days_left,
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
    Ok(evaluate(
        offers.as_deref(),
        &accounts,
        &txns,
        through,
        now,
        opts,
    ))
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

    #[test]
    fn unconfigured_offers_is_a_note_not_an_error() {
        let r = evaluate(None, &[], &[], d(2026, 11, 9), 0, &FindOptions::default());
        assert!(r.findings.is_empty());
        assert!(r.notes[0].contains("OUTFLOW_PERKS"));
    }
}
