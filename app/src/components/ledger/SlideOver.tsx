import { useEffect, useMemo, useState } from "react";
import type { Account, Mark, Stream, Transaction, TxnFlag } from "../../types";
import { api } from "../../api";
import { formatCents, formatDate } from "../../format";
import type { ToastMsg } from "../Toast";
import { accountSource, cadenceLabel, dollars } from "./labels";
import { RhythmStrip, SourceChip } from "./Streams";

export function StreamSlideOver({
  stream,
  committed,
  candidate = false,
  onPromote,
  win,
  accounts,
  vocab,
  hasCard,
  onClose,
  onEdited,
  notify,
}: {
  stream: Stream | null;
  committed: boolean;
  /// Candidate = a Noise merchant being previewed, not yet a real stream. Shows
  /// context stats + a "Promote to stream" action instead of Committed/Dismiss.
  candidate?: boolean;
  /// Called with the merchant key when the candidate is promoted. The parent
  /// owns the reload + open-the-new-stream flow (this panel just closes).
  onPromote?: (merchantKey: string) => void | Promise<void>;
  win: import("../../types").Window;
  accounts: Account[];
  vocab: string[];
  hasCard: boolean;
  onClose: () => void;
  onEdited: () => void;
  notify: (m: ToastMsg) => void;
}) {
  const [occ, setOcc] = useState<Transaction[]>([]);
  const [learn, setLearn] = useState(true);
  const [menu, setMenu] = useState<string | null>(null); // txn id with an open ⋯ menu
  const [catFor, setCatFor] = useState<string | null>(null); // txn id in categorize mode
  const [working, setWorking] = useState(false);

  const acctMap = useMemo(() => {
    const m: Record<string, Account> = {};
    for (const a of accounts) m[a.id] = a;
    return m;
  }, [accounts]);

  // Load occurrences whenever a stream opens.
  useEffect(() => {
    if (!stream) return;
    setMenu(null);
    setCatFor(null);
    let live = true;
    api
      .streamOccurrences(stream.merchant, win)
      .then((t) => live && setOcc(t))
      .catch((err) => notify({ kind: "err", text: String(err) }));
    return () => {
      live = false;
    };
  }, [stream, win, notify]);

  // Esc closes.
  useEffect(() => {
    const h = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", h);
    return () => document.removeEventListener("keydown", h);
  }, [onClose]);

  if (!stream) {
    return <div className="lg-scrim" onClick={onClose} />;
  }

  const refetchOcc = () => api.streamOccurrences(stream.merchant, win).then(setOcc);

  // Run an edit, refresh occurrences + the ledger behind the panel.
  const run = async (fn: () => Promise<unknown>, ok: string, close = false) => {
    setWorking(true);
    try {
      await fn();
      await refetchOcc();
      onEdited();
      notify({ kind: "ok", text: ok });
      if (close) onClose();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    } finally {
      setWorking(false);
    }
  };

  // Stream-level: flag the whole merchant (learn always on).
  const flagStream = () => {
    if (occ.length === 0) return;
    run(
      () => api.setFlag(occ[0].id, "Transfer", true).then(() => api.applyFlags()),
      `Flagged “${stream.merchant}” as a transfer`,
      true,
    );
  };
  const markCommitted = () =>
    run(() => api.markStream(stream.merchant, "Committed" as Mark), "Marked as committed", true);
  const returnToHunt = () =>
    run(() => api.markStream(stream.merchant, "Kept" as Mark), "Returned to the hunt", true);
  const dismiss = () =>
    run(() => api.markStream(stream.merchant, "Dismissed" as Mark), "Dismissed to noise", true);
  const promote = async () => {
    if (!onPromote) return;
    setWorking(true);
    try {
      await onPromote(stream.merchant);
    } finally {
      setWorking(false);
    }
  };

  // Context stats for the candidate header, from the loaded occurrences.
  const total = occ.reduce((a, t) => a + Math.abs(t.amount), 0);
  const avg = occ.length ? Math.round(total / occ.length) : 0;

  // Per-occurrence flag, with the card-payment guard.
  const flagTxn = (t: Transaction, flag: TxnFlag) => {
    if (flag === "CardPayment" && !hasCard) {
      const msg = `No card account is ingested — suppressing this removes ~${dollars(
        stream.monthly_estimate_cents,
      )}/mo with no offsetting charges. Flag anyway?`;
      if (!window.confirm(msg)) return;
    }
    setMenu(null);
    run(() => api.setFlag(t.id, flag, learn), `Flagged as ${flag === "CardPayment" ? "card payment" : "transfer"}`);
  };
  const categorize = (t: Transaction, cat: string) => {
    setMenu(null);
    setCatFor(null);
    run(() => api.setCategory(t.id, cat, learn), `Categorized as ${cat}`);
  };

  const primary = stream.sources[0];

  return (
    <>
      <div className="lg-scrim open" onClick={onClose} />
      <aside className="lg-over open">
        <div className="lg-oh">
          <span className="x" onClick={onClose}>
            ✕
          </span>
          <div className="t">
            {stream.merchant} <span className="lg-badge">{cadenceLabel(stream.cadence)}</span>
          </div>
          <div className="subline">
            {stream.occurrence_count} visits
            {primary && <SourceChip s={primary} />}·{" "}
            {candidate ? "candidate" : committed ? "committed" : "in the hunt"}
          </div>
          {candidate && (
            <div className="subline">
              seen {occ.length}× · {dollars(total)} total · {dollars(avg)} avg
            </div>
          )}
          <div className="bignum">
            {dollars(stream.monthly_estimate_cents)}{" "}
            <small>
              /mo · {dollars(stream.median_amount_cents)} median ({dollars(stream.amount_min_cents)}–
              {dollars(stream.amount_max_cents)})
            </small>
          </div>
          <RhythmStrip spark={stream.spark_cents} big />
        </div>

        <div className="lg-sect">
          <h4>{candidate ? "This looks recurring" : "Reclassify this stream"}</h4>
          <div className="lg-acts">
            {candidate ? (
              <button className="lg-act" disabled={working} onClick={promote}>
                <span className="ic">↥</span>
                <span className="grow">Promote to stream</span>
                <span className="hint">out of noise</span>
              </button>
            ) : committed ? (
              <button className="lg-act" disabled={working} onClick={returnToHunt}>
                <span className="ic">↩</span>
                <span className="grow">Return to the hunt</span>
                <span className="hint">back to streams</span>
              </button>
            ) : (
              <button className="lg-act" disabled={working} onClick={markCommitted}>
                <span className="ic">■</span>
                <span className="grow">Mark as Committed</span>
                <span className="hint">hide from the hunt</span>
              </button>
            )}
            <button className="lg-act" disabled={working} onClick={flagStream}>
              <span className="ic">⇄</span>
              <span className="grow">Flag as Transfer</span>
              <span className="hint">not spending</span>
            </button>
            {!candidate && (
              <button className="lg-act danger" disabled={working} onClick={dismiss}>
                <span className="ic">×</span>
                <span className="grow">Dismiss to Noise</span>
                <span className="hint">not a real stream</span>
              </button>
            )}
          </div>
          {!hasCard && (
            <div className="lg-warn">
              <span>⚠</span>
              <span>
                <b>Heads up.</b> No card account is ingested, so flagging this as a transfer or card
                payment removes <b>{dollars(stream.monthly_estimate_cents)}/mo</b> with no offsetting
                charges.
              </span>
            </div>
          )}
        </div>

        <div className="lg-sect">
          <h4>Occurrences · {occ.length}</h4>
          {occ.map((t) => {
            const src = acctMap[t.account_id]
              ? accountSource(acctMap[t.account_id])
              : { label: t.account_id, kind: "Other" as const, pct: 100 };
            return (
              <div key={t.id}>
                <div className="lg-occ" style={t.flag !== "Spending" ? { opacity: 0.5 } : undefined}>
                  <span className="d">{formatDate(t.transacted_at ?? t.posted)}</span>
                  <span className="a">{formatCents(Math.abs(t.amount))}</span>
                  <span className="s">
                    <SourceChip s={src} />
                  </span>
                  <span
                    className="k"
                    onClick={() => setMenu((m) => (m === t.id ? null : t.id))}
                  >
                    ⋯
                  </span>
                </div>
                {menu === t.id && (
                  <div className="lg-menu">
                    <button onClick={() => flagTxn(t, "Transfer")}>Flag as Transfer</button>
                    <button onClick={() => flagTxn(t, "CardPayment")}>Flag as Card payment</button>
                    <button onClick={() => setCatFor(catFor === t.id ? null : t.id)}>
                      Categorize…
                    </button>
                    {catFor === t.id && (
                      <select
                        defaultValue={t.category ?? ""}
                        onChange={(e) => e.target.value && categorize(t, e.target.value)}
                      >
                        <option value="">Pick a category…</option>
                        {vocab.map((c) => (
                          <option key={c} value={c}>
                            {c}
                          </option>
                        ))}
                      </select>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        <div className="lg-learn">
          <label>
            <input type="checkbox" checked={learn} onChange={(e) => setLearn(e.target.checked)} />
            Learn a rule — apply per-transaction choices to all “{stream.merchant}”, now and future
          </label>
        </div>
      </aside>
    </>
  );
}
