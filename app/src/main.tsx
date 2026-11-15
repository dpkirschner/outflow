import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { AuthGate } from "./components/AuthGate";
import { Embed } from "./components/Embed";
import "./ledger.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {/* /embed/* is a chrome-less view for another app to frame. */}
    <AuthGate>{window.location.pathname.startsWith("/embed/") ? <Embed /> : <App />}</AuthGate>
  </React.StrictMode>
);
