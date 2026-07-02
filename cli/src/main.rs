use clap::{Parser, Subcommand, ValueEnum};
use outflow_core::{
    detect, monthly_flow, parse_account_set, spend_by_category, top_merchants, CategorySource,
    Money, Store, TxnFilter,
};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "outflow", about = "Local spending analyzer")]
struct Cli {
    #[arg(long, env = "OUTFLOW_DB", default_value = "outflow.db", global = true)]
    db: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Pull {
        #[arg(long)]
        from_file: Option<String>,
    },
    Categorize,
    Report {
        #[arg(long, value_enum, default_value_t = By::Category)]
        by: By,
        #[arg(long, default_value_t = 15)]
        top: usize,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        until: Option<String>,
        #[arg(long)]
        posted_only: bool,
    },
    Subs,
    Fix {
        txn_id: String,
        category: String,
        #[arg(long)]
        no_learn: bool,
    },
}

#[derive(ValueEnum, Clone)]
enum By {
    Category,
    Merchant,
    Monthly,
}

fn dollars(cents: i64) -> String {
    format!("${}", Money::from_cents(cents).to_display())
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn parse_date(s: &str) -> Option<i64> {
    let mut it = s.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let m: i64 = it.next()?.parse().ok()?;
    let d: i64 = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some(days_from_civil(y, m, d) * 86400)
}

fn build_filter(
    since: &Option<String>,
    until: &Option<String>,
    posted_only: bool,
) -> Result<TxnFilter, String> {
    let mut f = TxnFilter::all();
    if let Some(s) = since {
        f.since = Some(parse_date(s).ok_or_else(|| format!("bad --since date: {s}"))?);
    }
    if let Some(u) = until {
        f.until = Some(parse_date(u).ok_or_else(|| format!("bad --until date: {u}"))?);
    }
    f.include_pending = !posted_only;
    Ok(f)
}

fn open_store(db: &str) -> Result<Store, String> {
    Store::open(db).map_err(|e| format!("open db {db}: {e}"))
}

#[cfg(feature = "net")]
fn access_url() -> Result<String, String> {
    if let Ok(u) = std::env::var("OUTFLOW_SFIN_URL") {
        if !u.is_empty() {
            return Ok(u);
        }
    }
    #[cfg(feature = "keychain")]
    {
        let entry = keyring::Entry::new("outflow", "simplefin-access-url")
            .map_err(|e| format!("keychain: {e}"))?;
        return entry.get_password().map_err(|e| format!("keychain: {e}"));
    }
    #[cfg(not(feature = "keychain"))]
    Err("no OUTFLOW_SFIN_URL set and keychain feature disabled".into())
}

#[cfg(feature = "net")]
fn fetch_live() -> Result<String, String> {
    let url = access_url()?;
    let (creds, base) = match url.split_once("://") {
        Some((scheme, rest)) => match rest.split_once('@') {
            Some((userinfo, host)) => (Some(userinfo.to_string()), format!("{scheme}://{host}")),
            None => (None, url.clone()),
        },
        None => (None, url.clone()),
    };
    let since = (now_secs() - 120 * 86400).max(0);
    let endpoint = format!("{base}/accounts?pending=1&start-date={since}");
    let mut req = ureq::get(&endpoint);
    if let Some(c) = creds {
        if let Some((user, pass)) = c.split_once(':') {
            let token = base64_basic(user, pass);
            req = req.set("Authorization", &format!("Basic {token}"));
        }
    }
    let resp = req.call().map_err(|e| format!("simplefin request: {e}"))?;
    resp.into_string().map_err(|e| format!("read body: {e}"))
}

#[cfg(feature = "net")]
fn now_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(feature = "net")]
fn base64_basic(user: &str, pass: &str) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let raw = format!("{user}:{pass}");
    let b = raw.as_bytes();
    let mut out = String::new();
    for chunk in b.chunks(3) {
        let n = chunk.len();
        let b0 = chunk[0] as u32;
        let b1 = if n > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if n > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(A[((triple >> 18) & 63) as usize] as char);
        out.push(A[((triple >> 12) & 63) as usize] as char);
        out.push(if n > 1 { A[((triple >> 6) & 63) as usize] as char } else { '=' });
        out.push(if n > 2 { A[(triple & 63) as usize] as char } else { '=' });
    }
    out
}

#[cfg(not(feature = "net"))]
fn fetch_live() -> Result<String, String> {
    Err("live pull needs --features net; use `pull --from-file <path>` here".into())
}

fn cmd_pull(store: &Store, from_file: Option<String>) -> Result<(), String> {
    let json = match from_file {
        Some(p) => std::fs::read_to_string(&p).map_err(|e| format!("read {p}: {e}"))?,
        None => fetch_live()?,
    };
    let fetched = parse_account_set(&json).map_err(|e| format!("parse: {e:?}"))?;
    store
        .upsert_accounts(&fetched.accounts)
        .map_err(|e| format!("save accounts: {e}"))?;
    let r = store
        .upsert_transactions(&fetched.transactions)
        .map_err(|e| format!("save transactions: {e}"))?;
    for a in &fetched.accounts {
        println!("  {} [{}]  {}", a.name, a.kind.as_str(), dollars(a.balance.cents()));
    }
    println!("added {}, updated {}", r.added, r.updated);
    Ok(())
}

fn cmd_categorize(store: &Store) -> Result<(), String> {
    let rules = store.ruleset().map_err(|e| format!("{e}"))?;
    let n = store
        .categorize_uncategorized(&rules, CategorySource::Rule)
        .map_err(|e| format!("{e}"))?;
    let remaining = store.uncategorized().map_err(|e| format!("{e}"))?.len();
    println!("categorized {n} by rule; {remaining} still uncategorized");
    Ok(())
}

fn cmd_report(store: &Store, by: By, top: usize, filter: &TxnFilter) -> Result<(), String> {
    match by {
        By::Category => {
            for r in spend_by_category(store, filter).map_err(|e| format!("{e}"))? {
                let name = r.category.unwrap_or_else(|| "(uncategorized)".into());
                println!("{:>12}  {:<24} {}", dollars(r.total_cents), name, r.count);
            }
        }
        By::Merchant => {
            for r in top_merchants(store, filter, top).map_err(|e| format!("{e}"))? {
                println!("{:>12}  {:<28} {}", dollars(r.total_cents), r.merchant, r.count);
            }
        }
        By::Monthly => {
            for r in monthly_flow(store, filter).map_err(|e| format!("{e}"))? {
                println!(
                    "{}-{:02}   in {:>11}   out {:>11}   net {:>11}",
                    r.year,
                    r.month,
                    dollars(r.inflow_cents),
                    dollars(r.outflow_cents),
                    dollars(r.net_cents)
                );
            }
        }
    }
    Ok(())
}

fn cmd_subs(store: &Store) -> Result<(), String> {
    let txns = store.all_transactions().map_err(|e| format!("{e}"))?;
    let subs = detect(&txns);
    if subs.is_empty() {
        println!("no recurring charges detected yet");
        return Ok(());
    }
    for s in subs {
        let cadence = match s.cadence {
            outflow_core::Cadence::Monthly => "monthly",
            outflow_core::Cadence::Yearly => "yearly",
        };
        println!(
            "{:<24} {:>10} {:<8} x{}  total {}",
            s.payee,
            dollars(s.typical_amount_cents),
            cadence,
            s.occurrences,
            dollars(s.total_cents)
        );
    }
    Ok(())
}

fn cmd_fix(store: &Store, txn_id: String, category: String, no_learn: bool) -> Result<(), String> {
    let rule = store
        .set_manual_category(&txn_id, &category, !no_learn)
        .map_err(|e| format!("{e}"))?;
    match rule {
        Some(id) => println!("set {txn_id} = {category}; learned rule #{id}"),
        None => println!("set {txn_id} = {category}"),
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    let store = open_store(&cli.db)?;
    match cli.cmd {
        Cmd::Pull { from_file } => cmd_pull(&store, from_file),
        Cmd::Categorize => cmd_categorize(&store),
        Cmd::Report {
            by,
            top,
            since,
            until,
            posted_only,
        } => {
            let f = build_filter(&since, &until, posted_only)?;
            cmd_report(&store, by, top, &f)
        }
        Cmd::Subs => cmd_subs(&store),
        Cmd::Fix {
            txn_id,
            category,
            no_learn,
        } => cmd_fix(&store, txn_id, category, no_learn),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
