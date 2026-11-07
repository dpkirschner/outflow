// Account display helpers shared by every screen: the label an account goes
// by, its kind-tinted chip, and a relative "last synced" label.
import type { Account, AccountKind } from "../types";

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

/** What an account chip shows: the display label, tinted by account kind. */
export interface AccountTag {
  label: string;
  kind: AccountKind;
}

export function accountSource(a: Account): AccountTag {
  return { label: accountDisplayLabel(a.name, a.nickname), kind: a.kind };
}

export function SourceChip({ s }: { s: AccountTag }) {
  return <span className={`lg-src ${s.kind.toLowerCase()}`}>{s.label}</span>;
}

// "today" / "2d ago" / "3w ago" / "4mo ago", from an epoch-seconds timestamp.
export function agoLabel(epochSecs: number): string {
  const days = Math.max(0, Math.floor((Date.now() / 1000 - epochSecs) / 86400));
  if (days === 0) return "today";
  if (days === 1) return "1d ago";
  if (days < 21) return `${days}d ago`;
  const weeks = Math.round(days / 7);
  if (weeks < 9) return `${weeks}w ago`;
  return `${Math.round(days / 30.44)}mo ago`;
}
