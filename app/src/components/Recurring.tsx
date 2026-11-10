// The recurring list on the Flow screen (GLOSSARY: Recurring). Every figure
// comes from core (GET /api/recurring): the 12-month window, usual monthly
// cost, the change marker and the spark. The browser only sorts and formats.
//
// A row drills the transaction table into that merchant's charges over the
// window, through the same Pick as the lanes. Ignore moves a merchant to the
// ignored list (one mark, no other states), with Undo. The Subscriptions
// switch asks core for steady merchants only (GLOSSARY: Steady); the total
// follows, the ignored list doesn't.
import { useRef, useState } from "react";
import type { RecurringMerchant, RecurringView, StreamCadence } from "../types";
import {
  everyFewMonths,
  formatCentsShort,
  monthLabel,
  signedCents,
  unsignedCents,
} from "../format";
import type { Pick } from "./Flow";

const PREVIEW = 8;
/** Mirrors core `recurring::CHANGE_PCT`, for the note's wording only. */
const CHANGE_PCT = 25;
/** Mirrors core `recurring::STEADY_PCT`, for the note's wording only. */
const STEADY_PCT = 5;

const CADENCE: Record<Exclude<StreamCadence, "EveryFewMonths">, string> = {
  Daily: "about daily",
  FewPerWeek: "a few times a week",
  Weekly: "weekly",
  FewPerMonth: "a few times a month",
  Monthly: "monthly",
  Yearly: "yearly",
};

/** Slower than monthly: judged charge to charge (core `is_slow`). */
function isSlow(r: RecurringMerchant): boolean {
  return r.cadence === "EveryFewMonths" || r.cadence === "Yearly";
}

function cadenceText(r: RecurringMerchant): string {
  return r.cadence === "EveryFewMonths"
    ? everyFewMonths(r.gap_days)
    : CADENCE[r.cadence];
}

/** Spend over the last 12 months against the 12 before; only when the
 *  archive reaches back that far, and only when it moved (or the merchant is
 *  slower than monthly, where it's the trend to watch). `short` sits under
 *  the usual cost (the header names the window); `spoken` has the amount. */
function yearText(
  r: RecurringMerchant,
): { short: string; spoken: string } | null {
  const prior = r.prior_year_cents;
  if (prior === null || prior <= 0) return null;
  const pct = Math.round(((r.year_cents - prior) / prior) * 100);
  if (Math.abs(pct) < 10 && !isSlow(r)) return null;
  const short =
    Math.abs(pct) < 5
      ? "same as last year"
      : `${pct > 0 ? "up" : "down"} ${Math.abs(pct)}% on last year`;
  return {
    short,
    spoken: `${unsignedCents(r.year_cents)} over the last 12 months, ${short}`,
  };
}

type SortKey = "monthly" | "change" | "name" | "last";

/** Bigger moves first: New, then by size of the change, Steady last. */
function changeRank(r: RecurringMerchant): number {
  if (r.change === "New") return Number.POSITIVE_INFINITY;
  if (r.change === "Steady") return -1;
  if (r.change === "Late") return 0;
  return Math.abs(r.change_pct ?? 0);
}

function compare(
  k: SortKey,
  a: RecurringMerchant,
  b: RecurringMerchant,
): number {
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

/** On a small base a % runs wild ("up 1440%"): say the dollars instead
 *  (Check-in's rule). */
const TINY_BASE_CENTS = 20_00;

/** The change marker in words, never by colour alone. */
function changeText(r: RecurringMerchant): string {
  const { change: c, change_pct: pct, change_base_cents: base } = r;
  if (c === "New") return "new";
  if (c === "Late") return "late";
  if ((c === "Up" || c === "Down") && pct !== null) {
    const dir = c === "Up" ? "up" : "down";
    if (base !== null && base < TINY_BASE_CENTS)
      return `${dir} ${formatCentsShort(Math.round((base * Math.abs(pct)) / 100))}`;
    return `${dir} ${Math.round(Math.abs(pct))}%`;
  }
  return "steady";
}

/** What an Up/Down % compares with, so it can't seem to contradict the
 *  usual-per-month column: the months before the last one, or for a slower
 *  merchant the charges before the latest. */
function baseText(r: RecurringMerchant): string | null {
  if (
    (r.change !== "Up" && r.change !== "Down") ||
    r.change_base_cents === null
  )
    return null;
  return `from ${unsignedCents(r.change_base_cents)} ${isSlow(r) ? "a charge" : "a month"}`;
}

function fmtDay(epoch: number): string {
  return new Date(epoch * 1000).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/** `YYYY-MM` minus `back` months, as (year, month). */
function monthBack(ym: string, back: number): [number, number] {
  const [y, m] = ym.split("-").map(Number);
  const idx = y * 12 + (m - 1) - back;
  return [Math.floor(idx / 12), (idx % 12) + 1];
}

/** "Dec 2025 – Nov 2026", from the view's inclusive bounds. */
export function windowLabel(v: RecurringView): string {
  const at = (d: string) =>
    monthLabel(Number(d.slice(0, 4)), Number(d.slice(5, 7)));
  return `${at(v.since)} – ${at(v.until)}`;
}

/** A slower merchant's last charges, one dot each at its amount, so a
 *  dearer visit shows instead of months that are empty by design. Dots, not
 *  bars, and no baseline: the axis is charges, not time. A late merchant
 *  gets a hollow dot after a gap, where the next charge should have been. */
function ChargeSpark({ r }: { r: RecurringMerchant }) {
  const max = Math.max(1, ...r.recent_charges.map((c) => c.cents));
  const n = r.recent_charges.length;
  const late = r.change === "Late";
  const lift = (cents: number) => Math.round((cents / max) * 17);
  const last = r.recent_charges[n - 1];
  const spoken = `Last ${n} charges: ${r.recent_charges
    .map((c) => `${fmtDay(c.at)} ${unsignedCents(c.cents)}`)
    .join(", ")}${late ? "; none since, late" : ""}`;
  return (
    <span
      className="fl-rc-spark fl-rc-spark-charges"
      role="img"
      aria-label={spoken}
      title={spoken}
    >
      {r.recent_charges.map((c) => (
        <i key={c.at} style={{ marginBottom: `${lift(c.cents)}px` }} />
      ))}
      {late && last && (
        <i
          className="missed"
          style={{ marginBottom: `${lift(last.cents)}px` }}
        />
      )}
    </span>
  );
}

function Spark({
  r,
  month,
  inProgress,
}: {
  r: RecurringMerchant;
  month: string;
  inProgress: boolean;
}) {
  if (isSlow(r) && r.recent_charges.length > 0) return <ChargeSpark r={r} />;
  const max = Math.max(1, ...r.spark_cents);
  const n = r.spark_cents.length;
  const spoken = r.spark_cents
    .map((c, i) => {
      const [y, m] = monthBack(month, n - 1 - i);
      return `${monthLabel(y, m)} ${unsignedCents(c)}`;
    })
    .join(", ");
  return (
    <span
      className="fl-rc-spark"
      role="img"
      aria-label={`Last ${n} months: ${spoken}`}
      title={`Last ${n} months: ${spoken}`}
    >
      {r.spark_cents.map((c, i) => (
        <i
          key={i}
          className={inProgress && i === n - 1 ? "sofar" : undefined}
          style={{
            height: `${c === 0 ? (inProgress && i === n - 1 ? 6 : 2) : Math.max(3, Math.round((c / max) * 22))}px`,
          }}
        />
      ))}
    </span>
  );
}

export function RecurringSection({
  load,
  onRetry,
  subscriptions,
  onSubscriptions,
  pick,
  onPick,
  onIgnore,
}: {
  load:
    | { state: "loading" }
    | { state: "error"; error: string }
    | { state: "ok"; data: RecurringView };
  onRetry: () => void;
  /** Steady merchants only (the list and its total come from core). */
  subscriptions: boolean;
  onSubscriptions: (on: boolean) => void;
  pick: Pick | null;
  onPick: (p: Pick) => void;
  /** Resolves true when the mark was saved. */
  onIgnore: (r: RecurringMerchant, ignored: boolean) => Promise<boolean>;
}) {
  const root = useRef<HTMLElement>(null);
  const [sort, setSort] = useState<SortKey>("monthly");
  const [dir, setDir] = useState<"asc" | "desc">("desc");
  const [all, setAll] = useState(false);
  const [showIgnored, setShowIgnored] = useState(false);

  const title = (
    <div className="fl-rc-head">
      <h2 id="fl-rc-title" className="fl-section-title" tabIndex={-1}>
        Recurring
      </h2>
      <div className="lg-win" role="group" aria-label="Show">
        <button
          className={subscriptions ? "" : "on"}
          aria-pressed={!subscriptions}
          onClick={() => onSubscriptions(false)}
        >
          All
        </button>
        <button
          className={subscriptions ? "on" : ""}
          aria-pressed={subscriptions}
          onClick={() => onSubscriptions(true)}
        >
          Subscriptions
        </button>
      </div>
    </div>
  );

  if (load.state !== "ok") {
    return (
      <section
        className="fl-rc"
        aria-labelledby="fl-rc-title"
        aria-busy={load.state === "loading"}
      >
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
      aria-sort={
        sort === k ? (dir === "asc" ? "ascending" : "descending") : "none"
      }
    >
      <button className="fl-sort" onClick={() => onSort(k)}>
        {label}
        <span className="fl-sort-mark" aria-hidden="true">
          {sort === k ? (dir === "desc" ? "↓" : "↑") : ""}
        </span>
      </button>
    </th>
  );

  // Ignore and Restore remove the control that had focus. Move focus to the
  // control now in its place, else the next sensible one, else the title.
  const refocus = (selectors: string[], index: number) =>
    window.requestAnimationFrame(() => {
      for (const sel of selectors) {
        const els = root.current?.querySelectorAll<HTMLElement>(sel) ?? [];
        if (els.length > 0) {
          els[Math.min(index, els.length - 1)].focus();
          return;
        }
      }
      document.getElementById("fl-rc-title")?.focus();
    });
  const ignore = async (r: RecurringMerchant, index: number) => {
    if (await onIgnore(r, true)) refocus([".fl-rc-table .fl-rc-pick"], index);
  };
  const restore = async (r: RecurringMerchant, index: number) => {
    if (await onIgnore(r, false)) {
      refocus(
        [".fl-rc-ignored .fl-mini", ".fl-rc-ignored-toggle", ".fl-rc-pick"],
        index,
      );
    }
  };

  return (
    <section className="fl-rc" aria-labelledby="fl-rc-title" ref={root}>
      {title}
      <p className="fl-rc-note">
        {subscriptions
          ? `Subscriptions: monthly or yearly merchants whose last three charges are within ${STEADY_PCT}% of each other, ${span}.`
          : `Merchants you pay on a regular cadence, ${span}.`}
        {v.items.length > 0 && (
          <>
            {" "}
            <span className="fl-nowrap">
              {subscriptions ? "Together" : "Usual cost"}{" "}
              <b>{signedCents(-v.monthly_total_cents)}</b> a month.
            </span>{" "}
            Marked: new, late, and moves over {CHANGE_PCT}% (charge to charge,
            and bars per charge, for merchants slower than monthly).
          </>
        )}
      </p>

      {v.items.length === 0 ? (
        <p className="fl-rc-empty">
          {subscriptions
            ? `No steady subscriptions in these 12 months. A monthly or yearly merchant shows up here once its last three charges are within ${STEADY_PCT}% of each other.`
            : "Nothing recurring in these 12 months yet. A merchant shows up here after three charges on a regular cadence."}
        </p>
      ) : (
        <>
          <div className="fl-table-wrap">
            <table className="fl-rc-table">
              <caption className="fl-sr">
                {subscriptions ? "Subscriptions" : "Recurring merchants"},{" "}
                {span}
              </caption>
              <thead>
                <tr>
                  {head("name", "Merchant", "fl-rc-col-name")}
                  <th scope="col" className="fl-rc-col-spark">
                    Recent
                    <span className="fl-sr">
                      : last 6 months, or last charges for slower merchants
                    </span>
                  </th>
                  {head("change", "Change", "fl-rc-col-change")}
                  {head("monthly", "Usual per month", "num fl-rc-col-monthly")}
                  {head("last", "Last charge", "fl-rc-col-last")}
                  <th scope="col" className="fl-rc-col-act">
                    <span className="fl-sr">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {shown.map((r, i) => {
                  const target: Pick = {
                    lane: "out",
                    direction: "out",
                    merchant: r.merchant,
                    name: r.name,
                  };
                  const on = pick?.merchant === r.merchant;
                  const change = changeText(r);
                  const base = baseText(r);
                  const year = yearText(r);
                  return (
                    <tr key={r.merchant} className={on ? "on" : undefined}>
                      <td className="fl-rc-col-name">
                        <button
                          className={`fl-rc-pick${on ? " on" : ""}`}
                          aria-pressed={on}
                          aria-label={`${r.name}, ${cadenceText(r)}, ${signedCents(-r.monthly_cents)} a month, ${change}${base ? ` ${base}` : ""}${year ? `, ${year.spoken}` : ""}: show its charges, ${span}`}
                          onClick={() => onPick(target)}
                        >
                          <span className="fl-rc-name">{r.name}</span>
                          <span className="fl-rc-meta">
                            {cadenceText(r)}
                            {r.category && ` · ${r.category}`}
                          </span>
                        </button>
                      </td>
                      <td className="fl-rc-col-spark">
                        <Spark
                          r={r}
                          month={v.month}
                          inProgress={v.in_progress}
                        />
                      </td>
                      <td
                        className={`fl-rc-col-change fl-rc-change ${r.change.toLowerCase()}`}
                      >
                        {/* Only a change gets a note; steady is said, not shown. */}
                        {r.change === "Steady" ? (
                          <span className="fl-sr">steady</span>
                        ) : (
                          change
                        )}
                        {base && <span className="fl-rc-base">{base}</span>}
                      </td>
                      <td className="num fl-rc-col-monthly">
                        {signedCents(-r.monthly_cents)}
                        {year && (
                          <span className="fl-rc-year">{year.short}</span>
                        )}
                      </td>
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
        </>
      )}

      {(sorted.length > PREVIEW || v.ignored.length > 0) && (
        <div className="fl-rc-foot">
          {sorted.length > PREVIEW && (
            <button
              className="fl-more"
              aria-expanded={all}
              onClick={() => setAll((a) => !a)}
            >
              {all ? "Show fewer" : `Show all ${sorted.length} merchants`}
            </button>
          )}
          {v.ignored.length > 0 && (
            <button
              className="fl-rc-ignored-toggle"
              aria-expanded={showIgnored}
              aria-controls="fl-rc-ignored-list"
              onClick={() => setShowIgnored((x) => !x)}
            >
              {showIgnored ? "Hide ignored" : `${v.ignored.length} ignored`}
            </button>
          )}
        </div>
      )}

      {v.ignored.length > 0 && showIgnored && (
        <ul
          id="fl-rc-ignored-list"
          className="fl-rows fl-rc-ignored"
          aria-label="Ignored merchants"
        >
          {v.ignored.map((r, i) => (
            <li key={r.merchant} className="fl-row">
              <span className="fl-row-label">
                {r.name}
                <span className="fl-sub">
                  {cadenceText(r)}
                  {r.category && ` · ${r.category}`}
                </span>
              </span>
              <span className="fl-rc-restore">
                <span className="fl-row-value">
                  {signedCents(-r.monthly_cents)} a month
                </span>
                <button
                  className="fl-mini fl-mini-shown"
                  onClick={() => void restore(r, i)}
                  aria-label={`Restore ${r.name} to the recurring list`}
                >
                  Restore
                </button>
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
