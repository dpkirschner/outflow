import { useState } from "react";
import type { Account, Window } from "../../types";

const WINDOWS: Window[] = ["3mo", "6mo", "12mo", "all"];
const WIN_LABEL: Record<Window, string> = { "3mo": "3mo", "6mo": "6mo", "12mo": "12mo", all: "All" };

function syncedAgo(accounts: Account[]): string {
  if (accounts.length === 0) return "not connected";
  const last = Math.max(...accounts.map((a) => a.last_synced));
  if (!last) return `${accounts.length} account${accounts.length === 1 ? "" : "s"}`;
  const mins = Math.max(0, Math.round((Date.now() / 1000 - last) / 60));
  const when = mins < 1 ? "just now" : mins < 60 ? `${mins}m ago` : `${Math.round(mins / 60)}h ago`;
  return `synced ${when} · ${accounts.length} account${accounts.length === 1 ? "" : "s"}`;
}

export function ActionBar({
  accounts,
  busy,
  window,
  onWindow,
  onConnect,
  onPull,
  onRefresh,
  onCategorize,
  onCategorizeLlm,
  onReset,
}: {
  accounts: Account[];
  busy: boolean;
  window: Window;
  onWindow: (w: Window) => void;
  onConnect: (token: string) => void;
  onPull: () => void;
  onRefresh: () => void;
  onCategorize: () => void;
  onCategorizeLlm: () => void;
  onReset: () => void;
}) {
  const [connecting, setConnecting] = useState(false);
  const [token, setToken] = useState("");

  return (
    <>
      <div className="lg-bar">
        <button className="lg-pill ghost" disabled={busy} onClick={() => setConnecting((c) => !c)}>
          Connect
        </button>
        <button className="lg-pill key" disabled={busy} onClick={onPull}>
          Pull
        </button>
        <button className="lg-pill" disabled={busy} onClick={onRefresh}>
          Refresh
        </button>
        <button className="lg-pill" disabled={busy} onClick={onCategorize}>
          Categorize
        </button>
        <button className="lg-pill" disabled={busy} onClick={onCategorizeLlm}>
          Categorize · AI
        </button>
        <button className="lg-pill ghost" disabled={busy} onClick={onReset}>
          Reset
        </button>
        <span className="lg-win">
          {WINDOWS.map((w) => (
            <button key={w} className={w === window ? "on" : ""} onClick={() => onWindow(w)}>
              {WIN_LABEL[w]}
            </button>
          ))}
        </span>
        <span className="lg-status">
          <span className={`lg-dot${accounts.length ? "" : " off"}`} />
          {syncedAgo(accounts)}
        </span>
      </div>
      {connecting && (
        <div className="lg-connect">
          <input
            autoFocus
            placeholder="Paste your SimpleFIN setup token…"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && token.trim()) {
                onConnect(token.trim());
                setToken("");
                setConnecting(false);
              }
            }}
          />
          <button
            className="lg-pill key"
            disabled={busy || !token.trim()}
            onClick={() => {
              onConnect(token.trim());
              setToken("");
              setConnecting(false);
            }}
          >
            Connect
          </button>
        </div>
      )}
    </>
  );
}
