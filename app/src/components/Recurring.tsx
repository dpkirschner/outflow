// The recurring list on the Flow screen (GLOSSARY: Recurring). Every figure
// comes from core (GET /api/recurring): the 12-month window, usual monthly
// cost, the change marker and the spark. The browser only sorts and formats.
//
// A row drills the transaction table into that merchant's charges over the
// window, through the same Pick as the lanes. Ignore moves a merchant to the
// ignored list (one mark, no other states), with Undo.
import { useState } from "react";
import type { Change, RecurringMerchant, RecurringView, StreamCadence } from "../types";
import { monthLabel, signedCents, unsignedCents } from "../format";
import type { Pick } from "./Flow";

const PREVIEW = 8;

const CADENCE: Record<StreamCadence, string> = {
  Daily: "about daily",
  FewPerWeek: "a few times a week",
  Weekly: "weekly",
  FewPerMonth: "a few times a month",
  Monthly: "monthly",
  Yearly: "yearly",
};

type SortKey = "monthly" | "change" | "name" | "last";

/** Bigger moves first: New, then by size of the change, Steady last. */
function changeRank(r: RecurringMerchant): number {
  if (r.change === "New") return Number.POSITIVE_INFINITY;
  if (r.change === "Steady") return -1;
  return Math.abs(r.change_pct ?? 0);
}

function compare(k: SortKey, a: RecurringMerchant, b: RecurringMerchant): number {
  switch (k) {
    case "monthly":
      return a.monthly_cents - b.monthly_cents;
    case "change":
      return changeRank(a) - changeRank(b);
    case "name":
      return a.name.localeCompare(b.name);
    case "last":
      return a.last_seen - b.last_seen;
  }
}

/** The change marker in words, never by colour alone. */
function changeText(c: Change, pct: number | null): string {
  if (c === "New") return "new";
  if (c === "Up" && pct !== null) return `up ${Math.round(pct)}%`;
  if (c === "Down" && pct !== null) return `down ${Math.round(-pct)}%`;
  return "steady";
}

function fmtDay(epoch: number): string {
  return new Date(epoch * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** `YYYY-MM` minus `back` months, as (year, month). */
function monthBack(ym: string, back: number): [number, number] {
  const [y, m] = ym.split("-").map(Number);
  const idx = y * 12 + (m - 1) - back;
  return [Math.floor(idx / 12), (idx % 12) + 1];
}

/** "Dec 2025 – Nov 2026", from the view's inclusive bounds. */
export function windowLabel(v: RecurringView): string {
  const at = (d: string) => monthLabel(Number(d.slice(0, 4)), Number(d.slice(5, 7)));
  return `${at(v.since)} – ${at(v.until)}`;
}

function Spark({ r, month, inProgress }: { r: RecurringMerchant; month: string; inProgress: boolean }) {
  const max = Math.max(1, ...r.spark_cents);
  const n = r.spark_cents.length;
  const spoken = r.spark_cents
    .map((c, i) => {
      const [y, m] = monthBack(month, n - 1 - i);
      return `${monthLabel(y, m)} ${unsignedCents(c)}`;
    })
    .join(", ");
  return (
    <span className="fl-rc-spark" role="img" aria-label={`Last ${n} months: ${spoken}`}>
      {r.spark_cents.map((c, i) => (
        <i
          key={i}
          className={inProgress && i === n - 1 ? "sofar" : undefined}
          style={{ height: `${c === 0 ? 1 : Math.max(3, Math.round((c / max) * 22))}px` }}
        />
      ))}
    </span>
  );
}

export function RecurringSection({
  load,
  onRetry,
  pick,
  onPick,
  onIgnore,
}: {
  load: { state: "loading" } | { state: "error"; error: string } | { state: "ok"; data: RecurringView };
  onRetry: () => void;
  pick: Pick | null;
  onPick: (p: Pick) => void;
  onIgnore: (r: RecurringMerchant, ignored: boolean) => Promise<void>;
}) {
  const [sort, setSort] = useState<SortKey>("monthly");
  const [dir, setDir] = useState<"asc" | "desc">("desc");
  const [all, setAll] = useState(false);
  const [showIgnored, setShowIgnored] = useState(false);

  const title = (
    <h2 id="fl-rc-title" className="fl-section-title" tabIndex={-1}>
      Recurring
    </h2>
  );

  if (load.state !== "ok") {
    return (
      <section className="fl-rc" aria-labelledby="fl-rc-title" aria-busy={load.state === "loading"}>
        {title}
        {load.state === "loading" ? (
          <p className="fl-lane-note">Loading recurring merchants…</p>
        ) : (
          <div className="fl-state" role="alert">
            <p>Couldn't load the recurring list: {load.error}</p>
            <button className="lg-pill" onClick={onRetry}>
              Try again
            </button>
          </div>
        )}
      </section>
    );
  }

  const v = load.data;
  const sorted = [...v.items].sort((a, b) => {
    const c = compare(sort, a, b);
    return (dir === "asc" ? c : -c) || a.merchant.localeCompare(b.merchant);
  });
  const shown = all ? sorted : sorted.slice(0, PREVIEW);
  const span = windowLabel(v);

  const onSort = (k: SortKey) => {
    if (sort === k) setDir((d) => (d === "desc" ? "asc" : "desc"));
    else {
      setSort(k);
      setDir(k === "name" ? "asc" : "desc");
    }
  };
  const head = (k: SortKey, label: string, cls?: string) => (
    <th
      scope="col"
      className={cls}
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

  // Ignoring removes the row (and the control that had focus): move focus to
  // the row now in its place, else the section title.
  const ignore = async (r: RecurringMerchant, index: number) => {
    await onIgnore(r, true);
    window.requestAnimationFrame(() => {
      const picks = document.querySelectorAll<HTMLElement>(".fl-rc-table .fl-rc-pick");
      (picks[Math.min(index, picks.length - 1)] ?? document.getElementById("fl-rc-title"))?.focus();
    });
  };

  return (
    <section className="fl-rc" aria-labelledby="fl-rc-title">
      {title}
      <p className="fl-rc-note">
        Merchants you pay on a regular cadence, {span}.
        {v.items.length > 0 && (
          <>
            {" "}
            Usual cost <b>{signedCents(-v.monthly_total_cents)}</b> a month.
          </>
        )}
      </p>

      {v.items.length === 0 ? (
        <p className="fl-rc-empty">
          Nothing recurring in these 12 months yet. A merchant shows up here after three charges on
          a regular cadence.
        </p>
      ) : (
        <>
          <div className="fl-table-wrap">
            <table className="fl-rc-table">
              <caption className="fl-sr">Recurring merchants, {span}</caption>
              <thead>
                <tr>
                  {head("name", "Merchant", "fl-rc-col-name")}
                  <th scope="col" className="fl-rc-col-spark">
                    Last 6 months
                  </th>
                  {head("change", "Change", "fl-rc-col-change")}
                  {head("monthly", "Usual / mo", "num fl-rc-col-monthly")}
                  {head("last", "Last", "fl-rc-col-last")}
                  <th scope="col" className="fl-rc-col-act">
                    <span className="fl-sr">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {shown.map((r, i) => {
                  const target: Pick = { lane: "out", direction: "out", merchant: r.merchant, name: r.name };
                  const on = pick?.merchant === r.merchant;
                  const change = changeText(r.change, r.change_pct);
                  return (
                    <tr key={r.merchant} className={on ? "on" : undefined}>
                      <td className="fl-rc-col-name">
                        <button
                          className={`fl-rc-pick${on ? " on" : ""}`}
                          aria-pressed={on}
                          aria-label={`${r.name}, ${signedCents(-r.monthly_cents)} a month, ${change}: show its charges, ${span}`}
                          onClick={() => onPick(target)}
                        >
                          <span className="fl-rc-name">{r.name}</span>
                          <span className="fl-rc-meta">
                            {CADENCE[r.cadence]}
                            {r.category && ` · ${r.category}`}
                          </span>
                        </button>
                      </td>
                      <td className="fl-rc-col-spark">
                        <Spark r={r} month={v.month} inProgress={v.in_progress} />
                      </td>
                      <td className={`fl-rc-col-change fl-rc-change ${r.change.toLowerCase()}`}>
                        {/* Only a change gets a note; steady is said, not shown. */}
                        {r.change === "Steady" ? <span className="fl-sr">steady</span> : change}
                      </td>
                      <td className="num fl-rc-col-monthly">{signedCents(-r.monthly_cents)}</td>
                      <td className="fl-rc-col-last">{fmtDay(r.last_seen)}</td>
                      <td className="fl-rc-col-act">
                        <button
                          className="fl-mini"
                          onClick={() => void ignore(r, i)}
                          aria-label={`Ignore ${r.name}`}
                        >
                          Ignore
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          {sorted.length > PREVIEW && (
            <button className="fl-more" aria-expanded={all} onClick={() => setAll((a) => !a)}>
              {all ? "Show fewer" : `Show all ${sorted.length} merchants`}
            </button>
          )}
        </>
      )}

      {v.ignored.length > 0 && (
        <div className="fl-rc-ignored">
          <button
            className="fl-more"
            aria-expanded={showIgnored}
            aria-controls="fl-rc-ignored-list"
            onClick={() => setShowIgnored((s) => !s)}
          >
            {showIgnored ? "Hide ignored" : `${v.ignored.length} ignored`}
          </button>
          {showIgnored && (
            <ul id="fl-rc-ignored-list" className="fl-rows" aria-label="Ignored merchants">
              {v.ignored.map((r) => (
                <li key={r.merchant} className="fl-row">
                  <span className="fl-row-label">
                    {r.name}
                    <span className="fl-sub">{CADENCE[r.cadence]}</span>
                  </span>
                  <span className="fl-rc-restore">
                    <span className="fl-row-value">{signedCents(-r.monthly_cents)} / mo</span>
                    <button
                      className="fl-mini fl-mini-shown"
                      onClick={() => void onIgnore(r, false)}
                      aria-label={`Restore ${r.name} to the recurring list`}
                    >
                      Restore
                    </button>
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </section>
  );
}
