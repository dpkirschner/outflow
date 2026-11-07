use crate::model::{Transaction, TxnFlag};
use crate::store::Store;
use crate::subscriptions::normalize_payee;
use chrono::{Datelike, Local, TimeZone};
use serde::Serialize;
use std::collections::HashMap;

pub struct TxnFilter {
    pub since: Option<i64>,
    pub until: Option<i64>,
    pub include_pending: bool,
    /// When false (the default), aggregations drop `Transfer`/`CardPayment`
    /// transactions so money moving between the user's own accounts isn't
    /// counted as spending. Set true for a "show transfers" view.
    pub include_non_spending: bool,
}

impl TxnFilter {
    pub fn all() -> Self {
        TxnFilter {
            since: None,
            until: None,
            include_pending: true,
            include_non_spending: false,
        }
    }

    pub fn range(since: i64, until: i64) -> Self {
        TxnFilter {
            since: Some(since),
            until: Some(until),
            include_pending: true,
            include_non_spending: false,
        }
    }
}

fn passes(t: &Transaction, f: &TxnFilter) -> bool {
    if !f.include_pending && t.pending {
        return false;
    }
    if !f.include_non_spending && t.flag != TxnFlag::Spending {
        return false;
    }
    // Filter on the behavioral date so a transaction lands in the same range it
    // is bucketed under. since inclusive, until exclusive.
    let d = t.effective_date();
    if let Some(s) = f.since {
        if d < s {
            return false;
        }
    }
    if let Some(u) = f.until {
        if d >= u {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Date,
    Amount,
    Merchant,
    Category,
}

impl SortKey {
    // Lenient DB/wire lookup (Option / total fallback), not FromStr's Result.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<SortKey> {
        match s {
            "date" => Some(SortKey::Date),
            "amount" => Some(SortKey::Amount),
            "merchant" => Some(SortKey::Merchant),
            "category" => Some(SortKey::Category),
            _ => None,
        }
    }
}

/// Search/sort/filter over the transaction list. Lives in core so the CLI and
/// the web client run the identical query semantics. All in-memory over
/// `all_transactions()` — the standing pattern, fine at personal scale.
pub struct TxnQuery {
    pub filter: TxnFilter,
    /// Case-insensitive substring over payee, description, category, and the
    /// normalized merchant.
    pub text: Option<String>,
    pub account_id: Option<String>,
    pub category: Option<String>,
    pub source: Option<String>,
    /// Keep only this flag. Filtering on Transfer/CardPayment requires
    /// `filter.include_non_spending = true` or the rows are dropped first.
    pub flag: Option<TxnFlag>,
    pub min_cents: Option<i64>,
    pub max_cents: Option<i64>,
    pub sort: SortKey,
    pub descending: bool,
    pub offset: usize,
    pub limit: usize,
}

impl Default for TxnQuery {
    fn default() -> Self {
        TxnQuery {
            filter: TxnFilter::all(),
            text: None,
            account_id: None,
            category: None,
            source: None,
            flag: None,
            min_cents: None,
            max_cents: None,
            sort: SortKey::Date,
            descending: true,
            offset: 0,
            limit: 100,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TxnPage {
    /// Matching rows before pagination.
    pub total: usize,
    /// Signed sum over ALL matching rows (not just this page).
    pub total_cents: i64,
    pub items: Vec<Transaction>,
}

pub fn search_transactions(store: &Store, q: &TxnQuery) -> rusqlite::Result<TxnPage> {
    let needle = q.text.as_deref().map(|s| s.to_lowercase());
    let mut matched: Vec<Transaction> = store
        .all_transactions()?
        .into_iter()
        .filter(|t| passes(t, &q.filter))
        .filter(|t| q.account_id.as_deref().is_none_or(|a| t.account_id == a))
        .filter(|t| {
            q.category
                .as_deref()
                .is_none_or(|c| t.category.as_deref() == Some(c))
        })
        .filter(|t| q.source.as_deref().is_none_or(|s| t.source == s))
        .filter(|t| q.flag.is_none_or(|f| t.flag == f))
        .filter(|t| q.min_cents.is_none_or(|m| t.amount.cents().abs() >= m))
        .filter(|t| q.max_cents.is_none_or(|m| t.amount.cents().abs() <= m))
        .filter(|t| {
            let Some(n) = &needle else { return true };
            t.payee.as_deref().unwrap_or("").to_lowercase().contains(n)
                || t.description.to_lowercase().contains(n)
                || t.category
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase()
                    .contains(n)
                || normalize_payee(t.merchant()).contains(n)
        })
        .collect();

    // Stable ordering: primary key, then date (newest first), then id — so
    // pagination never shuffles under equal keys.
    matched.sort_by(|a, b| {
        let primary = match q.sort {
            SortKey::Date => a.effective_date().cmp(&b.effective_date()),
            SortKey::Amount => a.amount.cents().cmp(&b.amount.cents()),
            SortKey::Merchant => normalize_payee(a.merchant()).cmp(&normalize_payee(b.merchant())),
            SortKey::Category => a.category.cmp(&b.category),
        };
        let primary = if q.descending {
            primary.reverse()
        } else {
            primary
        };
        primary
            .then_with(|| b.effective_date().cmp(&a.effective_date()))
            .then_with(|| a.id.cmp(&b.id))
    });

    let total = matched.len();
    let total_cents = matched.iter().map(|t| t.amount.cents()).sum();
    let items: Vec<Transaction> = matched
        .into_iter()
        .skip(q.offset)
        .take(q.limit.max(1))
        .collect();
    Ok(TxnPage {
        total,
        total_cents,
        items,
    })
}

/// (year, month) of an epoch-seconds instant in the machine's local timezone.
/// Local (not UTC) so a late-night purchase buckets into the day/month the user
/// actually made it, and DST-correct because each instant gets its own offset.
/// Shared with the rhythm detector and ledger so all month math agrees.
pub(crate) fn ym_local(secs: i64) -> (i64, u32) {
    match Local.timestamp_opt(secs, 0).single() {
        Some(dt) => (dt.year() as i64, dt.month()),
        None => (1970, 1),
    }
}

/// A monotonic month index (year*12 + month-1) in local time, so months can be
/// compared, diffed, and used as map keys. Shared with the detector/ledger.
pub(crate) fn month_index(secs: i64) -> i64 {
    let (y, m) = ym_local(secs);
    y * 12 + (m as i64 - 1)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CategorySpend {
    /// `None` = uncategorized.
    pub category: Option<String>,
    /// Spend net of refunds, positive = money spent.
    pub total_cents: i64,
    /// Number of purchases (refunds not counted).
    pub count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MerchantSpend {
    /// Normalized merchant (`normalize_payee`).
    pub merchant: String,
    /// Outflow total, positive = money spent.
    pub total_cents: i64,
    pub count: usize,
}

pub fn spend_by_category(store: &Store, f: &TxnFilter) -> rusqlite::Result<Vec<CategorySpend>> {
    let txns = store.all_transactions()?;
    let mut map: HashMap<Option<String>, (i64, usize)> = HashMap::new();
    for t in &txns {
        if !passes(t, f) {
            continue;
        }
        // Out nets refunds in their own category (GLOSSARY), so these totals
        // sum to `flow_lanes`' out_cents. `count` stays the number of purchases.
        if t.amount.is_outflow() {
            let e = map.entry(t.category.clone()).or_insert((0, 0));
            e.0 += t.amount.cents().abs();
            e.1 += 1;
        } else if t.flag == TxnFlag::Spending && is_refund(t) {
            map.entry(t.category.clone()).or_insert((0, 0)).0 -= t.amount.cents();
        }
    }
    let mut out: Vec<CategorySpend> = map
        .into_iter()
        .map(|(category, (total_cents, count))| CategorySpend {
            category,
            total_cents,
            count,
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.total_cents));
    Ok(out)
}

pub fn top_merchants(
    store: &Store,
    f: &TxnFilter,
    limit: usize,
) -> rusqlite::Result<Vec<MerchantSpend>> {
    let txns = store.all_transactions()?;
    let mut map: HashMap<String, (i64, usize)> = HashMap::new();
    for t in &txns {
        if !passes(t, f) || !t.amount.is_outflow() {
            continue;
        }
        let key = normalize_payee(t.merchant());
        if key.is_empty() {
            continue;
        }
        let e = map.entry(key).or_insert((0, 0));
        e.0 += t.amount.cents().abs();
        e.1 += 1;
    }
    let mut out: Vec<MerchantSpend> = map
        .into_iter()
        .map(|(merchant, (total_cents, count))| MerchantSpend {
            merchant,
            total_cents,
            count,
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.total_cents));
    out.truncate(limit);
    Ok(out)
}

/// The category that marks a positive Spending transaction as In (income)
/// rather than a refund. Plaid's INCOME seeds it; recategorizing fixes a miss.
pub const INCOME_CATEGORY: &str = "Income";

/// Which Flow lane a positive Spending transaction belongs to. In: income-type
/// money (category Income) or not yet categorized, so an unknown deposit
/// never silently shrinks Out. Refund: a positive carrying a spending
/// category, netted against Out (GLOSSARY: a refund is not In).
fn is_refund(t: &Transaction) -> bool {
    !t.amount.is_outflow() && t.category.as_deref().is_some_and(|c| c != INCOME_CATEGORY)
}

/// One Moved group's money, seen from the spending-side accounts (checking
/// and credit). `out_cents` left them, `in_cents` came back; both positive.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MovedFlow {
    pub out_cents: i64,
    pub in_cents: i64,
}

impl MovedFlow {
    fn add(&mut self, cents: i64) {
        if cents < 0 {
            self.out_cents += -cents;
        } else {
            self.in_cents += cents;
        }
    }
}

/// The Moved lane, split by group (GLOSSARY: Flow lanes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct MovedLane {
    /// Card balances paid. Both legs are on spending-side accounts (bank out,
    /// card in), so this is the larger of the two sides for the month: never
    /// double-counted when both legs synced, still counted when one is missing
    /// (card not linked, or paid from an unlinked bank).
    pub card_payment_cents: i64,
    pub invest: MovedFlow,
    pub save: MovedFlow,
    pub loan_paydown: MovedFlow,
    /// Between own accounts, and moved money with no finer group yet.
    pub between: MovedFlow,
}

/// One month of Flow: the In / Out / Moved lanes and the two summary figures.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FlowMonth {
    pub year: i64,
    pub month: u32,
    /// Income-type inflows, plus inflows not categorized yet.
    pub in_cents: i64,
    /// Spending net of refunds (positive = money spent).
    pub out_cents: i64,
    /// Refunds already netted into `out_cents`, shown for context.
    pub refunds_cents: i64,
    pub moved: MovedLane,
    /// In − Out.
    pub earned_minus_spent_cents: i64,
    /// Invest + Save + LoanPaydown out, minus what came back from them.
    pub saved_cents: i64,
}

/// Flow lanes per month (GLOSSARY: Flow lanes; ADR 0001).
///
/// In and Out read Spending-flagged rows on every visible account. Moved
/// totals read only the spending-side accounts (checking and credit): the
/// other leg of a transfer, on a savings, investment or loan account, is the
/// far side and isn't summed, so a move between two linked accounts counts
/// once and a move to an unlinked account still counts. Card payments are the
/// exception and are counted on the card side; see `MovedLane`.
///
/// Uses the filter's date range and pending setting; `include_non_spending` is
/// ignored because every lane is reported.
pub fn flow_lanes(store: &Store, f: &TxnFilter) -> rusqlite::Result<Vec<FlowMonth>> {
    use crate::model::AccountKind;
    let kinds: HashMap<String, AccountKind> = store
        .accounts()?
        .into_iter()
        .map(|a| (a.id, a.kind))
        .collect();
    let kind = |id: &str| kinds.get(id).copied().unwrap_or(AccountKind::Other);
    let spending_side = |id: &str| matches!(kind(id), AccountKind::Checking | AccountKind::Credit);

    let all = TxnFilter {
        include_non_spending: true,
        ..*f
    };
    let mut months: HashMap<(i64, u32), FlowMonth> = HashMap::new();
    // Card payments per month: (bank side out, card side in).
    let mut card_legs: HashMap<(i64, u32), (i64, i64)> = HashMap::new();
    for t in store.all_transactions()?.iter().filter(|t| passes(t, &all)) {
        let (year, month) = ym_local(t.effective_date());
        let m = months.entry((year, month)).or_insert_with(|| FlowMonth {
            year,
            month,
            in_cents: 0,
            out_cents: 0,
            refunds_cents: 0,
            moved: MovedLane::default(),
            earned_minus_spent_cents: 0,
            saved_cents: 0,
        });
        let cents = t.amount.cents();
        match t.flag {
            TxnFlag::Spending if t.amount.is_outflow() => m.out_cents += -cents,
            TxnFlag::Spending if is_refund(t) => {
                m.out_cents -= cents;
                m.refunds_cents += cents;
            }
            TxnFlag::Spending => m.in_cents += cents,
            TxnFlag::CardPayment => {
                let legs = card_legs.entry((year, month)).or_default();
                match kind(&t.account_id) {
                    AccountKind::Credit if cents > 0 => legs.1 += cents,
                    AccountKind::Checking if cents < 0 => legs.0 += -cents,
                    _ => {}
                }
            }
            flag if spending_side(&t.account_id) => match flag {
                TxnFlag::Invest => m.moved.invest.add(cents),
                TxnFlag::Save => m.moved.save.add(cents),
                TxnFlag::LoanPaydown => m.moved.loan_paydown.add(cents),
                _ => m.moved.between.add(cents),
            },
            _ => {} // far-side leg of a move
        }
    }
    let mut out: Vec<FlowMonth> = months
        .into_values()
        .map(|mut m| {
            let (bank, card) = card_legs
                .get(&(m.year, m.month))
                .copied()
                .unwrap_or_default();
            m.moved.card_payment_cents = bank.max(card);
            m.earned_minus_spent_cents = m.in_cents - m.out_cents;
            let saving = [m.moved.invest, m.moved.save, m.moved.loan_paydown];
            m.saved_cents = saving.iter().map(|g| g.out_cents - g.in_cents).sum();
            m
        })
        .collect();
    out.sort_by_key(|m| (m.year, m.month));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Transaction, TxnFlag};
    use crate::money::Money;

    // Mid-month, noon UTC: bucketing is unambiguous in any timezone from UTC-12
    // to UTC+14, so these local-time tests stay deterministic on any machine.
    const JAN15: i64 = 1610712000; // 2021-01-15 12:00:00 UTC
    const FEB15: i64 = 1613390400; // 2021-02-15 12:00:00 UTC

    fn t(
        id: &str,
        posted: i64,
        cents: i64,
        merchant: &str,
        cat: Option<&str>,
        pending: bool,
    ) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "acct".into(),
            posted,
            transacted_at: None,
            amount: Money::from_cents(cents),
            description: merchant.into(),
            payee: Some(merchant.into()),
            category: cat.map(|s| s.into()),
            category_source: None,
            pending,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            pfc_primary: None,
            pfc_detailed: None,
            source: crate::model::default_source(),
        }
    }

    fn seed() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t("1", JAN15, -5000, "Whole Foods", Some("Groceries"), false),
            t("2", JAN15, -1500, "Netflix", Some("Streaming"), false),
            t("3", JAN15, -2000, "SQ *SPOTIFY 01", None, false),
            t("4", FEB15, -3000, "Whole Foods", Some("Groceries"), false),
            t("5", FEB15, 250000, "Payroll", None, false),
            t("6", FEB15, -900, "SQ *SPOTIFY 02", None, true),
        ])
        .unwrap();
        s
    }

    #[test]
    fn search_matches_text_over_payee_and_normalized_merchant() {
        let s = seed();
        // "spotify" only matches after processor-prefix normalization.
        let q = TxnQuery {
            text: Some("spotify".into()),
            ..TxnQuery::default()
        };
        let page = search_transactions(&s, &q).unwrap();
        assert_eq!(page.total, 2);
        assert_eq!(page.total_cents, -2900);

        // Category text also hits.
        let q = TxnQuery {
            text: Some("stream".into()),
            ..TxnQuery::default()
        };
        assert_eq!(search_transactions(&s, &q).unwrap().total, 1);
    }

    #[test]
    fn search_sorts_and_paginates_stably() {
        let s = seed();
        let q = TxnQuery {
            sort: SortKey::Amount,
            descending: false, // most negative first
            offset: 0,
            limit: 2,
            ..TxnQuery::default()
        };
        let page = search_transactions(&s, &q).unwrap();
        assert_eq!(page.total, 6);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].amount.cents(), -5000);
        assert_eq!(page.items[1].amount.cents(), -3000);
        // total_cents covers ALL matches, not the page.
        assert_eq!(page.total_cents, -5000 - 1500 - 2000 - 3000 + 250000 - 900);

        let next = search_transactions(
            &s,
            &TxnQuery {
                sort: SortKey::Amount,
                descending: false,
                offset: 2,
                limit: 2,
                ..TxnQuery::default()
            },
        )
        .unwrap();
        assert_eq!(next.items[0].amount.cents(), -2000);
    }

    #[test]
    fn search_filters_amount_band_category_and_pending() {
        let s = seed();
        let q = TxnQuery {
            min_cents: Some(1500),
            max_cents: Some(3000),
            ..TxnQuery::default()
        };
        // 1500, 2000, 3000, 900(pending, excluded by band), 250000 out of band.
        assert_eq!(search_transactions(&s, &q).unwrap().total, 3);

        let q = TxnQuery {
            category: Some("Groceries".into()),
            ..TxnQuery::default()
        };
        assert_eq!(search_transactions(&s, &q).unwrap().total, 2);

        let mut q = TxnQuery::default();
        q.filter.include_pending = false;
        assert_eq!(search_transactions(&s, &q).unwrap().total, 5);
    }

    #[test]
    fn search_flag_filter_needs_non_spending_included() {
        let s = seed();
        s.set_flag("1", TxnFlag::Transfer, false).unwrap();
        // Default filter drops non-Spending rows before the flag test.
        let q = TxnQuery {
            flag: Some(TxnFlag::Transfer),
            ..TxnQuery::default()
        };
        assert_eq!(search_transactions(&s, &q).unwrap().total, 0);
        // The transfers view sets include_non_spending.
        let mut q = TxnQuery {
            flag: Some(TxnFlag::Transfer),
            ..TxnQuery::default()
        };
        q.filter.include_non_spending = true;
        assert_eq!(search_transactions(&s, &q).unwrap().total, 1);
    }

    #[test]
    fn category_spend_sums_outflows_and_buckets_uncategorized() {
        let s = seed();
        let rows = spend_by_category(&s, &TxnFilter::all()).unwrap();
        let groceries = rows
            .iter()
            .find(|r| r.category.as_deref() == Some("Groceries"))
            .unwrap();
        assert_eq!(groceries.total_cents, 8000);
        assert_eq!(groceries.count, 2);
        let uncat = rows.iter().find(|r| r.category.is_none()).unwrap();
        assert_eq!(uncat.total_cents, 2900);
        assert_eq!(rows[0].category.as_deref(), Some("Groceries"));
    }

    #[test]
    fn category_spend_ignores_inflows() {
        let s = seed();
        let rows = spend_by_category(&s, &TxnFilter::all()).unwrap();
        assert!(rows.iter().all(|r| r.total_cents > 0));
        let total: i64 = rows.iter().map(|r| r.total_cents).sum();
        assert_eq!(total, 5000 + 1500 + 2000 + 3000 + 900);
    }

    #[test]
    fn top_merchants_groups_normalized_and_limits() {
        let s = seed();
        let rows = top_merchants(&s, &TxnFilter::all(), 2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].merchant, "whole foods");
        assert_eq!(rows[0].total_cents, 8000);
        let spotify = top_merchants(&s, &TxnFilter::all(), 10)
            .unwrap()
            .into_iter()
            .find(|r| r.merchant == "spotify")
            .unwrap();
        assert_eq!(spotify.count, 2);
        assert_eq!(spotify.total_cents, 2900);
    }

    #[test]
    fn flow_lanes_split_in_and_out() {
        let s = seed();
        let rows = flow_lanes(&s, &TxnFilter::all()).unwrap();
        let jan = rows
            .iter()
            .find(|r| (r.year, r.month) == (2021, 1))
            .unwrap();
        assert_eq!(jan.out_cents, 8500);
        assert_eq!(jan.in_cents, 0);
        assert_eq!(jan.earned_minus_spent_cents, -8500);
        let feb = rows
            .iter()
            .find(|r| (r.year, r.month) == (2021, 2))
            .unwrap();
        // Uncategorized payroll counts as In, not as a refund.
        assert_eq!(feb.in_cents, 250000);
        assert_eq!(feb.out_cents, 3900);
        assert_eq!(feb.earned_minus_spent_cents, 246100);
    }

    fn account(id: &str, kind: crate::model::AccountKind) -> crate::model::Account {
        crate::model::Account {
            id: id.into(),
            org: "Example Bank".into(),
            name: id.into(),
            kind,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: crate::model::default_source(),
            nickname: None,
            hidden: false,
        }
    }

    /// A synthetic month across checking, a card and savings exercising every
    /// lane (all amounts invented).
    #[test]
    fn flow_lanes_classify_in_out_and_every_moved_group() {
        use crate::model::AccountKind::{Checking, Credit, Savings};
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[
            account("chk", Checking),
            account("card", Credit),
            account("sav", Savings),
        ])
        .unwrap();
        let on = |acct: &str, id: &str, cents: i64, cat: Option<&str>, flag: TxnFlag| {
            let mut x = t(id, JAN15, cents, id, cat, false);
            x.account_id = acct.into();
            x.flag = flag;
            x
        };
        use TxnFlag::*;
        s.upsert_transactions(&[
            on("chk", "pay", 300000, Some("Income"), Spending),
            on("card", "groc", -5000, Some("Groceries"), Spending),
            on("card", "refund", 1000, Some("Groceries"), Spending),
            on("chk", "ccpay-bank", -4000, None, CardPayment),
            on("card", "ccpay-card", 4000, None, CardPayment),
            on("chk", "to-sav", -50000, None, Save),
            on("sav", "into-sav", 50000, None, Save), // far side: not summed
            on("chk", "from-sav", 10000, None, Save),
            on("chk", "ira", -20000, None, Invest), // brokerage not linked
            on("chk", "car", -30000, None, LoanPaydown),
            on("chk", "venmo", -7000, None, Transfer),
        ])
        .unwrap();
        let rows = flow_lanes(&s, &TxnFilter::all()).unwrap();
        assert_eq!(rows.len(), 1);
        let m = &rows[0];
        assert_eq!(m.in_cents, 300000);
        assert_eq!(m.out_cents, 4000, "refund nets against Out");
        assert_eq!(m.refunds_cents, 1000);
        assert_eq!(m.moved.card_payment_cents, 4000, "two legs, counted once");
        assert_eq!(
            m.moved.save,
            MovedFlow {
                out_cents: 50000,
                in_cents: 10000
            }
        );
        assert_eq!(m.moved.invest.out_cents, 20000);
        assert_eq!(m.moved.loan_paydown.out_cents, 30000);
        assert_eq!(
            m.moved.between,
            MovedFlow {
                out_cents: 7000,
                in_cents: 0
            }
        );
        assert_eq!(m.earned_minus_spent_cents, 296000);
        assert_eq!(m.saved_cents, 50000 - 10000 + 20000 + 30000);

        // Category totals agree with Out: the refund nets in Groceries.
        let cats = spend_by_category(&s, &TxnFilter::all()).unwrap();
        let groc = cats
            .iter()
            .find(|c| c.category.as_deref() == Some("Groceries"))
            .unwrap();
        assert_eq!((groc.total_cents, groc.count), (4000, 1));
        assert!(cats.iter().all(|c| c.category.as_deref() != Some("Income")));
    }

    #[test]
    fn excluding_pending_drops_provisional_rows() {
        let s = seed();
        let mut f = TxnFilter::all();
        f.include_pending = false;
        let spotify = top_merchants(&s, &f, 10)
            .unwrap()
            .into_iter()
            .find(|r| r.merchant == "spotify")
            .unwrap();
        assert_eq!(spotify.count, 1);
        assert_eq!(spotify.total_cents, 2000);
    }

    #[test]
    fn range_filter_bounds_are_since_inclusive_until_exclusive() {
        let s = seed();
        let f = TxnFilter::range(1609459200, 1612137600);
        let rows = flow_lanes(&s, &f).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].year, rows[0].month), (2021, 1));
    }

    #[test]
    fn non_spending_excluded_by_default_and_readmitted_by_toggle() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t(
                "spend",
                JAN15,
                -2000,
                "Whole Foods",
                Some("Groceries"),
                false,
            ),
            t("xfer", JAN15, -50000, "Move to Savings", None, false),
            t("pay", JAN15, -100000, "Amex Payment", None, false),
        ])
        .unwrap();
        s.set_flag("xfer", TxnFlag::Transfer, false).unwrap();
        s.set_flag("pay", TxnFlag::CardPayment, false).unwrap();

        let total: i64 = spend_by_category(&s, &TxnFilter::all())
            .unwrap()
            .iter()
            .map(|c| c.total_cents)
            .sum();
        assert_eq!(total, 2000, "transfer + card payment must be suppressed");

        let merch = top_merchants(&s, &TxnFilter::all(), 10).unwrap();
        assert!(merch.iter().all(|m| m.merchant == "whole foods"));

        let mut f = TxnFilter::all();
        f.include_non_spending = true;
        let total_all: i64 = spend_by_category(&s, &f)
            .unwrap()
            .iter()
            .map(|c| c.total_cents)
            .sum();
        assert_eq!(total_all, 2000 + 50000 + 100000);
    }

    #[test]
    fn card_payment_with_only_the_bank_leg_still_counts() {
        use crate::model::AccountKind::{Checking, Credit};
        let s = Store::open_in_memory().unwrap();
        s.upsert_accounts(&[account("chk", Checking), account("card", Credit)])
            .unwrap();
        let mut bank = t("bank", JAN15, -50000, "Card Payment", None, false);
        bank.account_id = "chk".into();
        bank.flag = TxnFlag::CardPayment;
        // A second payment seen only on the card (paid from an unlinked bank).
        let mut card = t("card", JAN15, 20000, "Payment Thank You", None, false);
        card.account_id = "card".into();
        card.flag = TxnFlag::CardPayment;
        s.upsert_transactions(&[bank, card]).unwrap();
        let rows = flow_lanes(&s, &TxnFilter::all()).unwrap();
        // max(bank 500, card 200): a lower bound, never a double count.
        assert_eq!(rows[0].moved.card_payment_cents, 50000);
    }

    #[test]
    fn flow_lanes_keep_unknown_account_transfers_out_of_in_and_out() {
        let s = Store::open_in_memory().unwrap();
        s.upsert_transactions(&[
            t("in", JAN15, 30000, "Transfer from Savings", None, false),
            t("out", JAN15, -30000, "Transfer to Savings", None, false),
            t("spend", JAN15, -2000, "Coffee", Some("Coffee"), false),
        ])
        .unwrap();
        s.set_flag("in", TxnFlag::Transfer, false).unwrap();
        s.set_flag("out", TxnFlag::Transfer, false).unwrap();

        let rows = flow_lanes(&s, &TxnFilter::all()).unwrap();
        let jan = rows
            .iter()
            .find(|r| (r.year, r.month) == (2021, 1))
            .unwrap();
        assert_eq!(jan.in_cents, 0, "transfer-in is not income");
        assert_eq!(jan.out_cents, 2000, "transfer-out is not spend");
        assert_eq!(jan.earned_minus_spent_cents, -2000);
        // No account row, so not a spending-side account: not summed in Moved.
        assert_eq!(jan.moved.between, MovedFlow::default());
    }

    #[test]
    fn buckets_on_transacted_at_not_posted() {
        // Behaved in January, posted in February → must bucket into January.
        let s = Store::open_in_memory().unwrap();
        let mut tx = t(
            "late",
            FEB15,
            -1000,
            "Corner Store",
            Some("Shopping"),
            false,
        );
        tx.transacted_at = Some(JAN15);
        s.upsert_transactions(&[tx]).unwrap();
        let rows = flow_lanes(&s, &TxnFilter::all()).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!((rows[0].year, rows[0].month), (2021, 1));
    }
}
