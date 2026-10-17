//! Card offers (e.g. Amex Offers) matched against transaction history, to flag
//! value being left on the table: an available offer at a merchant you already
//! pay, an added offer whose merchant keeps getting charged to a different
//! card, or an added offer about to expire unused. Pure — offers come from a
//! hand-maintained TOML file (see `examples/amex-offers-1006.toml`), parsed and
//! validated at the boundary like Plaid JSON.

use crate::model::{mask_suffix, Account, AccountKind, Transaction, TxnFlag};
use crate::money::Money;
use crate::subscriptions::normalize_payee;
use chrono::{Local, NaiveDate, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

const DAY: i64 = 86_400;

#[derive(Debug, Clone, PartialEq)]
pub enum PerksError {
    /// Structurally bad TOML or a missing/mistyped field.
    Toml(String),
    /// A well-formed offer whose values don't make sense.
    Offer { merchant: String, msg: String },
    /// The offers file or directory couldn't be read.
    Io(String),
}

impl std::fmt::Display for PerksError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PerksError::Toml(m) => write!(f, "bad offers file: {m}"),
            PerksError::Offer { merchant, msg } => write!(f, "offer {merchant:?}: {msg}"),
            PerksError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for PerksError {}

/// What an offer pays. Cash rewards are exact cents; points are Membership
/// Rewards-style points and are never converted to dollars.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Reward {
    /// Fixed statement credit.
    Back(Money),
    /// Percent of the charge.
    Pct(u32),
    /// Flat points.
    Pts(u32),
    /// Extra points per whole dollar.
    PtsPerDollar(u32),
}

impl Reward {
    fn is_cash(self) -> bool {
        matches!(self, Reward::Back(_) | Reward::Pct(_))
    }
}

/// Total payout cap over the offer's life (or per year when `per_year`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Cap {
    Cash(Money),
    Points(u32),
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Offer {
    pub merchant: String,
    pub reward: Reward,
    /// Minimum spend per qualifying charge.
    pub min: Option<Money>,
    /// How many times the offer can trigger; `None` = until the cap is hit.
    pub uses: Option<u32>,
    pub cap: Cap,
    pub single: bool,
    pub per_year: bool,
    /// Enrolled on the card. Charges earn nothing on an offer that isn't added.
    pub added: bool,
    /// End of the expiry day, local time (epoch seconds).
    pub expires: i64,
    /// Merchant patterns, already run through `normalize_payee`.
    pub patterns: Vec<String>,
    pub note: Option<String>,
}

impl Offer {
    /// Whole-word match of any pattern against a normalized merchant that has
    /// been padded with a space on each side (see `padded`).
    fn matches(&self, padded_merchant: &str) -> bool {
        self.patterns
            .iter()
            .any(|p| padded_merchant.contains(&format!(" {p} ")))
    }
}

/// The offers loaded on (or available to) one card.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CardOffers {
    pub issuer: String,
    /// Last digits of the card, matched against the `" (1234)"` suffix
    /// `plaid.rs` bakes onto account names.
    pub mask: String,
    /// Start of the capture day, local time. Charges before this never count
    /// toward an offer (an offer only applies after it's added).
    pub captured: i64,
    pub offers: Vec<Offer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    card: RawCard,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCard {
    issuer: String,
    mask: String,
    captured: toml::value::Datetime,
    #[serde(default)]
    offers: Vec<RawOffer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOffer {
    merchant: String,
    back: Option<String>,
    pct: Option<u32>,
    pts: Option<u32>,
    pts_per_dollar: Option<u32>,
    min: Option<String>,
    uses: Option<u32>,
    max: Option<String>,
    max_pts: Option<u32>,
    #[serde(default)]
    single: bool,
    #[serde(default)]
    per_year: bool,
    #[serde(default)]
    added: bool,
    expires: toml::value::Datetime,
    #[serde(default, rename = "match")]
    patterns: Vec<String>,
    note: Option<String>,
}

fn local_date(dt: &toml::value::Datetime, what: &str) -> Result<NaiveDate, String> {
    match (dt.date, dt.time, dt.offset) {
        (Some(d), None, None) => {
            NaiveDate::from_ymd_opt(d.year as i32, d.month as u32, d.day as u32)
                .ok_or_else(|| format!("{what}: invalid date {dt}"))
        }
        _ => Err(format!(
            "{what}: expected a plain date like 2026-10-31, got {dt}"
        )),
    }
}

fn local_epoch(d: NaiveDate, h: u32, m: u32, s: u32) -> Result<i64, String> {
    let naive = d.and_hms_opt(h, m, s).ok_or("bad time")?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.timestamp())
        .ok_or_else(|| format!("unrepresentable local time for {d}"))
}

fn positive_money(s: &str, field: &str) -> Result<Money, String> {
    let m = Money::from_decimal_str(s).map_err(|e| format!("{field}: bad amount {s:?} ({e:?})"))?;
    if m.cents() <= 0 {
        return Err(format!("{field}: must be positive, got {s:?}"));
    }
    Ok(m)
}

fn convert_offer(r: RawOffer) -> Result<Offer, String> {
    let rewards = [
        r.back.is_some(),
        r.pct.is_some(),
        r.pts.is_some(),
        r.pts_per_dollar.is_some(),
    ];
    if rewards.iter().filter(|x| **x).count() != 1 {
        return Err("needs exactly one of back / pct / pts / pts_per_dollar".into());
    }
    let reward = if let Some(b) = &r.back {
        Reward::Back(positive_money(b, "back")?)
    } else if let Some(p) = r.pct {
        if !(1..=100).contains(&p) {
            return Err(format!("pct: must be 1-100, got {p}"));
        }
        Reward::Pct(p)
    } else if let Some(n) = r.pts {
        Reward::Pts(n)
    } else {
        Reward::PtsPerDollar(r.pts_per_dollar.unwrap_or_default())
    };
    if matches!(reward, Reward::Pts(0) | Reward::PtsPerDollar(0)) {
        return Err("points reward must be positive".into());
    }

    let cap = match (&r.max, r.max_pts) {
        (Some(_), Some(_)) => return Err("give max or max_pts, not both".into()),
        (Some(m), None) if reward.is_cash() => Cap::Cash(positive_money(m, "max")?),
        (Some(_), None) => return Err("a points reward is capped with max_pts, not max".into()),
        (None, Some(p)) if !reward.is_cash() => Cap::Points(p),
        (None, Some(_)) => return Err("a cash reward is capped with max, not max_pts".into()),
        (None, None) => Cap::None,
    };

    let min = r
        .min
        .as_deref()
        .map(|m| positive_money(m, "min"))
        .transpose()?;
    if r.uses == Some(0) {
        return Err("uses: must be at least 1".into());
    }
    let expires = local_epoch(local_date(&r.expires, "expires")?, 23, 59, 59)?;

    let mut patterns = Vec::new();
    for p in &r.patterns {
        let n = normalize_payee(p);
        if n.is_empty() {
            return Err(format!("match: pattern {p:?} normalizes to nothing"));
        }
        if !patterns.contains(&n) {
            patterns.push(n);
        }
    }

    Ok(Offer {
        merchant: r.merchant.trim().to_string(),
        reward,
        min,
        uses: r.uses,
        cap,
        single: r.single,
        per_year: r.per_year,
        added: r.added,
        expires,
        patterns,
        note: r.note.filter(|n| !n.trim().is_empty()),
    })
}

/// Parse and validate one offers file. Every offer is checked; the first bad
/// one is reported by merchant name.
pub fn parse_offers(src: &str) -> Result<CardOffers, PerksError> {
    let raw: RawFile = toml::from_str(src).map_err(|e| PerksError::Toml(e.to_string()))?;
    let card = raw.card;
    let mask = card.mask.trim().to_string();
    if mask.is_empty() {
        return Err(PerksError::Toml("card.mask is empty".into()));
    }
    let captured = local_date(&card.captured, "card.captured")
        .and_then(|d| local_epoch(d, 0, 0, 0))
        .map_err(PerksError::Toml)?;
    let mut offers = Vec::with_capacity(card.offers.len());
    for r in card.offers {
        let merchant = r.merchant.trim().to_string();
        if merchant.is_empty() {
            return Err(PerksError::Toml("an offer has an empty merchant".into()));
        }
        offers.push(convert_offer(r).map_err(|msg| PerksError::Offer { merchant, msg })?);
    }
    Ok(CardOffers {
        issuer: card.issuer.trim().to_string(),
        mask,
        captured,
        offers,
    })
}

/// Load a single offers file, or every `*.toml` in a directory (sorted by
/// name, one card per file). Errors name the file that failed.
pub fn load_offers(path: &Path) -> Result<Vec<CardOffers>, PerksError> {
    let io = |p: &Path, e: std::io::Error| PerksError::Io(format!("{}: {e}", p.display()));
    let files: Vec<_> = if path.is_dir() {
        let mut v: Vec<_> = std::fs::read_dir(path)
            .map_err(|e| io(path, e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "toml"))
            .collect();
        v.sort();
        v
    } else {
        vec![path.to_path_buf()]
    };
    files
        .iter()
        .map(|f| {
            let src = std::fs::read_to_string(f).map_err(|e| io(f, e))?;
            parse_offers(&src).map_err(|e| PerksError::Io(format!("{}: {e}", f.display())))
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    /// Not added, and you already spend at this merchant.
    AddOffer,
    /// Added, but the merchant was charged to a different card.
    WrongCard,
    /// Added, expiring soon, and it hasn't triggered on the card.
    Expiring,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Finding {
    pub kind: FindingKind,
    pub card_mask: String,
    pub offer: Offer,
    /// Whole days until expiry (0 on the last day).
    pub days_left: i64,
    /// The charges behind this finding: lookback history for `AddOffer` and
    /// `Expiring`, the off-card charges for `WrongCard`.
    pub charges: usize,
    pub last_charge: Option<i64>,
    pub typical_charge_cents: Option<i64>,
    /// What the offer would have paid on those charges (`WrongCard`: what it
    /// would have added beyond what the card already earned).
    pub est_cash_cents: i64,
    pub est_points: i64,
    pub txn_ids: Vec<String>,
    /// Distinct raw merchant strings the patterns caught — for checking them.
    pub matched_merchants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CardStatus {
    pub issuer: String,
    pub mask: String,
    pub account_id: Option<String>,
    /// Why the card couldn't be tied to an account (then only `AddOffer`
    /// findings are produced for it).
    pub problem: Option<String>,
    pub offers: usize,
    pub added: usize,
    pub expired: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PerksReport {
    pub cards: Vec<CardStatus>,
    /// Highest estimated cash first, then soonest expiry.
    pub findings: Vec<Finding>,
}

pub struct PerkOptions {
    /// History window start for "do you shop here" (`None` = all history).
    pub since: Option<i64>,
    /// Warn about added offers expiring within this many days.
    pub expiring_days: i64,
}

impl Default for PerkOptions {
    fn default() -> Self {
        PerkOptions {
            since: None,
            expiring_days: 14,
        }
    }
}

/// The card's account: names ending in `" (<mask>)"`. Credit accounts win a
/// tie with other kinds; any remaining ambiguity is reported, not guessed.
fn resolve_card(mask: &str, accounts: &[Account]) -> Result<String, String> {
    let hits: Vec<&Account> = accounts
        .iter()
        .filter(|a| mask_suffix(&a.name).map(|m| m.trim_matches(['(', ')'])) == Some(mask))
        .collect();
    let credit: Vec<&Account> = hits
        .iter()
        .copied()
        .filter(|a| a.kind == AccountKind::Credit)
        .collect();
    let pick = if credit.is_empty() { hits } else { credit };
    match pick.as_slice() {
        [a] => Ok(a.id.clone()),
        [] => Err(format!("no account name ends in ({mask})")),
        many => Err(format!(
            "{} accounts end in ({mask}): {}",
            many.len(),
            many.iter()
                .map(|a| a.id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[derive(Default)]
struct Accrual {
    cash_cents: i64,
    points: i64,
}

/// Replay charges (oldest first) through an offer's min / uses / cap rules.
fn accrue<'a>(offer: &Offer, charges: impl IntoIterator<Item = &'a Transaction>) -> Accrual {
    let mut acc = Accrual::default();
    let mut uses_left = offer.uses.map(i64::from);
    for t in charges {
        if uses_left == Some(0) {
            break;
        }
        let amt = t.amount.cents().abs();
        if offer.min.is_some_and(|m| amt < m.cents()) {
            continue;
        }
        let (cash, pts) = match offer.reward {
            Reward::Back(b) => (b.cents(), 0),
            Reward::Pct(p) => (amt * i64::from(p) / 100, 0),
            Reward::Pts(n) => (0, i64::from(n)),
            Reward::PtsPerDollar(n) => (0, amt / 100 * i64::from(n)),
        };
        acc.cash_cents += cash;
        acc.points += pts;
        match offer.cap {
            Cap::Cash(c) => acc.cash_cents = acc.cash_cents.min(c.cents()),
            Cap::Points(c) => acc.points = acc.points.min(i64::from(c)),
            Cap::None => {}
        }
        if let Some(u) = uses_left.as_mut() {
            *u -= 1;
        }
    }
    acc
}

fn padded(t: &Transaction) -> String {
    format!(" {} ", normalize_payee(t.merchant()))
}

fn finding(
    kind: FindingKind,
    card: &CardOffers,
    offer: &Offer,
    now: i64,
    charges: &[&Transaction],
    est: Accrual,
) -> Finding {
    let mut amounts: Vec<i64> = charges.iter().map(|t| t.amount.cents().abs()).collect();
    amounts.sort_unstable();
    let merchants: BTreeSet<String> = charges.iter().map(|t| t.merchant().to_string()).collect();
    Finding {
        kind,
        card_mask: card.mask.clone(),
        offer: offer.clone(),
        days_left: (offer.expires - now).max(0) / DAY,
        charges: charges.len(),
        last_charge: charges.iter().map(|t| t.effective_date()).max(),
        typical_charge_cents: amounts.get(amounts.len() / 2).copied(),
        est_cash_cents: est.cash_cents,
        est_points: est.points,
        txn_ids: charges.iter().map(|t| t.id.clone()).collect(),
        matched_merchants: merchants.into_iter().collect(),
    }
}

/// Match every card's live offers against the transaction history.
pub fn evaluate(
    cards: &[CardOffers],
    accounts: &[Account],
    txns: &[Transaction],
    now: i64,
    opts: &PerkOptions,
) -> PerksReport {
    // Real spending only, oldest first, merchant normalized once.
    let mut spend: Vec<(&Transaction, String)> = txns
        .iter()
        .filter(|t| t.amount.is_outflow() && t.flag == TxnFlag::Spending)
        .map(|t| (t, padded(t)))
        .collect();
    spend.sort_by_key(|(t, _)| t.effective_date());

    let mut statuses = Vec::new();
    let mut findings = Vec::new();
    for card in cards {
        let account = resolve_card(&card.mask, accounts);
        statuses.push(CardStatus {
            issuer: card.issuer.clone(),
            mask: card.mask.clone(),
            account_id: account.as_ref().ok().cloned(),
            problem: account.as_ref().err().cloned(),
            offers: card.offers.len(),
            added: card.offers.iter().filter(|o| o.added).count(),
            expired: card.offers.iter().filter(|o| o.expires < now).count(),
        });

        for offer in card.offers.iter().filter(|o| o.expires >= now) {
            let matched: Vec<&Transaction> = spend
                .iter()
                .filter(|(_, m)| offer.matches(m))
                .map(|(t, _)| *t)
                .collect();
            let history: Vec<&Transaction> = matched
                .iter()
                .copied()
                .filter(|t| opts.since.is_none_or(|s| t.effective_date() >= s))
                .collect();

            if !offer.added {
                if !history.is_empty() {
                    let est = accrue(offer, history.iter().copied());
                    findings.push(finding(
                        FindingKind::AddOffer,
                        card,
                        offer,
                        now,
                        &history,
                        est,
                    ));
                }
                continue;
            }
            let Ok(card_account) = &account else { continue };

            let live: Vec<&Transaction> = matched
                .iter()
                .copied()
                .filter(|t| (card.captured..=offer.expires).contains(&t.effective_date()))
                .collect();
            let (on_card, off_card): (Vec<&Transaction>, Vec<&Transaction>) =
                live.iter().partition(|t| &t.account_id == card_account);

            if !off_card.is_empty() {
                // Missed value = what the offer pays on all live charges beyond
                // what the on-card ones already earn (uses/cap are shared).
                let all = accrue(offer, live.iter().copied());
                let earned = accrue(offer, on_card.iter().copied());
                let est = Accrual {
                    cash_cents: all.cash_cents - earned.cash_cents,
                    points: all.points - earned.points,
                };
                // Nothing missed (offer already maxed, or charges under min).
                if est.cash_cents > 0 || est.points > 0 {
                    findings.push(finding(
                        FindingKind::WrongCard,
                        card,
                        offer,
                        now,
                        &off_card,
                        est,
                    ));
                }
            }
            let triggered = accrue(offer, on_card.iter().copied());
            let days_left = (offer.expires - now) / DAY;
            if triggered.cash_cents == 0 && triggered.points == 0 && days_left <= opts.expiring_days
            {
                let est = accrue(offer, history.iter().copied());
                findings.push(finding(
                    FindingKind::Expiring,
                    card,
                    offer,
                    now,
                    &history,
                    est,
                ));
            }
        }
    }

    findings.sort_by(|a, b| {
        b.est_cash_cents
            .cmp(&a.est_cash_cents)
            .then(a.offer.expires.cmp(&b.offer.expires))
            .then(a.offer.merchant.cmp(&b.offer.merchant))
    });
    PerksReport {
        cards: statuses,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::default_source;

    fn day(y: i32, m: u32, d: u32) -> i64 {
        local_epoch(NaiveDate::from_ymd_opt(y, m, d).unwrap(), 12, 0, 0).unwrap()
    }

    fn acct(id: &str, name: &str, kind: AccountKind) -> Account {
        Account {
            id: id.into(),
            org: "Bank".into(),
            name: name.into(),
            kind,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: default_source(),
            nickname: None,
            hidden: false,
        }
    }

    fn txn(id: &str, acct: &str, at: i64, cents: i64, merchant: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: acct.into(),
            posted: at,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: String::new(),
            source: default_source(),
        }
    }

    fn accounts() -> Vec<Account> {
        vec![
            acct("amex", "Platinum Card (1006)", AccountKind::Credit),
            acct("visa", "Sapphire (4707)", AccountKind::Credit),
        ]
    }

    fn card(offers: &str) -> CardOffers {
        parse_offers(&format!(
            "[card]\nissuer = \"American Express\"\nmask = \"1006\"\ncaptured = 2026-10-17\noffers = [\n{offers}\n]"
        ))
        .unwrap()
    }

    fn offer_err(line: &str) -> String {
        let src = format!(
            "[card]\nissuer = \"X\"\nmask = \"1\"\ncaptured = 2026-10-17\noffers = [ {{ merchant = \"M\", expires = 2026-12-31, {line} }} ]"
        );
        match parse_offers(&src) {
            Err(PerksError::Offer { msg, .. }) => msg,
            other => panic!("expected an offer error, got {other:?}"),
        }
    }

    #[test]
    fn sample_file_parses() {
        let c = parse_offers(include_str!("../../examples/amex-offers-1006.toml")).unwrap();
        assert_eq!(c.mask, "1006");
        assert_eq!(c.offers.len(), 90);
        assert!(c.offers.iter().all(|o| !o.added));
        let fubo = c.offers.iter().find(|o| o.merchant == "Fubo").unwrap();
        assert_eq!(fubo.reward, Reward::Back(Money::from_cents(3000)));
        assert_eq!(fubo.min, Some(Money::from_cents(6500)));
        assert_eq!(fubo.uses, Some(3));
        assert_eq!(fubo.cap, Cap::Cash(Money::from_cents(9000)));
    }

    #[test]
    fn rejects_bad_offers() {
        assert!(offer_err("back = \"5\", pct = 10").contains("exactly one"));
        assert!(offer_err("max = \"5\"").contains("exactly one"));
        assert!(offer_err("back = \"5\", max_pts = 10").contains("max, not max_pts"));
        assert!(offer_err("pts = 10, max = \"5\"").contains("max_pts, not max"));
        assert!(offer_err("back = \"5.x\"").contains("bad amount"));
        assert!(offer_err("pct = 0").contains("1-100"));
        assert!(offer_err("pct = 5, uses = 0").contains("at least 1"));
        assert!(offer_err("pct = 5, match = [\"***\"]").contains("normalizes to nothing"));
    }

    #[test]
    fn rejects_bad_dates_and_unknown_keys() {
        let base = "[card]\nissuer = \"X\"\nmask = \"1\"\n";
        assert!(parse_offers(&format!("{base}captured = 2026-10-17T10:00:00")).is_err());
        assert!(parse_offers(&format!("{base}captured = 2026-10-17\ncolor = \"gold\"")).is_err());
        assert!(parse_offers(&format!(
            "{base}captured = 2026-10-17\noffers = [ {{ merchant = \"M\", pct = 5, expires = 2026-12-31, typo = 1 }} ]"
        ))
        .is_err());
    }

    #[test]
    fn patterns_are_normalized_and_whole_word() {
        let c =
            card(r#"{ merchant = "CVS", pct = 10, expires = 2026-12-31, match = ["CVS", "cvs"] }"#);
        let o = &c.offers[0];
        assert_eq!(o.patterns, vec!["cvs".to_string()]);
        assert!(o.matches(&padded(&txn("a", "x", 0, -100, "CVS/PHARMACY #1234"))));
        assert!(!o.matches(&padded(&txn("b", "x", 0, -100, "CVSX OUTLET"))));
        let c =
            card(r#"{ merchant = "pura", pct = 4, expires = 2026-12-31, match = ["pura.com"] }"#);
        assert!(c.offers[0].matches(&padded(&txn("c", "x", 0, -100, "PURA.COM"))));
    }

    #[test]
    fn resolves_card_by_mask() {
        assert_eq!(resolve_card("1006", &accounts()), Ok("amex".into()));
        assert!(resolve_card("9999", &accounts())
            .unwrap_err()
            .contains("no account"));
        let mut accts = accounts();
        accts.push(acct("chk", "Checking (1006)", AccountKind::Checking));
        assert_eq!(resolve_card("1006", &accts), Ok("amex".into()));
        accts.push(acct("amex2", "Gold Card (1006)", AccountKind::Credit));
        assert!(resolve_card("1006", &accts)
            .unwrap_err()
            .contains("2 accounts"));
    }

    #[test]
    fn add_offer_for_a_merchant_you_already_pay() {
        let c = card(
            r#"{ merchant = "Fubo", back = "30.00", min = "65.00", uses = 3, max = "90.00", expires = 2026-11-07, match = ["fubo"] },
               { merchant = "MUJI", back = "20.00", min = "100.00", uses = 1, max = "20.00", expires = 2026-10-22, match = ["muji"] }"#,
        );
        let txns = vec![
            txn("f1", "visa", day(2026, 7, 1), -8499, "FUBO TV"),
            txn("f2", "visa", day(2026, 8, 1), -8499, "FUBO TV"),
            txn("f3", "visa", day(2026, 9, 1), -8499, "FUBO TV"),
            txn("f4", "visa", day(2026, 10, 1), -8499, "FUBO TV"),
            txn("other", "visa", day(2026, 10, 1), -5000, "SPROUTS"),
        ];
        let r = evaluate(
            &[c],
            &accounts(),
            &txns,
            day(2026, 10, 17),
            &PerkOptions::default(),
        );
        assert_eq!(r.findings.len(), 1);
        let f = &r.findings[0];
        assert_eq!(f.kind, FindingKind::AddOffer);
        assert_eq!(f.charges, 4);
        // 3 uses × $30, capped at $90.
        assert_eq!(f.est_cash_cents, 9000);
        assert_eq!(f.typical_charge_cents, Some(8499));
        assert_eq!(f.matched_merchants, vec!["FUBO TV".to_string()]);
        assert_eq!(f.days_left, 21);
    }

    #[test]
    fn lookback_window_and_non_spending_are_ignored() {
        let c = card(
            r#"{ merchant = "IKEA", pct = 7, max = "27.00", expires = 2026-10-31, match = ["ikea"] }"#,
        );
        let mut transfer = txn("t", "visa", day(2026, 9, 1), -10000, "IKEA");
        transfer.flag = TxnFlag::Transfer;
        let txns = vec![
            txn("old", "visa", day(2025, 1, 1), -10000, "IKEA"),
            txn("refund", "visa", day(2026, 9, 1), 10000, "IKEA"),
            transfer,
        ];
        let opts = PerkOptions {
            since: Some(day(2025, 10, 17)),
            ..PerkOptions::default()
        };
        let r = evaluate(&[c], &accounts(), &txns, day(2026, 10, 17), &opts);
        assert!(r.findings.is_empty());
    }

    #[test]
    fn pct_rounds_down_and_caps() {
        let c = card(
            r#"{ merchant = "IKEA", pct = 7, max = "27.00", expires = 2026-10-31, match = ["ikea"] }"#,
        );
        let one = vec![txn("a", "visa", day(2026, 9, 1), -1999, "IKEA")];
        let r = evaluate(
            std::slice::from_ref(&c),
            &accounts(),
            &one,
            day(2026, 10, 17),
            &PerkOptions::default(),
        );
        assert_eq!(r.findings[0].est_cash_cents, 139); // 1999 × 7% = 139.93
        let big = vec![txn("b", "visa", day(2026, 9, 1), -100000, "IKEA")];
        let r = evaluate(
            &[c],
            &accounts(),
            &big,
            day(2026, 10, 17),
            &PerkOptions::default(),
        );
        assert_eq!(r.findings[0].est_cash_cents, 2700);
    }

    #[test]
    fn wrong_card_after_capture_only() {
        let c = card(
            r#"{ merchant = "Dropbox", pct = 40, max = "30.00", added = true, expires = 2026-11-25, match = ["dropbox"] }"#,
        );
        let txns = vec![
            txn("before", "visa", day(2026, 10, 1), -1199, "DROPBOX"),
            txn("after", "visa", day(2026, 10, 20), -1199, "DROPBOX"),
        ];
        let r = evaluate(
            &[c],
            &accounts(),
            &txns,
            day(2026, 10, 25),
            &PerkOptions::default(),
        );
        assert_eq!(r.findings.len(), 1);
        let f = &r.findings[0];
        assert_eq!(f.kind, FindingKind::WrongCard);
        assert_eq!(f.txn_ids, vec!["after".to_string()]);
        assert_eq!(f.est_cash_cents, 479);
    }

    #[test]
    fn no_wrong_card_once_the_card_has_maxed_the_offer() {
        let c = card(
            r#"{ merchant = "Whatnot", back = "5.00", min = "25.00", uses = 2, max = "10.00", added = true, expires = 2026-11-24, match = ["whatnot"] }"#,
        );
        let txns = vec![
            txn("a1", "amex", day(2026, 10, 18), -3000, "WHATNOT"),
            txn("a2", "amex", day(2026, 10, 19), -3000, "WHATNOT"),
            txn("v1", "visa", day(2026, 10, 20), -3000, "WHATNOT"),
        ];
        let r = evaluate(
            &[c],
            &accounts(),
            &txns,
            day(2026, 10, 25),
            &PerkOptions::default(),
        );
        // Both uses were already earned on the card: nothing was missed.
        assert!(r.findings.is_empty());
    }

    #[test]
    fn expiring_when_added_and_untriggered() {
        let c = card(
            r#"{ merchant = "CVS", pct = 10, max = "5.00", added = true, expires = 2026-10-31, match = ["cvs"] },
               { merchant = "Zenni", back = "20.00", min = "60.00", uses = 1, max = "20.00", added = true, expires = 2027-03-30, match = ["zenni"] }"#,
        );
        let r = evaluate(
            std::slice::from_ref(&c),
            &accounts(),
            &[],
            day(2026, 10, 25),
            &PerkOptions::default(),
        );
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].kind, FindingKind::Expiring);
        assert_eq!(r.findings[0].offer.merchant, "CVS");
        assert_eq!(r.findings[0].days_left, 6);

        // Triggered on the card → no warning. Expired → ignored entirely.
        let used = vec![txn("c", "amex", day(2026, 10, 20), -2000, "CVS")];
        let r = evaluate(
            std::slice::from_ref(&c),
            &accounts(),
            &used,
            day(2026, 10, 25),
            &PerkOptions::default(),
        );
        assert!(r.findings.is_empty());
        let r = evaluate(
            &[c],
            &accounts(),
            &[],
            day(2026, 11, 1),
            &PerkOptions::default(),
        );
        assert!(r.findings.is_empty());
        assert_eq!(r.cards[0].expired, 1);
    }

    #[test]
    fn unresolved_card_still_suggests_offers() {
        let c = card(
            r#"{ merchant = "CVS", pct = 10, max = "5.00", expires = 2026-10-31, match = ["cvs"] },
               { merchant = "Zenni", back = "20.00", added = true, expires = 2026-10-31, match = ["zenni"] }"#,
        );
        let accts = vec![acct("visa", "Sapphire (4707)", AccountKind::Credit)];
        let txns = vec![
            txn("c", "visa", day(2026, 10, 1), -2000, "CVS"),
            txn("z", "visa", day(2026, 10, 20), -9000, "ZENNI"),
        ];
        let r = evaluate(
            &[c],
            &accts,
            &txns,
            day(2026, 10, 25),
            &PerkOptions::default(),
        );
        assert!(r.cards[0].problem.is_some());
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].kind, FindingKind::AddOffer);
    }

    #[test]
    fn findings_sort_by_cash_then_expiry() {
        let c = card(
            r#"{ merchant = "Small", back = "5.00", expires = 2026-10-20, match = ["small"] },
               { merchant = "Late", back = "50.00", expires = 2026-12-31, match = ["late"] },
               { merchant = "Soon", back = "50.00", expires = 2026-11-01, match = ["soon"] }"#,
        );
        let txns = vec![
            txn("1", "visa", day(2026, 9, 1), -1000, "SMALL"),
            txn("2", "visa", day(2026, 9, 1), -1000, "LATE"),
            txn("3", "visa", day(2026, 9, 1), -1000, "SOON"),
        ];
        let r = evaluate(
            &[c],
            &accounts(),
            &txns,
            day(2026, 10, 17),
            &PerkOptions::default(),
        );
        let order: Vec<&str> = r
            .findings
            .iter()
            .map(|f| f.offer.merchant.as_str())
            .collect();
        assert_eq!(order, vec!["Soon", "Late", "Small"]);
    }
}
