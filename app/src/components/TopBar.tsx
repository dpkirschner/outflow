import type { Account } from "../types";
import { formatCents } from "../format";

interface Props {
  accounts: Account[];
  busy: boolean;
  onCategorize: () => void;
  onRefresh: () => void;
}

export function TopBar({ accounts, busy, onCategorize, onRefresh }: Props) {
  return (
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
        <button className="btn" onClick={onRefresh} disabled={busy}>
          Refresh
        </button>
        <button className="btn primary" onClick={onCategorize} disabled={busy}>
          Categorize
        </button>
      </div>
    </div>
  );
}
