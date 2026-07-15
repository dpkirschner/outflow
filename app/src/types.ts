// TS mirrors of the outflow-core DTOs. serde serializes struct fields as
// snake_case and enums as their variant names. Every money value is integer
// cents (i64); outflows are negative.

export type AccountKind = "Checking" | "Savings" | "Credit" | "Other";
export type CategorySource = "SimpleFin" | "Plaid" | "Rule" | "Llm" | "Manual";
export type Cadence = "Monthly" | "Yearly";
// Suppression flag: how spend analytics treat a transaction. Non-Spending rows
// (money moved between the user's own accounts) are hidden from the charts.
export type TxnFlag = "Spending" | "Transfer" | "CardPayment";
export type Trend = "Rising" | "Falling" | "Steady";
// Rhythm cadence buckets (StreamCadence in core). UI maps these to badge labels.
export type StreamCadence =
  | "Daily"
  | "FewPerWeek"
  | "Weekly"
  | "FewPerMonth"
  | "Monthly"
  | "Yearly";
export type SourceKind = "Card" | "Ach";
export type Mark = "Committed" | "Dismissed" | "Kept";
export type Window = "3mo" | "6mo" | "12mo" | "all";

export interface Account {
  id: string;
  org: string;
  name: string;
  kind: AccountKind;
  balance: number; // cents
  currency: string;
  last_synced: number; // epoch seconds
  source: string; // "simplefin" | "plaid"
}

export interface Transaction {
  id: string;
  account_id: string;
  posted: number; // epoch seconds
  transacted_at: number | null; // epoch seconds; when the purchase happened
  amount: number; // cents, outflow negative
  description: string;
  payee: string | null;
  category: string | null;
  category_source: CategorySource | null;
  pending: boolean;
  flag: TxnFlag;
  raw: string;
  source: string; // "simplefin" | "plaid"
}

export interface CategorySpend {
  category: string | null;
  total_cents: number;
  count: number;
}

export interface MerchantSpend {
  merchant: string;
  total_cents: number;
  count: number;
}

export interface MonthlyFlow {
  year: number;
  month: number;
  inflow_cents: number;
  outflow_cents: number;
  net_cents: number;
}

export interface Subscription {
  payee: string;
  cadence: Cadence;
  typical_amount_cents: number;
  occurrences: number;
  first_seen: number;
  last_seen: number;
  total_cents: number;
}

// A source chip: which account a stream's charges land on.
export interface Source {
  label: string; // last-4 (cards) or account name
  kind: SourceKind;
  pct: number; // share of the stream's occurrences on this account
}

// A recurring stream row (core `Stream` = flattened `RhythmEntry` + sources).
export interface Stream {
  merchant: string;
  cadence: StreamCadence;
  occurrence_count: number;
  median_amount_cents: number;
  amount_min_cents: number;
  amount_max_cents: number;
  monthly_estimate_cents: number;
  last_seen: number; // epoch seconds
  trend_pct: number; // signed pct vs prior 3-mo avg
  trend: Trend;
  spark_cents: number[]; // per-month spend, oldest→newest; last = in-progress month
  sources: Source[];
  category: string | null;
}

// A single transaction row — Notable one-offs and the expandable Noise list.
export interface LineItem {
  id: string;
  merchant: string;
  date: number; // epoch seconds
  amount_cents: number;
  category: string | null;
  source: Source;
}

export interface TransferGroup {
  merchant: string;
  flag: TxnFlag;
  total_cents: number;
  count: number;
}

export interface LedgerStats {
  recurring_monthly_cents: number;
  stream_count: number;
  committed_monthly_cents: number;
  noise_total_cents: number;
  noise_count: number;
}

// How much data sits behind the view — to distinguish "window is at 12mo" from
// "you only have 3 months of history."
export interface Coverage {
  earliest: number | null; // earliest behavioral date in the whole DB
  window_txn_count: number; // outflow txns within the window
  covers_all: boolean; // window reaches all history (not clipping)
}

export interface LedgerView {
  coverage: Coverage;
  stats: LedgerStats;
  streams: Stream[];
  committed: Stream[];
  notable: LineItem[];
  noise_items: LineItem[];
  transfers: TransferGroup[];
}

export interface CategorizeResult {
  rule: number;
  remaining: number;
}

export interface PullResult {
  added: number;
  updated: number;
  accounts: number;
  warnings: string[];
}

export interface Filter {
  since?: number; // epoch seconds, inclusive
  until?: number; // epoch seconds, exclusive
  includePending?: boolean;
  includeNonSpending?: boolean; // charts hide transfers/card payments by default
}

// One source's outcome within a sync run.
export interface LegReport {
  source: string; // "simplefin" | "plaid:<institution>"
  added: number;
  updated: number;
  error: string | null;
}

export interface SyncReport {
  legs: LegReport[];
  flags_applied: number;
  categorized: number;
}

export interface SyncEntry {
  id: number;
  started: number;
  finished: number | null;
  source: string;
  added: number | null;
  updated: number | null;
  note: string | null;
}

// A linked Plaid connection (non-secret metadata only).
export interface PlaidItem {
  item_id: string;
  institution: string;
  cursor: string | null;
  status: string; // "ok" | "login_required" | "error"
  created: number;
  last_synced: number | null;
}

export type MatchStatus = "Proposed" | "Accepted" | "Rejected";
export type MatchConfidence = "High" | "Medium";

// A detected checking→card payment pair awaiting/holding a decision.
export interface TxnMatch {
  id: number;
  bank_txn_id: string;
  card_txn_id: string;
  status: MatchStatus;
  confidence: MatchConfidence;
  reason: string | null;
  created: number;
}

// Match enriched with both legs for display (server flattens TxnMatch fields).
export interface MatchView extends TxnMatch {
  bank: Transaction | null;
  card: Transaction | null;
}
