# Deployment — mac-mini + Tailscale + Plaid

The production posture: `outflow-server` runs as a launchd **daemon** on an
always-on mac-mini — as your own account, from a clone only that account can
write — bound to loopback, with `tailscale serve` terminating HTTPS on the
tailnet. Every device on the tailnet (laptop, phone) gets the web UI at
`https://<mini>.<tailnet>.ts.net` and prompts once for the API token;
agents/scripts use the CLI in `--server` mode against the same URL with the
read-only token.

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

- `brew install rustup` installs the *installer*, not a compiler — `cargo` does
  not exist until you run `rustup default stable`, and **toolchains are
  per-user**, so run it as the account that will build (§5).
- Tailscale: log in, enable **MagicDNS** and **HTTPS certificates** in the
  admin console.
- The clone and the build belong to the account that will run the daemon.
  Where they live is not a detail — see §5.

(Add `--features encryption` to the server build for SQLCipher; then also
provision `OUTFLOW_DB_KEY_FILE` below and never lose that file.)

## 3. Secrets & data dir (all 0600, no keychain)

Run this **as the account that will run the daemon** (§5): `~` below is that
account's home, and these paths go verbatim into the plist. If the box has more
than one account, running this as the wrong one puts your bank credentials in
the wrong home — the 0600 modes only protect you from *other* users.

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

`--bg` config is stored by `tailscaled` (itself a system daemon), not by your
login session, so it survives reboots with nobody logged in — which is what
lets the whole posture work headless.

## 5. launchd daemon

Run the server as a **LaunchDaemon**, not a LaunchAgent. An agent only runs
inside a loaded GUI login session: it starts as whoever is auto-logged-in and
never starts at all if that account isn't. A daemon starts at boot, with no
login, as an account you name via `UserName`.

### Two rules that decide whether any of this works

Both matter the moment anything else shares the box — a second account, a
container, an automation/agent user with a shell:

1. **Run as the human's account.** `UserName` is the account whose 0600 secrets
   the server reads. Not root, not a shared service account.
2. **Nothing else may write what the daemon executes or serves.** Whoever can
   write `target/release/outflow-server` can run arbitrary code *as the daemon's
   user* by swapping the binary; whoever can write `app/dist` can inject JS into
   the SPA and lift the API token out of your browser's localStorage. Either one
   makes `UserName` decoration.

So the account that runs the daemon needs **its own clone**, built in its own
home, writable by nobody else. A checkout sitting in some other account's home
is a dev tree, not a deploy artifact — ship to production through git (push,
then pull as the daemon's user), never by pointing the plist at someone else's
working copy.

```
sudo -u YOU -i          # as the account that will run it
git clone https://github.com/dpkirschner/outflow.git ~/code/outflow
cd ~/code/outflow && rustup default stable   # toolchains are per-user
cd app && npm install && npm run build && cd ..
cargo build --release -p outflow-server -p outflow-cli
```

### The plist

`/Library/LaunchDaemons/com.outflow.server.plist` (adjust paths/tailnet):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.outflow.server</string>
  <key>UserName</key><string>YOU</string>
  <key>GroupName</key><string>staff</string>
  <key>ProgramArguments</key><array>
    <string>/Users/YOU/code/outflow/target/release/outflow-server</string>
  </array>
  <key>WorkingDirectory</key><string>/Users/YOU/code/outflow</string>
  <key>EnvironmentVariables</key><dict>
    <!-- Set the data dir explicitly. A daemon gets no HOME, and the server
         falls back to "." — the DB and plaid-tokens.json would land in
         WorkingDirectory instead, silently. -->
    <key>OUTFLOW_DATA_DIR</key><string>/Users/YOU/Library/Application Support/outflow</string>
    <key>OUTFLOW_LISTEN</key><string>127.0.0.1:8080</string>
    <key>OUTFLOW_WEB_DIR</key><string>/Users/YOU/code/outflow/app/dist</string>
    <key>OUTFLOW_PLAID_CLIENT_ID</key><string>YOUR_CLIENT_ID</string>
    <key>OUTFLOW_PLAID_SECRET_FILE</key><string>/Users/YOU/Library/Application Support/outflow/plaid-secret</string>
    <key>OUTFLOW_PLAID_ENV</key><string>production</string>
    <key>OUTFLOW_OAUTH_REDIRECT</key><string>https://MINI.TAILNET.ts.net/oauth-return</string>
    <key>OUTFLOW_SYNC_INTERVAL_SECS</key><string>21600</string>
    <!-- Tokens by FILE, never inline: this plist is world-readable. -->
    <key>OUTFLOW_API_TOKEN_FILE</key><string>/Users/YOU/Library/Application Support/outflow/api-token</string>
    <key>OUTFLOW_API_TOKEN_RO_FILE</key><string>/Users/YOU/Library/Application Support/outflow/api-token-ro</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/Users/YOU/Library/Logs/outflow-server.log</string>
  <key>StandardErrorPath</key><string>/Users/YOU/Library/Logs/outflow-server.log</string>
</dict></plist>
```

Install it root-owned (launchd refuses a daemon plist writable by anyone else)
and load it into the **system** domain, not `gui/`:

```
sudo cp com.outflow.server.plist /Library/LaunchDaemons/
sudo chown root:wheel /Library/LaunchDaemons/com.outflow.server.plist
sudo chmod 644 /Library/LaunchDaemons/com.outflow.server.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.outflow.server.plist
```

Restart after a rebuild:

```
sudo launchctl kickstart -k system/com.outflow.server
```

Logs go to the daemon user's `~/Library/Logs`, not `/tmp` — `/tmp` is
world-readable and world-writable, so anyone on the box could read the server's
output or pre-create the file.

### API tokens

| Token | Grants |
|---|---|
| `OUTFLOW_API_TOKEN[_FILE]` | all of `/api/*` |
| `OUTFLOW_API_TOKEN_RO[_FILE]` | `GET /api/*` only — 403 on every mutation |

Generate both and write them 0600 next to the other secrets:

```
openssl rand -hex 32 > ~/Library/"Application Support"/outflow/api-token
openssl rand -hex 32 > ~/Library/"Application Support"/outflow/api-token-ro
chmod 600 ~/Library/"Application Support"/outflow/api-token{,-ro}
```

**Use the `_FILE` form in the plist, never the inline value.** Files under
`/Library/LaunchDaemons` are world-readable (0644 root:wheel — that is what
launchd wants), so a token in the env block is legible to every account on the
machine, including the ones the token exists to keep out. `_FILE` reads a
0600 file at startup and refuses to boot if it is group/other-readable; the
inline env form stays for dev and for the CLI.

Keep both in a password manager anyway. Losing one costs a file rewrite plus a
`kickstart` — nothing is encrypted with them, unlike `OUTFLOW_DB_KEY`.

Setting **either** flips the whole `/api` surface to deny-by-default (401
without a valid token); with neither set the API is open to anything that can
reach the port. "The tailnet is the boundary" holds only while everything on
the tailnet — and everything *on the box* — is equally trusted. A container or
an agent user running locally reaches `127.0.0.1:8080` too (on colima, the
VM's user-mode networking forwards `host.docker.internal` into the host's
loopback), so a loopback bind is not a boundary against it.

Give the **RO** token to agents and scripts (`OUTFLOW_API_TOKEN_RO` where the
CLI runs, in `--server` mode): they can query the archive but can never
`/reset_data`, trigger a Plaid pull, or spend LLM budget. Keep the full token
for yourself — the web UI prompts for it once and remembers it in that
browser's localStorage.

The read-only tier is method-based: every read here is a `GET` and every
mutation is a `POST`/`DELETE`, so a newly added mutating route is denied to the
RO token automatically, with no allowlist to maintain.

## 6. Link the banks

Open `https://<mini>.<tailnet>.ts.net` → **Connections** → **Connect a bank**.
Chase/Amex/CapOne bounce through the bank's OAuth page and return to
`/oauth-return`; the first sync runs immediately (Plaid backfills up to ~24
months). The background sync then runs every `OUTFLOW_SYNC_INTERVAL_SECS`;
**Sync now** and `POST /api/pull` force one. Item status `reconnect needed`
(expired OAuth consent) → **Reconnect** on the same screen.

## 7. Agent / CLI access over the tailnet

Give an agent the **read-only** token, never the full one:

```
export OUTFLOW_SERVER=https://MINI.TAILNET.ts.net
export OUTFLOW_API_TOKEN=<the api-token-ro value>
outflow txns --search starbucks --since 2026-06-01 --json
```

The read-only token goes in the same `OUTFLOW_API_TOKEN` variable: the CLI just
forwards whatever bearer it is given and the server decides the tier. Read
subcommands work; mutating ones (`pull`, `fix`, `matches accept`) come back
403 saying the token is read-only.

**From a container on the mini**, MagicDNS usually isn't configured inside the
container, so the `ts.net` name won't resolve. Reach the server on the host's
loopback instead — under colima and Docker Desktop the VM forwards
`host.docker.internal` into the host's loopback, so a `127.0.0.1`-bound server
is reachable:

```
export OUTFLOW_SERVER=http://host.docker.internal:8080
```

That hop is plain HTTP but never leaves the machine. It still requires the
token — being on the box is not authorization.

(Build the CLI with `--features client`. `report`, `subs`, `accounts`,
`matches`, `status`, `pull`, `fix` all work in server mode and print the same
JSON shapes as local `--json`.)

## 8. Backups

The DB is the permanent archive — back up the daemon user's
`~/Library/Application Support/outflow/` (DB + plaid-tokens.json + the API
token files + key file if encrypted). Time Machine covers it if the mini has a
backup target.

Only one of those is unrecoverable: **the DB key**. Re-linking banks re-mints
Plaid tokens and the API tokens are one `openssl rand` away, but a lost
`OUTFLOW_DB_KEY[_FILE]` orphans the encrypted archive permanently, and Plaid
only backfills ~24 months of it. Back that key up somewhere other than the mini.
