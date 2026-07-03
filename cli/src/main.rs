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
    /// Exchange a SimpleFIN setup token for a durable access URL.
    Claim {
        setup_token: String,
    },
    Pull {
        #[arg(long)]
        from_file: Option<String>,
    },
    Categorize {
        /// After the rule pass, send the remaining uncategorized merchants to
        /// an LLM (needs --features net and ANTHROPIC_API_KEY).
        #[arg(long)]
        llm: bool,
    },
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
    // With the encryption feature, an OUTFLOW_DB_KEY opens the DB via SQLCipher.
    // The key is a secret: read from the environment only, never argv or the DB.
    #[cfg(feature = "encryption")]
    {
        if let Ok(key) = std::env::var("OUTFLOW_DB_KEY") {
            if !key.is_empty() {
                return Store::open_encrypted(db, &key)
                    .map_err(|e| format!("open encrypted db {db}: {e}"));
            }
        }
    }
    Store::open(db).map_err(|e| format!("open db {db}: {e}"))
}

#[cfg(feature = "net")]
fn fetch_live() -> Result<String, String> {
    let url = outflow_net::access_url()?;
    outflow_net::fetch(&url)
}

/// Exchange a base64 setup token for an access URL and persist it.
#[cfg(feature = "net")]
fn claim(setup_token: &str) -> Result<(), String> {
    let access_url = outflow_net::claim_access_url(setup_token)?;
    store_access_url(&access_url)
}

/// Persist the access URL and tell the user where it went. Precedence lives in
/// `outflow_net::persist_access_url`; this only maps the outcome to a message.
#[cfg(feature = "net")]
fn store_access_url(access_url: &str) -> Result<(), String> {
    use outflow_net::Persisted;
    match outflow_net::persist_access_url(access_url)? {
        Persisted::File(p) => println!(
            "wrote access URL to {p} (0600); `pull` will read it via OUTFLOW_SFIN_URL_FILE"
        ),
        Persisted::Keychain => {
            println!("stored access URL in keychain (service=outflow); `pull` will use it")
        }
        Persisted::Ephemeral => {
            println!("access URL obtained. Persist it for future pulls, e.g.:");
            println!("  export OUTFLOW_SFIN_URL='{access_url}'");
        }
    }
    Ok(())
}

#[cfg(not(feature = "net"))]
fn claim(_setup_token: &str) -> Result<(), String> {
    Err("claim needs --features net".into())
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
    for w in &fetched.warnings {
        eprintln!("warning: {w}");
    }
    for a in &fetched.accounts {
        println!("  {} [{}]  {}", a.name, a.kind.as_str(), dollars(a.balance.cents()));
    }
    println!("added {}, updated {}", r.added, r.updated);
    Ok(())
}

fn cmd_categorize(store: &Store, llm: bool) -> Result<(), String> {
    let rules = store.ruleset().map_err(|e| format!("{e}"))?;
    let n = store
        .categorize_uncategorized(&rules, CategorySource::Rule)
        .map_err(|e| format!("{e}"))?;
    println!("categorized {n} by rule");
    if llm {
        let m = categorize_llm(store)?;
        println!("categorized {m} by llm");
    }
    let remaining = store.uncategorized().map_err(|e| format!("{e}"))?.len();
    println!("{remaining} still uncategorized");
    Ok(())
}

#[cfg(feature = "net")]
fn categorize_llm(store: &Store) -> Result<usize, String> {
    use outflow_core::{normalize_payee, LlmCategorizer, MerchantSample};
    use outflow_net::AnthropicPrompter;
    use std::collections::HashMap;

    let uncategorized = store.uncategorized().map_err(|e| format!("{e}"))?;
    if uncategorized.is_empty() {
        return Ok(0);
    }
    // One decision per distinct merchant; keep a representative amount.
    let mut samples: Vec<MerchantSample> = Vec::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for t in &uncategorized {
        let key = normalize_payee(t.merchant());
        if key.is_empty() || seen.insert(key.clone(), ()).is_some() {
            continue;
        }
        samples.push(MerchantSample {
            merchant: key,
            amount_cents: t.amount.cents(),
        });
    }

    let categories = store.categories().map_err(|e| format!("{e}"))?;
    if categories.is_empty() {
        return Err("no categories in vocabulary; nothing to constrain the LLM to".into());
    }
    let categorizer = LlmCategorizer::new(AnthropicPrompter::from_env()?, categories);
    let suggestions = categorizer
        .suggest(&samples)
        .map_err(|e| format!("llm: {e:?}"))?;

    let by_merchant: HashMap<String, String> = suggestions
        .into_iter()
        .map(|s| (s.merchant, s.category))
        .collect();

    let mut applied = 0usize;
    for t in &uncategorized {
        let key = normalize_payee(t.merchant());
        if let Some(cat) = by_merchant.get(&key) {
            store
                .set_category(&t.id, cat, CategorySource::Llm)
                .map_err(|e| format!("{e}"))?;
            applied += 1;
        }
    }
    Ok(applied)
}

#[cfg(not(feature = "net"))]
fn categorize_llm(_store: &Store) -> Result<usize, String> {
    Err("--llm needs --features net".into())
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
    // Claim is a bootstrap step that needs no database.
    if let Cmd::Claim { setup_token } = &cli.cmd {
        return claim(setup_token);
    }
    let store = open_store(&cli.db)?;
    match cli.cmd {
        Cmd::Claim { .. } => unreachable!("handled above"),
        Cmd::Pull { from_file } => cmd_pull(&store, from_file),
        Cmd::Categorize { llm } => cmd_categorize(&store, llm),
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
