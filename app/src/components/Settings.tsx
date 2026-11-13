// Settings: upkeep that doesn't belong on an analysis screen. Bank
// connections, card-payment match review, categorize, and the destructive
// reset all live here. Reset must never appear on Check-in,
// Holdings or Flow.
//
// The screen opens with one line saying what needs a look (from core's
// upkeep summary, the same numbers as the nav badge). Sections that need
// nothing start folded; ones that need a look start open.
import { useCallback, useEffect, useState } from "react";
import { api } from "../api";
import type { ResetPreview, Upkeep } from "../types";
import { Connections } from "./Connections";
import { MatchReview } from "./MatchReview";
import { Section } from "./Section";
import { agoLabel } from "./accounts";
import type { ToastMsg } from "./Toast";
import { errMessage } from "../format";

const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

/** "1 connection needs reconnecting · 2 card payments to review · synced 3d ago" */
function UpkeepLine({ u }: { u: Upkeep }) {
  const parts: { text: string; warn: boolean }[] = [];
  if (u.connections.length > 0)
    parts.push({
      text: `${plural(u.connections.length, "connection needs", "connections need")} reconnecting`,
      warn: true,
    });
  parts.push(
    u.proposed_matches > 0
      ? { text: `${plural(u.proposed_matches, "card payment", "card payments")} to review`, warn: true }
      : { text: "card payments all matched", warn: false },
  );
  parts.push({
    text: u.last_synced ? `synced ${agoLabel(u.last_synced)}` : "never synced",
    warn: false,
  });
  return (
    <p className="st-upkeep">
      {u.total === 0 && <b className="st-allgood">Nothing needs you. </b>}
      {parts.map((p, i) => (
        <span key={i} className={p.warn ? "st-warn" : undefined}>
          {i > 0 && <span aria-hidden="true"> · </span>}
          {i > 0 && <span className="lg-sr">, </span>}
          {p.text}
        </span>
      ))}
    </p>
  );
}

export function Settings({
  notify,
  onChanged,
}: {
  notify: (msg: ToastMsg) => void;
  onChanged: () => void;
}) {
  const [upkeep, setUpkeep] = useState<Upkeep | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const loadUpkeep = useCallback(async () => {
    try {
      setUpkeep(await api.upkeep());
      setLoadError(null);
    } catch (err) {
      setLoadError(errMessage(err));
    }
  }, []);
  useEffect(() => {
    void loadUpkeep();
  }, [loadUpkeep]);

  // A link into Settings (`/settings#connections`, from Check-in) names a
  // section: once the sections are on the page, scroll to it and put focus
  // on its title, then drop the anchor so a reload starts at the top.
  const loaded = upkeep !== null;
  useEffect(() => {
    const id = window.location.hash.slice(1);
    if (!loaded || !id) return;
    let tries = 0;
    let frame = 0;
    const find = () => {
      const el = document.getElementById(id);
      if (!el) {
        if (++tries < 60) frame = window.requestAnimationFrame(find);
        return;
      }
      const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      el.scrollIntoView({ block: "start", behavior: reduce ? "auto" : "smooth" });
      el.querySelector<HTMLElement>(".st-fold")?.focus({ preventScroll: true });
      window.history.replaceState(null, "", window.location.pathname);
    };
    frame = window.requestAnimationFrame(find);
    return () => window.cancelAnimationFrame(frame);
  }, [loaded]);

  // A change in any section can change the counts: refresh the line here
  // and the app's badge.
  const changed = useCallback(() => {
    void loadUpkeep();
    onChanged();
  }, [loadUpkeep, onChanged]);

  const act = async (fn: () => Promise<string>) => {
    setBusy(true);
    try {
      notify({ kind: "ok", text: await fn() });
      changed();
    } catch (err) {
      notify({ kind: "err", text: errMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  const onCategorize = () =>
    act(async () => {
      const r = await api.categorize();
      return `Categorized ${r.rule} by your rules · ${r.remaining} left`;
    });
  const onCategorizeLlm = () =>
    act(async () => {
      const r = await api.categorize();
      const m = await api.categorizeLlm();
      return `Categorized ${r.rule} by your rules and ${m} with AI`;
    });

  if (!upkeep) {
    return (
      <main className="st-wrap" aria-labelledby="st-title" aria-busy={!loadError}>
        <h1 id="st-title" className="lg-sr">
          Settings
        </h1>
        {loadError ? (
          <div className="hd-state" role="alert">
            <p>Couldn't load Settings: {loadError}</p>
            <button className="lg-pill" onClick={() => void loadUpkeep()}>
              Try again
            </button>
          </div>
        ) : (
          <p className="st-upkeep">Loading…</p>
        )}
      </main>
    );
  }

  return (
    <main className="st-wrap" aria-labelledby="st-title">
      <div>
        <h1 id="st-title" className="lg-sr">
          Settings
        </h1>
        <UpkeepLine u={upkeep} />
      </div>

      <Connections notify={notify} onChanged={changed} attention={upkeep.connections.length} />

      <MatchReview notify={notify} onChanged={changed} waiting={upkeep.proposed_matches} />

      <Section
        title="Categorize"
        open={false}
        actions={
          <>
            <button className="lg-pill" disabled={busy} onClick={() => void onCategorize()}>
              Categorize
            </button>
            <button className="lg-pill" disabled={busy} onClick={() => void onCategorizeLlm()}>
              Categorize with AI
            </button>
          </>
        }
      >
        <p className="st-blurb">
          <b>Categorize</b> applies the rules you've taught (each category you set on Flow teaches
          one) to transactions that have none. <b>With AI</b> then sends what's left, merchant
          names and amounts, to the AI categorizer. It works only when the server was built with
          it; otherwise it says so and changes nothing.
        </p>
      </Section>

      <DangerZone notify={notify} onCleared={changed} />
    </main>
  );
}

// ── Danger zone ────────────────────────────────────────────────────────────
// Reset is two deliberate steps on the page (no browser confirm): the first
// asks core what would be lost for good, the second needs that read and
// acknowledged before the red button works.
function DangerZone({
  notify,
  onCleared,
}: {
  notify: (msg: ToastMsg) => void;
  onCleared: () => void;
}) {
  const [preview, setPreview] = useState<ResetPreview | null>(null);
  const [ack, setAck] = useState(false);
  const [busy, setBusy] = useState(false);

  const start = async () => {
    setBusy(true);
    try {
      setPreview(await api.resetPreview());
      setAck(false);
    } catch (err) {
      notify({ kind: "err", text: `Couldn't check what would be lost: ${errMessage(err)}` });
    } finally {
      setBusy(false);
    }
  };
  const cancel = () => {
    setPreview(null);
    setAck(false);
  };
  const clear = async () => {
    setBusy(true);
    try {
      await api.resetData();
      setPreview(null);
      notify({ kind: "ok", text: "Pulled data cleared. Sync now to fetch what Plaid still has." });
      onCleared();
    } catch (err) {
      notify({ kind: "err", text: errMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  const p = preview;
  return (
    <Section title="Danger zone" open={false} danger>
      <p className="st-blurb">
        Clearing pulled data deletes accounts (with their nicknames and hidden settings),
        transactions, current holdings, match decisions and the sync log. It keeps your rules,
        bank links, merchant marks, balance history and investment activity. The next sync fetches
        again only what Plaid still has.
      </p>
      {!p ? (
        <button className="lg-pill st-danger-btn" disabled={busy} onClick={() => void start()}>
          Clear pulled data…
        </button>
      ) : (
        <div className="st-confirm" role="group" aria-labelledby="st-confirm-title">
          <h3 id="st-confirm-title">What clearing would delete</h3>
          <ul>
            <li>
              {plural(p.transactions, "transaction", "transactions")}
              {p.oldest_day && <> going back to {p.oldest_day}</>}.
            </li>
            <li>
              Plaid sends back only the last {p.refetch_days} days (from {p.refetch_from}).{" "}
              <b className="st-lost">
                {p.lost_for_good === 0
                  ? "Nothing older is stored, so nothing is lost for good."
                  : `${plural(p.lost_for_good, "transaction", "transactions")} older than that would be gone for good.`}
              </b>
            </li>
            {p.manual_categories > 0 && (
              <li>{plural(p.manual_categories, "category you set by hand", "categories you set by hand")} on those transactions.</li>
            )}
            <li>
              {plural(p.accounts, "account", "accounts")}
              {p.nicknamed_accounts + p.hidden_accounts > 0 && (
                <>
                  {" "}
                  ({plural(p.nicknamed_accounts, "nickname", "nicknames")},{" "}
                  {plural(p.hidden_accounts, "hidden setting", "hidden settings")})
                </>
              )}
              {p.decided_matches > 0 && <>, and {plural(p.decided_matches, "match decision", "match decisions")}</>}.
            </li>
          </ul>
          <label className="st-ack">
            <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />
            {p.lost_for_good > 0
              ? `I understand ${plural(p.lost_for_good, "transaction", "transactions")} can't be fetched again.`
              : "I understand this deletes the data above."}
          </label>
          <div className="cx-actions">
            <button
              className="lg-pill st-danger-btn"
              disabled={!ack || busy}
              onClick={() => void clear()}
            >
              Delete pulled data
            </button>
            <button className="lg-pill" disabled={busy} onClick={cancel}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </Section>
  );
}
