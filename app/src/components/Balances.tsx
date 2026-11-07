// Balances: the current state of every visible account — not money movement.
// Totals, net-worth and cash trend charts from per-sync snapshots, an
// idle-cash callout (investment accounts sitting on cash), and per-group
// account rows; investment rows expand to their holdings. All math is
// server-side (core::balances); this only renders it.
import { useCallback, useEffect, useState } from "react";
import {
  CartesianGrid,
  Line,
  LineChart,
  ResponsiveContainer,
  Tooltip,
  XAxis,
  YAxis,
} from "recharts";
import { api } from "../api";
import type { AccountBalance, BalanceGroup, BalancesView, DayTotals, HoldingRow } from "../types";
import { SourceChip, accountSource, agoLabel } from "./accounts";
import type { ToastMsg } from "./Toast";

const GROUP_LABEL: Record<BalanceGroup, string> = {
  cash: "Cash",
  investments: "Investments",
  credit: "Credit",
  loans: "Loans",
  other: "Other",
};

// Signed whole dollars: "$1,310", "−$410".
function money(cents: number): string {
  const s = "$" + Math.round(Math.abs(cents) / 100).toLocaleString();
  return cents < 0 ? `−${s}` : s;
}

function pct(bp: number): string {
  return `${(bp / 100).toFixed(1)}%`;
}

export function Balances({ notify }: { notify: (msg: ToastMsg) => void }) {
  const [view, setView] = useState<BalancesView | null>(null);
  const [history, setHistory] = useState<DayTotals[]>([]);
  const [open, setOpen] = useState<Set<string>>(new Set());

  const refresh = useCallback(async () => {
    try {
      const [v, h] = await Promise.all([api.balances(), api.balanceHistory()]);
      setView(v);
      setHistory(h);
    } catch (err) {
      notify({ kind: "err", text: `Load failed: ${String(err)}` });
    }
  }, [notify]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (!view) return <div className="lg-empty">Loading balances…</div>;
  const all = view.groups.flatMap((g) => g.accounts);
  if (all.length === 0) {
    return (
      <div className="lg-empty">No accounts yet — connect a bank on the Connections tab.</div>
    );
  }
  const idle = all.filter((a) => a.idle_cash);
  const lastSync = Math.max(...all.map((a) => a.account.last_synced));

  const toggle = (id: string) =>
    setOpen((s) => {
      const next = new Set(s);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div className="bl-wrap">
      <div className="cx-head">
        <h2>Balances</h2>
        <span className="bl-asof">as of last sync {agoLabel(lastSync)}</span>
      </div>

      <div className="bl-totals">
        <Stat label="Net worth" cents={view.totals.net_worth} />
        <Stat label="Cash" cents={view.totals.cash} />
        <Stat label="Invested" cents={view.totals.invested} />
        <Stat label="Credit owed" cents={view.totals.credit} />
        {view.totals.loans !== 0 && <Stat label="Loans" cents={view.totals.loans} />}
        {view.totals.other !== 0 && <Stat label="Other" cents={view.totals.other} />}
      </div>

      <Trend days={history} />

      {idle.length > 0 && (
        <div className="bl-idle">
          <div className="bl-idle-title">Idle cash in investment accounts</div>
          {idle.map((a) => (
            <div className="bl-idle-row" key={a.account.id}>
              <SourceChip s={accountSource(a.account)} />
              <span className="bl-idle-amt">{money(a.cash)}</span>
              <span className="bl-dim">{pct(a.cash_bp)} of the account isn't invested</span>
            </div>
          ))}
        </div>
      )}

      {view.groups.map((g) => (
        <div className="bl-group" key={g.group}>
          <div className="bl-group-head">
            <span>{GROUP_LABEL[g.group]}</span>
            <span className="bl-num">{money(g.total)}</span>
          </div>
          <ul className="bl-accts">
            {g.accounts.map((a) => (
              <AccountRow
                key={a.account.id}
                a={a}
                expanded={open.has(a.account.id)}
                onToggle={() => toggle(a.account.id)}
              />
            ))}
          </ul>
        </div>
      ))}
    </div>
  );
}

function Stat({ label, cents }: { label: string; cents: number }) {
  return (
    <div className="bl-stat">
      <div className="bl-stat-label">{label}</div>
      <div className={`bl-stat-val${cents < 0 ? " bl-neg" : ""}`}>{money(cents)}</div>
    </div>
  );
}

type SortKey = "label" | "quantity" | "price" | "value" | "share" | "gain";
type Sort = { key: SortKey; dir: "asc" | "desc" };

const COLUMNS: { key: SortKey; label: string; cls: string }[] = [
  { key: "label", label: "Holding", cls: "" },
  { key: "quantity", label: "Shares", cls: "r c-qty" },
  { key: "price", label: "Price", cls: "r c-price" },
  { key: "value", label: "Value", cls: "r c-val" },
  { key: "share", label: "Share", cls: "r c-share" },
  { key: "gain", label: "Gain", cls: "r c-gain" },
];

// Sort key per column. Quantity/price arrive as exact decimal strings; the
// float here only orders rows — display still uses the exact text.
function sortValue(h: HoldingRow, key: SortKey): number | string | null {
  switch (key) {
    case "label":
      return h.label.toLowerCase();
    case "quantity":
      return Number(h.quantity);
    case "price":
      return Number(h.price);
    case "value":
      return h.value;
    case "share":
      return h.share_bp;
    case "gain":
      return h.gain;
  }
}

function sortHoldings(rows: HoldingRow[], { key, dir }: Sort): HoldingRow[] {
  const sign = dir === "asc" ? 1 : -1;
  return [...rows].sort((x, y) => {
    const a = sortValue(x, key);
    const b = sortValue(y, key);
    // Unknown (no cost basis → no gain) always sinks to the bottom.
    if (a === null || b === null) return a === b ? 0 : a === null ? 1 : -1;
    const c = typeof a === "string" ? a.localeCompare(b as string) : a - (b as number);
    return c * sign || y.value - x.value;
  });
}

function AccountRow({
  a,
  expanded,
  onToggle,
}: {
  a: AccountBalance;
  expanded: boolean;
  onToggle: () => void;
}) {
  const invest = a.group === "investments";
  const canExpand = a.holdings.length > 0;
  // Largest position first by default; per account, kept while collapsed.
  const [sort, setSort] = useState<Sort>({ key: "value", dir: "desc" });
  // Same column flips direction; a new column starts A→Z for names and
  // largest-first for numbers.
  const sortBy = (key: SortKey) =>
    setSort((s) =>
      s.key === key
        ? { key, dir: s.dir === "asc" ? "desc" : "asc" }
        : { key, dir: key === "label" ? "asc" : "desc" },
    );
  return (
    <li className={`bl-acct${a.idle_cash ? " idle" : ""}`}>
      <div
        className={`bl-acct-main${canExpand ? " clickable" : ""}`}
        onClick={canExpand ? onToggle : undefined}
      >
        <span className="bl-caret">{canExpand ? (expanded ? "▾" : "▸") : ""}</span>
        <SourceChip s={accountSource(a.account)} />
        <span className="bl-org bl-dim">{a.account.org}</span>
        {invest && (
          <span className="bl-cash bl-dim">
            cash {money(a.cash)} · {pct(a.cash_bp)}
            {canExpand ? ` · ${a.holdings.length} positions` : ""}
          </span>
        )}
        <span className={`bl-num${a.account.balance < 0 ? " bl-neg" : ""}`}>
          {money(a.account.balance)}
        </span>
      </div>
      {expanded && (
        <div className="bl-hold-scroll">
          <table className="bl-hold">
            <thead>
              <tr>
                {COLUMNS.map((c) => {
                  const on = sort.key === c.key;
                  return (
                    <th
                      key={c.key}
                      className={`${c.cls} bl-sortable${on ? " on" : ""}`}
                      aria-sort={on ? (sort.dir === "asc" ? "ascending" : "descending") : "none"}
                      onClick={() => sortBy(c.key)}
                    >
                      {c.label}
                      <span className="bl-sort-ind">{on ? (sort.dir === "asc" ? "▲" : "▼") : ""}</span>
                    </th>
                  );
                })}
              </tr>
            </thead>
            <tbody>
              {sortHoldings(a.holdings, sort).map((h) => (
                <tr key={h.security_id}>
                  <td className="bl-hcell" title={h.label !== h.name ? `${h.label} · ${h.name}` : h.name}>
                    <span className="bl-ticker">{h.label}</span>
                    {h.is_cash && <span className="bl-cash-tag">cash</span>}
                    {h.label !== h.name && <span className="bl-dim bl-hname">{h.name}</span>}
                  </td>
                  <td className="r c-qty">{h.quantity}</td>
                  <td className="r c-price">${h.price}</td>
                  <td className="r c-val">{money(h.value)}</td>
                  <td className="r c-share bl-dim">{pct(h.share_bp)}</td>
                  <td className={`r c-gain${h.gain === null ? " bl-dim" : h.gain < 0 ? " bl-neg" : " bl-pos"}`}>
                    {h.gain === null ? "—" : money(h.gain)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </li>
  );
}

// Net worth and cash over time: two side-by-side charts, each on its own
// y-scale (fit to the visible data, not anchored at $0) so small moves on a
// large total still show. X is real dates, so sync gaps read as gaps. A range
// longer than the history clips to the first snapshot and fills the chart.
const RANGES = [
  { key: "1W", days: 7 },
  { key: "1M", days: 30 },
  { key: "3M", days: 91 },
  { key: "6M", days: 182 },
  { key: "1Y", days: 365 },
] as const;
type RangeKey = (typeof RANGES)[number]["key"];

const DAY_MS = 86_400_000;
// SVG presentation attributes don't resolve CSS vars; these mirror ledger.css.
const C_LINE = "#1e2a38";
const C_DIM = "#8b98a9";
const C_NW = "#2dd4bf";
const C_CASH = "#f5d38a";

type Field = "net_worth" | "cash";
type Pt = { t: number; day: string; net_worth: number; cash: number };

// "YYYY-MM-DD" → UTC midnight ms; formatting below is UTC too, so no TZ drift.
const dayMs = (day: string) => Date.parse(`${day}T00:00:00Z`);

// "Nov 3", or "Nov '26" for long ranges.
function tickDate(t: number, long: boolean): string {
  const d = new Date(t);
  const mon = d.toLocaleDateString("en-US", { month: "short", timeZone: "UTC" });
  return long ? `${mon} '${String(d.getUTCFullYear()).slice(2)}` : `${mon} ${d.getUTCDate()}`;
}

// Compact axis dollars: "$412k", "$322.5k", "$1.25M", "−$950". Up to two
// decimals, trailing zeros dropped, so round-number ticks read exactly.
function axisMoney(cents: number): string {
  const d = Math.abs(cents) / 100;
  const sign = cents < 0 ? "−" : "";
  const short = (n: number) => String(Number(n.toFixed(2)));
  if (d >= 1e6) return `${sign}$${short(d / 1e6)}M`;
  if (d >= 1e3) return `${sign}$${short(d / 1e3)}k`;
  return `${sign}$${Math.round(d)}`;
}

// ~5 evenly spaced whole-day ticks, counted back from the newest day so the
// latest snapshot is always labelled; the oldest day gets a label too unless
// it would crowd its neighbour.
function dayTicks(lo: number, hi: number): number[] {
  const days = Math.round((hi - lo) / DAY_MS);
  const step = Math.max(1, Math.ceil(days / 4));
  const out: number[] = [];
  for (let k = 0; k <= days; k += step) out.unshift(hi - k * DAY_MS);
  if (out[0] !== lo && out[0] - lo >= step * DAY_MS * 0.6) out.unshift(lo);
  return out;
}

// Round-number y ticks (1/2/2.5/5 × 10ⁿ) covering [min, max] in ~3 steps.
// Returns the ticks; the axis domain is their first and last.
function moneyTicks(min: number, max: number): number[] {
  // Flat or near-flat data: still show a band of at least ~$50 or 0.5%.
  const span = Math.max(max - min, Math.abs(max) * 0.005, 5_000);
  const raw = span / 3;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((s) => s >= raw)!;
  const mid = (min + max) / 2;
  let lo = Math.floor(Math.min(min, mid - span / 2) / step) * step;
  let hi = Math.ceil(Math.max(max, mid + span / 2) / step) * step;
  // Keep lines off the very edge: pad by a step only if they sit on it.
  if (min - lo < step * 0.03) lo -= step;
  if (hi - max < step * 0.03) hi += step;
  const out: number[] = [];
  for (let v = lo; v <= hi + step / 2; v += step) out.push(Math.round(v));
  return out;
}

function Trend({ days }: { days: DayTotals[] }) {
  const [range, setRange] = useState<RangeKey>("1M");
  if (days.length < 2) {
    return (
      <div className="bl-trend bl-dim">
        {days.length === 0
          ? "The trend starts with the next sync — every sync records a snapshot."
          : `History starts ${days[0].day}; the trend fills in as syncs accumulate.`}
      </div>
    );
  }
  const all: Pt[] = days.map((d) => ({
    t: dayMs(d.day),
    day: d.day,
    net_worth: d.net_worth,
    cash: d.cash,
  }));
  const end = all[all.length - 1].t;
  const want = end - RANGES.find((r) => r.key === range)!.days * DAY_MS;
  const clipped = want < all[0].t;
  const start = clipped ? all[0].t : want;
  const pts = all.filter((p) => p.t >= start);
  const spanDays = Math.round((end - start) / DAY_MS) + 1;

  return (
    <div className="bl-trend">
      <div className="bl-trend-head">
        <div className="lg-win bl-range" role="group" aria-label="Trend range">
          {RANGES.map((r) => (
            <button
              key={r.key}
              className={r.key === range ? "on" : ""}
              aria-pressed={r.key === range}
              onClick={() => setRange(r.key)}
            >
              {r.key}
            </button>
          ))}
        </div>
        {clipped && <span className="bl-dim">showing all {spanDays} days of history</span>}
      </div>
      {pts.length < 2 ? (
        <div className="bl-dim bl-trend-empty">
          Only one snapshot in this range ({pts[0]?.day ?? "none"}) — pick a longer range.
        </div>
      ) : (
        <div className="bl-charts">
          <TrendChart label="Net worth" field="net_worth" color={C_NW} pts={pts} lo={start} hi={end} />
          <TrendChart label="Cash" field="cash" color={C_CASH} pts={pts} lo={start} hi={end} />
        </div>
      )}
    </div>
  );
}

function TrendChart({
  label,
  field,
  color,
  pts,
  lo,
  hi,
}: {
  label: string;
  field: Field;
  color: string;
  pts: Pt[];
  lo: number;
  hi: number;
}) {
  const vals = pts.map((p) => p[field]);
  const min = Math.min(...vals);
  const max = Math.max(...vals);
  const yTicks = moneyTicks(min, max);
  const delta = vals[vals.length - 1] - vals[0];
  const longTicks = hi - lo > 180 * DAY_MS;
  return (
    <div className="bl-chart">
      <div className="bl-chart-head">
        <span>
          <i className="bl-key" style={{ background: color }} /> {label}
        </span>
        <span className={delta < 0 ? "bl-neg" : delta > 0 ? "bl-pos" : "bl-dim"}>
          {delta > 0 ? "+" : ""}
          {money(delta)}
        </span>
      </div>
      <ResponsiveContainer width="100%" height={180}>
        <LineChart data={pts} margin={{ top: 6, right: 12, bottom: 0, left: 0 }}>
          <CartesianGrid stroke={C_LINE} vertical={false} />
          <XAxis
            dataKey="t"
            type="number"
            scale="time"
            domain={[lo, hi]}
            ticks={dayTicks(lo, hi)}
            tickFormatter={(t: number) => tickDate(t, longTicks)}
            tick={{ fill: C_DIM, fontSize: 11 }}
            stroke={C_LINE}
            tickLine={false}
          />
          <YAxis
            domain={[yTicks[0], yTicks[yTicks.length - 1]]}
            ticks={yTicks}
            interval={0}
            tickFormatter={axisMoney}
            tick={{ fill: C_DIM, fontSize: 11 }}
            tickLine={false}
            axisLine={false}
            width={52}
          />
          <Tooltip
            content={<TrendTip title={label} field={field} />}
            cursor={{ stroke: C_DIM, strokeDasharray: "3 3" }}
            isAnimationActive={false}
          />
          <Line
            type="monotone"
            dataKey={field}
            stroke={color}
            strokeWidth={2}
            dot={pts.length <= 31 ? { r: 2, fill: color, strokeWidth: 0 } : false}
            activeDot={{ r: 4 }}
            isAnimationActive={false}
          />
        </LineChart>
      </ResponsiveContainer>
    </div>
  );
}

// `active`/`payload` are injected by recharts' Tooltip. So is `label` (the raw
// x value), which would clobber a prop of that name, hence `title`.
function TrendTip({
  active,
  payload,
  title,
  field,
}: {
  active?: boolean;
  payload?: { payload: Pt }[];
  title: string;
  field: Field;
}) {
  if (!active || !payload?.length) return null;
  const p = payload[0].payload;
  return (
    <div className="bl-tip">
      <div className="bl-dim">{p.day}</div>
      <div>
        {title} <b>{money(p[field])}</b>
      </div>
    </div>
  );
}

