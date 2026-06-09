#!/usr/bin/env bash
# tests/tools/baseline-diff.sh
#
# Repeatable origin/baseline output-diff developer tool (issue #145).
#
# WHAT THIS IS
#   A *developer* tool — NOT part of the default verify gate — that compares the
#   CLI output matrix of a historical git ref (e.g. origin/main, a release tag,
#   or db7e324) against a current target (the working tree's built binary, or an
#   explicit prebuilt binary). It builds the baseline ref in an ISOLATED git
#   worktree so the user's working tree is never mutated, runs the SAME command
#   matrix against both binaries, normalizes runtime entropy, and emits a report
#   listing changed commands, changed exit statuses, and normalized unified
#   diffs.
#
#   Use this when you deliberately change the CLI output standard and want to
#   review the full historical delta against a released/remote ref. For the
#   day-to-day "did output drift from its checked-in baseline" gate, use the
#   normalized snapshot harness (tests/contracts/*.sh, run by
#   tests/integration.sh). See docs/testing.md and docs/command-contracts.md for
#   when to reach for which.
#
# RELATIONSHIP TO THE SNAPSHOT HARNESS
#   This tool deliberately REUSES the normalizer from tests/contracts/_lib.sh
#   (the `contract_normalize` function) so the entropy stripping is identical to
#   the snapshot gate. It sources _lib.sh in a guarded way that does NOT register
#   or run any snapshot cases — it only borrows `contract_normalize`. If sourcing
#   ever fails, it falls back to an inline copy of the same sed program.
#
# SAFETY
#   - Refuses to run on a dirty working tree (guarded by `git status
#     --porcelain`) unless --allow-dirty is passed. The dirty guard protects the
#     "current worktree" target: a dirty tree means an ambiguous "current".
#   - The baseline ref is checked out into a detached `git worktree` under a
#     temp dir, built there, and the worktree is removed on exit. The user's
#     working tree, index, and HEAD are never touched.
#   - All temp dirs are mktemp -d and cleaned in an EXIT trap.
#
# SCOPING / SPEED
#   Building an old ref is a full cargo release build and can be slow. This tool
#   is intentionally NOT in the verify gate for that reason. Its smoke test
#   (tests/contracts/baseline_tool.sh) exercises control-flow (the dirty guard,
#   argument validation, and report structure) using fast/trivial inputs and the
#   --skip-baseline-build / explicit-binary paths, NOT a full cross-ref build.
#
# USAGE
#   tests/tools/baseline-diff.sh --baseline <ref> [options]
#
# OPTIONS
#   --baseline <ref>        REQUIRED. Git ref to use as the baseline
#                           (origin/main, a tag, a commit SHA, …).
#   --target-binary <path>  Use this prebuilt binary as the "current" target
#                           instead of building the working tree. Skips the
#                           current-side build.
#   --baseline-binary <path>
#                           Use this prebuilt binary as the baseline instead of
#                           building <ref>. Skips the isolated worktree build
#                           (useful for fast tests / when you already have it).
#   --out <path>            Write the report to this file (default: stdout).
#   --allow-dirty           Skip the dirty-working-tree guard. Use only when you
#                           understand "current" is ambiguous.
#   --skip-baseline-build   Do not build the baseline; requires
#                           --baseline-binary. (Self-documenting alias of just
#                           passing --baseline-binary.)
#   --keep-temp             Do not delete temp dirs / worktree on exit (debug).
#   -h, --help              Print this help and exit 0.
#
# EXIT CODES
#   0  ran successfully; report written (whether or not differences were found —
#      differences are data, not a failure of the tool).
#   2  usage / argument error (missing --baseline, bad flag, missing binary).
#   3  refused: dirty working tree without --allow-dirty.
#   4  environment error (not a git repo, ref not found, build failed).
#
# OUTPUT (report)
#   A plain-text report with three sections:
#     1. a header (baseline ref, baseline/target binary identities),
#     2. a per-command summary table marking each command SAME / DIFF and
#        whether its EXIT status changed,
#     3. for every changed command, a normalized unified diff (diff -u) of the
#        captured stdout+exit+stderr block, plus an exit-status delta line.

set -uo pipefail

PROG="$(basename "$0")"
TOOL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$TOOL_DIR/../.." && pwd)"

# ──────────────────────────────────────────────────────────────────────────
# usage
# ──────────────────────────────────────────────────────────────────────────
usage() {
    sed -n '2,/^set -uo pipefail$/p' "${BASH_SOURCE[0]}" | sed -e 's/^# \{0,1\}//' -e '/^set -uo pipefail$/d'
}

die() {
    # die <exit-code> <message...>
    local code="$1"; shift
    echo "ERROR: $*" >&2
    exit "$code"
}

# ──────────────────────────────────────────────────────────────────────────
# Borrow the snapshot harness normalizer so entropy stripping matches the gate.
# Sourcing _lib.sh defines contract_normalize WITHOUT running any cases (the
# library only *defines* helpers; cases live in the per-area contract files).
# Guard with a fallback inline copy in case _lib.sh is missing or changes shape.
# ──────────────────────────────────────────────────────────────────────────
LIB="$REPO_ROOT/tests/contracts/_lib.sh"
if [ -f "$LIB" ]; then
    # shellcheck source=tests/contracts/_lib.sh
    # `ITR` may be unset here; _lib.sh tolerates that and only resolves a default
    # binary path, which we never use from this tool.
    . "$LIB" >/dev/null 2>&1 || true
fi
if ! declare -F contract_normalize >/dev/null 2>&1; then
    # Fallback: identical sed program to _lib.sh::contract_normalize.
    contract_normalize() {
        local case_tmp="${1:-__no_such_tmp__}"
        sed -E \
            -e "s#${case_tmp}#<TMP>#g" \
            -e 's/[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}(\.[0-9]+)?Z/<TS>/g' \
            -e 's#/var/folders/[A-Za-z0-9_./+-]*#<TMP>#g' \
            -e 's#/tmp/tmp\.[A-Za-z0-9]+#<TMP>#g' \
            -e 's#/tmp/[A-Za-z0-9_.-]*tmp[A-Za-z0-9_.-]*#<TMP>#g' \
            -e 's#(127\.0\.0\.1):[0-9]+#\1:<PORT>#g' \
            -e 's#(localhost):[0-9]+#\1:<PORT>#g' \
            -e 's#(token=)[A-Za-z0-9._-]+#\1<TOKEN>#g' \
            -e 's#(X-ITR-Token: )[A-Za-z0-9._-]+#\1<TOKEN>#g' \
            -e 's#(itr )v?[0-9]+\.[0-9]+\.[0-9]+(-[0-9]+-g[0-9a-f]+)?(\+[0-9a-f]+)?(-dirty)?#\1X.Y.Z#g'
    }
fi

# ──────────────────────────────────────────────────────────────────────────
# The command matrix. Each entry is a label + the itr argv (after `itr`).
# Kept to deterministic, DB-free or freshly-seeded commands so the comparison
# is meaningful. Commands run against an isolated, freshly-initialized temp DB.
# Stdin (for batch) is supplied via the MATRIX_STDIN map keyed by label.
# ──────────────────────────────────────────────────────────────────────────
MATRIX_LABELS=(
    "version"
    "help"
    "schema"
    "agent_info"
    "empty_list_json"
    "empty_list_compact"
    "ready_empty"
    "get_missing"
    "stats_json"
    "graph_json"
    "add_then_get"
    "batch_add_json"
)

# Returns the argv (space-joined, eval-safe-ish for our fixed strings) for a
# label. We keep args simple (no embedded spaces except the quoted title) so a
# read -ra split is sufficient; the title token uses a sentinel underscore.
matrix_args() {
    case "$1" in
        version)             echo "--version" ;;
        help)                echo "--help" ;;
        schema)              echo "schema" ;;
        agent_info)          echo "agent-info" ;;
        empty_list_json)     echo "list -f json" ;;
        empty_list_compact)  echo "list" ;;
        ready_empty)         echo "ready -f json" ;;
        get_missing)         echo "get 999 -f json" ;;
        stats_json)          echo "stats -f json" ;;
        graph_json)          echo "graph -f json" ;;
        add_then_get)        echo "get 1 -f json" ;;   # seeded below
        batch_add_json)      echo "batch add -f json" ;;
        *)                   echo "" ;;
    esac
}

# Per-label seed: populate the temp DB before the asserted command runs.
matrix_seed() {
    # $1 label, $2 binary, $3 db-path
    local label="$1" bin="$2" db="$3"
    case "$label" in
        add_then_get)
            ITR_DB_PATH="$db" "$bin" add "Baseline fixture" -p high -k bug \
                -c "context" -a "accept" >/dev/null 2>&1
            ;;
        *) : ;;
    esac
}

# Per-label stdin payload (for batch). Empty for everything else.
matrix_stdin() {
    case "$1" in
        batch_add_json) printf '%s' '[{"title":"Batch A"},{"title":"Batch B"}]' ;;
        *) printf '' ;;
    esac
}

# ──────────────────────────────────────────────────────────────────────────
# run_matrix <binary> <out-dir>
#   Runs every matrix label against <binary> in its own freshly-init'd temp DB
#   and writes a normalized capture block to <out-dir>/<label>.txt. The capture
#   block mirrors the snapshot harness format so diffs read the same way.
# ──────────────────────────────────────────────────────────────────────────
run_matrix() {
    local bin="$1" outdir="$2"
    [ -x "$bin" ] || die 4 "binary not executable: $bin"
    mkdir -p "$outdir"

    local label argv_str
    for label in "${MATRIX_LABELS[@]}"; do
        argv_str="$(matrix_args "$label")"
        local -a argv
        read -ra argv <<<"$argv_str"

        local casedir db
        casedir="$(mktemp -d)"
        db="$casedir/.itr.db"
        # Init unless this is a no-DB command. Init-on-no-DB-command is harmless.
        ITR_DB_PATH="$db" "$bin" init -q >/dev/null 2>&1 || true
        matrix_seed "$label" "$bin" "$db"

        local out_file="$casedir/.stdout" err_file="$casedir/.stderr"
        local stdin_data exit_code
        stdin_data="$(matrix_stdin "$label")"
        if [ -n "$stdin_data" ]; then
            printf '%s' "$stdin_data" | ITR_DB_PATH="$db" "$bin" "${argv[@]}" \
                >"$out_file" 2>"$err_file"
        else
            ITR_DB_PATH="$db" "$bin" "${argv[@]}" >"$out_file" 2>"$err_file"
        fi
        exit_code=$?

        local norm_out norm_err
        norm_out="$(contract_normalize "$casedir" <"$out_file")"
        norm_err="$(contract_normalize "$casedir" <"$err_file")"

        {
            echo "\$ itr $argv_str"
            echo "--- exit ---"
            echo "$exit_code"
            echo "--- stdout ---"
            echo "$norm_out"
            echo "--- stderr ---"
            echo "$norm_err"
        } >"$outdir/$label.txt"

        # Record exit code separately for the summary table.
        echo "$exit_code" >"$outdir/$label.exit"

        rm -rf "$casedir"
    done
}

# ──────────────────────────────────────────────────────────────────────────
# build_baseline <ref> <build-root> -> echoes built binary path
#   Adds a detached worktree at <build-root>/wt pinned to <ref>, runs a release
#   build there with an isolated CARGO_TARGET_DIR, and echoes the binary path.
#   Never touches the user's working tree. The caller is responsible for
#   removing the worktree (we do it in the EXIT trap via WORKTREE_PATH).
# ──────────────────────────────────────────────────────────────────────────
WORKTREE_PATH=""
build_baseline() {
    local ref="$1" build_root="$2"
    git -C "$REPO_ROOT" rev-parse --verify "$ref^{commit}" >/dev/null 2>&1 \
        || die 4 "baseline ref not found: $ref"

    local wt="$build_root/wt"
    # Detached worktree so we never move the user's branch/HEAD.
    git -C "$REPO_ROOT" worktree add --detach --force "$wt" "$ref" >/dev/null 2>&1 \
        || die 4 "failed to create git worktree for $ref"
    WORKTREE_PATH="$wt"

    local tgt="$build_root/target"
    ( cd "$wt" && CARGO_TARGET_DIR="$tgt" cargo build --release >&2 ) \
        || die 4 "baseline build failed for $ref"

    local bin="$tgt/release/itr"
    [ -x "$bin" ] || die 4 "baseline binary missing after build: $bin"
    echo "$bin"
}

# ──────────────────────────────────────────────────────────────────────────
# build_current -> echoes current working-tree binary path
#   Builds the working tree's release binary (isolated target dir is NOT needed;
#   this is the user's own tree and a normal cargo build is expected). Reuses an
#   existing target/release/itr if present and newer is not required.
# ──────────────────────────────────────────────────────────────────────────
build_current() {
    local bin="$REPO_ROOT/target/release/itr"
    ( cd "$REPO_ROOT" && cargo build --release >&2 ) \
        || die 4 "current working-tree build failed"
    [ -x "$bin" ] || die 4 "current binary missing after build: $bin"
    echo "$bin"
}

# ──────────────────────────────────────────────────────────────────────────
# emit_report <baseline-dir> <target-dir> <baseline-id> <target-id> <ref>
#   Writes the report to stdout (caller redirects to --out if requested).
# ──────────────────────────────────────────────────────────────────────────
emit_report() {
    local bdir="$1" tdir="$2" bid="$3" tid="$4" ref="$5"

    echo "# itr baseline output diff"
    echo "#"
    echo "# baseline ref:    $ref"
    echo "# baseline binary: $bid"
    echo "# target binary:   $tid"
    echo "# generated:       (timestamps normalized in captures below)"
    echo ""

    # Summary table.
    echo "## Summary"
    echo ""
    printf '%-22s %-6s %-18s\n' "COMMAND" "RESULT" "EXIT"
    printf '%-22s %-6s %-18s\n' "----------------------" "------" "------------------"

    local changed_labels=()
    local label
    for label in "${MATRIX_LABELS[@]}"; do
        local bfile="$bdir/$label.txt" tfile="$tdir/$label.txt"
        local bexit texit result exitcol
        bexit="$(cat "$bdir/$label.exit" 2>/dev/null || echo "?")"
        texit="$(cat "$tdir/$label.exit" 2>/dev/null || echo "?")"

        if diff -q "$bfile" "$tfile" >/dev/null 2>&1; then
            result="SAME"
        else
            result="DIFF"
            changed_labels+=("$label")
        fi
        if [ "$bexit" = "$texit" ]; then
            exitcol="$bexit"
        else
            exitcol="$bexit -> $texit  *CHANGED*"
        fi
        printf '%-22s %-6s %-18s\n' "$label" "$result" "$exitcol"
    done
    echo ""

    if [ "${#changed_labels[@]}" -eq 0 ]; then
        echo "## No changed commands."
        echo ""
        echo "Baseline and target produced identical normalized output and exit"
        echo "status for every command in the matrix."
        return 0
    fi

    echo "## Changed commands (${#changed_labels[@]})"
    echo ""
    for label in "${changed_labels[@]}"; do
        local bexit texit
        bexit="$(cat "$bdir/$label.exit" 2>/dev/null || echo "?")"
        texit="$(cat "$tdir/$label.exit" 2>/dev/null || echo "?")"
        echo "### $label"
        if [ "$bexit" != "$texit" ]; then
            echo "exit status: $bexit (baseline) -> $texit (target)  *CHANGED*"
        else
            echo "exit status: $bexit (unchanged)"
        fi
        echo ""
        echo '```diff'
        diff -u \
            --label "baseline: $label" \
            --label "target:   $label" \
            "$bdir/$label.txt" "$tdir/$label.txt"
        echo '```'
        echo ""
    done
}

# ──────────────────────────────────────────────────────────────────────────
# Argument parsing.
# ──────────────────────────────────────────────────────────────────────────
BASELINE_REF=""
TARGET_BINARY=""
BASELINE_BINARY=""
OUT_PATH=""
ALLOW_DIRTY=0
SKIP_BASELINE_BUILD=0
KEEP_TEMP=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        --baseline)            BASELINE_REF="${2:-}"; shift 2 || die 2 "--baseline needs a value" ;;
        --target-binary)       TARGET_BINARY="${2:-}"; shift 2 || die 2 "--target-binary needs a value" ;;
        --baseline-binary)     BASELINE_BINARY="${2:-}"; shift 2 || die 2 "--baseline-binary needs a value" ;;
        --out)                 OUT_PATH="${2:-}"; shift 2 || die 2 "--out needs a value" ;;
        --allow-dirty)         ALLOW_DIRTY=1; shift ;;
        --skip-baseline-build) SKIP_BASELINE_BUILD=1; shift ;;
        --keep-temp)           KEEP_TEMP=1; shift ;;
        -h|--help)             usage; exit 0 ;;
        *)                     die 2 "unknown argument: $1 (try --help)" ;;
    esac
done

# Validate we are in a git repo.
git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1 \
    || die 4 "not a git repository: $REPO_ROOT"

[ -n "$BASELINE_REF" ] || die 2 "--baseline <ref> is required (try --help)"

if [ "$SKIP_BASELINE_BUILD" -eq 1 ] && [ -z "$BASELINE_BINARY" ]; then
    die 2 "--skip-baseline-build requires --baseline-binary <path>"
fi

# Dirty-tree guard. The "current" target is ambiguous when the tree is dirty.
# When an explicit --target-binary is given we still guard, because the report
# header documents the working tree as the comparison context; --allow-dirty is
# the explicit escape hatch.
if [ "$ALLOW_DIRTY" -ne 1 ]; then
    if [ -n "$(git -C "$REPO_ROOT" status --porcelain 2>/dev/null)" ]; then
        echo "ERROR: working tree is dirty; refusing to run." >&2
        echo "       The 'current' target is ambiguous with uncommitted changes." >&2
        echo "       Commit/stash, or pass --allow-dirty to override (or use" >&2
        echo "       --target-binary with --allow-dirty to compare an explicit binary)." >&2
        exit 3
    fi
fi

# ──────────────────────────────────────────────────────────────────────────
# Temp workspace + cleanup.
# ──────────────────────────────────────────────────────────────────────────
WORK="$(mktemp -d)"
cleanup() {
    if [ "$KEEP_TEMP" -eq 1 ]; then
        echo "REVIEW: --keep-temp set; left workspace at $WORK and worktree at ${WORKTREE_PATH:-<none>}" >&2
        return
    fi
    if [ -n "$WORKTREE_PATH" ] && [ -d "$WORKTREE_PATH" ]; then
        git -C "$REPO_ROOT" worktree remove --force "$WORKTREE_PATH" >/dev/null 2>&1 \
            || rm -rf "$WORKTREE_PATH"
        git -C "$REPO_ROOT" worktree prune >/dev/null 2>&1 || true
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

# ──────────────────────────────────────────────────────────────────────────
# Resolve the two binaries.
# ──────────────────────────────────────────────────────────────────────────
BASELINE_BIN=""
TARGET_BIN=""

if [ -n "$BASELINE_BINARY" ]; then
    [ -x "$BASELINE_BINARY" ] || die 2 "baseline binary not executable: $BASELINE_BINARY"
    BASELINE_BIN="$BASELINE_BINARY"
    echo "REVIEW: using prebuilt baseline binary, skipping worktree build" >&2
else
    echo "INFO: building baseline ref '$BASELINE_REF' in isolated worktree..." >&2
    BASELINE_BIN="$(build_baseline "$BASELINE_REF" "$WORK/baseline")"
fi

if [ -n "$TARGET_BINARY" ]; then
    [ -x "$TARGET_BINARY" ] || die 2 "target binary not executable: $TARGET_BINARY"
    TARGET_BIN="$TARGET_BINARY"
    echo "REVIEW: using prebuilt target binary, skipping current build" >&2
else
    echo "INFO: building current working tree (release)..." >&2
    TARGET_BIN="$(build_current)"
fi

# Identity strings for the report header.
BASELINE_ID="$("$BASELINE_BIN" --version 2>/dev/null | head -1) [$BASELINE_BIN]"
TARGET_ID="$("$TARGET_BIN" --version 2>/dev/null | head -1) [$TARGET_BIN]"

# ──────────────────────────────────────────────────────────────────────────
# Run the matrix against both and emit the report.
# ──────────────────────────────────────────────────────────────────────────
BDIR="$WORK/baseline-out"
TDIR="$WORK/target-out"
echo "INFO: running command matrix against baseline..." >&2
run_matrix "$BASELINE_BIN" "$BDIR"
echo "INFO: running command matrix against target..." >&2
run_matrix "$TARGET_BIN" "$TDIR"

if [ -n "$OUT_PATH" ]; then
    emit_report "$BDIR" "$TDIR" "$BASELINE_ID" "$TARGET_ID" "$BASELINE_REF" >"$OUT_PATH"
    echo "INFO: report written to $OUT_PATH" >&2
else
    emit_report "$BDIR" "$TDIR" "$BASELINE_ID" "$TARGET_ID" "$BASELINE_REF"
fi

exit 0
