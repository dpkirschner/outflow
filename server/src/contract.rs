//! Wire contract: one hand-written fixture per response DTO the API sends,
//! serialized exactly as the routes do and compared to a checked-in snapshot.
//!
//! The same test writes `app/src/contract.gen.ts`, which assigns each fixture
//! to its generated TS type, so the app's `tsc` run checks the real wire JSON
//! against `app/src/types.ts`. Both files regenerate with
//! `UPDATE_TYPES=1 cargo test -p outflow-server contract`.
//!
//! Every value here is invented. Keep it obviously synthetic: this repo is
//! public.

use std::path::PathBuf;

use outflow_core::balances::{
    AccountBalance, BalancesView, DayTotals, Group, GroupBalance, HoldingRow, PositionPoint,
    PositionRow, Totals,
};
use outflow_core::ledger::{
    Coverage, LedgerStats, LedgerView, LineItem, Source, Stream, TransferGroup,
};
use outflow_core::model::{
    Account, AccountKind, CategorySource, MatchConfidence, MatchStatus, PlaidItem, SyncEntry,
    Transaction, TxnFlag, TxnMatch,
};
use outflow_core::perks::{Cap, CardStatus, Finding, FindingKind, Offer, PerksReport, Reward};
use outflow_core::query::{CategorySpend, FlowMonth, MerchantSpend, MovedFlow, MovedLane, TxnPage};
use outflow_core::subscriptions::{Cadence, RhythmEntry, StreamCadence, Subscription, Trend};
use outflow_core::{ActivityRow, Money};
use serde::Serialize;

use crate::match_routes::MatchView;
use crate::plaid_routes::ExchangeResult;
use crate::routes::CategorizeResult;
use crate::sync::{LegReport, SyncReport};

/// 2026-01-15T00:00:00Z, the anchor every synthetic timestamp hangs off.
const T0: i64 = 1_768_435_200;
const DAY: i64 = 86_400;

fn m(cents: i64) -> Money {
    Money::from_cents(cents)
}

fn account() -> Account {
    Account {
        id: "acct-demo-card".into(),
        org: "Demo Bank".into(),
        name: "Demo Rewards Card (0000)".into(),
        kind: AccountKind::Credit,
        balance: m(-12_345),
        currency: "USD".into(),
        last_synced: T0,
        source: "plaid".into(),
        nickname: Some("Test card".into()),
        hidden: false,
    }
}

fn checking() -> Account {
    Account {
        id: "acct-demo-checking".into(),
        org: "Demo Bank".into(),
        name: "Demo Checking (1111)".into(),
        kind: AccountKind::Checking,
        balance: m(250_000),
        currency: "USD".into(),
        last_synced: T0,
        source: "plaid".into(),
        nickname: None,
        hidden: false,
    }
}

fn transaction() -> Transaction {
    Transaction {
        id: "txn-demo-001".into(),
        account_id: "acct-demo-card".into(),
        posted: T0,
        transacted_at: Some(T0 - DAY),
        amount: m(-1_234),
        description: "EXAMPLE COFFEE CO #42".into(),
        payee: Some("Example Coffee Co".into()),
        category: Some("Dining".into()),
        category_source: Some(CategorySource::Rule),
        pending: false,
        flag: TxnFlag::Spending,
        raw: "{}".into(),
        pfc_primary: None,
        pfc_detailed: None,
        source: "plaid".into(),
    }
}

fn payment_leg(id: &str, account_id: &str, cents: i64) -> Transaction {
    Transaction {
        id: id.into(),
        account_id: account_id.into(),
        posted: T0 + DAY,
        transacted_at: None,
        amount: m(cents),
        description: "DEMO CARD PAYMENT".into(),
        payee: None,
        category: None,
        category_source: None,
        pending: true,
        flag: TxnFlag::CardPayment,
        raw: "{}".into(),
        pfc_primary: None,
        pfc_detailed: None,
        source: "plaid".into(),
    }
}

fn txn_page() -> TxnPage {
    TxnPage {
        total: 2,
        total_cents: -1_234 - 50_000,
        items: vec![
            transaction(),
            payment_leg("txn-demo-002", "acct-demo-checking", -50_000),
        ],
    }
}

fn category_spend() -> CategorySpend {
    CategorySpend {
        category: Some("Groceries".into()),
        total_cents: -45_600,
        count: 7,
    }
}

fn merchant_spend() -> MerchantSpend {
    MerchantSpend {
        merchant: "Sample Grocer".into(),
        total_cents: -45_600,
        count: 7,
    }
}

fn flow_month() -> FlowMonth {
    FlowMonth {
        year: 2026,
        month: 1,
        in_cents: 500_000,
        in_uncategorized_cents: 15_000,
        out_cents: 310_000,
        refunds_cents: 2_500,
        moved: MovedLane {
            card_payment_cents: 120_000,
            card_payment_bank_cents: 120_000,
            card_payment_card_cents: 118_000,
            invest: MovedFlow {
                out_cents: 50_000,
                in_cents: 0,
            },
            save: MovedFlow {
                out_cents: 20_000,
                in_cents: 5_000,
            },
            loan_paydown: MovedFlow {
                out_cents: 30_000,
                in_cents: 0,
            },
            between: MovedFlow {
                out_cents: 10_000,
                in_cents: 10_000,
            },
        },
        earned_minus_spent_cents: 190_000,
        saved_cents: 95_000,
        gap_cents: 95_000,
    }
}

fn subscription() -> Subscription {
    Subscription {
        payee: "Sample Streaming".into(),
        cadence: Cadence::Yearly,
        typical_amount_cents: -9_900,
        occurrences: 3,
        first_seen: T0 - 730 * DAY,
        last_seen: T0,
        total_cents: -29_700,
    }
}

fn source(kind: AccountKind, pct: u8) -> Source {
    Source {
        label: "Demo Rewards Card (0000)".into(),
        kind,
        pct,
    }
}

fn stream() -> Stream {
    Stream {
        rhythm: RhythmEntry {
            merchant: "Example Coffee Co".into(),
            cadence: StreamCadence::FewPerWeek,
            occurrence_count: 36,
            median_amount_cents: -650,
            amount_min_cents: -450,
            amount_max_cents: -1_200,
            monthly_estimate_cents: -7_800,
            last_seen: T0,
            trend_pct: -12.5,
            trend: Trend::Falling,
            spark_cents: vec![-8_400, -7_900, -7_100, -3_200],
        },
        sources: vec![
            source(AccountKind::Credit, 75),
            source(AccountKind::Checking, 25),
        ],
        category: Some("Dining".into()),
    }
}

fn line_item() -> LineItem {
    LineItem {
        id: "txn-demo-003".into(),
        merchant: "Test Hardware Store".into(),
        merchant_key: "test hardware store".into(),
        date: T0 - 3 * DAY,
        amount_cents: -18_900,
        category: None,
        source: source(AccountKind::Credit, 100),
    }
}

fn ledger_view() -> LedgerView {
    let mut committed = stream();
    committed.rhythm.merchant = "Sample Utility".into();
    committed.rhythm.cadence = StreamCadence::Monthly;
    committed.rhythm.trend = Trend::Steady;
    committed.category = None;
    LedgerView {
        coverage: Coverage {
            earliest: Some(T0 - 400 * DAY),
            window_txn_count: 120,
            covers_all: false,
        },
        stats: LedgerStats {
            recurring_monthly_cents: -7_800,
            stream_count: 1,
            committed_monthly_cents: -9_000,
            noise_total_cents: -2_100,
            noise_count: 4,
        },
        streams: vec![stream()],
        committed: vec![committed],
        notable: vec![line_item()],
        noise_items: vec![line_item()],
        transfers: vec![TransferGroup {
            merchant: "Demo Brokerage".into(),
            flag: TxnFlag::Invest,
            total_cents: -50_000,
            count: 2,
        }],
    }
}

fn holding_row() -> HoldingRow {
    HoldingRow {
        security_id: "sec-demo-fund".into(),
        label: "DEMO".into(),
        name: "Demo Total Market Fund".into(),
        sec_type: "etf".into(),
        is_cash: false,
        quantity: "12.3456".into(),
        price: "101.25".into(),
        value: m(125_000),
        cost_basis: Some(m(100_000)),
        gain: Some(m(25_000)),
        share_bp: 8_000,
    }
}

fn balances_view() -> BalancesView {
    let brokerage = Account {
        id: "acct-demo-brokerage".into(),
        org: "Demo Brokerage".into(),
        name: "Demo Brokerage (2222)".into(),
        kind: AccountKind::Investment,
        balance: m(156_250),
        currency: "USD".into(),
        last_synced: T0,
        source: "plaid".into(),
        nickname: None,
        hidden: false,
    };
    let cash_row = HoldingRow {
        security_id: "sec-demo-mmf".into(),
        label: "Demo Money Market".into(),
        name: "Demo Money Market".into(),
        sec_type: "cash".into(),
        is_cash: true,
        quantity: "31250".into(),
        price: "1".into(),
        value: m(31_250),
        cost_basis: None,
        gain: None,
        share_bp: 2_000,
    };
    BalancesView {
        totals: Totals {
            net_worth: m(393_905),
            cash: m(281_250),
            invested: m(125_000),
            credit: m(-12_345),
        },
        groups: vec![
            GroupBalance {
                group: Group::Investments,
                total: m(156_250),
                accounts: vec![AccountBalance {
                    account: brokerage,
                    group: Group::Investments,
                    cash: m(31_250),
                    invested: m(125_000),
                    cash_bp: 2_000,
                    idle_cash: true,
                    holdings: vec![holding_row(), cash_row],
                }],
            },
            GroupBalance {
                group: Group::Credit,
                total: m(-12_345),
                accounts: vec![AccountBalance {
                    account: account(),
                    group: Group::Credit,
                    cash: m(0),
                    invested: m(0),
                    cash_bp: 0,
                    idle_cash: false,
                    holdings: vec![],
                }],
            },
        ],
    }
}

fn day_totals() -> DayTotals {
    DayTotals {
        day: "2026-01-15".into(),
        net_worth: m(393_905),
        cash: m(281_250),
        invested: m(125_000),
        credit: m(-12_345),
    }
}

fn position_row() -> PositionRow {
    PositionRow {
        account_id: "acct-demo-brokerage".into(),
        account: "Demo Brokerage (2222)".into(),
        security_id: "sec-demo-fund".into(),
        ticker: Some("DEMO".into()),
        label: "DEMO".into(),
        name: "Demo Total Market Fund".into(),
        sec_type: "etf".into(),
        is_cash: false,
        cusip: Some("000000000".into()),
        isin: None,
        currency: Some("USD".into()),
        quantity: "12.3456".into(),
        price: "101.25".into(),
        price_as_of: Some("2026-01-14".into()),
        close_price: Some("101.10".into()),
        close_price_as_of: Some("2026-01-14".into()),
        value: m(125_000),
        cost_basis: Some(m(100_000)),
        unit_cost: Some("81.0005".into()),
        gain: Some(m(25_000)),
        gain_bp: Some(2_500),
        share_bp: 8_000,
        synced_at: Some(T0),
    }
}

fn position_point() -> PositionPoint {
    PositionPoint {
        taken_at: T0,
        day: "2026-01-15".into(),
        account_id: "acct-demo-brokerage".into(),
        account: "Demo Brokerage (2222)".into(),
        security_id: "sec-demo-fund".into(),
        label: "DEMO".into(),
        quantity: "12.3456".into(),
        price: Some("101.25".into()),
        value: m(125_000),
        cost_basis: None,
        gain: None,
    }
}

fn activity_row() -> ActivityRow {
    ActivityRow {
        id: "inv-demo-001".into(),
        date: "2026-01-10".into(),
        account_id: "acct-demo-brokerage".into(),
        account: "Demo Brokerage (2222)".into(),
        security_id: Some("sec-demo-fund".into()),
        ticker: Some("DEMO".into()),
        label: "DEMO".into(),
        name: "BUY DEMO TOTAL MARKET FUND".into(),
        txn_type: "buy".into(),
        subtype: "buy".into(),
        quantity: "2.5".into(),
        price: "100.00".into(),
        amount: m(-25_000),
        fees: Some(m(0)),
        currency: Some("USD".into()),
        cancel_txn_id: None,
    }
}

fn offer(reward: Reward, cap: Cap) -> Offer {
    Offer {
        merchant: "Sample Outfitters".into(),
        reward,
        min: Some(m(5_000)),
        uses: Some(1),
        cap,
        single: true,
        per_year: false,
        added: false,
        expires: T0 + 30 * DAY,
        patterns: vec!["SAMPLE OUTFITTERS".into()],
        note: None,
    }
}

fn perks_report() -> PerksReport {
    PerksReport {
        cards: vec![
            CardStatus {
                issuer: "Demo Issuer".into(),
                mask: "0000".into(),
                account_id: Some("acct-demo-card".into()),
                problem: None,
                offers: 2,
                added: 1,
                expired: 0,
            },
            CardStatus {
                issuer: "Demo Issuer".into(),
                mask: "9999".into(),
                account_id: None,
                problem: Some("no linked account ends in 9999".into()),
                offers: 0,
                added: 0,
                expired: 0,
            },
        ],
        findings: vec![
            Finding {
                kind: FindingKind::AddOffer,
                card_mask: "0000".into(),
                offer: offer(Reward::Back(m(1_000)), Cap::None),
                days_left: 30,
                charges: 2,
                last_charge: Some(T0 - 10 * DAY),
                typical_charge_cents: Some(-6_000),
                est_cash_cents: 1_000,
                est_points: 0,
                txn_ids: vec!["txn-demo-004".into(), "txn-demo-005".into()],
                matched_merchants: vec!["Sample Outfitters".into()],
            },
            Finding {
                kind: FindingKind::Expiring,
                card_mask: "0000".into(),
                offer: offer(Reward::PtsPerDollar(3), Cap::Points(3_000)),
                days_left: 4,
                charges: 0,
                last_charge: None,
                typical_charge_cents: None,
                est_cash_cents: 0,
                est_points: 0,
                txn_ids: vec![],
                matched_merchants: vec![],
            },
        ],
    }
}

fn sync_report() -> SyncReport {
    SyncReport {
        legs: vec![
            LegReport {
                source: "plaid:Demo Bank".into(),
                added: 12,
                updated: 3,
                error: None,
                warning: Some("holdings: demo warning".into()),
            },
            LegReport {
                source: "plaid:Demo Brokerage".into(),
                added: 0,
                updated: 0,
                error: Some("ITEM_LOGIN_REQUIRED: demo error".into()),
                warning: None,
            },
        ],
        flags_applied: 4,
        categorized: 9,
        matches_auto: 1,
        matches_proposed: 2,
    }
}

fn sync_entry() -> SyncEntry {
    SyncEntry {
        id: 7,
        started: T0,
        finished: Some(T0 + 5),
        source: "plaid:Demo Bank".into(),
        added: Some(12),
        updated: Some(3),
        note: None,
    }
}

fn plaid_item() -> PlaidItem {
    PlaidItem {
        item_id: "item-demo-001".into(),
        institution: "Demo Bank".into(),
        cursor: Some("cursor-demo".into()),
        status: "login_required".into(),
        created: T0 - 90 * DAY,
        last_synced: None,
        investments_consent: true,
    }
}

fn match_view() -> MatchView {
    MatchView {
        m: TxnMatch {
            id: 3,
            bank_txn_id: "txn-demo-002".into(),
            card_txn_id: "txn-demo-006".into(),
            status: MatchStatus::Proposed,
            confidence: MatchConfidence::Medium,
            reason: Some("same amount, 1 day apart".into()),
            created: T0 + DAY,
        },
        bank: Some(payment_leg("txn-demo-002", &checking().id, -50_000)),
        card: None,
    }
}

fn categorize_result() -> CategorizeResult {
    CategorizeResult {
        rule: 18,
        remaining: 5,
    }
}

fn exchange_result() -> ExchangeResult {
    ExchangeResult {
        item_id: "item-demo-002".into(),
        institution: "Demo Credit Union".into(),
        added: 40,
        updated: 0,
        sync_error: None,
    }
}

/// `(TS type name, pretty JSON)` for every response DTO, in snapshot order.
/// Array routes (`Vec<T>`) are covered by one `T`.
fn entries() -> Vec<(&'static str, String)> {
    fn j<T: Serialize>(v: T) -> String {
        serde_json::to_string_pretty(&v).expect("serialize fixture")
    }
    vec![
        // routes.rs
        ("Account", j(account())),
        ("Transaction", j(transaction())),
        ("TxnPage", j(txn_page())),
        ("CategorySpend", j(category_spend())),
        ("MerchantSpend", j(merchant_spend())),
        ("FlowMonth", j(flow_month())),
        ("Subscription", j(subscription())),
        ("LedgerView", j(ledger_view())),
        ("Stream", j(stream())),
        ("BalancesView", j(balances_view())),
        ("DayTotals", j(day_totals())),
        ("PositionRow", j(position_row())),
        ("PositionPoint", j(position_point())),
        ("ActivityRow", j(activity_row())),
        ("SyncEntry", j(sync_entry())),
        ("SyncReport", j(sync_report())),
        ("CategorizeResult", j(categorize_result())),
        // perks_routes.rs
        ("PerksReport", j(perks_report())),
        // plaid_routes.rs
        ("PlaidItem", j(plaid_item())),
        ("ExchangeResult", j(exchange_result())),
        // match_routes.rs
        ("MatchView", j(match_view())),
    ]
}

fn indent(json: &str, by: &str) -> String {
    json.lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.to_owned()
            } else {
                format!("{by}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_json(entries: &[(&str, String)]) -> String {
    let body = entries
        .iter()
        .map(|(name, json)| format!("  \"{name}\": {}", indent(json, "  ")))
        .collect::<Vec<_>>()
        .join(",\n");
    format!("{{\n{body}\n}}\n")
}

fn lower_camel(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_ascii_lowercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

fn render_ts(entries: &[(&str, String)]) -> String {
    let mut names: Vec<&str> = entries.iter().map(|(n, _)| *n).collect();
    names.sort_unstable();
    let mut out = String::from(
        "// GENERATED by server/src/contract.rs from server/contract/responses.json.\n\
         // Do not edit. Regenerate: UPDATE_TYPES=1 cargo test -p outflow-server contract\n\
         //\n\
         // Each constant is a synthetic fixture exactly as the API serializes it,\n\
         // typed with the generated DTO type, so `tsc` fails when the two drift.\n\n",
    );
    out.push_str("import type {\n");
    for n in &names {
        out.push_str(&format!("  {n},\n"));
    }
    out.push_str("} from \"./types\";\n");
    for (name, json) in entries {
        out.push_str(&format!(
            "\nexport const {}: {name} = {json};\n",
            lower_camel(name)
        ));
    }
    out
}

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

#[test]
fn contract_snapshot() {
    let entries = entries();
    let files = [
        (repo_path("contract/responses.json"), render_json(&entries)),
        (repo_path("../app/src/contract.gen.ts"), render_ts(&entries)),
    ];
    if std::env::var_os("UPDATE_TYPES").is_some() {
        for (path, want) in &files {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).expect("create snapshot dir");
            }
            std::fs::write(path, want).expect("write contract snapshot");
        }
        return;
    }
    for (path, want) in &files {
        let have = std::fs::read_to_string(path).unwrap_or_default();
        assert!(
            &have == want,
            "{} is stale: a response DTO's wire shape changed. Review the diff, \
             then regenerate with:\n  UPDATE_TYPES=1 cargo test -p outflow-server contract",
            path.display()
        );
    }
}
