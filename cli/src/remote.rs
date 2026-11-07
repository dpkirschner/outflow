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

use crate::{history_period, By, Cmd, MatchCmd};
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
        Cmd::Subs => remote.get("/subscriptions"),
        Cmd::Fix {
            txn_id,
            category,
            no_learn,
        } => remote.post(
            &format!("/txn/{}/category", enc(txn_id)),
            Some(serde_json::json!({ "category": category, "learn": !no_learn })),
        ),
        Cmd::Accounts { all } => remote.get(&format!("/accounts?include_hidden={all}")),
        Cmd::Status { limit } => remote.get(&format!("/sync_log?limit={limit}")),
        // The server evaluates its own OUTFLOW_PERKS file; --file is local-only.
        Cmd::Perks {
            lookback,
            expiring_days,
            ..
        } => remote.get(&format!(
            "/perks?lookback={}&expiring_days={expiring_days}",
            enc(lookback)
        )),
        Cmd::Report { .. }
        | Cmd::Balances { .. }
        | Cmd::Holdings(_)
        | Cmd::Activity(_)
        | Cmd::Txns(_) => unreachable!("handled by read_path"),
        Cmd::Matches { action } => match action {
            MatchCmd::List { status } => match status {
                Some(s) => remote.get(&format!("/matches?status={}", enc(s))),
                None => remote.get("/matches"),
            },
            MatchCmd::Accept { id } => remote.post(&format!("/matches/{id}/accept"), None),
            MatchCmd::Reject { id } => remote.post(&format!("/matches/{id}/reject"), None),
        },
    }
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
    let period_q = |p: Option<Period>, q: &mut Vec<String>| {
        if let Some(p) = p {
            q.push(p.to_query());
        }
    };
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
            let range = history_period(a.history, &a.period, today)?;
            let mut q = Vec::new();
            if let Some(acct) = &a.account {
                q.push(format!("account={}", enc(acct)));
            }
            if let Some(t) = &a.ticker {
                q.push(format!("ticker={}", enc(t)));
            }
            period_q(range, &mut q);
            let path = if a.history {
                "/holdings/history"
            } else {
                "/holdings"
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
            with_query("/transactions", &q)
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
        assert!(path(&["holdings", "--since", "2026-01-01"]).is_err());
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
    fn bad_periods_fail_before_any_request() {
        let e = path(&["report", "--period", "2026-13"]).unwrap_err();
        assert!(e.contains("bad period"), "{e}");
        let e = path(&["txns", "--period", "2026-11", "--since", "2026-11-01"]).unwrap_err();
        assert!(e.contains("not both"), "{e}");
    }

    #[test]
    fn other_commands_are_not_reads() {
        assert_eq!(path(&["subs"]).unwrap(), None);
    }
}
