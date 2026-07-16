# Gotchas & hard-won lessons

Non-obvious things that have already cost time. Skim before touching the API
boundary, Plaid, secrets, encryption, or the demo setup.

## API boundary (server ↔ SPA ↔ CLI)

- **The wire format is snake_case serde, verbatim.** The old Tauri
  camelCase↔snake_case translation is gone; `app/src/api.ts` (the single HTTP
  client) sends Rust field names (`txn_id`, `public_token`).
- **`Money` is a number in JS.** serde-transparent newtype → bare number
  (cents). Never expect an object.
- **`Store` is not `Sync`** (holds a rusqlite `Connection`). The server keeps
  it in `Arc<Mutex<Store>>` and does all store work inside `spawn_blocking`;
  network calls happen **outside** the lock so reads stay responsive mid-sync.
- **Missing filter ≠ default filter.** Absent query params must mean
  "everything" (pending included). Do NOT derive a `Default` that sets
  `include_pending = false` — that silently drops pending rows (bug fixed once
  already). See `FilterParams::to_filter` in `server/src/routes.rs`.
- **The SPA fallback must return 200.** tower-http's
  `ServeDir::not_found_service` serves the fallback file but keeps the 404
  status; the server uses an explicit axum `.fallback(spa_index)` instead.
  `/oauth-return` must load the app with a 200 or Plaid OAuth resumption
  breaks.

## Plaid

- **Sign conventions are inverted vs the domain.** Plaid transaction amounts:
  positive = money out → negate at parse. Credit-account
  `balances.current`: positive = owed → negate for `AccountKind::Credit`.
  Both flips live in `core::plaid` only.
- **Amounts are JSON doubles.** Never do f64 math — `Number::to_string()` →
  `Money::from_decimal_str`. There's a cents-exactness test (`4.22` → `-422`).
- **Pending→posted changes the transaction id.** The posted row carries
  `pending_transaction_id`; the parser collects those and
  `apply_plaid_batch` deletes them with the batch.
- **The cursor is part of the batch transaction.** Persisting a cursor whose
  data didn't commit silently loses transactions forever. On
  `TRANSACTIONS_SYNC_MUTATION_DURING_PAGINATION`, restart the whole loop from
  the stored cursor (bounded retries in `server/src/sync.rs`).
- **OAuth banks (Chase/Amex/CapOne) need the exact registered redirect URI** —
  HTTPS, byte-for-byte match with the Plaid dashboard
  (`https://<mini>.<tailnet>.ts.net/oauth-return`). The SPA stashes the
  link_token in localStorage before redirect and re-initializes Link with
  `receivedRedirectUri` on return.
- **`ITEM_LOGIN_REQUIRED` is routine**, not fatal: the item's status flips to
  `login_required`, the sync run continues with other items, and the
  Connections screen offers Reconnect (update-mode link token, no re-exchange).
- **Access tokens count against the Trial plan's 10-Item lifetime cap** —
  removing items does NOT free slots. Don't link/unlink casually in
  production; use sandbox for testing.
- **Item removal at Plaid is best-effort on unlink** — local history is kept
  on purpose (the archive outliving the connection is the point of the app).

## Headless mac-mini (see DEPLOYMENT.md)

- **launchd services get no shell env and no unlocked GUI keychain.** All
  secrets via a plist env block + 0600 files: `OUTFLOW_PLAID_SECRET_FILE`,
  `OUTFLOW_PLAID_TOKENS_FILE`, `OUTFLOW_DB_KEY_FILE`.
  Setting env in your shell does NOT reach the daemon.
- **Bind loopback; let `tailscale serve` do TLS.** The server listens on
  `127.0.0.1:8080`; tailnet exposure + certs come from
  `tailscale serve https / http://127.0.0.1:8080`.
- **A loopback bind is not a boundary against the local machine.** Other
  accounts and containers reach `127.0.0.1:8080` too — colima's user-mode
  networking forwards `host.docker.internal` straight into the host's loopback.
  If anything else runs on the box, the API tokens are the boundary, not the
  bind address.
- **A daemon gets no `HOME`.** `Config::from_env` falls back to `"."`, so
  without `OUTFLOW_DATA_DIR` the DB and `plaid-tokens.json` are created under
  `WorkingDirectory` instead of Application Support — silently, and possibly
  somewhere another account can read. Always set `OUTFLOW_DATA_DIR` in the plist.
- **`/Library/LaunchDaemons` plists are world-readable** (0644 root:wheel, which
  is what launchd requires). Anything in the `EnvironmentVariables` block is
  readable by every account on the box, so secrets go in via `*_FILE` (0600),
  never inline. That includes the API tokens.
- **Whoever can write the binary or `app/dist` owns the daemon.** `UserName`
  only decides which account's secrets the server reads; it does not protect
  what it executes. Swapping `target/release/outflow-server` runs code as that
  user, and injecting JS into `app/dist` lifts the API token out of a browser's
  localStorage. The daemon user needs its own clone that nobody else can write —
  deploy via git, not by pointing the plist at another account's working copy.

## Encryption

- **Plaintext ↔ encrypted are not interchangeable.** A DB created plaintext
  cannot be opened with `open_encrypted`, and vice versa. Switching a path's
  mode requires deleting the file first.
- **The DB key comes from `OUTFLOW_DB_KEY` / `OUTFLOW_DB_KEY_FILE` (0600) only
  — there is no keychain path.** Losing or changing the key orphans the
  existing encrypted DB forever, so back the key file up alongside the DB.

## Data / demo

- **`/tmp` gets purged by macOS.** Don't stage the DB or secrets there — use a
  durable path (`$HOME/...` or the data dir).
- **The DB is the permanent archive.** Plaid only serves a window (up to
  ~24 months on first sync). `reset_data`
  clears transactions/accounts/sync_log/matches and resets Plaid cursors (so
  the next sync replays what Plaid still has) but **cannot recover anything
  older than the provider window** — back up the DB file before resetting.
- **Demo / testing:** Plaid sandbox (`user_good`/`pass_good`), or the offline
  fixture (`pull --from-file examples/plaid-fixture.json`) for a
  zero-credential pipeline run.

## Known design limits (not bugs)

- Card-payment auto-matching pairs **equal amounts within 5 days** — partial
  payments and split payments don't match (flag manually; the learn-a-rule
  path still works).
- Own-account transfers (checking↔savings) auto-flag only when Plaid
  classifies them (`TRANSFER_IN/OUT`); anything else is a manual flag.
- Annual subscriptions are undetectable until the DB holds >1 year.
