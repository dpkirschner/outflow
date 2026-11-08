// Card-payment match review: proposed checking→card payment pairs awaiting a
// decision, plus recently accepted pairs. Accepting flags both legs
// CardPayment (excluded from all spend analytics); "Different" keeps them as
// spending and never re-proposes the pair. "Back to review" on an accepted
// pair returns it to proposed (core's unaccept), so nothing is lost.
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { MatchView } from "../types";
import { unsignedCents, errMessage } from "../format";
import { agoLabel } from "./accounts";
import { Section } from "./Section";
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
  waiting,
}: {
  notify: (msg: ToastMsg) => void;
  onChanged: () => void;
  /** Pairs waiting for a decision (core's upkeep): the section starts open
   *  when there are any. */
  waiting: number;
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
      notify({ kind: "err", text: `Couldn't load card payments: ${errMessage(err)}` });
    }
  }, [notify]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const reopen = async (id: number) => {
    setBusy(true);
    try {
      await api.unacceptMatch(id);
      notify({
        kind: "ok",
        text: "Back in review. Until you decide, both sides count as spending.",
      });
      await refresh();
      onChanged();
    } catch (err) {
      notify({ kind: "err", text: errMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  const decide = async (id: number, ok: boolean) => {
    setBusy(true);
    try {
      if (ok) await api.acceptMatch(id);
      else await api.rejectMatch(id);
      notify({
        kind: "ok",
        text: ok
          ? "Same payment: both sides now count as a card payment, not spending."
          : "Kept apart: both still count as spending.",
      });
      await refresh();
      onChanged();
    } catch (err) {
      notify({ kind: "err", text: errMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Section
      title="Card-payment review"
      status={
        waiting > 0 ? (
          <span className="st-warn">{waiting} to review</span>
        ) : (
          "all matched"
        )
      }
      open={waiting > 0}
    >
      <div className="mr-wrap">
        <p className="mr-blurb">
          A payment from checking to a card is counted once, so the card's charges aren't counted
          twice. Sure pairs are matched on their own; these need your call.
        </p>

        {proposed.length === 0 ? (
          <p className="st-done">All card payments are matched.</p>
        ) : (
          <ul className="mr-list">
            {proposed.map((mv) => (
              <li key={mv.id} className="mr-item">
                <div className="mr-amount">{unsignedCents(mv.bank?.amount ?? 0)}</div>
                <div className="mr-legs">
                  <TxnCell label="bank" txn={mv.bank} />
                  <TxnCell label="card" txn={mv.card} />
                  <div className="mr-reason">{mv.reason}</div>
                </div>
                <div className="cx-actions">
                  <button className="lg-pill key" disabled={busy} onClick={() => void decide(mv.id, true)}>
                    Same payment
                  </button>
                  <button className="lg-pill" disabled={busy} onClick={() => void decide(mv.id, false)}>
                    Different
                  </button>
                </div>
              </li>
            ))}
          </ul>
        )}

        {accepted.length > 0 && (
          <div className="mr-accepted">
            <h3>Recently matched</h3>
            <ul className="mr-list">
              {accepted.map((mv) => (
                <li key={mv.id} className="mr-item mr-done">
                  <div className="mr-amount">{unsignedCents(mv.bank?.amount ?? 0)}</div>
                  <div className="mr-legs">
                    <TxnCell label="bank" txn={mv.bank} />
                    <TxnCell label="card" txn={mv.card} />
                  </div>
                  <div className="cx-actions">
                    <button className="lg-pill" disabled={busy} onClick={() => void reopen(mv.id)}>
                      Back to review
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </Section>
  );
}
