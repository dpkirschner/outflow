import type { MerchantSpend } from "../types";
import { formatCents } from "../format";

interface Props {
  rows: MerchantSpend[];
}

export function MerchantLeaderboard({ rows }: Props) {
  const max = rows.reduce((m, r) => Math.max(m, r.total_cents), 0);

  return (
    <div className="card col-7">
      <h2>Top merchants</h2>
      {rows.length === 0 ? (
        <div className="muted">Nothing to show.</div>
      ) : (
        <div>
          {rows.map((r) => (
            <div className="bar-row" key={r.merchant}>
              <div className="bar-label">
                <span className="m">
                  {r.merchant}{" "}
                  <span style={{ color: "var(--text-faint)" }}>×{r.count}</span>
                </span>
                <div className="bar-track">
                  <div
                    className="bar-fill"
                    style={{ width: `${max > 0 ? (r.total_cents / max) * 100 : 0}%` }}
                  />
                </div>
              </div>
              <span className="bar-amt">{formatCents(r.total_cents)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
