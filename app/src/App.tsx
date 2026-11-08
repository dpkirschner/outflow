import { Suspense, lazy, useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { Account } from "./types";
import { Toast, toastPersists, type ToastMsg } from "./components/Toast";
import { Flow } from "./components/Flow";
import { Settings } from "./components/Settings";
import { SCREENS, onLinkClick, useScreen } from "./route";
import { errMessage } from "./format";

// Holdings carries the chart library (recharts, most of the bundle); load it
// only when that screen opens.
const Holdings = lazy(() =>
  import("./components/Holdings").then((m) => ({ default: m.Holdings })),
);

export default function App() {
  // The screen comes from the URL (route.ts); /oauth-return maps to Settings
  // so Plaid Link can resume in Connections.
  const tab = useScreen();
  useEffect(() => {
    const label = SCREENS.find((s) => s.screen === tab)?.label;
    document.title = label ? `outflow · ${label}` : "outflow";
  }, [tab]);
  // Upkeep that needs a look (core's upkeep total: connections to reconnect
  // + card payments to review). Shown as a badge on the Settings tab.
  const [attention, setAttention] = useState(0);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [vocab, setVocab] = useState<string[]>([]);
  // The selected calendar month (`YYYY-MM`), shared across screens so Flow
  // and Holdings read the same one. null = the latest with data.
  const [month, setMonth] = useState<string | null>(null);
  // Bumped when a sync on open brings new rows, so Flow reloads its figures.
  const [dataVersion, setDataVersion] = useState(0);
  const [toast, setToast] = useState<ToastMsg | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);

  const notify = useCallback((msg: ToastMsg) => {
    // Rapid-fire notifications must reset the dismiss timer, not stack — an
    // earlier timeout would otherwise fire and clear the newest toast early.
    window.clearTimeout(toastTimer.current);
    setToast(msg);
    // A toast with an action (Undo) or an error waits to be dismissed: no
    // time limit on reaching or reading it (WCAG 2.2.1).
    if (!toastPersists(msg))
      toastTimer.current = window.setTimeout(() => setToast(null), 3200);
  }, []);

  const reload = useCallback(async () => {
    try {
      const [accts, cats, upkeep] = await Promise.all([
        api.accounts(),
        api.categories(),
        api.upkeep().catch(() => null),
      ]);
      setAccounts(accts);
      setVocab(cats);
      setAttention(upkeep?.total ?? 0);
    } catch (err) {
      notify({ kind: "err", text: `Load failed: ${errMessage(err)}` });
    }
  }, [notify]);

  const launched = useRef(false);
  useEffect(() => {
    if (launched.current) return;
    launched.current = true;
    void (async () => {
      await reload();
      // Best-effort live refresh on open; silent if not connected/offline.
      try {
        const r = await api.pullLive();
        if (r.added || r.updated) {
          notify({ kind: "ok", text: `Synced: ${r.added} new, ${r.updated} updated` });
          await reload();
          setDataVersion((v) => v + 1);
        }
      } catch {
        /* not connected / offline — stay quiet on launch */
      }
    })();
  }, [reload, notify]);

  return (
    <div className="lg-app">
      <header className="lg-top">
        <span className="lg-wm">outflow</span>
        <nav className="lg-nav" aria-label="Screens">
          {SCREENS.map(({ screen, path, label }) => {
            const on = tab === screen;
            const badge = screen === "settings" && attention > 0;
            return (
              <a
                key={screen}
                href={path}
                className={on ? "on" : "lg-navlink"}
                aria-current={on ? "page" : undefined}
                onClick={(e) => onLinkClick(e, path)}
              >
                {label}
                {badge && (
                  <>
                    <span className="st-badge" aria-hidden="true">
                      {attention}
                    </span>
                    <span className="lg-sr">
                      , {attention} {attention === 1 ? "item needs" : "items need"} a look
                    </span>
                  </>
                )}
              </a>
            );
          })}
        </nav>
      </header>

      {/* Each screen renders its own <main> with its h1. */}
      {tab === "settings" ? (
        <Settings notify={notify} onChanged={() => void reload()} />
      ) : tab === "holdings" ? (
        <Suspense
          fallback={
            <main className="hd-wrap" aria-busy="true">
              <h1 className="lg-sr">Holdings</h1>
            </main>
          }
        >
          <Holdings key={dataVersion} month={month} onMonth={setMonth} />
        </Suspense>
      ) : (
        <Flow
          key={dataVersion}
          accounts={accounts}
          vocab={vocab}
          notify={notify}
          month={month}
          onMonth={setMonth}
        />
      )}
      {/* Always in the page so screen readers announce what appears in them:
          confirmations politely, errors at once. */}
      <div className="toast-region" role="status">
        {toast?.kind === "ok" && <Toast msg={toast} onDismiss={() => setToast(null)} />}
      </div>
      <div className="toast-region" role="alert">
        {toast?.kind === "err" && <Toast msg={toast} onDismiss={() => setToast(null)} />}
      </div>
    </div>
  );
}
