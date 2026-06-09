#!/usr/bin/env bash
# tests/contracts/_lib.sh
#
# Shared library for the normalized CLI-output snapshot harness (issue #140).
#
# This file is SOURCED by both tests/integration.sh (the auto-discovery runner)
# and by individual per-area contract scripts under tests/contracts/<area>.sh.
# It is never executed directly.
#
# What it provides to contract scripts:
#   - `snapshot <area> <case> [stdin-data] -- <itr args...>`
#       Runs an itr command in an ISOLATED temp database, captures stdout,
#       stderr, and exit status, NORMALIZES runtime entropy, and compares the
#       result against a checked-in expected snapshot at
#       tests/snapshots/<area>/<case>.txt. On mismatch it prints a unified diff
#       (diff -u) labeled with command, args, stdout, stderr, and exit status,
#       then records a failure. The temp DB and its directory are discarded
#       after each call, so cases never share state.
#   - `snapshot_seeded <area> <case> <seed_fn> [stdin] -- <itr args...>`
#       Same as `snapshot`, but first calls `<seed_fn> <db_path>` to populate
#       the isolated DB with fixtures before running the asserted command.
#   - `UPDATE_SNAPSHOTS=1` mode: instead of asserting, (re)writes the expected
#       snapshot file with the normalized output so authors can generate
#       baselines, then review the diff in git.
#   - pass/fail integration: it calls the `pass`/`fail` helpers defined by
#       integration.sh, so contract results fold into the suite totals and the
#       existing reporting style. When sourced standalone (for local
#       development of a single contract file) it defines minimal fallbacks.
#
# Normalizations applied to BOTH stdout and stderr (see contract_normalize):
#   - UTC ISO-8601 timestamps  -> <TS>
#       e.g. 2026-05-29T20:24:12Z, 2026-05-29T20:24:12.123Z
#   - wall-clock day counts    -> <DAYS>   (issue #151)
#       summary's oldest-open age is RAW (unclamped) days since created_at, so
#       it drifts by 1 every day for any fixed fixture created_at. Collapse the
#       `"days_old":N` JSON field and the compact `(Nd old)` rendering so the
#       summary snapshots are date-independent. (The urgency `age` component is
#       made deterministic separately, by pinning created_at to an ancient date
#       so age saturates to a constant -- see contract_pin_created_at.)
#   - mktemp temp paths        -> <TMP>
#       e.g. /tmp/tmp.XXXX, /var/folders/.../T/tmp.XXXX, and the per-case DB dir
#   - localhost ports          -> 127.0.0.1:<PORT>  /  localhost:<PORT>
#   - UI session tokens        -> token=<TOKEN>  /  X-ITR-Token: <TOKEN>
#   - version describe suffix  -> itr X.Y.Z
#       strips the optional leading `v`, the `-<n>-g<hash>` git-describe suffix,
#       a `+<hash>` build-metadata suffix, and a trailing `-dirty`.
#
# Adding a new area is purely additive: drop tests/contracts/<area>.sh that
# sources THIS file and calls `snapshot`, plus tests/snapshots/<area>/*.txt.
# Never edit integration.sh for a new area — it auto-discovers contract files.

# ──────────────────────────────────────────────────────────────────────────
# Resolve paths. CONTRACTS_REPO_ROOT is the repository root; snapshots live
# under tests/snapshots relative to it.
# ──────────────────────────────────────────────────────────────────────────
CONTRACTS_LIB_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONTRACTS_REPO_ROOT="$(cd "$CONTRACTS_LIB_DIR/../.." && pwd)"
CONTRACTS_SNAPSHOT_DIR="$CONTRACTS_REPO_ROOT/tests/snapshots"

# ──────────────────────────────────────────────────────────────────────────
# Resolve the itr binary. When sourced by integration.sh, $ITR is already set
# to the same binary the suite (and the verify gate) uses. When a contract file
# is run standalone, fall back to a sensible default / first CLI argument.
# ──────────────────────────────────────────────────────────────────────────
if [ -z "${ITR:-}" ]; then
    if [ -n "${1:-}" ] && [ -x "$1" ]; then
        ITR="$1"
    else
        ITR="$CONTRACTS_REPO_ROOT/target/release/itr"
    fi
    case "$ITR" in
        /*) ;;
        *) ITR="$CONTRACTS_REPO_ROOT/$ITR" ;;
    esac
fi

# ──────────────────────────────────────────────────────────────────────────
# Fallback pass/fail/counters when run standalone. integration.sh defines the
# real ones and they take precedence because it sources this file AFTER
# defining them. These fallbacks mirror integration.sh's reporting style.
# ──────────────────────────────────────────────────────────────────────────
if ! declare -F pass >/dev/null 2>&1; then
    PASS=${PASS:-0}
    FAIL=${FAIL:-0}
    TESTS=()
    pass() {
        PASS=$((PASS + 1))
        TESTS+=("PASS: $1")
        echo "  PASS: $1"
    }
    fail() {
        FAIL=$((FAIL + 1))
        TESTS+=("FAIL: $1 — $2")
        echo "  FAIL: $1 — $2"
    }
fi

# ──────────────────────────────────────────────────────────────────────────
# contract_normalize: read stdin, write normalized text to stdout.
#
# Order matters: normalize the most specific patterns (timestamps, the
# per-case temp dir) before the broad temp-path catch-all.
# ──────────────────────────────────────────────────────────────────────────
contract_normalize() {
    # $1 (optional): the per-case temp dir to collapse first, so a known mktemp
    # path maps cleanly to <TMP> even if its layout differs across platforms.
    local case_tmp="${1:-__no_such_tmp__}"
    sed -E \
        -e "s#${case_tmp}#<TMP>#g" \
        -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z/<TS>/g' \
        -e 's/("days_old": *)[0-9]+/\1<DAYS>/g' \
        -e 's/DAYS:[0-9]+/DAYS:<DAYS>/g' \
        -e 's/\(([0-9]+)d old\)/(<DAYS>d old)/g' \
        -e 's/\(([0-9]+) days\)/(<DAYS> days)/g' \
        -e 's#/var/folders/[A-Za-z0-9_./+-]*#<TMP>#g' \
        -e 's#/tmp/tmp\.[A-Za-z0-9]+#<TMP>#g' \
        -e 's#/tmp/[A-Za-z0-9_.-]*tmp[A-Za-z0-9_.-]*#<TMP>#g' \
        -e 's#(127\.0\.0\.1):[0-9]+#\1:<PORT>#g' \
        -e 's#(localhost):[0-9]+#\1:<PORT>#g' \
        -e 's#(token=)[A-Za-z0-9._-]+#\1<TOKEN>#g' \
        -e 's#(X-ITR-Token: )[A-Za-z0-9._-]+#\1<TOKEN>#g' \
        -e 's#(itr )v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9]+-g[0-9a-f]+)?(\+[0-9a-f]+)?(-dirty)?#\1X.Y.Z#g'
}

# ──────────────────────────────────────────────────────────────────────────
# PUBLIC: contract_pin_created_at <db> <id> [iso]
#
# Pin an issue's created_at (and updated_at) to a fixed, deterministic instant
# so urgency-bearing snapshots are TIMING-INDEPENDENT.
#
# Why this exists (issue #151): the urgency `age` component is
#   config.age * clamp(days_since(created_at)/10, 0, 1)
# and `days_since` is `Utc::now() - created_at` in real seconds. A fixture READ
# in the same wall-clock second it was CREATED has age EXACTLY 0.0; once >=1s
# elapses, age becomes a hair >0 (e.g. 4.6e-06), which (a) flips whether `age`
# shows in compact output and (b) drifts the urgency TOTAL in JSON. That made
# urgency snapshots non-deterministic across runs.
#
# Fix: pin created_at to an ANCIENT date (default 2000-01-01T00:00:00Z). Then
# `days_since/10` clamps to 1.0, so `age` saturates to the CONSTANT
# `urgency.age` coefficient (default 2.0) on every run, forever -- fully
# deterministic, zero maintenance. The scoring math in src/urgency.rs is
# unchanged; only the fixture's stored timestamp is pinned.
#
# updated_at is pinned to the same instant for parity. The urgency scorer reads
# only created_at (for the `age` component) and not updated_at, so age is the
# sole urgency component this pin controls. The harness already normalizes all
# timestamps to <TS>, so the displayed created/updated lines stay <TS>
# regardless of the pinned value.
#
# Depends on python3 + the stdlib sqlite3 module, which the integration suite
# already relies on. Safe to call from a seed function with the DB path as $1.
# ──────────────────────────────────────────────────────────────────────────
contract_pin_created_at() {
    local db="$1" id="$2" iso="${3:-2000-01-01T00:00:00Z}"
    python3 -c '
import sqlite3, sys
db, issue_id, iso = sys.argv[1], int(sys.argv[2]), sys.argv[3]
conn = sqlite3.connect(db)
conn.execute(
    "UPDATE issues SET created_at = ?, updated_at = ? WHERE id = ?",
    (iso, iso, issue_id),
)
conn.commit()
conn.close()
' "$db" "$id" "$iso"
}

# ──────────────────────────────────────────────────────────────────────────
# _contract_run_capture: internal. Runs the itr command for a case in an
# isolated temp DB, normalizes, and stores results in globals:
#   CONTRACT_STDOUT  CONTRACT_STDERR  CONTRACT_EXIT  CONTRACT_NORMALIZED
#
# Usage: _contract_run_capture <seed_fn-or-empty> <stdin-data> -- <itr args...>
# ──────────────────────────────────────────────────────────────────────────
_contract_run_capture() {
    local seed_fn="$1"; shift
    local stdin_data="$1"; shift
    # Expect a literal `--` separator before the itr args.
    if [ "${1:-}" = "--" ]; then shift; fi
    local args=("$@")

    local tmpdir
    tmpdir="$(mktemp -d)"
    local db="$tmpdir/.itr.db"

    # Every case starts from a fresh, initialized DB. Commands like get/list
    # need a DB to read; init-on-init is idempotent for `init` cases.
    ITR_DB_PATH="$db" "$ITR" init -q >/dev/null 2>&1 || true

    if [ -n "$seed_fn" ]; then
        "$seed_fn" "$db" >/dev/null 2>&1 || true
    fi

    local out_file="$tmpdir/.stdout"
    local err_file="$tmpdir/.stderr"
    set +e
    if [ -n "$stdin_data" ]; then
        printf '%s' "$stdin_data" | ITR_DB_PATH="$db" "$ITR" "${args[@]}" >"$out_file" 2>"$err_file"
    else
        ITR_DB_PATH="$db" "$ITR" "${args[@]}" >"$out_file" 2>"$err_file"
    fi
    CONTRACT_EXIT=$?
    set -e

    CONTRACT_STDOUT="$(contract_normalize "$tmpdir" <"$out_file")"
    CONTRACT_STDERR="$(contract_normalize "$tmpdir" <"$err_file")"

    # Build the canonical normalized snapshot body. Stable, labeled sections so
    # a diff pinpoints exactly which channel drifted.
    CONTRACT_NORMALIZED="$(cat <<EOF
\$ itr ${args[*]}
--- exit ---
$CONTRACT_EXIT
--- stdout ---
$CONTRACT_STDOUT
--- stderr ---
$CONTRACT_STDERR
EOF
)"

    rm -rf "$tmpdir"
}

# ──────────────────────────────────────────────────────────────────────────
# _contract_assert: internal. Compares CONTRACT_NORMALIZED against the expected
# snapshot file, or (re)writes it under UPDATE_SNAPSHOTS=1.
# ──────────────────────────────────────────────────────────────────────────
_contract_assert() {
    local area="$1" case="$2"; shift 2
    local args_desc="$*"
    local snap_file="$CONTRACTS_SNAPSHOT_DIR/$area/$case.txt"

    if [ "${UPDATE_SNAPSHOTS:-0}" = "1" ]; then
        mkdir -p "$(dirname "$snap_file")"
        printf '%s\n' "$CONTRACT_NORMALIZED" >"$snap_file"
        pass "snapshot $area/$case (updated baseline)"
        return 0
    fi

    if [ ! -f "$snap_file" ]; then
        fail "snapshot $area/$case" \
            "missing expected snapshot $snap_file — run UPDATE_SNAPSHOTS=1 to create it"
        return 1
    fi

    local actual_file
    actual_file="$(mktemp)"
    printf '%s\n' "$CONTRACT_NORMALIZED" >"$actual_file"

    if diff -u "$snap_file" "$actual_file" >/dev/null 2>&1; then
        pass "snapshot $area/$case"
        rm -f "$actual_file"
        return 0
    fi

    # Mismatch: emit a labeled unified diff so the failure identifies command,
    # args, stdout, stderr, and exit status (all carried inside the snapshot
    # body), then point reviewers at the regen command.
    echo ""
    echo "    ── snapshot drift: $area/$case ──────────────────────────"
    echo "    command: itr $args_desc"
    echo "    exit:    $CONTRACT_EXIT"
    echo "    diff (expected vs actual, unified):"
    diff -u \
        --label "expected: tests/snapshots/$area/$case.txt" \
        --label "actual:   itr $args_desc" \
        "$snap_file" "$actual_file" | sed 's/^/    /'
    echo "    regen:   UPDATE_SNAPSHOTS=1 ./tests/integration.sh"
    echo "    ─────────────────────────────────────────────────────────"
    echo ""

    fail "snapshot $area/$case" "normalized output differs from expected snapshot"
    rm -f "$actual_file"
    return 1
}

# ──────────────────────────────────────────────────────────────────────────
# PUBLIC: snapshot <area> <case> [stdin-data] -- <itr args...>
#
# Runs an itr command in an isolated, freshly-initialized temp DB and asserts
# its normalized output against tests/snapshots/<area>/<case>.txt.
#
# Examples:
#   snapshot example version -- --version
#   snapshot example empty_list -- list -f json
#   snapshot example batch_add '[{"title":"A"}]' -- batch add -f json
# ──────────────────────────────────────────────────────────────────────────
snapshot() {
    local area="$1" case="$2"; shift 2
    local stdin_data=""
    if [ "${1:-}" != "--" ]; then
        stdin_data="$1"; shift
    fi
    # Now $1 should be `--`.
    _contract_run_capture "" "$stdin_data" "$@"
    # Strip the leading `--` for the human-facing args description.
    local desc_args=("$@")
    if [ "${desc_args[0]:-}" = "--" ]; then
        desc_args=("${desc_args[@]:1}")
    fi
    _contract_assert "$area" "$case" "${desc_args[@]}"
}

# ──────────────────────────────────────────────────────────────────────────
# PUBLIC: snapshot_seeded <area> <case> <seed_fn> [stdin-data] -- <itr args...>
#
# Like `snapshot`, but invokes `<seed_fn> <db_path>` to populate the isolated
# DB with fixtures (issues, deps, notes…) before running the asserted command.
# The seed function receives the DB path; use ITR_DB_PATH="$1" "$ITR" ... to
# write to it. Keep seeds deterministic so snapshots stay stable.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded() {
    local area="$1" case="$2" seed_fn="$3"; shift 3
    local stdin_data=""
    if [ "${1:-}" != "--" ]; then
        stdin_data="$1"; shift
    fi
    _contract_run_capture "$seed_fn" "$stdin_data" "$@"
    local desc_args=("$@")
    if [ "${desc_args[0]:-}" = "--" ]; then
        desc_args=("${desc_args[@]:1}")
    fi
    _contract_assert "$area" "$case" "${desc_args[@]}"
}
