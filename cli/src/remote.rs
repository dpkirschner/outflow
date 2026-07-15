//! HTTP client mode: run CLI subcommands against a running outflow-server
//! (`--server URL` / `OUTFLOW_SERVER`), e.g. over the tailnet from another
//! machine or an agent. Every command maps to one API call and prints the
//! server's JSON verbatim — identical shapes to local `--json` output, so
//! consumers don't care which mode produced it.
//!
//! Auth: `OUTFLOW_API_TOKEN` (if the server requires a bearer token). Env
//! only — never argv.

use crate::{By, Cmd, MatchCmd};

pub struct Remote {
    base: String,
    token: Option<String>,
}

impl Remote {
    pub fn new(server: &str) -> Remote {
        Remote {
            base: format!("{}/api", server.trim_end_matches('/')),
            token: std::env::var("OUTFLOW_API_TOKEN").ok().filter(|t| !t.is_empty()),
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
            Some(b) => req.set("content-type", "application/json").send_string(&b.to_string()),
            None => req.call(),
        };
        match result {
            Ok(resp) => resp.into_string().map_err(|e| format!("read body: {e}")),
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
        Cmd::Report {
            by,
            top,
            since,
            until,
            posted_only,
        } => {
            let mut q = Vec::new();
            if let Some(s) = since {
                q.push(format!(
                    "since={}",
                    crate::parse_date(s).ok_or_else(|| format!("bad --since date: {s}"))?
                ));
            }
            if let Some(u) = until {
                q.push(format!(
                    "until={}",
                    crate::parse_date(u).ok_or_else(|| format!("bad --until date: {u}"))?
                ));
            }
            if *posted_only {
                q.push("pending=false".into());
            }
            let path = match by {
                By::Category => "/spend/categories".to_string(),
                By::Merchant => {
                    q.push(format!("limit={top}"));
                    "/merchants".to_string()
                }
                By::Monthly => "/flow".to_string(),
            };
            let qs = if q.is_empty() { String::new() } else { format!("?{}", q.join("&")) };
            remote.get(&format!("{path}{qs}"))
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
        Cmd::Accounts => remote.get("/accounts"),
        Cmd::Status { limit } => remote.get(&format!("/sync_log?limit={limit}")),
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
            if let Some(s) = &a.since {
                push(
                    "since",
                    crate::parse_date(s)
                        .ok_or_else(|| format!("bad --since date: {s}"))?
                        .to_string(),
                );
            }
            if let Some(u) = &a.until {
                push(
                    "until",
                    crate::parse_date(u)
                        .ok_or_else(|| format!("bad --until date: {u}"))?
                        .to_string(),
                );
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
            remote.get(&format!("/transactions?{}", q.join("&")))
        }
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
