// Typed wrappers around the Tauri command layer. Tauri v2 converts snake_case
// Rust argument names to camelCase on the JS side (so `txn_id` -> `txnId`).
import { invoke } from "@tauri-apps/api/core";
import type {
  Account,
  CategorizeResult,
  CategorySpend,
  Filter,
  MerchantSpend,
  MonthlyFlow,
  PullResult,
  Subscription,
  Transaction,
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
  categories: () => invoke<string[]>("categories"),
  setCategory: (txnId: string, category: string, learn: boolean) =>
    invoke<number | null>("set_category", { txnId, category, learn }),
  pullFromFile: (path: string) => invoke<PullResult>("pull_from_file", { path }),

  // networked (require the `net` feature build)
  pullLive: () => invoke<PullResult>("pull_live"),
  claim: (setupToken: string) => invoke<string>("claim", { setupToken }),
  categorizeLlm: () => invoke<number>("categorize_llm"),
};
