import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { Account, LedgerView, Stream, Window } from "./types";
import { Toast, type ToastMsg } from "./components/Toast";
import { ActionBar } from "./components/ledger/ActionBar";
import { StatStrip, LedgerZones } from "./components/ledger/Zones";
import { StreamsCard, type SortMode } from "./components/ledger/Streams";
import { StreamSlideOver } from "./components/ledger/SlideOver";
import { coverageText } from "./components/ledger/labels";

export default function App() {
  const [view, setView] = useState<LedgerView | null>(null);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [vocab, setVocab] = useState<string[]>([]);
  const [hasCard, setHasCard] = useState(false);
  const [win, setWin] = useState<Window>("6mo");
  const [sort, setSort] = useState<SortMode>("size");
  const [open, setOpen] = useState<Stream | null>(null);
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
        const [v, accts, cats, card] = await Promise.all([
          api.ledger(w),
          api.accounts(),
          api.categories(),
          api.hasCreditAccount(),
        ]);
        setView(v);
        setAccounts(accts);
        setVocab(cats);
        setHasCard(card);
        // Keep an open slide-over in sync with fresh data (or close it if the
        // stream moved out of the streams/committed lists, e.g. dismissed).
        setOpen((cur) =>
          cur
            ? v.streams.find((s) => s.merchant === cur.merchant) ??
              v.committed.find((s) => s.merchant === cur.merchant) ??
              null
            : null,
        );
      } catch (err) {
        notify({ kind: "err", text: `Load failed: ${String(err)}` });
      } finally {
        setLoading(false);
      }
    },
    [notify],
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
  const onConnect = (token: string) => act(() => api.claim(token));
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
          <span className="on">Ledger</span>
          <span>Flow · soon</span>
          <span>Reconcile · soon</span>
        </span>
      </div>

      <ActionBar
        accounts={accounts}
        busy={busy || loading}
        window={win}
        onWindow={onWindow}
        onConnect={onConnect}
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
          No data yet — click <b>Connect</b> to link your bank with a SimpleFIN setup token, then
          hit <b>Pull</b>.
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
              <LedgerZones view={view} onOpen={setOpen} />
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

      {toast && <Toast msg={toast} />}
    </div>
  );
}
