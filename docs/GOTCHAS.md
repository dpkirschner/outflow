# Gotchas & hard-won lessons

Non-obvious things that have already cost time. Skim before touching the GUI,
keychain, encryption, or the demo setup.

## Tauri boundary

- **Argument casing.** Tauri v2 converts JS camelCase → Rust snake_case. A Rust
  command param `txn_id` is called from JS as `{ txnId }`. Single-word params are
  unaffected. (`app/src/api.ts` ↔ `app/src-tauri/src/main.rs`)
- **`Money` is a number in JS.** serde-transparent newtype → bare number (cents).
  Never expect an object; format with `formatCents`.
- **`Store` is not `Sync`** (holds a rusqlite `Connection`). Tauri state is a
  `Mutex<Store>`; every command locks it.
- **Missing filter ≠ default filter.** A `None` filter must mean "everything"
  (`TxnFilter::all()`). Do NOT rely on `FilterArg`'s derived `Default` — that
  sets `include_pending = false` and silently drops pending rows. Use the
  `to_filter` helper. (bug fixed once already)
- **`cargo test` at the root fails** on `app/src-tauri` (needs a built frontend
  for `generate_context!`). Test with `-p outflow-core -p outflow-net`.

## macOS packaging

- **Gatekeeper.** Because the app is **built locally** (not downloaded), macOS
  usually doesn't set the quarantine flag, so a copied `.app` double-clicks fine.
  If it ever balks: right-click → Open once, or
  `xattr -dr com.apple.quarantine /Applications/outflow.app`. No paid Apple
  Developer account is needed for personal use.
- **Ad-hoc signing (`signingIdentity: "-"`) changes per build.** Keychain ACLs
  are keyed to the code signature, so after a rebuild macOS may re-prompt for
  keychain access — click **Always Allow**. A stable self-signed cert would end
  the re-prompts if it becomes annoying.
- **Finder launch has no shell env.** `OUTFLOW_*` vars are all unset when
  double-clicked; that's why config resolves from app-data + keychain. Setting an
  env var in your shell does NOT reach the `.app`.

## Encryption

- **Plaintext ↔ encrypted are not interchangeable.** A DB created plaintext (dev)
  cannot be opened with `open_encrypted`, and vice versa. If you switch a given
  path's mode, delete the file first. Fresh installs are unaffected.
- **Verifying encryption is real:** a plaintext `sqlite3` open of the encrypted
  file must fail. The app-data DB lives at
  `~/Library/Application Support/com.outflow.app/outflow.db`.
- **Never regenerate the DB key except on `NoEntry`** — see INVARIANTS #8. A new
  key orphans the existing encrypted DB forever.

## Data / demo

- **`/tmp` gets purged by macOS.** Don't stage the DB or secrets there — a purge
  empties them and the app silently recreates an empty DB. Use a durable path
  (`$HOME/...` or the app-data dir).
- **Launch auto-pull uses whatever access URL is in the keychain.** This caused a
  real contamination: during testing the keychain held the **demo** URL, so the
  first launch auto-pulled demo data before the user connected their real bank;
  the real Pull then layered on top (distinct SimpleFIN ids → both coexist). A
  clean install can't hit this. Recovery: the **Reset** button
  (`Store::reset_data` — clears transactions/accounts/sync_log, keeps learned
  rules), or quit + `rm` the app-data DB + relaunch.
- **SimpleFIN setup tokens are single-use.** `claim` consumes the token and
  exchanges it for a durable access URL (stored in the keychain). Don't delete
  that keychain item expecting to re-Connect with the same token — you'd need a
  fresh token from SimpleFIN Bridge. The access URL is the durable secret.
- **Connect from inside the app**, not the CLI, so the keychain item is owned by
  the app's code signature (avoids cross-app keychain prompts).
- **Demo credentials** (for testing only): setup token
  `aHR0cHM6Ly9iZXRhLWJyaWRnZS5zaW1wbGVmaW4ub3JnL3NpbXBsZWZpbi9jbGFpbS9ERU1P`,
  which claims to `https://demo:demo@beta-bridge.simplefin.org/simplefin`.

## Known design limits (not bugs)

- Transfers between your own accounts read as outflow (no transfer detection yet).
- Annual subscriptions are undetectable until the DB holds >1 year.
- Months are bucketed in UTC, not local time.
