export interface ToastMsg {
  kind: "ok" | "err";
  text: string;
  /** Optional inline action (e.g. Undo). Runs on click; the toast then clears.
   *  A toast with an action stays until it's used or dismissed (no time
   *  limit on reaching Undo). */
  action?: { label: string; run: () => void };
  /** Optional second action shown before `action`: a follow-on the reader
   *  may take (e.g. apply to past rows). Clears the toast like `action`. */
  offer?: { label: string; run: () => void };
}

/** A toast stays up until dismissed when it has an action or is an error;
 *  plain confirmations clear on their own. */
export function toastPersists(msg: ToastMsg): boolean {
  return msg.kind === "err" || !!msg.action || !!msg.offer;
}

export function Toast({ msg, onDismiss }: { msg: ToastMsg; onDismiss?: () => void }) {
  return (
    <div className={`toast ${msg.kind === "err" ? "err" : ""}`}>
      <span className="toast-text">{msg.text}</span>
      {msg.offer && (
        <button
          className="toast-action"
          onClick={() => {
            msg.offer?.run();
            onDismiss?.();
          }}
        >
          {msg.offer.label}
        </button>
      )}
      {msg.action && (
        <button
          className="toast-action"
          onClick={() => {
            msg.action?.run();
            onDismiss?.();
          }}
        >
          {msg.action.label}
        </button>
      )}
      {toastPersists(msg) && (
        <button className="toast-close" onClick={() => onDismiss?.()}>
          Dismiss
        </button>
      )}
    </div>
  );
}
