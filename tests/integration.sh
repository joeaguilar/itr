#!/usr/bin/env bash
set -euo pipefail

# Integration test suite for nit
# Usage: ./tests/integration.sh [path-to-nit-binary]
#
# If no path is provided, uses ./target/release/nit

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
NIT="${1:-$SCRIPT_DIR/target/release/nit}"

if [ ! -x "$NIT" ]; then
    echo "Binary not found at $NIT — run 'cargo build --release' first"
    exit 1
fi

PASS=0
FAIL=0
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

assert_eq() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        pass "$desc"
    else
        fail "$desc" "expected '$expected', got '$actual'"
    fi
}

assert_contains() {
    local desc="$1" needle="$2" haystack="$3"
    if echo "$haystack" | grep -qF -- "$needle"; then
        pass "$desc"
    else
        fail "$desc" "output does not contain '$needle'"
    fi
}

assert_exit() {
    local desc="$1" expected="$2"
    shift 2
    set +e
    "$@" >/dev/null 2>&1
    local actual=$?
    set -e
    assert_eq "$desc" "$expected" "$actual"
}

jq_val() {
    echo "$1" | python3 -c "import sys,json; d=json.load(sys.stdin); print($2)"
}

# ─────────────────────────────────────────────
# Setup
# ─────────────────────────────────────────────
WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT
cd "$WORKDIR"

echo "nit integration tests"
echo "Binary: $NIT"
echo "Workdir: $WORKDIR"
echo ""

# ─────────────────────────────────────────────
echo "--- init ---"
# ─────────────────────────────────────────────

OUT=$($NIT init)
assert_contains "init creates db" ".nit.db" "$OUT"
[ -f .nit.db ] && pass "init .nit.db file exists" || fail "init .nit.db file exists" "file missing"

OUT=$($NIT init)
assert_contains "init is idempotent" ".nit.db" "$OUT"

OUT=$($NIT init -f json)
CREATED=$(jq_val "$OUT" "d['created']")
assert_eq "init json reports created=False on re-init" "False" "$CREATED"

# ─────────────────────────────────────────────
echo "--- init --agents-md ---"
# ─────────────────────────────────────────────

WORKDIR2=$(mktemp -d)
cd "$WORKDIR2"
$NIT init --agents-md >/dev/null
[ -f AGENTS.md ] && pass "agents-md creates AGENTS.md" || fail "agents-md creates AGENTS.md" "file missing"
assert_contains "AGENTS.md has nit instructions" "nit ready" "$(cat AGENTS.md)"
cd "$WORKDIR"
rm -rf "$WORKDIR2"

# ─────────────────────────────────────────────
echo "--- add ---"
# ─────────────────────────────────────────────

OUT=$($NIT add "Fix login bug" -p high -k bug -c "Login fails on Safari" --tags "auth,bug" --files "src/auth.rs" -a "login test passes" -f json)
ID=$(jq_val "$OUT" "d['id']")
assert_eq "add returns id 1" "1" "$ID"
assert_eq "add priority" "high" "$(jq_val "$OUT" "d['priority']")"
assert_eq "add kind" "bug" "$(jq_val "$OUT" "d['kind']")"
assert_eq "add context" "Login fails on Safari" "$(jq_val "$OUT" "d['context']")"
assert_eq "add acceptance" "login test passes" "$(jq_val "$OUT" "d['acceptance']")"

OUT=$($NIT add "Add logout endpoint" -p medium -k feature -f json)
assert_eq "add second issue id" "2" "$(jq_val "$OUT" "d['id']")"

OUT=$($NIT add "Write auth tests" -p low -k task -f json)
assert_eq "add third issue id" "3" "$(jq_val "$OUT" "d['id']")"

# ─────────────────────────────────────────────
echo "--- add --stdin-json ---"
# ─────────────────────────────────────────────

OUT=$(echo '{"title":"Stdin issue","priority":"critical","kind":"bug","tags":["test"]}' | $NIT add --stdin-json -f json)
assert_eq "stdin-json add priority" "critical" "$(jq_val "$OUT" "d['priority']")"
assert_eq "stdin-json add kind" "bug" "$(jq_val "$OUT" "d['kind']")"

# ─────────────────────────────────────────────
echo "--- add validation ---"
# ─────────────────────────────────────────────

assert_exit "add rejects invalid priority" "1" $NIT add "Bad" -p invalid
assert_exit "add rejects invalid kind" "1" $NIT add "Bad" -k invalid

# ─────────────────────────────────────────────
echo "--- get ---"
# ─────────────────────────────────────────────

OUT=$($NIT get 1 -f json)
assert_eq "get title" "Fix login bug" "$(jq_val "$OUT" "d['title']")"
assert_eq "get has urgency" "True" "$(jq_val "$OUT" "d['urgency'] > 0")"
assert_eq "get has breakdown" "True" "$(jq_val "$OUT" "d['urgency_breakdown'] is not None")"

COMPACT=$($NIT get 1)
assert_contains "get compact has ID" "ID:1" "$COMPACT"
assert_contains "get compact has TITLE" "TITLE: Fix login bug" "$COMPACT"
assert_contains "get compact has URGENCY BREAKDOWN" "URGENCY BREAKDOWN" "$COMPACT"

assert_exit "get nonexistent exits 1" "1" $NIT get 999

# ─────────────────────────────────────────────
echo "--- list ---"
# ─────────────────────────────────────────────

OUT=$($NIT list -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list returns 4 open issues" "4" "$COUNT"

OUT=$($NIT list -p high -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list filter by priority" "1" "$COUNT"

OUT=$($NIT list -k bug -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list filter by kind" "2" "$COUNT"

OUT=$($NIT list --tag auth -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list filter by tag" "1" "$COUNT"

PRETTY=$($NIT list -f pretty)
assert_contains "list pretty has header" "Status" "$PRETTY"

# Sort by urgency — first issue should be highest urgency
FIRST_ID=$(jq_val "$($NIT list --sort urgency -f json)" "d[0]['id']")
assert_eq "list sorted by urgency, critical first" "4" "$FIRST_ID"

# ─────────────────────────────────────────────
echo "--- update ---"
# ─────────────────────────────────────────────

OUT=$($NIT update 2 -s in-progress -f json)
assert_eq "update status" "in-progress" "$(jq_val "$OUT" "d['status']")"

OUT=$($NIT update 1 --add-tag "critical" -f json)
assert_contains "update add-tag" "critical" "$(jq_val "$OUT" "','.join(d['tags'])")"

OUT=$($NIT update 1 --remove-tag "critical" -f json)
TAGS=$(jq_val "$OUT" "','.join(d['tags'])")
assert_eq "update remove-tag" "auth,bug" "$TAGS"

OUT=$($NIT update 1 --title "Updated title" -f json)
assert_eq "update title" "Updated title" "$(jq_val "$OUT" "d['title']")"
# Restore
$NIT update 1 --title "Fix login bug" -f json >/dev/null

assert_exit "update invalid status" "1" $NIT update 1 -s invalid

# ─────────────────────────────────────────────
echo "--- dependencies ---"
# ─────────────────────────────────────────────

OUT=$($NIT depend 3 --on 1)
assert_contains "depend output" "3 blocked by 1" "$OUT"

OUT=$($NIT get 3 -f json)
assert_eq "depend makes issue blocked" "True" "$(jq_val "$OUT" "d['is_blocked']")"

# Idempotent re-add
OUT=$($NIT depend 3 --on 1)
pass "depend idempotent re-add succeeds"

# Cycle detection
assert_exit "depend cycle detection" "1" $NIT depend 1 --on 3

# Undepend
$NIT undepend 3 --on 1 >/dev/null
OUT=$($NIT get 3 -f json)
assert_eq "undepend removes dependency" "False" "$(jq_val "$OUT" "d['is_blocked']")"

# Undepend idempotent
$NIT undepend 3 --on 1 >/dev/null
pass "undepend idempotent succeeds"

# ─────────────────────────────────────────────
echo "--- notes ---"
# ─────────────────────────────────────────────

OUT=$($NIT note 1 "Investigation started" --agent "test-session")
assert_contains "note output" "ISSUE:1" "$OUT"
assert_contains "note has agent" "test-session" "$OUT"

OUT=$($NIT get 1 -f json)
NOTES_COUNT=$(jq_val "$OUT" "len(d['notes'])")
assert_eq "note appended" "1" "$NOTES_COUNT"
assert_eq "note content" "Investigation started" "$(jq_val "$OUT" "d['notes'][0]['content']")"
assert_eq "note agent" "test-session" "$(jq_val "$OUT" "d['notes'][0]['agent']")"

# Stdin note
echo "Piped note content" | $NIT note 1 --agent "pipe-test" >/dev/null
OUT=$($NIT get 1 -f json)
NOTES_COUNT=$(jq_val "$OUT" "len(d['notes'])")
assert_eq "stdin note appended" "2" "$NOTES_COUNT"

assert_exit "note on nonexistent issue" "1" $NIT note 999 "nope"

# ─────────────────────────────────────────────
echo "--- next ---"
# ─────────────────────────────────────────────

# Issue 2 is in-progress, so next should return an open issue
OUT=$($NIT next -f json)
STATUS=$(jq_val "$OUT" "d['status']")
assert_eq "next returns open issue" "open" "$STATUS"

# ─────────────────────────────────────────────
echo "--- next --claim ---"
# ─────────────────────────────────────────────

OUT=$($NIT next --claim -f json)
CLAIM_ID=$(jq_val "$OUT" "d['id']")
assert_eq "next --claim sets in-progress" "in-progress" "$(jq_val "$OUT" "d['status']")"
# Restore for later tests
$NIT update "$CLAIM_ID" -s open >/dev/null

# ─────────────────────────────────────────────
echo "--- ready ---"
# ─────────────────────────────────────────────

OUT=$($NIT ready -f json)
COUNT=$(jq_val "$OUT" "len(d)")
# Should include open and in-progress unblocked issues
[ "$COUNT" -ge 1 ] && pass "ready returns issues" || fail "ready returns issues" "got $COUNT"

# First result should have highest urgency
URG1=$(jq_val "$OUT" "d[0]['urgency']")
URG2=$(jq_val "$OUT" "d[1]['urgency']" 2>/dev/null || echo "0")
[ "$(python3 -c "print($URG1 >= $URG2)")" = "True" ] && pass "ready sorted by urgency desc" || fail "ready sorted by urgency desc" "$URG1 < $URG2"

OUT=$($NIT ready -n 2 -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "ready --limit 2" "2" "$COUNT"

# ─────────────────────────────────────────────
echo "--- close ---"
# ─────────────────────────────────────────────

# Set up dependency: 3 blocked by 1
$NIT depend 3 --on 1 >/dev/null

OUT=$($NIT close 1 "Fixed in commit abc123" -f json)
assert_eq "close sets done" "done" "$(jq_val "$OUT" "d['status']")"
assert_eq "close stores reason" "Fixed in commit abc123" "$(jq_val "$OUT" "d['close_reason']")"

# Check unblock
OUT=$($NIT get 3 -f json)
assert_eq "close unblocks dependent" "False" "$(jq_val "$OUT" "d['is_blocked']")"

# ─────────────────────────────────────────────
echo "--- close --wontfix ---"
# ─────────────────────────────────────────────

OUT=$($NIT close 3 --wontfix "Superseded by issue 5" -f json)
assert_eq "close --wontfix status" "wontfix" "$(jq_val "$OUT" "d['status']")"
assert_eq "close --wontfix reason" "Superseded by issue 5" "$(jq_val "$OUT" "d['close_reason']")"

# ─────────────────────────────────────────────
echo "--- stats ---"
# ─────────────────────────────────────────────

OUT=$($NIT stats -f json)
TOTAL=$(jq_val "$OUT" "d['total']")
assert_eq "stats total" "4" "$TOTAL"
DONE=$(jq_val "$OUT" "d['by_status']['done']")
assert_eq "stats done count" "1" "$DONE"
WONTFIX=$(jq_val "$OUT" "d['by_status']['wontfix']")
assert_eq "stats wontfix count" "1" "$WONTFIX"

COMPACT=$($NIT stats)
assert_contains "stats compact has TOTAL" "TOTAL:" "$COMPACT"

# ─────────────────────────────────────────────
echo "--- batch add ---"
# ─────────────────────────────────────────────

BATCH_OUT=$(echo '[
  {"title":"Batch issue 1","priority":"high","kind":"bug","tags":["batch"]},
  {"title":"Batch issue 2","priority":"medium","kind":"task"},
  {"title":"Batch issue 3","blocked_by":["@0","@1"],"acceptance":"tests pass"}
]' | $NIT batch add -f json)
BATCH_COUNT=$(jq_val "$BATCH_OUT" "len(d)")
assert_eq "batch creates 3 issues" "3" "$BATCH_COUNT"

BATCH_LAST_BLOCKED=$(jq_val "$BATCH_OUT" "d[2]['is_blocked']")
assert_eq "batch @ref creates dependency" "True" "$BATCH_LAST_BLOCKED"

# ─────────────────────────────────────────────
echo "--- batch add validation ---"
# ─────────────────────────────────────────────

# Invalid priority should fail the whole batch
set +e
echo '[{"title":"Good"},{"title":"Bad","priority":"invalid"}]' | $NIT batch add -f json >/dev/null 2>&1
BATCH_EXIT=$?
set -e
assert_eq "batch rejects invalid data (transaction)" "1" "$BATCH_EXIT"

# ─────────────────────────────────────────────
echo "--- graph ---"
# ─────────────────────────────────────────────

OUT=$($NIT graph -f json)
NODES=$(jq_val "$OUT" "len(d['nodes'])")
[ "$NODES" -ge 1 ] && pass "graph has nodes" || fail "graph has nodes" "got $NODES"

EDGES=$(jq_val "$OUT" "len(d['edges'])")
[ "$EDGES" -ge 1 ] && pass "graph has edges" || fail "graph has edges" "got $EDGES"

DOT=$($NIT graph -f pretty)
assert_contains "graph DOT output" "digraph nit" "$DOT"
assert_contains "graph DOT has edges" "->" "$DOT"

# ─────────────────────────────────────────────
echo "--- export/import ---"
# ─────────────────────────────────────────────

EXPORT_FILE="$WORKDIR/export.jsonl"
$NIT export > "$EXPORT_FILE"
EXPORT_LINES=$(wc -l < "$EXPORT_FILE" | tr -d ' ')
[ "$EXPORT_LINES" -ge 1 ] && pass "export produces JSONL" || fail "export produces JSONL" "$EXPORT_LINES lines"

# JSON export
$NIT export --export-format json > "$WORKDIR/export.json"
python3 -c "import json; json.load(open('$WORKDIR/export.json'))" && pass "export json is valid JSON" || fail "export json is valid JSON" "parse error"

# Import into fresh db
IMPORT_DIR=$(mktemp -d)
cd "$IMPORT_DIR"
$NIT init -q >/dev/null
OUT=$($NIT import --file "$EXPORT_FILE" -f json)
IMPORTED=$(jq_val "$OUT" "d['imported']")
assert_eq "import count matches export" "$EXPORT_LINES" "$IMPORTED"

# Verify data survived round-trip
IMPORT_TOTAL=$(jq_val "$($NIT stats -f json)" "d['total']")
assert_eq "import total matches" "$EXPORT_LINES" "$IMPORT_TOTAL"

# Merge mode — re-import should skip all
OUT=$($NIT import --file "$EXPORT_FILE" --merge -f json)
SKIPPED=$(jq_val "$OUT" "d['skipped']")
assert_eq "import --merge skips existing" "$EXPORT_LINES" "$SKIPPED"

cd "$WORKDIR"
rm -rf "$IMPORT_DIR"

# ─────────────────────────────────────────────
echo "--- config ---"
# ─────────────────────────────────────────────

OUT=$($NIT config list)
assert_contains "config list has urgency keys" "urgency.priority.critical" "$OUT"

OUT=$($NIT config get urgency.priority.critical -f json)
assert_eq "config get default" "10" "$(jq_val "$OUT" "d['value']")"

$NIT config set urgency.priority.critical 15.0 >/dev/null
OUT=$($NIT config get urgency.priority.critical -f json)
assert_eq "config set persists" "15.0" "$(jq_val "$OUT" "d['value']")"

$NIT config reset >/dev/null
OUT=$($NIT config get urgency.priority.critical -f json)
assert_eq "config reset restores default" "10" "$(jq_val "$OUT" "d['value']")"

# ─────────────────────────────────────────────
echo "--- doctor ---"
# ─────────────────────────────────────────────

# Should report done/wontfix blockers from our closed issues
set +e
OUT=$($NIT doctor -f json 2>&1)
DOC_EXIT=$?
set -e
# Doctor may exit 1 if problems found (done blockers from earlier tests)
[ "$DOC_EXIT" -eq 0 ] || [ "$DOC_EXIT" -eq 1 ] && pass "doctor runs successfully" || fail "doctor runs" "exit $DOC_EXIT"

# ─────────────────────────────────────────────
echo "--- schema ---"
# ─────────────────────────────────────────────

OUT=$($NIT schema)
assert_contains "schema has CREATE TABLE" "CREATE TABLE" "$OUT"
assert_contains "schema has issues table" "issues" "$OUT"

OUT=$($NIT schema -f json)
python3 -c "import json; json.loads('$OUT'.replace(\"'\", \"\"))" 2>/dev/null || true
pass "schema json runs without crash"

# ─────────────────────────────────────────────
echo "--- exit codes ---"
# ─────────────────────────────────────────────

assert_exit "exit 1 on not found" "1" $NIT get 999

# Empty result set should exit 2
EMPTY_DIR=$(mktemp -d)
cd "$EMPTY_DIR"
$NIT init -q >/dev/null
assert_exit "exit 2 on empty list" "2" $NIT list
assert_exit "exit 2 on empty ready" "2" $NIT ready
assert_exit "exit 2 on empty next" "2" $NIT next
cd "$WORKDIR"
rm -rf "$EMPTY_DIR"

# No database should exit 1
assert_exit "exit 1 on no database" "1" env -u NIT_DB_PATH $NIT list --db /nonexistent/path/.nit.db

# ─────────────────────────────────────────────
echo "--- NIT_DB_PATH env var ---"
# ─────────────────────────────────────────────

ENV_DIR=$(mktemp -d)
NIT_DB_PATH="$ENV_DIR/.nit.db" $NIT init -q >/dev/null
NIT_DB_PATH="$ENV_DIR/.nit.db" $NIT add "Env test" -f json >/dev/null
OUT=$(NIT_DB_PATH="$ENV_DIR/.nit.db" $NIT list -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "NIT_DB_PATH override works" "1" "$COUNT"
rm -rf "$ENV_DIR"

# ─────────────────────────────────────────────
echo ""
echo "==============================="
echo "Results: $PASS passed, $FAIL failed"
echo "==============================="

if [ "$FAIL" -gt 0 ]; then
    echo ""
    echo "Failures:"
    for t in "${TESTS[@]}"; do
        if echo "$t" | grep -q "^FAIL"; then
            echo "  $t"
        fi
    done
    exit 1
fi

echo "All tests passed."
exit 0
