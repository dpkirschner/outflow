import { Fragment, useEffect, useState, type FormEvent, type ReactNode } from "react";
import { setUnauthorizedHandler, token } from "../api";

/**
 * Gates the app behind the server's API token, but only when the server asks
 * for one. There is no "is auth enabled" endpoint to consult: the children
 * render and call the API optimistically, and only a 401 raises the gate. A
 * server with OUTFLOW_API_TOKEN unset never shows this at all, which keeps the
 * dev posture (`npm run dev` against a bare server) zero-config.
 */
export function AuthGate({ children }: { children: ReactNode }) {
  const [locked, setLocked] = useState(false);
  // Bumped on unlock to remount the tree: the children already failed their
  // initial fetches, so they need a fresh mount to refetch with the new token
  // rather than sitting on whatever error they landed in.
  const [attempt, setAttempt] = useState(0);
  const [value, setValue] = useState("");

  useEffect(() => {
    setUnauthorizedHandler(() => setLocked(true));
  }, []);

  if (!locked) return <Fragment key={attempt}>{children}</Fragment>;

  const unlock = (e: FormEvent) => {
    e.preventDefault();
    const t = value.trim();
    if (!t) return;
    token.set(t);
    setValue("");
    setLocked(false);
    setAttempt((a) => a + 1);
  };

  return (
    <div className="auth-gate">
      <form className="card auth-card" onSubmit={unlock}>
        <h2>outflow</h2>
        <p>This server requires an API token.</p>
        <input
          type="password"
          name="outflow-api-token"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          placeholder="API token"
          autoComplete="current-password"
          autoFocus
        />
        <button className="btn primary" type="submit" disabled={!value.trim()}>
          Unlock
        </button>
        <p className="auth-hint">
          The <code>OUTFLOW_API_TOKEN</code> from the server's launchd plist. Stored in this
          browser only — a wrong token just asks again.
        </p>
      </form>
    </div>
  );
}
