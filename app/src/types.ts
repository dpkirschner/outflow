// TS mirrors of the outflow-core DTOs. serde serializes struct fields as
// snake_case and enums as their variant names. Every money value is integer
// cents (i64); outflows are negative.

export type AccountKind = "Checking" | "Savings" | "Credit" | "Other";
export type CategorySource = "SimpleFin" | "Rule" | "Llm" | "Manual";
export type Cadence = "Monthly" | "Yearly";
// Suppression flag: how spend analytics treat a transaction. Non-Spending rows
// (money moved between the user's own accounts) are hidden from the charts.
export type TxnFlag = "Spending" | "Transfer" | "CardPayment";
export type Trend = "Rising" | "Falling" | "Steady";

export interface Account {
  id: string;
  org: string;
  name: string;
  kind: AccountKind;
  balance: number; // cents
  currency: string;
  last_synced: number; // epoch seconds
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

// A recurring merchant whose amount varies (the rhythm roster row).
export interface RhythmEntry {
  merchant: string;
  cadence: Cadence;
  occurrence_count: number;
  median_amount_cents: number;
  amount_min_cents: number;
  amount_max_cents: number;
  monthly_estimate_cents: number;
  last_seen: number; // epoch seconds
  trend: Trend;
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
