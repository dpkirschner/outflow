//! Card choice, one source of [`crate::find`] Findings: spending charged to a
//! card that earns less on its category than another card you hold.
//!
//! Each card's offers file may carry an `[earn]` table (see
//! `examples/amex-offers-4242.toml`): a `default` rate, per-category `rates`
//! keyed on Plaid's `pfc_detailed` or `pfc_primary` (detailed wins), an
//! optional `cents_per_point`, and optional yearly spend `caps`. A card with
//! no `cents_per_point` is cash back and its rates are percent.
//!
//! Every card's earn is valued in cents (points at the card's own
//! `cents_per_point`), so cards in different currencies compare. The gap for a
//! category is what the 12-month window's Out would have earned split across
//! the cards in the best order the caps allow, minus what it earned where it
//! was charged. Rotating quarterly categories, annual fees and sign-up
//! bonuses are not modelled.

use crate::find::{self, Evidence, Finding, FindingKind};
use crate::model::{Account, Transaction, TxnFlag};
use crate::money::Money;
use crate::perks::{resolve_card, CardOffers};
use serde::{Deserialize, Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A non-negative decimal with up to three places (an earn rate, a point
/// value), held exactly in thousandths. On the wire it is a decimal string
/// such as `"1.5"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Rate(u32);

impl Rate {
    pub const ONE: Rate = Rate(1_000);

    pub fn from_thousandths(t: u32) -> Self {
        Rate(t)
    }

    pub fn thousandths(self) -> u32 {
        self.0
    }

    /// `"4"`, `"1.5"`, `"1.25"`; at most three decimal places, never a sign.
    pub fn parse(s: &str) -> Result<Rate, String> {
        let t = s.trim();
        let (int, frac) = t.split_once('.').unwrap_or((t, ""));
        let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
            return Err(format!("{s:?} is not a number like 1.5"));
        }
        if frac.len() > 3 {
            return Err(format!("{s:?} has more than three decimal places"));
        }
        let whole: u32 = if int.is_empty() {
            0
        } else {
            int.parse().map_err(|_| format!("{s:?} is too large"))?
        };
        let frac: u32 = format!("{frac:0<3}").parse().unwrap_or(0);
        whole
            .checked_mul(1_000)
            .and_then(|w| w.checked_add(frac))
            .filter(|t| *t <= 1_000_000)
            .map(Rate)
            .ok_or_else(|| format!("{s:?} is too large"))
    }

    /// A TOML integer, float or string. A float goes through its shortest
    /// decimal spelling, so `1.5` reads as exactly 1.5.
    fn from_toml(v: &toml::Value) -> Result<Rate, String> {
        match v {
            toml::Value::Integer(i) if *i >= 0 => Rate::parse(&i.to_string()),
            toml::Value::Float(f) if *f >= 0.0 => Rate::parse(&f.to_string()),
            toml::Value::String(s) => Rate::parse(s),
            toml::Value::Integer(_) | toml::Value::Float(_) => {
                Err("must be non-negative".to_string())
            }
            other => Err(format!("expected a number, got a {}", other.type_str())),
        }
    }
}

impl std::fmt::Display for Rate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (w, r) = (self.0 / 1_000, self.0 % 1_000);
        if r == 0 {
            write!(f, "{w}")
        } else {
            let frac = format!("{r:03}");
            write!(f, "{w}.{}", frac.trim_end_matches('0'))
        }
    }
}

impl Serialize for Rate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

#[cfg(feature = "ts")]
impl ts_rs::TS for Rate {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;
    fn name(_: &ts_rs::Config) -> String {
        "string".to_owned()
    }
    fn inline(cfg: &ts_rs::Config) -> String {
        <Self as ts_rs::TS>::name(cfg)
    }
}

/// What a card earns, per Plaid category.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EarnTable {
    /// What one point is worth to you, in cents. `None` = cash back: the
    /// rates are percent, which is the same as points worth 1¢.
    pub cents_per_point: Option<Rate>,
    /// The rate for a category the table doesn't name.
    pub default: Rate,
    /// Rate by Plaid category (`FOOD_AND_DRINK` or
    /// `FOOD_AND_DRINK_GROCERIES`).
    pub rates: BTreeMap<String, Rate>,
    /// Yearly spend earning a category's rate; past it, `default`.
    pub caps: BTreeMap<String, Money>,
}

impl EarnTable {
    /// Cents per point (1 for cash back).
    pub fn point_value(&self) -> Rate {
        self.cents_per_point.unwrap_or(Rate::ONE)
    }

    /// The first of `keys` the table names, with its rate, else `default`.
    fn lookup<'k>(&self, keys: &[&'k str]) -> (Rate, Option<&'k str>) {
        keys.iter()
            .find_map(|k| self.rates.get(*k).map(|r| (*r, Some(*k))))
            .unwrap_or((self.default, None))
    }

    /// Value of one cent of spend at `rate`, in units of 1e-8 cents.
    fn units(&self, rate: Rate) -> i128 {
        i128::from(rate.0) * i128::from(self.point_value().0)
    }
}

/// `[earn]` as written in an offers file.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawEarn {
    cents_per_point: Option<toml::Value>,
    default: toml::Value,
    #[serde(default)]
    rates: BTreeMap<String, toml::Value>,
    #[serde(default)]
    caps: BTreeMap<String, String>,
}

fn is_category(k: &str) -> bool {
    k.starts_with(|c: char| c.is_ascii_uppercase())
        && k.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub(crate) fn convert(raw: RawEarn) -> Result<EarnTable, String> {
    let cents_per_point = raw
        .cents_per_point
        .as_ref()
        .map(|v| Rate::from_toml(v).map_err(|e| format!("cents_per_point: {e}")))
        .transpose()?;
    if cents_per_point == Some(Rate(0)) {
        return Err("cents_per_point: must be positive".into());
    }
    let default = Rate::from_toml(&raw.default).map_err(|e| format!("default: {e}"))?;
    let mut rates = BTreeMap::new();
    for (k, v) in &raw.rates {
        if !is_category(k) {
            return Err(format!(
                "rates: {k:?} is not a Plaid category like FOOD_AND_DRINK_GROCERIES"
            ));
        }
        rates.insert(
            k.clone(),
            Rate::from_toml(v).map_err(|e| format!("rates.{k}: {e}"))?,
        );
    }
    let mut caps = BTreeMap::new();
    for (k, v) in &raw.caps {
        if !rates.contains_key(k) {
            return Err(format!("caps: {k} has no rate in rates"));
        }
        let m = Money::from_decimal_str(v)
            .ok()
            .filter(|m| m.cents() > 0)
            .ok_or_else(|| format!("caps.{k}: {v:?} is not a positive amount"))?;
        caps.insert(k.clone(), m);
    }
    Ok(EarnTable {
        cents_per_point,
        default,
        rates,
        caps,
    })
}

/// One card's side of a card choice Finding.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CardEarn {
    pub issuer: String,
    pub mask: String,
    /// The window's Out in this category on this card.
    pub spend_cents: i64,
    pub charges: usize,
    /// Points per dollar here (percent for a cash back card).
    pub rate: Rate,
    /// `None` = cash back.
    pub cents_per_point: Option<Rate>,
    /// What a dollar earns here at `rate`, in hundredths of a percent.
    pub return_bp: i64,
    /// Yearly spend left at `rate` for this category, when it is capped
    /// (the cap less this card's spend in other categories sharing it).
    pub cap_left_cents: Option<i64>,
    /// What this card earned on its own spend here, in cents.
    pub earned_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EarnEvidence {
    /// The Plaid category the cards' rates were read for (detailed or
    /// primary, the finest any card names), or `default` for spending no
    /// card names.
    pub category: String,
    /// Every card with an earn table, highest `return_bp` first.
    pub cards: Vec<CardEarn>,
}

struct Usable<'a> {
    card: &'a CardOffers,
    account: String,
    earn: &'a EarnTable,
}

/// The rate lookup for a group: detailed first, then its primary.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Group {
    key: String,
    primary: Option<String>,
}

impl Group {
    fn keys(&self) -> Vec<&str> {
        match (&self.key, &self.primary) {
            (k, _) if k == DEFAULT => vec![],
            (k, Some(p)) => vec![k.as_str(), p.as_str()],
            (k, None) => vec![k.as_str()],
        }
    }
}

const DEFAULT: &str = "default";
/// `units` is 1e-8 of a cent per cent spent.
const UNITS_PER_CENT: i128 = 100_000_000;

/// Charges in one category: the finest Plaid key any card names, else the
/// primary, else `default`, so every card's rate is the same across a group.
fn group_of(t: &Transaction, known: &BTreeSet<&str>) -> Group {
    let d = t.pfc_detailed.as_deref().filter(|d| known.contains(d));
    let p = t.pfc_primary.as_deref().filter(|p| known.contains(p));
    match (d, p) {
        (Some(d), _) => Group {
            key: d.to_string(),
            primary: t.pfc_primary.clone(),
        },
        (None, Some(p)) => Group {
            key: p.to_string(),
            primary: None,
        },
        (None, None) => Group {
            key: DEFAULT.to_string(),
            primary: None,
        },
    }
}

/// "FOOD_AND_DRINK_GROCERIES" → "Food and drink groceries".
fn label(key: &str) -> String {
    if key == DEFAULT {
        return "Everything else".to_string();
    }
    let lower = key.to_lowercase().replace('_', " ");
    let mut c = lower.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

/// Card choice Findings and the notes on cards left out. `window` is
/// `[since, until)` epoch seconds. Unsorted and unfloored; `find` does both.
pub(crate) fn findings(
    cards: &[CardOffers],
    accounts: &[Account],
    txns: &[Transaction],
    window: (i64, i64),
) -> (Vec<Finding>, Vec<String>) {
    let mut notes = Vec::new();
    let without: Vec<&str> = cards
        .iter()
        .filter(|c| c.earn.is_none())
        .map(|c| c.mask.as_str())
        .collect();
    if without.len() == cards.len() {
        notes.push("card choice: no offers file has an [earn] table".to_string());
        return (Vec::new(), notes);
    }
    if !without.is_empty() {
        notes.push(format!(
            "card choice leaves out cards with no [earn] table: {}",
            without.join(", ")
        ));
    }
    let mut usable = Vec::new();
    for card in cards {
        let Some(earn) = &card.earn else { continue };
        match resolve_card(&card.mask, accounts) {
            Ok(account) => usable.push(Usable {
                card,
                account,
                earn,
            }),
            Err(e) => notes.push(format!("card choice leaves out {}: {e}", card.mask)),
        }
    }
    if usable.len() < 2 {
        notes.push("card choice needs two cards with [earn] tables tied to accounts".to_string());
        return (Vec::new(), notes);
    }

    let known: BTreeSet<&str> = usable
        .iter()
        .flat_map(|u| u.earn.rates.keys().map(String::as_str))
        .collect();
    let by_account: HashMap<&str, usize> = usable
        .iter()
        .enumerate()
        .map(|(i, u)| (u.account.as_str(), i))
        .collect();

    // group → per-card charges.
    let mut groups: BTreeMap<Group, Vec<Vec<&Transaction>>> = BTreeMap::new();
    for t in txns {
        let in_window = (window.0..window.1).contains(&t.effective_date());
        if !(in_window && t.flag == TxnFlag::Spending && t.amount.is_outflow()) {
            continue;
        }
        let Some(&i) = by_account.get(t.account_id.as_str()) else {
            continue;
        };
        groups
            .entry(group_of(t, &known))
            .or_insert_with(|| vec![Vec::new(); usable.len()])[i]
            .push(t);
    }
    let spend = |v: &[&Transaction]| v.iter().map(|t| t.amount.cents().abs()).sum::<i64>();

    // Each card's own spend per capped key, across every group sharing it.
    let mut capped_spend: HashMap<(usize, String), i64> = HashMap::new();
    for (g, per_card) in &groups {
        for (i, u) in usable.iter().enumerate() {
            if let (_, Some(k)) = u.earn.lookup(&g.keys()) {
                if u.earn.caps.contains_key(k) {
                    *capped_spend.entry((i, k.to_string())).or_default() += spend(&per_card[i]);
                }
            }
        }
    }

    let mut out = Vec::new();
    for (g, per_card) in &groups {
        let keys = g.keys();
        let mut rows = Vec::new();
        // (value units per cent, capacity in cents (None = unlimited), card)
        let mut tiers: Vec<(i128, Option<i64>, usize)> = Vec::new();
        let mut actual: i128 = 0;
        for (i, u) in usable.iter().enumerate() {
            let s = spend(&per_card[i]);
            let (rate, key) = u.earn.lookup(&keys);
            let left = key.and_then(|k| {
                let cap = u.earn.caps.get(k)?.cents();
                let elsewhere = capped_spend[&(i, k.to_string())] - s;
                Some((cap - elsewhere).max(0))
            });
            let bonus = u.earn.units(rate);
            let base = u.earn.units(u.earn.default);
            let at_rate = left.map_or(s, |l| s.min(l));
            let earned = i128::from(at_rate) * bonus + i128::from(s - at_rate) * base;
            actual += earned;
            tiers.push((bonus, left, i));
            if left.is_some() {
                tiers.push((base, None, i));
            }
            rows.push(CardEarn {
                issuer: u.card.issuer.clone(),
                mask: u.card.mask.clone(),
                spend_cents: s,
                charges: per_card[i].len(),
                rate,
                cents_per_point: u.earn.cents_per_point,
                return_bp: (bonus / 10_000) as i64,
                cap_left_cents: left,
                earned_cents: (earned / UNITS_PER_CENT) as i64,
            });
        }

        // The best split: fill the most valuable tiers first.
        tiers.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.cmp(&b.2)));
        let total: i64 = per_card.iter().map(|v| spend(v)).sum();
        let mut rest = total;
        let mut best_split: i128 = 0;
        let mut best: Option<(usize, Option<i64>)> = None;
        for (units, cap, i) in &tiers {
            if rest == 0 {
                break;
            }
            let take = cap.map_or(rest, |c| rest.min(c));
            if take == 0 {
                continue;
            }
            best.get_or_insert((*i, *cap));
            best_split += i128::from(take) * units;
            rest -= take;
        }
        let gap = ((best_split - actual) / UNITS_PER_CENT) as i64;
        let Some((b, cap)) = best else { continue };
        if gap <= 0 {
            continue;
        }

        let to = &usable[b];
        let subject = label(&g.key);
        let mut action = format!(
            "Charge {} to the {} card ending {}",
            subject.to_lowercase(),
            to.card.issuer,
            to.card.mask
        );
        if let Some(c) = cap.filter(|c| *c < total) {
            action.push_str(&format!(
                ", up to ${} a year at that rate",
                Money::from_cents(c).to_display()
            ));
        }
        let txn_ids = per_card
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != b)
            .flat_map(|(_, v)| v.iter().map(|t| t.id.clone()))
            .collect();
        rows.sort_by(|x, y| y.return_bp.cmp(&x.return_bp).then(x.mask.cmp(&y.mask)));
        out.push(Finding {
            kind: FindingKind::CardChoice,
            key: find::key(FindingKind::CardChoice, &to.card.mask, &subject),
            subject,
            card_mask: Some(to.card.mask.clone()),
            est_cents_per_year: gap,
            est_points_per_year: 0,
            est_value_cents_per_year: gap,
            txn_ids,
            action,
            evidence: Evidence::Earn(EarnEvidence {
                category: g.key.clone(),
                cards: rows,
            }),
        });
    }
    (out, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{default_source, AccountKind};
    use crate::perks::parse_offers;

    #[test]
    fn rates_parse_exactly() {
        assert_eq!(Rate::parse("1.5"), Ok(Rate(1_500)));
        assert_eq!(Rate::parse("4"), Ok(Rate(4_000)));
        assert_eq!(Rate::parse(".25"), Ok(Rate(250)));
        assert!(Rate::parse("1.2345").is_err());
        assert!(Rate::parse("-1").is_err());
        assert!(Rate::parse("x").is_err());
        assert_eq!(Rate(1_250).to_string(), "1.25");
        assert_eq!(Rate(3_000).to_string(), "3");
        assert_eq!(Rate::from_toml(&toml::Value::Float(1.1)), Ok(Rate(1_100)));
    }

    fn card(mask: &str, earn: &str) -> CardOffers {
        parse_offers(&format!(
            "[card]\nissuer = \"Issuer {mask}\"\nmask = \"{mask}\"\ncaptured = 2026-10-01\n{earn}"
        ))
        .unwrap()
    }

    fn earn_err(earn: &str) -> String {
        match parse_offers(&format!(
            "[card]\nissuer = \"X\"\nmask = \"1\"\ncaptured = 2026-10-01\n[earn]\n{earn}"
        )) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("expected an error for {earn}"),
        }
    }

    #[test]
    fn rejects_bad_earn_tables() {
        assert!(earn_err("cents_per_point = 1").contains("default"));
        assert!(earn_err("default = 1\nrates = { dining = 3 }").contains("Plaid category"));
        assert!(earn_err("default = 1\ncaps = { TRAVEL = \"10.00\" }").contains("no rate"));
        assert!(
            earn_err("default = 1\nrates = { TRAVEL = 2 }\ncaps = { TRAVEL = \"0\" }")
                .contains("positive")
        );
        assert!(earn_err("default = 1\ncents_per_point = 0").contains("positive"));
        assert!(earn_err("default = 1\ncolor = 2").contains("unknown field"));
        assert!(earn_err("default = -1").contains("non-negative"));
    }

    fn acct(id: &str, mask: &str) -> Account {
        Account {
            id: id.into(),
            org: "Bank".into(),
            name: format!("Card ({mask})"),
            kind: AccountKind::Credit,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: default_source(),
            nickname: None,
            hidden: false,
            item_id: None,
            kind_from_bank: None,
        }
    }

    fn txn(id: &str, acct: &str, cents: i64, primary: &str, detailed: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: acct.into(),
            posted: 1_000,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: "SAMPLE SHOP".into(),
            payee: None,
            category: None,
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: String::new(),
            pfc_primary: Some(primary.into()),
            pfc_detailed: Some(detailed.into()),
            source: default_source(),
        }
    }

    const W: (i64, i64) = (0, 10_000);
    const GROCERY: (&str, &str) = ("FOOD_AND_DRINK", "FOOD_AND_DRINK_GROCERIES");
    const DINING: (&str, &str) = ("FOOD_AND_DRINK", "FOOD_AND_DRINK_RESTAURANT");
    const GAS: (&str, &str) = ("TRANSPORTATION", "TRANSPORTATION_GAS");

    fn points_card() -> CardOffers {
        card(
            "4242",
            "[earn]\ncents_per_point = 1.25\ndefault = 1\nrates = { FOOD_AND_DRINK = 3, FOOD_AND_DRINK_GROCERIES = 4 }",
        )
    }

    fn cash_card() -> CardOffers {
        card("5555", "[earn]\ndefault = 2")
    }

    fn accounts() -> Vec<Account> {
        vec![acct("a", "4242"), acct("b", "5555"), acct("c", "0000")]
    }

    fn by_subject<'a>(f: &'a [Finding], s: &str) -> &'a Finding {
        f.iter().find(|f| f.subject == s).unwrap()
    }

    #[test]
    fn gap_is_best_rate_minus_earned_per_category() {
        let t = |id, a, c, (p, d): (&str, &str)| txn(id, a, c, p, d);
        let txns = vec![
            // Groceries on the 2% card: the 4x at 1.25¢ card pays 5%.
            t("g1", "b", -100_000, GROCERY),
            t("g2", "b", -50_000, GROCERY),
            // Dining (FOOD_AND_DRINK primary): 3x × 1.25 = 3.75% vs 2%.
            t("d1", "b", -80_000, DINING),
            // Gas on the points card: 1.25% there, 2% on the cash card.
            t("s1", "a", -40_000, GAS),
            // Already on the best card: no gap.
            t("g3", "a", -20_000, GROCERY),
        ];
        let (f, notes) = findings(&[points_card(), cash_card()], &accounts(), &txns, W);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(f.len(), 3);

        let g = by_subject(&f, "Food and drink groceries");
        assert_eq!(g.kind, FindingKind::CardChoice);
        assert_eq!(g.key, "card_choice:4242:food-and-drink-groceries");
        assert_eq!(g.card_mask.as_deref(), Some("4242"));
        // $1,500 × (5% − 2%) = $45.
        assert_eq!(g.est_cents_per_year, 4_500);
        assert_eq!(g.est_value_cents_per_year, 4_500);
        assert_eq!(g.est_points_per_year, 0);
        assert_eq!(g.txn_ids, vec!["g1".to_string(), "g2".to_string()]);
        let Evidence::Earn(e) = &g.evidence else {
            panic!()
        };
        assert_eq!(e.category, "FOOD_AND_DRINK_GROCERIES");
        assert_eq!(e.cards[0].mask, "4242");
        assert_eq!(e.cards[0].return_bp, 500);
        assert_eq!(e.cards[0].rate, Rate(4_000));
        assert_eq!(e.cards[1].return_bp, 200);
        assert_eq!(e.cards[1].spend_cents, 150_000);
        assert_eq!(e.cards[1].earned_cents, 3_000);

        // The restaurant charge groups under FOOD_AND_DRINK: no card names
        // the detailed category.
        let d = by_subject(&f, "Food and drink");
        assert_eq!(d.est_cents_per_year, 1_400); // $800 × 1.75%
        let s = by_subject(&f, "Everything else");
        assert_eq!(s.key, "card_choice:5555:everything-else");
        assert_eq!(s.est_cents_per_year, 300); // $400 × 0.75%, floored later
    }

    #[test]
    fn caps_limit_what_the_best_card_takes() {
        let capped = card(
            "4242",
            "[earn]\ndefault = 1\nrates = { FOOD_AND_DRINK = 6 }\ncaps = { FOOD_AND_DRINK = \"1000.00\" }",
        );
        // The cash card names groceries, so they group apart from dining,
        // but both draw on the capped card's FOOD_AND_DRINK cap.
        let cash = card(
            "5555",
            "[earn]\ndefault = 2\nrates = { FOOD_AND_DRINK_GROCERIES = 2 }",
        );
        let txns = vec![
            txn("own", "a", -30_000, DINING.0, DINING.1),
            txn("off", "b", -200_000, GROCERY.0, GROCERY.1),
        ];
        let (f, _) = findings(&[capped, cash], &accounts(), &txns, W);
        assert_eq!(f.len(), 1);
        let g = &f[0];
        // $700 of the cap is left for groceries. Best split of $2,000: $700
        // at 6%, $1,300 at 2% = $68. Earned: $2,000 at 2% = $40. Gap $28.
        assert_eq!(g.est_cents_per_year, 2_800);
        let Evidence::Earn(e) = &g.evidence else {
            panic!()
        };
        assert_eq!(e.cards[0].cap_left_cents, Some(70_000));
        assert!(
            g.action.ends_with("up to $700.00 a year at that rate"),
            "{}",
            g.action
        );
    }

    #[test]
    fn cards_without_tables_are_left_out_and_said() {
        let bare = card("0000", "");
        let txns = vec![txn("x", "c", -500_000, GROCERY.0, GROCERY.1)];
        let (f, notes) = findings(
            &[points_card(), cash_card(), bare.clone()],
            &accounts(),
            &txns,
            W,
        );
        assert!(f.is_empty());
        assert_eq!(
            notes,
            vec!["card choice leaves out cards with no [earn] table: 0000".to_string()]
        );

        let (_, notes) = findings(&[bare], &accounts(), &txns, W);
        assert!(notes[0].contains("no offers file has an [earn] table"));

        let (_, notes) = findings(
            &[points_card(), card("9999", "[earn]\ndefault = 1")],
            &accounts(),
            &txns,
            W,
        );
        assert!(notes[0].contains("leaves out 9999: no account name ends in (9999)"));
        assert!(notes[1].contains("needs two cards"));
    }

    #[test]
    fn only_spending_out_in_the_window_counts() {
        let mut moved = txn("m", "b", -900_000, GROCERY.0, GROCERY.1);
        moved.flag = TxnFlag::Transfer;
        let mut old = txn("o", "b", -900_000, GROCERY.0, GROCERY.1);
        old.posted = 20_000;
        let refund = txn("r", "b", 900_000, GROCERY.0, GROCERY.1);
        let (f, _) = findings(
            &[points_card(), cash_card()],
            &accounts(),
            &[moved, old, refund],
            W,
        );
        assert!(f.is_empty());
    }
}
