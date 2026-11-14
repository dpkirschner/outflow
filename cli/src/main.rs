use chrono::NaiveDate;
use clap::{Args, Parser, Subcommand, ValueEnum};
use outflow_core::{
    account_display_label, activity, balance_history, find, flow_lanes, parse_fixture,
    position_change, position_history, positions, search_transactions, spend_by_category,
    summarize_balances, today_local, top_merchants, ActivityFilter, Cap, CategorySource, Direction,
    Evidence, FindOptions, Lane, MatchStatus, Money, Offer, Period, PositionFilter, PositionLens,
    Reward, SortKey, Store, TxnFilter, TxnFlag, TxnQuery,
};
use outflow_core::{recurring, Change, StreamCadence};
use std::process::ExitCode;

#[cfg(feature = "client")]
mod remote;

#[derive(Parser)]
#[command(
    name = "outflow",
    version = outflow_core::build::VERSION.unwrap_or("unreleased build"),
    about = "Where your money is and where it's going: Stock, Flow, Check-in, from a local archive or a running outflow-server"
)]
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
    /// `position` combines a security across accounts (keyed on ticker).
    /// Implies the period view.
    #[arg(long, value_enum)]
    by: Option<ByArg>,
    /// Without --history: the period to show position gain over (default
    /// this month when --by is given). With --history: the snapshot range.
    #[command(flatten)]
    period: PeriodArgs,
}

#[derive(Clone, Copy, ValueEnum, PartialEq, Eq, Debug)]
pub(crate) enum ByArg {
    Account,
    Position,
}

impl ByArg {
    pub(crate) fn lens(self) -> PositionLens {
        match self {
            ByArg::Account => PositionLens::Account,
            ByArg::Position => PositionLens::Position,
        }
    }

    /// Wire spelling (server mode).
    #[cfg_attr(not(feature = "client"), allow(dead_code))]
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ByArg::Account => "account",
            ByArg::Position => "position",
        }
    }
}

/// What `holdings` shows.
#[derive(Debug, PartialEq)]
pub(crate) enum HoldingsView {
    /// Current positions (no period, no --by).
    Current,
    /// Per-sync snapshots (--history), over an optional range.
    History(Option<Period>),
    /// Position gain over a period (a period or --by given).
    Change(Period, PositionLens),
}

pub(crate) fn holdings_view(a: &HoldingsArgs, today: NaiveDate) -> Result<HoldingsView, String> {
    let period = a.period.resolve_at(today)?;
    if a.history {
        if a.by.is_some() {
            return Err("--by applies to the period view, not --history".into());
        }
        return Ok(HoldingsView::History(period));
    }
    if period.is_none() && a.by.is_none() {
        return Ok(HoldingsView::Current);
    }
    let period = match period {
        Some(p) => p,
        None => Period::parse("this-month", today).map_err(|e| e.to_string())?,
    };
    Ok(HoldingsView::Change(
        period,
        a.by.map_or(PositionLens::Account, ByArg::lens),
    ))
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
    /// Write a consistent, verified snapshot of the DB to a new file, still
    /// encrypted with the same key (safe while the server runs). Refuses to
    /// overwrite; the file is 0600. Local only.
    Backup {
        /// Where to write the snapshot (must not exist).
        #[arg(long)]
        to: String,
    },
    /// Ingest a Plaid fixture file offline (`--from-file`); live syncs run on
    /// the server (use `--server URL pull`).
    Pull {
        /// A Plaid fixture envelope (see examples/plaid-fixture.json).
        #[arg(long)]
        from_file: Option<String>,
    },
    /// Run the rule categorizer over uncategorized transactions.
    Categorize {
        /// After the rule pass, send the remaining uncategorized merchants to
        /// an LLM (needs --features net and ANTHROPIC_API_KEY).
        #[arg(long)]
        llm: bool,
    },
    /// Spending by category or merchant, In by source (`--by source`: payee),
    /// or the Flow lanes per month (`--by monthly`: in / out / earned -
    /// spent / saved / Moved groups).
    /// No period = all of history.
    Report {
        #[arg(long, value_enum, default_value_t = By::Category)]
        by: By,
        /// Rows to show for --by category|merchant.
        #[arg(long, default_value_t = 15)]
        top: usize,
        #[command(flatten)]
        period: PeriodArgs,
        /// Leave out pending transactions.
        #[arg(long)]
        posted_only: bool,
    },
    /// Recurring merchants over the 12 months ending with the period's last
    /// month (default: this month): cadence, usual monthly cost, change
    /// (new / late / up / down >25% vs usual; charge to charge for
    /// every-few-months merchants), 12-month year on year. --json prints the
    /// whole view, ignored merchants included.
    Recurring {
        #[command(flatten)]
        period: PeriodArgs,
        /// List the ignored merchants instead.
        #[arg(long)]
        ignored: bool,
        /// Only steady merchants (monthly or yearly, last 3 charges within
        /// 5% of their median); the total is theirs. Ignored stays whole.
        #[arg(long)]
        subscriptions: bool,
    },
    /// Set a transaction's category by hand. Also points the merchant's rule
    /// at it, so its uncategorized rows follow on the next `categorize`.
    Fix {
        /// Transaction id (the `id` from `txns --json`).
        txn_id: String,
        category: String,
        /// Fix this row only; don't learn a rule.
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
        /// The daily trend instead of current state.
        #[arg(long)]
        history: bool,
        /// With --history: the day range.
        #[command(flatten)]
        period: PeriodArgs,
    },
    /// Check-in for a week, month or quarter (default this month): net worth
    /// change, earned - spent and saved, each against its usual (median of
    /// the 12 periods before), and what's unusual: new or changed recurring
    /// merchants, large one-off charges, connections and match reviews.
    Checkin {
        /// 2026-11, 2026-W45, 2026-Q4, or this-/last- week|month|quarter.
        #[arg(long, value_name = "P")]
        period: Option<String>,
        /// Out per category against each one's usual instead, marking those
        /// at 2x or half their usual and $50 or more away.
        #[arg(long)]
        categories: bool,
        /// Offers file or folder, for the card offer Findings (local mode
        /// only; with --server the server reads its own OUTFLOW_PERKS). A
        /// bad file skips those Findings instead of failing.
        #[arg(long, env = "OUTFLOW_PERKS")]
        offers: Option<String>,
        /// Offers expiring within this many days count as new.
        #[arg(long, default_value_t = 14)]
        expiring_days: i64,
    },
    /// Net worth change over a period (default this month), split into
    /// Markets (gain net of contributions on measurable investment accounts)
    /// + Added (everything else), with per-account change and gain.
    Change {
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
    /// Card-payment match review (list / accept / reject / unaccept).
    Matches {
        #[command(subcommand)]
        action: MatchCmd,
    },
    /// Find money: every Finding (card offers, card choice, subscriptions to
    /// review and price rises), ranked by estimated dollars per year over
    /// the 12 months ending with the period's last month (default: this
    /// month). Card offers come from the offers TOML
    /// file or folder (see examples/amex-offers-4242.toml); a bad file is an
    /// error, so this also checks the files.
    Find {
        /// Offers file, or a folder of them (local mode only; with --server
        /// the server reads its own OUTFLOW_PERKS).
        #[arg(long, env = "OUTFLOW_PERKS")]
        file: Option<String>,
        #[command(flatten)]
        period: PeriodArgs,
        /// Flag added offers expiring within this many days.
        #[arg(long, default_value_t = 14)]
        expiring_days: i64,
        /// Dismiss the Finding with this key (shown under each one). It
        /// comes back only if its estimate more than doubles.
        #[arg(long, value_name = "KEY", group = "find_mark")]
        dismiss: Option<String>,
        /// Restore a dismissed Finding by its key.
        #[arg(long, value_name = "KEY", group = "find_mark")]
        restore: Option<String>,
        /// Mark a subscription's merchant Reviewed (shown under each one):
        /// hidden for a year, or until its price rises.
        #[arg(long, value_name = "MERCHANT", group = "find_mark")]
        review: Option<String>,
        /// Clear a merchant's Reviewed mark.
        #[arg(long, value_name = "MERCHANT", group = "find_mark")]
        unreview: Option<String>,
    },
    /// What needs you (connections, card payments to review, last sync),
    /// then recent sync runs. --json: {"version": ..., "upkeep": ...,
    /// "sync_log": [...]}; version is this CLI's build locally, the server's
    /// with --server.
    Status {
        /// Sync runs to list.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
}

#[derive(Args)]
pub(crate) struct TxnsArgs {
    /// Case-insensitive text over payee/description/category/merchant.
    #[arg(long)]
    pub search: Option<String>,
    /// Category name (exact).
    #[arg(long)]
    pub category: Option<String>,
    /// Account id (see `accounts`).
    #[arg(long)]
    pub account: Option<String>,
    /// Provenance: plaid (simplefin on rows from the retired integration).
    #[arg(long)]
    pub source: Option<String>,
    /// Keep only this flag (a Moved flag implies --show-excluded).
    #[arg(long, value_enum)]
    pub flag: Option<FlagArg>,
    /// Only the rows behind this Flow lane figure (`report --by monthly`).
    #[arg(long, value_enum)]
    pub lane: Option<LaneArg>,
    /// Only this normalized merchant (the `merchant` key from `recurring`).
    #[arg(long)]
    pub merchant: Option<String>,
    /// Only these transaction ids, comma-separated (a Finding's `txn_ids`).
    #[arg(long, value_delimiter = ',')]
    pub ids: Vec<String>,
    /// Only rows with no category.
    #[arg(long)]
    pub uncategorized: bool,
    /// Only money out or money in (on a card payment: bank or card side).
    #[arg(long, value_enum)]
    pub direction: Option<DirectionArg>,
    #[command(flatten)]
    pub period: PeriodArgs,
    /// Leave out pending transactions.
    #[arg(long)]
    pub posted_only: bool,
    /// Include Moved rows (transfer, card-payment, invest, save,
    /// loan-paydown), hidden by default.
    #[arg(long)]
    pub show_excluded: bool,
    #[arg(long, value_enum, default_value_t = SortArg::Date)]
    pub sort: SortArg,
    /// Ascending sort (default is descending).
    #[arg(long)]
    pub asc: bool,
    /// Page size (the server caps it at 1000).
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
    /// Rows to skip, for paging.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
    /// The matching rows' sum per month instead of the rows (Flow's month
    /// strip with a pick); sort and paging are ignored.
    #[arg(long)]
    pub by_month: bool,
}

#[derive(Subcommand)]
pub(crate) enum MatchCmd {
    /// Card-payment pairs, with their ids.
    List {
        /// proposed | accepted | rejected (default: all)
        #[arg(long)]
        status: Option<String>,
    },
    /// Accept a proposed pair: both legs become card payments.
    Accept { id: i64 },
    /// Reject a pair for good (an accepted one is undone first).
    Reject { id: i64 },
    /// Put an accepted pair back in review; both legs count as before.
    Unaccept { id: i64 },
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
    /// The In lane by payee.
    Source,
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

/// The SQLCipher key: `OUTFLOW_DB_KEY`, else `OUTFLOW_DB_KEY_FILE` (0600).
/// Neither set = plaintext. A secret: env or a 0600 file, never argv.
fn db_key() -> Result<Option<String>, String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if let Some(k) = env("OUTFLOW_DB_KEY") {
        return Ok(Some(k));
    }
    match env("OUTFLOW_DB_KEY_FILE") {
        Some(p) => outflow_core::secrets::read_secret_file(&p).map(Some),
        None => Ok(None),
    }
}

fn open_store(db: &str) -> Result<Store, String> {
    #[cfg(feature = "encryption")]
    if let Some(key) = db_key()? {
        return Store::open_encrypted(db, &key).map_err(|e| format!("open encrypted db {db}: {e}"));
    }
    Store::open(db).map_err(|e| format!("open db {db}: {e}"))
}

/// `backup` opens the DB itself, without migrating (see `core::backup`).
fn cmd_backup(db: &str, to: &str, json: bool) -> Result<(), String> {
    let key = db_key()?;
    let r = outflow_core::backup::backup_db(db, to, key.as_deref())?;
    if json {
        return json_out(&r);
    }
    println!(
        "backed up {db} → {}: {:.1} MB, {}, schema v{}, {} tables, {} rows, integrity ok",
        r.path,
        r.bytes as f64 / 1_048_576.0,
        if r.encrypted {
            "encrypted"
        } else {
            "plaintext"
        },
        r.schema_version,
        r.tables.len(),
        r.total_rows()
    );
    Ok(())
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
        By::Source => {
            for r in outflow_core::in_sources(store, filter).map_err(|e| format!("{e}"))? {
                let name = if r.name.is_empty() {
                    "(no payee)"
                } else {
                    &r.name
                };
                println!("{:>12}  {:<28} {}", dollars(r.total_cents), name, r.count);
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
    subscriptions: bool,
    json: bool,
) -> Result<(), String> {
    let through = period.resolve()?.map_or_else(today_local, |p| p.last_day());
    let now = now_secs();
    let mut v = recurring(store, through, now).map_err(|e| format!("{e}"))?;
    if subscriptions {
        v = v.subscriptions();
    }
    if json {
        return json_out(&v);
    }
    let rows = if ignored { &v.ignored } else { &v.items };
    println!(
        "{} {} to {}{}",
        if subscriptions {
            "subscriptions"
        } else {
            "recurring"
        },
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
            } else if subscriptions {
                "no steady subscriptions detected"
            } else {
                "no recurring merchants detected"
            }
        );
        return Ok(());
    }
    for r in rows {
        let change = match (r.change, r.change_pct) {
            (Change::New, _) => "new".to_string(),
            (Change::Late, _) => "late".to_string(),
            (Change::Up | Change::Down, Some(p)) => format!("{p:+.0}%"),
            _ => String::new(),
        };
        // Year on year, when the archive reaches back two years.
        let yoy = match r.prior_year_cents {
            Some(p) if p > 0 => format!(
                "  12mo {} ({:+.0}%)",
                dollars(r.year_cents),
                (r.year_cents - p) as f64 / p as f64 * 100.0
            ),
            _ => String::new(),
        };
        println!(
            "{:<28} {:<14} {:>10}/mo  {:<6} last {}  {}{}",
            truncate(&r.name, 28),
            cadence_label(r.cadence, r.gap_days),
            dollars(r.monthly_cents),
            change,
            date_label(r.last_seen),
            r.merchant,
            yoy
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

fn cadence_label(c: StreamCadence, gap_days: i64) -> String {
    match c {
        StreamCadence::Daily => "~daily".into(),
        StreamCadence::FewPerWeek => "~3x/wk".into(),
        StreamCadence::Weekly => "weekly".into(),
        StreamCadence::FewPerMonth => "~4x/mo".into(),
        StreamCadence::Monthly => "monthly".into(),
        StreamCadence::EveryFewMonths => format!("every ~{}mo", months_of_gap(gap_days)),
        StreamCadence::Yearly => "yearly".into(),
    }
}

/// "every ~2 months": the median gap in whole months, at least 2.
fn months_of_gap(gap_days: i64) -> i64 {
    ((gap_days as f64 / 30.44).round() as i64).max(2)
}

fn cmd_fix(store: &Store, txn_id: String, category: String, no_learn: bool) -> Result<(), String> {
    let edit = store
        .set_manual_category(&txn_id, &category, !no_learn)
        .map_err(|e| format!("{e}"))?;
    match edit.rule_id {
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
        ids: (!a.ids.is_empty()).then(|| a.ids.iter().cloned().collect()),
        uncategorized: a.uncategorized,
        direction: a.direction.map(DirectionArg::to_direction),
        min_cents: None,
        max_cents: None,
        sort: a.sort.to_key(),
        descending: !a.asc,
        offset: a.offset,
        limit: a.limit,
    };
    if a.by_month {
        let series = outflow_core::pick_series(store, &q).map_err(|e| format!("{e}"))?;
        if json {
            return json_out(&series);
        }
        for m in &series {
            println!("{}-{:02}  {:>12}", m.year, m.month, dollars(m.cents));
        }
        return Ok(());
    }
    let page = search_transactions(store, &q).map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&page);
    }
    for row in &page.items {
        let t = &row.txn;
        println!(
            "{:>12}  {:<10}  {:<32} {:<14} {}",
            dollars(t.amount.cents()),
            date_label(t.effective_date()),
            truncate(&row.display_name, 32),
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

/// A mark `outflow find` sets or clears before listing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindMark<'a> {
    /// Dismiss (true) or restore a Finding by key.
    Dismissed(&'a str, bool),
    /// Mark (true) or clear a merchant's Reviewed mark.
    Reviewed(&'a str, bool),
}

impl<'a> FindMark<'a> {
    fn from_args(
        dismiss: &'a Option<String>,
        restore: &'a Option<String>,
        review: &'a Option<String>,
        unreview: &'a Option<String>,
    ) -> Option<Self> {
        match (dismiss, restore, review, unreview) {
            (Some(k), ..) => Some(FindMark::Dismissed(k, true)),
            (_, Some(k), ..) => Some(FindMark::Dismissed(k, false)),
            (_, _, Some(m), _) => Some(FindMark::Reviewed(m, true)),
            (.., Some(m)) => Some(FindMark::Reviewed(m, false)),
            _ => None,
        }
    }
}

fn cmd_find(
    store: &Store,
    file: Option<String>,
    period: &PeriodArgs,
    expiring_days: i64,
    mark: Option<FindMark>,
    json: bool,
) -> Result<(), String> {
    let through = period.resolve()?.map_or(today_local(), |p| p.last_day());
    let opts = FindOptions {
        expiring_days: expiring_days.max(0),
    };
    let path = file.map(std::path::PathBuf::from);
    let (p, now) = (path.as_deref(), now_secs());
    let report = match mark {
        Some(FindMark::Dismissed(key, on)) => {
            outflow_core::set_dismissed(store, p, through, now, &opts, key, on)
        }
        Some(FindMark::Reviewed(m, on)) => {
            outflow_core::set_reviewed(store, p, through, now, &opts, m, on)
        }
        None => find(store, p, through, now, &opts),
    }
    .map_err(|e| e.to_string())?;
    if json {
        return json_out(&report);
    }

    let w = &report.window;
    println!("estimates over {} .. {}", w.start, w.last_day());
    for n in &report.notes {
        println!("note: {n}");
    }
    for c in &report.cards {
        let acct = c.account_id.as_deref().unwrap_or("?");
        println!(
            "card {} ({}): {} offers, {} added, {} expired, {}, account {acct}",
            c.mask,
            c.issuer,
            c.offers,
            c.added,
            c.expired,
            if c.earn {
                "earn table"
            } else {
                "no earn table"
            }
        );
        if let Some(p) = &c.problem {
            println!("  ! {p}; only add-offer suggestions are possible");
        }
    }
    if report.findings.is_empty() {
        println!("\nnothing found");
    } else {
        println!("\nFINDINGS ({})", report.findings.len());
        print_findings(&report.findings);
    }
    if !report.dismissed.is_empty() {
        println!("\nDISMISSED ({}; --restore KEY)", report.dismissed.len());
        print_findings(&report.dismissed);
    }
    if !report.reviewed.is_empty() {
        println!(
            "\nREVIEWED ({}; --unreview MERCHANT)",
            report.reviewed.len()
        );
        print_findings(&report.reviewed);
    }
    Ok(())
}

fn print_findings(findings: &[outflow_core::Finding]) {
    for f in findings {
        let value = match (f.est_cents_per_year, f.est_points_per_year) {
            (c, 0) => format!("{}/yr", dollars(c)),
            (0, p) => format!("{p} pts/yr"),
            (c, p) => format!("{} + {p} pts/yr", dollars(c)),
        };
        println!("  {value:>14}  {:<12}  {}", f.kind.as_str(), f.action);
        let o = match &f.evidence {
            Evidence::Offer(o) => o,
            Evidence::Subscription(s) => {
                let mut detail = format!(
                    "{} {}, last {}",
                    dollars(s.charge_cents),
                    if s.per_year == 1 { "yearly" } else { "monthly" },
                    date_label(s.last_charge)
                );
                if let Some(p) = &s.price_rise {
                    detail.push_str(&format!(
                        " · was {}, up since {}",
                        dollars(p.from_cents),
                        date_label(p.since)
                    ));
                }
                if let Some(m) = &f.card_mask {
                    detail.push_str(&format!(" · card {m}"));
                }
                if let Some(at) = s.reviewed_at {
                    detail.push_str(&format!(" · reviewed {}", date_label(at)));
                }
                println!("  {:>14}  {detail}", "");
                println!("  {:>14}  merchant {:?} · key {}", "", s.merchant, f.key);
                continue;
            }
            Evidence::Earn(e) => {
                let cards: Vec<String> = e
                    .cards
                    .iter()
                    .map(|c| {
                        let rate = match c.cents_per_point {
                            Some(v) => format!("{}x at {v}¢", c.rate),
                            None => format!("{}%", c.rate),
                        };
                        format!(
                            "{} {rate} = {}%: {} spent",
                            c.mask,
                            Money::from_cents(c.return_bp).to_display(),
                            dollars(c.spend_cents)
                        )
                    })
                    .collect();
                println!("  {:>14}  {} · {}", "", e.category, cards.join(" · "));
                println!("  {:>14}  key {}", "", f.key);
                continue;
            }
        };
        // Points valued off 1¢ (the card's earn table says what they're worth).
        if f.est_value_cents_per_year != f.est_cents_per_year + f.est_points_per_year {
            println!(
                "  {:>14}  worth about {}/yr at the card's point value",
                "",
                dollars(f.est_value_cents_per_year)
            );
        }
        let history = match (o.last_charge, o.typical_charge_cents) {
            (Some(last), Some(typ)) => format!(
                "{} charge{}, typical {}, last {}",
                o.charges,
                if o.charges == 1 { "" } else { "s" },
                dollars(typ),
                date_label(last)
            ),
            _ => "no charges in the window".to_string(),
        };
        let mut detail = format!("{history} · {}", reward_label(&o.offer));
        if !o.matched_merchants.is_empty() {
            detail.push_str(&format!(" · matched: {}", o.matched_merchants.join(", ")));
        }
        if let Some(n) = &o.offer.note {
            detail.push_str(&format!(" · {n}"));
        }
        println!("  {:>14}  {detail}", "");
        println!("  {:>14}  key {}", "", f.key);
    }
}

fn date_label(secs: i64) -> String {
    // The local day, like every other date in outflow.
    outflow_core::period::local_day(secs)
        .format("%Y-%m-%d")
        .to_string()
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

/// The signed breakdown on one line, terms that are 0 left out (cash and
/// invested always shown): `cash + invested - credit - loans = net worth`,
/// owed amounts printed after a minus sign.
fn breakdown_line(t: &outflow_core::Totals) -> String {
    let mut line = format!("cash {}", dollars(t.cash.cents()));
    line += &format!("  + invested {}", dollars(t.invested.cents()));
    if t.uninvested.cents() != 0 {
        line += &format!(" (incl. {} uninvested)", dollars(t.uninvested.cents()));
    }
    for (name, m) in [("credit", t.credit), ("loans", t.loans), ("other", t.other)] {
        let c = m.cents();
        if c != 0 {
            let sign = if c < 0 { '-' } else { '+' };
            line += &format!("  {sign} {name} {}", dollars(c.abs()));
        }
    }
    line + &format!("  = net worth {}", dollars(t.net_worth.cents()))
}

/// `change`'s range: the one given, else this month.
pub(crate) fn change_period(p: &PeriodArgs, today: NaiveDate) -> Result<Period, String> {
    match p.resolve_at(today)? {
        Some(period) => Ok(period),
        None => Period::parse("this-month", today).map_err(|e| e.to_string()),
    }
}

/// `+1.2%` / `-1.2%`: a change in percent always carries its sign.
fn signed_pct(bp: i64) -> String {
    format!("{}{}", if bp > 0 { "+" } else { "" }, pct(bp))
}

/// `+$1.00` / `-$1.00`: a change always carries its sign.
fn signed(cents: i64) -> String {
    let sign = if cents < 0 { '-' } else { '+' };
    format!("{sign}{}", dollars(cents.abs()))
}

/// `checkin`'s period: the one given, else this month.
pub(crate) fn checkin_period(p: Option<&str>, today: NaiveDate) -> Result<Period, String> {
    Period::parse(p.unwrap_or("this-month"), today).map_err(|e| e.to_string())
}

fn cmd_checkin_categories(store: &Store, period: Option<&str>, json: bool) -> Result<(), String> {
    use outflow_core::CategoryChange;
    let today = today_local();
    let p = checkin_period(period, today)?;
    let c = outflow_core::categories_vs_usual(store, p, today).map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&c);
    }
    let by = c
        .to_date
        .as_deref()
        .map(|d| format!(" by {d}"))
        .unwrap_or_default();
    println!(
        "{}  out by category, usual{by} = median of {} earlier",
        c.label, c.periods
    );
    for i in &c.items {
        let usual = i.usual_cents.map(dollars).unwrap_or_else(|| "-".into());
        let mark = match i.change {
            Some(CategoryChange::Up) => "  UP",
            Some(CategoryChange::Down) => "  down",
            None => "",
        };
        println!(
            "  {:<28} {:>12}   usual {:>12}{mark}",
            i.category.as_deref().unwrap_or("(uncategorized)"),
            dollars(i.total_cents),
            usual
        );
    }
    Ok(())
}

fn cmd_checkin(
    store: &Store,
    period: Option<&str>,
    offers: Option<&std::path::Path>,
    expiring_days: i64,
    json: bool,
) -> Result<(), String> {
    use outflow_core::{Check, NotCheckedReason, UnusualKind};
    let today = today_local();
    let p = checkin_period(period, today)?;
    let opts = FindOptions {
        expiring_days: expiring_days.max(0),
    };
    let c = outflow_core::checkin(store, p, today, now_secs(), offers, &opts)
        .map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&c);
    }
    let by = c
        .usual
        .to_date
        .as_deref()
        .map(|d| format!(" by {d}"))
        .unwrap_or_default();
    let usual = |u: Option<i64>| {
        u.map(|v| format!("   usual{by} {}", signed(v)))
            .unwrap_or_default()
    };
    println!(
        "{}  ({} to {}){}",
        c.label,
        p.start,
        p.last_day(),
        if c.in_progress { "  so far" } else { "" }
    );
    let n = &c.net_worth;
    match n.change_cents {
        Some(ch) => println!(
            "net worth change  {}{}   markets {}   earned - spent {}   other {}{}",
            signed(ch),
            n.change_bp
                .map(|bp| format!(" ({})", signed_pct(bp)))
                .unwrap_or_default(),
            n.markets_cents.map(signed).unwrap_or_else(|| "-".into()),
            n.earned_minus_spent_cents
                .map(signed)
                .unwrap_or_else(|| "-".into()),
            n.other_cents.map(signed).unwrap_or_else(|| "-".into()),
            usual(c.usual.net_worth_change_cents)
        ),
        None => println!("net worth change  -  (fewer than two snapshots in the period)"),
    }
    if !n.excluded.is_empty() {
        println!(
            "  other includes unmeasured accounts: {}",
            n.excluded.join(", ")
        );
    }
    let f = &c.flow;
    println!(
        "earned - spent    {}{}",
        signed(f.earned_minus_spent_cents),
        usual(c.usual.earned_minus_spent_cents)
    );
    println!(
        "saved             {}{}",
        signed(f.saved_cents),
        usual(c.usual.saved_cents)
    );
    println!(
        "  in {}   out {}   left in checking {}   (usual over {} earlier periods)",
        dollars(f.in_cents),
        dollars(f.out_cents),
        signed(f.gap_cents),
        c.usual.periods
    );
    if let Some(v) = &c.versus {
        let what = match v.figure {
            outflow_core::checkin::VersusFigure::Spent => "spent",
            outflow_core::checkin::VersusFigure::EarnedMinusSpent => "earned - spent",
        };
        let judged = if v.outside {
            "outside the usual"
        } else if v.far {
            "far from the usual, as some periods are"
        } else {
            "within the usual"
        };
        print!(
            "versus usual      {what} {} vs {}{by}: {} ({judged})",
            signed(v.cents),
            signed(v.usual_cents),
            signed(v.diff_cents)
        );
        if let Some(d) = v.in_diff_cents.filter(|d| *d != 0) {
            print!("; income {}", signed(d));
        }
        println!();
        if !v.contributors.is_empty() {
            let names: Vec<String> = v
                .contributors
                .iter()
                .map(|x| format!("{} {}", x.label, dollars(x.cents)))
                .collect();
            println!("  behind it: {}", names.join(", "));
        }
    }
    // Month check-ins: the finished months that fell outside their usual.
    let marked: Vec<String> = c
        .months
        .iter()
        .filter(|m| m.outside)
        .map(|m| {
            format!(
                "{} ({})",
                m.month,
                if m.worse { "kept less" } else { "kept more" }
            )
        })
        .collect();
    if !marked.is_empty() {
        println!("months outside usual: {}", marked.join(", "));
    }
    println!();
    if c.unusual.is_empty() {
        println!("nothing unusual");
    }
    for u in &c.unusual {
        let amt = |v: Option<i64>| v.map(dollars).unwrap_or_else(|| "-".into());
        let line = match u.kind {
            UnusualKind::Late => format!(
                "late        {}  last charge {}, usually every {} days, {} a charge",
                u.label,
                u.day.as_deref().unwrap_or("?"),
                u.gap_days.unwrap_or(0),
                amt(u.cents)
            ),
            UnusualKind::Connection => format!(
                "connection  {}: {}",
                u.label,
                u.status.as_deref().unwrap_or("?")
            ),
            UnusualKind::MatchReview => {
                format!(
                    "review      {} card payment(s) to review",
                    u.count.unwrap_or(0)
                )
            }
            UnusualKind::NewRecurring => format!(
                "new         {}  {} a charge, first {}",
                u.label,
                amt(u.cents),
                u.day.as_deref().unwrap_or("?")
            ),
            UnusualKind::RecurringUp | UnusualKind::RecurringDown => {
                let dir = if u.kind == UnusualKind::RecurringUp {
                    "up  "
                } else {
                    "down"
                };
                let pct = u
                    .change_pct
                    .map(|p| format!(" ({p:+.0}%)"))
                    .unwrap_or_default();
                if u.per_charge {
                    format!(
                        "{dir}        {}  {} charge on {} vs usual charge {}{pct}",
                        u.label,
                        amt(u.cents),
                        u.day.as_deref().unwrap_or("?"),
                        amt(u.usual_cents),
                    )
                } else {
                    format!(
                        "{dir}        {}  {} in {} vs {} a month before{pct}",
                        u.label,
                        amt(u.cents),
                        u.month.as_deref().unwrap_or("?"),
                        amt(u.usual_cents),
                    )
                }
            }
            UnusualKind::LargeCharge => format!(
                "large       {}  {} on {} (your 95th percentile {})  [{}]",
                u.label,
                amt(u.cents),
                u.day.as_deref().unwrap_or("?"),
                amt(u.usual_cents),
                u.txn_id.as_deref().unwrap_or("")
            ),
            UnusualKind::Finding => format!(
                "find money  {}: {} (about {} a year)  [{}]",
                u.label,
                u.action.as_deref().unwrap_or(""),
                amt(u.cents),
                u.finding_key.as_deref().unwrap_or("")
            ),
        };
        let minor = if u.minor { "  (minor)" } else { "" };
        let earlier = match (u.kind, u.change_pct, u.month.as_deref()) {
            (UnusualKind::Late, Some(p), Some(m)) => format!("; {m} was {p:+.0}%"),
            _ => String::new(),
        };
        println!("  {line}{earlier}{minor}");
    }
    for nc in &c.not_checked {
        let what = match nc.check {
            Check::RecurringChanges => "recurring cost changes",
            Check::NewRecurring => "new recurring merchants",
            Check::LargeCharges => "large charges",
            Check::CardOffers => "card offer Findings",
        };
        let why = match nc.reason {
            NotCheckedReason::Week => "monthly costs aren't judged by the week",
            NotCheckedReason::InProgress => "no completed month in the period yet",
            NotCheckedReason::ShortHistory => "too few charges before the period",
            NotCheckedReason::Range => "check-in covers weeks, months and quarters",
            NotCheckedReason::OffersFile => "the offers file can't be read; run outflow find",
        };
        println!("  not checked: {what} ({why})");
    }
    Ok(())
}

fn cmd_change(store: &Store, period: &PeriodArgs, json: bool) -> Result<(), String> {
    use outflow_core::{ChangeReason, GainReason};
    let p = change_period(period, today_local())?;
    let (since, until) = p.bounds();
    let c =
        outflow_core::stock_change(store, Some(since), Some(until)).map_err(|e| format!("{e}"))?;
    if json {
        return json_out(&c);
    }
    println!("{p}  ({} to {})", p.start, p.last_day());
    let Some(change) = c.change else {
        let why = match c.reason {
            Some(ChangeReason::NoSnapshots) => "no snapshots in this period",
            _ => "only one snapshot in this period",
        };
        println!("no change: {why}");
        return Ok(());
    };
    println!(
        "net worth {} -> {}   change {}{}{}",
        dollars(c.start.net_worth.cents()),
        dollars(c.end.net_worth.cents()),
        signed(change.cents()),
        c.change_bp
            .map(|bp| format!(" ({})", signed_pct(bp)))
            .unwrap_or_default(),
        if c.partial_start {
            "   (partial start: some accounts begin mid-period)"
        } else {
            ""
        }
    );
    if let (Some(m), Some(e), Some(o)) = (c.markets, c.earned_minus_spent, c.other) {
        println!(
            "  markets {}   earned - spent {}   other {}",
            signed(m.cents()),
            signed(e.cents()),
            signed(o.cents())
        );
    }
    for x in &c.excluded {
        let why = match x.reason {
            GainReason::BalanceMismatch => "holdings don't match the balance",
            _ => "contributions unknown",
        };
        println!(
            "  other includes {} {} ({why})",
            x.account,
            signed(x.change.cents())
        );
    }
    println!(
        "\n  {:<40} {:>14} {:>14} {:>13} {:>13}",
        "account", "start", "end", "change", "gain"
    );
    for a in &c.accounts {
        let change = a
            .change
            .map(|m| signed(m.cents()))
            .unwrap_or_else(|| "-".into());
        let gain = match (a.gain, a.gain_reason) {
            (Some(g), _) => signed(g.cents()),
            (None, Some(GainReason::Cash)) => "cash".into(),
            (None, Some(GainReason::ContributionsUnknown)) => "unknown".into(),
            (None, Some(GainReason::BalanceMismatch)) => "mismatch".into(),
            (None, None) => String::new(),
        };
        println!(
            "  {:<40} {:>14} {:>14} {:>13} {:>13}{}",
            truncate(&a.account, 40),
            dollars(a.start.cents()),
            dollars(a.end.cents()),
            change,
            gain,
            if a.partial_start {
                "  (from first snapshot)"
            } else {
                ""
            }
        );
    }
    Ok(())
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
            "{:<10} {:>15} {:>14} {:>15} {:>14} {:>13} {:>14} {:>12}",
            "day", "net worth", "cash", "invested", "uninvested", "credit", "loans", "other"
        );
        for d in &days {
            println!(
                "{:<10} {:>15} {:>14} {:>15} {:>14} {:>13} {:>14} {:>12}",
                d.day,
                dollars(d.net_worth.cents()),
                dollars(d.cash.cents()),
                dollars(d.invested.cents()),
                dollars(d.uninvested.cents()),
                dollars(d.credit.cents()),
                dollars(d.loans.cents()),
                dollars(d.other.cents())
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
    println!("{}", breakdown_line(&view.totals));
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
    let view = holdings_view(a, today_local())?;
    let accounts = store.accounts().map_err(|e| format!("{e}"))?;
    let securities = store.securities().map_err(|e| format!("{e}"))?;
    if let HoldingsView::Change(p, lens) = view {
        return holdings_change(store, &accounts, &securities, &filter, p, lens, json);
    }
    if let HoldingsView::History(range) = view {
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
            r.gain_bp.map(signed_pct).unwrap_or_default()
        );
    }
    Ok(())
}

/// `holdings --period P [--by position]`: gain over the period per position.
fn holdings_change(
    store: &Store,
    accounts: &[outflow_core::Account],
    securities: &[outflow_core::Security],
    filter: &PositionFilter,
    p: Period,
    lens: PositionLens,
    json: bool,
) -> Result<(), String> {
    use outflow_core::{ChangeReason, GainReason};
    let (since, until) = p.bounds();
    let snaps = store
        .holding_snapshots(None, Some(until))
        .map_err(|e| format!("{e}"))?;
    let current = store
        .current_holdings_synced()
        .map_err(|e| format!("{e}"))?;
    let bals = store
        .balance_snapshots(None, Some(until))
        .map_err(|e| format!("{e}"))?;
    let txns = store.investment_txns().map_err(|e| format!("{e}"))?;
    let rows = position_change(
        accounts,
        securities,
        &snaps,
        &bals,
        &current,
        &txns,
        Some(since),
        Some(until),
        filter,
        lens,
    );
    if json {
        return json_out(&rows);
    }
    println!("{p}  ({} to {})", p.start, p.last_day());
    println!(
        "{:<12} {:<28} {:>12} {:>14} {:>13} {:>8} {:>14} {:>13} {:>8}",
        "holding", "account", "qty", "value", "period gain", "", "basis", "unrealized", ""
    );
    for r in &rows {
        let gain = match (r.gain, r.gain_reason, r.change_reason) {
            (Some(g), _, _) => signed(g.cents()),
            (None, Some(GainReason::Cash), _) => "cash".into(),
            (None, Some(GainReason::ContributionsUnknown), _) => "unknown".into(),
            (None, _, Some(ChangeReason::OneSnapshot)) => "one snapshot".into(),
            _ => "-".into(),
        };
        let note = [
            r.sold.then_some("sold"),
            r.partial_start.then_some("from first snapshot"),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(", ");
        println!(
            "{:<12} {:<28} {:>12} {:>14} {:>13} {:>8} {:>14} {:>13} {:>8}{}{}",
            truncate(&r.label, 12),
            truncate(&r.account, 28),
            r.end_quantity,
            dollars(r.end_value.cents()),
            gain,
            r.gain_bp.map(signed_pct).unwrap_or_default(),
            r.cost_basis
                .map(|b| dollars(b.cents()))
                .unwrap_or_else(|| "-".into()),
            r.unrealized_gain
                .map(|g| signed(g.cents()))
                .unwrap_or_default(),
            r.unrealized_bp.map(signed_pct).unwrap_or_default(),
            r.price_as_of
                .as_deref()
                .map(|d| format!("  priced as of {d}"))
                .unwrap_or_default(),
            if note.is_empty() {
                String::new()
            } else {
                format!("  ({note})")
            }
        );
    }
    if lens == PositionLens::Account && !rows.is_empty() {
        println!("\nPosition gains don't sum to an account's gain (see `outflow change`).");
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
    let up = outflow_core::upkeep(store).map_err(|e| format!("{e}"))?;
    let version = outflow_core::build::VERSION;
    if json {
        return json_out(&serde_json::json!({
            "version": version,
            "upkeep": up,
            "sync_log": log,
        }));
    }
    println!("version: {}", version.unwrap_or("unreleased build"));
    print_upkeep(&up);
    println!();
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

/// The upkeep lines `status` leads with (core::upkeep, same as /api/upkeep).
fn print_upkeep(up: &outflow_core::Upkeep) {
    match up.last_synced {
        Some(t) => println!("last synced {}", date_label(t)),
        None => println!("never synced"),
    }
    if up.total == 0 {
        println!("nothing needs you");
    }
    for c in &up.connections {
        println!(
            "connection  {}: {} ({})",
            c.institution, c.status, c.item_id
        );
    }
    if up.proposed_matches > 0 {
        println!(
            "review      {} card payment(s) to review",
            up.proposed_matches
        );
    }
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
            if !store.accept_match(*id).map_err(|e| format!("{e}"))? {
                return Err(format!("no match {id}"));
            }
            println!("accepted #{id}: both legs excluded as card payment");
            Ok(())
        }
        MatchCmd::Reject { id } => {
            if !store.reject_match(*id).map_err(|e| format!("{e}"))? {
                return Err(format!("no match {id}"));
            }
            println!("rejected #{id}");
            Ok(())
        }
        MatchCmd::Unaccept { id } => match store.unaccept_match(*id).map_err(|e| format!("{e}"))? {
            outflow_core::Unaccept::Done => {
                println!("#{id} is back in review; both legs count as before");
                Ok(())
            }
            outflow_core::Unaccept::NotFound => Err(format!("no match {id}")),
            outflow_core::Unaccept::NotAccepted(s) => {
                Err(format!("match {id} is {}, not accepted", s.as_str()))
            }
        },
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
    if let Cmd::Backup { to } = &cli.cmd {
        return cmd_backup(&cli.db, to, cli.json);
    }
    let store = open_store(&cli.db)?;
    match cli.cmd {
        Cmd::Backup { .. } => unreachable!("handled above"),
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
        Cmd::Recurring {
            period,
            ignored,
            subscriptions,
        } => cmd_recurring(&store, &period, ignored, subscriptions, cli.json),
        Cmd::Fix {
            txn_id,
            category,
            no_learn,
        } => cmd_fix(&store, txn_id, category, no_learn),
        Cmd::Txns(args) => cmd_txns(&store, &args, cli.json),
        Cmd::Accounts { all } => cmd_accounts(&store, all, cli.json),
        Cmd::Balances { history, period } => cmd_balances(&store, history, &period, cli.json),
        Cmd::Change { period } => cmd_change(&store, &period, cli.json),
        Cmd::Checkin {
            period,
            categories: true,
            ..
        } => cmd_checkin_categories(&store, period.as_deref(), cli.json),
        Cmd::Checkin {
            period,
            offers,
            expiring_days,
            ..
        } => cmd_checkin(
            &store,
            period.as_deref(),
            offers.as_deref().map(std::path::Path::new),
            expiring_days,
            cli.json,
        ),
        Cmd::Holdings(args) => cmd_holdings(&store, &args, cli.json),
        Cmd::Activity(args) => cmd_activity(&store, &args, cli.json),
        Cmd::Matches { action } => cmd_matches(&store, &action, cli.json),
        Cmd::Status { limit } => cmd_status(&store, limit, cli.json),
        Cmd::Find {
            file,
            period,
            expiring_days,
            dismiss,
            restore,
            review,
            unreview,
        } => {
            let mark = FindMark::from_args(&dismiss, &restore, &review, &unreview);
            cmd_find(&store, file, &period, expiring_days, mark, cli.json)
        }
    }
}

fn report_json(store: &Store, by: By, top: usize, filter: &TxnFilter) -> Result<(), String> {
    match by {
        By::Category => json_out(&spend_by_category(store, filter).map_err(|e| format!("{e}"))?),
        By::Merchant => json_out(&top_merchants(store, filter, top).map_err(|e| format!("{e}"))?),
        By::Monthly => json_out(&flow_lanes(store, filter).map_err(|e| format!("{e}"))?),
        By::Source => {
            json_out(&outflow_core::in_sources(store, filter).map_err(|e| format!("{e}"))?)
        }
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
    #[allow(clippy::inconsistent_digit_grouping)] // dollars_cents literals
    fn breakdown_line_sums_visibly() {
        let m = Money::from_cents;
        let t = outflow_core::Totals {
            net_worth: m(128_860_00),
            cash: m(320_00),
            invested: m(141_350_00),
            uninvested: m(6_422_00),
            credit: m(-410_00),
            loans: m(-12_400_00),
            other: m(0),
        };
        let line = breakdown_line(&t);
        assert!(line.starts_with("cash $320.00  + invested $141350.00 (incl. $6422.00 uninvested)"));
        assert!(line.ends_with("- credit $410.00  - loans $12400.00  = net worth $128860.00"));
    }

    #[test]
    fn holdings_view_picks_current_history_or_change() {
        let parse = |argv: &[&str]| {
            let mut v = vec!["outflow", "holdings"];
            v.extend_from_slice(argv);
            match Cli::try_parse_from(v).unwrap().cmd {
                Cmd::Holdings(a) => holdings_view(&a, today()),
                _ => unreachable!(),
            }
        };
        assert_eq!(parse(&[]), Ok(HoldingsView::Current));
        assert!(matches!(
            parse(&["--history"]),
            Ok(HoldingsView::History(None))
        ));
        let nov = Period::parse("2026-11", today()).unwrap();
        assert_eq!(
            parse(&["--by", "position"]),
            Ok(HoldingsView::Change(nov, PositionLens::Position))
        );
        let oct = Period::parse("2026-10", today()).unwrap();
        assert_eq!(
            parse(&["--period", "last-month"]),
            Ok(HoldingsView::Change(oct, PositionLens::Account))
        );
        assert!(parse(&["--history", "--by", "position"]).is_err());
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
            &["outflow", "find", "--period", "last-month"],
        ] {
            assert!(Cli::try_parse_from(argv).is_ok(), "{argv:?}");
        }
    }
}
