// Find money: every open Finding, highest estimate first (GLOSSARY: Finding,
// Dismissed, Reviewed; PRODUCT.md: Find money). Each row says what
// it is, what it's worth a year, the one thing to do, and the evidence
// behind it, and can open the charges it was estimated from.
//
// Every figure comes from core (GET /api/find), the same report the agent
// reads; the browser only words and formats it. The order is the server's.
// The month is App state, shared with the other screens: the estimates read
// the 12 months ending with it.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type {
  Account,
  CardEarn,
  CardStatus,
  Finding,
  FindingKind,
  FindReport,
  FlowMonth,
  Offer,
  OfferEvidence,
  SubscriptionEvidence,
  TxnPage,
} from "../types";
import { errMessage, monthLabel, signedCents, unsignedCents } from "../format";
import { onLinkClick, pathOf } from "../route";
import { accountSource, SourceChip } from "./accounts";
import { monthKey } from "./Flow";
import type { ToastMsg } from "./Toast";

type Load<T> =
  | { state: "loading" }
  | { state: "error"; error: string }
  | { state: "ok"; data: T };

/** The margin note for a Finding, by kind (Check-in's Finding rows too). */
export const FINDING_NOTE: Record<FindingKind, string> = {
  add_offer: "add offer",
  wrong_card: "wrong card",
  expiring: "expiring",
  card_choice: "card choice",
  subscription: "review",
  price_rise: "price up",
};

/** Kinds with a clock or a change on them: an Attention Amber note. */
const WARN: ReadonlySet<FindingKind> = new Set(["expiring", "price_rise"]);

const NUMBER_WORDS = [
  "no",
  "one",
  "two",
  "three",
  "four",
  "five",
  "six",
  "seven",
  "eight",
  "nine",
];

/** Numbers in prose: words up to nine, digits after. */
function num(n: number): string {
  return n < NUMBER_WORDS.length ? NUMBER_WORDS[n] : n.toLocaleString("en-US");
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

function plural(n: number, one: string, many = `${one}s`): string {
  return `${num(n)} ${n === 1 ? one : many}`;
}

/** Core's action, as a sentence. */
function sentence(s: string): string {
  return /[.?!]$/.test(s) ? s : `${s}.`;
}

function points(n: number): string {
  return `${n.toLocaleString("en-US")} ${n === 1 ? "point" : "points"}`;
}

/** "Oct 22", with the year when it isn't this year. */
function day(epochSecs: number): string {
  const d = new Date(epochSecs * 1000);
  return d.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    ...(d.getFullYear() !== new Date().getFullYear()
      ? { year: "numeric" }
      : {}),
  });
}

/** `YYYY-MM-DD` → its month, "Nov 2026". */
function monthOf(isoDay: string): string {
  const [y, m] = isoDay.split("-").map(Number);
  return monthLabel(y, m);
}

/** The window's months, "Dec 2025 – Nov 2026" (`end` is exclusive). */
function windowLabel(r: FindReport): string {
  const [y, m] = r.window.end.split("-").map(Number);
  const last = new Date(y, m - 2, 1);
  return `${monthOf(r.window.start)} – ${monthLabel(last.getFullYear(), last.getMonth() + 1)}`;
}

function windowInProgress(r: FindReport): boolean {
  const today = new Date();
  const iso = `${monthKey(today.getFullYear(), today.getMonth() + 1)}-${String(today.getDate()).padStart(2, "0")}`;
  return iso < r.window.end;
}

/** "Add offer" etc. for prose. */
const KIND_WORDS: Record<FindingKind, string> = {
  add_offer: "offer to add",
  wrong_card: "offer on the wrong card",
  expiring: "offer expiring",
  card_choice: "card choice",
  subscription: "subscription to review",
  price_rise: "price rise",
};

// ── Evidence, in words ──────────────────────────────────────────────────────

/** "$15.00 back on $40.00 or more, up to 6 times, $90.00 max". */
function offerTerms(o: Offer): string {
  const r = o.reward;
  let what =
    r.kind === "back"
      ? `${unsignedCents(r.value)} back`
      : r.kind === "pct"
        ? `${r.value}% back`
        : r.kind === "pts"
          ? points(r.value)
          : `${r.value} extra points a dollar`;
  if (o.min !== null) what += ` on ${unsignedCents(o.min)} or more`;
  if (o.single) what += " in one purchase";
  const parts = [what];
  if (o.uses === 1) parts.push("once");
  else if (o.uses !== null) parts.push(`up to ${o.uses} times`);
  // A cap that only repeats a one-time reward says nothing new.
  const repeats =
    o.uses === 1 &&
    ((o.cap.kind === "cash" && r.kind === "back" && o.cap.value === r.value) ||
      (o.cap.kind === "points" && r.kind === "pts" && o.cap.value === r.value));
  if (!repeats && o.cap.kind === "cash")
    parts.push(
      `${unsignedCents(o.cap.value)} max${o.per_year ? " a year" : ""}`,
    );
  if (!repeats && o.cap.kind === "points")
    parts.push(`${points(o.cap.value)} max${o.per_year ? " a year" : ""}`);
  return parts.join(", ");
}

function expiresWords(e: OfferEvidence): string {
  if (e.days_left <= 0) return "expires today";
  if (e.days_left === 1) return "expires tomorrow";
  return `expires ${day(e.offer.expires)}, in ${e.days_left} days`;
}

function offerLines(f: Finding, e: OfferEvidence): string[] {
  const lines = [`${cap(offerTerms(e.offer))}; ${expiresWords(e)}.`];
  const typical =
    e.typical_charge_cents !== null
      ? `, usually ${unsignedCents(e.typical_charge_cents)}`
      : "";
  const last = e.last_charge !== null ? `, last ${day(e.last_charge)}` : "";
  if (f.kind === "wrong_card")
    lines.push(
      `${cap(plural(e.charges, "charge"))} on another card since the offer was added${typical}${last}.`,
    );
  else
    lines.push(
      `${cap(plural(e.charges, "charge"))} here in these 12 months${typical}${last}.`,
    );
  if (f.kind === "expiring") lines.push("Added, and nothing earned on it yet.");
  // The bank's spellings the patterns caught, when they aren't just the
  // offer's name.
  const bare = (m: string) => m.toLowerCase().replace(/[^a-z0-9]/g, "");
  const others = e.matched_merchants.filter(
    (m) => bare(m) !== bare(e.offer.merchant),
  );
  if (others.length > 0)
    lines.push(`Matched as ${e.matched_merchants.join(", ")}.`);
  if (e.offer.note) lines.push(`Terms: ${e.offer.note}.`);
  return lines;
}

function per(e: SubscriptionEvidence): string {
  return e.per_year === 1 ? "a year" : "a month";
}

function subscriptionLines(f: Finding, e: SubscriptionEvidence): string[] {
  const on = f.card_mask ? `, on the card ending ${f.card_mask}` : "";
  if (e.price_rise) {
    return [
      `Was ${unsignedCents(e.price_rise.from_cents)} ${per(e)}, ${unsignedCents(
        e.price_rise.to_cents,
      )} since ${day(e.price_rise.since)}${on}.`,
    ];
  }
  return [
    `${unsignedCents(e.charge_cents)} ${per(e)}, last charged ${day(e.last_charge)}${on}.`,
  ];
}

/** "4 points a dollar at 1.25¢" / "1.5% back". */
function rateWords(c: CardEarn): string {
  return c.cents_per_point === null
    ? `${c.rate}% back`
    : `${c.rate} ${c.rate === "1" ? "point" : "points"} a dollar at ${c.cents_per_point}¢`;
}

function pct(bp: number): string {
  return `${(bp / 100).toFixed(2).replace(/\.?0+$/, "")}%`;
}

function EarnTable({ f, cards }: { f: Finding; cards: CardEarn[] }) {
  return (
    <table className="fm-earn">
      <caption className="lg-sr">
        What each card earns on {f.subject.toLowerCase()}
      </caption>
      <thead>
        <tr>
          <th scope="col">Card</th>
          <th scope="col" className="num">
            Earns
          </th>
          <th scope="col" className="num">
            Spent here
          </th>
          <th scope="col" className="num">
            Earned
          </th>
        </tr>
      </thead>
      <tbody>
        {cards.map((c, i) => (
          <tr key={c.mask} className={i === 0 ? "fm-earn-best" : undefined}>
            <th scope="row">
              {c.issuer} <span className="fm-nowrap">ending {c.mask}</span>
              {i === 0 && <span className="fm-earn-tag">best</span>}
            </th>
            <td className="num">
              <b>{pct(c.return_bp)}</b>
              <span className="fm-earn-sub">
                {rateWords(c)}
                {c.cap_left_cents !== null &&
                  `; ${unsignedCents(c.cap_left_cents)} of the cap left`}
              </span>
            </td>
            <td className="num">
              {unsignedCents(c.spend_cents)}
              <span className="fm-earn-sub">{plural(c.charges, "charge")}</span>
            </td>
            <td className="num">{unsignedCents(c.earned_cents)}</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Evidence({ f }: { f: Finding }) {
  const e = f.evidence;
  if (e.source === "earn")
    return (
      <>
        <p className="fm-ev">
          {e.category === "default"
            ? "Spending no card's earn table names, over these 12 months."
            : "Over these 12 months, by card."}
        </p>
        <EarnTable f={f} cards={e.cards} />
      </>
    );
  const lines =
    e.source === "offer" ? offerLines(f, e) : subscriptionLines(f, e);
  if (e.source === "subscription" && e.reviewed_at !== null)
    lines.push(`Marked reviewed ${day(e.reviewed_at)}.`);
  return (
    <>
      {lines.map((l) => (
        <p key={l} className="fm-ev">
          {l}
        </p>
      ))}
    </>
  );
}

/** "$54.00", with the points the cash figure already counts. */
function Estimate({ f }: { f: Finding }) {
  const pts = f.est_points_per_year;
  return (
    <span className="fm-est" aria-hidden="true">
      <span className="fm-est-val">
        {unsignedCents(f.est_value_cents_per_year)}
      </span>
      <span className="fm-est-per">a year</span>
      {pts > 0 && (
        <span className="fm-est-pts">
          {f.est_cents_per_year > 0
            ? `${unsignedCents(f.est_cents_per_year)} + `
            : ""}
          {points(pts)}
        </span>
      )}
    </span>
  );
}

function estimateWords(f: Finding): string {
  const pts = f.est_points_per_year;
  const parts =
    pts > 0
      ? ` (${f.est_cents_per_year > 0 ? `${unsignedCents(f.est_cents_per_year)} and ` : ""}${points(pts)})`
      : "";
  return `about ${unsignedCents(f.est_value_cents_per_year)} a year${parts}`;
}

// ── The charges behind a Finding ────────────────────────────────────────────

function Charges({
  f,
  period,
  accounts,
  id,
}: {
  f: Finding;
  period: string | undefined;
  accounts: Account[];
  id: string;
}) {
  const [page, setPage] = useState<Load<TxnPage>>({ state: "loading" });
  const key = f.key;
  const load = useCallback(async () => {
    setPage({ state: "loading" });
    try {
      setPage({
        state: "ok",
        data: await api.findCharges(period, key, {
          sort: "date",
          dir: "desc",
          limit: 200,
        }),
      });
    } catch (err) {
      setPage({ state: "error", error: errMessage(err) });
    }
  }, [period, key]);
  useEffect(() => {
    void load();
  }, [load]);
  const byId = useMemo(
    () => new Map(accounts.map((a) => [a.id, a])),
    [accounts],
  );

  return (
    <div id={id} className="fm-charges">
      {page.state === "loading" && (
        <p className="fm-ev" role="status">
          Loading the charges…
        </p>
      )}
      {page.state === "error" && (
        <p className="fm-ev" role="alert">
          Couldn't load the charges: {page.error}{" "}
          <button className="fm-act" onClick={() => void load()}>
            Try again
          </button>
        </p>
      )}
      {page.state === "ok" && (
        <>
          <ul className="fm-txns" aria-label={`Charges behind ${f.subject}`}>
            {page.data.items.map((t) => {
              const a = byId.get(t.account_id);
              return (
                <li key={t.id} className="fm-txn">
                  <span className="fm-txn-day">
                    {day(t.transacted_at ?? t.posted)}
                  </span>
                  <span className="fm-txn-name">
                    {t.display_name}
                    {a && <SourceChip s={accountSource(a)} />}
                  </span>
                  <span className="fm-txn-amt">{signedCents(t.amount)}</span>
                </li>
              );
            })}
          </ul>
          <p className="fm-txns-sum">
            {page.data.total > page.data.items.length
              ? `The latest ${page.data.items.length} of ${page.data.total} charges`
              : cap(plural(page.data.total, "charge"))}
            , {signedCents(page.data.total_cents)} in all
          </p>
        </>
      )}
    </div>
  );
}

// ── One Finding ─────────────────────────────────────────────────────────────

function isSubscription(f: Finding): boolean {
  return f.kind === "subscription" || f.kind === "price_rise";
}

function merchantOf(f: Finding): string | null {
  return f.evidence.source === "subscription" ? f.evidence.merchant : null;
}

function FindingRow({
  f,
  period,
  on,
  accounts,
  busy,
  onDismiss,
  onReview,
}: {
  f: Finding;
  period: string | undefined;
  on: boolean;
  accounts: Account[];
  busy: boolean;
  onDismiss: () => void;
  onReview: (() => void) | null;
}) {
  const [open, setOpen] = useState(false);
  const chargesId = `fm-charges-${f.key.replace(/[^a-z0-9]+/gi, "-")}`;
  const n = f.txn_ids.length;
  return (
    <li className={`fm-row${on ? " on" : ""}`} data-key={f.key}>
      <span
        className={`fm-note${WARN.has(f.kind) ? " fm-warn" : ""}`}
        aria-hidden="true"
      >
        {FINDING_NOTE[f.kind]}
      </span>
      <div className="fm-main">
        <h3 className="fm-name" tabIndex={-1}>
          {f.subject}
          <span className="lg-sr">
            , {KIND_WORDS[f.kind]}, {estimateWords(f)}
          </span>
        </h3>
        <p className="fm-action">{sentence(f.action)}</p>
        <Evidence f={f} />
        <div className="fm-acts">
          {n > 0 && (
            <button
              className="fm-drill"
              aria-expanded={open}
              aria-controls={chargesId}
              onClick={() => setOpen((o) => !o)}
            >
              <Chevron dir={open ? "down" : "right"} />
              {open
                ? "Hide the charges"
                : `Show ${n === 1 ? "the charge" : `the ${n} charges`}`}
            </button>
          )}
          <span className="fm-marks">
            {onReview && (
              <button
                className="fm-act"
                aria-disabled={busy}
                onClick={() => !busy && onReview()}
                aria-label={`Mark ${f.subject} reviewed`}
              >
                Reviewed
              </button>
            )}
            <button
              className="fm-act"
              aria-disabled={busy}
              onClick={() => !busy && onDismiss()}
              aria-label={`Dismiss ${f.subject}`}
            >
              Dismiss
            </button>
          </span>
        </div>
        {open && <Charges f={f} period={period} accounts={accounts} id={chargesId} />}
      </div>
      <Estimate f={f} />
    </li>
  );
}

function Chevron({ dir }: { dir: "left" | "right" | "down" }) {
  const d =
    dir === "left"
      ? "M10 3 5 8l5 5"
      : dir === "right"
        ? "M6 3l5 5-5 5"
        : "M3 6l5 5 5-5";
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 16 16"
      aria-hidden="true"
      focusable="false"
    >
      <path
        d={d}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

// ── Marked lists: dismissed and reviewed ────────────────────────────────────

function MarkedList({
  id,
  label,
  items,
  detail,
  action,
  busy,
  highlight,
  onAction,
}: {
  id: string;
  label: string;
  items: Finding[];
  detail: (f: Finding) => string;
  /** "Restore" / "Unreview". */
  action: string;
  busy: boolean;
  highlight: string | null;
  onAction: (f: Finding) => void;
}) {
  return (
    <div id={id}>
      <h3 className="fm-marked-title">{label}</h3>
      <ul className="fm-marked" aria-label={label}>
        {items.map((f) => (
          <li
            key={f.key}
            className={`fm-marked-row${highlight === f.key ? " on" : ""}`}
            data-key={f.key}
          >
            <span className="fm-note" aria-hidden="true">
              {FINDING_NOTE[f.kind]}
            </span>
            <span className="fm-marked-main">
              <span className="fm-marked-name" tabIndex={-1}>
                {f.subject}
              </span>
              <span className="fm-ev">{detail(f)}</span>
            </span>
            <span className="fm-marked-est">
              {unsignedCents(f.est_value_cents_per_year)} a year
            </span>
            <button
              className="fm-act"
              aria-disabled={busy}
              onClick={() => !busy && onAction(f)}
              aria-label={`${action} ${f.subject}`}
            >
              {action}
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

// ── Cards and notes ─────────────────────────────────────────────────────────

function cardLine(c: CardStatus): { text: string; warn: boolean } {
  const offers =
    c.offers === 0
      ? "no offers"
      : `${plural(c.offers, "offer")}, ${c.added === 0 ? "none" : num(c.added)} added${
          c.expired > 0 ? `, ${num(c.expired)} expired` : ""
        }`;
  const parts = [`${c.issuer} ending ${c.mask}: ${offers}.`];
  if (c.problem)
    parts.push(`${sentence(cap(c.problem))} Only offers to add come from it.`);
  if (!c.earn) parts.push("No earn table, so it's left out of card choice.");
  return { text: parts.join(" "), warn: !!c.problem };
}

/** Core's notes that the card lines don't already say, as sentences. */
function noteLines(r: FindReport): string[] {
  return r.notes
    .filter(
      (n) =>
        !(
          r.cards.length > 0 &&
          n.startsWith("card choice leaves out cards with no")
        ),
    )
    .filter((n) => !n.startsWith("card offers not configured"))
    .map((n) => sentence(cap(n)));
}

function Sources({ r }: { r: FindReport }) {
  const unset = r.notes.some((n) => n.startsWith("card offers not configured"));
  const notes = noteLines(r);
  if (r.cards.length === 0 && !unset && notes.length === 0) return null;
  return (
    <section className="fm-sources" aria-labelledby="fm-sources-title">
      <h2 id="fm-sources-title" className="fm-h2">
        Cards
      </h2>
      {unset && (
        <p className="fm-unset">
          No card offers are set up, so only subscriptions are checked. Card
          offers and card choice come from the offers files the server's{" "}
          <code>OUTFLOW_PERKS</code> names, one per card.
        </p>
      )}
      {(r.cards.length > 0 || notes.length > 0) && (
        <ul className="fm-cards">
          {r.cards.map((c) => {
            const l = cardLine(c);
            return (
              <li
                key={`${c.issuer}-${c.mask}`}
                className={l.warn ? "fm-warn-line" : undefined}
              >
                {l.text}
              </li>
            );
          })}
          {notes.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

// ── The screen ──────────────────────────────────────────────────────────────

export function FindMoney({
  month,
  onMonth,
  accounts,
  notify,
  focusKey,
  onFocused,
}: {
  /** Selected month, `YYYY-MM`, shared with the other screens. */
  month: string | null;
  onMonth: (m: string) => void;
  accounts: Account[];
  notify: (msg: ToastMsg) => void;
  /** A Finding to bring into view (from a Check-in row), read once. */
  focusKey: string | null;
  onFocused: () => void;
}) {
  // Months with data: the stepper's ends.
  const [flow, setFlow] = useState<Load<FlowMonth[]>>({ state: "loading" });
  const loadFlow = useCallback(async () => {
    try {
      setFlow({ state: "ok", data: await api.flow() });
    } catch (err) {
      setFlow({ state: "error", error: errMessage(err) });
    }
  }, []);
  useEffect(() => {
    void loadFlow();
  }, [loadFlow]);
  const keys = useMemo(
    () =>
      flow.state === "ok"
        ? flow.data.map((m) => monthKey(m.year, m.month))
        : [],
    [flow],
  );
  // No month chosen yet: the latest there is (this month, once it has a row).
  const period =
    month && keys.includes(month)
      ? month
      : keys.length > 0
        ? keys[keys.length - 1]
        : null;

  const [data, setData] = useState<Load<FindReport>>({ state: "loading" });
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);
  const cache = useRef(new Map<string, FindReport>());
  const load = useCallback(async (p: string) => {
    const mine = ++seq.current;
    const hit = cache.current.get(p);
    if (hit) {
      setBusy(false);
      setData({ state: "ok", data: hit });
      return;
    }
    const dim = window.setTimeout(
      () => mine === seq.current && setBusy(true),
      150,
    );
    try {
      const r = await api.find(p);
      cache.current.set(p, r);
      if (mine === seq.current) setData({ state: "ok", data: r });
    } catch (err) {
      if (mine === seq.current)
        setData({ state: "error", error: errMessage(err) });
    } finally {
      window.clearTimeout(dim);
      if (mine === seq.current) setBusy(false);
    }
  }, []);
  useEffect(() => {
    if (period) void load(period);
  }, [period, load]);

  // A mark changes the report for every month: start the cache over.
  const apply = useCallback(
    (p: string, r: FindReport) => {
      cache.current.clear();
      cache.current.set(p, r);
      if (p === period) setData({ state: "ok", data: r });
    },
    [period],
  );

  const [working, setWorking] = useState(false);
  const [showDismissed, setShowDismissed] = useState(false);
  const [showReviewed, setShowReviewed] = useState(false);
  const [highlight, setHighlight] = useState<string | null>(null);

  // After a row leaves the list, focus goes to the row now in its place,
  // else the one before, else the list's heading.
  const refocus = (index: number) => {
    window.requestAnimationFrame(() => {
      const names = document.querySelectorAll<HTMLElement>(
        ".fm-list > .fm-row .fm-name",
      );
      const to =
        names[index] ??
        names[index - 1] ??
        document.getElementById("fm-list-title");
      to?.focus();
    });
  };

  const mark = async (
    run: (p: string) => Promise<FindReport>,
    ok: ToastMsg | null,
    failed: string,
    after?: () => void,
  ) => {
    if (!period || working) return;
    const p = period;
    setWorking(true);
    try {
      apply(p, await run(p));
      after?.();
      if (ok) notify(ok);
    } catch (err) {
      notify({ kind: "err", text: `${failed}: ${errMessage(err)}` });
    } finally {
      setWorking(false);
    }
  };

  const restore = (f: Finding) =>
    mark(
      (p) => api.setFindingDismissed(p, f.key, false),
      { kind: "ok", text: `Restored ${f.subject}` },
      `Couldn't restore ${f.subject}`,
    );
  const dismiss = (f: Finding, index: number) =>
    mark(
      (p) => api.setFindingDismissed(p, f.key, true),
      {
        kind: "ok",
        text: `Dismissed ${f.subject}`,
        action: { label: "Undo", run: () => void restore(f) },
      },
      `Couldn't dismiss ${f.subject}`,
      () => refocus(index),
    );
  const unreview = (f: Finding) => {
    const m = merchantOf(f);
    if (!m) return;
    void mark(
      (p) => api.setMerchantReviewed(p, m, false),
      { kind: "ok", text: `${f.subject} is back for review` },
      `Couldn't change ${f.subject}`,
    );
  };
  const review = (f: Finding, index: number) => {
    const m = merchantOf(f);
    if (!m) return;
    void mark(
      (p) => api.setMerchantReviewed(p, m, true),
      {
        kind: "ok",
        text: `Marked ${f.subject} reviewed`,
        action: { label: "Undo", run: () => unreview(f) },
      },
      `Couldn't mark ${f.subject} reviewed`,
      () => refocus(index),
    );
  };

  // Arriving from a Check-in row: open the list the Finding is in, bring it
  // into view and focus it. Once.
  const r = data.state === "ok" ? data.data : null;
  useEffect(() => {
    if (!focusKey || !r) return;
    onFocused();
    if (r.dismissed.some((f) => f.key === focusKey)) setShowDismissed(true);
    else if (r.reviewed.some((f) => f.key === focusKey)) setShowReviewed(true);
    else if (!r.findings.some((f) => f.key === focusKey)) return;
    setHighlight(focusKey);
    window.requestAnimationFrame(() => {
      const row = document.querySelector<HTMLElement>(
        `[data-key="${CSS.escape(focusKey)}"]`,
      );
      row?.scrollIntoView({ block: "center" });
      row
        ?.querySelector<HTMLElement>(".fm-name, .fm-marked-name")
        ?.focus({ preventScroll: true });
    });
  }, [focusKey, r, onFocused]);

  if (flow.state === "error") {
    return (
      <main className="fm-wrap">
        <h1 className="lg-sr">Find money</h1>
        <div className="fl-state" role="alert">
          <p>Couldn't load Find money: {flow.error}</p>
          <button className="lg-pill" onClick={() => void loadFlow()}>
            Try again
          </button>
        </div>
      </main>
    );
  }
  if (flow.state === "ok" && keys.length === 0) {
    return (
      <main className="fm-wrap">
        <h1 className="lg-sr">Find money</h1>
        <div className="fl-state">
          <p>No transactions yet.</p>
          <p className="fl-state-hint">
            Link a bank in{" "}
            <a
              href={`${pathOf("settings")}#connections`}
              onClick={(e) =>
                onLinkClick(e, `${pathOf("settings")}#connections`)
              }
            >
              Settings → Connections
            </a>{" "}
            and the first sync fills this in.
          </p>
        </div>
      </main>
    );
  }

  const at = period ? keys.indexOf(period) : -1;
  const atStart = at <= 0;
  const atEnd = at === keys.length - 1;
  const step = (dir: -1 | 1) => {
    const next = keys[at + dir];
    if (next) onMonth(next);
  };

  return (
    <main className="fm-wrap" aria-labelledby="fm-title">
      <div className="fm-titlebar">
        <h1 id="fm-title" className="fm-title">
          Find money
        </h1>
        {r && (
          <p className="fm-window">
            <span className="lg-sr">Estimates read </span>
            {windowLabel(r)}
            {windowInProgress(r) && <span className="fl-sofar">so far</span>}
          </p>
        )}
        {period && (
          <div
            className="ci-step fm-step"
            role="group"
            aria-label="Change the last month"
          >
            <button
              className="ci-stepbtn"
              onClick={() => !atStart && step(-1)}
              aria-disabled={atStart}
            >
              <Chevron dir="left" />
              <span className="lg-sr">Previous month</span>
            </button>
            <button
              className="ci-stepbtn"
              onClick={() => !atEnd && step(1)}
              aria-disabled={atEnd}
            >
              <Chevron dir="right" />
              <span className="lg-sr">Next month</span>
            </button>
          </div>
        )}
      </div>

      {data.state === "loading" && (
        <p className="ci-quiet" role="status">
          Loading…
        </p>
      )}
      {data.state === "error" && (
        <div className="fl-state" role="alert">
          <p>Couldn't load Find money: {data.error}</p>
          <button
            className="lg-pill"
            onClick={() => period && void load(period)}
          >
            Try again
          </button>
        </div>
      )}
      {r && (
        <div className={busy ? "fm-body fm-busy" : "fm-body"} aria-busy={busy}>
          <Lead r={r} />
          <section className="fm-findings" aria-labelledby="fm-list-title">
            <h2 id="fm-list-title" className="fm-h2" tabIndex={-1}>
              Open findings
              {r.findings.length > 0 && (
                <span className="fm-count">
                  <span className="lg-sr">, </span>
                  {r.findings.length}
                </span>
              )}
              {r.findings.length > 0 && (
                <span className="fm-h2-key">highest estimate first</span>
              )}
            </h2>
            {r.findings.length === 0 ? (
              <p className="fm-none">
                {r.dismissed.length + r.reviewed.length > 0
                  ? "Nothing open: everything found is dismissed or reviewed."
                  : "Nothing found in these 12 months."}
              </p>
            ) : (
              <ul className="fm-list">
                {r.findings.map((f, i) => (
                  <FindingRow
                    key={f.key}
                    f={f}
                    period={period ?? undefined}
                    on={highlight === f.key}
                    accounts={accounts}
                    busy={working}
                    onDismiss={() => void dismiss(f, i)}
                    onReview={isSubscription(f) ? () => review(f, i) : null}
                  />
                ))}
              </ul>
            )}
            {(r.dismissed.length > 0 || r.reviewed.length > 0) && (
              <div className="fm-foot">
                {r.reviewed.length > 0 && (
                  <button
                    className="fm-toggle"
                    aria-expanded={showReviewed}
                    aria-controls="fm-reviewed"
                    onClick={() => setShowReviewed((x) => !x)}
                  >
                    {showReviewed
                      ? "Hide reviewed"
                      : `${r.reviewed.length} reviewed`}
                  </button>
                )}
                {r.dismissed.length > 0 && (
                  <button
                    className="fm-toggle"
                    aria-expanded={showDismissed}
                    aria-controls="fm-dismissed"
                    onClick={() => setShowDismissed((x) => !x)}
                  >
                    {showDismissed
                      ? "Hide dismissed"
                      : `${r.dismissed.length} dismissed`}
                  </button>
                )}
              </div>
            )}
            {showReviewed && r.reviewed.length > 0 && (
              <MarkedList
                id="fm-reviewed"
                label="Reviewed"
                items={r.reviewed}
                detail={(f) =>
                  f.evidence.source === "subscription" &&
                  f.evidence.reviewed_at !== null
                    ? `Reviewed ${day(f.evidence.reviewed_at)}; back in a year, or sooner if the price goes up`
                    : "Reviewed"
                }
                action="Unreview"
                busy={working}
                highlight={highlight}
                onAction={unreview}
              />
            )}
            {showDismissed && r.dismissed.length > 0 && (
              <MarkedList
                id="fm-dismissed"
                label="Dismissed"
                items={r.dismissed}
                detail={() =>
                  "Dismissed; back on its own only if its estimate more than doubles"
                }
                action="Restore"
                busy={working}
                highlight={highlight}
                onAction={(f) => void restore(f)}
              />
            )}
          </section>
          <Sources r={r} />
        </div>
      )}
    </main>
  );
}

/** One line on what's here: how many, their total, and the largest. The
 *  total is the open Findings only (a price rise already replaces its
 *  subscription, so nothing is counted twice). */
function Lead({ r }: { r: FindReport }) {
  const n = r.findings.length;
  if (n === 0) return null;
  const top = r.findings[0];
  const total = r.findings.reduce((s, f) => s + f.est_value_cents_per_year, 0);
  return (
    <p className="fm-lead">
      {cap(plural(n, "thing"))} could save or earn money
      {n > 1 && (
        <>
          , about <b>{unsignedCents(total)}</b> a year together
        </>
      )}
      , estimated from these 12 months. {n === 1 ? "It's" : "The largest is"}{" "}
      {top.subject}, about{" "}
      <b>{unsignedCents(top.est_value_cents_per_year)}</b> a year.
    </p>
  );
}
