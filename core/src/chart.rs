//! One chart, picked by a strict set of query parameters: the data behind
//! the embeddable chart view (`/embed/chart`) that agent-avatar frames, and
//! `GET /api/chart` for anyone else who wants the same numbers.
//!
//! The parameters are an allow-list (docs/API.md "Embed views"): an unknown
//! name, a repeated name or a bad value is a [`ParamError`] naming the
//! parameter, never a guess. agent-avatar validates against the same list
//! before it builds the URL.
//!
//! - `metric` (required): `spending` (Flow's Out per month), `category`
//!   (Out in one category per month, needs `category=`), `net_worth`,
//!   `cash`, `invested` (balances per day, as `/balances/history`).
//! - `range`: `3m` `6m` `12m` (default) `ytd` `all`; or `from` / `to` as
//!   `YYYY-MM` or `YYYY-MM-DD` (a month `from` starts on its 1st, a month
//!   `to` ends on its last day; `to` past today is today). Not both.
//! - `highlight`: a month `2026-10`, a day `2026-10-03`, or `A..B` with
//!   either spelling at each end. It must overlap the range.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use chrono::{Datelike, Days, Months, NaiveDate};
use serde::Serialize;

use crate::balances::{history, DayTotals};
use crate::model::AccountKind;
use crate::period::Period;
use crate::query::{flow_lanes, pick_series, Lane, TxnFilter, TxnQuery};
use crate::store::Store;

/// What the chart plots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChartMetric {
    Spending,
    Category,
    NetWorth,
    Cash,
    Invested,
}

impl ChartMetric {
    const NAMES: &'static str = "spending, category, net_worth, cash, invested";

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "spending" => Self::Spending,
            "category" => Self::Category,
            "net_worth" => Self::NetWorth,
            "cash" => Self::Cash,
            "invested" => Self::Invested,
            _ => return None,
        })
    }

    fn step(self) -> ChartStep {
        match self {
            Self::Spending | Self::Category => ChartStep::Month,
            Self::NetWorth | Self::Cash | Self::Invested => ChartStep::Day,
        }
    }
}

/// One point per month (bars) or per day (a line).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ChartStep {
    Month,
    Day,
}

/// A parameter the chart can't use, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamError {
    pub param: String,
    pub reason: String,
}

impl ParamError {
    fn new(param: &str, reason: impl Into<String>) -> Self {
        ParamError {
            param: param.into(),
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.param, self.reason)
    }
}

impl std::error::Error for ParamError {}

#[derive(Debug)]
pub enum ChartError {
    Param(ParamError),
    Store(rusqlite::Error),
}

impl fmt::Display for ChartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChartError::Param(e) => e.fmt(f),
            ChartError::Store(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for ChartError {}

impl From<rusqlite::Error> for ChartError {
    fn from(e: rusqlite::Error) -> Self {
        ChartError::Store(e)
    }
}

impl From<ParamError> for ChartError {
    fn from(e: ParamError) -> Self {
        ChartError::Param(e)
    }
}

/// The highlighted days, inclusive at both ends, and how they were spelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    from: NaiveDate,
    to: NaiveDate,
    /// `from` was spelled as a month.
    from_month: bool,
    /// `to` was spelled as a month.
    to_month: bool,
}

/// Validated parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartRequest {
    pub metric: ChartMetric,
    /// As given; checked against the category list by [`chart`].
    pub category: Option<String>,
    /// First day; `None` = all history.
    pub since: Option<NaiveDate>,
    /// Last day (inclusive), never after today.
    pub until: NaiveDate,
    pub range_label: String,
    highlight: Option<Span>,
}

const PARAMS: [&str; 6] = ["metric", "category", "range", "from", "to", "highlight"];

/// Checks the query pairs against the allow-list. `today` anchors the
/// presets and caps `to`.
pub fn parse_params(
    pairs: &[(String, String)],
    today: NaiveDate,
) -> Result<ChartRequest, ParamError> {
    let mut got: HashMap<&str, &str> = HashMap::new();
    for (k, v) in pairs {
        let Some(name) = PARAMS.iter().find(|p| **p == k.as_str()) else {
            return Err(ParamError::new(
                k,
                format!("not a chart parameter (use {})", PARAMS.join(", ")),
            ));
        };
        if got.insert(name, v.trim()).is_some() {
            return Err(ParamError::new(name, "given more than once"));
        }
    }

    let metric = match got.get("metric") {
        None | Some(&"") => return Err(ParamError::new("metric", "required")),
        Some(m) => ChartMetric::parse(m).ok_or_else(|| {
            ParamError::new("metric", format!("use one of {}", ChartMetric::NAMES))
        })?,
    };

    let category = match (metric, got.get("category")) {
        (ChartMetric::Category, None | Some(&"")) => {
            return Err(ParamError::new("category", "required with metric=category"))
        }
        (ChartMetric::Category, Some(c)) => Some(c.to_string()),
        (_, Some(_)) => return Err(ParamError::new("category", "only with metric=category")),
        (_, None) => None,
    };

    let this_month = first_of_month(today);
    let preset = |months: u32, label: &str| {
        let since = this_month - Months::new(months - 1);
        (Some(since), today, label.to_string())
    };
    let (since, until, range_label) = match (got.get("range"), got.get("from"), got.get("to")) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(ParamError::new("range", "give range or from/to, not both"))
        }
        (Some(r), None, None) => match *r {
            "3m" => preset(3, "Last 3 months"),
            "6m" => preset(6, "Last 6 months"),
            "12m" => preset(12, "Last 12 months"),
            "ytd" => (
                Some(NaiveDate::from_ymd_opt(today.year(), 1, 1).expect("Jan 1")),
                today,
                "Year to date".into(),
            ),
            "all" => (None, today, "All history".into()),
            _ => return Err(ParamError::new("range", "use one of 3m, 6m, 12m, ytd, all")),
        },
        (None, None, None) => preset(12, "Last 12 months"),
        (None, from, to) => {
            let from = from.map(|s| parse_end("from", s, false)).transpose()?;
            let to = to.map(|s| parse_end("to", s, true)).transpose()?;
            let until = to.map_or(today, |(d, _)| d.min(today));
            if let Some((f, _)) = from {
                if f > until {
                    return Err(ParamError::new("from", "after to (or after today)"));
                }
            }
            let label = match (from, to) {
                (Some(f), Some(t)) => format!("{} – {}", label_of(f), label_of(t)),
                (Some(f), None) => format!("Since {}", label_of(f)),
                (None, Some(t)) => format!("Through {}", label_of(t)),
                (None, None) => unreachable!("matched above"),
            };
            (from.map(|(d, _)| d), until, label)
        }
    };

    let highlight = match got.get("highlight") {
        None | Some(&"") => None,
        Some(h) => {
            let (a, b) = match h.split_once("..") {
                Some((a, b)) => (a, b),
                None => (*h, *h),
            };
            let (from, from_month) = parse_end("highlight", a, false)?;
            let (to, to_month) = parse_end("highlight", b, true)?;
            if from > to {
                return Err(ParamError::new("highlight", "start is after end"));
            }
            if to < since.unwrap_or(NaiveDate::MIN) || from > until {
                return Err(ParamError::new("highlight", "outside the range"));
            }
            Some(Span {
                from,
                to,
                from_month,
                to_month,
            })
        }
    };

    Ok(ChartRequest {
        metric,
        category,
        since,
        until,
        range_label,
        highlight,
    })
}

/// `YYYY-MM` or `YYYY-MM-DD`. A month is its first day, or with `end` its
/// last day. Returns the day and whether it was spelled as a month.
fn parse_end(param: &str, s: &str, end: bool) -> Result<(NaiveDate, bool), ParamError> {
    let s = s.trim();
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok((d, false));
    }
    let bad = || ParamError::new(param, format!("bad date {s:?}: use YYYY-MM or YYYY-MM-DD"));
    if s.len() != 7 {
        return Err(bad());
    }
    let first = NaiveDate::parse_from_str(&format!("{s}-01"), "%Y-%m-%d").map_err(|_| bad())?;
    Ok((if end { last_of_month(first) } else { first }, true))
}

fn first_of_month(d: NaiveDate) -> NaiveDate {
    d.with_day(1).expect("day 1 exists")
}

fn last_of_month(d: NaiveDate) -> NaiveDate {
    first_of_month(d) + Months::new(1) - Days::new(1)
}

fn label_of((d, month): (NaiveDate, bool)) -> String {
    if month {
        d.format("%b %Y").to_string()
    } else {
        d.format("%b %-d, %Y").to_string()
    }
}

/// One bar (`YYYY-MM`) or one day on the line (`YYYY-MM-DD`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ChartPoint {
    pub key: String,
    /// Spending: positive = money spent. Balances: signed, as `/balances`.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cents: i64,
    pub highlighted: bool,
}

/// The one big figure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ChartHeadline {
    pub label: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cents: i64,
    /// Balances: the change over the highlighted range, or over the whole
    /// range without a highlight. `null` for spending and a one-day
    /// highlight.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub change_cents: Option<i64>,
}

/// `GET /api/chart`: what the embed view draws.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ChartView {
    pub metric: ChartMetric,
    pub step: ChartStep,
    /// "Spending", the category's name, "Net worth", "Cash", "Invested".
    pub title: String,
    /// "Last 6 months", "Year to date", "Mar 2026 – Oct 2026", …
    pub range_label: String,
    /// Oldest first. Months with no spending are $0 bars; balance days run
    /// from the first snapshot in the range.
    pub points: Vec<ChartPoint>,
    /// The key of the month still in progress, when it is the last bar.
    pub partial_key: Option<String>,
    pub headline: ChartHeadline,
}

/// Builds the chart. `today` decides which month is still in progress.
pub fn chart(store: &Store, req: &ChartRequest, today: NaiveDate) -> Result<ChartView, ChartError> {
    match req.metric.step() {
        ChartStep::Month => monthly(store, req, today),
        ChartStep::Day => daily(store, req),
    }
}

fn filter(req: &ChartRequest) -> TxnFilter {
    let (since, until) = Period::custom(req.since.unwrap_or(req.until), req.until)
        .expect("since <= until")
        .bounds();
    TxnFilter {
        since: req.since.map(|_| since),
        until: Some(until),
        ..TxnFilter::all()
    }
}

fn monthly(store: &Store, req: &ChartRequest, today: NaiveDate) -> Result<ChartView, ChartError> {
    let f = filter(req);
    let (title, by): (String, BTreeMap<(i64, u32), i64>) = match req.metric {
        ChartMetric::Category => {
            let asked = req.category.as_deref().unwrap_or_default();
            let name = store
                .categories()?
                .into_iter()
                .find(|c| c.eq_ignore_ascii_case(asked))
                .ok_or_else(|| ParamError::new("category", format!("no category {asked:?}")))?;
            let q = TxnQuery {
                filter: f,
                category: Some(name.clone()),
                lane: Some(Lane::Out),
                ..TxnQuery::default()
            };
            let by = pick_series(store, &q)?
                .into_iter()
                .map(|m| ((m.year, m.month), -m.cents))
                .collect();
            (name, by)
        }
        _ => {
            let by = flow_lanes(store, &f)?
                .into_iter()
                .map(|m| ((m.year, m.month), m.out_cents))
                .collect();
            ("Spending".into(), by)
        }
    };

    let last = first_of_month(req.until);
    let first = match req.since {
        Some(d) => first_of_month(d),
        None => by
            .keys()
            .next()
            .and_then(|&(y, m)| NaiveDate::from_ymd_opt(y as i32, m, 1))
            .unwrap_or(last)
            .min(last),
    };
    let hl = req
        .highlight
        .map(|s| (first_of_month(s.from), first_of_month(s.to)));
    let mut points = Vec::new();
    let mut month = first;
    while month <= last {
        points.push(ChartPoint {
            key: month.format("%Y-%m").to_string(),
            cents: by
                .get(&(month.year() as i64, month.month()))
                .copied()
                .unwrap_or(0),
            highlighted: hl.is_some_and(|(a, b)| a <= month && month <= b),
        });
        month = month + Months::new(1);
    }
    let in_progress = first_of_month(today) == last && req.until >= today;
    let partial_key = in_progress.then(|| last.format("%Y-%m").to_string());

    let headline = match req.highlight {
        Some(s) => {
            let label = if first_of_month(s.from) == first_of_month(s.to) {
                s.from.format("%b %Y").to_string()
            } else {
                format!("{} – {}", s.from.format("%b %Y"), s.to.format("%b %Y"))
            };
            ChartHeadline {
                label,
                cents: points
                    .iter()
                    .filter(|p| p.highlighted)
                    .map(|p| p.cents)
                    .sum(),
                change_cents: None,
            }
        }
        None => {
            let done: Vec<i64> = points
                .iter()
                .filter(|p| Some(&p.key) != partial_key.as_ref())
                .map(|p| p.cents)
                .collect();
            if done.is_empty() {
                ChartHeadline {
                    label: "So far this month".into(),
                    cents: points.last().map_or(0, |p| p.cents),
                    change_cents: None,
                }
            } else {
                ChartHeadline {
                    label: "Average month".into(),
                    cents: done.iter().sum::<i64>() / done.len() as i64,
                    change_cents: None,
                }
            }
        }
    };

    Ok(ChartView {
        metric: req.metric,
        step: ChartStep::Month,
        title,
        range_label: req.range_label.clone(),
        points,
        partial_key,
        headline,
    })
}

fn daily(store: &Store, req: &ChartRequest) -> Result<ChartView, ChartError> {
    // All snapshots, then trim: the first day in the range carries the last
    // value from before it, as on the Holdings chart.
    let kinds: HashMap<String, AccountKind> = store
        .accounts()?
        .into_iter()
        .map(|a| (a.id, a.kind))
        .collect();
    let days = history(&store.balance_snapshots(None, None)?, &kinds);
    let (title, pick): (&str, fn(&DayTotals) -> i64) = match req.metric {
        ChartMetric::Cash => ("Cash", |d| d.cash.cents()),
        ChartMetric::Invested => ("Invested", |d| d.invested.cents()),
        _ => ("Net worth", |d| d.net_worth.cents()),
    };
    let fmt = |d: NaiveDate| d.format("%Y-%m-%d").to_string();
    let since = req.since.map(fmt);
    let until = fmt(req.until);
    let hl = req.highlight.map(|s| (fmt(s.from), fmt(s.to)));
    let points: Vec<ChartPoint> = days
        .iter()
        .filter(|d| since.as_ref().is_none_or(|s| &d.day >= s) && d.day <= until)
        .map(|d| ChartPoint {
            key: d.day.clone(),
            cents: pick(d),
            highlighted: hl.as_ref().is_some_and(|(a, b)| a <= &d.day && &d.day <= b),
        })
        .collect();

    let day_label = |d: NaiveDate| d.format("%b %-d, %Y").to_string();
    let headline = match req.highlight {
        Some(s) if s.from == s.to && !s.from_month => ChartHeadline {
            label: day_label(s.from),
            cents: points.iter().find(|p| p.highlighted).map_or(0, |p| p.cents),
            change_cents: None,
        },
        Some(s) => {
            let lit: Vec<&ChartPoint> = points.iter().filter(|p| p.highlighted).collect();
            let label = match (
                s.from_month && s.to_month,
                first_of_month(s.from) == first_of_month(s.to),
            ) {
                (true, true) => s.from.format("%b %Y").to_string(),
                (true, false) => format!("{} – {}", s.from.format("%b %Y"), s.to.format("%b %Y")),
                _ => format!("{} – {}", day_label(s.from), day_label(s.to)),
            };
            ChartHeadline {
                label,
                cents: lit.last().map_or(0, |p| p.cents),
                change_cents: match (lit.first(), lit.last()) {
                    (Some(a), Some(b)) => Some(b.cents - a.cents),
                    _ => None,
                },
            }
        }
        None => ChartHeadline {
            label: points
                .last()
                .and_then(|p| NaiveDate::parse_from_str(&p.key, "%Y-%m-%d").ok())
                .map_or_else(|| "No balances yet".into(), day_label),
            cents: points.last().map_or(0, |p| p.cents),
            change_cents: match (points.first(), points.last()) {
                (Some(a), Some(b)) => Some(b.cents - a.cents),
                _ => None,
            },
        },
    };

    Ok(ChartView {
        metric: req.metric,
        step: ChartStep::Day,
        title: title.into(),
        range_label: req.range_label.clone(),
        points,
        partial_key: None,
        headline,
    })
}

#[cfg(test)]
#[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
mod tests {
    //! Synthetic data only.
    use super::*;
    use crate::model::{Account, Transaction, TxnFlag};
    use crate::money::Money;
    use chrono::{Local, TimeZone};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn noon(d: NaiveDate) -> i64 {
        Local
            .with_ymd_and_hms(d.year(), d.month(), d.day(), 12, 0, 0)
            .earliest()
            .unwrap()
            .timestamp()
    }

    const TODAY: (i32, u32, u32) = (2026, 11, 14);

    fn today() -> NaiveDate {
        day(TODAY.0, TODAY.1, TODAY.2)
    }

    fn parse(q: &str) -> Result<ChartRequest, ParamError> {
        let pairs: Vec<(String, String)> = q
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                (k.to_string(), v.to_string())
            })
            .collect();
        parse_params(&pairs, today())
    }

    fn err_param(q: &str) -> String {
        parse(q).unwrap_err().param
    }

    #[test]
    fn rejects_anything_off_the_list() {
        assert_eq!(err_param(""), "metric");
        assert_eq!(err_param("metric=income"), "metric");
        assert_eq!(err_param("metric=spending&url=x"), "url");
        assert_eq!(err_param("metric=spending&metric=cash"), "metric");
        assert_eq!(err_param("metric=category"), "category");
        assert_eq!(err_param("metric=spending&category=Groceries"), "category");
        assert_eq!(err_param("metric=spending&range=2y"), "range");
        assert_eq!(err_param("metric=spending&range=6m&from=2026-01"), "range");
        assert_eq!(err_param("metric=spending&from=2026-13"), "from");
        assert_eq!(err_param("metric=spending&from=2026-10&to=2026-09"), "from");
        assert_eq!(err_param("metric=spending&from=2027-01"), "from");
        assert_eq!(
            err_param("metric=spending&highlight=2026-10..2026-09"),
            "highlight"
        );
        assert_eq!(
            err_param("metric=spending&range=3m&highlight=2026-01"),
            "highlight"
        );
        assert_eq!(err_param("metric=spending&highlight=Oct"), "highlight");
    }

    #[test]
    fn ranges_resolve_to_days() {
        let r = parse("metric=spending").unwrap();
        assert_eq!((r.since, r.until), (Some(day(2025, 12, 1)), today()));
        assert_eq!(r.range_label, "Last 12 months");
        let r = parse("metric=spending&range=3m").unwrap();
        assert_eq!(r.since, Some(day(2026, 9, 1)));
        let r = parse("metric=cash&range=ytd").unwrap();
        assert_eq!(r.since, Some(day(2026, 1, 1)));
        let r = parse("metric=cash&range=all").unwrap();
        assert_eq!(r.since, None);
        // A month `to` ends on its last day; past today is today.
        let r = parse("metric=spending&from=2026-03&to=2026-11").unwrap();
        assert_eq!((r.since, r.until), (Some(day(2026, 3, 1)), today()));
        assert_eq!(r.range_label, "Mar 2026 – Nov 2026");
        let r = parse("metric=spending&from=2026-03&to=2026-06").unwrap();
        assert_eq!(r.until, day(2026, 6, 30));
        let r = parse("metric=net_worth&to=2026-06-15").unwrap();
        assert_eq!((r.since, r.until), (None, day(2026, 6, 15)));
        assert_eq!(r.range_label, "Through Jun 15, 2026");
    }

    fn chk() -> Account {
        Account {
            id: "chk".into(),
            org: "Demo Bank".into(),
            name: "Demo Checking (0000)".into(),
            kind: AccountKind::Checking,
            balance: Money::from_cents(0),
            currency: "USD".into(),
            last_synced: 0,
            source: "plaid".into(),
            nickname: None,
            hidden: false,
            item_id: None,
            kind_from_bank: None,
        }
    }

    fn spend(id: &str, d: NaiveDate, cents: i64, category: &str) -> Transaction {
        Transaction {
            id: id.into(),
            account_id: "chk".into(),
            posted: noon(d),
            transacted_at: None,
            amount: Money::from_cents(-cents),
            description: format!("Demo shop {id}"),
            payee: Some(format!("Demo shop {id}")),
            category: Some(category.into()),
            category_source: None,
            pending: false,
            flag: TxnFlag::Spending,
            raw: "{}".into(),
            pfc_primary: None,
            pfc_detailed: None,
            source: crate::model::default_source(),
        }
    }

    /// Aug–Nov 2026: groceries every month, one dining charge in Sep, none
    /// in Oct.
    fn seed() -> Store {
        let s = Store::open_in_memory().unwrap();
        s.add_category("Demo Groceries").unwrap();
        s.add_category("Demo Dining").unwrap();
        s.upsert_accounts(&[chk()]).unwrap();
        s.upsert_transactions(&[
            spend("a", day(2026, 8, 5), 100_00, "Demo Groceries"),
            spend("b", day(2026, 9, 5), 120_00, "Demo Groceries"),
            spend("c", day(2026, 9, 9), 60_00, "Demo Dining"),
            spend("d", day(2026, 10, 5), 90_00, "Demo Groceries"),
            spend("e", day(2026, 11, 5), 30_00, "Demo Groceries"),
        ])
        .unwrap();
        s
    }

    fn cents(v: &ChartView) -> Vec<(&str, i64, bool)> {
        v.points
            .iter()
            .map(|p| (p.key.as_str(), p.cents, p.highlighted))
            .collect()
    }

    #[test]
    fn spending_fills_months_and_averages_complete_ones() {
        let s = seed();
        let v = chart(&s, &parse("metric=spending&range=6m").unwrap(), today()).unwrap();
        assert_eq!(v.title, "Spending");
        assert_eq!(v.step, ChartStep::Month);
        assert_eq!(
            cents(&v),
            [
                ("2026-06", 0, false),
                ("2026-07", 0, false),
                ("2026-08", 100_00, false),
                ("2026-09", 180_00, false),
                ("2026-10", 90_00, false),
                ("2026-11", 30_00, false),
            ]
        );
        assert_eq!(v.partial_key.as_deref(), Some("2026-11"));
        // Five complete months: (0 + 0 + 100 + 180 + 90) / 5.
        assert_eq!(v.headline.label, "Average month");
        assert_eq!(v.headline.cents, 74_00);
    }

    #[test]
    fn all_history_starts_at_the_first_month_with_data() {
        let s = seed();
        let v = chart(&s, &parse("metric=spending&range=all").unwrap(), today()).unwrap();
        assert_eq!(v.points.first().unwrap().key, "2026-08");
    }

    #[test]
    fn category_trend_with_a_highlighted_range() {
        let s = seed();
        let req =
            parse("metric=category&category=demo groceries&range=3m&highlight=2026-09..2026-10")
                .unwrap();
        let v = chart(&s, &req, today()).unwrap();
        assert_eq!(v.title, "Demo Groceries");
        assert_eq!(
            cents(&v),
            [
                ("2026-09", 120_00, true),
                ("2026-10", 90_00, true),
                ("2026-11", 30_00, false),
            ]
        );
        assert_eq!(v.headline.label, "Sep 2026 – Oct 2026");
        assert_eq!(v.headline.cents, 210_00);
    }

    #[test]
    fn unknown_category_is_a_param_error() {
        let s = seed();
        let req = parse("metric=category&category=Demo Travel").unwrap();
        match chart(&s, &req, today()) {
            Err(ChartError::Param(e)) => assert_eq!(e.param, "category"),
            other => panic!("expected a param error, got {other:?}"),
        }
    }

    #[test]
    fn a_closed_range_has_no_partial_month() {
        let s = seed();
        let req = parse("metric=spending&from=2026-08&to=2026-10&highlight=2026-09-20").unwrap();
        let v = chart(&s, &req, today()).unwrap();
        assert_eq!(v.partial_key, None);
        assert_eq!(v.headline.label, "Sep 2026");
        assert_eq!(v.headline.cents, 180_00);
    }

    fn with_balances() -> Store {
        let s = seed();
        let at = |d: NaiveDate, cents: i64| {
            let a = Account {
                balance: Money::from_cents(cents),
                ..chk()
            };
            s.record_item_snapshot(&[a], None, noon(d)).unwrap();
        };
        at(day(2026, 9, 28), 1_000_00);
        at(day(2026, 10, 2), 1_500_00);
        at(day(2026, 10, 30), 1_200_00);
        s
    }

    #[test]
    fn balances_carry_into_the_range_and_report_change() {
        let s = with_balances();
        let req = parse("metric=cash&from=2026-10-01&to=2026-10-31").unwrap();
        let v = chart(&s, &req, today()).unwrap();
        assert_eq!(v.step, ChartStep::Day);
        // Oct 1 carries Sep 28's value; the line ends at the last snapshot.
        assert_eq!(v.points.first().unwrap().key, "2026-10-01");
        assert_eq!(v.points.first().unwrap().cents, 1_000_00);
        assert_eq!(v.points.last().unwrap().key, "2026-10-30");
        assert_eq!(v.headline.cents, 1_200_00);
        assert_eq!(v.headline.change_cents, Some(200_00));
        assert_eq!(v.headline.label, "Oct 30, 2026");
    }

    #[test]
    fn balance_highlights_a_day_or_a_span() {
        let s = with_balances();
        let req = parse("metric=net_worth&range=3m&highlight=2026-10-02").unwrap();
        let v = chart(&s, &req, today()).unwrap();
        assert_eq!(v.points.iter().filter(|p| p.highlighted).count(), 1);
        assert_eq!(v.headline.label, "Oct 2, 2026");
        assert_eq!(v.headline.cents, 1_500_00);
        assert_eq!(v.headline.change_cents, None);

        let req = parse("metric=net_worth&range=3m&highlight=2026-10").unwrap();
        let v = chart(&s, &req, today()).unwrap();
        assert_eq!(v.headline.label, "Oct 2026");
        assert_eq!(v.headline.change_cents, Some(200_00));
    }
}
