// Balances: the current state of every visible account — not money movement.
// Totals, a net-worth/cash trend from per-sync snapshots, an idle-cash
// callout (investment accounts sitting on cash), and per-group account rows;
// investment rows expand to their holdings. All math is server-side
// (core::balances); this only renders it.
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { AccountBalance, BalanceGroup, BalancesView, DayTotals } from "../types";
import { accountSource, agoLabel } from "./ledger/labels";
import { SourceChip } from "./ledger/Streams";
import type { ToastMsg } from "./Toast";

const GROUP_LABEL: Record<BalanceGroup, string> = {
  cash: "Cash",
  investments: "Investments",
  credit: "Credit",
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
                <th>Holding</th>
                <th className="r c-qty">Shares</th>
                <th className="r c-price">Price</th>
                <th className="r c-val">Value</th>
                <th className="r c-share">Share</th>
                <th className="r c-gain">Gain</th>
              </tr>
            </thead>
            <tbody>
              {a.holdings.map((h) => (
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

// Net worth + cash over time as a small inline SVG (no chart library). Both
// lines share one y-scale so the gap between them reads as "invested".
function Trend({ days }: { days: DayTotals[] }) {
  if (days.length < 2) {
    return (
      <div className="bl-trend bl-dim">
        {days.length === 0
          ? "The trend starts with the next sync — every sync records a snapshot."
          : `History starts ${days[0].day}; the trend fills in as syncs accumulate.`}
      </div>
    );
  }
  const W = 600;
  const H = 90;
  const vals = days.flatMap((d) => [d.net_worth, d.cash]);
  const lo = Math.min(0, ...vals);
  const hi = Math.max(...vals);
  const span = hi - lo || 1;
  const pts = (pick: (d: DayTotals) => number) =>
    days
      .map((d, i) => {
        const x = (i / (days.length - 1)) * W;
        const y = H - ((pick(d) - lo) / span) * H;
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(" ");
  const first = days[0];
  const last = days[days.length - 1];
  const delta = last.net_worth - first.net_worth;
  return (
    <div className="bl-trend">
      <div className="bl-trend-legend">
        <span>
          <i className="bl-key nw" /> net worth
        </span>
        <span>
          <i className="bl-key cash" /> cash
        </span>
        <span className="bl-dim">
          {first.day} → {last.day} · {delta >= 0 ? "+" : ""}
          {money(delta)}
        </span>
      </div>
      <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="bl-trend-svg">
        <polyline className="nw" points={pts((d) => d.net_worth)} />
        <polyline className="cash" points={pts((d) => d.cash)} />
      </svg>
    </div>
  );
}
