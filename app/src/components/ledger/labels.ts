// Display helpers for the rhythm ledger.
import type { Account, Coverage, Source, StreamCadence, Trend, Window } from "../../types";
import { formatDate } from "../../format";

const WINDOW_SPAN: Record<Window, string> = {
  "3mo": "the last 3 months",
  "6mo": "the last 6 months",
  "12mo": "the last 12 months",
  all: "all time",
};

// A plain-language coverage line: always states how far back the data goes, and
// whether the current window is showing all of it or clipping — so "window is at
// 12mo" is distinguishable from "you only have 3 months of data." No date math.
export function coverageText(c: Coverage, win: Window): string {
  if (c.earliest === null) return "No transactions yet — Connect and Pull.";
  const since = formatDate(c.earliest);
  const n = c.window_txn_count;
  const txns = `${n} transaction${n === 1 ? "" : "s"}`;
  const tail = c.covers_all ? "showing all of it" : `showing ${WINDOW_SPAN[win]}`;
  return `Data since ${since} · ${tail} · ${txns}`;
}

// Display label for an account chip: the user's nickname (when set) with the
// auto last-4 mask preserved, else the raw name. Mirror of core's
// `account_display_label` (core/src/model.rs) — plaid parsing bakes the mask
// onto `name` as a trailing " (1234)"; strip it off and re-attach to the nickname.
export function accountDisplayLabel(name: string, nickname: string | null): string {
  const nick = nickname?.trim();
  if (!nick) return name;
  const trimmed = name.trimEnd();
  if (trimmed.endsWith(")")) {
    const open = trimmed.lastIndexOf(" (");
    if (open >= 0) return `${nick} ${trimmed.slice(open + 1)}`;
  }
  return nick;
}

// Derive a source chip from an account (mirrors core's ledger::account_source):
// the display label, colored by the account kind.
export function accountSource(a: Account): Source {
  return { label: accountDisplayLabel(a.name, a.nickname), kind: a.kind, pct: 100 };
}

// Whole-dollar money for the ledger (the mockup drops cents): "$1,310".
export function dollars(cents: number): string {
  return "$" + Math.round(Math.abs(cents) / 100).toLocaleString();
}

export function cadenceLabel(c: StreamCadence): string {
  switch (c) {
    case "Daily":
      return "~daily";
    case "FewPerWeek":
      return "~3×/wk";
    case "Weekly":
      return "~weekly";
    case "FewPerMonth":
      return "~4×/mo";
    case "Monthly":
      return "monthly";
    case "Yearly":
      return "yearly";
    case "Irregular":
      return "irregular";
  }
}

// "last 2d ago" / "last 3w ago" / "today", from an epoch-seconds timestamp.
export function agoLabel(epochSecs: number): string {
  const days = Math.max(0, Math.floor((Date.now() / 1000 - epochSecs) / 86400));
  if (days === 0) return "today";
  if (days === 1) return "1d ago";
  if (days < 21) return `${days}d ago`;
  const weeks = Math.round(days / 7);
  if (weeks < 9) return `${weeks}w ago`;
  return `${Math.round(days / 30.44)}mo ago`;
}

// Signed trend, e.g. "↑ 34%" / "↓ 8%" / "→ flat". Rising = spending more.
export function trendLabel(pct: number, trend: Trend): string {
  const p = Math.round(Math.abs(pct));
  if (trend === "Rising") return `↑ ${p}% vs 3mo avg`;
  if (trend === "Falling") return `↓ ${p}% vs 3mo avg`;
  return "→ flat";
}

// Row modifier class → colors the trend (up = red/more spend, down = green).
export function trendClass(trend: Trend): string {
  if (trend === "Rising") return "up";
  if (trend === "Falling") return "dn";
  return "fl";
}
