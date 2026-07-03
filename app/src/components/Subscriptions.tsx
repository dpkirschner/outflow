import type { Subscription } from "../types";
import { formatCents, formatDate } from "../format";

interface Props {
  rows: Subscription[];
}

export function Subscriptions({ rows }: Props) {
  return (
    <div className="card col-12">
      <h2>Recurring charges</h2>
      {rows.length === 0 ? (
        <div className="muted">No recurring charges detected yet (needs ≥3 regular hits).</div>
      ) : (
        <table className="tbl">
          <thead>
            <tr>
              <th>Merchant</th>
              <th>Cadence</th>
              <th className="num">Typical</th>
              <th className="num">Count</th>
              <th className="num">Total</th>
              <th>Last seen</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((s) => (
              <tr key={s.payee}>
                <td>{s.payee}</td>
                <td>
                  <span
                    className={`badge ${s.cadence === "Yearly" ? "cadence-yearly" : ""}`}
                  >
                    {s.cadence.toLowerCase()}
                  </span>
                </td>
                <td className="num">{formatCents(s.typical_amount_cents)}</td>
                <td className="num">{s.occurrences}</td>
                <td className="num amt-neg">{formatCents(s.total_cents)}</td>
                <td style={{ color: "var(--text-dim)" }}>{formatDate(s.last_seen)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
