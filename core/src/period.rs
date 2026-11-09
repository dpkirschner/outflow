//! One date-range type for every screen, endpoint and CLI command.
//!
//! A [`Period`] is a half-open range of calendar days `[start, end)`, aligned
//! to weeks (ISO, Monday first), months, quarters or years, or a custom range.
//! Days are local-time days, the same as month bucketing in `query`: turn a
//! period into epoch seconds with [`Period::bounds`] (machine-local zone) or
//! [`Period::bounds_in`] (any zone, for tests).
//!
//! One spelling on the wire and the command line ([`Period::parse`]):
//! `2026-11`, `2026-W45`, `2026-Q4`, `2026`, or the relative words
//! `this-week`, `last-week`, `this-month`, `last-month`, `this-quarter`,
//! `last-quarter`, `this-year`, `last-year`. Custom ranges come from
//! `since=YYYY-MM-DD&until=YYYY-MM-DD` ([`Period::resolve`]), `until`
//! inclusive.

use std::fmt;

use chrono::{Datelike, Days, Local, Months, NaiveDate, TimeZone, Weekday};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum PeriodKind {
    Week,
    Month,
    Quarter,
    Year,
    Custom,
}

/// A half-open range of local calendar days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Period {
    pub kind: PeriodKind,
    /// First day, `YYYY-MM-DD`.
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub start: NaiveDate,
    /// The day after the last day, `YYYY-MM-DD` (exclusive).
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub end: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeriodError {
    /// Not one of the spellings in the module docs.
    BadSpelling(String),
    /// `since` / `until` isn't `YYYY-MM-DD`.
    BadDate(String),
    /// `since` is after `until`.
    Reversed,
    /// `period` given together with `since` / `until`.
    Ambiguous,
}

impl fmt::Display for PeriodError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeriodError::BadSpelling(s) => write!(
                f,
                "bad period {s:?}: use 2026-11, 2026-W45, 2026-Q4, 2026, \
                 or this-/last- week, month, quarter, year"
            ),
            PeriodError::BadDate(s) => write!(f, "bad date {s:?}: use YYYY-MM-DD"),
            PeriodError::Reversed => write!(f, "since is after until"),
            PeriodError::Ambiguous => write!(f, "give either period or since/until, not both"),
        }
    }
}

impl std::error::Error for PeriodError {}

/// Lower bound for a custom range with no `since`: before any archived row.
const EPOCH_DAY: NaiveDate = match NaiveDate::from_ymd_opt(1970, 1, 1) {
    Some(d) => d,
    None => panic!(),
};

/// Today in the machine's local zone.
pub fn today_local() -> NaiveDate {
    Local::now().date_naive()
}

/// The local calendar day an epoch-seconds instant falls on (the same
/// local-day convention as [`Period::bounds`]). Out-of-range instants give
/// 1970-01-01.
pub fn local_day(secs: i64) -> NaiveDate {
    Local
        .timestamp_opt(secs, 0)
        .earliest()
        .map(|dt| dt.date_naive())
        .unwrap_or_default()
}

/// Parses `YYYY-MM-DD`.
pub fn parse_day(s: &str) -> Result<NaiveDate, PeriodError> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").map_err(|_| PeriodError::BadDate(s.into()))
}

impl Period {
    /// The calendar period of `kind` that contains `day`. A `Custom` kind
    /// gives the single day.
    pub fn containing(kind: PeriodKind, day: NaiveDate) -> Period {
        let (start, end) = match kind {
            PeriodKind::Week => {
                let start = day - Days::new(day.weekday().num_days_from_monday().into());
                (start, start + Days::new(7))
            }
            PeriodKind::Month => {
                let start = day.with_day(1).expect("day 1 exists");
                (start, start + Months::new(1))
            }
            PeriodKind::Quarter => {
                let m0 = (day.month0() / 3) * 3 + 1;
                let start = NaiveDate::from_ymd_opt(day.year(), m0, 1).expect("valid quarter");
                (start, start + Months::new(3))
            }
            PeriodKind::Year => {
                let start = NaiveDate::from_ymd_opt(day.year(), 1, 1).expect("jan 1");
                (start, start + Months::new(12))
            }
            PeriodKind::Custom => (day, day + Days::new(1)),
        };
        Period { kind, start, end }
    }

    /// The calendar period of `kind` containing `today`.
    pub fn current(kind: PeriodKind, today: NaiveDate) -> Period {
        Period::containing(kind, today)
    }

    /// The period just before this one: the previous week, month, quarter or
    /// year, or for a custom range the same number of days ending at `start`.
    pub fn previous(&self) -> Period {
        match self.kind {
            PeriodKind::Custom => {
                let len = self.days();
                Period {
                    kind: PeriodKind::Custom,
                    start: self.start - Days::new(len),
                    end: self.start,
                }
            }
            kind => Period::containing(kind, self.start - Days::new(1)),
        }
    }

    /// A custom range from `since` through `until`, both inclusive.
    pub fn custom(since: NaiveDate, until: NaiveDate) -> Result<Period, PeriodError> {
        if since > until {
            return Err(PeriodError::Reversed);
        }
        Ok(Period {
            kind: PeriodKind::Custom,
            start: since,
            end: until + Days::new(1),
        })
    }

    /// Parses one `period=` spelling (see the module docs). `today` anchors
    /// the relative words.
    pub fn parse(s: &str, today: NaiveDate) -> Result<Period, PeriodError> {
        let s = s.trim();
        let bad = || PeriodError::BadSpelling(s.to_owned());
        if let Some((rel, unit)) = s.split_once('-') {
            let kind = match unit {
                "week" => Some(PeriodKind::Week),
                "month" => Some(PeriodKind::Month),
                "quarter" => Some(PeriodKind::Quarter),
                "year" => Some(PeriodKind::Year),
                _ => None,
            };
            if let Some(kind) = kind {
                let this = Period::current(kind, today);
                return match rel {
                    "this" => Ok(this),
                    "last" => Ok(this.previous()),
                    _ => Err(bad()),
                };
            }
        }

        let num = |t: &str| -> Option<u32> {
            (!t.is_empty() && t.bytes().all(|b| b.is_ascii_digit()))
                .then(|| t.parse().ok())
                .flatten()
        };
        let (year, rest) = match s.split_once('-') {
            Some((y, r)) => (y, Some(r)),
            None => (s, None),
        };
        if year.len() != 4 {
            return Err(bad());
        }
        let year = num(year).ok_or_else(bad)? as i32;
        let first_of = |m: u32| NaiveDate::from_ymd_opt(year, m, 1);
        match rest {
            None => Ok(Period::containing(
                PeriodKind::Year,
                first_of(1).ok_or_else(bad)?,
            )),
            Some(r) if r.starts_with('W') => {
                let w = r
                    .get(1..)
                    .filter(|w| w.len() == 2)
                    .and_then(num)
                    .ok_or_else(bad)?;
                let monday = NaiveDate::from_isoywd_opt(year, w, Weekday::Mon).ok_or_else(bad)?;
                Ok(Period::containing(PeriodKind::Week, monday))
            }
            Some(r) if r.starts_with('Q') => {
                let q = r
                    .get(1..)
                    .filter(|q| q.len() == 1)
                    .and_then(num)
                    .ok_or_else(bad)?;
                if !(1..=4).contains(&q) {
                    return Err(bad());
                }
                let day = first_of((q - 1) * 3 + 1).ok_or_else(bad)?;
                Ok(Period::containing(PeriodKind::Quarter, day))
            }
            Some(r) if r.len() == 2 => {
                let m = num(r).ok_or_else(bad)?;
                Ok(Period::containing(
                    PeriodKind::Month,
                    first_of(m).ok_or_else(bad)?,
                ))
            }
            Some(_) => Err(bad()),
        }
    }

    /// Resolves the shared query parameters: `period`, or `since` / `until`
    /// (`YYYY-MM-DD`, both inclusive). Neither gives `None`, meaning all of
    /// history. A missing `since` starts at 1970-01-01; a missing `until`
    /// ends today.
    pub fn resolve(
        period: Option<&str>,
        since: Option<&str>,
        until: Option<&str>,
        today: NaiveDate,
    ) -> Result<Option<Period>, PeriodError> {
        let (period, since, until) = (blank(period), blank(since), blank(until));
        match (period, since, until) {
            (Some(_), Some(_), _) | (Some(_), _, Some(_)) => Err(PeriodError::Ambiguous),
            (Some(p), None, None) => Period::parse(p, today).map(Some),
            (None, None, None) => Ok(None),
            (None, s, u) => {
                let since = s.map(parse_day).transpose()?.unwrap_or(EPOCH_DAY);
                let until = u.map(parse_day).transpose()?.unwrap_or(today);
                Period::custom(since, until).map(Some)
            }
        }
    }

    /// Number of days in the range.
    pub fn days(&self) -> u64 {
        (self.end - self.start).num_days().max(0) as u64
    }

    /// The last day in the range (inclusive).
    pub fn last_day(&self) -> NaiveDate {
        self.end - Days::new(1)
    }

    pub fn contains(&self, day: NaiveDate) -> bool {
        self.start <= day && day < self.end
    }

    /// `[start, end)` as epoch seconds at local midnight in `tz`. On a day
    /// whose midnight doesn't exist (a DST jump at 00:00) the day starts at
    /// its first valid instant.
    pub fn bounds_in<Tz: TimeZone>(&self, tz: &Tz) -> (i64, i64) {
        (day_start(self.start, tz), day_start(self.end, tz))
    }

    /// [`Period::bounds_in`] in the machine's local zone.
    pub fn bounds(&self) -> (i64, i64) {
        self.bounds_in(&Local)
    }

    /// The query string that reproduces this period: `period=2026-11` for a
    /// calendar period, `since=..&until=..` (until inclusive) for a custom one.
    pub fn to_query(&self) -> String {
        match self.kind {
            PeriodKind::Custom => format!("since={}&until={}", self.start, self.last_day()),
            _ => format!("period={self}"),
        }
    }
}

impl fmt::Display for Period {
    /// The `period=` spelling (`2026-11`, `2026-W45`, ...); a custom range
    /// prints as `2026-01-01..2026-01-31`, last day inclusive.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.start;
        match self.kind {
            PeriodKind::Week => {
                let w = s.iso_week();
                write!(f, "{}-W{:02}", w.year(), w.week())
            }
            PeriodKind::Month => write!(f, "{}-{:02}", s.year(), s.month()),
            PeriodKind::Quarter => write!(f, "{}-Q{}", s.year(), s.month0() / 3 + 1),
            PeriodKind::Year => write!(f, "{}", s.year()),
            PeriodKind::Custom => write!(f, "{}..{}", s, self.last_day()),
        }
    }
}

/// `None` for a missing or blank query value.
fn blank(o: Option<&str>) -> Option<&str> {
    o.map(str::trim).filter(|s| !s.is_empty())
}

fn day_start<Tz: TimeZone>(day: NaiveDate, tz: &Tz) -> i64 {
    let midnight = day.and_hms_opt(0, 0, 0).expect("midnight");
    // A DST gap is at most a couple of hours; walk forward a minute at a time
    // only in that rare case.
    (0..=180)
        .find_map(|m| {
            tz.from_local_datetime(&(midnight + chrono::Duration::minutes(m)))
                .earliest()
        })
        .expect("a valid instant within three hours of midnight")
        .timestamp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::{New_York, Sao_Paulo};

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn p(s: &str) -> Period {
        Period::parse(s, d(2026, 11, 7)).unwrap()
    }

    #[test]
    fn month_end_and_leap_edges() {
        let jan = Period::containing(PeriodKind::Month, d(2026, 1, 31));
        assert_eq!((jan.start, jan.end), (d(2026, 1, 1), d(2026, 2, 1)));
        // previous of a 31st lands in the shorter month, not past it
        let feb = Period::containing(PeriodKind::Month, d(2028, 3, 31)).previous();
        assert_eq!((feb.start, feb.last_day()), (d(2028, 2, 1), d(2028, 2, 29)));
        assert_eq!(feb.days(), 29);
        let dec = jan.previous();
        assert_eq!((dec.start, dec.end), (d(2025, 12, 1), d(2026, 1, 1)));
    }

    #[test]
    fn iso_weeks_start_monday_and_cross_years() {
        // Saturday 2026-11-07 sits in the week starting Monday 2026-11-02.
        let w = Period::containing(PeriodKind::Week, d(2026, 11, 7));
        assert_eq!((w.start, w.end), (d(2026, 11, 2), d(2026, 11, 9)));
        assert_eq!(w.to_string(), "2026-W45");
        // 2020 has an ISO week 53 running into 2021.
        let w53 = p("2020-W53");
        assert_eq!((w53.start, w53.end), (d(2020, 12, 28), d(2021, 1, 4)));
        assert_eq!(Period::containing(PeriodKind::Week, d(2021, 1, 3)), w53);
        assert!(Period::parse("2021-W53", d(2026, 1, 1)).is_err());
    }

    #[test]
    fn quarters_and_years() {
        let q = Period::current(PeriodKind::Quarter, d(2026, 11, 7));
        assert_eq!((q.start, q.end), (d(2026, 10, 1), d(2027, 1, 1)));
        assert_eq!(q.to_string(), "2026-Q4");
        assert_eq!(q.previous(), p("2026-Q3"));
        assert_eq!(p("2026-Q1").previous(), p("2025-Q4"));
        let y = p("2026");
        assert_eq!(
            (y.start, y.end, y.kind),
            (d(2026, 1, 1), d(2027, 1, 1), PeriodKind::Year)
        );
    }

    #[test]
    fn relative_words() {
        assert_eq!(p("this-month"), p("2026-11"));
        assert_eq!(p("last-month"), p("2026-10"));
        assert_eq!(p("this-week"), p("2026-W45"));
        assert_eq!(p("last-week"), p("2026-W44"));
        assert_eq!(p("last-quarter"), p("2026-Q3"));
        assert_eq!(p("last-year"), p("2025"));
        // January's last-month is December of the year before.
        assert_eq!(
            Period::parse("last-month", d(2026, 1, 15)).unwrap(),
            p("2025-12")
        );
    }

    #[test]
    fn spellings_round_trip_and_bad_ones_fail() {
        for s in [
            "2026-11", "2026-01", "2026-W01", "2026-W45", "2020-W53", "2026-Q4", "2026",
        ] {
            assert_eq!(p(s).to_string(), s);
            assert_eq!(p(s).to_query(), format!("period={s}"));
        }
        for s in [
            "",
            "2026-13",
            "2026-00",
            "2026-1",
            "2026-W54",
            "2026-W5",
            "2026-Q5",
            "2026-Q0",
            "26-11",
            "next-month",
            "this-day",
            "2026-11-01",
            "garbage",
        ] {
            assert!(
                matches!(
                    Period::parse(s, d(2026, 11, 7)),
                    Err(PeriodError::BadSpelling(_))
                ),
                "{s:?} should fail"
            );
        }
    }

    #[test]
    fn custom_ranges_are_inclusive_and_resolve() {
        let c = Period::custom(d(2026, 6, 1), d(2026, 6, 30)).unwrap();
        assert_eq!((c.end, c.days()), (d(2026, 7, 1), 30));
        assert_eq!(c.to_query(), "since=2026-06-01&until=2026-06-30");
        assert_eq!(c.previous().start, d(2026, 5, 2));
        assert_eq!(
            Period::custom(d(2026, 6, 2), d(2026, 6, 1)),
            Err(PeriodError::Reversed)
        );

        let today = d(2026, 11, 7);
        let r = |p, s, u| Period::resolve(p, s, u, today);
        assert_eq!(r(None, None, None), Ok(None));
        assert_eq!(r(Some(" "), Some(""), None), Ok(None));
        assert_eq!(
            r(Some("2026-06"), None, None).unwrap().unwrap(),
            p("2026-06")
        );
        assert_eq!(r(None, Some("2026-06-01"), Some("2026-06-30")), Ok(Some(c)));
        let open = r(None, Some("2026-06-01"), None).unwrap().unwrap();
        assert_eq!(open.last_day(), today);
        let from_start = r(None, None, Some("2026-06-30")).unwrap().unwrap();
        assert_eq!(from_start.start, EPOCH_DAY);
        assert_eq!(
            r(Some("2026-06"), Some("2026-06-01"), None),
            Err(PeriodError::Ambiguous)
        );
        assert!(matches!(
            r(None, Some("1717200000"), None),
            Err(PeriodError::BadDate(_))
        ));
    }

    #[test]
    fn local_day_reads_an_instant_in_the_local_zone() {
        let day = d(2026, 11, 8);
        let start = Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .earliest()
            .unwrap()
            .timestamp();
        // First and last second of the local day, whatever the zone.
        assert_eq!(local_day(start), day);
        assert_eq!(local_day(start + 86_399), day);
        assert_eq!(local_day(start - 1), d(2026, 11, 7));
    }

    #[test]
    fn dst_changes_shift_the_epoch_length_by_an_hour() {
        // New York: DST starts Sun 2026-03-08, ends Sun 2026-11-01.
        let march = p("2026-03");
        let (s, e) = march.bounds_in(&New_York);
        assert_eq!(s, 1_772_341_200); // 2026-03-01 00:00 EST = 05:00 UTC
        assert_eq!(e - s, 31 * 86_400 - 3_600);
        let spring = Period::containing(PeriodKind::Week, d(2026, 3, 8)).bounds_in(&New_York);
        assert_eq!(spring.1 - spring.0, 7 * 86_400 - 3_600);
        let fall = Period::containing(PeriodKind::Week, d(2026, 11, 1)).bounds_in(&New_York);
        assert_eq!(fall.1 - fall.0, 7 * 86_400 + 3_600);
        // A period's end is exactly the next period's start: no gap, no overlap.
        assert_eq!(e, p("2026-04").bounds_in(&New_York).0);
    }

    #[test]
    fn a_missing_midnight_starts_the_day_at_its_first_valid_instant() {
        // Sao Paulo's 2018 DST began at 00:00 on 2018-11-04, so the clock
        // jumped straight to 01:00 local (-02:00 = 03:00 UTC).
        let day = Period::custom(d(2018, 11, 4), d(2018, 11, 4)).unwrap();
        let (s, e) = day.bounds_in(&Sao_Paulo);
        assert_eq!(s, 1_541_300_400);
        assert_eq!(e - s, 23 * 3_600);
    }
}
