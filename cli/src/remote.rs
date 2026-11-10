//! HTTP client mode: run CLI subcommands against a running outflow-server
//! (`--server URL` / `OUTFLOW_SERVER`), e.g. over the tailnet from another
//! machine or an agent. Every command maps to one API call and prints the
//! server's JSON verbatim — identical shapes to local `--json` output, so
//! consumers don't care which mode produced it.
//!
//! Auth: `OUTFLOW_API_TOKEN` (if the server requires a bearer token). Env
//! only — never argv. The server's read-only token goes in the same variable:
//! it is just a bearer token with fewer rights, so read-only callers (agents,
//! scripts) get 403 on any mutating subcommand.

use crate::{history_period, By, Cmd, MatchCmd, PeriodArgs};
use chrono::NaiveDate;
use outflow_core::{today_local, Period};

pub struct Remote {
    base: String,
    token: Option<String>,
}

impl Remote {
    pub fn new(server: &str) -> Remote {
        Remote {
            base: format!("{}/api", server.trim_end_matches('/')),
            token: std::env::var("OUTFLOW_API_TOKEN")
                .ok()
                .filter(|t| !t.is_empty()),
        }
    }

    fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<String, String> {
        let url = format!("{}{}", self.base, path);
        let mut req = ureq::request(method, &url);
        if let Some(t) = &self.token {
            req = req.set("authorization", &format!("Bearer {t}"));
        }
        let result = match body {
            Some(b) => req
                .set("content-type", "application/json")
                .send_string(&b.to_string()),
            None => req.call(),
        };
        match result {
            Ok(resp) => resp.into_string().map_err(|e| format!("read body: {e}")),
            // 403 means the token is real but read-only: say so, rather than
            // leaving a bare status for an agent to guess at (the server sends
            // no body on this one).
            Err(ureq::Error::Status(403, _)) => Err(format!(
                "{url}: HTTP 403: read-only token — this command mutates and \
                 needs the full OUTFLOW_API_TOKEN"
            )),
            Err(ureq::Error::Status(code, resp)) => {
                let detail = resp.into_string().unwrap_or_default();
                Err(format!("{url}: HTTP {code}: {detail}"))
            }
            Err(e) => Err(format!("{url}: {e}")),
        }
    }

    fn get(&self, path: &str) -> Result<String, String> {
        self.call("GET", path, None)
    }

    fn post(&self, path: &str, body: Option<serde_json::Value>) -> Result<String, String> {
        self.call("POST", path, body)
    }
}

fn enc(s: &str) -> String {
    // Percent-encode the characters that matter in a query value.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Dispatch a subcommand to the server; returns the response body to print.
pub fn dispatch(remote: &Remote, cmd: &Cmd) -> Result<String, String> {
    if let Some(path) = read_path(cmd, today_local())? {
        return remote.get(&path);
    }
    match cmd {
        Cmd::Backup { .. } => Err("backup reads the DB file directly; drop --server".into()),
        Cmd::Pull { from_file } => {
            if from_file.is_some() {
                return Err("--from-file reads a LOCAL file; drop --server for it".into());
            }
            remote.post("/pull", None)
        }
        Cmd::Categorize { llm } => {
            let rule = remote.post("/categorize", None)?;
            if *llm {
                let n = remote.post("/categorize_llm", None)?;
                Ok(format!("{rule}\n{{\"llm\":{n}}}"))
            } else {
                Ok(rule)
            }
        }
        Cmd::Fix {
            txn_id,
            category,
            no_learn,
        } => remote.post(
            &format!("/txn/{}/category", enc(txn_id)),
            Some(serde_json::json!({ "category": category, "learn": !no_learn })),
        ),
        Cmd::Accounts { all } => remote.get(&format!("/accounts?include_hidden={all}")),
        // Same shape as local `status --json`: {"version": ..., "upkeep": ...,
        // "sync_log": [...]}, with the server's version. A server older than
        // /version reports null.
        Cmd::Status { limit } => {
            let version = remote
                .get("/version")
                .ok()
                .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
                .map(|v| v["version"].clone())
                .unwrap_or(serde_json::Value::Null);
            let up = remote.get("/upkeep")?;
            let log = remote.get(&format!("/sync_log?limit={limit}"))?;
            Ok(format!(
                "{{\"version\":{version},\"upkeep\":{up},\"sync_log\":{log}}}"
            ))
        }
        // Dismiss, restore, review or unreview: the same params, so the
        // server finds the Finding in the same report.
        Cmd::Find {
            period,
            expiring_days,
            dismiss,
            restore,
            review,
            unreview,
            ..
        } => {
            let (route, body) = match (dismiss, restore, review, unreview) {
                (Some(k), ..) => (
                    "/find/dismissed",
                    serde_json::json!({ "key": k, "dismissed": true }),
                ),
                (_, Some(k), ..) => (
                    "/find/dismissed",
                    serde_json::json!({ "key": k, "dismissed": false }),
                ),
                (_, _, Some(m), _) => (
                    "/find/reviewed",
                    serde_json::json!({ "merchant": m, "reviewed": true }),
                ),
                (.., Some(m)) => (
                    "/find/reviewed",
                    serde_json::json!({ "merchant": m, "reviewed": false }),
                ),
                _ => unreachable!("handled by read_path"),
            };
            let path = find_path(route, period, *expiring_days, today_local())?;
            remote.post(&path, Some(body))
        }
        Cmd::Report { .. }
        | Cmd::Balances { .. }
        | Cmd::Change { .. }
        | Cmd::Checkin { .. }
        | Cmd::Holdings(_)
        | Cmd::Activity(_)
        | Cmd::Recurring { .. }
        | Cmd::Txns(_) => unreachable!("handled by read_path"),
        Cmd::Matches { action } => match action {
            MatchCmd::List { status } => match status {
                Some(s) => remote.get(&format!("/matches?status={}", enc(s))),
                None => remote.get("/matches"),
            },
            MatchCmd::Accept { id } => remote.post(&format!("/matches/{id}/accept"), None),
            MatchCmd::Reject { id } => remote.post(&format!("/matches/{id}/reject"), None),
            MatchCmd::Unaccept { id } => remote.post(&format!("/matches/{id}/unaccept"), None),
        },
    }
}

/// Append a period's wire spelling, if there is one.
fn period_q(p: Option<Period>, q: &mut Vec<String>) {
    if let Some(p) = p {
        q.push(p.to_query());
    }
}

/// `/find` or `/find/dismissed` with the period and `expiring_days`.
fn find_path(
    path: &str,
    period: &PeriodArgs,
    expiring_days: i64,
    today: NaiveDate,
) -> Result<String, String> {
    let mut q = Vec::new();
    period_q(period.resolve_at(today)?, &mut q);
    q.push(format!("expiring_days={expiring_days}"));
    Ok(with_query(path, &q))
}

/// `path?q1&q2...`, or the bare path when there are no parameters.
fn with_query(path: &str, q: &[String]) -> String {
    if q.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", q.join("&"))
    }
}

/// The GET path for the date-filtered reads (`report`, `balances`,
/// `holdings`, `activity`, `txns`); `None` for every other command. The
/// period goes over the wire in its own spelling (`Period::to_query`), so the
/// server resolves the same local-time days the CLI would.
fn read_path(cmd: &Cmd, today: NaiveDate) -> Result<Option<String>, String> {
    let path = match cmd {
        Cmd::Report {
            by,
            top,
            period,
            posted_only,
        } => {
            let mut q = Vec::new();
            period_q(period.resolve_at(today)?, &mut q);
            if *posted_only {
                q.push("pending=false".into());
            }
            let path = match by {
                By::Category => "/spend/categories",
                By::Merchant => {
                    q.push(format!("limit={top}"));
                    "/merchants"
                }
                By::Monthly => "/flow",
                By::Source => "/flow/in-sources",
            };
            with_query(path, &q)
        }
        Cmd::Balances { history, period } => {
            let range = history_period(*history, period, today)?;
            if !*history {
                return Ok(Some("/balances".into()));
            }
            let mut q = Vec::new();
            period_q(range, &mut q);
            with_query("/balances/history", &q)
        }
        Cmd::Holdings(a) => {
            let view = crate::holdings_view(a, today)?;
            let mut q = Vec::new();
            if let Some(acct) = &a.account {
                q.push(format!("account={}", enc(acct)));
            }
            if let Some(t) = &a.ticker {
                q.push(format!("ticker={}", enc(t)));
            }
            let path = match view {
                crate::HoldingsView::Current => "/holdings",
                crate::HoldingsView::History(range) => {
                    period_q(range, &mut q);
                    "/holdings/history"
                }
                crate::HoldingsView::Change(p, _) => {
                    q.push(p.to_query());
                    if let Some(by) = a.by {
                        q.push(format!("by={}", by.as_str()));
                    }
                    "/holdings/change"
                }
            };
            with_query(path, &q)
        }
        Cmd::Activity(a) => {
            let mut q: Vec<String> = [
                ("account", &a.account),
                ("ticker", &a.ticker),
                ("type", &a.kind),
            ]
            .into_iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| format!("{k}={}", enc(v))))
            .collect();
            period_q(a.period.resolve_at(today)?, &mut q);
            with_query("/activity", &q)
        }
        // --ignored only picks which list the text printer shows; the JSON
        // view always carries both.
        Cmd::Change { period } => {
            let p = crate::change_period(period, today)?;
            with_query("/net-worth/change", &[p.to_query()])
        }
        Cmd::Checkin {
            period, categories, ..
        } => {
            let p = crate::checkin_period(period.as_deref(), today)?;
            let path = if *categories {
                "/checkin/categories"
            } else {
                "/checkin"
            };
            with_query(path, &[p.to_query()])
        }
        Cmd::Recurring { period, .. } => {
            let mut q = Vec::new();
            period_q(period.resolve_at(today)?, &mut q);
            with_query("/recurring", &q)
        }
        // The server reads its own OUTFLOW_PERKS; --file is local-only.
        Cmd::Find {
            period,
            expiring_days,
            dismiss: None,
            restore: None,
            review: None,
            unreview: None,
            ..
        } => find_path("/find", period, *expiring_days, today)?,
        Cmd::Txns(a) => {
            let mut q = Vec::new();
            let mut push = |k: &str, v: String| q.push(format!("{k}={v}"));
            if let Some(s) = &a.search {
                push("q", enc(s));
            }
            if let Some(c) = &a.category {
                push("category", enc(c));
            }
            if let Some(acct) = &a.account {
                push("account", enc(acct));
            }
            if let Some(src) = &a.source {
                push("source", enc(src));
            }
            if let Some(f) = &a.flag {
                push("flag", f.wire().into());
            }
            if let Some(l) = &a.lane {
                push("lane", l.to_lane().as_str().into());
            }
            if !a.ids.is_empty() {
                push("ids", enc(&a.ids.join(",")));
            }
            if let Some(m) = &a.merchant {
                push("merchant", enc(m));
            }
            if a.uncategorized {
                push("uncategorized", "true".into());
            }
            if let Some(d) = &a.direction {
                push("direction", d.wire().into());
            }
            if a.posted_only {
                push("pending", "false".into());
            }
            if a.show_excluded {
                push("transfers", "true".into());
            }
            push("sort", a.sort.wire().into());
            push("dir", if a.asc { "asc".into() } else { "desc".into() });
            push("limit", a.limit.to_string());
            push("offset", a.offset.to_string());
            period_q(a.period.resolve_at(today)?, &mut q);
            let path = if a.by_month {
                "/flow/series"
            } else {
                "/transactions"
            };
            with_query(path, &q)
        }
        _ => return Ok(None),
    };
    Ok(Some(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cli;
    use clap::Parser;

    /// A fixed "today" so relative periods are stable (invented date).
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()
    }

    fn path(args: &[&str]) -> Result<Option<String>, String> {
        let mut argv = vec!["outflow"];
        argv.extend_from_slice(args);
        let cli = Cli::try_parse_from(argv).expect("args parse");
        read_path(&cli.cmd, today())
    }

    fn ok(args: &[&str]) -> String {
        path(args).unwrap().expect("a read path")
    }

    #[test]
    fn recurring_and_merchant_filter_paths() {
        assert_eq!(ok(&["recurring"]), "/recurring");
        assert_eq!(
            ok(&["recurring", "--period", "last-month"]),
            "/recurring?period=2026-10"
        );
        assert!(ok(&["txns", "--merchant", "streamflix com"]).contains("merchant=streamflix%20com"));
    }

    #[test]
    fn find_reads_but_dismiss_and_restore_post() {
        assert_eq!(ok(&["find"]), "/find?expiring_days=14");
        assert_eq!(
            path(&["find", "--dismiss", "add_offer:4242:fubo"]),
            Ok(None)
        );
        assert_eq!(
            path(&["find", "--restore", "add_offer:4242:fubo"]),
            Ok(None)
        );
        assert_eq!(
            find_path("/find/dismissed", &PeriodArgs::default(), 7, today()),
            Ok("/find/dismissed?expiring_days=7".to_string())
        );
        assert_eq!(path(&["find", "--review", "streamflix"]), Ok(None));
        assert_eq!(path(&["find", "--unreview", "streamflix"]), Ok(None));
        let both = Cli::try_parse_from(["outflow", "find", "--review", "a", "--dismiss", "b"]);
        assert!(both.is_err(), "one mark at a time");
    }

    #[test]
    fn change_defaults_to_this_month() {
        assert_eq!(ok(&["change"]), "/net-worth/change?period=2026-11");
        assert_eq!(
            ok(&["change", "--period", "last-month"]),
            "/net-worth/change?period=2026-10"
        );
    }

    #[test]
    fn report_sends_the_period_spelling() {
        assert_eq!(
            ok(&["report", "--by", "monthly", "--period", "last-month"]),
            "/flow?period=2026-10"
        );
        assert_eq!(
            ok(&["report", "--by", "merchant", "--top", "5", "--period", "2026-Q3"]),
            "/merchants?period=2026-Q3&limit=5"
        );
        assert_eq!(ok(&["report"]), "/spend/categories");
        assert_eq!(
            ok(&["report", "--by", "source", "--period", "last-month"]),
            "/flow/in-sources?period=2026-10"
        );
    }

    #[test]
    fn checkin_categories_reads_its_own_path() {
        assert_eq!(
            ok(&["checkin", "--categories", "--period", "last-month"]),
            "/checkin/categories?period=2026-10"
        );
        assert_eq!(
            ok(&["checkin", "--period", "2026-10"]),
            "/checkin?period=2026-10"
        );
    }

    #[test]
    fn custom_range_keeps_until_inclusive() {
        assert_eq!(
            ok(&[
                "report",
                "--since",
                "2026-01-05",
                "--until",
                "2026-01-20",
                "--posted-only"
            ]),
            "/spend/categories?since=2026-01-05&until=2026-01-20&pending=false"
        );
        // since alone runs through today.
        assert_eq!(
            ok(&["activity", "--type", "buy", "--since", "2026-10-01"]),
            "/activity?type=buy&since=2026-10-01&until=2026-11-07"
        );
    }

    #[test]
    fn balances_history_takes_a_period() {
        assert_eq!(ok(&["balances"]), "/balances");
        assert_eq!(ok(&["balances", "--history"]), "/balances/history");
        assert_eq!(
            ok(&["balances", "--history", "--period", "this-year"]),
            "/balances/history?period=2026"
        );
        assert!(path(&["balances", "--period", "2026"]).is_err());
    }

    #[test]
    fn holdings_and_txns_append_the_period() {
        assert_eq!(
            ok(&[
                "holdings",
                "--history",
                "--ticker",
                "ABC",
                "--period",
                "2026-W45"
            ]),
            "/holdings/history?ticker=ABC&period=2026-W45"
        );
        assert_eq!(
            ok(&["holdings", "--since", "2026-01-01", "--until", "2026-01-31"]),
            "/holdings/change?since=2026-01-01&until=2026-01-31"
        );
        assert_eq!(
            ok(&["holdings", "--ticker", "ABC", "--by", "position"]),
            "/holdings/change?ticker=ABC&period=2026-11&by=position"
        );
        assert_eq!(
            ok(&["txns", "--limit", "10", "--period", "2026-11"]),
            "/transactions?sort=date&dir=desc&limit=10&offset=0&period=2026-11"
        );
    }

    #[test]
    fn txns_lane_filters_go_on_the_wire() {
        assert_eq!(
            ok(&[
                "txns",
                "--lane",
                "card-payment",
                "--direction",
                "out",
                "--uncategorized",
                "--period",
                "2026-11"
            ]),
            "/transactions?lane=card_payment&uncategorized=true&direction=out\
             &sort=date&dir=desc&limit=50&offset=0&period=2026-11"
        );
    }

    #[test]
    fn txns_by_month_reads_the_series() {
        assert_eq!(
            ok(&[
                "txns",
                "--lane",
                "out",
                "--category",
                "Groceries",
                "--by-month"
            ]),
            "/flow/series?category=Groceries&lane=out&sort=date&dir=desc&limit=50&offset=0"
        );
    }

    #[test]
    fn bad_periods_fail_before_any_request() {
        let e = path(&["report", "--period", "2026-13"]).unwrap_err();
        assert!(e.contains("bad period"), "{e}");
        let e = path(&["txns", "--period", "2026-11", "--since", "2026-11-01"]).unwrap_err();
        assert!(e.contains("not both"), "{e}");
    }

    #[test]
    fn other_commands_are_not_reads() {
        assert_eq!(path(&["status"]).unwrap(), None);
    }
}
