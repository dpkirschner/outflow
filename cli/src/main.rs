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
fn access_url() -> Result<String, String> {
    // 1. Explicit value in the environment (never logged, never in argv).
    if let Ok(u) = std::env::var("OUTFLOW_SFIN_URL") {
        if !u.is_empty() {
            return Ok(u);
        }
    }
    // 2. A 0600 secret file. This is the headless-cron path: unlike the login
    //    keychain, a file does not require an unlocked GUI session, so a
    //    launchd/cron job on a headless Mac can read it.
    if let Ok(p) = std::env::var("OUTFLOW_SFIN_URL_FILE") {
        if !p.is_empty() {
            return read_secret_file(&p);
        }
    }
    // 3. OS keychain — the default for an interactive machine (written by `claim`).
    #[cfg(feature = "keychain")]
    {
        let entry = keyring::Entry::new("outflow", "simplefin-access-url")
            .map_err(|e| format!("keychain: {e}"))?;
        return entry.get_password().map_err(|e| {
            format!("keychain: {e} (headless? set OUTFLOW_SFIN_URL_FILE to a 0600 file instead)")
        });
    }
    #[cfg(not(feature = "keychain"))]
    Err("no access URL: set OUTFLOW_SFIN_URL, or OUTFLOW_SFIN_URL_FILE to a 0600 file".into())
}

/// Read an access URL from a file, refusing it if group/other can read it.
#[cfg(feature = "net")]
fn read_secret_file(path: &str) -> Result<String, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path)
            .map_err(|e| format!("stat {path}: {e}"))?
            .permissions()
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!(
                "{path} is group/other-accessible (mode {:o}); run `chmod 600 {path}`",
                mode & 0o777
            ));
        }
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
    let url = contents.trim();
    if url.is_empty() {
        return Err(format!("{path} is empty"));
    }
    Ok(url.to_string())
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
    let since = (now_secs() - 90 * 86400).max(0);
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

#[cfg(feature = "net")]
fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let clean: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    for chunk in clean.chunks(4) {
        let mut acc = 0u32;
        let mut bits = 0;
        for &c in chunk {
            let v = val(c).ok_or_else(|| format!("invalid base64 char {:?}", c as char))?;
            acc = (acc << 6) | v;
            bits += 6;
        }
        // Emit the high-order full bytes gathered from this chunk.
        for shift in (0..bits / 8).map(|i| bits - 8 * (i + 1)) {
            out.push(((acc >> shift) & 0xff) as u8);
        }
    }
    Ok(out)
}

/// Exchange a base64 setup token for an access URL and persist it.
#[cfg(feature = "net")]
fn claim(setup_token: &str) -> Result<(), String> {
    let decoded = base64_decode(setup_token.trim())
        .map_err(|e| format!("decode setup token: {e}"))?;
    let claim_url = String::from_utf8(decoded)
        .map_err(|_| "setup token did not decode to a URL".to_string())?;
    let claim_url = claim_url.trim();
    if !claim_url.starts_with("http") {
        return Err(format!("decoded claim URL looks wrong: {claim_url}"));
    }
    let resp = ureq::post(claim_url)
        .call()
        .map_err(|e| format!("claim request: {e}"))?;
    let access_url = resp
        .into_string()
        .map_err(|e| format!("read claim body: {e}"))?
        .trim()
        .to_string();
    if !access_url.starts_with("http") {
        return Err(format!("claim did not return an access URL: {access_url}"));
    }
    store_access_url(&access_url)
}

/// Persist the access URL. Precedence mirrors `access_url` reads:
/// an explicit `OUTFLOW_SFIN_URL_FILE` wins (the headless path), else the
/// keychain, else we just print an export line for the user to place.
#[cfg(feature = "net")]
fn store_access_url(access_url: &str) -> Result<(), String> {
    if let Ok(p) = std::env::var("OUTFLOW_SFIN_URL_FILE") {
        if !p.is_empty() {
            return write_secret_file(&p, access_url);
        }
    }
    #[cfg(feature = "keychain")]
    {
        let entry = keyring::Entry::new("outflow", "simplefin-access-url")
            .map_err(|e| format!("keychain: {e}"))?;
        entry
            .set_password(access_url)
            .map_err(|e| format!("keychain store: {e}"))?;
        println!("stored access URL in keychain (service=outflow); `pull` will use it");
        return Ok(());
    }
    #[cfg(not(feature = "keychain"))]
    {
        println!("access URL obtained. Persist it for future pulls, e.g.:");
        println!("  export OUTFLOW_SFIN_URL='{access_url}'");
        Ok(())
    }
}

/// Write the access URL to a file with 0600 permissions (owner-only).
#[cfg(feature = "net")]
fn write_secret_file(path: &str, access_url: &str) -> Result<(), String> {
    std::fs::write(path, format!("{access_url}\n")).map_err(|e| format!("write {path}: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("chmod {path}: {e}"))?;
    }
    println!("wrote access URL to {path} (0600); `pull` will read it via OUTFLOW_SFIN_URL_FILE");
    Ok(())
}

/// Anthropic Messages API adapter for the core `Prompter` trait. Lives in the
/// CLI (where ureq already lives) so `core` stays network-free. Endpoint and
/// model are env-configurable; auth is `ANTHROPIC_API_KEY`.
#[cfg(feature = "net")]
struct AnthropicPrompter {
    url: String,
    model: String,
    api_key: String,
}

#[cfg(feature = "net")]
impl AnthropicPrompter {
    fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| "ANTHROPIC_API_KEY not set".to_string())?;
        if api_key.is_empty() {
            return Err("ANTHROPIC_API_KEY is empty".into());
        }
        let url = std::env::var("OUTFLOW_LLM_URL")
            .unwrap_or_else(|_| "https://api.anthropic.com/v1/messages".into());
        let model =
            std::env::var("OUTFLOW_LLM_MODEL").unwrap_or_else(|_| "claude-opus-4-8".into());
        Ok(AnthropicPrompter {
            url,
            model,
            api_key,
        })
    }
}

#[cfg(feature = "net")]
impl outflow_core::Prompter for AnthropicPrompter {
    fn complete(&self, system: &str, user: &str) -> Result<String, outflow_core::LlmError> {
        use outflow_core::LlmError;

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "system": system,
            "messages": [{ "role": "user", "content": user }],
        })
        .to_string();
        let resp = ureq::post(&self.url)
            .set("x-api-key", &self.api_key)
            .set("anthropic-version", "2023-06-01")
            .set("content-type", "application/json")
            .send_string(&body)
            .map_err(|e| LlmError::Transport(format!("{e}")))?;
        let raw = resp
            .into_string()
            .map_err(|e| LlmError::Transport(format!("read body: {e}")))?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| LlmError::Parse(format!("response not JSON: {e}")))?;
        // Concatenate all text blocks in content[].
        let text: String = value
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if text.is_empty() {
            return Err(LlmError::Parse(format!(
                "no text in response: {}",
                value
            )));
        }
        Ok(text)
    }
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
