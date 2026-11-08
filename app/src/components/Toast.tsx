export interface ToastMsg {
  kind: "ok" | "err";
  text: string;
  /** Optional inline action (e.g. Undo). Runs on click; the toast then clears.
   *  A toast with an action stays until it's used or dismissed (no time
   *  limit on reaching Undo). */
  action?: { label: string; run: () => void };
}

/** A toast stays up until dismissed when it has an action or is an error;
 *  plain confirmations clear on their own. */
export function toastPersists(msg: ToastMsg): boolean {
  return msg.kind === "err" || !!msg.action;
}

export function Toast({ msg, onDismiss }: { msg: ToastMsg; onDismiss?: () => void }) {
  return (
    <div className={`toast ${msg.kind === "err" ? "err" : ""}`}>
      <span className="toast-text">{msg.text}</span>
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
