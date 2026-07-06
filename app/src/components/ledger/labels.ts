// Display helpers for the rhythm ledger.
import type { Account, Source, SourceKind, StreamCadence, Trend } from "../../types";

// Derive a source chip from an account (mirrors core's ledger::account_source):
// cards show their last-4, ACH accounts show their name.
export function accountSource(a: Account): Source {
  const kind: SourceKind = a.kind === "Credit" ? "Card" : "Ach";
  const runs = a.name.match(/\d{4,}/g);
  const last4 = runs ? runs[runs.length - 1].slice(-4) : null;
  return { label: kind === "Card" ? last4 ?? a.name : a.name, kind, pct: 100 };
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
