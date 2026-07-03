import { useMemo, useState } from "react";
import { api } from "../api";
import type { CategorySource, Transaction } from "../types";
import type { ToastMsg } from "./Toast";
import { formatCents, formatDate } from "../format";

interface Props {
  txns: Transaction[];
  vocab: string[];
  onChanged: () => void | Promise<void>;
  notify: (msg: ToastMsg) => void;
}

function sourceBadge(src: CategorySource | null) {
  if (!src) return null;
  const cls = `badge src-${src.toLowerCase()}`;
  return <span className={cls}>{src.toLowerCase()}</span>;
}

export function TransactionList({ txns, vocab, onChanged, notify }: Props) {
  const [learn, setLearn] = useState(true);
  const [query, setQuery] = useState("");
  const [savingId, setSavingId] = useState<string | null>(null);

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return txns;
    return txns.filter((t) => {
      const merchant = (t.payee ?? t.description).toLowerCase();
      return (
        merchant.includes(q) ||
        t.description.toLowerCase().includes(q) ||
        (t.category ?? "").toLowerCase().includes(q)
      );
    });
  }, [txns, query]);

  async function change(t: Transaction, category: string) {
    if (!category || category === t.category) return;
    setSavingId(t.id);
    try {
      const ruleId = await api.setCategory(t.id, category, learn);
      notify({
        kind: "ok",
        text:
          ruleId != null
            ? `Set “${t.payee ?? t.description}” → ${category} · learned rule #${ruleId}`
            : `Set → ${category}`,
      });
      await onChanged();
    } catch (err) {
      notify({ kind: "err", text: `Update failed: ${String(err)}` });
    } finally {
      setSavingId(null);
    }
  }

  return (
    <div className="card col-12">
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 12,
          flexWrap: "wrap",
          marginBottom: 12,
        }}
      >
        <h2 style={{ margin: 0 }}>
          Transactions{" "}
          <span style={{ color: "var(--text-faint)", fontWeight: 400 }}>
            {filtered.length}
            {filtered.length !== txns.length ? ` / ${txns.length}` : ""}
          </span>
        </h2>
        <div style={{ display: "flex", gap: 14, alignItems: "center" }}>
          <input
            className="cat"
            style={{ maxWidth: 220 }}
            placeholder="Filter merchant / category…"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
          <label className="learn">
            <input
              type="checkbox"
              checked={learn}
              onChange={(e) => setLearn(e.target.checked)}
            />
            Learn a rule for this merchant on change
          </label>
        </div>
      </div>

      <div className="scroll-y">
        <table className="tbl">
          <thead>
            <tr>
              <th style={{ width: 110 }}>Date</th>
              <th>Merchant</th>
              <th style={{ width: 90 }}>Source</th>
              <th className="num" style={{ width: 120 }}>
                Amount
              </th>
              <th style={{ width: 176 }}>Category</th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((t) => {
              const merchant = t.payee ?? t.description;
              const isOut = t.amount < 0;
              return (
                <tr key={t.id} style={savingId === t.id ? { opacity: 0.55 } : undefined}>
                  <td style={{ color: "var(--text-dim)", whiteSpace: "nowrap" }}>
                    {formatDate(t.posted)}
                  </td>
                  <td>
                    <div style={{ display: "flex", flexDirection: "column" }}>
                      <span>
                        {merchant}
                        {t.pending && <span className="badge pending" style={{ marginLeft: 8 }}>pending</span>}
                      </span>
                      {t.description !== merchant && (
                        <span style={{ color: "var(--text-faint)", fontSize: 11 }}>
                          {t.description}
                        </span>
                      )}
                    </div>
                  </td>
                  <td>{sourceBadge(t.category_source)}</td>
                  <td className={`num ${isOut ? "amt-neg" : "amt-pos"}`}>
                    {formatCents(t.amount)}
                  </td>
                  <td>
                    <select
                      className="cat"
                      value={t.category ?? ""}
                      disabled={savingId === t.id}
                      onChange={(e) => change(t, e.target.value)}
                    >
                      <option value="" disabled>
                        {t.category ? t.category : "Set category…"}
                      </option>
                      {vocab.map((c) => (
                        <option key={c} value={c}>
                          {c}
                        </option>
                      ))}
                    </select>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
