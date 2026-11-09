// Check-in, the home screen: one week, month or quarter read against your own
// history (GLOSSARY: Check-in, Unusual; ADR 0001). It answers "what changed,
// and is anything unusual?" in words first, then the three figures behind the
// answer, then the unusual items, each a link to the rows behind it.
//
// Every figure comes from core (GET /api/checkin), the same numbers the agent
// reads; the browser only words and formats them. The month is App state,
// shared with Flow and Holdings; with none chosen yet, Check-in opens on the
// last finished month. Week and quarter step with previous / next.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../api";
import type { CheckIn as CheckInData, FlowMonth, NotChecked, Unusual, Versus } from "../types";
import {
  errMessage,
  everyFewMonths,
  formatCentsShort,
  monthLabel,
  monthLabelShort,
  signedCents,
  unsignedCents,
} from "../format";
import { onLinkClick, pathOf, navigate } from "../route";
import { MonthStrip, type StripMonth } from "./MonthStrip";
import { monthKey, type Pick } from "./Flow";

export type CheckinView = "week" | "month" | "quarter";

/** Where Check-in is, kept in App state so a drill-down and Back return to it. */
export interface CheckinNav {
  view: CheckinView;
  /** `2026-W45`; null = last week. */
  week: string | null;
  /** `2026-Q3`; null = last quarter. */
  quarter: string | null;
}

const VIEWS: { key: CheckinView; label: string }[] = [
  { key: "week", label: "Week" },
  { key: "month", label: "Month" },
  { key: "quarter", label: "Quarter" },
];

const LONG_MONTHS = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];

type Load<T> = { state: "loading" } | { state: "error"; error: string } | { state: "ok"; data: T };

// Where a drill-down left from, so Back returns focus there (a row, or a
// figure). Module state: Check-in unmounts while Flow is open.
let returnTo: string | null = null;

const NUMBER_WORDS = ["no", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine"];

/** Numbers in prose: words up to nine, digits after. */
function num(n: number): string {
  return n < NUMBER_WORDS.length ? NUMBER_WORDS[n] : String(n);
}

function cap(s: string): string {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** "A, B and C". */
function listOf(items: string[]): string {
  return items.length <= 1
    ? (items[0] ?? "")
    : `${items.slice(0, -1).join(", ")} and ${items[items.length - 1]}`;
}

// ── Dates (period stepping is scaffolding; core decides the edges) ──────────

function parseDay(s: string): Date {
  const [y, m, d] = s.split("-").map(Number);
  return new Date(y, m - 1, d);
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

function dayLabel(d: Date, year = false): string {
  return d.toLocaleDateString("en-US", {
    month: "short",
    day: "numeric",
    ...(year ? { year: "numeric" } : {}),
  });
}

/** ISO 8601 week spelling (`2026-W45`) of the week containing `d`. */
function isoWeek(d: Date): string {
  const t = new Date(Date.UTC(d.getFullYear(), d.getMonth(), d.getDate()));
  const dow = t.getUTCDay() || 7;
  t.setUTCDate(t.getUTCDate() + 4 - dow); // the Thursday decides the year
  const y = t.getUTCFullYear();
  const week = Math.ceil(((t.getTime() - Date.UTC(y, 0, 1)) / 86400000 + 1) / 7);
  return `${y}-W${String(week).padStart(2, "0")}`;
}

function quarterOf(d: Date): string {
  return `${d.getFullYear()}-Q${Math.floor(d.getMonth() / 3) + 1}`;
}

function previousMonthKey(): string {
  const now = new Date();
  const d = new Date(now.getFullYear(), now.getMonth() - 1, 1);
  return monthKey(d.getFullYear(), d.getMonth() + 1);
}

/** "2026-11" → "October" style long name (with the year if not this year). */
function monthPhrase(key: string): string {
  const [y, m] = key.split("-").map(Number);
  return y === new Date().getFullYear() ? LONG_MONTHS[m - 1] : `${LONG_MONTHS[m - 1]} ${y}`;
}

/** The period in three forms: the title, inside a sentence, and as a unit. */
function describe(c: CheckInData): { title: string; phrase: string; unit: string } {
  const start = parseDay(c.period.start);
  if (c.period.kind === "Week") {
    const last = addDays(parseDay(c.period.end), -1);
    const sameYear = last.getFullYear() === new Date().getFullYear();
    return {
      title: `Week of ${dayLabel(start, !sameYear)}`,
      phrase: c.in_progress ? "this week" : `the week of ${dayLabel(start)}`,
      unit: "weeks",
    };
  }
  if (c.period.kind === "Quarter") {
    const q = Math.floor(start.getMonth() / 3) + 1;
    return {
      title: `Q${q} ${start.getFullYear()}`,
      phrase: c.in_progress ? "this quarter" : `Q${q} ${start.getFullYear()}`,
      unit: "quarters",
    };
  }
  const key = monthKey(start.getFullYear(), start.getMonth() + 1);
  return {
    title: monthLabel(start.getFullYear(), start.getMonth() + 1),
    phrase: c.in_progress ? "this month" : monthPhrase(key),
    unit: "months",
  };
}

// ── The screen ──────────────────────────────────────────────────────────────

export function CheckIn({
  month,
  onMonth,
  nav,
  onNav,
  onDrill,
}: {
  /** Selected month, `YYYY-MM`, shared with Flow and Holdings. */
  month: string | null;
  onMonth: (m: string) => void;
  nav: CheckinNav;
  onNav: (n: CheckinNav) => void;
  /** Open Flow on `month` with `pick` already applied. */
  onDrill: (pick: Pick, month: string) => void;
}) {
  // The month strip, and where history starts.
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
  const months = flow.state === "ok" ? flow.data : [];
  const keys = useMemo(() => months.map((m) => monthKey(m.year, m.month)), [months]);

  // No month chosen yet (or one with no data): the last finished month,
  // else the latest there is.
  useEffect(() => {
    if (keys.length === 0 || (month && keys.includes(month))) return;
    const last = previousMonthKey();
    onMonth(keys.includes(last) ? last : keys[keys.length - 1]);
  }, [keys, month, onMonth]);

  const period =
    nav.view === "week"
      ? (nav.week ?? "last-week")
      : nav.view === "quarter"
        ? (nav.quarter ?? "last-quarter")
        : month && keys.includes(month)
          ? month
          : null;

  // The check-in. A switch keeps the old figures (dimmed) until the new ones
  // land; only the newest request may write.
  const [data, setData] = useState<Load<CheckInData>>({ state: "loading" });
  const [busy, setBusy] = useState(false);
  const seq = useRef(0);
  const load = useCallback(async (p: string) => {
    const mine = ++seq.current;
    const dim = window.setTimeout(() => mine === seq.current && setBusy(true), 150);
    try {
      const c = await api.checkin(p);
      if (mine === seq.current) setData({ state: "ok", data: c });
    } catch (err) {
      if (mine === seq.current) setData({ state: "error", error: errMessage(err) });
    } finally {
      window.clearTimeout(dim);
      if (mine === seq.current) setBusy(false);
    }
  }, []);
  useEffect(() => {
    if (period) void load(period);
  }, [period, load]);

  // After a change of period, say which one loaded (the step buttons and
  // the strip change it without moving focus). Not on first load.
  const [announce, setAnnounce] = useState("");
  const loads = useRef(0);
  const loaded = data.state === "ok" ? data.data : null;
  useEffect(() => {
    if (!loaded) return;
    if (loads.current++ > 0) setAnnounce(describe(loaded).title);
    // Back from a drill-down: focus the link that was used.
    if (returnTo) {
      const key = returnTo;
      returnTo = null;
      window.requestAnimationFrame(() =>
        document.querySelector<HTMLElement>(`[data-return="${key}"]`)?.focus(),
      );
    }
  }, [loaded]);

  if (flow.state === "error") {
    return (
      <main className="ci-wrap">
        <h1 className="lg-sr">Check-in</h1>
        <div className="fl-state" role="alert">
          <p>Couldn't load the check-in: {flow.error}</p>
          <button className="lg-pill" onClick={() => void loadFlow()}>
            Try again
          </button>
        </div>
      </main>
    );
  }
  if (flow.state === "ok" && months.length === 0) {
    return (
      <main className="ci-wrap">
        <h1 className="lg-sr">Check-in</h1>
        <div className="fl-state">
          <p>No transactions yet.</p>
          <p className="fl-state-hint">
            Link a bank in{" "}
            <a
              href={`${pathOf("settings")}#connections`}
              onClick={(e) => onLinkClick(e, `${pathOf("settings")}#connections`)}
            >
              Settings → Connections
            </a>{" "}
            and the first sync fills this in.
          </p>
        </div>
      </main>
    );
  }

  const c = data.state === "ok" ? data.data : null;
  const d = c ? describe(c) : null;
  const firstMonth = keys[0] ?? null;

  // Previous / next for week and quarter, from the period core returned.
  const step = (dir: -1 | 1) => {
    if (!c) return;
    const start = parseDay(c.period.start);
    if (nav.view === "week") onNav({ ...nav, week: isoWeek(addDays(start, 7 * dir)) });
    else onNav({ ...nav, quarter: quarterOf(new Date(start.getFullYear(), start.getMonth() + 3 * dir, 1)) });
  };
  const atStart = !!c && !!firstMonth && c.period.start <= `${firstMonth}-01`;
  const unitWord = nav.view === "week" ? "week" : "quarter";

  return (
    <main className="ci-wrap" aria-labelledby="ci-title">
      {nav.view === "month" && months.length > 0 && (
        <MonthStrip
          caption="Earned − spent per month"
          months={stripMonths(months)}
          selected={month}
          onSelect={onMonth}
        />
      )}

      <div className="ci-titlebar">
        <h1 id="ci-title" className="ci-title">
          {d ? d.title : "Check-in"}
          {c?.in_progress && <span className="fl-sofar">so far</span>}
        </h1>
        {nav.view !== "month" && c && (
          <div className="ci-step" role="group" aria-label={`Change ${unitWord}`}>
            {/* aria-disabled, not disabled: a button that disables itself
                would drop keyboard focus to the page. */}
            <button className="ci-stepbtn" onClick={() => !atStart && step(-1)} aria-disabled={atStart}>
              <Chevron dir="left" />
              <span className="lg-sr">Previous {unitWord}</span>
            </button>
            <button
              className="ci-stepbtn"
              onClick={() => !c.in_progress && step(1)}
              aria-disabled={c.in_progress}
            >
              <Chevron dir="right" />
              <span className="lg-sr">Next {unitWord}</span>
            </button>
          </div>
        )}
        <div className="lg-win ci-views" role="group" aria-label="Period">
          {VIEWS.map((v) => (
            <button
              key={v.key}
              className={v.key === nav.view ? "on" : ""}
              aria-pressed={v.key === nav.view}
              onClick={() => onNav({ ...nav, view: v.key })}
            >
              {v.label}
            </button>
          ))}
        </div>
      </div>

      <p className="lg-sr" role="status">
        {announce}
      </p>
      {data.state === "loading" && (
        <p className="ci-quiet" role="status">
          Loading…
        </p>
      )}
      {data.state === "error" && (
        <div className="fl-state" role="alert">
          <p>Couldn't load the check-in: {data.error}</p>
          <button className="lg-pill" onClick={() => period && void load(period)}>
            Try again
          </button>
        </div>
      )}
      {c && d && (
        <div className={busy ? "ci-body ci-busy" : "ci-body"} aria-busy={busy}>
          <Answer c={c} phrase={d.phrase} />
          <Figures c={c} unit={d.unit} linkMonth={nav.view === "month" ? month : null} />
          <UnusualList c={c} phrase={d.phrase} fallbackMonth={month} onDrill={onDrill} />
        </div>
      )}
    </main>
  );
}

// The strip shows earned − spent per month: a signed amount, so no bar.
function stripMonths(months: FlowMonth[]): StripMonth[] {
  const now = new Date();
  return months.map((m) => ({
    key: monthKey(m.year, m.month),
    label: monthLabel(m.year, m.month),
    short: monthLabelShort(m.year, m.month),
    figure: shortSigned(m.earned_minus_spent_cents),
    spoken: `${signedCents(m.earned_minus_spent_cents)} earned minus spent`,
    sofar: m.year === now.getFullYear() && m.month === now.getMonth() + 1,
  }));
}

function shortSigned(cents: number): string {
  const s = formatCentsShort(Math.abs(cents));
  return cents > 0 ? `+${s}` : cents < 0 ? `−${s}` : s;
}

function Chevron({ dir }: { dir: "left" | "right" }) {
  return (
    <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true" focusable="false">
      <path
        d={dir === "left" ? "M10 3 5 8l5 5" : "M6 3l5 5-5 5"}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.75"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}

// ── The answer, in words ────────────────────────────────────────────────────

const ATTENTION = new Set(["connection", "match_review"]);

/** "Nov 8" for the usual-to-date day, else null. */
function byDay(c: CheckInData): string | null {
  return c.usual.to_date ? dayLabel(parseDay(c.usual.to_date)) : null;
}

/** The headline figure against its usual, as a clause tail. */
function versusWords(v: Versus | null, by: string | null): React.ReactNode {
  if (!v) return null;
  const when = by ? ` by ${by}` : "";
  if (!v.outside) return <>, within your usual range{when}</>;
  const amount = <b>{unsignedCents(Math.abs(v.diff_cents))}</b>;
  // After "kept $X" or "spent $X", more / less reads right; after "spent
  // more than you earned" it would read backwards, so: behind / ahead.
  if (v.figure === "earned_minus_spent" && v.cents <= 0)
    return (
      <>
        , {amount} {v.diff_cents > 0 ? "ahead of" : "behind"} your usual{when}
      </>
    );
  return (
    <>
      , {amount} {v.diff_cents > 0 ? "more" : "less"} than usual{when}
    </>
  );
}

/** Why it was worse: the items behind it, or a smaller income. */
function becauseWords(v: Versus | null): string | null {
  if (!v || !v.outside || !v.worse) return null;
  if (v.contributors.length > 0) {
    const sum = v.contributors.reduce((a, x) => a + x.cents, 0);
    const names = listOf(v.contributors.map((x) => x.label));
    return v.contributors.length === 1
      ? `${names} was ${unsignedCents(sum)} of it`
      : `${names} were ${unsignedCents(sum)} of it`;
  }
  const inDiff = v.in_diff_cents ?? 0;
  if (inDiff < 0 && -inDiff * 2 >= Math.abs(v.diff_cents))
    return `you earned ${unsignedCents(-inDiff)} less than usual`;
  return null;
}

function Answer({ c, phrase }: { c: CheckInData; phrase: string }) {
  const sofar = c.in_progress;
  const nw = c.net_worth.change_cents;
  const f = c.flow;
  const v = c.versus;
  const by = byDay(c);
  const ve = sofar ? "'ve" : "";

  const clauses: React.ReactNode[] = [];
  if (nw !== null) {
    clauses.push(
      nw === 0 ? (
        <>net worth {sofar ? "is" : "was"} unchanged</>
      ) : (
        <>
          net worth {sofar ? (nw > 0 ? "is up" : "is down") : nw > 0 ? "rose" : "fell"}{" "}
          <b>{unsignedCents(nw)}</b>
        </>
      ),
    );
  }
  clauses.push(
    v?.figure === "spent" ? (
      // A week: paydays make earned − spent swing, so spending is the read.
      <>
        you{ve} spent <b>{unsignedCents(f.out_cents)}</b>
        {versusWords(v, by)}
      </>
    ) : f.in_cents === 0 && f.out_cents === 0 ? (
      <>nothing came in or went out</>
    ) : f.earned_minus_spent_cents > 0 ? (
      <>
        you{ve} kept <b>{unsignedCents(f.earned_minus_spent_cents)}</b> of what you earned
        {versusWords(v, by)}
      </>
    ) : f.earned_minus_spent_cents < 0 ? (
      <>
        you{ve} spent <b>{unsignedCents(-f.earned_minus_spent_cents)}</b> more than you earned
        {versusWords(v, by)}
      </>
    ) : (
      <>
        you{ve} spent exactly what you earned
        {versusWords(v, by)}
      </>
    ),
  );
  const because = becauseWords(v);

  const lead = sofar ? `So far ${phrase === "this month" || phrase === "this week" || phrase === "this quarter" ? phrase : `in ${phrase}`}` : `In ${phrase}`;

  // Minor items are folded on the list; the sentence counts the rest.
  const needs = c.unusual.filter((u) => ATTENTION.has(u.kind)).length;
  const odd = c.unusual.filter((u) => !ATTENTION.has(u.kind) && !u.minor).length;
  const unchecked = c.not_checked.length > 0;
  let tail: string;
  if (needs === 0 && odd === 0)
    tail = unchecked ? "Nothing looks unusual in what could be checked." : "Nothing looks unusual.";
  else if (needs === 0) tail = `${cap(num(odd))} ${odd === 1 ? "thing looks" : "things look"} unusual.`;
  else if (odd === 0) tail = `${cap(num(needs))} ${needs === 1 ? "thing needs" : "things need"} you.`;
  else
    tail = `${cap(num(needs))} ${needs === 1 ? "thing needs" : "things need"} you and ${num(odd)} ${
      odd === 1 ? "looks" : "look"
    } unusual.`;

  return (
    <p className="ci-answer">
      {lead}, {clauses[0]}
      {clauses.length > 1 && <> and {clauses[1]}</>}
      {because && <>; {because}</>}. <span className="ci-answer-tail">{tail}</span>
    </p>
  );
}

// ── The three figures ───────────────────────────────────────────────────────

function UsualLine({
  cents,
  signed,
  n,
  unit,
  by,
}: {
  cents: number | null;
  signed: boolean;
  n: number;
  unit: string;
  /** In progress: "Nov 8", the day the usual is taken to. */
  by: string | null;
}) {
  if (cents === null)
    return (
      <span className="ci-usual">
        no usual yet<span className="lg-sr">: fewer than 3 earlier {unit} to compare with ({n})</span>
      </span>
    );
  return (
    <span className="ci-usual">
      usually <b>{signed ? signedCents(cents) : unsignedCents(cents, true)}</b>
      {by && <> by {by}</>}
    </span>
  );
}

/** A figure that opens the screen with its rows (Month view only). */
function FigureLink({ text, to, where, id }: { text: string; to: string; where: string; id: string }) {
  return (
    <a
      className="ci-figlink"
      href={to}
      data-return={id}
      onClick={(e) => {
        returnTo = id;
        onLinkClick(e, to);
      }}
    >
      {text}
      <span className="lg-sr">, open {where}</span>
    </a>
  );
}

function Figures({ c, unit, linkMonth }: { c: CheckInData; unit: string; linkMonth: string | null }) {
  const nw = c.net_worth;
  const f = c.flow;
  const u = c.usual;
  const where = linkMonth ? monthLabel(Number(linkMonth.slice(0, 4)), Number(linkMonth.slice(5))) : "";
  const week = c.period.kind === "Week";
  const by = byDay(c);
  const fig = (text: string, screen: "flow" | "holdings", id: string) =>
    linkMonth ? (
      <FigureLink
        id={id}
        text={text}
        to={pathOf(screen)}
        where={`${where} in ${screen === "flow" ? "Flow" : "Holdings"}`}
      />
    ) : (
      text
    );
  const nwWhy =
    nw.reason === "no_snapshots"
      ? "no balance snapshots for this period"
      : nw.reason === "one_snapshot"
        ? c.in_progress
          ? "only one balance snapshot so far"
          : "only one balance snapshot"
        : null;

  return (
    <section className="ci-figs" aria-label="Figures">
      <dl className="ci-figrow">
        <div className="ci-fig">
          <dt>Net worth change</dt>
          <dd className="ci-figval">
            {nw.change_cents === null ? (
              <>
                <span aria-hidden="true">—</span>
                <span className="lg-sr">none</span>
              </>
            ) : (
              fig(signedCents(nw.change_cents), "holdings", "fig-nw")
            )}
          </dd>
          {nw.change_cents === null ? (
            <dd className="ci-sub">{nwWhy}</dd>
          ) : (
            // No market moves (say, no investment accounts): nothing to split.
            !!nw.markets_cents &&
            nw.added_cents !== null && (
              <dd className="ci-sub">
                Markets {signedCents(nw.markets_cents)} · Added {signedCents(nw.added_cents)}
              </dd>
            )
          )}
          {nw.excluded.length > 0 && (
            <dd className="ci-sub">
              Added includes all of {nw.excluded.join(", ")}: no gain known
            </dd>
          )}
          {nw.partial_start && <dd className="ci-sub">from the first snapshot</dd>}
          <dd>
            <UsualLine cents={u.net_worth_change_cents} signed n={u.net_worth_periods} unit={unit} by={by} />
          </dd>
        </div>
        {week ? (
          // A week reads spending: paydays make earned − spent swing.
          <div className="ci-fig">
            <dt>Spent</dt>
            <dd className="ci-figval">{fig(unsignedCents(f.out_cents, true), "flow", "fig-spent")}</dd>
            <dd className="ci-sub">
              <span className="ci-nowrap">{signedCents(f.in_cents)}</span> in · earned − spent{" "}
              <span className="ci-nowrap">{signedCents(f.earned_minus_spent_cents)}</span>
            </dd>
            <dd>
              <UsualLine cents={u.out_cents} signed={false} n={u.periods} unit={unit} by={by} />
            </dd>
          </div>
        ) : (
          <div className="ci-fig">
            <dt>Earned − spent</dt>
            <dd className="ci-figval">
              {fig(signedCents(f.earned_minus_spent_cents), "flow", "fig-es")}
            </dd>
            <dd className="ci-sub">
              <span className="ci-nowrap">{signedCents(f.in_cents)}</span> in ·{" "}
              <span className="ci-nowrap">{signedCents(-f.out_cents)}</span> out
            </dd>
            <dd>
              <UsualLine cents={u.earned_minus_spent_cents} signed n={u.periods} unit={unit} by={by} />
            </dd>
          </div>
        )}
        <div className="ci-fig">
          <dt>Saved</dt>
          <dd className="ci-figval">{fig(unsignedCents(f.saved_cents, true), "flow", "fig-saved")}</dd>
          <dd className="ci-sub">
            {f.gap_cents > 0
              ? `${unsignedCents(f.gap_cents)} stayed as cash`
              : f.gap_cents < 0
                ? `${unsignedCents(-f.gap_cents)} drawn from cash`
                : "nothing left as cash"}
          </dd>
          {!week && (
            <dd>
              <UsualLine cents={u.saved_cents} signed={false} n={u.periods} unit={unit} by={by} />
            </dd>
          )}
        </div>
      </dl>
      <p className="ci-foot">
        Usually is the middle value of the {unit} before this one (up to 12)
        {by ? `, each taken to the same day, since this one isn't over` : ""}
        {week ? "; a week is read by what you spent, since paydays swing the rest" : ""}.
      </p>
    </section>
  );
}

// ── Unusual ─────────────────────────────────────────────────────────────────

const NOT_CHECKED: Record<string, string> = {
  week: "Recurring cost changes and new merchants aren't checked on a week: their costs are monthly.",
  in_progress: "Recurring cost changes wait for a finished month.",
  short_history: "Large charges need 30 charges in the year before to know what's large for you.",
  range: "Recurring and large-charge checks cover weeks, months and quarters only.",
};

function notCheckedLines(list: NotChecked[]): string[] {
  // A week lists both recurring checks with the same reason: say it once.
  const seen = new Set<string>();
  const out: string[] = [];
  for (const n of list) {
    const text =
      n.reason === "short_history" || n.check === "large_charges"
        ? NOT_CHECKED.short_history
        : NOT_CHECKED[n.reason];
    if (!seen.has(text)) {
      seen.add(text);
      out.push(text);
    }
  }
  return out;
}

interface Row {
  key: string;
  minor: boolean;
  note: string;
  /** Attention Amber note, else Text Dim. */
  warn: boolean;
  name: string;
  detail: string;
  /** Spend, shown signed (negative). */
  cents: number | null;
  dest: "Flow" | "Settings";
  href: string;
  go?: () => void;
}

/** "monthly", "every ~2 months": how often, from the usual gap (mirrors
 *  core's cadence buckets, for wording only). */
function howOften(days: number): string {
  if (days <= 1.5) return "about daily";
  if (days <= 3.5) return "a few times a week";
  if (days <= 10) return "weekly";
  if (days <= 20) return "a few times a month";
  if (days <= 45) return "monthly";
  return everyFewMonths(days);
}

function rowOf(
  u: Unusual,
  i: number,
  firstLarge: boolean,
  fallbackMonth: string | null,
  onDrill: (p: Pick, m: string) => void,
): Row {
  const r = rowBody(u, firstLarge, fallbackMonth, onDrill);
  return { ...r, key: `row-${i}`, minor: u.minor };
}

function rowBody(
  u: Unusual,
  firstLarge: boolean,
  fallbackMonth: string | null,
  onDrill: (p: Pick, m: string) => void,
): Omit<Row, "key" | "minor"> {
  const day = u.day ? parseDay(u.day) : null;
  const flowGo = (pick: Pick, m: string | null) => (m ? () => onDrill(pick, m) : undefined);
  const flowHref = pathOf("flow");
  const merchantPick = (inMonth: boolean): Pick => ({
    lane: "out",
    direction: "out",
    merchant: u.merchant ?? undefined,
    name: u.label,
    ...(inMonth ? { inMonth } : {}),
  });
  switch (u.kind) {
    case "connection":
      return {
        note: "reconnect",
        warn: true,
        name: u.label,
        detail:
          u.status === "login_required"
            ? "The bank wants you to sign in again."
            : "The connection reports an error.",
        cents: null,
        dest: "Settings",
        href: `${pathOf("settings")}#connections`,
      };
    case "match_review": {
      const n = u.count ?? 0;
      return {
        note: "review",
        warn: true,
        name: n === 1 ? "1 card payment to review" : `${n} card payments to review`,
        detail: "Confirm each pair is one payment, so it isn't counted as spending.",
        cents: null,
        dest: "Settings",
        href: `${pathOf("settings")}#card-payment-review`,
      };
    }
    case "new_recurring":
      return {
        note: "new",
        warn: true,
        name: u.label,
        detail: `New recurring merchant, first charge ${day ? dayLabel(day) : "recently"}${
          u.cents !== null ? `; usually ${unsignedCents(u.cents)} a charge` : ""
        }.`,
        cents: null,
        dest: "Flow",
        href: flowHref,
        go: flowGo(merchantPick(false), u.day?.slice(0, 7) ?? fallbackMonth),
      };
    case "late": {
      // Changed earlier in the period, then went quiet: one row, both said.
      const before =
        u.change_pct !== null && u.month
          ? ` ${cap(monthPhrase(u.month))} was ${u.change_pct > 0 ? "up" : "down"} ${Math.round(
              Math.abs(u.change_pct),
            )}%.`
          : "";
      return {
        note: "late",
        warn: false,
        name: u.label,
        detail: `No charge since ${day ? dayLabel(day) : "a while"}; usually ${howOften(u.gap_days ?? 30)}${
          u.cents !== null ? `, about ${unsignedCents(u.cents)} a charge` : ""
        }.${before}`,
        cents: null,
        dest: "Flow",
        href: flowHref,
        go: flowGo(merchantPick(false), fallbackMonth),
      };
    }
    case "recurring_up":
    case "recurring_down": {
      const pct = Math.round(Math.abs(u.change_pct ?? 0));
      const m = u.month ?? fallbackMonth;
      if (u.per_charge)
        return {
          note: `${u.kind === "recurring_up" ? "up" : "down"} ${pct}%`,
          warn: u.kind === "recurring_up",
          name: u.label,
          detail: `${day ? dayLabel(day) : "Latest"} charge, against a usual ${
            u.usual_cents !== null ? unsignedCents(u.usual_cents) : ""
          } a charge (${howOften(u.gap_days ?? 60)}).`,
          cents: u.cents ? -u.cents : null,
          dest: "Flow",
          href: flowHref,
          go: flowGo(merchantPick(false), m),
        };
      return {
        note: `${u.kind === "recurring_up" ? "up" : "down"} ${pct}%`,
        warn: u.kind === "recurring_up",
        name: u.label,
        detail: `${cap(m ? monthPhrase(m) : "this month")}${
          u.usual_cents !== null ? `; usually ${unsignedCents(u.usual_cents)} a month` : ""
        }.`,
        cents: u.cents ? -u.cents : null,
        dest: "Flow",
        href: flowHref,
        go: flowGo(merchantPick(false), m),
      };
    }
    case "large_charge":
      return {
        note: "large",
        warn: true,
        name: u.label,
        // The threshold is the same on every large row: say it once.
        detail: `${day ? dayLabel(day) : "One charge"}${
          firstLarge && u.usual_cents !== null
            ? `; your top 5% of charges start at ${unsignedCents(u.usual_cents)}`
            : ""
        }.`,
        cents: u.cents !== null ? -u.cents : null,
        dest: "Flow",
        href: flowHref,
        go: flowGo(merchantPick(true), u.day?.slice(0, 7) ?? fallbackMonth),
      };
  }
}

function UnusualList({
  c,
  phrase,
  fallbackMonth,
  onDrill,
}: {
  c: CheckInData;
  phrase: string;
  fallbackMonth: string | null;
  onDrill: (p: Pick, m: string) => void;
}) {
  const firstLarge = c.unusual.findIndex((u) => u.kind === "large_charge");
  const all = c.unusual.map((u, i) => rowOf(u, i, i === firstLarge, fallbackMonth, onDrill));
  const rows = all.filter((r) => !r.minor);
  const minor = all.filter((r) => r.minor);
  const skipped = notCheckedLines(c.not_checked);
  return (
    <section className="ci-unusual" aria-labelledby="ci-unusual-title">
      <h2 id="ci-unusual-title" className="ci-h2">
        Unusual
        {all.length > 0 && (
          <span className="ci-count">
            <span className="lg-sr">, </span>
            {all.length}
            <span className="lg-sr"> {all.length === 1 ? "item" : "items"}</span>
          </span>
        )}
      </h2>
      {all.length === 0 ? (
        <p className="ci-none">Nothing stands out in {phrase}.</p>
      ) : (
        <ul className="ci-list">
          {rows.map((r) => (
            <RowItem key={r.key} r={r} />
          ))}
          {minor.length > 0 && (
            <li className="ci-fold">
              <details>
                <summary>
                  <span className="ci-fold-mark">
                    <Chevron dir="right" />
                  </span>
                  <span className="ci-fold-title">
                    {cap(num(minor.length))} smaller {minor.length === 1 ? "change" : "changes"}
                  </span>
                  <span className="ci-detail">
                    Places you visit often, and changes under $20 a month
                  </span>
                </summary>
                <ul className="ci-sublist">
                  {minor.map((r) => (
                    <RowItem key={r.key} r={r} />
                  ))}
                </ul>
              </details>
            </li>
          )}
        </ul>
      )}
      {skipped.length > 0 && (
        <ul className="ci-skipped" aria-label="Not checked">
          {skipped.map((s) => (
            <li key={s}>{s}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

function RowItem({ r }: { r: Row }) {
  return (
    <li className="ci-row">
      <span className={r.warn ? "ci-note ci-warn" : "ci-note"} aria-hidden="true">
        {r.note}
      </span>
      <span className="ci-main">
        <a
          className="ci-name"
          href={r.href}
          data-return={r.key}
          onClick={(e) => {
            if (e.defaultPrevented || e.button !== 0) return;
            if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
            e.preventDefault();
            returnTo = r.key;
            if (r.go) r.go();
            else navigate(r.href);
          }}
        >
          {r.name}
          <span className="lg-sr">
            , {r.note}
            {r.cents !== null ? `, ${signedCents(r.cents)}` : ""}. {r.detail} Opens in {r.dest}.
          </span>
        </a>
        <span className="ci-detail" aria-hidden="true">
          {r.detail}
        </span>
      </span>
      <span className="ci-amt" aria-hidden="true">
        {r.cents !== null ? signedCents(r.cents) : ""}
      </span>
      <span className="ci-dest" aria-hidden="true">
        {r.dest}
      </span>
    </li>
  );
}
