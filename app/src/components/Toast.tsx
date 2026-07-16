export interface ToastMsg {
  kind: "ok" | "err";
  text: string;
  /** Optional inline action (e.g. Undo). Runs on click; the toast then clears. */
  action?: { label: string; run: () => void };
}

export function Toast({ msg, onDismiss }: { msg: ToastMsg; onDismiss?: () => void }) {
  return (
    <div className={`toast ${msg.kind === "err" ? "err" : ""}`}>
      <span>{msg.text}</span>
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
    </div>
  );
}
