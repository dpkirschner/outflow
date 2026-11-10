// Typed HTTP client for the outflow server. Same wire shapes as before (serde
// snake_case fields, enum variant names, cents as bare numbers) — only the
// transport changed from Tauri IPC to fetch. Bodies use the Rust field names
// directly (snake_case): there is no camelCase translation layer anymore.
import type {
  Account,
  CategoriesVsUsual,
  CheckIn,
  InSource,
  BalancesView,
  DayTotals,
  ExchangeResult,
  CategorizeResult,
  CategorySpend,
  MatchView,
  MerchantSpend,
  FlowMonth,
  NetWorthChange,
  PositionChange,
  PlaidItem,
  RecurringView,
  SyncEntry,
  SyncReport,
  TxnFlag,
  Lane,
  Direction,
  SeriesMonth,
  TxnPage,
  Upkeep,
  ResetPreview,
  CategoryEdit,
  CategoryUndo,
  GroupEdit,
  GroupUndo,
  AccountKind,
} from "./types";

// Request-side shapes. types.ts is generated from the Rust response DTOs;
// these are browser-side query parameters and client-built results, so they
// live here, next to the calls that use them.

export type SortKey = "date" | "amount" | "merchant" | "category";

/** The flags a moved row can be grouped under (Transfer = between own accounts). */
export type MovedGroup = Extract<TxnFlag, "Invest" | "Save" | "LoanPaydown" | "Transfer">;

/** A Stock range: a period spelling (`2026-11`, `this-year`), or a first day
 *  (`YYYY-MM-DD`, through today), or neither for all of history. */
export interface StockRange {
  period?: string;
  since?: string;
}

function rangeQuery(r: StockRange, extra: Record<string, string> = {}): string {
  const q = new URLSearchParams(extra);
  if (r.period) q.set("period", r.period);
  if (r.since) q.set("since", r.since);
  const s = q.toString();
  return s ? `?${s}` : "";
}

export interface Filter {
  period?: string; // e.g. "2026-11"; resolved in server-local days
  since?: number; // epoch seconds, inclusive
  until?: number; // epoch seconds, exclusive
  includePending?: boolean;
  includeNonSpending?: boolean; // charts hide transfers/card payments by default
}

// Search/sort/filter over the transaction list (GET /api/transactions).
export interface TxnSearch {
  since?: number | string; // epoch seconds, or YYYY-MM-DD (inclusive)
  until?: number | string; // epoch seconds (exclusive), or YYYY-MM-DD (inclusive)
  includePending?: boolean;
  includeNonSpending?: boolean;
  q?: string;
  account?: string;
  category?: string;
  source?: string;
  flag?: TxnFlag;
  period?: string; // e.g. "2026-11"; resolved in server-local days
  lane?: Lane; // exactly the rows behind that Flow figure
  merchant?: string; // normalized merchant key, e.g. a recurring merchant's
  uncategorized?: boolean;
  direction?: Direction; // card payment: out = bank side, in = card side
  minCents?: number;
  maxCents?: number;
  sort?: SortKey;
  dir?: "asc" | "desc";
  offset?: number;
  limit?: number;
}

// pullLive's summary, folded from the SyncReport that POST /pull returns.
export interface PullResult {
  added: number;
  updated: number;
  accounts: number;
  warnings: string[];
}

// A Plaid Link error as forwarded to /api/plaid/link_event (see
// server/src/plaid_routes.rs `LinkEventBody`). Snake_case, all optional.
export interface LinkErrorReport {
  stage: "event" | "exit";
  error_type?: string | null;
  error_code?: string | null;
  error_message?: string | null;
  display_message?: string | null;
  institution?: string | null;
  institution_id?: string | null;
  link_session_id?: string | null;
  request_id?: string | null;
}

const TOKEN_KEY = "outflow_token";

// The server's OUTFLOW_API_TOKEN, typed in once via AuthGate and kept in
// localStorage so it survives reloads. Deliberately not a build-time constant:
// /assets is served without auth, so a token baked into the bundle would be
// readable by anything that can reach the port — which includes any agent or
// container on the box, i.e. exactly what the token is meant to keep out.
export const token = {
  get: () => localStorage.getItem(TOKEN_KEY) ?? "",
  set: (t: string) => localStorage.setItem(TOKEN_KEY, t),
  clear: () => localStorage.removeItem(TOKEN_KEY),
};

// The app never asks the server whether auth is on — it just calls, and treats
// a 401 as "prompt for a token". So a server with no token configured (the dev
// default) never shows a gate, and a rotated token re-prompts on the next call.
let onUnauthorized: (() => void) | null = null;
export function setUnauthorizedHandler(fn: () => void) {
  onUnauthorized = fn;
}

async function req<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  if (body !== undefined) headers["content-type"] = "application/json";
  const t = token.get();
  if (t) headers.authorization = `Bearer ${t}`;

  const res = await fetch(`/api${path}`, {
    method,
    headers,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });

  // Absent, wrong, or rotated token: drop it so the gate asks again rather than
  // retrying a dead credential on every call.
  if (res.status === 401) {
    token.clear();
    onUnauthorized?.();
    throw new Error("Unauthorized — enter the API token.");
  }
  // A read-only token reaching a mutation. Distinct from 401: the token is
  // real, the action isn't allowed.
  if (res.status === 403) {
    throw new Error("This token is read-only — that action needs the full token.");
  }
  const text = await res.text();
  if (!res.ok) {
    // The server's JSON `error` when there is one; a proxy's HTML or empty
    // body (a 502) falls back to the status line, never a SyntaxError.
    let message: string | undefined;
    try {
      message = (JSON.parse(text) as { error?: string }).error;
    } catch {
      /* not JSON */
    }
    throw new Error(message || `${res.status} ${res.statusText}`.trim());
  }
  return (text ? JSON.parse(text) : undefined) as T;
}

const get = <T>(path: string) => req<T>("GET", path);
const post = <T>(path: string, body?: unknown) => req<T>("POST", path, body);

// The /transactions query string; /flow/series reads the same one.
function txnQuery(s?: TxnSearch): string {
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
  set("period", s?.period);
  set("lane", s?.lane);
  set("merchant", s?.merchant);
  set("uncategorized", s?.uncategorized || undefined);
  set("direction", s?.direction);
  set("min_cents", s?.minCents);
  set("max_cents", s?.maxCents);
  set("sort", s?.sort);
  set("dir", s?.dir);
  set("offset", s?.offset);
  set("limit", s?.limit);
  const qs = q.toString();
  return qs ? `?${qs}` : "";
}

// Filter → query params. Server defaults mirror the old to_filter guard:
// missing params mean "everything" (pending included, transfers excluded).
function filterQuery(filter?: Filter, extra?: Record<string, string>): string {
  const q = new URLSearchParams(extra);
  if (filter?.period !== undefined) q.set("period", filter.period);
  if (filter?.since !== undefined) q.set("since", String(filter.since));
  if (filter?.until !== undefined) q.set("until", String(filter.until));
  if (filter?.includePending !== undefined) q.set("pending", String(filter.includePending));
  if (filter?.includeNonSpending !== undefined)
    q.set("transfers", String(filter.includeNonSpending));
  const s = q.toString();
  return s ? `?${s}` : "";
}

export const api = {
  // Visible accounts; `includeHidden` adds hidden ones (Connections only).
  accounts: (includeHidden = false) =>
    get<Account[]>(includeHidden ? "/accounts?include_hidden=true" : "/accounts"),
  // Current state of every visible account + holdings; and its daily trend.
  balances: () => get<BalancesView>("/balances"),
  balanceHistory: (since?: number) =>
    get<DayTotals[]>(since ? `/balances/history?since=${since}` : "/balances/history"),
  // Net worth change over a range, split Markets + Added (core::stock).
  netWorthChange: (r: StockRange) => get<NetWorthChange>(`/net-worth/change${rangeQuery(r)}`),
  // Gain over a range per position; `position` combines a ticker across accounts.
  holdingsChange: (r: StockRange, by: "account" | "position") =>
    get<PositionChange[]>(`/holdings/change${rangeQuery(r, { by })}`),
  // Search/sort/filter over the whole archive; returns one page + totals.
  transactions: (s?: TxnSearch) => get<TxnPage>(`/transactions${txnQuery(s)}`),
  // The rows `transactions` would list for the same search, summed per month.
  flowSeries: (s?: TxnSearch) => get<SeriesMonth[]>(`/flow/series${txnQuery(s)}`),
  categorize: () => post<CategorizeResult>("/categorize"),
  inSources: (filter?: Filter) => get<InSource[]>(`/flow/in-sources${filterQuery(filter)}`),
  spendCategories: (filter?: Filter) =>
    get<CategorySpend[]>(`/spend/categories${filterQuery(filter)}`),
  merchants: (filter?: Filter, limit?: number) =>
    get<MerchantSpend[]>(
      `/merchants${filterQuery(filter, limit !== undefined ? { limit: String(limit) } : undefined)}`,
    ),
  flow: (filter?: Filter) => get<FlowMonth[]>(`/flow${filterQuery(filter)}`),
  // The recurring list over the 12 months ending at `month` (YYYY-MM).
  recurring: (month: string) =>
    get<RecurringView>(`/recurring?period=${encodeURIComponent(month)}`),
  setRecurringIgnored: (merchant: string, ignored: boolean) =>
    post<void>("/recurring/ignored", { merchant, ignored }),
  categories: () => get<string[]>("/categories"),
  setCategory: (txnId: string, category: string, learn: boolean) =>
    post<CategoryEdit>(`/txn/${encodeURIComponent(txnId)}/category`, { category, learn }),
  // Reverse one category edit (and the rule it learned) with the undo it
  // returned. 409 = something changed since; nothing was touched.
  undoCategory: (txnId: string, undo: CategoryUndo) =>
    post<void>(`/txn/${encodeURIComponent(txnId)}/category/undo`, undo),
  // flag is the serde variant name, e.g. "Transfer" | "CardPayment" | "Spending".
  setFlag: (txnId: string, flag: TxnFlag, learn: boolean) =>
    post<number | null>(`/txn/${encodeURIComponent(txnId)}/flag`, { flag, learn }),
  // Put a moved row in a Moved group and teach its merchant; returns how many
  // rows changed and the undo token.
  setGroup: (txnId: string, flag: MovedGroup) =>
    post<GroupEdit>(`/txn/${encodeURIComponent(txnId)}/group`, { flag }),
  undoGroup: (txnId: string, undo: GroupUndo) =>
    post<number>(`/txn/${encodeURIComponent(txnId)}/group/undo`, undo),
  // An account's type over the bank's; null = the bank's again.
  setAccountKind: (accountId: string, kind: AccountKind | null) =>
    post<void>(`/account/${encodeURIComponent(accountId)}/kind`, { kind }),
  // Rename an account. Empty string clears the nickname (chip reverts to name).
  setAccountNickname: (accountId: string, nickname: string) =>
    post<void>(`/account/${encodeURIComponent(accountId)}/nickname`, { nickname }),
  // Hide/un-hide an account everywhere (lists, dropdowns, balances, analytics).
  setAccountHidden: (accountId: string, hidden: boolean) =>
    post<void>(`/account/${encodeURIComponent(accountId)}/hidden`, { hidden }),
  applyFlags: () => post<number>("/apply_flags"),
  hasCreditAccount: () => get<boolean>("/has_credit_account"),
  resetData: () => post<void>("/reset_data"),
  resetPreview: () => get<ResetPreview>("/reset/preview"),
  // What needs the owner: connections not ok, card payments to review, last sync.
  upkeep: () => get<Upkeep>("/upkeep"),
  // One week, month or quarter (`2026-W45`, `2026-11`, `2026-Q4`,
  // `last-month`...): net worth change, earned − spent, saved, usuals, unusual.
  checkin: (period: string) => get<CheckIn>(`/checkin?period=${encodeURIComponent(period)}`),
  checkinCategories: (period: string) =>
    get<CategoriesVsUsual>(`/checkin/categories?period=${encodeURIComponent(period)}`),

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
  plaidLinkToken: (itemId?: string, consentInvestments = false, investmentsOnly = false) =>
    post<{ link_token: string }>("/plaid/link_token", {
      item_id: itemId ?? null,
      consent_investments: consentInvestments,
      investments_only: investmentsOnly,
    }),
  plaidExchange: (publicToken: string, institution: string | null) =>
    post<ExchangeResult>(
      "/plaid/exchange",
      { public_token: publicToken, institution },
    ),
  // Forward a Plaid Link error to the server log (Link talks to Plaid
  // directly, so the server never sees these otherwise).
  plaidLinkEvent: (e: LinkErrorReport) => post<void>("/plaid/link_event", e),
  plaidItems: () => get<PlaidItem[]>("/plaid/items"),
  plaidRemoveItem: (itemId: string) =>
    req<void>("DELETE", `/plaid/items/${encodeURIComponent(itemId)}`),

  // Card-payment match review.
  matches: (status?: string) =>
    get<MatchView[]>(`/matches${status ? `?status=${status}` : ""}`),
  acceptMatch: (id: number) => post<void>(`/matches/${id}/accept`),
  rejectMatch: (id: number) => post<void>(`/matches/${id}/reject`),
  // An accepted pair goes back to proposed (its legs count as spending again).
  unacceptMatch: (id: number) => post<void>(`/matches/${id}/unaccept`),
};
