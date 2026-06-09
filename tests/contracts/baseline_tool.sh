#!/usr/bin/env bash
# tests/contracts/baseline_tool.sh
#
# Auto-discovered smoke test for the historical baseline output-diff developer
# tool (tests/tools/baseline-diff.sh, issue #145).
#
# This file is sourced by tests/integration.sh (it loops over
# tests/contracts/*.sh, excluding _lib.sh) so the verify gate runs it. It
# sources _lib.sh ONLY for the pass/fail counters and the same `$ITR` binary the
# suite resolves; it does NOT register any snapshot cases.
#
# It deliberately does NOT do a full cross-ref cargo build (that is slow and not
# what we are testing here). Instead it exercises the TOOL'S control flow using
# the resolved `$ITR` binary on BOTH sides via the explicit-binary flags:
#   - argument validation (missing --baseline -> exit 2),
#   - the dirty-working-tree guard (refuses with exit 3),
#   - the dirty-guard override (--allow-dirty) producing a real report,
#   - report structure (header, summary table, SAME rows, no changed commands
#     when both binaries are identical),
#   - a genuine DIFF detection path using a tiny stub baseline binary whose
#     --version differs, proving the report lists changed commands, a changed
#     exit status, and a normalized unified diff.
#
# Local iteration:
#   ITR=./target/release/itr bash tests/contracts/baseline_tool.sh

CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tests/contracts/_lib.sh
. "$CONTRACT_DIR/_lib.sh"

TOOL="$CONTRACTS_REPO_ROOT/tests/tools/baseline-diff.sh"

echo ""
echo "--- contract: baseline_tool (historical diff dev tool smoke) ---"

# Helper: run the tool, capture stdout+stderr and exit code without tripping
# the caller's set -e.
bt_run() {
    BT_OUT=""
    BT_EXIT=0
    set +e
    BT_OUT="$(bash "$TOOL" "$@" 2>&1)"
    BT_EXIT=$?
    set -e
}

# ── 0) The tool exists and is executable ─────────────────────────────────────
if [ -x "$TOOL" ]; then
    pass "baseline-diff tool exists and is executable"
else
    fail "baseline-diff tool exists and is executable" \
        "missing or non-executable: $TOOL"
fi

# ── 1) --help exits 0 and documents the tool ─────────────────────────────────
bt_run --help
if [ "$BT_EXIT" -eq 0 ] && printf '%s' "$BT_OUT" | grep -qF "USAGE"; then
    pass "baseline-diff --help exits 0 with usage"
else
    fail "baseline-diff --help exits 0 with usage" \
        "exit=$BT_EXIT out=$BT_OUT"
fi

# ── 2) Missing --baseline is an argument error (exit 2) ───────────────────────
bt_run
if [ "$BT_EXIT" -eq 2 ] && printf '%s' "$BT_OUT" | grep -qF "baseline"; then
    pass "baseline-diff missing --baseline exits 2"
else
    fail "baseline-diff missing --baseline exits 2" \
        "exit=$BT_EXIT out=$BT_OUT"
fi

# ── 3) Unknown argument is an argument error (exit 2) ─────────────────────────
bt_run --baseline HEAD --no-such-flag
if [ "$BT_EXIT" -eq 2 ]; then
    pass "baseline-diff unknown flag exits 2"
else
    fail "baseline-diff unknown flag exits 2" "exit=$BT_EXIT out=$BT_OUT"
fi

# ── 4) Dirty-tree guard: create a guaranteed-dirty scratch repo and assert the
#       tool REFUSES (exit 3) without --allow-dirty. We run the tool from a
#       throwaway clone so the assertion does not depend on the state of the
#       real working tree (which may be clean or dirty). ───────────────────────
SCRATCH="$(mktemp -d)"
(
    cd "$SCRATCH"
    git init -q
    git config user.email t@t.t
    git config user.name t
    # Mirror the tool into the scratch repo so REPO_ROOT resolves here.
    mkdir -p tests/tools tests/contracts
    cp "$TOOL" tests/tools/baseline-diff.sh
    cp "$CONTRACTS_REPO_ROOT/tests/contracts/_lib.sh" tests/contracts/_lib.sh
    echo "seed" >seed.txt
    git add -A >/dev/null 2>&1
    git commit -qm init >/dev/null 2>&1
    # Now dirty the tree.
    echo "dirty" >>seed.txt
)
set +e
DIRTY_OUT="$(bash "$SCRATCH/tests/tools/baseline-diff.sh" --baseline HEAD 2>&1)"
DIRTY_EXIT=$?
set -e
if [ "$DIRTY_EXIT" -eq 3 ] && printf '%s' "$DIRTY_OUT" | grep -qiF "dirty"; then
    pass "baseline-diff refuses dirty working tree (exit 3)"
else
    fail "baseline-diff refuses dirty working tree (exit 3)" \
        "exit=$DIRTY_EXIT out=$DIRTY_OUT"
fi

# ── 5) --allow-dirty override: still refuses? No — it must PROCEED. Using the
#       same resolved $ITR on both sides, the report must be produced and report
#       NO changed commands (identical binaries). This exercises the happy path
#       and report structure WITHOUT a cross-ref build. ───────────────────────
SAME_REPORT="$(mktemp)"
bt_run --baseline HEAD --allow-dirty \
    --baseline-binary "$ITR" --target-binary "$ITR" --out "$SAME_REPORT"
if [ "$BT_EXIT" -eq 0 ] && [ -s "$SAME_REPORT" ]; then
    pass "baseline-diff happy path with --allow-dirty produces a report"
else
    fail "baseline-diff happy path with --allow-dirty produces a report" \
        "exit=$BT_EXIT report-size=$(wc -c <"$SAME_REPORT" 2>/dev/null)"
fi

# Report must contain the header + summary table + the SAME verdict for an
# identical-binary comparison.
if grep -qF "# itr baseline output diff" "$SAME_REPORT" \
    && grep -qF "## Summary" "$SAME_REPORT" \
    && grep -qF "COMMAND" "$SAME_REPORT" \
    && grep -qF "## No changed commands." "$SAME_REPORT"; then
    pass "identical-binary report has header, summary, and no-changes verdict"
else
    fail "identical-binary report has header, summary, and no-changes verdict" \
        "report did not contain all expected structural markers"
fi

# Every matrix command should be marked SAME for identical binaries.
if grep -qF " SAME " "$SAME_REPORT" && ! grep -qF " DIFF " "$SAME_REPORT"; then
    pass "identical-binary report marks every command SAME"
else
    fail "identical-binary report marks every command SAME" \
        "found a DIFF row (or no SAME rows) for identical binaries"
fi

# ── 6) Genuine DIFF detection: build a tiny stub "baseline" binary whose
#       behavior diverges from $ITR for two matrix commands:
#         - `--version` prints a different version  (changed stdout)
#         - `get 999 -f json` exits 0 instead of 1  (changed EXIT status)
#       The report must then list changed commands, a changed exit status, and a
#       normalized unified diff. This proves the diff machinery really detects
#       and renders divergence (not just the no-op path). ─────────────────────
STUB_DIR="$(mktemp -d)"
STUB="$STUB_DIR/itr"
cat >"$STUB" <<STUBEOF
#!/usr/bin/env bash
# Minimal itr stand-in for the baseline side of the diff smoke test.
# It shadows just enough of the matrix to force a stdout diff and an exit diff,
# and otherwise delegates to the real binary so other commands stay comparable.
REAL="$ITR"
case "\$1" in
    --version)
        echo "itr 0.0.1-stub"   # different version -> stdout DIFF
        exit 0
        ;;
esac
# get 999 should be a miss (exit 1) on the real tool; here force exit 0 to make
# the EXIT status diverge for that matrix command.
if [ "\$1" = "get" ] && [ "\$2" = "999" ]; then
    echo "{}"
    exit 0
fi
exec "\$REAL" "\$@"
STUBEOF
chmod +x "$STUB"

DIFF_REPORT="$(mktemp)"
bt_run --baseline HEAD --allow-dirty \
    --baseline-binary "$STUB" --target-binary "$ITR" --out "$DIFF_REPORT"

if [ "$BT_EXIT" -eq 0 ] && grep -qF "## Changed commands" "$DIFF_REPORT"; then
    pass "stub-vs-real report lists changed commands"
else
    fail "stub-vs-real report lists changed commands" \
        "exit=$BT_EXIT (expected a '## Changed commands' section)"
fi

# The version command's stdout differs -> a DIFF row and a unified diff hunk.
if grep -qE "^version +DIFF" "$DIFF_REPORT" \
    && grep -qF '```diff' "$DIFF_REPORT" \
    && grep -qF "0.0.1-stub" "$DIFF_REPORT"; then
    pass "stub-vs-real report shows a normalized unified diff for version"
else
    fail "stub-vs-real report shows a normalized unified diff for version" \
        "missing DIFF row / diff fence / changed bytes for version"
fi

# The get_missing command's EXIT status differs -> a *CHANGED* exit marker.
if grep -qF "*CHANGED*" "$DIFF_REPORT"; then
    pass "stub-vs-real report flags a changed exit status"
else
    fail "stub-vs-real report flags a changed exit status" \
        "no '*CHANGED*' exit-status marker found"
fi

# ── 7) Cleanup. The tool must NOT have left a worktree behind in the real repo. ─
LEFTOVER="$(git -C "$CONTRACTS_REPO_ROOT" worktree list 2>/dev/null | grep -cF "$(dirname "$SAME_REPORT")" || true)"
if [ "${LEFTOVER:-0}" -eq 0 ]; then
    pass "baseline-diff left no stray git worktree"
else
    fail "baseline-diff left no stray git worktree" "found $LEFTOVER leftover(s)"
fi

rm -rf "$SCRATCH" "$STUB_DIR" "$SAME_REPORT" "$DIFF_REPORT"
