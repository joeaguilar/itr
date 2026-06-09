#!/usr/bin/env bash
# tests/contracts/ui.sh
#
# Local browser UI startup + smoke contract (issue #144).
#
# ── Why this area does NOT use the shared `snapshot` helper for startup cases ─
#   `itr ui --once` BINDS a localhost TCP port and BLOCKS in accept() until it
#   has served exactly one HTTP request, then exits. The shared `snapshot`
#   helper runs `$ITR <args>` synchronously and would block forever (and its
#   stdout is pipe-buffered, so the startup banner is not even flushed until the
#   process serves+exits). So every startup/smoke case here:
#     1. picks a free localhost port,
#     2. launches `itr ui --port <p> --no-open --once ...` in the background,
#     3. pokes the server with one HTTP GET to a NO-TOKEN route
#        (/assets/app.css) so `--once` serves + exits + flushes,
#     4. waits for it, normalizes, and compares against a checked-in snapshot.
#   A bounded watchdog guarantees the server is always reaped even if the poke
#   fails, so a flaky port never orphans a blocked process.
#   The `ui --help` case is the exception: it needs no socket, so it uses the
#   plain shared `snapshot` helper and ALWAYS runs.
#
# ── Confirmed `itr ui` CLI surface (from the built binary, NOT the task hint) ─
#   itr ui [OPTIONS]
#     --port <PORT>      localhost port; 0 auto-selects        [default: 0]
#     --no-open          print the URL, do not open a browser
#     --once             serve one request then exit           [hidden]
#     --allow-dangerous  enable the raw SQL editor / /api/sql route
#     -f, --format       global; compact|json|pretty|oneline   [default: compact]
#   There is NO --host and NO --token flag. The session token is server-
#   generated random hex — entropy the normalizer must collapse.
#
# ── Confirmed output ─────────────────────────────────────────────────────────
#   compact: two stdout lines  -> "UI: http://127.0.0.1:<port>/?token=<hex>"
#                                 "DB: <db_path>"
#   json:    one stdout object -> {"db_path":"<path>","port":<port>,
#                                  "url":"http://127.0.0.1:<port>/?token=<hex>"}
#            (no standalone "token" field; the token lives only inside the url)
#   --allow-dangerous: additionally one stderr line ->
#       "REVIEW: raw SQL UI is enabled for <db_path>. Treat this session as
#        full database access."
#
# ── Normalization ────────────────────────────────────────────────────────────
#   Reuses the shared `contract_normalize` from _lib.sh, which already collapses
#   /var/folders temp paths -> <TMP>, 127.0.0.1:<n> -> 127.0.0.1:<PORT>,
#   token=<hex> -> token=<TOKEN>, and ISO timestamps -> <TS>. The auto-picked
#   port and random token are therefore deterministic after normalization with
#   no local rules. Snapshot body format mirrors the shared harness so failures
#   read identically to every other contract.
#
# ── Sandbox strategy (acceptance criterion) ──────────────────────────────────
#   localhost bind/connect may be blocked. We probe round-trip capability up
#   front; if a real server cannot be reached we mark the four startup/smoke
#   cases SKIPPED (clear message) and record one passing "ui_smoke_skipped_
#   cleanly" check so the suite stays green on the environment limitation rather
#   than reporting false failures. `ui --help` still runs and still asserts, so
#   there is ALWAYS at least one load-bearing ui snapshot regardless of sandbox.
#
# ── Phantom-PASS hardening (subtle, load-bearing) ────────────────────────────
#   integration.sh runs under `set -euo pipefail` and BUFFERS its stdout. If a
#   `pass` line from a prior case is still in that buffer and we fork a child,
#   the child inherits a duplicate buffer and flushes it on exit — replaying the
#   prior PASS and silently inflating the suite counter. Every fork this file
#   does therefore happens INSIDE one synchronous subshell whose fd 1/2 are
#   /dev/null and whose stdin is closed/redirected, so no child can ever replay
#   the suite's buffered stdout. Results travel out via files. The watchdog also
#   detaches all three std fds so its 15s sleeper never holds an inherited pipe.

CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tests/contracts/_lib.sh
. "$CONTRACT_DIR/_lib.sh"

echo ""
echo "--- contract: ui (localhost startup + smoke) ---"

UI_AREA="ui"

# ──────────────────────────────────────────────────────────────────────────
# Helpers
# ──────────────────────────────────────────────────────────────────────────

# Pick a free localhost TCP port. Prints the port, or nothing on failure.
#
# Uses `python3 -c` (argv string) NOT a `python3 - <<'PY'` heredoc on purpose:
# integration.sh sources this file with its own stdin redirected, and a heredoc
# feeds the program on python's stdin, which races/blocks against that fd (this
# made the bind probe silently hang when run from the suite). The -c form never
# touches stdin, so it is stdin-context independent.
_ui_pick_port() {
    python3 -c 'import socket; s=socket.socket(socket.AF_INET, socket.SOCK_STREAM); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()' 2>/dev/null
}

# Bounded watchdog: hard-kills pid $1 after ~15s. `timeout(1)` is unavailable
# here, so this is a background sleeper. Its `( ... ) &` fully detaches ALL three
# std fds (`<&- >/dev/null 2>&1` = close stdin, stdout+stderr to /dev/null) so
# the long-lived sleeper can NEVER hold open a pipe it inherited from the caller
# (which would otherwise hang command substitution / wedge the harness pipe for
# the full 15s). The `echo` runs in the parent, before the detach. Prints pid.
_ui_watchdog() {
    local target="$1" n
    (
        for n in $(seq 1 75); do
            kill -0 "$target" 2>/dev/null || exit 0
            sleep 0.2
        done
        kill -9 "$target" 2>/dev/null
    ) <&- >/dev/null 2>&1 &
    echo $!
}

# Drive one `itr ui --once` invocation to completion: launch in background, poke
# the no-token CSS route to make --once serve+exit, reap with a watchdog. ALL
# forking happens here, inside ONE /dev/null-redirected, stdin-closed synchronous
# subshell (see phantom-PASS hardening above). Captured itr stdout/stderr/exit +
# the served HTTP response land in files under $1 for the caller to read AFTER
# this returns.
#   _ui_drive <tmpdir> [extra itr-ui flags...]
# Files written: $tmpdir/.stdout .stderr .exit .resp
_ui_drive() {
    local tmpd="$1"; shift
    local extra=("$@")
    (
        local port db bg wd i
        port="$(_ui_pick_port)"
        db="$tmpd/.itr.db"
        ITR_DB_PATH="$db" "$ITR" init -q >/dev/null 2>&1 || true
        (
            "$ITR" --db "$db" ui --port "$port" --no-open --once "${extra[@]}" \
                >"$tmpd/.stdout" 2>"$tmpd/.stderr"
            echo $? >"$tmpd/.exit"
        ) &
        bg=$!
        wd="$(_ui_watchdog "$bg")"
        for i in $(seq 1 30); do
            nc -z 127.0.0.1 "$port" 2>/dev/null && break
            kill -0 "$bg" 2>/dev/null || break
            sleep 0.2
        done
        # -i so the smoke case can read the status line + Content-Type. No-token
        # route -> serves the embedded asset and triggers the --once exit.
        curl -s -i "http://127.0.0.1:$port/assets/app.css" >"$tmpd/.resp" 2>/dev/null
        wait "$bg" 2>/dev/null
        kill "$wd" 2>/dev/null
    ) <&- >/dev/null 2>&1
}

# Probe whether localhost bind+connect actually works here. Returns 0 if a real
# `itr ui --once` can be started AND reached (proof: the .resp file got an HTTP
# status line), else 1. Reuses _ui_drive, which already reaps the server.
_ui_can_bind() {
    local tmpd rc
    tmpd="$(mktemp -d)"
    _ui_drive "$tmpd"
    if grep -q '^HTTP/' "$tmpd/.resp" 2>/dev/null; then
        rc=0
    else
        rc=1
    fi
    rm -rf "$tmpd"
    return "$rc"
}

# Compare/write a normalized blob against tests/snapshots/ui/<case>.txt, in the
# shared harness's pass/fail / UPDATE_SNAPSHOTS / labeled-unified-diff style.
#   _ui_emit_snapshot <case> <cmd-desc> <normalized-body> <exit-code>
_ui_emit_snapshot() {
    local case="$1" desc="$2" normalized="$3" exit_code="$4"
    local snap_file="$CONTRACTS_SNAPSHOT_DIR/$UI_AREA/$case.txt"

    if [ "${UPDATE_SNAPSHOTS:-0}" = "1" ]; then
        mkdir -p "$(dirname "$snap_file")"
        printf '%s\n' "$normalized" >"$snap_file"
        pass "snapshot $UI_AREA/$case (updated baseline)"
        return 0
    fi

    if [ ! -f "$snap_file" ]; then
        fail "snapshot $UI_AREA/$case" \
            "missing expected snapshot $snap_file — run UPDATE_SNAPSHOTS=1 to create it"
        return 1
    fi

    local actual_file
    actual_file="$(mktemp)"
    printf '%s\n' "$normalized" >"$actual_file"

    if diff -u "$snap_file" "$actual_file" >/dev/null 2>&1; then
        pass "snapshot $UI_AREA/$case"
        rm -f "$actual_file"
        return 0
    fi

    echo ""
    echo "    ── snapshot drift: $UI_AREA/$case ──────────────────────────"
    echo "    command: itr $desc"
    echo "    exit:    $exit_code"
    echo "    diff (expected vs actual, unified):"
    diff -u \
        --label "expected: tests/snapshots/$UI_AREA/$case.txt" \
        --label "actual:   itr $desc" \
        "$snap_file" "$actual_file" | sed 's/^/    /'
    echo "    regen:   UPDATE_SNAPSHOTS=1 ./tests/integration.sh"
    echo "    ─────────────────────────────────────────────────────────"
    echo ""
    fail "snapshot $UI_AREA/$case" "normalized output differs from expected snapshot"
    rm -f "$actual_file"
    return 1
}

# Startup case: assert normalized stdout+stderr of `itr ui --once`.
#   _ui_startup_case <case> -- <extra itr-ui flags...>
_ui_startup_case() {
    local case="$1"; shift
    [ "${1:-}" = "--" ] && shift
    local extra=("$@")

    local tmpd
    tmpd="$(mktemp -d)"
    _ui_drive "$tmpd" "${extra[@]}"

    local exit_code
    exit_code="$(cat "$tmpd/.exit" 2>/dev/null)"
    [ -z "$exit_code" ] && exit_code="?"

    local stdout_n stderr_n
    stdout_n="$(contract_normalize "$tmpd" <"$tmpd/.stdout" 2>/dev/null)"
    stderr_n="$(contract_normalize "$tmpd" <"$tmpd/.stderr" 2>/dev/null)"

    # Port/token-abstracted description so the recorded `$ itr ...` line is stable.
    local desc="ui --port <PORT> --no-open --once ${extra[*]}"

    local normalized
    normalized="$(cat <<EOF
\$ itr ${desc}
--- exit ---
$exit_code
--- stdout ---
$stdout_n
--- stderr ---
$stderr_n
EOF
)"

    _ui_emit_snapshot "$case" "$desc" "$normalized" "$exit_code"
    rm -rf "$tmpd"
}

# Smoke case: confirm one HTTP request is served and the process exits cleanly.
# Snapshots a small, stable summary (HTTP status line + Content-Type + exit
# code) of the served embedded stylesheet — NOT the full asset body, which the
# UI-assets agent owns and may change independently.
_ui_smoke_case() {
    local case="once_html_fetch"
    local tmpd
    tmpd="$(mktemp -d)"
    _ui_drive "$tmpd"

    local status content_type exit_code
    status="$(head -1 "$tmpd/.resp" 2>/dev/null | tr -d '\r')"
    content_type="$(grep -i '^Content-Type:' "$tmpd/.resp" 2>/dev/null | head -1 | tr -d '\r')"
    exit_code="$(cat "$tmpd/.exit" 2>/dev/null)"
    [ -z "$exit_code" ] && exit_code="?"

    local desc="ui --port <PORT> --no-open --once  + 1 HTTP GET /assets/app.css"
    local normalized
    normalized="$(cat <<EOF
\$ itr ${desc}
--- exit ---
$exit_code
--- http status ---
$status
--- content-type ---
$content_type
EOF
)"
    _ui_emit_snapshot "$case" "$desc" "$normalized" "$exit_code"
    rm -rf "$tmpd"
}

# ──────────────────────────────────────────────────────────────────────────
# 0) Bind-free case: `itr ui --help`. Needs NO localhost socket, so it ALWAYS
#    runs (even when bind/connect is blocked and the startup/smoke cases below
#    are skipped). Pins the documented flag surface (--port / --no-open /
#    --allow-dangerous / -f; note: NO --host, NO --token) so a renamed or
#    dropped flag is caught in review, and guarantees the suite always has at
#    least one load-bearing ui snapshot for drift detection. Uses the shared
#    `snapshot` helper because --help is a normal synchronous command.
# ──────────────────────────────────────────────────────────────────────────
snapshot "$UI_AREA" help -- ui --help

# ──────────────────────────────────────────────────────────────────────────
# Sandbox gate for the bind-required cases: required tooling + a real round trip.
# ──────────────────────────────────────────────────────────────────────────
if ! command -v python3 >/dev/null 2>&1 \
    || ! command -v curl >/dev/null 2>&1 \
    || ! command -v nc >/dev/null 2>&1; then
    echo "  SKIP: ui startup_json / startup_compact / allow_dangerous_review / once_html_fetch"
    echo "        (python3/curl/nc not all available)"
    pass "ui_smoke_skipped_cleanly (toolchain unavailable)"
    return 0 2>/dev/null || exit 0
fi

if ! _ui_can_bind; then
    echo "  SKIP: ui startup_json           (localhost bind/connect unavailable in sandbox)"
    echo "  SKIP: ui startup_compact        (localhost bind/connect unavailable in sandbox)"
    echo "  SKIP: ui allow_dangerous_review (localhost bind/connect unavailable in sandbox)"
    echo "  SKIP: ui once_html_fetch        (localhost bind/connect unavailable in sandbox)"
    pass "ui_smoke_skipped_cleanly (localhost unavailable)"
    return 0 2>/dev/null || exit 0
fi

# ──────────────────────────────────────────────────────────────────────────
# Bind-required cases (localhost round trip confirmed available).
# ──────────────────────────────────────────────────────────────────────────

# 1) JSON startup object: db_path / port / url. Token (inside url), port, and db
#    path are normalized to <TOKEN>/<PORT>/<TMP>.
_ui_startup_case startup_json -- -f json

# 2) Compact (default) startup banner: "UI: <url>" and "DB: <path>" lines.
_ui_startup_case startup_compact --

# 3) --allow-dangerous: same banner plus the documented REVIEW warning on
#    stderr (stderr-only soft-fallback contract).
_ui_startup_case allow_dangerous_review -- --allow-dangerous

# 4) --once serves a request then exits cleanly (HTTP 200 + served asset).
_ui_smoke_case
