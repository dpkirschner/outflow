// Bank connections screen: linked Plaid items + the Link flow. Linking is
// browser-driven — the server mints a link_token, Plaid Link runs here, and
// the resulting public_token goes back for exchange. OAuth banks (Chase/Amex/
// Capital One) redirect away and come back to /oauth-return, so the link_token
// is stashed in localStorage and Link is re-initialized with the full return
// URL (receivedRedirectUri) to resume the session.
import { useCallback, useEffect, useRef, useState } from "react";
import { usePlaidLink } from "react-plaid-link";
import { api } from "../api";
import type { Account, PlaidItem, SyncEntry } from "../types";
import { accountDisplayLabel, accountSource, agoLabel } from "./ledger/labels";
import { SourceChip } from "./ledger/Streams";
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

export function Connections({
  notify,
  onChanged,
}: {
  notify: (msg: ToastMsg) => void;
  /// Re-fetch app-wide state after a nickname or hide change so the other
  /// tabs (which read the shared, visible-only accounts list) update too.
  onChanged: () => void;
}) {
  const [items, setItems] = useState<PlaidItem[]>([]);
  // Every account, hidden included — this is the one screen that can un-hide.
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [log, setLog] = useState<SyncEntry[]>([]);
  const [busy, setBusy] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  // Escape must cancel an edit without the input's onBlur committing it.
  const cancelEdit = useRef(false);
  const [session, setSession] = useState<{
    token: string;
    receivedRedirectUri?: string;
  } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [it, lg, accts] = await Promise.all([
        api.plaidItems(),
        api.syncLog(15),
        api.accounts(true),
      ]);
      setItems(it);
      setLog(lg);
      setAccounts(accts);
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

  const startLink = async (itemId?: string, consentInvestments = false) => {
    setBusy(true);
    try {
      const r = await api.plaidLinkToken(itemId, consentInvestments);
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
        notify(
          r.sync_error
            ? {
                kind: "err",
                text: `Linked ${r.institution}, but the first sync failed (it will retry): ${r.sync_error}`,
              }
            : {
                kind: "ok",
                text: `Linked ${r.institution}: ${r.added} transactions pulled`,
              },
        );
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

  // Commit a nickname edit (single path: Enter/blur both route here). Empty
  // clears it; an unchanged value skips the round-trip.
  const saveNickname = async (a: Account, raw: string) => {
    setEditing(null);
    const next = raw.trim();
    if (next === (a.nickname ?? "")) return;
    try {
      await api.setAccountNickname(a.id, next);
      notify({ kind: "ok", text: next ? `Nicknamed “${next}”` : "Nickname cleared" });
      await refresh();
      onChanged();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    }
  };

  const toggleHidden = async (a: Account) => {
    try {
      await api.setAccountHidden(a.id, !a.hidden);
      const label = accountDisplayLabel(a.name, a.nickname);
      notify({
        kind: "ok",
        text: a.hidden ? `Showing ${label} again` : `Hid ${label} from every view`,
      });
      await refresh();
      onChanged();
    } catch (err) {
      notify({ kind: "err", text: String(err) });
    }
  };

  // Accounts grouped by institution; org === PlaidItem.institution on the live
  // path, so headers line up with the items above (fixture accounts group too).
  const byOrg = accounts.reduce<Record<string, Account[]>>((m, a) => {
    (m[a.org] ??= []).push(a);
    return m;
  }, {});

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
                {it.investments_consent ? (
                  <span className="lg-src investment" title="Holdings access granted">
                    ✓ HOLDINGS
                  </span>
                ) : byOrg[it.institution]?.some((a) => a.kind === "Investment") && (
                  // Items linked before outflow requested Investments consent
                  // need this update-mode pass before holdings can be fetched.
                  <button
                    className="lg-src investment cx-holdings-btn"
                    disabled={busy}
                    title="Re-open Plaid Link to grant access to holdings"
                    onClick={() => void startLink(it.item_id, true)}
                  >
                    + ALLOW HOLDINGS
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

      {accounts.length > 0 && (
        <div className="cx-accts">
          <h3>Accounts</h3>
          <p className="cx-accts-hint">
            Give a card or account a nickname to spot it at a glance — the last-4
            is kept automatically. Hide one to drop it and its transactions from
            every view; it keeps syncing, and you can show it again here.
          </p>
          {Object.entries(byOrg).map(([org, accts]) => (
            <div className="cx-acct-group" key={org}>
              <div className="cx-acct-org">{org}</div>
              <ul className="cx-acct-list">
                {accts.map((a) => (
                  <li className={`cx-acct-row${a.hidden ? " cx-acct-hidden" : ""}`} key={a.id}>
                    <SourceChip s={accountSource(a)} />
                    {a.hidden && <span className="cx-hidden-tag">hidden</span>}
                    {editing === a.id ? (
                      <input
                        className="cx-nick-input"
                        type="text"
                        autoFocus
                        defaultValue={a.nickname ?? ""}
                        placeholder="nickname"
                        maxLength={40}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            e.currentTarget.blur();
                          } else if (e.key === "Escape") {
                            cancelEdit.current = true;
                            e.currentTarget.blur();
                          }
                        }}
                        onBlur={(e) => {
                          if (cancelEdit.current) {
                            cancelEdit.current = false;
                            setEditing(null);
                            return;
                          }
                          void saveNickname(a, e.target.value);
                        }}
                      />
                    ) : (
                      <button className="cx-nick-edit" onClick={() => setEditing(a.id)}>
                        {a.nickname ? "Rename" : "Add nickname"}
                      </button>
                    )}
                    <button className="cx-hide" onClick={() => void toggleHidden(a)}>
                      {a.hidden ? "Show" : "Hide"}
                    </button>
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </div>
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
