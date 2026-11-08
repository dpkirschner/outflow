// Holdings (Stock): what it's worth, what changed over the month, and what
// the market did. Replaces Balances. Every figure comes from core::stock and
// core::balances through the API; this renders it and draws the charts.
//
// The month is App state shared with Flow; longer ranges (3M / YTD / 1Y /
// All) are remembered between visits in this browser.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api, type StockRange } from "../api";
import { formatCents, formatCentsShort, monthLabel, monthLabelShort, signedCents, errMessage } from "../format";
import { onLinkClick, pathOf } from "../route";
import type {
  Account,
  AccountBalance,
  AccountChange,
  BalanceGroup,
  BalancesView,
  DayTotals,
  NetWorthChange,
  PositionChange,
  Totals,
} from "../types";
import { SourceChip, accountSource, agoLabel } from "./accounts";
import { MonthStrip, type StripMonth } from "./MonthStrip";

type Load<T> = { state: "loading" } | { state: "error"; error: string } | { state: "ok"; data: T };

type View = "month" | "3m" | "ytd" | "1y" | "all";
const VIEWS: { key: View; label: string }[] = [
  { key: "month", label: "Month" },
  { key: "3m", label: "3M" },
  { key: "ytd", label: "YTD" },
  { key: "1y", label: "1Y" },
  { key: "all", label: "All" },
];
const VIEW_TITLE: Record<Exclude<View, "month">, string> = {
  "3m": "Last 3 months",
  ytd: "This year",
  "1y": "Last 12 months",
  all: "All history",
};
// How a range reads inside a sentence: "in the last 3 months".
const VIEW_PHRASE: Record<Exclude<View, "month">, string> = {
  "3m": "the last 3 months",
  ytd: "this year",
  "1y": "the last 12 months",
  all: "all of history",
};
const VIEW_KEY = "outflow.holdings.view";

function loadView(): View {
  try {
    const v = window.localStorage.getItem(VIEW_KEY);
    return VIEWS.some((x) => x.key === v) ? (v as View) : "month";
  } catch {
    return "month";
  }
}

type Lens = "account" | "position";
const LENS_KEY = "outflow.holdings.lens";
function loadLens(): Lens {
  try {
    return window.localStorage.getItem(LENS_KEY) === "position" ? "position" : "account";
  } catch {
    return "account";
  }
}

// Open account rows survive a trip to another screen (this module stays
// loaded); a reload starts closed.
let rememberedOpen = new Set<string>();

/** Uninvested cash under a dollar is a rounding remainder, not a holding:
 *  left out of the rows and the "and cash" wording. Totals are unchanged. */
const TINY_CASH_CENTS = 100;
function shown(rows: PositionChange[]): PositionChange[] {
  return rows.filter((p) => !(p.is_cash && Math.abs(p.end_value) < TINY_CASH_CENTS));
}

function saveView(v: View) {
  try {
    window.localStorage.setItem(VIEW_KEY, v);
  } catch {
    /* private window: the view just isn't remembered */
  }
}

const GROUPS: { group: BalanceGroup; label: string; total: keyof Totals }[] = [
  { group: "cash", label: "Cash", total: "cash" },
  { group: "investments", label: "Investments", total: "invested" },
  { group: "credit", label: "Credit", total: "credit" },
  { group: "loans", label: "Loans", total: "loans" },
  { group: "other", label: "Other", total: "other" },
];

// ── dates ──────────────────────────────────────────────────────────────────
const pad = (n: number) => String(n).padStart(2, "0");
const isoDay = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const localDay = (epoch: number) => isoDay(new Date(epoch * 1000));
const thisMonth = () => isoDay(new Date()).slice(0, 7);
// "YYYY-MM-DD" → UTC midnight ms; chart formatting is UTC too, so no TZ drift.
const dayMs = (day: string) => Date.parse(`${day}T00:00:00Z`);
const DAY_MS = 86_400_000;

/** "Oct 31", or "Oct 31, 2025" when it isn't this year. Same order as the axis. */
function dayLabel(day: string): string {
  const d = new Date(dayMs(day));
  const md = d.toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
  return d.getUTCFullYear() === new Date().getFullYear() ? md : `${md}, ${d.getUTCFullYear()}`;
}

function rangeOf(view: View, month: string | null): StockRange {
  const now = new Date();
  switch (view) {
    case "month":
      return month ? { period: month } : { period: thisMonth() };
    case "3m":
      return { since: isoDay(new Date(now.getFullYear(), now.getMonth() - 3, now.getDate())) };
    case "ytd":
      return { period: "this-year" };
    case "1y":
      return { since: isoDay(new Date(now.getFullYear() - 1, now.getMonth(), now.getDate())) };
    case "all":
      return {};
  }
}

// ── numbers ────────────────────────────────────────────────────────────────
/** Basis points as a signed percent: +5.4%, −0.8%, 0.0%. */
function signedPct(bp: number): string {
  const body = `${(Math.abs(bp) / 100).toFixed(1)}%`;
  return bp > 0 ? `+${body}` : bp < 0 ? `−${body}` : body;
}

/** An error as a sentence: the message, without "Error: " in front. */

/** Share quantity without trailing zeros: "936.0000" → "936", "1.2500" → "1.25". */
function qtyLabel(q: string): string {
  return q.includes(".") ? q.replace(/\.?0+$/, "") : q;
}

const tone = (cents: number | null) =>
  cents === null || cents === 0 ? "" : cents > 0 ? " hd-up" : " hd-down";

// ── screen ─────────────────────────────────────────────────────────────────
export function Holdings({
  month,
  onMonth,
}: {
  /** Selected month, `YYYY-MM`, shared with Flow; null until the first load. */
  month: string | null;
  onMonth: (m: string) => void;
}) {
  const [history, setHistory] = useState<Load<DayTotals[]>>({ state: "loading" });
  const [balances, setBalances] = useState<BalancesView | null>(null);
  const [view, setViewState] = useState<View>(loadView);
  const [lens, setLensState] = useState<Lens>(loadLens);
  const setLens = (l: Lens) => {
    setLensState(l);
    try {
      window.localStorage.setItem(LENS_KEY, l);
    } catch {
      /* private window: the lens just isn't remembered */
    }
  };
  const [change, setChange] = useState<Load<NetWorthChange>>({ state: "loading" });
  const [positions, setPositions] = useState<Load<PositionChange[]>>({ state: "loading" });
  const [combined, setCombined] = useState<Load<PositionChange[]>>({ state: "loading" });
  // A range switch keeps the last figures on screen (dimmed) until the new
  // ones land; only the newest request may write, so a slow earlier
  // response can't overwrite a later month.
  const [busy, setBusy] = useState(false);
  const [combinedBusy, setCombinedBusy] = useState(false);
  const changeSeq = useRef(0);
  const combinedSeq = useRef(0);

  const setView = (v: View) => {
    setViewState(v);
    saveView(v);
  };

  const loadBase = useCallback(async () => {
    setHistory({ state: "loading" });
    try {
      const [h, b] = await Promise.all([api.balanceHistory(), api.balances()]);
      setHistory({ state: "ok", data: h });
      setBalances(b);
    } catch (err) {
      setHistory({ state: "error", error: errMessage(err) });
    }
  }, []);
  useEffect(() => {
    void loadBase();
  }, [loadBase]);

  // Months that have snapshots, each with its month-end net worth.
  const months = useMemo<StripMonth[]>(() => {
    if (history.state !== "ok") return [];
    const last = new Map<string, DayTotals>();
    for (const d of history.data) last.set(d.day.slice(0, 7), d);
    const now = thisMonth();
    return [...last.entries()].map(([key, d]) => {
      const [y, m] = key.split("-").map(Number);
      return {
        key,
        label: monthLabel(y, m),
        short: monthLabelShort(y, m),
        figure: formatCentsShort(d.net_worth).replace("-", "−"),
        spoken: `net worth ${formatCents(d.net_worth)} at ${dayLabel(d.day)}`,
        sofar: key === now,
      };
    });
  }, [history]);

  // Default to the latest month with snapshots when nothing is selected yet.
  // A month picked on Flow that has no snapshots stays selected; the screen
  // says so instead of moving it.
  useEffect(() => {
    if (!month && months.length > 0) onMonth(months[months.length - 1].key);
  }, [month, months, onMonth]);

  const range = useMemo(() => rangeOf(view, month), [view, month]);
  const rangeKey = JSON.stringify(range);

  const loadChange = useCallback(async () => {
    const seq = ++changeSeq.current;
    setBusy(true);
    setChange((c) => (c.state === "ok" ? c : { state: "loading" }));
    try {
      const [c, p] = await Promise.all([
        api.netWorthChange(range),
        api.holdingsChange(range, "account"),
      ]);
      if (seq !== changeSeq.current) return;
      setChange({ state: "ok", data: c });
      setPositions({ state: "ok", data: p });
    } catch (err) {
      if (seq !== changeSeq.current) return;
      setChange({ state: "error", error: errMessage(err) });
      setPositions({ state: "error", error: errMessage(err) });
    } finally {
      if (seq === changeSeq.current) setBusy(false);
    }
    // rangeKey stands for range (a fresh object each render).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rangeKey]);
  useEffect(() => {
    if (view === "month" && !month) return;
    void loadChange();
  }, [loadChange, view, month]);

  const loadCombined = useCallback(async () => {
    const seq = ++combinedSeq.current;
    setCombinedBusy(true);
    setCombined((c) => (c.state === "ok" ? c : { state: "loading" }));
    try {
      const data = await api.holdingsChange(range, "position");
      if (seq === combinedSeq.current) setCombined({ state: "ok", data });
    } catch (err) {
      if (seq === combinedSeq.current) setCombined({ state: "error", error: errMessage(err) });
    } finally {
      if (seq === combinedSeq.current) setCombinedBusy(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rangeKey]);
  useEffect(() => {
    if (lens === "position") void loadCombined();
  }, [lens, loadCombined]);

  // ── states ──
  if (history.state === "loading") {
    return (
      <main className="hd-wrap" aria-labelledby="hd-title">
        <h1 id="hd-title" className="lg-sr">
          Holdings
        </h1>
        <p className="lg-empty" role="status">
          Loading holdings…
        </p>
      </main>
    );
  }
  if (history.state === "error") {
    return (
      <main className="hd-wrap" aria-labelledby="hd-title">
        <h1 id="hd-title" className="lg-sr">
          Holdings
        </h1>
        <div className="hd-state" role="alert">
          <p>Couldn't load holdings: {history.error}</p>
          <button className="hd-btn" onClick={() => void loadBase()}>
            Try again
          </button>
        </div>
      </main>
    );
  }
  const noAccounts = (balances?.groups.length ?? 0) === 0;
  if (noAccounts && history.data.length === 0) {
    return (
      <main className="hd-wrap" aria-labelledby="hd-title">
        <h1 id="hd-title" className="lg-sr">
          Holdings
        </h1>
        <div className="hd-state">
          <p>No accounts yet.</p>
          <p className="hd-state-hint">
            Connect a bank in{" "}
            <a href={pathOf("settings")} onClick={(e) => onLinkClick(e, pathOf("settings"))}>
              Settings
            </a>
            ; the first sync records the first snapshot.
          </p>
        </div>
      </main>
    );
  }

  const selected = months.find((m) => m.key === month) ?? null;
  const title =
    view === "month"
      ? month
        ? monthLabel(Number(month.slice(0, 4)), Number(month.slice(5, 7)))
        : "Holdings"
      : VIEW_TITLE[view];
  // Whether the range reaches today (idle cash and stale syncs only make
  // sense then).
  const live = view !== "month" || month === thisMonth();
  const acctById = new Map<string, AccountBalance>(
    (balances?.groups ?? []).flatMap((g) => g.accounts.map((a) => [a.account.id, a] as const)),
  );

  return (
    <main className="hd-wrap" aria-labelledby="hd-title">
      <MonthStrip
        caption="Net worth at month end"
        months={months}
        selected={view === "month" ? month : null}
        onSelect={(k) => {
          onMonth(k);
          if (view !== "month") setView("month");
        }}
      />

      <div className="hd-titlebar">
        <h1 id="hd-title" className="hd-title">
          {title}
          {view === "month" && selected?.sofar && <span className="hd-sofar">so far</span>}
        </h1>
        <div className="lg-win hd-views" role="group" aria-label="Range">
          {VIEWS.map((v) => (
            <button
              key={v.key}
              className={v.key === view ? "on" : ""}
              aria-pressed={v.key === view}
              onClick={() => setView(v.key)}
            >
              {v.label}
            </button>
          ))}
        </div>
      </div>

      {change.state === "loading" && (
        <p className="hd-state hd-quiet" role="status">
          Loading…
        </p>
      )}
      {change.state === "error" && (
        <div className="hd-state" role="alert">
          <p>Couldn't load the change: {change.error}</p>
          <button className="hd-btn" onClick={() => void loadChange()}>
            Try again
          </button>
        </div>
      )}
      {change.state === "ok" && (
        <div className={busy ? "hd-body hd-busy" : "hd-body"} aria-busy={busy}>
          <Hero
            c={change.data}
            phrase={view === "month" ? title : VIEW_PHRASE[view]}
            sofar={view !== "month" || !!selected?.sofar}
            firstDay={history.data[0]?.day ?? null}
          />
          <Breakdown c={change.data} fallback={balances?.totals ?? null} />
          <Charts c={change.data} history={history.data} />
          {live && <IdleCash balances={balances} />}

          <section className="hd-section" aria-labelledby="hd-accts-title">
            <div className="hd-section-head">
              <h2 id="hd-accts-title">{lens === "account" ? "Accounts" : "Positions"}</h2>
              <div className="lg-win" role="group" aria-label="Group by">
                <button
                  className={lens === "account" ? "on" : ""}
                  aria-pressed={lens === "account"}
                  onClick={() => setLens("account")}
                >
                  By account
                </button>
                <button
                  className={lens === "position" ? "on" : ""}
                  aria-pressed={lens === "position"}
                  onClick={() => setLens("position")}
                >
                  By position
                </button>
              </div>
            </div>
            {lens === "account" ? (
              <AccountGroups c={change.data} positions={positions} acctById={acctById} live={live} />
            ) : (
              <CombinedPositions
                load={combined}
                busy={combinedBusy}
                onRetry={() => void loadCombined()}
              />
            )}
          </section>
        </div>
      )}
    </main>
  );
}

// ── Hero: Change = Markets + Added, and the answer in words ─────────────────
function Hero({
  c,
  phrase,
  sofar,
  firstDay,
}: {
  c: NetWorthChange;
  /** The range inside a sentence: "Nov 2026", "the last 3 months". */
  phrase: string;
  /** The range reaches today, so more snapshots are still coming. */
  sofar: boolean;
  firstDay: string | null;
}) {
  if (c.change === null) {
    const on = c.end_at ? ` (${dayLabel(localDay(c.end_at))})` : "";
    const why =
      c.reason === "no_snapshots"
        ? `No snapshots in ${phrase}.${firstDay ? ` History starts ${dayLabel(firstDay)}.` : ""}`
        : sofar
          ? `Only one snapshot in ${phrase} so far${on}, so there's no change to show yet.`
          : `Only one snapshot in ${phrase}${on}, so there's no change to show.`;
    return (
      <section className="hd-head" aria-label="Summary">
        <p className="hd-say">{why}</p>
      </section>
    );
  }
  const start = c.start_at ? dayLabel(localDay(c.start_at)) : "";
  const end = c.end_at ? dayLabel(localDay(c.end_at)) : "";
  const invest = c.accounts.filter(
    (a) => a.group === "investments" && a.change !== null && a.gain_reason !== "cash",
  );
  const measured = invest.filter((a) => a.gain !== null).length;
  return (
    <section className="hd-head" aria-label="Summary">
      <dl className="hd-eq">
        <div className="hd-eq-main">
          <dt>Net worth change</dt>
          <dd>
            {signedCents(c.change)}
            {c.change_bp !== null && <span className="hd-eq-pct">{signedPct(c.change_bp)}</span>}
          </dd>
        </div>
        <div className="hd-eq-op" aria-hidden="true">
          =
        </div>
        <div>
          <dt>Markets</dt>
          <dd>{signedCents(c.markets ?? 0)}</dd>
        </div>
        <div className="hd-eq-op" aria-hidden="true">
          +
        </div>
        <div>
          <dt>Added</dt>
          <dd>{signedCents(c.added ?? 0)}</dd>
        </div>
      </dl>
      <p className="hd-say">
        From <b>{formatCents(c.start.net_worth)}</b> on {start} to{" "}
        <b>{formatCents(c.end.net_worth)}</b> on {end}.
      </p>
      <details className="hd-defs hd-note">
        <summary>What Markets and Added mean</summary>
        <p className="hd-quiet">
          Markets: what investments earned beyond the money put in. Added: saved from income, less
          spending.
        </p>
      </details>
      {c.added_includes_unmeasured && (
        <p className="hd-note">
          Markets covers {measured} of {invest.length} investment accounts.{" "}
          {listAnd(c.excluded.map((x) => x.account))}{" "}
          {c.excluded.length === 1 ? "doesn't report its" : "don't report their"} contributions, so{" "}
          {c.excluded.length === 1 ? "its" : "their"} whole change,{" "}
          {listAnd(c.excluded.map((x) => signedCents(x.change)))}, is counted in Added.
        </p>
      )}
      {c.partial_start && (
        <p className="hd-note hd-quiet">
          {firstDay ? `History starts ${dayLabel(firstDay)}, so some` : "Some"} accounts are
          measured from their first snapshot.
        </p>
      )}
    </section>
  );
}

/** "a", "a and b", "a, b and c". */
function listAnd(xs: string[]): string {
  return xs.length < 2 ? (xs[0] ?? "") : `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`;
}

// ── Breakdown: one signed line that sums to net worth ──────────────────────
function Breakdown({ c, fallback }: { c: NetWorthChange; fallback: Totals | null }) {
  const inRange = c.accounts.length > 0;
  const t = inRange ? c.end : fallback;
  if (!t) return null;
  const at = inRange && c.end_at ? `On ${dayLabel(localDay(c.end_at))}` : "As of the last sync";
  type Term = { op: "+" | "−" | "="; word: string; label: string; cents: number; sub?: string };
  const terms: Term[] = [
    { op: "+", word: "", label: "Cash", cents: t.cash },
    {
      op: "+",
      word: "plus",
      label: "Invested",
      cents: t.invested,
      sub: t.uninvested > 0 ? `incl. ${formatCents(t.uninvested)} uninvested` : undefined,
    },
    { op: "−", word: "minus", label: "Credit owed", cents: -t.credit },
  ];
  if (t.loans !== 0) terms.push({ op: "−", word: "minus", label: "Loans", cents: -t.loans });
  if (t.other !== 0)
    terms.push({
      op: t.other < 0 ? "−" : "+",
      word: t.other < 0 ? "minus" : "plus",
      label: "Other",
      cents: Math.abs(t.other),
    });
  terms.push({ op: "=", word: "equals", label: "Net worth", cents: t.net_worth });
  return (
    <section aria-label={`Net worth. ${at}`}>
      <ol className="hd-sum">
        {terms.map((x, i) => (
          <li key={x.label} className={x.op === "=" ? "hd-sum-total" : undefined}>
            {i > 0 && (
              <span className="hd-sum-op">
                <span aria-hidden="true">{x.op}</span>
                <span className="lg-sr">{x.word}</span>
              </span>
            )}
            <span className="hd-tile">
              <span className="hd-tile-key">{x.label}</span>
              <span className="hd-tile-val">{formatCents(x.cents)}</span>
              {x.sub && <span className="hd-tile-sub">{x.sub}</span>}
            </span>
          </li>
        ))}
      </ol>
      <p className="hd-meta">{at}. Credit owed and loans count against net worth.</p>
    </section>
  );
}

// ── Charts ─────────────────────────────────────────────────────────────────
// SVG presentation attributes don't resolve CSS vars, so the charts read the
// --lg-* tokens from ledger.css when they first render (not at module load:
// main.tsx imports the app before the stylesheet).
let palette: { line: string; dim: string; nw: string; invest: string } | null = null;
function colors() {
  if (!palette) {
    const css = getComputedStyle(document.documentElement);
    const v = (name: string) => css.getPropertyValue(name).trim();
    palette = { line: v("--lg-line"), dim: v("--lg-dim"), nw: v("--lg-teal"), invest: v("--lg-kind-invest") };
  }
  return palette;
}

type Pt = { t: number; day: string; [k: string]: number | string };
type Series = { key: string; label: string; color: string; dash?: string };

function Charts({ c, history }: { c: NetWorthChange; history: DayTotals[] }) {
  if (!c.start_at || !c.end_at) return null;
  const from = localDay(c.start_at);
  const to = localDay(c.end_at);
  const days = history.filter((d) => d.day >= from && d.day <= to);
  const nw: Pt[] = days.map((d) => ({ t: dayMs(d.day), day: d.day, net_worth: d.net_worth }));
  const inv: Pt[] = c.contributed.map((p) => ({
    t: dayMs(p.day),
    day: p.day,
    invested: p.invested,
    contributed: p.contributed,
  }));
  if (nw.length < 2) return null;
  const first = days[0];
  const last = days[days.length - 1];
  const lastInv = c.contributed[c.contributed.length - 1];
  const firstInv = c.contributed[0];
  return (
    <section className="hd-charts" aria-label="Over time">
      <Chart
        title="Net worth"
        series={[{ key: "net_worth", label: "Net worth", color: colors().nw }]}
        pts={nw}
        summary={`Net worth from ${dayLabel(first.day)} to ${dayLabel(last.day)}: ${formatCents(
          first.net_worth,
        )} to ${formatCents(last.net_worth)}, ${signedCents(last.net_worth - first.net_worth)}.`}
        note={
          <>
            Cash (checking and savings) {formatCents(last.cash)} on {dayLabel(last.day)}, was{" "}
            {formatCents(first.cash)}.
          </>
        }
      />
      {inv.length >= 2 && firstInv && lastInv && (
        <Chart
          title="Invested vs contributed"
          series={[
            { key: "invested", label: "Invested", color: colors().invest },
            { key: "contributed", label: "Contributed", color: colors().dim, dash: "5 4" },
          ]}
          pts={inv}
          summary={`Invested from ${dayLabel(firstInv.day)} to ${dayLabel(lastInv.day)}: ${formatCents(
            firstInv.invested,
          )} to ${formatCents(lastInv.invested)}; contributions add ${signedCents(
            lastInv.contributed - firstInv.contributed,
          )}. ${
            c.contributed_complete
              ? "The gap between the lines is what the market did."
              : "Some contributions aren't reported, so the gap overstates the market."
          }`}
          note={
            c.contributed_complete ? (
              <>The gap between the lines is what the market did.</>
            ) : (
              <>
                Some contributions aren't reported, so the contributed line runs low and the gap
                overstates the market.
              </>
            )
          }
        />
      )}
    </section>
  );
}

function tickDate(t: number, long: boolean): string {
  const d = new Date(t);
  const mon = d.toLocaleDateString("en-US", { month: "short", timeZone: "UTC" });
  return long ? `${mon} '${String(d.getUTCFullYear()).slice(2)}` : `${mon} ${d.getUTCDate()}`;
}

// Compact axis dollars: "$412k", "$1.25M", "−$950".
function axisMoney(cents: number): string {
  const d = Math.abs(cents) / 100;
  const sign = cents < 0 ? "−" : "";
  const short = (n: number) => String(Number(n.toFixed(2)));
  if (d >= 1e6) return `${sign}$${short(d / 1e6)}M`;
  if (d >= 1e3) return `${sign}$${short(d / 1e3)}k`;
  return `${sign}$${Math.round(d)}`;
}

// ~5 whole-day ticks counted back from the newest day, so it's always labelled.
function dayTicks(lo: number, hi: number): number[] {
  const days = Math.round((hi - lo) / DAY_MS);
  const step = Math.max(1, Math.ceil(days / 4));
  const out: number[] = [];
  for (let k = 0; k <= days; k += step) out.unshift(hi - k * DAY_MS);
  if (out[0] !== lo && out[0] - lo >= step * DAY_MS * 0.6) out.unshift(lo);
  return out;
}

// Round-number y ticks (1/2/2.5/5 × 10ⁿ) covering [min, max] in ~3 steps.
function moneyTicks(min: number, max: number): number[] {
  const span = Math.max(max - min, Math.abs(max) * 0.005, 5_000);
  const raw = span / 3;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw)!;
  const mid = (min + max) / 2;
  let lo = Math.floor(Math.min(min, mid - span / 2) / step) * step;
  let hi = Math.ceil(Math.max(max, mid + span / 2) / step) * step;
  if (min - lo < step * 0.03) lo -= step;
  if (hi - max < step * 0.03) hi += step;
  const out: number[] = [];
  for (let v = lo; v <= hi + step / 2; v += step) out.push(Math.round(v));
  return out;
}

function Chart({
  title,
  series,
  pts,
  summary,
  note,
}: {
  title: string;
  series: Series[];
  pts: Pt[];
  summary: string;
  note: React.ReactNode;
}) {
  const lo = pts[0].t;
  const hi = pts[pts.length - 1].t;
  const vals = pts.flatMap((p) => series.map((s) => p[s.key] as number));
  const yTicks = moneyTicks(Math.min(...vals), Math.max(...vals));
  const long = hi - lo > 180 * DAY_MS;
  return (
    <figure className="hd-chart">
      <figcaption className="hd-chart-head">
        <span className="hd-chart-title">{title}</span>
        {series.length > 1 && (
          <span className="hd-legend">
            {series.map((s) => (
              <span key={s.key}>
                <svg width="18" height="8" aria-hidden="true">
                  <line
                    x1="1"
                    y1="4"
                    x2="17"
                    y2="4"
                    stroke={s.color}
                    strokeWidth="2"
                    strokeDasharray={s.dash}
                  />
                </svg>
                {s.label}
              </span>
            ))}
          </span>
        )}
      </figcaption>
      <div role="img" aria-label={summary}>
        <ResponsiveContainer width="100%" height={180}>
          <LineChart data={pts} margin={{ top: 6, right: 12, bottom: 0, left: 0 }}>
            <CartesianGrid stroke={colors().line} vertical={false} />
            <XAxis
              dataKey="t"
              type="number"
              scale="time"
              domain={[lo, hi]}
              ticks={dayTicks(lo, hi)}
              tickFormatter={(t: number) => tickDate(t, long)}
              tick={{ fill: colors().dim, fontSize: 11 }}
              stroke={colors().line}
              tickLine={false}
            />
            <YAxis
              domain={[yTicks[0], yTicks[yTicks.length - 1]]}
              ticks={yTicks}
              interval={0}
              tickFormatter={axisMoney}
              tick={{ fill: colors().dim, fontSize: 11 }}
              tickLine={false}
              axisLine={false}
              width={56}
            />
            <Tooltip
              content={<ChartTip series={series} />}
              cursor={{ stroke: colors().dim, strokeDasharray: "3 3" }}
              isAnimationActive={false}
            />
            {series.map((s) => (
              <Line
                key={s.key}
                type="linear"
                dataKey={s.key}
                stroke={s.color}
                strokeWidth={2}
                strokeDasharray={s.dash}
                dot={false}
                activeDot={{ r: 4, strokeWidth: 0 }}
                isAnimationActive={false}
              />
            ))}
          </LineChart>
        </ResponsiveContainer>
      </div>
      <p className="hd-meta">{note}</p>
    </figure>
  );
}

// `active`/`payload` are injected by recharts' Tooltip.
function ChartTip({
  active,
  payload,
  series,
}: {
  active?: boolean;
  payload?: { payload: Pt }[];
  series: Series[];
}) {
  if (!active || !payload?.length) return null;
  const p = payload[0].payload;
  return (
    <div className="hd-tip">
      <div className="hd-quiet">{dayLabel(p.day)}</div>
      {series.map((s) => (
        <div key={s.key}>
          {s.label} <b>{formatCents(p[s.key] as number)}</b>
        </div>
      ))}
    </div>
  );
}

// ── Idle cash: investment accounts sitting on cash (attention amber) ───────
function IdleCash({ balances }: { balances: BalancesView | null }) {
  const idle = (balances?.groups ?? []).flatMap((g) => g.accounts).filter((a) => a.idle_cash);
  if (idle.length === 0) return null;
  return (
    <section className="hd-idle" aria-labelledby="hd-idle-title">
      <h2 id="hd-idle-title" className="hd-idle-title">
        Idle cash in investment accounts
      </h2>
      <ul>
        {idle.map((a) => (
          <li key={a.account.id}>
            <SourceChip s={accountSource(a.account)} />
            <b>{formatCents(a.cash)}</b>
            <span className="hd-quiet">
              {(a.cash_bp / 100).toFixed(1)}% of the account isn't invested
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}

// ── Accounts by type ───────────────────────────────────────────────────────
function AccountGroups({
  c,
  positions,
  acctById,
  live,
}: {
  c: NetWorthChange;
  positions: Load<PositionChange[]>;
  acctById: Map<string, AccountBalance>;
  live: boolean;
}) {
  const [open, setOpenState] = useState<Set<string>>(rememberedOpen);
  const setOpen = (f: (s: Set<string>) => Set<string>) =>
    setOpenState((s) => {
      const next = f(s);
      rememberedOpen = next;
      return next;
    });
  if (c.accounts.length === 0) {
    return <p className="hd-quiet hd-pad">No account has a snapshot in this range.</p>;
  }
  const toggle = (id: string) =>
    setOpen((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  // Every account starting at its first snapshot is said once, in the hero.
  const allPartial = c.accounts.every((a) => a.partial_start);
  const posOf = (id: string) =>
    positions.state === "ok" ? shown(positions.data.filter((p) => p.account_ids[0] === id)) : [];
  return (
    <div className="hd-groups">
      <div className="hd-cols" aria-hidden="true">
        <span />
        <span>Value</span>
        <span>Change</span>
        <span>Gain</span>
      </div>
      {GROUPS.map(({ group, label, total }) => {
        const rows = c.accounts.filter((a) => a.group === group);
        if (rows.length === 0) return null;
        const owed = group === "credit" || group === "loans";
        const sum = c.end[total];
        return (
          <section key={group} className="hd-group" aria-labelledby={`hd-g-${group}`}>
            <div className="hd-group-head">
              <h3 id={`hd-g-${group}`}>{label}</h3>
              <span className="hd-num hd-group-total">
                {owed ? `${formatCents(Math.abs(sum))} owed` : formatCents(sum)}
              </span>
            </div>
            <ul className="hd-accts">
              {rows
                .slice()
                .sort((x, y) => Math.abs(y.end) - Math.abs(x.end))
                .map((a) => (
                  <AccountRow
                    key={a.account_id}
                    a={a}
                    bal={acctById.get(a.account_id)}
                    owed={owed}
                    live={live}
                    partialNote={a.partial_start && !allPartial}
                    positions={posOf(a.account_id)}
                    expanded={open.has(a.account_id)}
                    onToggle={() => toggle(a.account_id)}
                  />
                ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}

const STALE_SECS = 36 * 3600;

function AccountRow({
  a,
  bal,
  owed,
  live,
  partialNote,
  positions,
  expanded,
  onToggle,
}: {
  a: AccountChange;
  bal: AccountBalance | undefined;
  owed: boolean;
  live: boolean;
  /** This account started later than the rest. */
  partialNote: boolean;
  positions: PositionChange[];
  expanded: boolean;
  onToggle: () => void;
}) {
  const account: Account | undefined = bal?.account;
  const stale = live && account && Date.now() / 1000 - account.last_synced > STALE_SECS;
  const invest = a.group === "investments";
  const held = positions.filter((p) => !p.is_cash || p.end_value !== 0);
  const panel = `hd-pos-${a.account_id}`;
  let gain: React.ReactNode = null;
  if (invest) {
    gain =
      a.gain !== null ? (
        <span className={`hd-num${tone(a.gain)}`}>{signedCents(a.gain)}</span>
      ) : a.gain_reason === "contributions_unknown" ? (
        <span className="hd-quiet">contributions unknown</span>
      ) : a.gain_reason === "cash" ? (
        <span className="hd-quiet">cash</span>
      ) : (
        <span className="hd-quiet">—</span>
      );
  }
  return (
    <li className="hd-acct">
      <div className="hd-acct-main">
        <span className="hd-acct-name">
          {account ? (
            <SourceChip s={accountSource(account)} />
          ) : (
            <span className="lg-src other">{a.account}</span>
          )}
          {account && <span className="hd-quiet">{account.org}</span>}
          {stale && account && (
            <span className="hd-stale">synced {agoLabel(account.last_synced)}</span>
          )}
          {partialNote && <span className="hd-quiet">from first snapshot</span>}
        </span>
        <span className="hd-fig">
            <span className="hd-fig-key">Value</span>
            <span className="hd-num">
              {owed ? `${formatCents(Math.abs(a.end))} owed` : formatCents(a.end)}
            </span>
          </span>
          <span className="hd-fig">
            <span className="hd-fig-key">Change</span>
            {a.change === null ? (
              <span className="hd-quiet">
                <span aria-hidden="true">—</span>
                <span className="lg-sr">no change: one snapshot in this range</span>
              </span>
            ) : owed ? (
              <OwedChange cents={a.change} />
            ) : (
              <span className="hd-num">{signedCents(a.change)}</span>
            )}
          </span>
        {invest ? (
          <span className="hd-fig">
            <span className="hd-fig-key">Gain</span>
            {gain}
          </span>
        ) : (
          <span className="hd-fig" aria-hidden="true" />
        )}
      </div>
      {invest && held.length > 0 && (
        <>
          <button
            className="hd-expand"
            aria-expanded={expanded}
            aria-controls={panel}
            onClick={onToggle}
          >
            {expanded ? "Hide positions" : showLabel(held)}
          </button>
          {expanded && (
            <div id={panel}>
              <PositionTable rows={held} showAccounts={false} />
            </div>
          )}
        </>
      )}
    </li>
  );
}

/** "Show 3 positions", "Show 3 positions and cash". */
function showLabel(held: PositionChange[]): string {
  const n = held.filter((p) => !p.is_cash).length;
  const cash = held.some((p) => p.is_cash);
  if (n === 0) return "Show cash";
  return `Show ${n} position${n === 1 ? "" : "s"}${cash ? " and cash" : ""}`;
}

/** A liability's change in its own frame: what's owed went up or down.
 * `cents` is the effect on net worth (paying down is positive). */
function OwedChange({ cents }: { cents: number }) {
  if (cents === 0) return <span className="hd-num">{formatCents(0)}</span>;
  return (
    <span className="hd-num">
      <span className="lg-sr">owed </span>
      {cents < 0 ? "up " : "down "}
      {formatCents(Math.abs(cents))}
    </span>
  );
}

// ── By position ────────────────────────────────────────────────────────────
function CombinedPositions({
  load,
  busy,
  onRetry,
}: {
  load: Load<PositionChange[]>;
  busy: boolean;
  onRetry: () => void;
}) {
  if (load.state === "loading") {
    return (
      <p className="hd-quiet hd-pad" role="status">
        Loading positions…
      </p>
    );
  }
  if (load.state === "error") {
    return (
      <div className="hd-state" role="alert">
        <p>Couldn't load positions: {load.error}</p>
        <button className="hd-btn" onClick={onRetry}>
          Try again
        </button>
      </div>
    );
  }
  const rows = shown(load.data);
  if (rows.length === 0) {
    return <p className="hd-quiet hd-pad">No positions in this range.</p>;
  }
  return (
    <div className={busy ? "hd-busy" : undefined} aria-busy={busy}>
      <p className="hd-meta hd-pad">
        Each security combined across accounts. Position gains count a reinvested dividend as money
        put in, so they don't add up to the account gains.
      </p>
      <PositionTable rows={rows} showAccounts />
    </div>
  );
}

type PosSort = "label" | "value" | "gain" | "unrealized";
const POS_SORT: Record<PosSort, (p: PositionChange) => number | string | null> = {
  label: (p) => p.label.toLowerCase(),
  value: (p) => p.end_value,
  gain: (p) => p.gain,
  unrealized: (p) => p.unrealized_gain,
};

function PositionTable({
  rows: given,
  showAccounts,
}: {
  rows: PositionChange[];
  showAccounts: boolean;
}) {
  // null = the server's order. Clicking a header sorts by it (names A-Z,
  // figures largest first); clicking again flips it. Unknowns sort last.
  const [sort, setSort] = useState<{ key: PosSort; desc: boolean } | null>(null);
  const rows = useMemo(() => {
    if (!sort) return given;
    const get = POS_SORT[sort.key];
    return [...given].sort((x, y) => {
      const a = get(x);
      const b = get(y);
      if (a === null || b === null) return a === null ? (b === null ? 0 : 1) : -1;
      const c = a < b ? -1 : a > b ? 1 : 0;
      return sort.desc ? -c : c;
    });
  }, [given, sort]);
  const onSort = (key: PosSort) =>
    setSort((s) => (s?.key === key ? { key, desc: !s.desc } : { key, desc: key !== "label" }));
  // A plain function (not a component) so header buttons keep focus.
  const head = (key: PosSort, label: string, right = true) => (
    <th
      scope="col"
      className={right ? "r" : undefined}
      aria-sort={sort?.key === key ? (sort.desc ? "descending" : "ascending") : "none"}
    >
      <button className="fl-sort" onClick={() => onSort(key)}>
        {label}
        <span className="fl-sort-mark" aria-hidden="true">
          {sort?.key === key ? (sort.desc ? "↓" : "↑") : ""}
        </span>
      </button>
    </th>
  );
  // Reasons are footnotes under the table, not hover-only titles.
  const noBasis = rows.some((p) => !p.is_cash && !p.sold && p.cost_basis === null);
  const unknown = rows.some(
    (p) => p.gain === null && p.gain_reason !== "cash" && p.change_reason !== "one_snapshot",
  );
  return (
    <>
    <table className="hd-pos">
      <colgroup>
        <col />
        <col className="hd-col-qty" />
        <col className="hd-col-val" />
        <col className="hd-col-gain" />
        <col className="hd-col-unr" />
      </colgroup>
      <thead>
        <tr>
          {head("label", "Holding", false)}
          <th scope="col" className="r">
            Shares
          </th>
          {head("value", "Value")}
          {head("gain", "Gain this range")}
          {head("unrealized", "Unrealized")}
        </tr>
      </thead>
      <tbody>
        {rows.map((p) => {
          const key = `${p.account_ids.join(",")}:${p.security_id}`;
          const stalePrice =
            p.price_as_of &&
            p.priced_at &&
            dayMs(localDay(p.priced_at)) - dayMs(p.price_as_of) > 3 * DAY_MS;
          return (
            <tr key={key}>
              <td className="hd-hold">
                <span className="hd-ticker">{p.label}</span>
                {p.is_cash && <span className="hd-tag">cash</span>}
                {p.sold && <span className="hd-tag">sold</span>}
                {p.label !== p.name && p.name && <span className="hd-quiet hd-hname">{p.name}</span>}
                {showAccounts && <span className="hd-quiet hd-hname">{p.account}</span>}
              </td>
              <td className="r hd-c-qty" data-label="Shares">
                {p.sold ? <span className="hd-quiet">sold</span> : p.is_cash ? "—" : qtyLabel(p.end_quantity)}
              </td>
              <td className="r hd-c-val" data-label="Value">
                {formatCents(p.end_value)}
                {stalePrice && <span className="hd-stale hd-sub">priced {dayLabel(p.price_as_of!)}</span>}
              </td>
              <td className="r hd-c-gain" data-label="Gain">
                <PositionGain p={p} />
              </td>
              <td className="r hd-c-unr" data-label="Unrealized">
                {p.unrealized_gain !== null && p.cost_basis !== null ? (
                  <span className="hd-unr">
                    <span className={tone(p.unrealized_gain).trim()}>
                      {signedCents(p.unrealized_gain)}
                      {p.unrealized_bp !== null && ` ${signedPct(p.unrealized_bp)}`}
                    </span>
                    <span className="hd-quiet hd-sub">basis {formatCents(p.cost_basis)}</span>
                  </span>
                ) : (
                  <span className="hd-unr hd-quiet">
                    <span aria-hidden="true">—</span>
                    <span className="lg-sr">
                      {p.sold || p.is_cash ? "none" : "no cost basis reported"}
                    </span>
                  </span>
                )}
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
      {(noBasis || unknown) && (
        <p className="hd-meta hd-pos-notes">
          {noBasis && "— under Unrealized: the broker reports no cost basis. "}
          {unknown && "Unknown gain: shares changed without reported trades."}
        </p>
      )}
    </>
  );
}

function PositionGain({ p }: { p: PositionChange }) {
  if (p.gain !== null) {
    return (
      <span className={tone(p.gain).trim()}>
        {signedCents(p.gain)}
        {p.gain_bp !== null && <span className="hd-sub-inline"> {signedPct(p.gain_bp)}</span>}
      </span>
    );
  }
  if (p.gain_reason === "cash") return <span className="hd-quiet">cash</span>;
  if (p.change_reason === "one_snapshot") {
    return (
      <span className="hd-quiet">
        <span aria-hidden="true">—</span>
        <span className="lg-sr">no change: one snapshot in this range</span>
      </span>
    );
  }
  return <span className="hd-quiet">unknown</span>;
}
