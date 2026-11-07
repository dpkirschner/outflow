export interface ToastMsg {
  kind: "ok" | "err";
  text: string;
  /** Optional inline action (e.g. Undo). Runs on click; the toast then clears.
   *  A toast with an action stays until it's used or dismissed (no time
   *  limit on reaching Undo). */
  action?: { label: string; run: () => void };
}

export function Toast({ msg, onDismiss }: { msg: ToastMsg; onDismiss?: () => void }) {
  return (
    <div className={`toast ${msg.kind === "err" ? "err" : ""}`}>
      <span>{msg.text}</span>
      {msg.action && (
        <>
          <button
            className="toast-action"
            onClick={() => {
              msg.action?.run();
              onDismiss?.();
            }}
          >
            {msg.action.label}
          </button>
          <button className="toast-close" onClick={() => onDismiss?.()}>
            Dismiss
          </button>
        </>
      )}
    </div>
  );
}
