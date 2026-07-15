// Bank connections screen: linked Plaid items + the Link flow. Linking is
// browser-driven — the server mints a link_token, Plaid Link runs here, and
// the resulting public_token goes back for exchange. OAuth banks (Chase/Amex/
// Capital One) redirect away and come back to /oauth-return, so the link_token
// is stashed in localStorage and Link is re-initialized with the full return
// URL (receivedRedirectUri) to resume the session.
import { useCallback, useEffect, useState } from "react";
import { usePlaidLink } from "react-plaid-link";
import { api } from "../api";
import type { PlaidItem, SyncEntry } from "../types";
import { agoLabel } from "./ledger/labels";
import type { ToastMsg } from "./Toast";

const LINK_TOKEN_KEY = "outflow_plaid_link_token";
const LINK_ITEM_KEY = "outflow_plaid_link_item";

function isOauthReturn(): boolean {
  return window.location.pathname === "/oauth-return";
}

// Mounted only while a link session is active; opens Link as soon as the SDK
// is ready and unmounts on completion/exit.
function LinkSession({
  token,
  receivedRedirectUri,
  onDone,
}: {
  token: string;
  receivedRedirectUri?: string;
  onDone: (publicToken: string | null, institution: string | null) => void;
}) {
  const { open, ready } = usePlaidLink({
    token,
    receivedRedirectUri,
    onSuccess: (publicToken, metadata) =>
      onDone(publicToken, metadata.institution?.name ?? null),
    onExit: () => onDone(null, null),
  });
  useEffect(() => {
    if (ready) open();
  }, [ready, open]);
  return null;
}

export function Connections({ notify }: { notify: (msg: ToastMsg) => void }) {
  const [items, setItems] = useState<PlaidItem[]>([]);
  const [log, setLog] = useState<SyncEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [session, setSession] = useState<{
    token: string;
    receivedRedirectUri?: string;
  } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [it, lg] = await Promise.all([api.plaidItems(), api.syncLog(15)]);
      setItems(it);
      setLog(lg);
    } catch (err) {
      notify({ kind: "err", text: `Load failed: ${String(err)}` });
    }
  }, [notify]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // OAuth resumption: back from the bank with the stashed link_token.
  useEffect(() => {
    if (!isOauthReturn()) return;
    const token = localStorage.getItem(LINK_TOKEN_KEY);
    if (token) {
      setSession({ token, receivedRedirectUri: window.location.href });
    } else {
      notify({ kind: "err", text: "OAuth return without a saved Link session — start over." });
      window.history.replaceState(null, "", "/");
    }
  }, [notify]);

  const startLink = async (itemId?: string) => {
    setBusy(true);
    try {
      const r = await api.plaidLinkToken(itemId);
      localStorage.setItem(LINK_TOKEN_KEY, r.link_token);
      if (itemId) localStorage.setItem(LINK_ITEM_KEY, itemId);
      else localStorage.removeItem(LINK_ITEM_KEY);
      setSession({ token: r.link_token });
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    } finally {
      setBusy(false);
    }
  };

  const onLinkDone = async (publicToken: string | null, institution: string | null) => {
    const relinkItem = localStorage.getItem(LINK_ITEM_KEY);
    localStorage.removeItem(LINK_TOKEN_KEY);
    localStorage.removeItem(LINK_ITEM_KEY);
    setSession(null);
    if (isOauthReturn()) window.history.replaceState(null, "", "/");
    if (!publicToken) return; // user closed Link
    setBusy(true);
    try {
      if (relinkItem) {
        // Update mode: same item, credentials refreshed — no exchange needed.
        notify({ kind: "ok", text: "Reconnected — pulling latest…" });
        await api.pull();
      } else {
        const r = await api.plaidExchange(publicToken, institution);
        notify({
          kind: "ok",
          text: `Linked ${r.institution}: ${r.added} transactions pulled`,
        });
      }
      await refresh();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    } finally {
      setBusy(false);
    }
  };

  const onRemove = async (item: PlaidItem) => {
    if (
      !window.confirm(
        `Unlink ${item.institution}? Pulled history is kept; new transactions stop syncing.`,
      )
    )
      return;
    setBusy(true);
    try {
      await api.plaidRemoveItem(item.item_id);
      notify({ kind: "ok", text: `${item.institution} unlinked.` });
      await refresh();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    } finally {
      setBusy(false);
    }
  };

  const onPull = async () => {
    setBusy(true);
    try {
      const r = await api.pull();
      const added = r.legs.reduce((n, l) => n + l.added, 0);
      const errs = r.legs.filter((l) => l.error);
      notify({
        kind: errs.length ? "err" : "ok",
        text: errs.length
          ? `Synced with errors: ${errs.map((l) => `${l.source}: ${l.error}`).join("; ")}`
          : `Synced ${r.legs.length} source(s): ${added} added`,
      });
      await refresh();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cx-wrap">
      {session && (
        <LinkSession
          token={session.token}
          receivedRedirectUri={session.receivedRedirectUri}
          onDone={onLinkDone}
        />
      )}

      <div className="cx-head">
        <h2>Connections</h2>
        <div className="cx-actions">
          <button className="lg-pill" disabled={busy} onClick={() => void startLink()}>
            Connect a bank
          </button>
          <button className="lg-pill" disabled={busy} onClick={() => void onPull()}>
            Sync now
          </button>
        </div>
      </div>

      {items.length === 0 ? (
        <div className="lg-empty">
          No banks linked yet. <b>Connect a bank</b> opens Plaid Link — Amex, Capital One and
          Chase all work once the server's OAuth redirect is configured.
        </div>
      ) : (
        <ul className="cx-items">
          {items.map((it) => (
            <li key={it.item_id} className="cx-item">
              <div className="cx-inst">
                <b>{it.institution}</b>
                <span className={`cx-status cx-${it.status}`}>
                  {it.status === "ok"
                    ? "connected"
                    : it.status === "login_required"
                      ? "reconnect needed"
                      : "error"}
                </span>
              </div>
              <div className="cx-meta">
                {it.last_synced ? `last sync ${agoLabel(it.last_synced)}` : "never synced"}
              </div>
              <div className="cx-actions">
                {it.status !== "ok" && (
                  <button
                    className="lg-pill"
                    disabled={busy}
                    onClick={() => void startLink(it.item_id)}
                  >
                    Reconnect
                  </button>
                )}
                <button className="lg-pill ghost" disabled={busy} onClick={() => void onRemove(it)}>
                  Unlink
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}

      {log.length > 0 && (
        <div className="cx-log">
          <h3>Recent syncs</h3>
          <ul>
            {log.map((s) => (
              <li key={s.id}>
                <span className="cx-log-src">{s.source}</span>
                <span>{agoLabel(s.started)}</span>
                <span>
                  {s.finished === null
                    ? "running…"
                    : s.note
                      ? `failed: ${s.note}`
                      : `${s.added ?? 0} added, ${s.updated ?? 0} updated`}
                </span>
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
