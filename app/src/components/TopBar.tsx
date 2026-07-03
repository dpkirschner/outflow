import { useState } from "react";
import type { Account } from "../types";
import { formatCents } from "../format";

interface Props {
  accounts: Account[];
  busy: boolean;
  onPull: () => void;
  onCategorize: () => void;
  onCategorizeLlm: () => void;
  onClaim: (token: string) => void;
  onRefresh: () => void;
}

export function TopBar({
  accounts,
  busy,
  onPull,
  onCategorize,
  onCategorizeLlm,
  onClaim,
  onRefresh,
}: Props) {
  const [connecting, setConnecting] = useState(false);
  const [token, setToken] = useState("");

  function submitClaim() {
    const t = token.trim();
    if (!t) return;
    onClaim(t);
    setToken("");
    setConnecting(false);
  }

  return (
    <div>
      <div className="topbar">
        <div className="brand">
          <h1>outflow</h1>
          <span className="tag">where did my money go?</span>
        </div>

        <div className="topbar-right">
          <div className="chips">
            {accounts.map((a) => (
              <div className="chip" key={a.id}>
                <span className="name">{a.name}</span>
                <span className="bal">{formatCents(a.balance)}</span>
              </div>
            ))}
          </div>
          <button className="btn" onClick={() => setConnecting((v) => !v)} disabled={busy}>
            Connect
          </button>
          <button className="btn" onClick={onRefresh} disabled={busy}>
            Refresh
          </button>
          <button className="btn primary" onClick={onPull} disabled={busy}>
            Pull
          </button>
          <button className="btn" onClick={onCategorize} disabled={busy}>
            Categorize
          </button>
          <button className="btn" onClick={onCategorizeLlm} disabled={busy}>
            Categorize · AI
          </button>
        </div>
      </div>

      {connecting && (
        <div className="connect-row">
          <input
            className="cat"
            style={{ flex: 1, maxWidth: 520 }}
            placeholder="Paste SimpleFIN setup token, then Save…"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && submitClaim()}
            autoFocus
          />
          <button className="btn primary" onClick={submitClaim} disabled={busy || !token.trim()}>
            Save
          </button>
          <button className="btn" onClick={() => setConnecting(false)}>
            Cancel
          </button>
        </div>
      )}
    </div>
  );
}
