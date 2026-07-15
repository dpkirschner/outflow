// Card-payment match review: proposed checking→card payment pairs awaiting a
// decision, plus recently auto-accepted pairs with an undo. Accepting flags
// both legs CardPayment (excluded from all spend analytics); rejecting keeps
// them as spending and never re-proposes the pair.
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { MatchView } from "../types";
import { agoLabel, dollars } from "./ledger/labels";
import type { ToastMsg } from "./Toast";

function TxnCell({ label, txn }: { label: string; txn: MatchView["bank"] }) {
  if (!txn) return <div className="mr-txn mr-missing">{label}: transaction gone</div>;
  return (
    <div className="mr-txn">
      <span className="mr-side">{label}</span>
      <b>{txn.payee ?? txn.description}</b>
      <span className="mr-date">{agoLabel(txn.transacted_at ?? txn.posted)}</span>
    </div>
  );
}

export function MatchReview({
  notify,
  onChanged,
}: {
  notify: (msg: ToastMsg) => void;
  onChanged: () => void;
}) {
  const [proposed, setProposed] = useState<MatchView[]>([]);
  const [accepted, setAccepted] = useState<MatchView[]>([]);
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      const [p, a] = await Promise.all([api.matches("proposed"), api.matches("accepted")]);
      setProposed(p);
      setAccepted(a.slice(0, 20));
    } catch (err) {
      notify({ kind: "err", text: `Load failed: ${String(err)}` });
    }
  }, [notify]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const decide = async (id: number, ok: boolean) => {
    setBusy(true);
    try {
      if (ok) await api.acceptMatch(id);
      else await api.rejectMatch(id);
      notify({ kind: "ok", text: ok ? "Match accepted — both legs excluded." : "Rejected." });
      await refresh();
      onChanged();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mr-wrap">
      <div className="cx-head">
        <h2>Card-payment review</h2>
      </div>
      <p className="mr-blurb">
        Checking→card payments get excluded from outflows so the card's charges aren't counted
        twice. High-confidence pairs are excluded automatically; these need your call.
      </p>

      {proposed.length === 0 ? (
        <div className="lg-empty">Nothing to review.</div>
      ) : (
        <ul className="mr-list">
          {proposed.map((mv) => (
            <li key={mv.id} className="mr-item">
              <div className="mr-amount">{dollars(mv.bank?.amount ?? 0)}</div>
              <div className="mr-legs">
                <TxnCell label="bank" txn={mv.bank} />
                <TxnCell label="card" txn={mv.card} />
                <div className="mr-reason">{mv.reason}</div>
              </div>
              <div className="cx-actions">
                <button className="lg-pill key" disabled={busy} onClick={() => void decide(mv.id, true)}>
                  Same payment
                </button>
                <button className="lg-pill ghost" disabled={busy} onClick={() => void decide(mv.id, false)}>
                  Different
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {accepted.length > 0 && (
        <div className="mr-accepted">
          <h3>Recently excluded</h3>
          <ul className="mr-list">
            {accepted.map((mv) => (
              <li key={mv.id} className="mr-item mr-dim">
                <div className="mr-amount">{dollars(mv.bank?.amount ?? 0)}</div>
                <div className="mr-legs">
                  <TxnCell label="bank" txn={mv.bank} />
                  <TxnCell label="card" txn={mv.card} />
                </div>
                <div className="cx-actions">
                  <button
                    className="lg-pill ghost"
                    disabled={busy}
                    onClick={() => void decide(mv.id, false)}
                  >
                    Undo
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
