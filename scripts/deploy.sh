#!/bin/bash
# deploy.sh: build and install outflow releases on a macOS launchd host.
#
#   scripts/deploy.sh try <ref> [--port P] [--check SCRIPT] [--oura]
#   scripts/deploy.sh try --clean
#   scripts/deploy.sh build <tag> [--rebuild]
#   scripts/deploy.sh install <tag> [--check SCRIPT]
#   scripts/deploy.sh rollback [<tag>] [--restore-db]
#   scripts/deploy.sh env list
#   scripts/deploy.sh env set|unset|restore [--yes] [--restart] ARGS
#   scripts/deploy.sh verify [--log RE]... [--wait-log RE] [--timeout S]
#                            [--check SCRIPT]
#   scripts/deploy.sh status
#
# Every deploy is a release (docs/RELEASING.md). Each release lives in
# its own folder under <data dir>/releases/<tag>/ (outflow-server,
# outflow, dist/, BUILD) and launchd runs releases/current, a symlink.
# Installing or rolling back switches that link, so the development
# checkout is never what's live.
#
# try      No sudo; nothing live changes. Builds <ref> (a branch or
#          commit) with the release features in $BUILD_DIR/try-src,
#          snapshots the live DB into $BUILD_DIR/try-data (0700), starts
#          that build on 127.0.0.1:8091 (--port) with no bank sync, runs
#          the install checks against it, then waits. Ctrl-C or TERM
#          stops it and deletes the scratch data; --clean does the same
#          after a kill -9. --oura is vitals-only and refused here.
# build    No sudo. Checks the tag is a published release on origin/main,
#          checks it out in a separate build worktree, compiles with
#          OUTFLOW_VERSION=<tag> (so /api/version reports it) and the UI,
#          and stages releases/<tag>/. Nothing live changes.
# install  Needs sudo, so run it in a terminal. Stops the server, backs
#          up the DB as <db>.pre-<tag>, points current at <tag>, starts,
#          then checks pages, the main API endpoints (with the read-only
#          token when one is configured) and /api/version. --check runs
#          `python3 SCRIPT DIR` on the fetched JSON for release-specific
#          checks. The first install moves launchd from the old layout
#          (binary and app/dist in a checkout) to releases/current: the
#          old binary and UI are kept as releases/pre-releases and the old
#          plist as <data dir>/<plist name>.pre-releases.
#          Afterwards it prunes to the newest $KEEP_RELEASES releases and
#          $KEEP_DB_BACKUPS DB backups.
# rollback Points current back at <tag>, default the release before the
#          live one. --restore-db also puts back the DB backup taken
#          before the live release was installed (needed after a
#          migration).
# env      The plist's EnvironmentVariables. list prints them. set
#          KEY=VALUE..., unset KEY... and restore [FILE] (default: the
#          newest backup) edit a temp copy, lint it, show the diff, ask,
#          back up the live plist to <data dir>/plist-backups/, and
#          install it root:wheel 644 (sudo). Secret-looking keys (*TOKEN,
#          *SECRET, *PASSWORD, *KEY) are refused: the plist is world
#          readable, so it names a 0600 file (*_FILE) instead. A change
#          takes effect at the next start: install, rollback, or
#          --restart now. --yes skips the question (tests).
# verify   No sudo. Reports only what it observed: the live tag against
#          /api/version, the install checks (plus --check), and each
#          --log pattern in the log lines written since the live release
#          was installed or rolled back to. --wait-log polls for a line
#          until --timeout (default 900s). Exits 1 if anything is missing.
# status   What's live, what's installed, and the install history.
#
# Settings come from the launchd plist (data dir, listen address, token
# file, log); override with env: OUTFLOW_PLIST, OUTFLOW_BUILD_DIR
# (default ~/outflow-build), OUTFLOW_FEATURES (default encryption),
# KEEP_RELEASES (5), KEEP_DB_BACKUPS (3), KEEP_PLIST_BACKUPS (10). Tests
# set LAUNCHCTL and SUDO.
set -euo pipefail

PLIST=${OUTFLOW_PLIST:-/Library/LaunchDaemons/com.outflow.server.plist}
BUILD_DIR=${OUTFLOW_BUILD_DIR:-$HOME/outflow-build}
FEATURES=${OUTFLOW_FEATURES-encryption}
KEEP_RELEASES=${KEEP_RELEASES:-5}
KEEP_DB_BACKUPS=${KEEP_DB_BACKUPS:-3}
KEEP_PLIST_BACKUPS=${KEEP_PLIST_BACKUPS:-10}
LAUNCHCTL=${LAUNCHCTL:-launchctl}
SUDO=${SUDO-sudo}
REPO=$(cd "$(dirname "$0")/.." && pwd)
TAG_RE='^v[0-9]{4}\.[0-9]{2}\.[0-9]+$'
TRY_SRC="$BUILD_DIR/try-src"
TRY_BIN="$BUILD_DIR/try-bin"
TRY_DATA="$BUILD_DIR/try-data"
TRY_PIDFILE="$BUILD_DIR/try.pid"

STOPPED=0 SWITCHED=0
die() {
  echo "deploy: $*" >&2
  if [ "$STOPPED" = 1 ] || [ "$SWITCHED" = 1 ]; then
    [ "$STOPPED" = 0 ] || echo "deploy: the server may be down." >&2
    echo "deploy: restore the previous release with: scripts/deploy.sh rollback" >&2
  fi
  exit 1
}
step() { printf '\n--- %s\n' "$*"; }
pb() { /usr/libexec/PlistBuddy -c "Print :$1" "${2:-$PLIST}" 2>/dev/null || true; }

load_plist() {
  [ -r "$PLIST" ] || die "can't read $PLIST"
  LABEL=$(pb Label)
  PROG=$(pb ProgramArguments:0)
  DATA=$(pb EnvironmentVariables:OUTFLOW_DATA_DIR)
  DATA=${DATA:-$HOME/Library/Application Support/outflow}
  DB=$(pb EnvironmentVariables:OUTFLOW_DB)
  DB=${DB:-$DATA/outflow.db}
  WEB=$(pb EnvironmentVariables:OUTFLOW_WEB_DIR)
  LISTEN=$(pb EnvironmentVariables:OUTFLOW_LISTEN)
  LISTEN=${LISTEN:-127.0.0.1:8080}
  RO_FILE=$(pb EnvironmentVariables:OUTFLOW_API_TOKEN_RO_FILE)
  LOG=$(pb StandardOutPath)
  [ -n "$LABEL" ] && [ -n "$PROG" ] || die "$PLIST has no Label or ProgramArguments"
  REL="$DATA/releases"
  CUR="$REL/current"
}

check_tag() {
  [[ $1 =~ $TAG_RE ]] || die "$1 isn't a release tag (vYYYY.MM.N)"
}

live_tag() { # the tag current points at, or empty
  [ -L "$CUR" ] && basename "$(readlink "$CUR")" || true
}

previous_tag() { # newest history entry that isn't live and still exists
  local live t
  live=$(live_tag)
  [ -f "$REL/history" ] || return 0
  # history lines: date time tag [note]
  tail -r "$REL/history" | awk '{print $3}' | while read -r t; do
    if [ "$t" != "$live" ] && [ -d "$REL/$t" ]; then echo "$t"; break; fi
  done
}

# ── shared: check out, compile ─────────────────────────────────────────
checkout_into() { # dir commit: a detached worktree of this repo at commit
  local src=$1 ref=$2
  mkdir -p "$(dirname "$src")"
  if [ -d "$src/.git" ] || [ -f "$src/.git" ]; then
    git -C "$src" checkout --quiet --detach "$ref"
    git -C "$src" clean -fdq
  else
    git worktree add --quiet --detach "$src" "$ref"
  fi
}

compile_into() { # src version dir: build src, copy the binaries and UI to dir
  local src=$1 ver=$2 dir=$3
  step "compile (features: ${FEATURES:-none}, OUTFLOW_VERSION=$ver)"
  local feat=()
  [ -z "$FEATURES" ] || feat=(--features "$FEATURES")
  (cd "$src" && OUTFLOW_VERSION="$ver" CARGO_TARGET_DIR="$BUILD_DIR/target" \
    cargo build --release --locked ${feat[@]+"${feat[@]}"} -p outflow-server -p outflow-cli)

  step "build the UI"
  (cd "$src/app" && npm ci --no-audit --no-fund --loglevel=error && npm run build --silent)

  mkdir -p "$dir"
  cp -p "$BUILD_DIR/target/release/outflow-server" "$BUILD_DIR/target/release/outflow" "$dir/"
  cp -Rp "$src/app/dist" "$dir/dist"
}

# ── build ──────────────────────────────────────────────────────────────
cmd_build() {
  local tag=$1 rebuild=${2:-}
  check_tag "$tag"
  load_plist
  cd "$REPO"
  git fetch --quiet --tags origin
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null || die "no tag $tag"
  git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null ||
    die "$tag isn't on origin; publish the release first"
  git merge-base --is-ancestor "$tag" origin/main || die "$tag isn't on main"
  local dest="$REL/$tag"
  if [ -e "$dest" ]; then
    [ "$(live_tag)" != "$tag" ] || die "$tag is live; nothing to build"
    [ "$rebuild" = --rebuild ] || die "$dest exists; pass --rebuild to replace it"
    rm -rf "$dest"
  fi

  local src="$BUILD_DIR/src"
  step "check out $tag in $src"
  checkout_into "$src" "$tag"
  local sha
  sha=$(git -C "$src" rev-parse HEAD)

  local part="$REL/.$tag.partial"
  rm -rf "$part"
  compile_into "$src" "$tag" "$part"
  step "stage $dest"
  printf 'tag %s\ncommit %s\nbuilt %s\nfeatures %s\n' \
    "$tag" "$sha" "$(date '+%F %T %z')" "${FEATURES:-none}" >"$part/BUILD"
  mv "$part" "$dest"
  local v
  if v=$("$dest/outflow" --version 2>/dev/null); then
    [ "$v" = "outflow $tag" ] || die "built CLI reports '$v', expected outflow $tag"
  else
    v=
  fi
  echo "built $tag ($sha): $dest"
  [ -n "$v" ] || echo "note: $tag predates version reporting (outflow --version)"
  echo "next, in a terminal: scripts/deploy.sh install $tag"
}

# ── shared: stop, start, checks ────────────────────────────────────────
stop_server() {
  STOPPED=1
  $SUDO "$LAUNCHCTL" bootout "system/$LABEL" || die "bootout failed"
  local _
  for _ in $(seq 1 30); do
    pgrep -f "$PROG" >/dev/null || break
    sleep 1
  done
  ! pgrep -f "$PROG" >/dev/null || die "server still running after bootout"
  echo "stopped"
}

log_lines() { # lines in the server log so far
  [ -n "$LOG" ] && [ -f "$LOG" ] && wc -l <"$LOG" | tr -d ' ' || echo 0
}

start_server() {
  local before up=0 _
  before=$(log_lines)
  $SUDO "$LAUNCHCTL" bootstrap system "$PLIST" || die "bootstrap failed"
  if [ -n "$LOG" ]; then
    for _ in $(seq 1 60); do
      if tail -n +"$((before + 1))" "$LOG" 2>/dev/null | grep -q "outflow server up"; then
        up=1
        break
      fi
      sleep 1
    done
    echo "new log lines:"
    tail -n +"$((before + 1))" "$LOG" | tail -15 | sed 's/^/   /'
    [ "$up" = 1 ] || die "no 'outflow server up' in $LOG within 60s"
  fi
  STOPPED=0
}

mark_started() { # tag log-line: what verify reads the log from
  printf 'started %s\nlog_line %s\n' "$(date '+%F %T %z')" "$2" >"$REL/$1/INSTALLED"
}

api_get() { # name path -> $TMP/name.json, prints the status code
  if [ -n "$RO_FILE" ] && [ -r "$RO_FILE" ]; then
    # The token goes to curl on stdin, never argv.
    printf 'Authorization: Bearer %s\n' "$(cat "$RO_FILE")" |
      curl -s -H @- -o "$TMP/$1.json" -w '%{http_code}' "http://$LISTEN/api$2"
  else
    curl -s -o "$TMP/$1.json" -w '%{http_code}' "http://$LISTEN/api$2"
  fi
}

run_checks() { # tag [check-script]
  local tag=$1 extra=${2:-} path code name
  step "checks"
  for path in / /flow /holdings /settings; do
    code=$(curl -s -o /dev/null -w '%{http_code}' "http://$LISTEN$path")
    echo "GET $path -> $code"
    [ "$code" = 200 ] || die "$path returned $code"
  done
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/outflow-deploy.XXXXXX")
  while read -r name path; do
    code=$(api_get "$name" "$path")
    echo "GET /api$path -> $code"
    [ "$code" = 200 ] || die "/api$path returned $code"
  done <<'EOF'
checkin /checkin
checkin_last /checkin?period=last-month
checkin_week /checkin?period=last-week
checkin_quarter /checkin?period=last-quarter
flow /flow
recurring /recurring
balances /balances
change /net-worth/change?period=last-month
change_now /net-worth/change?period=this-month
change_last /net-worth/change?period=last-month
accounts_now /holdings/change?period=this-month
positions /holdings/change?by=position
upkeep /upkeep
reset_preview /reset/preview
accounts /accounts?include_hidden=true
items /plaid/items
accepted /matches?status=accepted
proposed /matches?status=proposed
EOF
  code=$(api_get version /version)
  if [ "$code" = 200 ]; then
    local got
    got=$(/usr/bin/python3 -c '
import json, sys
try:
    print(json.load(open(sys.argv[1])).get("version"))
except Exception:
    print("unreadable")' "$TMP/version.json")
    echo "GET /api/version -> $got"
    [ "$tag" = pre-releases ] || [ "$got" = "$tag" ] || die "server reports $got, expected $tag"
  else
    echo "GET /api/version -> $code (this release predates /api/version)"
  fi
  if [ -n "$extra" ]; then
    /usr/bin/python3 -I "$extra" "$TMP" || die "$extra failed"
  fi
  rm -rf "$TMP"
}

# ── try ────────────────────────────────────────────────────────────────
# Product part: which live settings the try server gets, and how the DB
# is copied. Everything else (Plaid, OAuth, sync) is left out, so a try
# server never talks to a bank.
try_env() { # KEY=VALUE lines for the try server
  local k v
  for k in OUTFLOW_DB_KEY_FILE OUTFLOW_API_TOKEN_FILE OUTFLOW_API_TOKEN_RO_FILE OUTFLOW_PERKS TZ; do
    v=$(pb "EnvironmentVariables:$k")
    [ -z "$v" ] || echo "$k=$v"
  done
  echo "OUTFLOW_DATA_DIR=$TRY_DATA"
  echo "OUTFLOW_DB=$TRY_DATA/outflow.db"
  echo "OUTFLOW_WEB_DIR=$TRY_BIN/dist"
  echo "OUTFLOW_LISTEN=$LISTEN"
  echo "OUTFLOW_SYNC_INTERVAL_SECS=999999999"
}

try_snapshot() { # live DB -> $TRY_DATA/outflow.db
  # `outflow backup` uses SQLite's online backup and verifies the copy,
  # so it's safe while the live server writes. The live release's CLI
  # reads its own schema; the branch's might expect a newer one.
  local cli="$TRY_BIN/outflow"
  [ ! -x "$CUR/outflow" ] || cli="$CUR/outflow"
  OUTFLOW_DB_KEY_FILE=$(pb EnvironmentVariables:OUTFLOW_DB_KEY_FILE) \
    "$cli" --db "$DB" backup --to "$TRY_DATA/outflow.db"
}

try_cli_hint() { # how to run the branch's CLI on the scratch DB
  echo "CLI:  OUTFLOW_DB_KEY_FILE=\"$(pb EnvironmentVariables:OUTFLOW_DB_KEY_FILE)\" \\"
  echo "        \"$TRY_BIN/outflow\" --db \"$TRY_DATA/outflow.db\" <command>"
}

try_clean() { # stop a leftover try server, delete scratch data
  local pid
  if [ -f "$TRY_PIDFILE" ]; then
    pid=$(cat "$TRY_PIDFILE")
    if ps -p "$pid" -o command= 2>/dev/null | grep -q "$TRY_BIN/"; then
      kill "$pid" 2>/dev/null || true
      echo "stopped try server $pid"
    fi
    rm -f "$TRY_PIDFILE"
  fi
  if [ -e "$TRY_DATA" ]; then
    rm -rf "$TRY_DATA"
    echo "deleted $TRY_DATA"
  fi
}

TRY_PID=
try_stop() {
  trap - EXIT INT TERM
  if [ -n "$TRY_PID" ]; then
    kill "$TRY_PID" 2>/dev/null || true
    wait "$TRY_PID" 2>/dev/null || true
    echo "try server stopped; last log lines:"
    tail -5 "$TRY_DATA/server.log" 2>/dev/null | sed 's/^/   /' || true
  fi
  rm -f "$TRY_PIDFILE"
  rm -rf "$TRY_DATA"
  echo "deleted $TRY_DATA"
}

cmd_try() { # ref port check-script oura
  local ref=$1 port=$2 extra=$3 oura=$4
  [ "$oura" = 0 ] || die "--oura is for vitals; outflow has no Oura"
  [[ $port =~ ^[0-9]+$ ]] || die "--port takes a number"
  load_plist
  [ -z "$extra" ] || [ -r "$extra" ] || die "can't read $extra"
  [ -e "$DB" ] || die "no DB at $DB"
  LISTEN=127.0.0.1:$port
  ! lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1 ||
    die "something already listens on $port; pick another with --port"
  cd "$REPO"
  git fetch --quiet origin || echo "note: git fetch failed; using local refs"
  local sha ver
  sha=$(git rev-parse -q --verify "$ref^{commit}") || die "no commit $ref"
  ver="try-$(git rev-parse --short "$sha")"
  try_clean

  step "check out $ref ($sha) in $TRY_SRC"
  checkout_into "$TRY_SRC" "$sha"
  rm -rf "$TRY_BIN"
  compile_into "$TRY_SRC" "$ver" "$TRY_BIN"

  step "snapshot the live DB into $TRY_DATA"
  (umask 077 && mkdir -p "$TRY_DATA")
  chmod 700 "$TRY_DATA"
  trap try_stop EXIT
  trap 'exit 130' INT TERM
  try_snapshot

  step "start $ver on $LISTEN"
  local envs=() line _ up=0
  while IFS= read -r line; do envs+=("$line"); done < <(try_env)
  env -i HOME="$HOME" PATH="/usr/bin:/bin:/usr/sbin:/sbin" "${envs[@]}" \
    "$TRY_BIN/outflow-server" >"$TRY_DATA/server.log" 2>&1 &
  TRY_PID=$!
  echo "$TRY_PID" >"$TRY_PIDFILE"
  for _ in $(seq 1 60); do
    kill -0 "$TRY_PID" 2>/dev/null || break
    if grep -q "outflow server up" "$TRY_DATA/server.log"; then up=1; break; fi
    sleep 1
  done
  sed 's/^/   /' "$TRY_DATA/server.log" | tail -15
  [ "$up" = 1 ] || die "the try server didn't come up"
  run_checks "$ver" "$extra"

  printf '\n=== %s (%s) runs on http://%s ===\n' "$ref" "$ver" "$LISTEN"
  echo "data: $TRY_DATA (a snapshot of the live DB, deleted on stop)"
  echo "log:  $TRY_DATA/server.log"
  try_cli_hint
  echo "stop: Ctrl-C, or kill $$"
  wait "$TRY_PID" || true
  echo "the try server exited by itself"
}

# ── env ────────────────────────────────────────────────────────────────
install_plist() { # file: install it as the live plist, root:wheel 644
  if [ -n "$SUDO" ]; then
    $SUDO install -m 644 -o root -g wheel "$1" "$PLIST"
  else
    install -m 644 "$1" "$PLIST"
  fi
}

plist_xml() { plutil -convert xml1 -o - "$1"; }

env_key() { # key: refuse names that aren't env vars or look like secrets
  [[ $1 =~ ^[A-Z][A-Z0-9_]*$ ]] || die "$1 isn't an env var name (A-Z, 0-9, _)"
  case $1 in
  *TOKEN | *SECRET | *PASSWORD | *KEY)
    die "$1 would put a secret in a world-readable plist; point ${1}_FILE at a 0600 file instead"
    ;;
  *_DATA_DIR | *_DB | *_LISTEN)
    echo "warning: $1 changes where the server keeps its data or listens" >&2
    ;;
  esac
}

ENV_YES=0 ENV_RESTART=0
env_apply() { # new-plist: lint, diff, ask, back up, install
  local new=$1 backups="$DATA/plist-backups" b
  plutil -lint -s "$new" || die "the edited plist doesn't lint; $PLIST is unchanged"
  if cmp -s <(plist_xml "$PLIST") <(plist_xml "$new"); then
    echo "no change to $PLIST"
    return 0
  fi
  step "changes to $PLIST"
  diff -u --label "now" --label "new" <(plist_xml "$PLIST") <(plist_xml "$new") || true
  if [ "$ENV_YES" != 1 ]; then
    [ -t 0 ] || die "run this in a terminal to confirm (nothing changed)"
    local a
    read -r -p "install this plist? [y/N] " a
    [ "$a" = y ] || [ "$a" = Y ] || die "not installed; $PLIST is unchanged"
  fi
  (umask 077 && mkdir -p "$backups")
  # Named by time, so sorting by name puts the newest last (cp -p keeps
  # the live plist's mtime, so ls -t can't tell).
  b="$backups/$(basename "$PLIST").$(date '+%Y%m%d-%H%M%S')"
  local base=$b n=1
  while [ -e "$b" ]; do n=$((n + 1)) && b="$base-$n"; done
  cp -p "$PLIST" "$b"
  cmp -s "$PLIST" "$b" || die "plist backup differs from the original"
  echo "backed up the live plist to $b"
  install_plist "$new" || die "install failed; the old plist is in $b"
  echo "installed $PLIST"
  n=0
  for b in $(ls "$backups" | grep -F "$(basename "$PLIST")." | sort -r); do
    n=$((n + 1))
    [ "$n" -le "$KEEP_PLIST_BACKUPS" ] || rm -f "$backups/$b"
  done
  if [ "$ENV_RESTART" = 1 ]; then
    load_plist
    [ "$PROG" = "$CUR/outflow-server" ] || die "--restart needs launchd on releases/current"
    step "restart $(live_tag)"
    stop_server
    mark_started "$(live_tag)" "$(log_lines)"
    start_server
    run_checks "$(live_tag)"
  else
    echo "takes effect at the next start: install, rollback, or env --restart"
  fi
}

cmd_env() { # action args...
  local action=$1
  shift
  load_plist
  if [ "$action" = list ]; then
    plutil -extract EnvironmentVariables json -o - "$PLIST" | /usr/bin/python3 -I -c '
import json, sys
for k, v in sorted(json.load(sys.stdin).items()):
    print(f"{k}={v}")'
    return 0
  fi
  local a args=()
  for a in "$@"; do
    case $a in
    --yes) ENV_YES=1 ;;
    --restart) ENV_RESTART=1 ;;
    *) args+=("$a") ;;
    esac
  done
  [ "$ENV_RESTART" = 0 ] || [ -t 0 ] || [ -z "$SUDO" ] ||
    die "run --restart in a terminal (sudo asks for a password)"
  local new k v
  new=$(mktemp "${TMPDIR:-/tmp}/outflow-plist.XXXXXX")
  case $action in
  set | unset)
    [ ${#args[@]} -gt 0 ] || die "usage: deploy.sh env set KEY=VALUE... | env unset KEY..."
    cp "$PLIST" "$new"
    /usr/libexec/PlistBuddy -c "Print :EnvironmentVariables" "$new" >/dev/null 2>&1 ||
      /usr/libexec/PlistBuddy -c "Add :EnvironmentVariables dict" "$new"
    for a in "${args[@]}"; do
      if [ "$action" = set ]; then
        [[ $a == *=* ]] || die "set takes KEY=VALUE, got $a"
        k=${a%%=*} v=${a#*=}
        env_key "$k"
        [ -n "$v" ] || die "$k: empty value; use env unset $k"
        [[ $v != *[\"\'\\]* ]] && [[ $v != *$'\n'* ]] ||
          die "$k: quotes, backslashes and newlines aren't supported in values"
        /usr/libexec/PlistBuddy -c "Delete :EnvironmentVariables:$k" "$new" 2>/dev/null || true
        /usr/libexec/PlistBuddy -c "Add :EnvironmentVariables:$k string $v" "$new"
        [ "$(pb "EnvironmentVariables:$k" "$new")" = "$v" ] || die "$k didn't read back as written"
      else
        [[ $a =~ ^[A-Z][A-Z0-9_]*$ ]] || die "$a isn't an env var name"
        if [ -z "$(pb "EnvironmentVariables:$a" "$new")" ]; then
          echo "$a isn't set"
        else
          /usr/libexec/PlistBuddy -c "Delete :EnvironmentVariables:$a" "$new"
        fi
      fi
    done
    ;;
  restore)
    [ ${#args[@]} -le 1 ] || die "usage: deploy.sh env restore [FILE]"
    local from=${args[0]:-}
    if [ -z "$from" ]; then
      from=$(ls "$DATA/plist-backups" 2>/dev/null | grep -F "$(basename "$PLIST")." | sort | tail -1 || true)
      [ -n "$from" ] || die "no backups in $DATA/plist-backups"
      from="$DATA/plist-backups/$from"
    fi
    [ -r "$from" ] || die "can't read $from"
    echo "restoring $from"
    cp "$from" "$new"
    ;;
  *) die "usage: deploy.sh env list|set|unset|restore ..." ;;
  esac
  env_apply "$new"
  rm -f "$new"
}

# ── install ────────────────────────────────────────────────────────────
migrate_plist() { # old layout -> releases/current; server must be stopped
  local legacy="$REL/pre-releases" new
  step "move launchd to $CUR"
  if [ ! -d "$legacy" ]; then
    mkdir -p "$legacy.partial"
    cp -p "$PROG" "$legacy.partial/outflow-server"
    [ ! -e "$(dirname "$PROG")/outflow" ] || cp -p "$(dirname "$PROG")/outflow" "$legacy.partial/"
    [ -z "$WEB" ] || cp -Rp "$WEB" "$legacy.partial/dist"
    printf 'tag pre-releases\nfrom %s\nsaved %s\n' "$PROG" "$(date '+%F %T %z')" \
      >"$legacy.partial/BUILD"
    mv "$legacy.partial" "$legacy"
    echo "kept the old build as $legacy"
  fi
  cp -p "$PLIST" "$DATA/$(basename "$PLIST").pre-releases"
  new=$(mktemp "${TMPDIR:-/tmp}/outflow-plist.XXXXXX")
  cp "$PLIST" "$new"
  /usr/libexec/PlistBuddy -c "Set :ProgramArguments:0 $CUR/outflow-server" "$new"
  /usr/libexec/PlistBuddy -c "Delete :WorkingDirectory" "$new" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Add :WorkingDirectory string $DATA" "$new"
  /usr/libexec/PlistBuddy -c "Delete :EnvironmentVariables:OUTFLOW_WEB_DIR" "$new" 2>/dev/null || true
  /usr/libexec/PlistBuddy -c "Add :EnvironmentVariables:OUTFLOW_WEB_DIR string $CUR/dist" "$new"
  plutil -lint -s "$new" || die "rewritten plist doesn't lint"
  install_plist "$new"
  rm -f "$new"
  # pre-releases is the rollback target until the first install succeeds.
  ln -sfn pre-releases "$CUR"
  echo "$(date '+%F %T') pre-releases" >>"$REL/history"
  echo "launchd now runs $CUR/outflow-server (old plist kept in $DATA)"
}

prune() {
  local live prev keep d n
  live=$(live_tag)
  prev=$(previous_tag)
  n=0
  # Newest first by when they were installed or built.
  for d in $(ls -t "$REL" | grep -E "$TAG_RE|^pre-releases$"); do
    if [ "$d" = "$live" ] || [ "$d" = "$prev" ]; then continue; fi
    n=$((n + 1))
    keep=$((KEEP_RELEASES - 2))
    if [ "$n" -gt "$keep" ]; then
      rm -rf "${REL:?}/$d"
      echo "pruned release $d"
    fi
  done
  n=0
  # Only backups this script made (pre-<release tag>); older ones such as
  # pre-v15 (a schema version) are left alone.
  for d in $(ls -t "$(dirname "$DB")" | grep -E "^$(basename "$DB")\.pre-v[0-9]{4}\.[0-9]{2}\.[0-9]+$"); do
    n=$((n + 1))
    if [ "$n" -gt "$KEEP_DB_BACKUPS" ]; then
      rm -f "$(dirname "$DB")/$d" "$(dirname "$DB")/$d-wal" "$(dirname "$DB")/$d-shm"
      echo "pruned DB backup $d"
    fi
  done
}

cmd_install() {
  local tag=$1 extra=${2:-}
  check_tag "$tag"
  [ -t 0 ] || [ -z "$SUDO" ] || die "run install in a terminal (sudo asks for a password)"
  load_plist
  local dest="$REL/$tag"
  [ -f "$dest/BUILD" ] || die "$tag isn't built; run: scripts/deploy.sh build $tag"
  [ "$(live_tag)" != "$tag" ] || die "$tag is already live"
  [ -z "$extra" ] || [ -r "$extra" ] || die "can't read $extra"
  [ -e "$DB" ] || die "no DB at $DB"
  [ ! -e "$DB.pre-$tag" ] ||
    die "$tag was installed before ($DB.pre-$tag exists); go back to it with: scripts/deploy.sh rollback $tag"
  local migrate=0
  [ "$PROG" = "$CUR/outflow-server" ] || migrate=1
  step "install $tag (live: $(live_tag || true))"
  [ "$migrate" = 0 ] || echo "launchd runs $PROG today; this install moves it to $CUR"
  sed 's/^/   /' "$dest/BUILD"
  [ -z "$SUDO" ] || $SUDO -v

  step "stop"
  stop_server
  step "back up the DB to $DB.pre-$tag"
  cp -p "$DB" "$DB.pre-$tag"
  local ext
  for ext in -wal -shm; do
    [ ! -e "$DB$ext" ] || cp -p "$DB$ext" "$DB.pre-$tag$ext"
  done
  cmp -s "$DB" "$DB.pre-$tag" || die "DB backup differs from the original"
  [ "$migrate" = 0 ] || migrate_plist
  load_plist # PROG may have changed
  step "switch current to $tag"
  ln -sfn "$tag" "$CUR"
  SWITCHED=1
  echo "$(date '+%F %T') $tag" >>"$REL/history"
  mark_started "$tag" "$(log_lines)"
  step "start"
  start_server
  run_checks "$tag" "$extra"
  step "prune"
  prune
  printf '\n=== %s is live ===\n' "$tag"
  echo "next: scripts/deploy.sh verify [--log RE] [--wait-log RE]"
}

# ── rollback ───────────────────────────────────────────────────────────
cmd_rollback() {
  local tag=${1:-} restore=${2:-}
  load_plist
  [ "$PROG" = "$CUR/outflow-server" ] || die "launchd isn't on releases/current yet; nothing to roll back to"
  [ -t 0 ] || [ -z "$SUDO" ] || die "run rollback in a terminal (sudo asks for a password)"
  local live
  live=$(live_tag)
  [ -n "$tag" ] || tag=$(previous_tag)
  [ -n "$tag" ] || die "no earlier release to roll back to"
  [ -d "$REL/$tag" ] || die "$tag isn't installed in $REL"
  [ "$tag" != "$live" ] || die "$tag is already live"
  if [ "$restore" = --restore-db ]; then
    [ -e "$DB.pre-$live" ] || die "no $DB.pre-$live to restore"
  fi
  step "roll back $live -> $tag${restore:+ (restoring the DB from before $live)}"
  [ -z "$SUDO" ] || $SUDO -v
  stop_server
  if [ "$restore" = --restore-db ]; then
    cp -p "$DB" "$DB.rolled-back-$live"
    cp -p "$DB.pre-$live" "$DB"
    rm -f "$DB-wal" "$DB-shm"
    local ext
    for ext in -wal -shm; do
      [ ! -e "$DB.pre-$live$ext" ] || cp -p "$DB.pre-$live$ext" "$DB$ext"
    done
    echo "DB restored (the replaced one is $DB.rolled-back-$live)"
  fi
  ln -sfn "$tag" "$CUR"
  echo "$(date '+%F %T') $tag rollback from $live" >>"$REL/history"
  mark_started "$tag" "$(log_lines)"
  start_server
  run_checks "$tag"
  printf '\n=== rolled back to %s ===\n' "$tag"
}

# ── verify ─────────────────────────────────────────────────────────────
log_since() { # line: the log from that line on, without colour codes
  tail -n +"$1" "$LOG" | sed $'s/\x1b\\[[0-9;]*m//g'
}

cmd_verify() { # extra timeout wait-re log-re...
  local extra=$1 timeout=$2 wait_re=$3
  shift 3
  load_plist
  local tag from=1 fail=0 re hit
  tag=$(live_tag)
  [ -n "$tag" ] || die "launchd isn't on releases/current yet"
  [ -z "$extra" ] || [ -r "$extra" ] || die "can't read $extra"
  step "live: $tag"
  if [ -f "$CUR/INSTALLED" ]; then
    sed 's/^/   /' "$CUR/INSTALLED"
    from=$(($(awk '$1 == "log_line" {print $2}' "$CUR/INSTALLED") + 1))
    [ "$from" -le "$(($(log_lines) + 1))" ] || from=1 # the log was rotated
  else
    echo "   no INSTALLED mark (installed by an older deploy.sh); reading the whole log"
  fi
  run_checks "$tag" "$extra"
  if [ $# -gt 0 ] || [ -n "$wait_re" ]; then
    [ -n "$LOG" ] && [ -r "$LOG" ] || die "can't read the server log ($LOG)"
    step "log since $tag started (line $from)"
  fi
  for re in "$@"; do
    hit=$(log_since "$from" | grep -E -- "$re" | tail -1 || true)
    if [ -n "$hit" ]; then
      echo "found /$re/: $hit"
    else
      echo "NOT FOUND /$re/"
      fail=1
    fi
  done
  if [ -n "$wait_re" ]; then
    local waited=0
    echo "waiting up to ${timeout}s for /$wait_re/"
    while :; do
      hit=$(log_since "$from" | grep -E -- "$wait_re" | tail -1 || true)
      [ -z "$hit" ] || break
      [ "$waited" -lt "$timeout" ] || break
      sleep 10
      waited=$((waited + 10))
    done
    if [ -n "$hit" ]; then
      echo "found /$wait_re/ after ~${waited}s: $hit"
    else
      echo "NOT FOUND /$wait_re/ within ${timeout}s"
      fail=1
    fi
  fi
  if [ "$fail" = 1 ]; then
    echo "verify: not everything expected was observed (see NOT FOUND above)" >&2
    exit 1
  fi
  printf '\n=== %s verified ===\n' "$tag"
}

# ── status ─────────────────────────────────────────────────────────────
cmd_status() {
  load_plist
  if [ "$PROG" != "$CUR/outflow-server" ]; then
    echo "launchd runs $PROG (old layout; the first install moves it to releases/)"
  else
    echo "live: $(live_tag)"
    [ ! -f "$CUR/BUILD" ] || sed 's/^/   /' "$CUR/BUILD"
  fi
  TMP=$(mktemp -d "${TMPDIR:-/tmp}/outflow-deploy.XXXXXX")
  local code
  code=$(api_get version /version || true)
  if [ "$code" = 200 ]; then
    echo "server says: $(cat "$TMP/version.json")"
  else
    echo "server says: /api/version -> ${code:-no answer}"
  fi
  rm -rf "$TMP"
  if [ -d "$REL" ]; then
    echo "installed: $(ls -t "$REL" | grep -E "$TAG_RE|^pre-releases$" | tr '\n' ' ')"
    echo "rollback target: $(previous_tag)"
    [ ! -f "$REL/history" ] || { echo "history:"; tail -5 "$REL/history" | sed 's/^/   /'; }
  fi
  [ ! -d "$TRY_DATA" ] || echo "try scratch data exists: $TRY_DATA (scripts/deploy.sh try --clean)"
}

usage() {
  sed -n '4,13p' "$0" >&2
  exit 2
}

case ${1:-} in
try)
  shift
  ref= port=8091 check= oura=0
  [ "${1:-}" != --clean ] || { [ $# -eq 1 ] || usage; try_clean; exit 0; }
  while [ $# -gt 0 ]; do
    case $1 in
    --port) [ $# -ge 2 ] || usage; port=$2; shift ;;
    --check) [ $# -ge 2 ] || usage; check=$2; shift ;;
    --oura) oura=1 ;;
    -*) usage ;;
    *) [ -z "$ref" ] || usage; ref=$1 ;;
    esac
    shift
  done
  [ -n "$ref" ] || usage
  cmd_try "$ref" "$port" "$check" "$oura"
  ;;
build)
  [ $# -ge 2 ] || die "usage: deploy.sh build <tag> [--rebuild]"
  cmd_build "$2" "${3:-}"
  ;;
install)
  [ $# -eq 2 ] || { [ $# -eq 4 ] && [ "$3" = --check ]; } ||
    die "usage: deploy.sh install <tag> [--check SCRIPT]"
  cmd_install "$2" "${4:-}"
  ;;
rollback)
  shift
  t= r=
  for a in "$@"; do
    case $a in --restore-db) r=$a ;; *) t=$a ;; esac
  done
  cmd_rollback "$t" "$r"
  ;;
env)
  [ $# -ge 2 ] || usage
  shift
  cmd_env "$@"
  ;;
verify)
  shift
  check= timeout=900 wait_re= res=()
  while [ $# -gt 0 ]; do
    case $1 in
    --log) [ $# -ge 2 ] || usage; res+=("$2"); shift ;;
    --wait-log) [ $# -ge 2 ] || usage; wait_re=$2; shift ;;
    --timeout) [ $# -ge 2 ] || usage; timeout=$2; shift ;;
    --check) [ $# -ge 2 ] || usage; check=$2; shift ;;
    *) usage ;;
    esac
    shift
  done
  [[ $timeout =~ ^[0-9]+$ ]] || die "--timeout takes seconds"
  cmd_verify "$check" "$timeout" "$wait_re" ${res[@]+"${res[@]}"}
  ;;
status) cmd_status ;;
*) usage ;;
esac
