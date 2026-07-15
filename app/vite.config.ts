import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Dev: vite serves the SPA and proxies /api to a locally running
// outflow-server (cargo run -p outflow-server). Production: the server serves
// the built dist/ itself, so everything is same-origin.
export default defineConfig({
  plugins: [react()],
  server: {
    port: 1420,
    proxy: {
      "/api": "http://127.0.0.1:8080",
    },
  },
});
