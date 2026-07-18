// Outflows screen: month-by-month spending with a searchable, sortable,
// filterable transaction table under it. Clicking a month bar scopes the table
// to that month; the totals line always reflects the CURRENT query (all
// matches, not just the visible page). Card payments/transfers stay excluded
// unless the "show excluded" chip is on — that's the whole double-count story.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type {
  Account,
  MonthlyFlow,
  SortKey,
  Transaction,
  TxnFlag,
  TxnPage,
} from "../types";
import { accountSource, dollars } from "./ledger/labels";
import { SourceChip } from "./ledger/Streams";
import type { ToastMsg } from "./Toast";

const PAGE = 50;

function monthBounds(year: number, month: number): { since: number; until: number } {
  // Local-time month edges, matching the server's local-time bucketing.
  const since = new Date(year, month - 1, 1).getTime() / 1000;
  const until = new Date(year, month, 1).getTime() / 1000;
  return { since, until };
}

function monthLabel(year: number, month: number): string {
  return new Date(year, month - 1, 1).toLocaleDateString(undefined, {
    month: "short",
    year: "2-digit",
  });
}

function fmtDay(epoch: number): string {
  return new Date(epoch * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
  });
}

export function Outflows({
  accounts,
  vocab,
  notify,
}: {
  accounts: Account[];
  vocab: string[];
  notify: (msg: ToastMsg) => void;
}) {
  const [flow, setFlow] = useState<MonthlyFlow[]>([]);
  const [month, setMonth] = useState<{ year: number; month: number } | null>(null);
  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [account, setAccount] = useState("");
  const [category, setCategory] = useState("");
  const [showExcluded, setShowExcluded] = useState(false);
  const [sort, setSort] = useState<SortKey>("date");
  const [dir, setDir] = useState<"asc" | "desc">("desc");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<TxnPage | null>(null);
  const [editing, setEditing] = useState<string | null>(null);

  // Debounce the search box.
  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(search), 250);
    return () => window.clearTimeout(t);
  }, [search]);

  // Reset paging whenever the query changes shape.
  useEffect(() => {
    setOffset(0);
  }, [debounced, account, category, showExcluded, month, sort, dir]);

  const loadFlow = useCallback(async () => {
    try {
      const f = await api.flow();
      setFlow(f);
      // Default to the latest month with data.
      if (f.length > 0) {
        setMonth((cur) => cur ?? { year: f[f.length - 1].year, month: f[f.length - 1].month });
      }
    } catch (err) {
      notify({ kind: "err", text: `Load failed: ${String(err)}` });
    }
  }, [notify]);

  useEffect(() => {
    void loadFlow();
  }, [loadFlow]);

  const query = useMemo(() => {
    const bounds = month ? monthBounds(month.year, month.month) : {};
    return {
      ...bounds,
      q: debounced || undefined,
      account: account || undefined,
      category: category || undefined,
      includeNonSpending: showExcluded || undefined,
      sort,
      dir,
      offset,
      limit: PAGE,
    };
  }, [month, debounced, account, category, showExcluded, sort, dir, offset]);

  const seq = useRef(0);
  const loadPage = useCallback(async () => {
    const mine = ++seq.current;
    try {
      const p = await api.transactions(query);
      if (mine === seq.current) setPage(p);
    } catch (err) {
      notify({ kind: "err", text: `Search failed: ${String(err)}` });
    }
  }, [query, notify]);

  useEffect(() => {
    void loadPage();
  }, [loadPage]);

  const onSort = (k: SortKey) => {
    if (sort === k) setDir((d) => (d === "desc" ? "asc" : "desc"));
    else {
      setSort(k);
      setDir("desc");
    }
  };

  const setTxnCategory = async (t: Transaction, cat: string) => {
    try {
      await api.setCategory(t.id, cat, true);
      notify({ kind: "ok", text: `${t.payee ?? t.description} → ${cat} (rule learned)` });
      await loadPage();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    }
  };

  const setTxnFlag = async (t: Transaction, flag: TxnFlag) => {
    try {
      await api.setFlag(t.id, flag, false);
      notify({
        kind: "ok",
        text: flag === "Spending" ? "Counted as spending again." : `Excluded as ${flag}.`,
        // Excluding hides the row from the default view; offer a one-click way
        // back instead of making the user hunt for the "show excluded" chip.
        action:
          flag === "Spending"
            ? undefined
            : { label: "Undo", run: () => void setTxnFlag(t, "Spending") },
      });
      await Promise.all([loadPage(), loadFlow()]);
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    }
  };

  const acctFor = (id: string) => accounts.find((a) => a.id === id);
  const maxOut = Math.max(1, ...flow.map((m) => m.outflow_cents));
  const arrow = (k: SortKey) => (sort === k ? (dir === "desc" ? " ↓" : " ↑") : "");

  return (
    <div className="of-wrap">
      <div className="of-months">
        {flow.map((m) => {
          const active = month?.year === m.year && month?.month === m.month;
          return (
            <button
              key={`${m.year}-${m.month}`}
              className={`of-month${active ? " on" : ""}`}
              onClick={() => setMonth(active ? null : { year: m.year, month: m.month })}
              title={`${dollars(-m.outflow_cents)} out`}
            >
              <span
                className="of-bar"
                style={{ height: `${Math.round((m.outflow_cents / maxOut) * 48) + 2}px` }}
              />
              <span className="of-mlabel">{monthLabel(m.year, m.month)}</span>
              <span className="of-mout">{dollars(-m.outflow_cents)}</span>
            </button>
          );
        })}
      </div>

      <div className="of-controls">
        <input
          className="of-search"
          placeholder="Search merchants, descriptions, categories…"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
        />
        <select className="of-select" value={account} onChange={(e) => setAccount(e.target.value)}>
          <option value="">All accounts</option>
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name}
            </option>
          ))}
        </select>
        <select
          className="of-select"
          value={category}
          onChange={(e) => setCategory(e.target.value)}
        >
          <option value="">All categories</option>
          {vocab.map((c) => (
            <option key={c} value={c}>
              {c}
            </option>
          ))}
        </select>
        <label className="of-chip">
          <input
            type="checkbox"
            checked={showExcluded}
            onChange={(e) => setShowExcluded(e.target.checked)}
          />
          show excluded
        </label>
      </div>

      <div className="of-total">
        {page ? (
          <>
            <b>{page.total}</b> transactions · net <b>{dollars(page.total_cents)}</b>
            {month ? ` in ${monthLabel(month.year, month.month)}` : " all time"}
          </>
        ) : (
          "Loading…"
        )}
      </div>

      <table className="of-table">
        <thead>
          <tr>
            <th onClick={() => onSort("date")}>Date{arrow("date")}</th>
            <th onClick={() => onSort("merchant")}>Merchant{arrow("merchant")}</th>
            <th onClick={() => onSort("category")}>Category{arrow("category")}</th>
            <th>Account</th>
            <th className="num" onClick={() => onSort("amount")}>
              Amount{arrow("amount")}
            </th>
            <th />
          </tr>
        </thead>
        <tbody>
          {page?.items.map((t) => (
            <tr key={t.id} className={t.flag !== "Spending" ? "of-excluded" : ""}>
              <td className="of-date">{fmtDay(t.transacted_at ?? t.posted)}</td>
              <td className="of-merch">
                <span className="of-merch-name">{t.payee ?? t.description}</span>
                {t.pending ? <span className="of-pending">pending</span> : null}
              </td>
              <td>
                {editing === t.id ? (
                  <select
                    autoFocus
                    className="of-select"
                    value={t.category ?? ""}
                    onBlur={() => setEditing(null)}
                    onChange={(e) => {
                      setEditing(null);
                      if (e.target.value) void setTxnCategory(t, e.target.value);
                    }}
                  >
                    <option value="">—</option>
                    {vocab.map((c) => (
                      <option key={c} value={c}>
                        {c}
                      </option>
                    ))}
                  </select>
                ) : (
                  <span className="of-cat" onClick={() => setEditing(t.id)}>
                    {t.category ?? "—"}
                  </span>
                )}
              </td>
              <td className="of-acct">
                {(() => {
                  const a = acctFor(t.account_id);
                  return a ? <SourceChip s={accountSource(a)} /> : t.account_id;
                })()}
              </td>
              <td className={`num ${t.amount < 0 ? "of-out" : "of-in"}`}>{dollars(t.amount)}</td>
              <td className="of-rowact">
                {t.flag === "Spending" ? (
                  <button
                    className="of-mini"
                    title="Exclude (own-account transfer / card payment)"
                    onClick={() => void setTxnFlag(t, "Transfer")}
                  >
                    exclude
                  </button>
                ) : (
                  <button
                    className="of-mini"
                    title={`Currently ${t.flag} — count as spending again`}
                    onClick={() => void setTxnFlag(t, "Spending")}
                  >
                    {t.flag === "CardPayment" ? "card pmt" : "transfer"} ✕
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      {page && page.total > PAGE && (
        <div className="of-pager">
          <button
            className="lg-pill ghost"
            disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - PAGE))}
          >
            ← Prev
          </button>
          <span>
            {offset + 1}–{Math.min(offset + PAGE, page.total)} of {page.total}
          </span>
          <button
            className="lg-pill ghost"
            disabled={offset + PAGE >= page.total}
            onClick={() => setOffset(offset + PAGE)}
          >
            Next →
          </button>
        </div>
      )}
    </div>
  );
}
