use chrono::NaiveDate;
use clap::{Args, Parser, Subcommand, ValueEnum};
use outflow_core::{
    account_display_label, activity, balance_history, evaluate_perks, flow_lanes, load_offers,
    parse_fixture, position_history, positions, search_transactions, spend_by_category,
    summarize_balances, today_local, top_merchants, ActivityFilter, Cap, CategorySource, Direction,
    FindingKind, Lane, MatchStatus, Money, Offer, Period, PerkOptions, PositionFilter, Reward,
    SortKey, Store, TxnFilter, TxnFlag, TxnQuery,
};
use outflow_core::{recurring, Change, StreamCadence};
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

/// The one date range every date-filtered subcommand takes. Give `--period`,
/// or `--since`/`--until`, not both; none means all of history.
#[derive(Args, Default)]
pub(crate) struct PeriodArgs {
    /// A calendar period: 2026-11 (month), 2026-W45 (ISO week), 2026-Q4,
    /// 2026 (year), or this-/last- week|month|quarter|year
    /// (e.g. last-month). Local-time days.
    #[arg(long, value_name = "P")]
    pub period: Option<String>,
    /// First day, YYYY-MM-DD (inclusive). Alone: through today.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub since: Option<String>,
    /// Last day, YYYY-MM-DD (inclusive). Alone: from the start of history.
    #[arg(long, value_name = "YYYY-MM-DD")]
    pub until: Option<String>,
}

impl PeriodArgs {
    /// The resolved range, anchored on today's local date.
    pub(crate) fn resolve(&self) -> Result<Option<Period>, String> {
        self.resolve_at(today_local())
    }

    pub(crate) fn resolve_at(&self, today: NaiveDate) -> Result<Option<Period>, String> {
        Period::resolve(
            self.period.as_deref(),
            self.since.as_deref(),
            self.until.as_deref(),
            today,
        )
        .map_err(|e| e.to_string())
    }
}

/// For `balances` / `holdings`, whose range only applies to `--history`.
pub(crate) fn history_period(
    history: bool,
    p: &PeriodArgs,
    today: NaiveDate,
) -> Result<Option<Period>, String> {
    let period = p.resolve_at(today)?;
    if period.is_some() && !history {
        return Err("--period/--since/--until apply to --history only".into());
    }
    Ok(period)
}

#[derive(Args)]
pub(crate) struct HoldingsArgs {
    /// Account id, or part of its name/nickname (case-insensitive).
    #[arg(long)]
    account: Option<String>,
    /// Ticker or security id (case-insensitive).
    #[arg(long)]
    ticker: Option<String>,
    /// Per-sync position history instead of current positions.
    #[arg(long)]
    history: bool,
    /// With --history: the snapshot range.
    #[command(flatten)]
    period: PeriodArgs,
}

#[derive(Args)]
pub(crate) struct ActivityArgs {
    /// Account id, or part of its name/nickname (case-insensitive).
    #[arg(long)]
    account: Option<String>,
    /// Ticker or security id (case-insensitive).
    #[arg(long)]
    ticker: Option<String>,
    /// Plaid type or subtype: buy, sell, dividend, contribution, fee, ...
    #[arg(long = "type")]
    kind: Option<String>,
    #[command(flatten)]
    period: PeriodArgs,
}

impl ActivityArgs {
    /// Core's filter takes inclusive `YYYY-MM-DD` bounds: the period's first
    /// and last day.
    fn filter(&self, period: Option<Period>) -> ActivityFilter {
        ActivityFilter {
            account: self.account.clone(),
            ticker: self.ticker.clone(),
            kind: self.kind.clone(),
            since: period.map(|p| p.start.to_string()),
            until: period.map(|p| p.last_day().to_string()),
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum Cmd {
    /// Ingest a Plaid fixture file offline (`--from-file`); live syncs run on
    /// the server (use `--server URL pull`).
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
        #[command(flatten)]
        period: PeriodArgs,
        #[arg(long)]
        posted_only: bool,
    },
    /// Recurring merchants over the 12 months ending with the period's last
    /// month (default: this month): cadence, usual monthly cost, change
    /// (new / up / down >25% vs usual). --json prints the whole view,
    /// ignored merchants included.
    Recurring {
        #[command(flatten)]
        period: PeriodArgs,
        /// List the ignored merchants instead.
        #[arg(long)]
        ignored: bool,
    },
    Fix {
        txn_id: String,
        category: String,
        #[arg(long)]
        no_learn: bool,
    },
    /// Search/sort/filter the transaction list.
    Txns(TxnsArgs),
    /// List ingested accounts (one balance each; see `holdings` for the
    /// positions inside investment accounts).
    Accounts {
        /// Include hidden accounts (marked `hidden`).
        #[arg(long)]
        all: bool,
    },
    /// Current state of every visible account: cash vs invested, holdings,
    /// and investment accounts holding idle cash. `--history` prints the
    /// daily trend from per-sync snapshots instead.
    Balances {
        #[arg(long)]
        history: bool,
        /// With --history: the day range.
        #[command(flatten)]
        period: PeriodArgs,
    },
    /// Every investment position, one row each: quantity, institution price
    /// (+ as-of date), Plaid close price, value, cost basis, unit cost, gain.
    /// `--history` prints per-sync snapshots instead.
    Holdings(HoldingsArgs),
    /// Investment transactions (buys, sells, dividends, contributions, fees)
    /// archived from Plaid, newest first. Amount is the cash effect on the
    /// account: negative = cash out (a buy), positive = cash in.
    Activity(ActivityArgs),
    /// Card-payment match review (list / accept / reject).
    Matches {
        #[command(subcommand)]
        action: MatchCmd,
    },
    /// Card offers worth acting on: available offers at merchants you already
    /// pay, added offers charged to the wrong card, and added offers expiring
    /// unused. Reads an offers TOML (see examples/amex-offers-4242.toml).
    Perks {
        /// Offers file, or a directory of them (local mode only; with --server
        /// the server reads its own OUTFLOW_PERKS).
        #[arg(long, env = "OUTFLOW_PERKS")]
        file: Option<String>,
        /// How far back to look for spending at an offer's merchant.
        #[arg(long, default_value = "12mo", value_parser = ["3mo", "6mo", "12mo", "all"])]
        lookback: String,
        /// Warn about added offers expiring within this many days.
        #[arg(long, default_value_t = 14)]
        expiring_days: i64,
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
    /// Only the rows behind this Flow lane figure (`report --by monthly`).
    #[arg(long, value_enum)]
    pub lane: Option<LaneArg>,
    /// Only this normalized merchant (the `merchant` key from `recurring`).
    #[arg(long)]
    pub merchant: Option<String>,
    /// Only rows with no category.
    #[arg(long)]
    pub uncategorized: bool,
    /// Only money out or money in (on a card payment: bank or card side).
    #[arg(long, value_enum)]
    pub direction: Option<DirectionArg>,
    #[command(flatten)]
    pub period: PeriodArgs,
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
    /// Between own accounts (or moved, group unknown).
    Transfer,
    CardPayment,
    Invest,
    Save,
    LoanPaydown,
}

impl FlagArg {
    fn to_flag(self) -> TxnFlag {
        match self {
            FlagArg::Spending => TxnFlag::Spending,
            FlagArg::Transfer => TxnFlag::Transfer,
            FlagArg::CardPayment => TxnFlag::CardPayment,
            FlagArg::Invest => TxnFlag::Invest,
            FlagArg::Save => TxnFlag::Save,
            FlagArg::LoanPaydown => TxnFlag::LoanPaydown,
        }
    }

    /// Serde variant name on the wire (server mode).
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn wire(self) -> &'static str {
        match self {
            FlagArg::Spending => "Spending",
            FlagArg::Transfer => "Transfer",
            FlagArg::CardPayment => "CardPayment",
            FlagArg::Invest => "Invest",
            FlagArg::Save => "Save",
            FlagArg::LoanPaydown => "LoanPaydown",
        }
    }
}

#[derive(ValueEnum, Clone, Copy)]
pub(crate) enum LaneArg {
    In,
    Out,
    CardPayment,
    Invest,
    Save,
    LoanPaydown,
    /// Between own accounts.
    Between,
}

impl LaneArg {
    pub(crate) fn to_lane(self) -> Lane {
        match self {
            LaneArg::In => Lane::In,
            LaneArg::Out => Lane::Out,
            LaneArg::CardPayment => Lane::CardPayment,
            LaneArg::Invest => Lane::Invest,
            LaneArg::Save => Lane::Save,
            LaneArg::LoanPaydown => Lane::LoanPaydown,
            LaneArg::Between => Lane::Between,
        }
    }
}

#[derive(ValueEnum, Clone, Copy)]
pub(crate) enum DirectionArg {
    Out,
    In,
}

impl DirectionArg {
    pub(crate) fn to_direction(self) -> Direction {
        match self {
            DirectionArg::Out => Direction::Out,
            DirectionArg::In => Direction::In,
        }
    }

    /// Wire spelling (server mode).
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn wire(self) -> &'static str {
        match self {
            DirectionArg::Out => "out",
            DirectionArg::In => "in",
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

/// Epoch bounds `(since, until)` of a period at local midnight, until
/// exclusive; `(None, None)` for all of history.
fn epoch_bounds(period: Option<Period>) -> (Option<i64>, Option<i64>) {
    match period.map(|p| p.bounds()) {
        Some((s, u)) => (Some(s), Some(u)),
        None => (None, None),
    }
}

fn build_filter(period: &PeriodArgs, posted_only: bool) -> Result<TxnFilter, String> {
    let mut f = TxnFilter::all();
    (f.since, f.until) = epoch_bounds(period.resolve()?);
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

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Offline ingest of a Plaid fixture envelope (see `core::plaid::parse_fixture`)
/// — the zero-feature, zero-credential pipeline. Live syncs are the server's
/// job: `outflow --server URL pull`.
fn cmd_pull(store: &Store, from_file: Option<String>) -> Result<(), String> {
    let Some(path) = from_file else {
        return Err(
            "live sync runs on the server: use `--server URL pull`; for offline ingest use \
             `pull --from-file examples/plaid-fixture.json`"
                .into(),
        );
    };
    let json = std::fs::read_to_string(&path).map_err(|e| format!("read {path}: {e}"))?;
    let fixture = parse_fixture(&json, now_secs()).map_err(|e| format!("parse: {e:?}"))?;
    store
        .upsert_accounts(&fixture.accounts)
        .map_err(|e| format!("save accounts: {e}"))?;
    let mut txns = fixture.sync.added;
    txns.extend(fixture.sync.modified);
    let r = store
        .upsert_transactions(&txns)
        .map_err(|e| format!("save transactions: {e}"))?;
    let mut removed = fixture.sync.removed_ids;
    removed.extend(fixture.sync.pending_superseded);
    let deleted = store
        .delete_transactions(&removed)
        .map_err(|e| format!("apply removals: {e}"))?;
    // Same sync-point history the server records (fixtures carry no holdings).
    store
        .record_item_snapshot(&fixture.accounts, None, now_secs())
        .map_err(|e| format!("record snapshot: {e}"))?;
    for a in &fixture.accounts {
        println!(
            "  {} [{}]  {}",
            a.name,
            a.kind.as_str(),
            dollars(a.balance.cents())
        );
    }
    println!(
        "added {}, updated {}, removed {}",
        r.added, r.updated, deleted
    );
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
                println!(
                    "{:>12}  {:<28} {}",
                    dollars(r.total_cents),
                    r.merchant,
                    r.count
                );
            }
        }
        By::Monthly => {
            // Flow lanes: In / Out (refunds netted) / earned − spent / saved,
            // then the Moved groups (out of checking+cards, minus money back).
            println!(
                "{:<7}  {:>11}  {:>11}  {:>12}  {:>11}   moved: card · invest · save · loan · between",
                "month", "in", "out", "earned-spent", "saved"
            );
            for r in flow_lanes(store, filter).map_err(|e| format!("{e}"))? {
                let net = |g: outflow_core::MovedFlow| dollars(g.out_cents - g.in_cents);
                println!(
                    "{}-{:02}  {:>11}  {:>11}  {:>12}  {:>11}   {} · {} · {} · {} · {}",
                    r.year,
                    r.month,
                    dollars(r.in_cents),
                    dollars(r.out_cents),
                    dollars(r.earned_minus_spent_cents),
                    dollars(r.saved_cents),
                    dollars(r.moved.card_payment_cents),
                    net(r.moved.invest),
                    net(r.moved.save),
                    net(r.moved.loan_paydown),
                    net(r.moved.between),
                );
            }
        }
    }
    Ok(())
}

fn cmd_recurring(
    store: &Store,
    period: &PeriodArgs,
    ignored: bool,
    json: bool,
) -> Result<(), String> {
    let through = period.resolve()?.map_or_else(today_local, |p| p.last_day());
    let now = now_secs();
    let v = recurring(store, through, now).map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&v);
    }
    let rows = if ignored { &v.ignored } else { &v.items };
    println!(
        "recurring {} to {}{}",
        v.since,
        v.until,
        if v.in_progress {
            " (month in progress)"
        } else {
            ""
        }
    );
    if rows.is_empty() {
        println!(
            "{}",
            if ignored {
                "nothing ignored"
            } else {
                "no recurring merchants detected"
            }
        );
        return Ok(());
    }
    for r in rows {
        let change = match (r.change, r.change_pct) {
            (Change::New, _) => "new".to_string(),
            (Change::Up | Change::Down, Some(p)) => format!("{p:+.0}%"),
            _ => String::new(),
        };
        println!(
            "{:<28} {:<10} {:>10}/mo  {:<6} last {}  {}",
            truncate(&r.name, 28),
            cadence_label(r.cadence),
            dollars(r.monthly_cents),
            change,
            date_label(r.last_seen),
            r.merchant
        );
    }
    if !ignored {
        println!(
            "total {}/mo · {} ignored",
            dollars(v.monthly_total_cents),
            v.ignored.len()
        );
    }
    Ok(())
}

fn cadence_label(c: StreamCadence) -> &'static str {
    match c {
        StreamCadence::Daily => "~daily",
        StreamCadence::FewPerWeek => "~3x/wk",
        StreamCadence::Weekly => "weekly",
        StreamCadence::FewPerMonth => "~4x/mo",
        StreamCadence::Monthly => "monthly",
        StreamCadence::Yearly => "yearly",
    }
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
    let mut filter = build_filter(&a.period, a.posted_only)?;
    filter.include_non_spending = a.show_excluded || a.flag.is_some_and(|f| f.to_flag().is_moved());
    let q = TxnQuery {
        filter,
        text: a.search.clone(),
        account_id: a.account.clone(),
        category: a.category.clone(),
        source: a.source.clone(),
        flag: a.flag.map(FlagArg::to_flag),
        lane: a.lane.map(LaneArg::to_lane),
        merchant: a.merchant.clone(),
        uncategorized: a.uncategorized,
        direction: a.direction.map(DirectionArg::to_direction),
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

/// Same windows as the server's `window_since` (~30.5-day months).
fn lookback_since(window: &str, now: i64) -> Option<i64> {
    const MONTH_SECS: i64 = 2_635_200;
    match window {
        "all" => None,
        "3mo" => Some(now - 3 * MONTH_SECS),
        "6mo" => Some(now - 6 * MONTH_SECS),
        _ => Some(now - 12 * MONTH_SECS),
    }
}

fn reward_label(o: &Offer) -> String {
    let mut s = match o.reward {
        Reward::Back(m) => format!("{} back", dollars(m.cents())),
        Reward::Pct(p) => format!("{p}% back"),
        Reward::Pts(n) => format!("{n} pts"),
        Reward::PtsPerDollar(n) => format!("+{n} pts/$"),
    };
    if let Some(m) = o.min {
        s.push_str(&format!(" on {}+", dollars(m.cents())));
    }
    if o.single {
        s.push_str(", single purchase");
    }
    if let Some(u) = o.uses {
        s.push_str(&format!(", {u} use{}", if u == 1 { "" } else { "s" }));
    }
    match o.cap {
        Cap::Cash(m) => s.push_str(&format!(", max {}", dollars(m.cents()))),
        Cap::Points(p) => s.push_str(&format!(", max {p} pts")),
        Cap::None => {}
    }
    if o.per_year {
        s.push_str("/yr");
    }
    s
}

fn cmd_perks(
    store: &Store,
    file: Option<String>,
    lookback: &str,
    expiring_days: i64,
    json: bool,
) -> Result<(), String> {
    let path = file.ok_or("no offers file: pass --file or set OUTFLOW_PERKS")?;
    let cards = load_offers(std::path::Path::new(&path)).map_err(|e| e.to_string())?;
    let accounts = store.accounts().map_err(|e| format!("{e}"))?;
    let txns = store.all_transactions().map_err(|e| format!("{e}"))?;
    let now = now_secs();
    let opts = PerkOptions {
        since: lookback_since(lookback, now),
        expiring_days: expiring_days.max(0),
    };
    let report = evaluate_perks(&cards, &accounts, &txns, now, &opts);
    if json {
        return json_out(&report);
    }

    for c in &report.cards {
        let acct = c.account_id.as_deref().unwrap_or("?");
        println!(
            "card {} ({}): {} offers, {} added, {} expired, account {acct}",
            c.mask, c.issuer, c.offers, c.added, c.expired
        );
        if let Some(p) = &c.problem {
            println!("  ! {p}; only add-offer suggestions are possible");
        }
    }
    let sections = [
        (FindingKind::WrongCard, "CHARGED TO THE WRONG CARD"),
        (FindingKind::Expiring, "ADDED, EXPIRING UNUSED"),
        (FindingKind::AddOffer, "ADD THESE OFFERS"),
    ];
    if report.findings.is_empty() {
        println!("\nnothing to act on");
    }
    for (kind, title) in sections {
        let fs: Vec<_> = report.findings.iter().filter(|f| f.kind == kind).collect();
        if fs.is_empty() {
            continue;
        }
        println!("\n{title} ({})", fs.len());
        for f in fs {
            let value = match (f.est_cash_cents, f.est_points) {
                (0, 0) => "-".to_string(),
                (0, p) => format!("{p} pts"),
                (c, 0) => dollars(c),
                (c, p) => format!("{} + {p} pts", dollars(c)),
            };
            // Expiry is end-of-day local; step back to local noon so the UTC
            // day label lands on the same calendar date.
            let expires = date_label(f.offer.expires - 43_199);
            let history = match (f.last_charge, f.typical_charge_cents) {
                (Some(last), Some(typ)) => format!(
                    "{} charge{}, typical {}, last {}",
                    f.charges,
                    if f.charges == 1 { "" } else { "s" },
                    dollars(typ),
                    date_label(last)
                ),
                _ => "no charges in lookback".to_string(),
            };
            println!(
                "  {:<36} {:>10}  expires {expires} ({}d)  {history}",
                f.offer.merchant, value, f.days_left
            );
            let mut detail = reward_label(&f.offer);
            if !f.matched_merchants.is_empty() {
                detail.push_str(&format!(" · matched: {}", f.matched_merchants.join(", ")));
            }
            if let Some(n) = &f.offer.note {
                detail.push_str(&format!(" · {n}"));
            }
            println!("      {detail}");
        }
    }
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

fn cmd_accounts(store: &Store, all: bool, json: bool) -> Result<(), String> {
    let accounts = if all {
        store.all_accounts()
    } else {
        store.accounts()
    }
    .map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&accounts);
    }
    for a in &accounts {
        println!(
            "{:<40} {:<9} {:>12}  [{}] {}{}",
            a.name,
            a.kind.as_str(),
            dollars(a.balance.cents()),
            a.source,
            a.id,
            if a.hidden { "  hidden" } else { "" }
        );
    }
    Ok(())
}

fn pct(bp: i64) -> String {
    let sign = if bp < 0 { "-" } else { "" };
    let bp = bp.abs();
    format!("{sign}{}.{}%", bp / 100, (bp % 100) / 10)
}

fn cmd_balances(
    store: &Store,
    history: bool,
    period: &PeriodArgs,
    json: bool,
) -> Result<(), String> {
    let range = history_period(history, period, today_local())?;
    let accounts = store.accounts().map_err(|e| format!("{e}"))?;
    if history {
        let kinds = accounts.iter().map(|a| (a.id.clone(), a.kind)).collect();
        let (since, until) = epoch_bounds(range);
        let snaps = store
            .balance_snapshots(since, until)
            .map_err(|e| format!("{e}"))?;
        let days = balance_history(&snaps, &kinds);
        if json {
            return json_out(&days);
        }
        println!(
            "{:<10} {:>15} {:>14} {:>15} {:>13}",
            "day", "net worth", "cash", "invested", "credit"
        );
        for d in &days {
            println!(
                "{:<10} {:>15} {:>14} {:>15} {:>13}",
                d.day,
                dollars(d.net_worth.cents()),
                dollars(d.cash.cents()),
                dollars(d.invested.cents()),
                dollars(d.credit.cents())
            );
        }
        return Ok(());
    }
    let securities = store.securities().map_err(|e| format!("{e}"))?;
    let holdings = store.current_holdings().map_err(|e| format!("{e}"))?;
    let view = summarize_balances(&accounts, &securities, &holdings);
    if json {
        return json_out(&view);
    }
    let t = &view.totals;
    println!(
        "net worth {}   cash {}   invested {}   credit {}",
        dollars(t.net_worth.cents()),
        dollars(t.cash.cents()),
        dollars(t.invested.cents()),
        dollars(t.credit.cents())
    );
    for g in &view.groups {
        println!("\n{:?}  {}", g.group, dollars(g.total.cents()));
        for a in &g.accounts {
            let label = account_display_label(&a.account.name, a.account.nickname.as_deref());
            let cash = if a.group == outflow_core::Group::Investments {
                format!("  cash {} ({})", dollars(a.cash.cents()), pct(a.cash_bp))
            } else {
                String::new()
            };
            println!(
                "  {:<44} {:>14}{}{}",
                truncate(&label, 44),
                dollars(a.account.balance.cents()),
                cash,
                if a.idle_cash { "  ← idle cash" } else { "" }
            );
            for h in &a.holdings {
                println!(
                    "      {:<12} {:>12} sh  {:>13}  {:>6}{}{}",
                    truncate(&h.label, 12),
                    h.quantity,
                    dollars(h.value.cents()),
                    pct(h.share_bp),
                    h.gain
                        .map(|g| format!("  gain {}", dollars(g.cents())))
                        .unwrap_or_default(),
                    if h.is_cash { "  (cash)" } else { "" }
                );
            }
        }
    }
    Ok(())
}

fn cmd_holdings(store: &Store, a: &HoldingsArgs, json: bool) -> Result<(), String> {
    let filter = PositionFilter {
        account: a.account.clone(),
        ticker: a.ticker.clone(),
    };
    let range = history_period(a.history, &a.period, today_local())?;
    let accounts = store.accounts().map_err(|e| format!("{e}"))?;
    let securities = store.securities().map_err(|e| format!("{e}"))?;
    if a.history {
        let (since, until) = epoch_bounds(range);
        let snaps = store
            .holding_snapshots(since, until)
            .map_err(|e| format!("{e}"))?;
        let points = position_history(&snaps, &accounts, &securities, &filter);
        if json {
            return json_out(&points);
        }
        println!(
            "{:<10} {:<12} {:<28} {:>12} {:>11} {:>14} {:>14}",
            "day", "holding", "account", "qty", "price", "value", "gain"
        );
        for p in &points {
            println!(
                "{:<10} {:<12} {:<28} {:>12} {:>11} {:>14} {:>14}",
                p.day,
                truncate(&p.label, 12),
                truncate(&p.account, 28),
                p.quantity,
                p.price.as_deref().unwrap_or("-"),
                dollars(p.value.cents()),
                p.gain.map(|g| dollars(g.cents())).unwrap_or_default()
            );
        }
        return Ok(());
    }
    let holdings = store
        .current_holdings_synced()
        .map_err(|e| format!("{e}"))?;
    let rows = positions(&accounts, &securities, &holdings, &filter);
    if json {
        return json_out(&rows);
    }
    println!(
        "{:<12} {:<28} {:>12} {:>11} {:<10} {:>14} {:>14} {:>11} {:>8}",
        "holding", "account", "qty", "price", "as of", "value", "basis", "unit cost", "gain"
    );
    for r in &rows {
        println!(
            "{:<12} {:<28} {:>12} {:>11} {:<10} {:>14} {:>14} {:>11} {:>8}",
            truncate(&r.label, 12),
            truncate(&r.account, 28),
            r.quantity,
            r.price,
            r.price_as_of.as_deref().unwrap_or("-"),
            dollars(r.value.cents()),
            r.cost_basis.map(|b| dollars(b.cents())).unwrap_or_default(),
            r.unit_cost.as_deref().unwrap_or(""),
            r.gain_bp.map(pct).unwrap_or_default()
        );
    }
    Ok(())
}

fn cmd_activity(store: &Store, a: &ActivityArgs, json: bool) -> Result<(), String> {
    let filter = a.filter(a.period.resolve()?);
    let accounts = store.accounts().map_err(|e| format!("{e}"))?;
    let securities = store.securities().map_err(|e| format!("{e}"))?;
    let txns = store.investment_txns().map_err(|e| format!("{e}"))?;
    let rows = activity(&txns, &accounts, &securities, &filter);
    if json {
        return json_out(&rows);
    }
    println!(
        "{:<10} {:<12} {:<28} {:<18} {:>12} {:>11} {:>14}",
        "date", "holding", "account", "type", "qty", "price", "amount"
    );
    for r in &rows {
        println!(
            "{:<10} {:<12} {:<28} {:<18} {:>12} {:>11} {:>14}",
            r.date,
            truncate(&r.label, 12),
            truncate(&r.account, 28),
            truncate(&format!("{}/{}", r.txn_type, r.subtype), 18),
            r.quantity,
            r.price,
            dollars(r.amount.cents())
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
        println!(
            "#{:<4} {:<24} {}  {}",
            s.id,
            s.source,
            date_label(s.started),
            outcome
        );
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
                    .clear_flag(&m.bank_txn_id)
                    .map_err(|e| format!("{e}"))?;
                store
                    .clear_flag(&m.card_txn_id)
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
    let store = open_store(&cli.db)?;
    match cli.cmd {
        Cmd::Pull { from_file } => cmd_pull(&store, from_file),
        Cmd::Categorize { llm } => cmd_categorize(&store, llm),
        Cmd::Report {
            by,
            top,
            period,
            posted_only,
        } => {
            let f = build_filter(&period, posted_only)?;
            if cli.json {
                return report_json(&store, by, top, &f);
            }
            cmd_report(&store, by, top, &f)
        }
        Cmd::Recurring { period, ignored } => cmd_recurring(&store, &period, ignored, cli.json),
        Cmd::Fix {
            txn_id,
            category,
            no_learn,
        } => cmd_fix(&store, txn_id, category, no_learn),
        Cmd::Txns(args) => cmd_txns(&store, &args, cli.json),
        Cmd::Accounts { all } => cmd_accounts(&store, all, cli.json),
        Cmd::Balances { history, period } => cmd_balances(&store, history, &period, cli.json),
        Cmd::Holdings(args) => cmd_holdings(&store, &args, cli.json),
        Cmd::Activity(args) => cmd_activity(&store, &args, cli.json),
        Cmd::Matches { action } => cmd_matches(&store, &action, cli.json),
        Cmd::Status { limit } => cmd_status(&store, limit, cli.json),
        Cmd::Perks {
            file,
            lookback,
            expiring_days,
        } => cmd_perks(&store, file, &lookback, expiring_days, cli.json),
    }
}

fn report_json(store: &Store, by: By, top: usize, filter: &TxnFilter) -> Result<(), String> {
    match by {
        By::Category => json_out(&spend_by_category(store, filter).map_err(|e| format!("{e}"))?),
        By::Merchant => json_out(&top_merchants(store, filter, top).map_err(|e| format!("{e}"))?),
        By::Monthly => json_out(&flow_lanes(store, filter).map_err(|e| format!("{e}"))?),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Invented "today" so relative periods are stable.
    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 7).unwrap()
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn args(period: Option<&str>, since: Option<&str>, until: Option<&str>) -> PeriodArgs {
        PeriodArgs {
            period: period.map(Into::into),
            since: since.map(Into::into),
            until: until.map(Into::into),
        }
    }

    #[test]
    fn none_means_all_of_history() {
        assert_eq!(PeriodArgs::default().resolve_at(today()), Ok(None));
    }

    #[test]
    fn period_spellings_resolve() {
        let p = args(Some("2026-02"), None, None)
            .resolve_at(today())
            .unwrap()
            .unwrap();
        assert_eq!((p.start, p.last_day()), (day(2026, 2, 1), day(2026, 2, 28)));
        let p = args(Some("last-quarter"), None, None)
            .resolve_at(today())
            .unwrap()
            .unwrap();
        assert_eq!((p.start, p.end), (day(2026, 7, 1), day(2026, 10, 1)));
    }

    #[test]
    fn since_until_are_both_inclusive() {
        let p = args(None, Some("2026-03-10"), Some("2026-03-12"))
            .resolve_at(today())
            .unwrap()
            .unwrap();
        assert_eq!(p.start, day(2026, 3, 10));
        assert_eq!(p.last_day(), day(2026, 3, 12));
        assert_eq!(p.end, day(2026, 3, 13));
        // The activity filter gets the first and last day as text.
        let a = ActivityArgs {
            account: None,
            ticker: None,
            kind: None,
            period: PeriodArgs::default(),
        };
        let f = a.filter(Some(p));
        assert_eq!(f.since.as_deref(), Some("2026-03-10"));
        assert_eq!(f.until.as_deref(), Some("2026-03-12"));
    }

    #[test]
    fn period_with_since_is_rejected() {
        let e = args(Some("2026-11"), Some("2026-11-01"), None)
            .resolve_at(today())
            .unwrap_err();
        assert!(e.contains("not both"), "{e}");
    }

    #[test]
    fn bad_spelling_lists_the_accepted_forms() {
        let e = args(Some("november"), None, None)
            .resolve_at(today())
            .unwrap_err();
        assert!(e.contains("2026-W45") && e.contains("this-/last-"), "{e}");
        let e = args(None, Some("2026/11/01"), None)
            .resolve_at(today())
            .unwrap_err();
        assert!(e.contains("YYYY-MM-DD"), "{e}");
    }

    #[test]
    fn range_needs_history_on_snapshot_commands() {
        let p = args(Some("this-year"), None, None);
        assert!(history_period(false, &p, today()).is_err());
        assert!(history_period(true, &p, today()).unwrap().is_some());
        assert_eq!(
            history_period(false, &PeriodArgs::default(), today()),
            Ok(None)
        );
    }

    #[test]
    fn flattened_flags_parse_on_every_dated_subcommand() {
        for argv in [
            &["outflow", "report", "--period", "2026-11"][..],
            &["outflow", "txns", "--since", "2026-11-01"],
            &["outflow", "activity", "--until", "2026-11-01"],
            &["outflow", "holdings", "--history", "--period", "2026"],
            &["outflow", "balances", "--history", "--period", "this-year"],
        ] {
            assert!(Cli::try_parse_from(argv).is_ok(), "{argv:?}");
        }
    }
}
