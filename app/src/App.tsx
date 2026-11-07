import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "./api";
import type { Account } from "./types";
import { Toast, type ToastMsg } from "./components/Toast";
import { Balances } from "./components/Balances";
import { Flow } from "./components/Flow";
import { Settings } from "./components/Settings";
import { SCREENS, onLinkClick, useScreen } from "./route";

export default function App() {
  // The screen comes from the URL (route.ts); /oauth-return maps to Settings
  // so Plaid Link can resume in Connections.
  const tab = useScreen();
  useEffect(() => {
    const label = SCREENS.find((s) => s.screen === tab)?.label;
    document.title = label ? `outflow · ${label}` : "outflow";
  }, [tab]);
  // Upkeep that needs a look: proposed card-payment matches + connections
  // not "ok". Shown as a badge on the Settings tab.
  const [attention, setAttention] = useState(0);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [vocab, setVocab] = useState<string[]>([]);
  // The selected calendar month (`YYYY-MM`), shared across screens so Flow
  // and, later, Holdings read the same one. null = the latest with data.
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
    // A toast with an action (Undo) waits to be used or dismissed: no time
    // limit on reaching it from the keyboard (WCAG 2.2.1).
    if (!msg.action) toastTimer.current = window.setTimeout(() => setToast(null), 3200);
  }, []);

  const reload = useCallback(async () => {
    try {
      const [accts, cats, pending, items] = await Promise.all([
        api.accounts(),
        api.categories(),
        api.matches("proposed").catch(() => []),
        api.plaidItems().catch(() => []),
      ]);
      setAccounts(accts);
      setVocab(cats);
      setAttention(pending.length + items.filter((it) => it.status !== "ok").length);
    } catch (err) {
      notify({ kind: "err", text: `Load failed: ${String(err)}` });
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
          notify({ kind: "ok", text: `Refreshed: ${r.added} added, ${r.updated} updated` });
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
      <div className="lg-top">
        <span className="lg-wm">outflow</span>
        <span className="lg-sub">where did my money go?</span>
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
                title={badge ? `${attention} thing(s) need a look` : undefined}
              >
                {label}
                {badge && <span className="st-badge">{attention}</span>}
              </a>
            );
          })}
        </nav>
      </div>

      {tab === "settings" ? (
        <Settings notify={notify} onChanged={() => void reload()} />
      ) : tab === "balances" ? (
        <Balances notify={notify} />
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
      {/* Always in the page so screen readers announce what appears in it. */}
      <div className="toast-region" role="status" aria-live="polite">
        {toast && <Toast msg={toast} onDismiss={() => setToast(null)} />}
      </div>
    </div>
  );
}
