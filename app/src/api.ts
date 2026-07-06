// Typed wrappers around the Tauri command layer. Tauri v2 converts snake_case
// Rust argument names to camelCase on the JS side (so `txn_id` -> `txnId`).
import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  CategorizeResult,
  CategorySpend,
  Filter,
  LedgerView,
  Mark,
  MerchantSpend,
  MonthlyFlow,
  PullResult,
  Subscription,
  Transaction,
  TxnFlag,
  Window,
} from "./types";

export const api = {
  accounts: () => invoke<Account[]>("accounts"),
  transactions: (filter?: Filter) => invoke<Transaction[]>("transactions", { filter }),
  categorize: () => invoke<CategorizeResult>("categorize"),
  spendCategories: (filter?: Filter) =>
    invoke<CategorySpend[]>("spend_categories", { filter }),
  merchants: (filter?: Filter, limit?: number) =>
    invoke<MerchantSpend[]>("merchants", { filter, limit }),
  flow: (filter?: Filter) => invoke<MonthlyFlow[]>("flow", { filter }),
  subscriptions: () => invoke<Subscription[]>("subscriptions"),
  // Rhythm ledger — the primary screen query.
  ledger: (window: Window) => invoke<LedgerView>("ledger", { window }),
  streamOccurrences: (merchant: string, window: Window) =>
    invoke<Transaction[]>("stream_occurrences", { merchant, window }),
  markStream: (merchant: string, mark: Mark) =>
    invoke<void>("mark_stream", { merchant, mark }),
  clearStreamMark: (merchant: string) =>
    invoke<void>("clear_stream_mark", { merchant }),
  categories: () => invoke<string[]>("categories"),
  setCategory: (txnId: string, category: string, learn: boolean) =>
    invoke<number | null>("set_category", { txnId, category, learn }),
  // flag is the serde variant name, e.g. "Transfer" | "CardPayment" | "Spending".
  setFlag: (txnId: string, flag: TxnFlag, learn: boolean) =>
    invoke<number | null>("set_flag", { txnId, flag, learn }),
  applyFlags: () => invoke<number>("apply_flags"),
  hasCreditAccount: () => invoke<boolean>("has_credit_account"),
  pullFromFile: (path: string) => invoke<PullResult>("pull_from_file", { path }),

  resetData: () => invoke<void>("reset_data"),

  // networked (require the `net` feature build)
  pullLive: () => invoke<PullResult>("pull_live"),
  claim: (setupToken: string) => invoke<string>("claim", { setupToken }),
  categorizeLlm: () => invoke<number>("categorize_llm"),
};
