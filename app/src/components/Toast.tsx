export interface ToastMsg {
  kind: "ok" | "err";
  text: string;
}

export function Toast({ msg }: { msg: ToastMsg }) {
  return <div className={`toast ${msg.kind === "err" ? "err" : ""}`}>{msg.text}</div>;
}
