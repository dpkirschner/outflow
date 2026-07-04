# Development

## Agent constraint (read first)

The agent's sandbox shell **cannot run `cargo` or `node`** — they aren't on the
non-interactive PATH. Ask the user to run build/test/run commands themselves
(via the `!` prefix or their terminal) and report output. `cargo` is via rustup
(`~/.cargo/bin`); Node + npm are present. Give commands as **one single-line
command per code block** — the user copy-pastes and wrapped/multi-line blocks
break (this preference is also in the user's memory).

## Layout

```
core/  net/  cli/         Rust crates
app/                      npm project (NOT a cargo member)
  src/                    React + Vite + TS frontend
  src-tauri/              Rust Tauri backend (a cargo member)
examples/sample-accounts.json   SimpleFIN fixture (12 txns) for --from-file
```

## Test

```
cargo test -p outflow-core -p outflow-net
```

Do NOT run bare `cargo test` at the root — it tries to compile `app/src-tauri`,
whose `generate_context!` needs a built frontend (`../dist`), which fails without
a prior `vite build`. Test the pure crates by `-p`.

## CLI

Build with the features you need (`net` = live SimpleFIN + LLM, `keychain`,
`encryption`):

```
cargo build -p outflow-cli --features "net,keychain,encryption"
```

Run subcommands (`--db` / `OUTFLOW_DB` selects the database):

```
cargo run -p outflow-cli -- --db "$HOME/outflow.db" pull --from-file examples/sample-accounts.json
```

```
cargo run -p outflow-cli --features "net,keychain" -- --db "$HOME/outflow.db" claim <setup-token>
```

Subcommands: `claim <token>`, `pull [--from-file P]`, `categorize [--llm]`,
`report --by category|merchant|monthly [--top --since --until --posted-only]`,
`subs`, `fix <id> <category> [--no-learn]`.

## GUI — dev

Runs plaintext (no `encryption`), net+keychain on by default. Point `OUTFLOW_DB`
at a populated DB to see data:

```
cd app && npm install
```

```
cd app && OUTFLOW_DB="$HOME/outflow.db" npm run tauri dev
```

The backend prints `outflow: opened db <path> (<N> transactions)` on startup — a
one-glance check of which DB was opened. An empty/fresh file is the usual reason
"no data shows".

## GUI — production `.app`

One command produces the encrypted, ad-hoc-signed bundle:

```
cd app && npm run bundle
```

(= `tauri build --features encryption`.) Output:
`app/src-tauri/target/release/bundle/macos/outflow.app` (+ `.dmg`). Install:

```
cp -R app/src-tauri/target/release/bundle/macos/outflow.app /Applications/
```

The production app resolves its DB path and encryption key itself (app-data dir +
keychain) — no env needed. See ARCHITECTURE.md "Config resolution" and GOTCHAS.md.

## Definition of done (project)

`cargo test` green; release build with `net,keychain,encryption` succeeds on
macOS; live `pull` populates the DB from the real SimpleFIN endpoint; the GUI
shows the dashboards and recategorization persists and learns rules; CLI is
cron-runnable headless.

## Toolchain note

`cli/Cargo.toml` pins `clap = "=4.4.18"` only because it was first built on Rust
1.75. On current stable, loosen to `clap = "4"`. Nothing else depends on the pin.
