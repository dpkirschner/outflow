// Flow screen: one month of money In, Out and Moved (GLOSSARY: Flow lanes),
// with the month's transactions under it. Every figure comes from core
// (GET /api/flow, /api/spend/categories); the browser only formats them.
// The month is App state, sent as `period=YYYY-MM` so core decides the
// month's edges in server-local time.
//
// Drill-down: every figure with rows behind it is a button. Selecting one
// filters the table through core's lane filter (lane + direction /
// uncategorized / category), so the rows always sum to the figure.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, type SortKey } from "../api";
import type {
  Account,
  CategorySpend,
  Direction,
  FlowMonth,
  Lane,
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

/** What the table is drilled into. Every field maps to a core filter. */
export interface Pick {
  lane: Lane;
  direction?: Direction;
  uncategorized?: boolean;
  category?: string;
}

function samePick(a: Pick | null, b: Pick | null): boolean {
  return (
    !!a &&
    !!b &&
    a.lane === b.lane &&
    a.direction === b.direction &&
    !!a.uncategorized === !!b.uncategorized &&
    a.category === b.category
  );
}

const LANE_PATH: Record<Lane, string[]> = {
  in: ["In"],
  out: ["Out"],
  card_payment: ["Moved", "Card payment"],
  invest: ["Moved", "Invest"],
  save: ["Moved", "Save"],
  loan_paydown: ["Moved", "Loan paydown"],
  between: ["Moved", "Between own accounts"],
};

/** "Moved › Save › back", for the chip and the empty state. */
function pickLabel(p: Pick): string {
  const parts = [...LANE_PATH[p.lane]];
  if (p.uncategorized) parts.push("Not yet categorized");
  if (p.category) parts.push(p.category);
  if (p.direction && p.lane !== "in" && p.lane !== "out") {
    parts.push(p.direction === "out" ? "out" : "back");
  }
  return parts.join(" › ");
}

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
  // Survives a month change (Flow stays mounted); resets on leaving Flow.
  const [pick, setPick] = useState<Pick | null>(null);
  const onPick = useCallback((p: Pick) => {
    setPick((cur) => {
      if (samePick(cur, p)) return null; // selecting it again clears it
      bringTableIntoView();
      return p;
    });
  }, []);

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
            <InLane m={selected} pick={pick} onPick={onPick} />
            <OutLane
              m={selected}
              cats={cats}
              all={allCats}
              onToggleAll={() => setAllCats((a) => !a)}
              pick={pick}
              onPick={onPick}
            />
            <MovedLaneCard m={selected} pick={pick} onPick={onPick} />
          </div>
        </>
      )}

      {month && (
        <TxnTable
          key={month}
          period={month}
          periodLabel={selected ? monthLabel(selected.year, selected.month) : month}
          month={selected}
          pick={pick}
          onClearPick={() => setPick(null)}
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
                  {formatCentsShort(-m.out_cents).replace("-", "\u2212")}
                </span>
                {sofar && (
                  <span className="fl-msofar" aria-hidden="true">
                    so far
                  </span>
                )}
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
        <b>{unsignedCents(gap)}</b> left after spending and saving, sitting as cash in checking.
      </>
    ) : gap < 0 ? (
      <>
        Spending and saving ran <b>{unsignedCents(-gap)}</b> past what came in, drawn from
        cash.
      </>
    ) : (
      <>Everything left after spending went to savings.</>
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

// The table sits below the lanes; on a phone a pick would otherwise change
// rows the reader can't see.
function bringTableIntoView() {
  window.requestAnimationFrame(() => {
    const el = document.getElementById("fl-txns-title");
    if (!el || el.getBoundingClientRect().top < window.innerHeight * 0.75) return;
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    el.scrollIntoView({ block: "start", behavior: reduce ? "auto" : "smooth" });
  });
}

// ── Lanes ───────────────────────────────────────────────────────────────────
type PickProps = { pick: Pick | null; onPick: (p: Pick) => void };

function Lane({
  id,
  title,
  figure,
  note,
  children,
}: {
  id: string;
  title: string;
  figure?: React.ReactNode;
  note: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <section className="fl-lane" aria-labelledby={`fl-lane-${id}`}>
      <h2 id={`fl-lane-${id}`} className="fl-lane-title">
        {title}
      </h2>
      {figure}
      <p className="fl-lane-note">{note}</p>
      {children}
    </section>
  );
}

/** A lane's headline figure; a button when there are rows behind it. */
function Figure({
  value,
  cents,
  target,
  label,
  pick,
  onPick,
}: { value: string; cents: number; target: Pick; label: string } & PickProps) {
  if (cents === 0) return <p className="fl-lane-figure">{value}</p>;
  const on = samePick(pick, target);
  return (
    <button
      className={`fl-pick fl-lane-figure${on ? " on" : ""}`}
      aria-pressed={on}
      aria-label={`${label} ${value}: show these transactions`}
      onClick={() => onPick(target)}
    >
      {value}
    </button>
  );
}

/** One label/value line. With a target (and money behind it) the whole line
 *  is a button that drills the table into it. */
function Row({
  label,
  value,
  cents,
  target,
  pick,
  onPick,
}: {
  label: React.ReactNode;
  value: string;
  cents: number;
  target?: Pick;
} & PickProps) {
  if (!target || cents === 0) {
    return (
      <li className={`fl-row${cents === 0 ? " quiet" : ""}`}>
        <span className="fl-row-label">{label}</span>
        <span className="fl-row-value">{value}</span>
      </li>
    );
  }
  const on = samePick(pick, target);
  return (
    <li>
      <button
        className={`fl-row fl-pick${on ? " on" : ""}`}
        aria-pressed={on}
        onClick={() => onPick(target)}
      >
        <span className="fl-row-label">{label}</span>
        <span className="fl-row-value">{value}</span>
      </button>
    </li>
  );
}

function InLane({ m, pick, onPick }: { m: FlowMonth } & PickProps) {
  return (
    <Lane
      id="in"
      title="In"
      figure={
        <Figure
          value={signedCents(m.in_cents)}
          cents={m.in_cents}
          target={{ lane: "in" }}
          label="In"
          pick={pick}
          onPick={onPick}
        />
      }
      note="Income from outside your own accounts."
    >
      <ul className="fl-rows">
        <Row
          label="Not yet categorized"
          value={signedCents(m.in_uncategorized_cents)}
          cents={m.in_uncategorized_cents}
          target={{ lane: "in", uncategorized: true }}
          pick={pick}
          onPick={onPick}
        />
      </ul>
    </Lane>
  );
}

function OutLane({
  m,
  cats,
  all,
  onToggleAll,
  pick,
  onPick,
}: {
  m: FlowMonth;
  cats: CategorySpend[] | null;
  all: boolean;
  onToggleAll: () => void;
} & PickProps) {
  const shown = cats ? (all ? cats : cats.slice(0, CATEGORY_PREVIEW)) : [];
  const hidden = cats ? cats.length - CATEGORY_PREVIEW : 0;
  return (
    <Lane
      id="out"
      title="Out"
      figure={
        <Figure
          value={signedCents(-m.out_cents)}
          cents={m.out_cents}
          target={{ lane: "out" }}
          label="Out"
          pick={pick}
          onPick={onPick}
        />
      }
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
          <ul className="fl-rows" aria-label="Out by category">
            {shown.map((c) => (
              <Row
                key={c.category ?? "\u0000"}
                label={c.category ?? <i>Not yet categorized</i>}
                value={signedCents(-c.total_cents)}
                cents={c.total_cents}
                target={
                  c.category
                    ? { lane: "out", category: c.category }
                    : { lane: "out", uncategorized: true }
                }
                pick={pick}
                onPick={onPick}
              />
            ))}
          </ul>
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

/** A Moved group's line: the label, then out and back as separate buttons,
 *  each drilling into its own direction (each matches its own figure). */
function MovedRow({
  label,
  lane,
  f,
  pick,
  onPick,
}: { label: string; lane: Lane; f: MovedFlow } & PickProps) {
  if (f.out_cents === 0 && f.in_cents === 0) {
    return (
      <li className="fl-row quiet">
        <span className="fl-row-label">{label}</span>
        <span className="fl-row-value">$0.00</span>
      </li>
    );
  }
  const side = (direction: Direction, cents: number, text: string, name: string) => {
    const target: Pick = { lane, direction };
    const on = samePick(pick, target);
    return (
      <button
        className={`fl-pick fl-amt-pick${on ? " on" : ""}`}
        aria-pressed={on}
        aria-label={`${label} ${name} ${signedCents(cents)}: show these transactions`}
        onClick={() => onPick(target)}
      >
        {text}
      </button>
    );
  };
  return (
    <li className="fl-row fl-row-split">
      <span className="fl-row-label">{label}</span>
      <span className="fl-row-value fl-sides">
        {f.out_cents > 0 && side("out", -f.out_cents, signedCents(-f.out_cents), "out")}
        {f.in_cents > 0 && side("in", f.in_cents, `${signedCents(f.in_cents)} back`, "back")}
      </span>
    </li>
  );
}

function MovedLaneCard({ m, pick, onPick }: { m: FlowMonth } & PickProps) {
  const mv = m.moved;
  const groups: [string, Lane, MovedFlow][] = [
    ["Invest", "invest", mv.invest],
    ["Save", "save", mv.save],
    ["Loan paydown", "loan_paydown", mv.loan_paydown],
    ["Between own accounts", "between", mv.between],
  ];
  return (
    <Lane id="moved" title="Moved" note="Between your own accounts. Not spending, not income.">
      <ul className="fl-rows">
        <Row
          label={
            <>
              Card payment
              <span className="fl-sub">counted once: the larger of checking out and cards in</span>
            </>
          }
          value={mv.card_payment_cents === 0 ? "$0.00" : signedCents(-mv.card_payment_cents)}
          cents={mv.card_payment_cents}
          target={{ lane: "card_payment" }}
          pick={pick}
          onPick={onPick}
        />
        {groups.map(([label, lane, f]) => (
          <MovedRow key={lane} label={label} lane={lane} f={f} pick={pick} onPick={onPick} />
        ))}
      </ul>
    </Lane>
  );
}

// ── Transactions for the month ──────────────────────────────────────────────

/** One block of rows: the whole table, or one side of a card payment. */
interface Block {
  key: string;
  title?: string;
  page: TxnPage;
}

function TxnTable({
  period,
  periodLabel,
  month,
  pick,
  onClearPick,
  accounts,
  vocab,
  notify,
  onFiguresChanged,
}: {
  period: string;
  periodLabel: string;
  month: FlowMonth | null;
  pick: Pick | null;
  onClearPick: () => void;
  accounts: Account[];
  vocab: string[];
  notify: (msg: ToastMsg) => void;
  onFiguresChanged: () => Promise<void>;
}) {
  const [search, setSearch] = useState("");
  const [debounced, setDebounced] = useState("");
  const [account, setAccount] = useState("");
  const [sort, setSort] = useState<SortKey>("date");
  const [dir, setDir] = useState<"asc" | "desc">("desc");
  const [offset, setOffset] = useState(0);
  const [blocks, setBlocks] = useState<Load<Block[]>>({ state: "loading" });
  const [editing, setEditing] = useState<string | null>(null);

  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(search), 250);
    return () => window.clearTimeout(t);
  }, [search]);

  useEffect(() => {
    setOffset(0);
  }, [debounced, account, sort, dir, pick]);

  // Card payment (whole group) is shown by side: each side is its own core
  // query, so its subtotal covers every row, not just one page.
  const bySide = pick?.lane === "card_payment" && !pick.direction;

  const base = useMemo(
    () => ({
      period,
      // Moved rows are shown, never hidden (GLOSSARY).
      includeNonSpending: true,
      q: debounced || undefined,
      account: account || undefined,
      lane: pick?.lane,
      direction: pick?.direction,
      uncategorized: pick?.uncategorized,
      category: pick?.category,
      sort,
      dir,
    }),
    [period, debounced, account, pick, sort, dir],
  );

  const seq = useRef(0);
  const load = useCallback(async () => {
    const mine = ++seq.current;
    try {
      const next: Block[] = bySide
        ? await Promise.all(
            (
              [
                ["out", "From checking"],
                ["in", "Into cards"],
              ] as const
            ).map(async ([direction, title]) => ({
              key: direction,
              title,
              page: await api.transactions({ ...base, direction, offset: 0, limit: 200 }),
            })),
          )
        : [{ key: "all", page: await api.transactions({ ...base, offset, limit: PAGE }) }];
      if (mine === seq.current) setBlocks({ state: "ok", data: next });
    } catch (err) {
      if (mine === seq.current) setBlocks({ state: "error", error: String(err) });
    }
  }, [base, bySide, offset]);

  useEffect(() => {
    void load();
  }, [load]);

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
      await Promise.all([load(), onFiguresChanged()]);
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
        text:
          flag === "Spending"
            ? "Counted as spending again."
            : "Marked as moved between your own accounts.",
        action: { label: "Undo", run: () => void setTxnFlag(t, before) },
      });
      await Promise.all([load(), onFiguresChanged()]);
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    }
  };

  const acctFor = (id: string) => accounts.find((a) => a.id === id);
  const narrowed = !!(debounced || account);
  const clearNarrowing = () => {
    setSearch("");
    setDebounced("");
    setAccount("");
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

  const row = (t: Transaction) => {
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
          {/* A move has a group, not a category: the lanes never read its
              category, so there's nothing to set. */}
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
              aria-label={t.category ? `Category: ${t.category}. Change` : "Set a category"}
            >
              {t.category ?? "Set category"}
            </button>
          )}
        </td>
        <td className="fl-col-acct">
          {a ? (
            <SourceChip s={accountSource(a)} />
          ) : (
            <span className="fl-faint">{t.account_id}</span>
          )}
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
  };

  const data = blocks.state === "ok" ? blocks.data : [];
  const count = data.reduce((n, b) => n + b.page.total, 0);
  // Render from what has loaded, not from the current pick: between a click
  // and its response the old blocks are still on screen.
  const sides = data.length === 2;
  const single = !sides ? data[0]?.page : undefined;
  const what = pick ? pickLabel(pick) : null;

  const totalLine = () => {
    if (blocks.state === "loading") return "Loading transactions…";
    if (blocks.state === "error") return null;
    const n = (
      <>
        <b>{count}</b> {count === 1 ? "transaction" : "transactions"}
      </>
    );
    const narrowNote = narrowed && pick && (
      <span className="fl-total-hint">
        {" "}
        Narrowed by {debounced && account ? "search and account" : debounced ? "search" : "account"},
        so this won't match the figure above.
      </span>
    );
    if (sides) {
      const [out, into] = data;
      return (
        <>
          {n}: <b>{signedCents(out.page.total_cents)}</b> from checking,{" "}
          <b>{signedCents(into.page.total_cents)}</b> into cards.
          {month && (
            <>
              {" "}
              Moved counts it once, as the larger side:{" "}
              <b>{signedCents(-month.moved.card_payment_cents)}</b>.
            </>
          )}
          {narrowNote}
        </>
      );
    }
    if (pick && single) {
      return (
        <>
          {n}, total <b>{signedCents(single.total_cents)}</b>
          {narrowNote}
        </>
      );
    }
    return (
      single && (
        <>
          {n} in {periodLabel}, net <b>{signedCents(single.total_cents)}</b>
          <span className="fl-total-hint">, including moves between your accounts</span>
        </>
      )
    );
  };

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
          <select
            className="fl-input"
            value={account}
            onChange={(e) => setAccount(e.target.value)}
          >
            <option value="">All accounts</option>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {accountDisplayLabel(a.name, a.nickname)}
              </option>
            ))}
          </select>
        </label>
      </div>

      {what && (
        <div className="fl-chip">
          <span>
            <span className="fl-sr">Showing </span>
            {what}
          </span>
          <button className="fl-chip-clear" onClick={onClearPick} aria-label={`Clear ${what}`}>
            Clear
          </button>
        </div>
      )}

      <p className="fl-total" aria-live="polite">
        {totalLine()}
      </p>

      {blocks.state === "error" ? (
        <div className="fl-state" role="alert">
          <p>Couldn't load transactions: {blocks.error}</p>
          <button className="lg-pill" onClick={() => void load()}>
            Try again
          </button>
        </div>
      ) : blocks.state === "ok" && count === 0 ? (
        <div className="fl-state">
          <p>
            {what
              ? `Nothing in ${what} for ${periodLabel}${narrowed ? " with this search or account" : ""}.`
              : narrowed
                ? "No transactions match this search or account."
                : `No transactions in ${periodLabel}.`}
          </p>
          <div className="fl-state-actions">
            {narrowed && (
              <button className="lg-pill" onClick={clearNarrowing}>
                Clear search and account
              </button>
            )}
            {what && (
              <button className="lg-pill" onClick={onClearPick}>
                Show all of {periodLabel}
              </button>
            )}
          </div>
        </div>
      ) : (
        <div className="fl-table-wrap">
          <table className="fl-table">
            <caption className="fl-sr">
              {what ? `Transactions in ${what}, ${periodLabel}` : `Transactions in ${periodLabel}`}
            </caption>
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
            {data.map((b) => (
              <tbody key={b.key}>
                {b.title && (
                  <tr className="fl-group">
                    <th scope="rowgroup" colSpan={4}>
                      {b.title}
                      <span className="fl-group-count">
                        {b.page.total} {b.page.total === 1 ? "row" : "rows"}
                      </span>
                    </th>
                    <td className="num fl-amt">{signedCents(b.page.total_cents)}</td>
                    <td className="fl-col-act" />
                  </tr>
                )}
                {b.title && b.page.total === 0 ? (
                  <tr>
                    <td colSpan={6} className="fl-group-empty">
                      None this month.
                    </td>
                  </tr>
                ) : (
                  b.page.items.map(row)
                )}
              </tbody>
            ))}
          </table>
        </div>
      )}

      {single && single.total > PAGE && (
        <nav className="fl-pager" aria-label="Pages">
          <button
            className="lg-pill"
            disabled={offset === 0}
            onClick={() => setOffset(Math.max(0, offset - PAGE))}
          >
            Previous
          </button>
          <span>
            {offset + 1}–{Math.min(offset + PAGE, single.total)} of {single.total}
          </span>
          <button
            className="lg-pill"
            disabled={offset + PAGE >= single.total}
            onClick={() => setOffset(offset + PAGE)}
          >
            Next
          </button>
        </nav>
      )}
    </section>
  );
}
