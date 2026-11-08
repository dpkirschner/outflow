// Bank connections screen: linked Plaid items + the Link flow. Linking is
// browser-driven — the server mints a link_token, Plaid Link runs here, and
// the resulting public_token goes back for exchange. OAuth banks (Chase/Amex/
// Capital One) redirect away and come back to /oauth-return, so the link_token
// is stashed in localStorage and Link is re-initialized with the full return
// URL (receivedRedirectUri) to resume the session.
import { useCallback, useEffect, useRef, useState } from "react";
import { usePlaidLink } from "react-plaid-link";
import { api, type LinkErrorReport } from "../api";
import type { Account, PlaidItem, SyncEntry } from "../types";
import { SourceChip, accountDisplayLabel, accountSource, agoLabel } from "./accounts";
import type { ToastMsg } from "./Toast";
import { isOauthReturn, navigate, pathOf } from "../route";
import { errMessage } from "../format";
import { Section } from "./Section";

const LINK_TOKEN_KEY = "outflow_plaid_link_token";
const LINK_ITEM_KEY = "outflow_plaid_link_item";

// Best-effort: a failed report must never mask the Link error itself.
async function reportLinkError(r: LinkErrorReport) {
  try {
    await api.plaidLinkEvent(r);
  } catch (e) {
    console.warn("could not forward Plaid Link error to the server", e);
  }
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
  onDone: (
    publicToken: string | null,
    institution: string | null,
    linkError?: LinkErrorReport,
  ) => void;
}) {
  const { open, ready } = usePlaidLink({
    token,
    receivedRedirectUri,
    onSuccess: (publicToken, metadata) =>
      onDone(publicToken, metadata.institution?.name ?? null),
    // Errors inside Link never reach the server on their own, so forward each
    // one to the server log. ERROR events catch failures the user recovers
    // from (e.g. picks another bank); onExit's error is the one that ended it.
    onEvent: (eventName, md) => {
      if (eventName !== "ERROR") return;
      void reportLinkError({
        stage: "event",
        error_type: md.error_type,
        error_code: md.error_code,
        error_message: md.error_message,
        institution: md.institution_name,
        institution_id: md.institution_id,
        link_session_id: md.link_session_id,
        request_id: md.request_id,
      });
    },
    onExit: (err, md) => {
      if (!err) return onDone(null, null); // user closed Link
      const report: LinkErrorReport = {
        stage: "exit",
        error_type: err.error_type,
        error_code: err.error_code,
        error_message: err.error_message,
        display_message: err.display_message,
        institution: md.institution?.name,
        institution_id: md.institution?.institution_id,
        link_session_id: md.link_session_id,
        request_id: md.request_id,
      };
      void reportLinkError(report);
      onDone(null, null, report);
    },
  });
  useEffect(() => {
    if (ready) open();
  }, [ready, open]);
  return null;
}

export function Connections({
  notify,
  onChanged,
  attention,
}: {
  notify: (msg: ToastMsg) => void;
  /// Re-fetch app-wide state after a nickname or hide change so the other
  /// tabs (which read the shared, visible-only accounts list) update too.
  onChanged: () => void;
  /// Connections that need reconnecting (core's upkeep): the section starts
  /// open when there are any.
  attention: number;
}) {
  const [loaded, setLoaded] = useState(false);
  const [confirmUnlink, setConfirmUnlink] = useState<string | null>(null);
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
      notify({ kind: "err", text: `Couldn't load connections: ${errMessage(err)}` });
    } finally {
      setLoaded(true);
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
      navigate(pathOf("settings"), { replace: true });
    }
  }, [notify]);

  const startLink = async (
    itemId?: string,
    consentInvestments = false,
    investmentsOnly = false,
  ) => {
    setBusy(true);
    try {
      const r = await api.plaidLinkToken(itemId, consentInvestments, investmentsOnly);
      localStorage.setItem(LINK_TOKEN_KEY, r.link_token);
      if (itemId) localStorage.setItem(LINK_ITEM_KEY, itemId);
      else localStorage.removeItem(LINK_ITEM_KEY);
      setSession({ token: r.link_token });
    } catch (err) {
      notify({ kind: "err", text: errMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  const onLinkDone = async (
    publicToken: string | null,
    institution: string | null,
    linkError?: LinkErrorReport,
  ) => {
    const relinkItem = localStorage.getItem(LINK_ITEM_KEY);
    localStorage.removeItem(LINK_TOKEN_KEY);
    localStorage.removeItem(LINK_ITEM_KEY);
    setSession(null);
    if (isOauthReturn()) navigate(pathOf("settings"), { replace: true });
    if (linkError) {
      const where = linkError.institution ? `${linkError.institution}: ` : "";
      const msg = linkError.display_message || linkError.error_message || "";
      notify({
        kind: "err",
        text: `Plaid Link failed — ${where}${linkError.error_code ?? "unknown error"}${msg ? ` (${msg})` : ""}`,
      });
      return;
    }
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
      notify({ kind: "err", text: errMessage(err) });
    } finally {
      setBusy(false);
    }
  };

  const onRemove = async (item: PlaidItem) => {
    setConfirmUnlink(null);
    setBusy(true);
    try {
      await api.plaidRemoveItem(item.item_id);
      notify({ kind: "ok", text: `${item.institution} unlinked.` });
      await refresh();
    } catch (err) {
      notify({ kind: "err", text: errMessage(err) });
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
          ? `Sync didn't finish for ${errs.map((l) => l.source).join(", ")}: ${errs.map((l) => l.error).join("; ")}`
          : `Synced: ${added} new transaction${added === 1 ? "" : "s"}`,
      });
      await refresh();
    } catch (err) {
      notify({ kind: "err", text: errMessage(err) });
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
      notify({ kind: "err", text: errMessage(err) });
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
      notify({ kind: "err", text: errMessage(err) });
    }
  };

  // Accounts sit under their connection: the one each last synced through
  // (item_id). An account that hasn't synced since that was recorded falls
  // back to its institution name; anything unplaced (a fixture pull, an
  // unlinked connection) is listed after.
  const itemIds = new Set(items.map((it) => it.item_id));
  const byName = new Map(items.map((it) => [it.institution, it.item_id]));
  const placedIn = (a: Account) =>
    a.item_id && itemIds.has(a.item_id) ? a.item_id : !a.item_id ? byName.get(a.org) : undefined;
  const byItem: Record<string, Account[]> = {};
  const unmatched: Account[] = [];
  for (const a of accounts) {
    const id = placedIn(a);
    if (id) (byItem[id] ??= []).push(a);
    else unmatched.push(a);
  }

  if (!loaded) return null;

  const accountRow = (a: Account) => (
    <li className="cx-acct-row" key={a.id}>
      <SourceChip s={accountSource(a)} />
      {a.hidden && <span className="cx-hidden-tag">hidden</span>}
      <span className="cx-acct-acts">
      {editing === a.id ? (
        <input
          className="cx-nick-input"
          type="text"
          autoFocus
          aria-label={`Nickname for ${accountDisplayLabel(a.name, a.nickname)}`}
          defaultValue={a.nickname ?? ""}
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
        <button className="cx-mini" onClick={() => setEditing(a.id)}>
          {a.nickname ? "Rename" : "Add nickname"}
        </button>
      )}
      <button className="cx-mini" onClick={() => void toggleHidden(a)}>
        {a.hidden ? "Show" : "Hide"}
      </button>
      </span>
    </li>
  );

  const okCount = items.filter((it) => it.status === "ok").length;
  const status =
    items.length === 0
      ? "none linked yet"
      : attention > 0
        ? `${attention} of ${items.length} need${attention === 1 ? "s" : ""} reconnecting`
        : `${items.length === 1 ? "1 connection" : `${okCount} connections`}, all fine`;

  return (
    <Section
      title="Connections"
      status={<span className={attention > 0 ? "st-warn" : undefined}>{status}</span>}
      open={attention > 0 || items.length === 0}
      actions={
        <>
          <button className="lg-pill" disabled={busy} onClick={() => void startLink()}>
            Connect a bank
          </button>
          <button className="lg-pill" disabled={busy} onClick={() => void onPull()}>
            Sync now
          </button>
        </>
      }
    >
      {session && (
        <LinkSession
          token={session.token}
          receivedRedirectUri={session.receivedRedirectUri}
          onDone={onLinkDone}
        />
      )}
      <div className="cx-wrap">
        {items.length === 0 ? (
          <p className="st-blurb">
            No banks linked yet. <b>Connect a bank</b> opens Plaid Link. Amex, Capital One and
            Chase work once the server's OAuth redirect is configured.
          </p>
        ) : (
          <>
            <p className="st-blurb">
              Give an account a nickname to spot it at a glance; the last four digits stay. Hide one
              to drop it and its transactions from every view. It keeps syncing, and you can show it
              again here.
            </p>
            <ul className="cx-items">
              {items.map((it) => (
                <li key={it.item_id} className="cx-item">
                  <div className="cx-item-head">
                    <div className="cx-inst">
                      <h3>{it.institution}</h3>
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
                          className="lg-pill key"
                          disabled={busy}
                          onClick={() => void startLink(it.item_id)}
                        >
                          Reconnect
                        </button>
                      )}
                      {it.investments_consent ? (
                        <span className="lg-src investment">holdings allowed</span>
                      ) : (
                        byItem[it.item_id]?.some((a) => a.kind === "Investment") && (
                          // Items linked before outflow requested Investments consent
                          // need this update-mode pass before holdings can be fetched.
                          <button
                            className="lg-src investment cx-holdings-btn"
                            disabled={busy}
                            onClick={() => void startLink(it.item_id, true)}
                          >
                            Allow holdings
                          </button>
                        )
                      )}
                    </div>
                  </div>
                  {(byItem[it.item_id]?.length ?? 0) > 0 && (
                    <ul className="cx-acct-list" aria-label={`${it.institution} accounts`}>
                      {byItem[it.item_id].map(accountRow)}
                    </ul>
                  )}
                  <div className="cx-item-foot">
                    {confirmUnlink === it.item_id ? (
                      <div className="cx-confirm" role="group" aria-label={`Unlink ${it.institution}`}>
                        <span>
                          Unlink {it.institution}? Pulled history stays; new transactions stop
                          syncing.
                        </span>
                        <button
                          className="cx-mini cx-danger"
                          disabled={busy}
                          onClick={() => void onRemove(it)}
                        >
                          Unlink
                        </button>
                        <button className="cx-mini" onClick={() => setConfirmUnlink(null)}>
                          Keep it
                        </button>
                      </div>
                    ) : (
                      <button className="cx-mini" onClick={() => setConfirmUnlink(it.item_id)}>
                        Unlink…
                      </button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          </>
        )}

        {unmatched.length > 0 && (
          <div className="cx-accts">
            <h3>Other accounts</h3>
            <ul className="cx-acct-list">{unmatched.map(accountRow)}</ul>
          </div>
        )}

        <p className="st-blurb">
          A brokerage, retirement or HSA provider that <b>Connect a bank</b> can't find?{" "}
          <button
            className="cx-mini"
            disabled={busy}
            onClick={() => void startLink(undefined, false, true)}
          >
            Connect investments only
          </button>
        </p>

        {log.length > 0 && (
          <div className="cx-log">
            <h3>Recent syncs</h3>
            <ul>
              {log.map((s) => (
                <li key={s.id}>
                  <span className="cx-log-src">{s.source}</span>
                  <span>{agoLabel(s.started)}</span>
                  <span className={s.note && s.finished !== null ? "cx-log-fail" : undefined}>
                    {s.finished === null
                      ? "running…"
                      : s.note
                        ? `didn't finish (${s.note})`
                        : `${s.added ?? 0} new, ${s.updated ?? 0} updated`}
                  </span>
                </li>
              ))}
            </ul>
          </div>
        )}
      </div>
    </Section>
  );
}
