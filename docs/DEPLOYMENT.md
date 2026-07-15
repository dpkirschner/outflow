# Deployment — mac-mini + Tailscale + Plaid

The production posture: `outflow-server` runs as a launchd agent on an
always-on mac-mini, bound to loopback, with `tailscale serve` terminating
HTTPS on the tailnet. Every device on the tailnet (laptop, phone) gets the web
UI at `https://<mini>.<tailnet>.ts.net`; agents/scripts use the CLI in
`--server` mode against the same URL.

## 1. Plaid account (do this first — approval is the long pole)

1. Sign up at <https://dashboard.plaid.com>. New US teams get the free
   **Trial plan**: real production data, **10 Items lifetime** (removals do
   NOT free slots), Transactions product included.
2. Grab the **sandbox** client_id + secret immediately — everything works
   against sandbox while you wait.
3. Request **Production** access (Dashboard → settings). Chase/Amex/Capital
   One are OAuth institutions; approval can take days. Fill in the personal-use
   profile honestly.
4. Products: enable **Transactions** only (Investments later for brokerage —
   it affects billing).
5. Dashboard → Developers → API → **Allowed redirect URIs**: add exactly
   `https://<mini>.<tailnet>.ts.net/oauth-return`.
6. Webhooks: skip (the server polls; it isn't internet-reachable).

## 2. mac-mini prerequisites

```
brew install rustup node tailscale
```

- Tailscale: log in, enable **MagicDNS** and **HTTPS certificates** in the
  admin console.
- Clone the repo, then build:

```
cd app && npm install && npm run build && cd .. && cargo build --release -p outflow-server -p outflow-cli
```

(Add `--features encryption` to the server build for SQLCipher; then also
provision `OUTFLOW_DB_KEY_FILE` below and never lose that file.)

## 3. Secrets & data dir (all 0600, no keychain)

```
mkdir -p ~/Library/"Application Support"/outflow && chmod 700 ~/Library/"Application Support"/outflow
```

```
printf '%s\n' 'YOUR_PLAID_SECRET' > ~/Library/"Application Support"/outflow/plaid-secret && chmod 600 ~/Library/"Application Support"/outflow/plaid-secret
```

The Plaid **access tokens** file (`plaid-tokens.json`) is created by the
server on first link; it lives in the same dir. If migrating an existing
laptop DB, copy it to
`~/Library/Application Support/outflow/outflow.db` (plaintext↔encrypted modes
are not interchangeable — see GOTCHAS).

## 4. Tailscale serve (HTTPS on the tailnet)

```
tailscale serve --bg https / http://127.0.0.1:8080
```

Verify: `https://<mini>.<tailnet>.ts.net` from another tailnet device. This
URL (plus `/oauth-return`) is what goes in the Plaid dashboard redirect list
and in `OUTFLOW_OAUTH_REDIRECT`.

## 5. launchd agent

`~/Library/LaunchAgents/com.outflow.server.plist` (adjust paths/tailnet):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.outflow.server</string>
  <key>ProgramArguments</key><array>
    <string>/Users/YOU/code/outflow/target/release/outflow-server</string>
  </array>
  <key>WorkingDirectory</key><string>/Users/YOU/code/outflow</string>
  <key>EnvironmentVariables</key><dict>
    <key>OUTFLOW_LISTEN</key><string>127.0.0.1:8080</string>
    <key>OUTFLOW_WEB_DIR</key><string>/Users/YOU/code/outflow/app/dist</string>
    <key>OUTFLOW_PLAID_CLIENT_ID</key><string>YOUR_CLIENT_ID</string>
    <key>OUTFLOW_PLAID_SECRET_FILE</key><string>/Users/YOU/Library/Application Support/outflow/plaid-secret</string>
    <key>OUTFLOW_PLAID_ENV</key><string>production</string>
    <key>OUTFLOW_OAUTH_REDIRECT</key><string>https://MINI.TAILNET.ts.net/oauth-return</string>
    <key>OUTFLOW_SYNC_INTERVAL_SECS</key><string>21600</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/tmp/outflow-server.log</string>
  <key>StandardErrorPath</key><string>/tmp/outflow-server.log</string>
</dict></plist>
```

```
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.outflow.server.plist
```

Restart after a rebuild:

```
launchctl kickstart -k gui/$(id -u)/com.outflow.server
```

Optional hardening: add `OUTFLOW_API_TOKEN` to the env block and export it
wherever the CLI runs.

## 6. Link the banks

Open `https://<mini>.<tailnet>.ts.net` → **Connections** → **Connect a bank**.
Chase/Amex/CapOne bounce through the bank's OAuth page and return to
`/oauth-return`; the first sync runs immediately (Plaid backfills up to ~24
months). The background sync then runs every `OUTFLOW_SYNC_INTERVAL_SECS`;
**Sync now** and `POST /api/pull` force one. Item status `reconnect needed`
(expired OAuth consent) → **Reconnect** on the same screen.

## 7. Agent / CLI access over the tailnet

```
OUTFLOW_SERVER=https://MINI.TAILNET.ts.net outflow txns --search starbucks --since 2026-06-01 --json
```

(Build the CLI with `--features client`. `report`, `subs`, `accounts`,
`matches`, `status`, `pull`, `fix` all work in server mode and print the same
JSON shapes as local `--json`.)

## 8. Backups

The DB is the permanent archive — back up
`~/Library/Application Support/outflow/` (DB + plaid-tokens.json + key file if
encrypted). Time Machine covers it if the mini has a backup target.
