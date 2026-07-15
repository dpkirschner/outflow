import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { Account, LedgerView, Stream, Window } from "./types";
import { Toast, type ToastMsg } from "./components/Toast";
import { ActionBar } from "./components/ledger/ActionBar";
import { StatStrip, LedgerZones } from "./components/ledger/Zones";
import { StreamsCard, type SortMode } from "./components/ledger/Streams";
import { StreamSlideOver } from "./components/ledger/SlideOver";
import { coverageText } from "./components/ledger/labels";
import { Connections } from "./components/Connections";
import { MatchReview } from "./components/MatchReview";
import { Outflows } from "./components/Outflows";

type Tab = "ledger" | "outflows" | "review" | "connections";

export default function App() {
  // A Plaid OAuth return must land on Connections so Link can resume there.
  const [tab, setTab] = useState<Tab>(
    window.location.pathname === "/oauth-return" ? "connections" : "ledger",
  );
  const [reviewCount, setReviewCount] = useState(0);
  const [view, setView] = useState<LedgerView | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [vocab, setVocab] = useState<string[]>([]);
  const [hasCard, setHasCard] = useState(false);
  const [win, setWin] = useState<Window>("6mo");
  const [sort, setSort] = useState<SortMode>("size");
  const [open, setOpen] = useState<Stream | null>(null);
  // A Noise merchant being previewed for promotion: the clicked normalized key,
  // and the synthesized stream fetched for it.
  const [noiseKey, setNoiseKey] = useState<string | null>(null);
  const [preview, setPreview] = useState<Stream | null>(null);
  // The Noise fold's open state lives here (not in the fold) so it survives
  // opening the promote popover and can be collapsed on promotion.
  const [noiseOpen, setNoiseOpen] = useState(false);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [toast, setToast] = useState<ToastMsg | null>(null);

  const notify = useCallback((msg: ToastMsg) => {
    setToast(msg);
    window.setTimeout(() => setToast(null), 3200);
  }, []);

  const reload = useCallback(
    async (w: Window) => {
      try {
        const [v, accts, cats, card, pending] = await Promise.all([
          api.ledger(w),
          api.accounts(),
          api.categories(),
          api.hasCreditAccount(),
          api.matches("proposed").catch(() => []),
        ]);
        setView(v);
        setAccounts(accts);
        setVocab(cats);
        setHasCard(card);
        setReviewCount(pending.length);
        // Keep an open slide-over in sync with fresh data (or close it if the
        // stream moved out of the streams/committed lists, e.g. dismissed).
        setOpen((cur) =>
          cur
            ? v.streams.find((s) => s.merchant === cur.merchant) ??
              v.committed.find((s) => s.merchant === cur.merchant) ??
              null
            : null,
        );
        return v;
      } catch (err) {
        notify({ kind: "err", text: `Load failed: ${String(err)}` });
        return undefined;
      } finally {
        setLoading(false);
      }
    },
    [notify],
  );

  // Fetch the promote-preview stream whenever a noise merchant is opened (or the
  // window changes while it's open). Clearing noiseKey closes the popover.
  useEffect(() => {
    if (!noiseKey) {
      setPreview(null);
      return;
    }
    let live = true;
    api
      .streamPreview(noiseKey, win)
      .then((s) => {
        if (!live) return;
        if (s) setPreview(s);
        else {
          notify({ kind: "err", text: "No occurrences to preview." });
          setNoiseKey(null);
        }
      })
      .catch((err) => {
        if (live) {
          notify({ kind: "err", text: String(err) });
          setNoiseKey(null);
        }
      });
    return () => {
      live = false;
    };
  }, [noiseKey, win, notify]);

  // Promote a noise merchant to a stream: mark Kept, reload, then close the
  // candidate popover, collapse the Noise fold, and open the promoted stream's
  // own sidebar so the user lands on what they just created.
  const promoteNoise = useCallback(
    async (merchantKey: string) => {
      try {
        await api.markStream(merchantKey, "Kept");
        const v = await reload(win);
        const promoted =
          v?.streams.find((s) => s.merchant === merchantKey) ??
          v?.committed.find((s) => s.merchant === merchantKey) ??
          null;
        setNoiseKey(null);
        setNoiseOpen(false);
        setOpen(promoted);
        notify({ kind: "ok", text: `Promoted “${merchantKey}” to a stream` });
      } catch (err) {
        notify({ kind: "err", text: String(err) });
      }
    },
    [reload, win, notify],
  );

  const launched = useRef(false);
  useEffect(() => {
    if (launched.current) return;
    launched.current = true;
    void (async () => {
      await reload(win);
      // Best-effort live refresh on open; silent if not connected/offline.
      try {
        const r = await api.pullLive();
        if (r.added || r.updated) {
          notify({ kind: "ok", text: `Refreshed: ${r.added} added, ${r.updated} updated` });
          await reload(win);
        }
      } catch {
        /* not connected / offline — stay quiet on launch */
      }
    })();
  }, [reload, notify, win]);

  // Generic action wrapper: flip busy, run, toast, reload.
  const act = useCallback(
    async (fn: () => Promise<string>) => {
      setBusy(true);
      try {
        const msg = await fn();
        notify({ kind: "ok", text: msg });
        await reload(win);
      } catch (err) {
        notify({ kind: "err", text: String(err) });
      } finally {
        setBusy(false);
      }
    },
    [notify, reload, win],
  );

  const onPull = () =>
    act(async () => {
      const r = await api.pullLive();
      return `Pulled ${r.accounts} account(s): ${r.added} added, ${r.updated} updated`;
    });
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
        "Clear all pulled data (accounts + transactions)? Learned rules are kept. Then hit Pull.",
      )
    )
      return;
    void act(async () => {
      await api.resetData();
      return "Data cleared — hit Pull to re-fetch.";
    });
  };
  const onWindow = (w: Window) => {
    setWin(w);
    void reload(w);
  };

  const connected = accounts.length > 0;
  const empty = !loading && !connected && (view?.streams.length ?? 0) === 0;
  // Whether the open stream is currently in the Committed section, so the
  // slide-over offers "Return to the hunt" instead of "Mark as Committed".
  const openCommitted = !!open && !!view && view.committed.some((s) => s.merchant === open.merchant);

  return (
    <div className="lg-app">
      <div className="lg-top">
        <span className="lg-wm">outflow</span>
        <span className="lg-sub">where did my money go?</span>
        <span className="lg-nav">
          <span
            className={tab === "ledger" ? "on" : "lg-navlink"}
            onClick={() => setTab("ledger")}
          >
            Ledger
          </span>
          <span
            className={tab === "outflows" ? "on" : "lg-navlink"}
            onClick={() => setTab("outflows")}
          >
            Outflows
          </span>
          <span
            className={tab === "review" ? "on" : "lg-navlink"}
            onClick={() => setTab("review")}
          >
            Review{reviewCount > 0 ? ` · ${reviewCount}` : ""}
          </span>
          <span
            className={tab === "connections" ? "on" : "lg-navlink"}
            onClick={() => setTab("connections")}
          >
            Connections
          </span>
        </span>
      </div>

      {tab === "connections" ? (
        <>
          <Connections notify={notify} />
          {toast && <Toast msg={toast} />}
        </>
      ) : tab === "review" ? (
        <>
          <MatchReview notify={notify} onChanged={() => void reload(win)} />
          {toast && <Toast msg={toast} />}
        </>
      ) : tab === "outflows" ? (
        <>
          <Outflows accounts={accounts} vocab={vocab} notify={notify} />
          {toast && <Toast msg={toast} />}
        </>
      ) : (
        // Called as a function, NOT <LedgerTab/>: a nested component rendered as
        // an element gets a fresh identity each render and remounts its whole
        // subtree (resetting fold/expand state). Inlining keeps that state.
        LedgerTab()
      )}
    </div>
  );

  // Original single-screen ledger, as a closure over the state above. Contains
  // no hooks, so calling it inline is safe (see the call site above).
  function LedgerTab() {
    return (
      <>
      <ActionBar
        accounts={accounts}
        busy={busy || loading}
        window={win}
        onWindow={onWindow}
        onPull={onPull}
        onRefresh={() => reload(win)}
        onCategorize={onCategorize}
        onCategorizeLlm={onCategorizeLlm}
        onReset={onReset}
      />

      {loading ? (
        <div className="lg-empty">Loading…</div>
      ) : empty ? (
        <div className="lg-empty">
          No data yet — open <b>Connections</b> to link your banks through Plaid.
        </div>
      ) : (
        view && (
          <>
            <div className="lg-coverage">{coverageText(view.coverage, win)}</div>
            <StatStrip stats={view.stats} />
            <StreamsCard
              streams={view.streams}
              mode={sort}
              onMode={setSort}
              activeMerchant={open?.merchant ?? null}
              onOpen={setOpen}
            >
              <LedgerZones
                view={view}
                onOpen={setOpen}
                onOpenNoise={setNoiseKey}
                noiseOpen={noiseOpen}
                onNoiseToggle={() => setNoiseOpen((o) => !o)}
              />
            </StreamsCard>
          </>
        )
      )}

      <StreamSlideOver
        stream={open}
        committed={openCommitted}
        win={win}
        accounts={accounts}
        vocab={vocab}
        hasCard={hasCard}
        onClose={() => setOpen(null)}
        onEdited={() => reload(win)}
        notify={notify}
      />

      {/* Promote-from-Noise popover — same slide-over in candidate mode. */}
      <StreamSlideOver
        stream={preview}
        committed={false}
        candidate
        onPromote={promoteNoise}
        win={win}
        accounts={accounts}
        vocab={vocab}
        hasCard={hasCard}
        onClose={() => setNoiseKey(null)}
        onEdited={() => reload(win)}
        notify={notify}
      />

      {toast && <Toast msg={toast} />}
      </>
    );
  }
}
