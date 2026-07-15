use clap::{Args, Parser, Subcommand, ValueEnum};
use outflow_core::{
    detect, monthly_flow, parse_account_set, search_transactions, spend_by_category,
    top_merchants, CategorySource, MatchStatus, Money, SortKey, Store, TxnFilter, TxnFlag,
    TxnQuery,
};
use std::process::ExitCode;

#[cfg(feature = "client")]
mod remote;

#[derive(Parser)]
#[command(name = "outflow", about = "Local spending analyzer")]
struct Cli {
    #[arg(long, env = "OUTFLOW_DB", default_value = "outflow.db", global = true)]
    db: String,
    /// Run against a remote outflow-server instead of a local DB
    /// (needs --features client). Auth via OUTFLOW_API_TOKEN if set.
    #[arg(long, env = "OUTFLOW_SERVER", global = true)]
    server: Option<String>,
    /// Machine-readable JSON output (server mode always emits JSON).
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
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
    /// Search/sort/filter the transaction list.
    Txns(TxnsArgs),
    /// List ingested accounts.
    Accounts,
    /// Card-payment match review (list / accept / reject).
    Matches {
        #[command(subcommand)]
        action: MatchCmd,
    },
    /// Recent sync runs.
    Status {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Args)]
pub(crate) struct TxnsArgs {
    /// Case-insensitive text over payee/description/category/merchant.
    #[arg(long)]
    pub search: Option<String>,
    #[arg(long)]
    pub category: Option<String>,
    /// Account id (see `accounts`).
    #[arg(long)]
    pub account: Option<String>,
    /// Provenance: simplefin | plaid.
    #[arg(long)]
    pub source: Option<String>,
    /// Keep only this flag (transfer/card-payment imply --show-excluded).
    #[arg(long, value_enum)]
    pub flag: Option<FlagArg>,
    #[arg(long)]
    pub since: Option<String>,
    #[arg(long)]
    pub until: Option<String>,
    #[arg(long)]
    pub posted_only: bool,
    /// Include Transfer/CardPayment rows.
    #[arg(long)]
    pub show_excluded: bool,
    #[arg(long, value_enum, default_value_t = SortArg::Date)]
    pub sort: SortArg,
    /// Ascending sort (default is descending).
    #[arg(long)]
    pub asc: bool,
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

#[derive(Subcommand)]
pub(crate) enum MatchCmd {
    List {
        /// proposed | accepted | rejected (default: all)
        #[arg(long)]
        status: Option<String>,
    },
    Accept {
        id: i64,
    },
    Reject {
        id: i64,
    },
}

#[derive(ValueEnum, Clone, Copy)]
pub(crate) enum FlagArg {
    Spending,
    Transfer,
    CardPayment,
}

impl FlagArg {
    fn to_flag(self) -> TxnFlag {
        match self {
            FlagArg::Spending => TxnFlag::Spending,
            FlagArg::Transfer => TxnFlag::Transfer,
            FlagArg::CardPayment => TxnFlag::CardPayment,
        }
    }

    /// Serde variant name on the wire (server mode).
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn wire(self) -> &'static str {
        match self {
            FlagArg::Spending => "Spending",
            FlagArg::Transfer => "Transfer",
            FlagArg::CardPayment => "CardPayment",
        }
    }
}

#[derive(ValueEnum, Clone, Copy)]
pub(crate) enum SortArg {
    Date,
    Amount,
    Merchant,
    Category,
}

impl SortArg {
    fn to_key(self) -> SortKey {
        match self {
            SortArg::Date => SortKey::Date,
            SortArg::Amount => SortKey::Amount,
            SortArg::Merchant => SortKey::Merchant,
            SortArg::Category => SortKey::Category,
        }
    }

    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn wire(self) -> &'static str {
        match self {
            SortArg::Date => "date",
            SortArg::Amount => "amount",
            SortArg::Merchant => "merchant",
            SortArg::Category => "category",
        }
    }
}

#[derive(ValueEnum, Clone)]
pub(crate) enum By {
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

pub(crate) fn parse_date(s: &str) -> Option<i64> {
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

fn json_out<T: serde::Serialize>(v: &T) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string(v).map_err(|e| format!("serialize: {e}"))?
    );
    Ok(())
}

fn cmd_txns(store: &Store, a: &TxnsArgs, json: bool) -> Result<(), String> {
    let mut filter = build_filter(&a.since, &a.until, a.posted_only)?;
    filter.include_non_spending = a.show_excluded
        || matches!(a.flag, Some(FlagArg::Transfer) | Some(FlagArg::CardPayment));
    let q = TxnQuery {
        filter,
        text: a.search.clone(),
        account_id: a.account.clone(),
        category: a.category.clone(),
        source: a.source.clone(),
        flag: a.flag.map(FlagArg::to_flag),
        min_cents: None,
        max_cents: None,
        sort: a.sort.to_key(),
        descending: !a.asc,
        offset: a.offset,
        limit: a.limit,
    };
    let page = search_transactions(store, &q).map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&page);
    }
    for t in &page.items {
        println!(
            "{:>12}  {:<10}  {:<32} {:<14} {}",
            dollars(t.amount.cents()),
            date_label(t.effective_date()),
            truncate(t.merchant(), 32),
            t.category.as_deref().unwrap_or("—"),
            t.id
        );
    }
    println!(
        "{} of {} shown · net {}",
        page.items.len(),
        page.total,
        dollars(page.total_cents)
    );
    Ok(())
}

fn date_label(secs: i64) -> String {
    // Coarse UTC day label; fine for a terminal listing.
    let days = secs / 86_400;
    let (y, m, d) = civil_from_days(days);
    format!("{y}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).collect::<String>() + "…"
    }
}

fn cmd_accounts(store: &Store, json: bool) -> Result<(), String> {
    let accounts = store.accounts().map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&accounts);
    }
    for a in &accounts {
        println!(
            "{:<40} {:<9} {:>12}  [{}] {}",
            a.name,
            a.kind.as_str(),
            dollars(a.balance.cents()),
            a.source,
            a.id
        );
    }
    Ok(())
}

fn cmd_status(store: &Store, limit: usize, json: bool) -> Result<(), String> {
    let log = store.sync_log(limit).map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&log);
    }
    for s in &log {
        let outcome = match (s.finished, &s.note) {
            (None, _) => "running".to_string(),
            (Some(_), Some(n)) => format!("failed: {n}"),
            (Some(_), None) => format!("+{} ~{}", s.added.unwrap_or(0), s.updated.unwrap_or(0)),
        };
        println!("#{:<4} {:<24} {}  {}", s.id, s.source, date_label(s.started), outcome);
    }
    Ok(())
}

/// Local match handling mirrors the server handlers exactly: accept flags both
/// legs CardPayment and retires competing proposals; rejecting an accepted
/// match restores both legs to Spending.
fn cmd_matches(store: &Store, action: &MatchCmd, json: bool) -> Result<(), String> {
    match action {
        MatchCmd::List { status } => {
            let status = match status.as_deref() {
                None => None,
                Some(s) => Some(
                    MatchStatus::from_str(&s.to_lowercase())
                        .ok_or_else(|| format!("bad status {s:?}"))?,
                ),
            };
            let ms = store.matches(status).map_err(|e| format!("{e}"))?;
            if json {
                return json_out(&ms);
            }
            for m in &ms {
                println!(
                    "#{:<4} {:<9} {:<7} {} ↔ {}  {}",
                    m.id,
                    format!("{:?}", m.status).to_lowercase(),
                    format!("{:?}", m.confidence).to_lowercase(),
                    m.bank_txn_id,
                    m.card_txn_id,
                    m.reason.as_deref().unwrap_or("")
                );
            }
            Ok(())
        }
        MatchCmd::Accept { id } => {
            let m = store
                .match_by_id(*id)
                .map_err(|e| format!("{e}"))?
                .ok_or_else(|| format!("no match {id}"))?;
            store
                .set_flag(&m.bank_txn_id, TxnFlag::CardPayment, false)
                .map_err(|e| format!("{e}"))?;
            store
                .set_flag(&m.card_txn_id, TxnFlag::CardPayment, false)
                .map_err(|e| format!("{e}"))?;
            store
                .set_match_status(*id, MatchStatus::Accepted)
                .map_err(|e| format!("{e}"))?;
            store
                .reject_conflicting_proposals(*id, &m.bank_txn_id, &m.card_txn_id)
                .map_err(|e| format!("{e}"))?;
            println!("accepted #{id}: both legs excluded as card payment");
            Ok(())
        }
        MatchCmd::Reject { id } => {
            let m = store
                .match_by_id(*id)
                .map_err(|e| format!("{e}"))?
                .ok_or_else(|| format!("no match {id}"))?;
            if m.status == MatchStatus::Accepted {
                store
                    .set_flag(&m.bank_txn_id, TxnFlag::Spending, false)
                    .map_err(|e| format!("{e}"))?;
                store
                    .set_flag(&m.card_txn_id, TxnFlag::Spending, false)
                    .map_err(|e| format!("{e}"))?;
            }
            store
                .set_match_status(*id, MatchStatus::Rejected)
                .map_err(|e| format!("{e}"))?;
            println!("rejected #{id}");
            Ok(())
        }
    }
}

#[cfg(feature = "client")]
fn run_remote(server: &str, cmd: &Cmd) -> Result<(), String> {
    let remote = remote::Remote::new(server);
    let body = remote::dispatch(&remote, cmd)?;
    println!("{body}");
    Ok(())
}

#[cfg(not(feature = "client"))]
fn run_remote(_server: &str, _cmd: &Cmd) -> Result<(), String> {
    Err("--server needs --features client".into())
}

fn run() -> Result<(), String> {
    let cli = Cli::parse();
    // Server mode: every command maps to one API call; output is the server's
    // JSON (identical shapes to local --json).
    if let Some(server) = &cli.server {
        return run_remote(server, &cli.cmd);
    }
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
            if cli.json {
                return report_json(&store, by, top, &f);
            }
            cmd_report(&store, by, top, &f)
        }
        Cmd::Subs => {
            if cli.json {
                let txns = store.all_transactions().map_err(|e| format!("{e}"))?;
                return json_out(&detect(&txns));
            }
            cmd_subs(&store)
        }
        Cmd::Fix {
            txn_id,
            category,
            no_learn,
        } => cmd_fix(&store, txn_id, category, no_learn),
        Cmd::Txns(args) => cmd_txns(&store, &args, cli.json),
        Cmd::Accounts => cmd_accounts(&store, cli.json),
        Cmd::Matches { action } => cmd_matches(&store, &action, cli.json),
        Cmd::Status { limit } => cmd_status(&store, limit, cli.json),
    }
}

fn report_json(store: &Store, by: By, top: usize, filter: &TxnFilter) -> Result<(), String> {
    match by {
        By::Category => json_out(&spend_by_category(store, filter).map_err(|e| format!("{e}"))?),
        By::Merchant => json_out(&top_merchants(store, filter, top).map_err(|e| format!("{e}"))?),
        By::Monthly => json_out(&monthly_flow(store, filter).map_err(|e| format!("{e}"))?),
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
