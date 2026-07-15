// Typed HTTP client for the outflow server. Same wire shapes as before (serde
// snake_case fields, enum variant names, cents as bare numbers) — only the
// transport changed from Tauri IPC to fetch. Bodies use the Rust field names
// directly (snake_case): there is no camelCase translation layer anymore.
import type {
  Account,
  CategorizeResult,
  CategorySpend,
  Filter,
  LedgerView,
  Mark,
  MatchView,
  MerchantSpend,
  MonthlyFlow,
  PlaidItem,
  PullResult,
  Subscription,
  SyncEntry,
  SyncReport,
  Transaction,
  TxnFlag,
  TxnPage,
  TxnSearch,
  Window,
} from "./types";

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api${path}`, {
    method,
    headers: body !== undefined ? { "content-type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  if (!res.ok) {
    try {
      const parsed = JSON.parse(text) as { error?: string };
      throw new Error(parsed.error ?? `${res.status} ${res.statusText}`);
    } catch (e) {
      if (e instanceof Error && e.message !== text) throw e;
      throw new Error(`${res.status} ${res.statusText}`);
    }
  }
  return (text ? JSON.parse(text) : undefined) as T;
}

const get = <T>(path: string) => req<T>("GET", path);
const post = <T>(path: string, body?: unknown) => req<T>("POST", path, body);

// Filter → query params. Server defaults mirror the old to_filter guard:
// missing params mean "everything" (pending included, transfers excluded).
function filterQuery(filter?: Filter, extra?: Record<string, string>): string {
  const q = new URLSearchParams(extra);
  if (filter?.since !== undefined) q.set("since", String(filter.since));
  if (filter?.until !== undefined) q.set("until", String(filter.until));
  if (filter?.includePending !== undefined) q.set("pending", String(filter.includePending));
  if (filter?.includeNonSpending !== undefined)
    q.set("transfers", String(filter.includeNonSpending));
  const s = q.toString();
  return s ? `?${s}` : "";
}

export const api = {
  accounts: () => get<Account[]>("/accounts"),
  // Search/sort/filter over the whole archive; returns one page + totals.
  transactions: (s?: TxnSearch) => {
    const q = new URLSearchParams();
    const set = (k: string, v: unknown) => {
      if (v !== undefined && v !== "" && v !== null) q.set(k, String(v));
    };
    set("since", s?.since);
    set("until", s?.until);
    set("pending", s?.includePending);
    set("transfers", s?.includeNonSpending);
    set("q", s?.q);
    set("account", s?.account);
    set("category", s?.category);
    set("source", s?.source);
    set("flag", s?.flag);
    set("min_cents", s?.minCents);
    set("max_cents", s?.maxCents);
    set("sort", s?.sort);
    set("dir", s?.dir);
    set("offset", s?.offset);
    set("limit", s?.limit);
    const qs = q.toString();
    return get<TxnPage>(`/transactions${qs ? `?${qs}` : ""}`);
  },
  categorize: () => post<CategorizeResult>("/categorize"),
  spendCategories: (filter?: Filter) =>
    get<CategorySpend[]>(`/spend/categories${filterQuery(filter)}`),
  merchants: (filter?: Filter, limit?: number) =>
    get<MerchantSpend[]>(
      `/merchants${filterQuery(filter, limit !== undefined ? { limit: String(limit) } : undefined)}`,
    ),
  flow: (filter?: Filter) => get<MonthlyFlow[]>(`/flow${filterQuery(filter)}`),
  subscriptions: () => get<Subscription[]>("/subscriptions"),
  // Rhythm ledger — the primary screen query.
  ledger: (window: Window) => get<LedgerView>(`/ledger?window=${window}`),
  streamOccurrences: (merchant: string, window: Window) =>
    get<Transaction[]>(
      `/stream_occurrences?merchant=${encodeURIComponent(merchant)}&window=${window}`,
    ),
  markStream: (merchant: string, mark: Mark) => post<void>("/mark_stream", { merchant, mark }),
  clearStreamMark: (merchant: string) => post<void>("/clear_stream_mark", { merchant }),
  categories: () => get<string[]>("/categories"),
  setCategory: (txnId: string, category: string, learn: boolean) =>
    post<number | null>(`/txn/${encodeURIComponent(txnId)}/category`, { category, learn }),
  // flag is the serde variant name, e.g. "Transfer" | "CardPayment" | "Spending".
  setFlag: (txnId: string, flag: TxnFlag, learn: boolean) =>
    post<number | null>(`/txn/${encodeURIComponent(txnId)}/flag`, { flag, learn }),
  applyFlags: () => post<number>("/apply_flags"),
  hasCreditAccount: () => get<boolean>("/has_credit_account"),
  resetData: () => post<void>("/reset_data"),

  // Sync all linked Plaid items.
  pull: () => post<SyncReport>("/pull"),
  // Back-compat shape for callers that expect the old PullResult summary.
  pullLive: async (): Promise<PullResult> => {
    const r = await post<SyncReport>("/pull");
    return {
      added: r.legs.reduce((n, l) => n + l.added, 0),
      updated: r.legs.reduce((n, l) => n + l.updated, 0),
      accounts: r.legs.filter((l) => !l.error).length,
      warnings: r.legs.flatMap((l) => (l.error ? [`${l.source}: ${l.error}`] : [])),
    };
  },
  categorizeLlm: () => post<number>("/categorize_llm"),
  syncLog: (limit?: number) =>
    get<SyncEntry[]>(`/sync_log${limit !== undefined ? `?limit=${limit}` : ""}`),

  // Plaid item lifecycle. Linking runs in the browser via Plaid Link.
  plaidLinkToken: (itemId?: string) =>
    post<{ link_token: string }>("/plaid/link_token", { item_id: itemId ?? null }),
  plaidExchange: (publicToken: string, institution: string | null) =>
    post<{ item_id: string; institution: string; added: number; updated: number }>(
      "/plaid/exchange",
      { public_token: publicToken, institution },
    ),
  plaidItems: () => get<PlaidItem[]>("/plaid/items"),
  plaidRemoveItem: (itemId: string) =>
    req<void>("DELETE", `/plaid/items/${encodeURIComponent(itemId)}`),

  // Card-payment match review.
  matches: (status?: string) =>
    get<MatchView[]>(`/matches${status ? `?status=${status}` : ""}`),
  acceptMatch: (id: number) => post<void>(`/matches/${id}/accept`),
  rejectMatch: (id: number) => post<void>(`/matches/${id}/reject`),
};
