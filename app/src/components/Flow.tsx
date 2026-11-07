// Flow screen: one month of money In, Out and Moved (GLOSSARY: Flow lanes),
// with the month's transactions under it. Every figure comes from core
// (GET /api/flow, /api/spend/categories); the browser only formats them.
// The month is App state, sent as `period=YYYY-MM` so core decides the
// month's edges in server-local time.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type SortKey } from "../api";
import type {
  Account,
  CategorySpend,
  FlowMonth,
  MovedFlow,
  Transaction,
  TxnFlag,
  TxnPage,
} from "../types";
import { formatCentsShort, monthLabel, monthLabelShort, signedCents, unsignedCents } from "../format";
import { onLinkClick, pathOf } from "../route";
import { accountDisplayLabel, accountSource } from "./ledger/labels";
import { SourceChip } from "./ledger/Streams";
import type { ToastMsg } from "./Toast";

const PAGE = 50;
const CATEGORY_PREVIEW = 6;

export function monthKey(year: number, month: number): string {
  return `${year}-${String(month).padStart(2, "0")}`;
}

// A label only (selects no rows): the month that contains today's date.
function isCurrentMonth(m: FlowMonth): boolean {
  const now = new Date();
  return m.year === now.getFullYear() && m.month === now.getMonth() + 1;
}

function fmtDay(epoch: number): string {
  return new Date(epoch * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

const MOVED_LABEL: Record<Exclude<TxnFlag, "Spending">, string> = {
  CardPayment: "Card payment",
  Invest: "Invest",
  Save: "Save",
  LoanPaydown: "Loan paydown",
  Transfer: "Between own accounts",
};

type Load<T> = { state: "loading" } | { state: "error"; error: string } | { state: "ok"; data: T };

export function Flow({
  accounts,
  vocab,
  notify,
  month,
  onMonth,
}: {
  accounts: Account[];
  vocab: string[];
  notify: (msg: ToastMsg) => void;
  /** Selected month, `YYYY-MM`; null until the first load picks the latest. */
  month: string | null;
  onMonth: (m: string) => void;
}) {
  const [flow, setFlow] = useState<Load<FlowMonth[]>>({ state: "loading" });
  const [cats, setCats] = useState<CategorySpend[] | null>(null);
  const [allCats, setAllCats] = useState(false);

  const loadFlow = useCallback(async () => {
    try {
      setFlow({ state: "ok", data: await api.flow() });
    } catch (err) {
      setFlow({ state: "error", error: String(err) });
    }
  }, []);

  useEffect(() => {
    void loadFlow();
  }, [loadFlow]);

  const months = flow.state === "ok" ? flow.data : [];
  const selected = months.find((m) => monthKey(m.year, m.month) === month) ?? null;

  // Default (and fallback, if the remembered month has no data) = latest month.
  useEffect(() => {
    if (months.length === 0 || selected) return;
    const last = months[months.length - 1];
    onMonth(monthKey(last.year, last.month));
  }, [months, selected, onMonth]);

  const loadCats = useCallback(async () => {
    if (!month) return;
    try {
      setCats(await api.spendCategories({ period: month }));
    } catch (err) {
      setCats(null);
      notify({ kind: "err", text: `Couldn't load categories: ${String(err)}` });
    }
  }, [month, notify]);

  useEffect(() => {
    setCats(null);
    void loadCats();
  }, [loadCats]);

  const refreshFigures = useCallback(async () => {
    await Promise.all([loadFlow(), loadCats()]);
  }, [loadFlow, loadCats]);

  if (flow.state === "loading") {
    return (
      <main className="fl-wrap" aria-busy="true">
        <h1 className="fl-sr">Flow</h1>
        <p className="fl-state">Loading flow…</p>
      </main>
    );
  }
  if (flow.state === "error") {
    return (
      <main className="fl-wrap">
        <h1 className="fl-sr">Flow</h1>
        <div className="fl-state" role="alert">
          <p>Couldn't load the flow figures: {flow.error}</p>
          <button className="lg-pill" onClick={() => void loadFlow()}>
            Try again
          </button>
        </div>
      </main>
    );
  }
  if (months.length === 0) {
    return (
      <main className="fl-wrap">
        <h1 className="fl-sr">Flow</h1>
        <div className="fl-state">
          <p>No transactions yet.</p>
          <p className="fl-state-hint">
            Link a bank in{" "}
            <a href={pathOf("settings")} onClick={(e) => onLinkClick(e, pathOf("settings"))}>
              Settings → Connections
            </a>{" "}
            and the first sync fills this in.
          </p>
        </div>
      </main>
    );
  }

  return (
    <main className="fl-wrap" aria-labelledby="fl-title">
      <MonthStrip months={months} selected={month} onSelect={onMonth} />

      {selected && (
        <>
          <h1 id="fl-title" className="fl-title">
            {monthLabel(selected.year, selected.month)}
            {isCurrentMonth(selected) && <span className="fl-sofar">so far</span>}
          </h1>

          <Headline m={selected} />

          <div className="fl-lanes">
            <InLane m={selected} />
            <OutLane m={selected} cats={cats} all={allCats} onToggleAll={() => setAllCats((a) => !a)} />
            <MovedLaneCard m={selected} />
          </div>
        </>
      )}

      {month && (
        <TxnTable
          key={month}
          period={month}
          periodLabel={selected ? monthLabel(selected.year, selected.month) : month}
          accounts={accounts}
          vocab={vocab}
          notify={notify}
          onFiguresChanged={refreshFigures}
        />
      )}
    </main>
  );
}

// ── Month strip: Out per month, one bar each, latest on the right ───────────
function MonthStrip({
  months,
  selected,
  onSelect,
}: {
  months: FlowMonth[];
  selected: string | null;
  onSelect: (m: string) => void;
}) {
  const scroller = useRef<HTMLOListElement>(null);
  // Start scrolled to the latest months.
  useEffect(() => {
    const el = scroller.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [months.length]);

  const maxOut = Math.max(1, ...months.map((m) => m.out_cents));
  return (
    <nav className="fl-strip" aria-label="Month">
      <p className="fl-strip-key" id="fl-strip-key">
        Out per month
      </p>
      <ol ref={scroller} aria-describedby="fl-strip-key">
        {months.map((m) => {
          const key = monthKey(m.year, m.month);
          const on = key === selected;
          const sofar = isCurrentMonth(m);
          const h = Math.max(2, Math.round((Math.max(0, m.out_cents) / maxOut) * 48));
          return (
            <li key={key}>
              <button
                className={`fl-month${on ? " on" : ""}`}
                aria-pressed={on}
                aria-label={`${monthLabel(m.year, m.month)}${sofar ? " so far" : ""}: ${signedCents(-m.out_cents)} out`}
                onClick={() => onSelect(key)}
              >
                <span className="fl-bar-track" aria-hidden="true">
                  <span className="fl-bar" style={{ height: `${h}px` }} />
                </span>
                <span className="fl-mlabel" aria-hidden="true">
                  {monthLabelShort(m.year, m.month)}
                </span>
                <span className="fl-mout" aria-hidden="true">
                  {sofar ? "so far" : formatCentsShort(-m.out_cents).replace("-", "−")}
                </span>
              </button>
            </li>
          );
        })}
      </ol>
    </nav>
  );
}

// ── Headline: Earned − spent next to Saved, the gap in words ────────────────
function Headline({ m }: { m: FlowMonth }) {
  const gap = m.gap_cents;
  const gapText =
    gap > 0 ? (
      <>
        <b>{unsignedCents(gap)}</b> of what came in is cash left sitting in checking.
      </>
    ) : gap < 0 ? (
      <>
        <b>{unsignedCents(-gap)}</b> more went to savings and spending than came in, drawn
        from cash.
      </>
    ) : (
      <>Everything earned beyond spending went to savings.</>
    );
  return (
    <section className="fl-head" aria-label="Summary">
      <dl className="fl-pair">
        <div>
          <dt>Earned − spent</dt>
          <dd>{signedCents(m.earned_minus_spent_cents)}</dd>
        </div>
        <div>
          <dt>Saved</dt>
          <dd>{unsignedCents(m.saved_cents, true)}</dd>
        </div>
      </dl>
      <p className="fl-gap">{gapText}</p>
    </section>
  );
}

// ── Lanes ───────────────────────────────────────────────────────────────────
function Lane({
  id,
  title,
  figure,
  note,
  children,
}: {
  id: string;
  title: string;
  figure?: string;
  note: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="fl-lane" aria-labelledby={`fl-lane-${id}`}>
      <h2 id={`fl-lane-${id}`} className="fl-lane-title">
        {title}
      </h2>
      {figure !== undefined && <p className="fl-lane-figure">{figure}</p>}
      <p className="fl-lane-note">{note}</p>
      {children}
    </section>
  );
}

function Row({ label, value, quiet }: { label: React.ReactNode; value: React.ReactNode; quiet?: boolean }) {
  return (
    <div className={`fl-row${quiet ? " quiet" : ""}`}>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}

function InLane({ m }: { m: FlowMonth }) {
  return (
    <Lane id="in" title="In" figure={signedCents(m.in_cents)} note="Income from outside your own accounts.">
      <dl className="fl-rows">
        <Row
          label="Not yet categorized"
          value={signedCents(m.in_uncategorized_cents)}
          quiet={m.in_uncategorized_cents === 0}
        />
      </dl>
    </Lane>
  );
}

function OutLane({
  m,
  cats,
  all,
  onToggleAll,
}: {
  m: FlowMonth;
  cats: CategorySpend[] | null;
  all: boolean;
  onToggleAll: () => void;
}) {
  const shown = cats ? (all ? cats : cats.slice(0, CATEGORY_PREVIEW)) : [];
  const hidden = cats ? cats.length - CATEGORY_PREVIEW : 0;
  return (
    <Lane
      id="out"
      title="Out"
      figure={signedCents(-m.out_cents)}
      note={
        m.refunds_cents > 0
          ? `Spending, after ${unsignedCents(m.refunds_cents)} of refunds.`
          : "Spending. No refunds this month."
      }
    >
      {cats === null ? (
        <p className="fl-lane-note">Loading categories…</p>
      ) : cats.length === 0 ? (
        <p className="fl-lane-note">No spending this month.</p>
      ) : (
        <>
          <dl className="fl-rows" aria-label="Out by category">
            {shown.map((c) => (
              <Row
                key={c.category ?? "\u0000"}
                label={c.category ?? <i>Not yet categorized</i>}
                value={signedCents(-c.total_cents)}
              />
            ))}
          </dl>
          {hidden > 0 && (
            <button className="fl-more" aria-expanded={all} onClick={onToggleAll}>
              {all ? "Show fewer" : `Show all ${cats.length} categories`}
            </button>
          )}
        </>
      )}
    </Lane>
  );
}

function movedValue(f: MovedFlow): { text: string; quiet: boolean } {
  if (f.out_cents === 0 && f.in_cents === 0) return { text: "$0.00", quiet: true };
  if (f.in_cents === 0) return { text: signedCents(-f.out_cents), quiet: false };
  if (f.out_cents === 0) return { text: `${signedCents(f.in_cents)} back`, quiet: false };
  return { text: `${signedCents(-f.out_cents)}, ${signedCents(f.in_cents)} back`, quiet: false };
}

function MovedLaneCard({ m }: { m: FlowMonth }) {
  const mv = m.moved;
  const groups: [string, MovedFlow][] = [
    ["Invest", mv.invest],
    ["Save", mv.save],
    ["Loan paydown", mv.loan_paydown],
    ["Between own accounts", mv.between],
  ];
  return (
    <Lane id="moved" title="Moved" note="Between your own accounts. Not spending, not income.">
      <dl className="fl-rows">
        <Row
          label={
            <>
              Card payment
              <span className="fl-sub">counted once, the larger side</span>
            </>
          }
          value={mv.card_payment_cents === 0 ? "$0.00" : signedCents(-mv.card_payment_cents)}
          quiet={mv.card_payment_cents === 0}
        />
        {groups.map(([label, f]) => {
          const v = movedValue(f);
          return <Row key={label} label={label} value={v.text} quiet={v.quiet} />;
        })}
      </dl>
    </Lane>
  );
}

// ── Transactions for the month ──────────────────────────────────────────────
function TxnTable({
  period,
  periodLabel,
  accounts,
  vocab,
  notify,
  onFiguresChanged,
}: {
  period: string;
  periodLabel: string;
  accounts: Account[];
  vocab: string[];
  notify: (msg: ToastMsg) => void;
  onFiguresChanged: () => Promise<void>;
}) {
  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [account, setAccount] = useState("");
  const [category, setCategory] = useState("");
  const [sort, setSort] = useState<SortKey>("date");
  const [dir, setDir] = useState<"asc" | "desc">("desc");
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<Load<TxnPage>>({ state: "loading" });
  const [editing, setEditing] = useState<string | null>(null);

  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(search), 250);
    return () => window.clearTimeout(t);
  }, [search]);

  useEffect(() => {
    setOffset(0);
  }, [debounced, account, category, sort, dir]);

  const query = useMemo(
    () => ({
      period,
      // Moved rows are shown, never hidden (GLOSSARY).
      includeNonSpending: true,
      q: debounced || undefined,
      account: account || undefined,
      category: category || undefined,
      sort,
      dir,
      offset,
      limit: PAGE,
    }),
    [period, debounced, account, category, sort, dir, offset],
  );

  const seq = useRef(0);
  const loadPage = useCallback(async () => {
    const mine = ++seq.current;
    try {
      const p = await api.transactions(query);
      if (mine === seq.current) setPage({ state: "ok", data: p });
    } catch (err) {
      if (mine === seq.current) setPage({ state: "error", error: String(err) });
    }
  }, [query]);

  useEffect(() => {
    void loadPage();
  }, [loadPage]);

  const onSort = (k: SortKey) => {
    if (sort === k) setDir((d) => (d === "desc" ? "asc" : "desc"));
    else {
      setSort(k);
      setDir(k === "merchant" || k === "category" ? "asc" : "desc");
    }
  };

  const setTxnCategory = async (t: Transaction, cat: string) => {
    try {
      await api.setCategory(t.id, cat, true);
      notify({ kind: "ok", text: `${t.payee ?? t.description} → ${cat} (rule learned)` });
      await Promise.all([loadPage(), onFiguresChanged()]);
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    }
  };

  const setTxnFlag = async (t: Transaction, flag: TxnFlag) => {
    const before = t.flag;
    try {
      await api.setFlag(t.id, flag, false);
      notify({
        kind: "ok",
        text: flag === "Spending" ? "Counted as spending again." : "Marked as moved between your own accounts.",
        action: { label: "Undo", run: () => void setTxnFlag(t, before) },
      });
      await Promise.all([loadPage(), onFiguresChanged()]);
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    }
  };

  const acctFor = (id: string) => accounts.find((a) => a.id === id);
  const filtered = !!(debounced || account || category);
  const clearFilters = () => {
    setSearch("");
    setDebounced("");
    setAccount("");
    setCategory("");
  };

  // A plain function (not a component) so the header buttons keep their
  // identity, and keyboard focus, across re-renders.
  const sortHead = (k: SortKey, label: string, num?: boolean) => (
    <th
      scope="col"
      className={num ? "num" : undefined}
      aria-sort={sort === k ? (dir === "asc" ? "ascending" : "descending") : "none"}
    >
      <button className="fl-sort" onClick={() => onSort(k)}>
        {label}
        <span className="fl-sort-mark" aria-hidden="true">
          {sort === k ? (dir === "desc" ? "↓" : "↑") : ""}
        </span>
      </button>
    </th>
  );

  return (
    <section className="fl-txns" aria-labelledby="fl-txns-title">
      <h2 id="fl-txns-title" className="fl-section-title">
        Transactions
      </h2>

      <div className="fl-controls" role="search">
        <label className="fl-field fl-field-grow">
          <span className="fl-sr">Search</span>
          <input
            type="search"
            className="fl-input"
            placeholder="Search merchants, descriptions, categories"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
        </label>
        <label className="fl-field">
          <span className="fl-sr">Account</span>
          <select className="fl-input" value={account} onChange={(e) => setAccount(e.target.value)}>
            <option value="">All accounts</option>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {accountDisplayLabel(a.name, a.nickname)}
              </option>
            ))}
          </select>
        </label>
        <label className="fl-field">
          <span className="fl-sr">Category</span>
          <select className="fl-input" value={category} onChange={(e) => setCategory(e.target.value)}>
            <option value="">All categories</option>
            {vocab.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </label>
      </div>

      <p className="fl-total" aria-live="polite">
        {page.state === "ok" ? (
          <>
            <b>{page.data.total}</b> {page.data.total === 1 ? "transaction" : "transactions"} in {periodLabel},
            net <b>{signedCents(page.data.total_cents)}</b>
            <span className="fl-total-hint"> (moves included)</span>
          </>
        ) : page.state === "loading" ? (
          "Loading transactions…"
        ) : null}
      </p>

      {page.state === "error" ? (
        <div className="fl-state" role="alert">
          <p>Couldn't load transactions: {page.error}</p>
          <button className="lg-pill" onClick={() => void loadPage()}>
            Try again
          </button>
        </div>
      ) : page.state === "ok" && page.data.total === 0 ? (
        <div className="fl-state">
          <p>{filtered ? "No transactions match these filters." : `No transactions in ${periodLabel}.`}</p>
          {filtered && (
            <button className="lg-pill" onClick={clearFilters}>
              Clear filters
            </button>
          )}
        </div>
      ) : (
        <div className="fl-table-wrap">
          <table className="fl-table">
            <caption className="fl-sr">Transactions in {periodLabel}</caption>
            <thead>
              <tr>
                {sortHead("date", "Date")}
                {sortHead("merchant", "Merchant")}
                {sortHead("category", "Category")}
                <th scope="col" className="fl-col-acct">
                  Account
                </th>
                {sortHead("amount", "Amount", true)}
                <th scope="col" className="fl-col-act">
                  <span className="fl-sr">Counted as</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {page.state === "ok" &&
                page.data.items.map((t) => {
                  const a = acctFor(t.account_id);
                  const moved = t.flag !== "Spending";
                  return (
                    <tr key={t.id} className={moved ? "fl-moved" : undefined}>
                      <td className="fl-date">{fmtDay(t.transacted_at ?? t.posted)}</td>
                      <td>
                        <span className="fl-merch">
                          <span className="fl-merch-name">{t.payee ?? t.description}</span>
                          {t.pending && <span className="fl-pending">pending</span>}
                        </span>
                      </td>
                      <td className="fl-cat-cell">
                        {/* A move has a group, not a category: the lanes never read
                            its category, so there's nothing to set. */}
                        {moved ? (
                          <span className="fl-tag">
                            Moved · {MOVED_LABEL[t.flag as Exclude<TxnFlag, "Spending">]}
                          </span>
                        ) : editing === t.id ? (
                          <label>
                            <span className="fl-sr">Category for {t.payee ?? t.description}</span>
                            <select
                              autoFocus
                              className="fl-input fl-input-sm"
                              value={t.category ?? ""}
                              onBlur={() => setEditing(null)}
                              onKeyDown={(e) => e.key === "Escape" && setEditing(null)}
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
                          </label>
                        ) : (
                          <button
                            className={`fl-cat${t.category ? "" : " unset"}`}
                            onClick={() => setEditing(t.id)}
                            aria-label={
                              t.category ? `Category: ${t.category}. Change` : "Set a category"
                            }
                          >
                            {t.category ?? "Set category"}
                          </button>
                        )}
                      </td>
                      <td className="fl-col-acct">
                        {a ? <SourceChip s={accountSource(a)} /> : <span className="fl-faint">{t.account_id}</span>}
                      </td>
                      <td className={`num fl-amt${t.amount > 0 ? " in" : ""}`}>{signedCents(t.amount)}</td>
                      <td className="fl-col-act">
                        {moved ? (
                          <button className="fl-mini" onClick={() => void setTxnFlag(t, "Spending")}>
                            Count as spending
                          </button>
                        ) : (
                          <button className="fl-mini" onClick={() => void setTxnFlag(t, "Transfer")}>
                            Mark as moved
                          </button>
                        )}
                      </td>
                    </tr>
                  );
                })}
            </tbody>
          </table>
        </div>
      )}

      {page.state === "ok" && page.data.total > PAGE && (
        <nav className="fl-pager" aria-label="Pages">
          <button className="lg-pill" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - PAGE))}>
            Previous
          </button>
          <span>
            {offset + 1}–{Math.min(offset + PAGE, page.data.total)} of {page.data.total}
          </span>
          <button
            className="lg-pill"
            disabled={offset + PAGE >= page.data.total}
            onClick={() => setOffset(offset + PAGE)}
          >
            Next
          </button>
        </nav>
      )}
    </section>
  );
}
