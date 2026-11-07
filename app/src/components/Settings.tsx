// Settings: upkeep that doesn't belong on an analysis screen. Bank
// connections, card-payment match review, categorize, and the destructive
// reset all live here (ADR 0001). Reset must never appear on Check-in,
// Holdings or Flow.
import { useState } from "react";
import { api } from "../api";
import { Connections } from "./Connections";
import { MatchReview } from "./MatchReview";
import type { ToastMsg } from "./Toast";

export function Settings({
  notify,
  onChanged,
}: {
  notify: (msg: ToastMsg) => void;
  onChanged: () => void;
}) {
  const [busy, setBusy] = useState(false);

  const act = async (fn: () => Promise<string>) => {
    setBusy(true);
    try {
      notify({ kind: "ok", text: await fn() });
      onChanged();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    } finally {
      setBusy(false);
    }
  };

  const onCategorize = () =>
    act(async () => {
      const r = await api.categorize();
      return `Categorized ${r.rule} by rule · ${r.remaining} left`;
    });
  const onCategorizeLlm = () =>
    act(async () => {
      const r = await api.categorize();
      const m = await api.categorizeLlm();
      return `Categorized ${r.rule} by rule · ${m} by AI`;
    });
  const onReset = () => {
    if (
      !window.confirm(
        "Clear all pulled data (accounts, transactions, match decisions)? Learned rules and bank links are kept. Then hit Sync now.",
      )
    )
      return;
    void act(async () => {
      await api.resetData();
      return "Data cleared — hit Sync now to re-fetch.";
    });
  };

  return (
    <div className="st-wrap">
      <section className="st-section">
        <Connections notify={notify} onChanged={onChanged} />
      </section>

      <section className="st-section">
        <MatchReview notify={notify} onChanged={onChanged} />
      </section>

      <section className="st-section">
        <div className="cx-head">
          <h2>Categorize</h2>
          <div className="cx-actions">
            <button className="lg-pill" disabled={busy} onClick={() => void onCategorize()}>
              Categorize
            </button>
            <button className="lg-pill" disabled={busy} onClick={() => void onCategorizeLlm()}>
              Categorize · AI
            </button>
          </div>
        </div>
        <p className="st-blurb">
          Applies your learned rules to uncategorized transactions. <b>AI</b> sends what the
          rules leave to the LLM categorizer (needs the server's <code>net</code> feature).
        </p>
      </section>

      <section className="st-section st-danger">
        <div className="cx-head">
          <h2>Reset</h2>
          <div className="cx-actions">
            <button className="lg-pill ghost" disabled={busy} onClick={onReset}>
              Clear pulled data
            </button>
          </div>
        </div>
        <p className="st-blurb">
          Deletes pulled accounts (with their nicknames and hidden settings), transactions,
          current holdings, match decisions and the sync log. Kept: learned rules, bank links,
          merchant marks, balance history and investment activity. The next sync re-fetches
          what Plaid still serves; transactions older than its window are gone for good.
        </p>
      </section>
    </div>
  );
}
