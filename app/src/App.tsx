import { Suspense, lazy, useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { api } from "./api";
import type { Account } from "./types";
import { Toast, toastPersists, type ToastMsg } from "./components/Toast";
import { Flow, type Pick } from "./components/Flow";
import { CheckIn, type CheckinNav } from "./components/CheckIn";
import { Settings } from "./components/Settings";
import { FindMoney } from "./components/FindMoney";
import { SCREENS, navigate, onLinkClick, pathOf, useScreen } from "./route";
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
  // A screen change that drops focus (the link that was used is gone) puts
  // it on the new screen's heading, not the page. A screen that moves focus
  // itself (a drill-down, Back to a Check-in row) does so later and wins.
  const firstScreen = useRef(true);
  useEffect(() => {
    if (firstScreen.current) {
      firstScreen.current = false;
      return;
    }
    // Checked again for a moment: a screen swaps its loading <main> for the
    // loaded one, which drops focus a second time.
    const place = () => {
      const lost = !document.activeElement || document.activeElement === document.body;
      const h1 = document.querySelector<HTMLElement>("main h1");
      if (!lost || !h1) return;
      if (!h1.hasAttribute("tabindex")) h1.setAttribute("tabindex", "-1");
      h1.focus({ preventScroll: true });
    };
    const timers = [0, 300, 900, 1800].map((ms) => window.setTimeout(place, ms));
    return () => timers.forEach((t) => window.clearTimeout(t));
  }, [tab]);
  // Upkeep that needs a look (core's upkeep total: connections to reconnect
  // + card payments to review). Shown as a badge on the Settings tab.
  const [attention, setAttention] = useState(0);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [vocab, setVocab] = useState<string[]>([]);
  // The selected calendar month (`YYYY-MM`), shared across screens so Flow
  // and Holdings read the same one. null = the latest with data.
  const [month, setMonth] = useState<string | null>(null);
  // Check-in's view (week / month / quarter) and its week and quarter.
  const [checkin, setCheckin] = useState<CheckinNav>({ view: "month", week: null, quarter: null });
  // A drill-down from Check-in into Flow: Flow reads it once as it opens.
  const [flowPick, setFlowPick] = useState<Pick | null>(null);
  const drill = useCallback((pick: Pick, m: string) => {
    setMonth(m);
    setFlowPick(pick);
    navigate(pathOf("flow"));
  }, []);
  // Leaving Flow drops the handed-over pick, so the next visit opens plain.
  useEffect(() => {
    if (tab !== "flow") setFlowPick(null);
  }, [tab]);
  // A Check-in Finding row opens Find money on that Finding: read once.
  const [findKey, setFindKey] = useState<string | null>(null);
  const openFinding = useCallback((key: string) => {
    setFindKey(key);
    navigate(pathOf("find"));
  }, []);
  const findFocused = useCallback(() => setFindKey(null), []);
  // Bumped when a sync on open brings new rows, so Flow reloads its figures.
  const [dataVersion, setDataVersion] = useState(0);
  const [toast, setToast] = useState<ToastMsg | null>(null);
  const toastTimer = useRef<number | undefined>(undefined);
  // While a toast is up, the page gets room below its content for it, so
  // whatever the toast covers at the bottom can be scrolled clear (an error
  // or Undo toast stays until dismissed).
  const toastRegions = useRef<HTMLDivElement>(null);
  const [toastRoom, setToastRoom] = useState(0);
  useEffect(() => {
    const el = toastRegions.current;
    if (!el) return;
    const measure = () => {
      const h = Math.max(
        0,
        ...Array.from(el.children, (r) => r.getBoundingClientRect().height),
      );
      // The toast floats this far above the viewport bottom (ledger.css).
      setToastRoom(h > 0 ? h + 22 : 0);
    };
    const ro = new ResizeObserver(measure);
    Array.from(el.children).forEach((r) => ro.observe(r));
    return () => ro.disconnect();
  }, []);

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
    <div
      className="lg-app"
      style={toastRoom ? ({ "--toast-room": `${toastRoom}px` } as CSSProperties) : undefined}
    >
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
      {tab === "checkin" ? (
        <CheckIn
          key={dataVersion}
          month={month}
          onMonth={setMonth}
          nav={checkin}
          onNav={setCheckin}
          onDrill={drill}
          onFinding={openFinding}
        />
      ) : tab === "find" ? (
        <FindMoney
          key={dataVersion}
          month={month}
          onMonth={setMonth}
          accounts={accounts}
          notify={notify}
          focusKey={findKey}
          onFocused={findFocused}
        />
      ) : tab === "settings" ? (
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
          initialPick={flowPick}
        />
      )}
      {/* Always in the page so screen readers announce what appears in them:
          confirmations politely, errors at once. */}
      <div ref={toastRegions} className="toast-regions">
        <div className="toast-region" role="status">
          {toast?.kind === "ok" && <Toast msg={toast} onDismiss={() => setToast(null)} />}
        </div>
        <div className="toast-region" role="alert">
          {toast?.kind === "err" && <Toast msg={toast} onDismiss={() => setToast(null)} />}
        </div>
      </div>
    </div>
  );
}
