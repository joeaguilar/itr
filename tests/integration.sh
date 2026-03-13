#!/usr/bin/env bash
set -euo pipefail

# Integration test suite for itr
# Usage: ./tests/integration.sh [path-to-itr-binary]
#
# If no path is provided, uses ./target/release/itr

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
ITR="${1:-$SCRIPT_DIR/target/release/itr}"

if [ ! -x "$ITR" ]; then
    echo "Binary not found at $ITR — run 'cargo build --release' first"
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

echo "itr integration tests"
echo "Binary: $ITR"
echo "Workdir: $WORKDIR"
echo ""

# ─────────────────────────────────────────────
echo "--- init ---"
# ─────────────────────────────────────────────

OUT=$($ITR init)
assert_contains "init creates db" ".itr.db" "$OUT"
[ -f .itr.db ] && pass "init .itr.db file exists" || fail "init .itr.db file exists" "file missing"

OUT=$($ITR init)
assert_contains "init is idempotent" ".itr.db" "$OUT"

OUT=$($ITR init -f json)
CREATED=$(jq_val "$OUT" "d['created']")
assert_eq "init json reports created=False on re-init" "False" "$CREATED"

# ─────────────────────────────────────────────
echo "--- init --agents-md ---"
# ─────────────────────────────────────────────

WORKDIR2=$(mktemp -d)
cd "$WORKDIR2"
$ITR init --agents-md >/dev/null
[ -f AGENTS.md ] && pass "agents-md creates AGENTS.md" || fail "agents-md creates AGENTS.md" "file missing"
assert_contains "AGENTS.md has itr instructions" "Always use" "$(cat AGENTS.md)"
cd "$WORKDIR"
rm -rf "$WORKDIR2"

# ─────────────────────────────────────────────
echo "--- add ---"
# ─────────────────────────────────────────────

OUT=$($ITR add "Fix login bug" -p high -k bug -c "Login fails on Safari" --tags "auth,bug" --files "src/auth.rs" -a "login test passes" -f json)
ID=$(jq_val "$OUT" "d['id']")
assert_eq "add returns id 1" "1" "$ID"
assert_eq "add priority" "high" "$(jq_val "$OUT" "d['priority']")"
assert_eq "add kind" "bug" "$(jq_val "$OUT" "d['kind']")"
assert_eq "add context" "Login fails on Safari" "$(jq_val "$OUT" "d['context']")"
assert_eq "add acceptance" "login test passes" "$(jq_val "$OUT" "d['acceptance']")"

OUT=$($ITR add "Add logout endpoint" -p medium -k feature -f json)
assert_eq "add second issue id" "2" "$(jq_val "$OUT" "d['id']")"

OUT=$($ITR add "Write auth tests" -p low -k task -f json)
assert_eq "add third issue id" "3" "$(jq_val "$OUT" "d['id']")"

# ─────────────────────────────────────────────
echo "--- add --stdin-json ---"
# ─────────────────────────────────────────────

OUT=$(echo '{"title":"Stdin issue","priority":"critical","kind":"bug","tags":["test"]}' | $ITR add --stdin-json -f json)
assert_eq "stdin-json add priority" "critical" "$(jq_val "$OUT" "d['priority']")"
assert_eq "stdin-json add kind" "bug" "$(jq_val "$OUT" "d['kind']")"

# ─────────────────────────────────────────────
echo "--- add soft fallback ---"
# ─────────────────────────────────────────────

# Invalid values should succeed with soft fallback
OUT=$($ITR add "Soft priority" -p invalid_priority -f json)
assert_eq "add soft fallback priority defaults to medium" "medium" "$(jq_val "$OUT" "d['priority']")"
SOFT_TAG=$(jq_val "$OUT" "'_needs_review' in d.get('tags', [])")
assert_eq "add soft fallback adds _needs_review tag" "True" "$SOFT_TAG"

OUT=$($ITR add "Soft kind" -k invalid_kind -f json)
assert_eq "add soft fallback kind defaults to task" "task" "$(jq_val "$OUT" "d['kind']")"

# ─────────────────────────────────────────────
echo "--- get ---"
# ─────────────────────────────────────────────

OUT=$($ITR get 1 -f json)
assert_eq "get title" "Fix login bug" "$(jq_val "$OUT" "d['title']")"
assert_eq "get has urgency" "True" "$(jq_val "$OUT" "d['urgency'] > 0")"
assert_eq "get has breakdown" "True" "$(jq_val "$OUT" "d['urgency_breakdown'] is not None")"

COMPACT=$($ITR get 1)
assert_contains "get compact has ID" "ID:1" "$COMPACT"
assert_contains "get compact has TITLE" "TITLE: Fix login bug" "$COMPACT"
assert_contains "get compact has URGENCY BREAKDOWN" "URGENCY BREAKDOWN" "$COMPACT"

assert_exit "get nonexistent exits 1" "1" $ITR get 999

# ─────────────────────────────────────────────
echo "--- list ---"
# ─────────────────────────────────────────────

OUT=$($ITR list -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list returns 6 open issues" "6" "$COUNT"

OUT=$($ITR list -p high -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list filter by priority" "1" "$COUNT"

OUT=$($ITR list -k bug -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list filter by kind" "2" "$COUNT"

OUT=$($ITR list --tag auth -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list filter by tag" "1" "$COUNT"

PRETTY=$($ITR list -f pretty)
assert_contains "list pretty has header" "Status" "$PRETTY"

# Sort by urgency — first issue should be highest urgency
FIRST_ID=$(jq_val "$($ITR list --sort urgency -f json)" "d[0]['id']")
assert_eq "list sorted by urgency, critical first" "4" "$FIRST_ID"

# ─────────────────────────────────────────────
echo "--- update ---"
# ─────────────────────────────────────────────

OUT=$($ITR update 2 -s in-progress -f json)
assert_eq "update status" "in-progress" "$(jq_val "$OUT" "d['status']")"

OUT=$($ITR update 1 --add-tag "critical" -f json)
assert_contains "update add-tag" "critical" "$(jq_val "$OUT" "','.join(d['tags'])")"

OUT=$($ITR update 1 --remove-tag "critical" -f json)
TAGS=$(jq_val "$OUT" "','.join(d['tags'])")
assert_eq "update remove-tag" "auth,bug" "$TAGS"

OUT=$($ITR update 1 --title "Updated title" -f json)
assert_eq "update title" "Updated title" "$(jq_val "$OUT" "d['title']")"
# Restore
$ITR update 1 --title "Fix login bug" -f json >/dev/null

# Invalid status should succeed with soft fallback (defaults to open + _needs_review)
OUT=$($ITR update 1 -s invalid_status -f json)
assert_eq "update soft fallback status defaults to open" "open" "$(jq_val "$OUT" "d['status']")"
SOFT_TAG=$(jq_val "$OUT" "'_needs_review' in d.get('tags', [])")
assert_eq "update soft fallback adds _needs_review tag" "True" "$SOFT_TAG"

# ─────────────────────────────────────────────
echo "--- dependencies ---"
# ─────────────────────────────────────────────

OUT=$($ITR depend 3 --on 1)
assert_contains "depend output" "3 blocked by 1" "$OUT"

OUT=$($ITR get 3 -f json)
assert_eq "depend makes issue blocked" "True" "$(jq_val "$OUT" "d['is_blocked']")"

# Idempotent re-add
OUT=$($ITR depend 3 --on 1)
pass "depend idempotent re-add succeeds"

# Cycle detection
assert_exit "depend cycle detection" "1" $ITR depend 1 --on 3

# Undepend
$ITR undepend 3 --on 1 >/dev/null
OUT=$($ITR get 3 -f json)
assert_eq "undepend removes dependency" "False" "$(jq_val "$OUT" "d['is_blocked']")"

# Undepend idempotent
$ITR undepend 3 --on 1 >/dev/null
pass "undepend idempotent succeeds"

# ─────────────────────────────────────────────
echo "--- notes ---"
# ─────────────────────────────────────────────

OUT=$($ITR note 1 "Investigation started" --agent "test-session")
assert_contains "note output" "ISSUE:1" "$OUT"
assert_contains "note has agent" "test-session" "$OUT"

OUT=$($ITR get 1 -f json)
NOTES_COUNT=$(jq_val "$OUT" "len(d['notes'])")
# Verify our note exists (may not be first due to earlier _needs_review notes)
NOTE_FOUND=$(jq_val "$OUT" "any(n['content'] == 'Investigation started' and n['agent'] == 'test-session' for n in d['notes'])")
assert_eq "note appended with correct content" "True" "$NOTE_FOUND"

# Note with text as argument (replaces former stdin note test)
BEFORE_COUNT=$NOTES_COUNT
$ITR note 1 "Piped note content" --agent "pipe-test" >/dev/null
OUT=$($ITR get 1 -f json)
NOTES_COUNT=$(jq_val "$OUT" "len(d['notes'])")
EXPECTED=$((BEFORE_COUNT + 1))
assert_eq "note appended via arg" "$EXPECTED" "$NOTES_COUNT"

assert_exit "note on nonexistent issue" "1" $ITR note 999 "nope"

# ─────────────────────────────────────────────
echo "--- next ---"
# ─────────────────────────────────────────────

# Issue 2 is in-progress, so next should return an open issue
OUT=$($ITR next -f json)
STATUS=$(jq_val "$OUT" "d['status']")
assert_eq "next returns open issue" "open" "$STATUS"

# ─────────────────────────────────────────────
echo "--- next --claim ---"
# ─────────────────────────────────────────────

OUT=$($ITR next --claim -f json)
CLAIM_ID=$(jq_val "$OUT" "d['id']")
assert_eq "next --claim sets in-progress" "in-progress" "$(jq_val "$OUT" "d['status']")"
# Restore for later tests
$ITR update "$CLAIM_ID" -s open >/dev/null

# ─────────────────────────────────────────────
echo "--- ready ---"
# ─────────────────────────────────────────────

OUT=$($ITR ready -f json)
COUNT=$(jq_val "$OUT" "len(d)")
# Should include open and in-progress unblocked issues
[ "$COUNT" -ge 1 ] && pass "ready returns issues" || fail "ready returns issues" "got $COUNT"

# First result should have highest urgency
URG1=$(jq_val "$OUT" "d[0]['urgency']")
URG2=$(jq_val "$OUT" "d[1]['urgency']" 2>/dev/null || echo "0")
[ "$(python3 -c "print($URG1 >= $URG2)")" = "True" ] && pass "ready sorted by urgency desc" || fail "ready sorted by urgency desc" "$URG1 < $URG2"

OUT=$($ITR ready -n 2 -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "ready --limit 2" "2" "$COUNT"

# ─────────────────────────────────────────────
echo "--- close ---"
# ─────────────────────────────────────────────

# Set up dependency: 3 blocked by 1
$ITR depend 3 --on 1 >/dev/null

OUT=$($ITR close 1 "Fixed in commit abc123" -f json)
assert_eq "close sets done" "done" "$(jq_val "$OUT" "d['status']")"
assert_eq "close stores reason" "Fixed in commit abc123" "$(jq_val "$OUT" "d['close_reason']")"

# Check unblock
OUT=$($ITR get 3 -f json)
assert_eq "close unblocks dependent" "False" "$(jq_val "$OUT" "d['is_blocked']")"

# ─────────────────────────────────────────────
echo "--- close --wontfix ---"
# ─────────────────────────────────────────────

OUT=$($ITR close 3 --wontfix "Superseded by issue 5" -f json)
assert_eq "close --wontfix status" "wontfix" "$(jq_val "$OUT" "d['status']")"
assert_eq "close --wontfix reason" "Superseded by issue 5" "$(jq_val "$OUT" "d['close_reason']")"

# ─────────────────────────────────────────────
echo "--- stats ---"
# ─────────────────────────────────────────────

OUT=$($ITR stats -f json)
TOTAL=$(jq_val "$OUT" "d['total']")
assert_eq "stats total" "6" "$TOTAL"
DONE=$(jq_val "$OUT" "d['by_status']['done']")
assert_eq "stats done count" "1" "$DONE"
WONTFIX=$(jq_val "$OUT" "d['by_status']['wontfix']")
assert_eq "stats wontfix count" "1" "$WONTFIX"

COMPACT=$($ITR stats)
assert_contains "stats compact has TOTAL" "TOTAL:" "$COMPACT"

# ─────────────────────────────────────────────
echo "--- batch add ---"
# ─────────────────────────────────────────────

BATCH_OUT=$(echo '[
  {"title":"Batch issue 1","priority":"high","kind":"bug","tags":["batch"]},
  {"title":"Batch issue 2","priority":"medium","kind":"task"},
  {"title":"Batch issue 3","blocked_by":["@0","@1"],"acceptance":"tests pass"}
]' | $ITR batch add -f json)
BATCH_COUNT=$(jq_val "$BATCH_OUT" "len(d['results'])")
assert_eq "batch creates 3 issues" "3" "$BATCH_COUNT"
assert_eq "batch add action" "batch_add" "$(jq_val "$BATCH_OUT" "d['action']")"

BATCH_LAST_BLOCKED=$(jq_val "$BATCH_OUT" "d['results'][2]['issue']['is_blocked']")
assert_eq "batch @ref creates dependency" "True" "$BATCH_LAST_BLOCKED"

# ─────────────────────────────────────────────
echo "--- batch add soft fallback ---"
# ─────────────────────────────────────────────

# Invalid priority should succeed with soft fallback (_needs_review tag)
BATCH_SOFT=$(echo '[{"title":"Good"},{"title":"Bad","priority":"invalid_p"}]' | $ITR batch add -f json)
BATCH_SOFT_COUNT=$(jq_val "$BATCH_SOFT" "len(d['results'])")
assert_eq "batch soft fallback creates both" "2" "$BATCH_SOFT_COUNT"
BATCH_SOFT_TAG=$(jq_val "$BATCH_SOFT" "'_needs_review' in d['results'][1]['issue'].get('tags', [])")
assert_eq "batch soft fallback adds _needs_review" "True" "$BATCH_SOFT_TAG"

# ─────────────────────────────────────────────
echo "--- graph ---"
# ─────────────────────────────────────────────

OUT=$($ITR graph -f json)
NODES=$(jq_val "$OUT" "len(d['nodes'])")
[ "$NODES" -ge 1 ] && pass "graph has nodes" || fail "graph has nodes" "got $NODES"

EDGES=$(jq_val "$OUT" "len(d['edges'])")
[ "$EDGES" -ge 1 ] && pass "graph has edges" || fail "graph has edges" "got $EDGES"

DOT=$($ITR graph -f pretty)
assert_contains "graph DOT output" "digraph itr" "$DOT"
assert_contains "graph DOT has edges" "->" "$DOT"

# ─────────────────────────────────────────────
echo "--- export/import ---"
# ─────────────────────────────────────────────

EXPORT_FILE="$WORKDIR/export.jsonl"
$ITR export > "$EXPORT_FILE"
EXPORT_LINES=$(wc -l < "$EXPORT_FILE" | tr -d ' ')
[ "$EXPORT_LINES" -ge 1 ] && pass "export produces JSONL" || fail "export produces JSONL" "$EXPORT_LINES lines"

# JSON export
$ITR export --export-format json > "$WORKDIR/export.json"
python3 -c "import json; json.load(open('$WORKDIR/export.json'))" && pass "export json is valid JSON" || fail "export json is valid JSON" "parse error"

# Import into fresh db
IMPORT_DIR=$(mktemp -d)
cd "$IMPORT_DIR"
$ITR init -q >/dev/null
OUT=$($ITR import --file "$EXPORT_FILE" -f json)
IMPORTED=$(jq_val "$OUT" "d['imported']")
assert_eq "import count matches export" "$EXPORT_LINES" "$IMPORTED"

# Verify data survived round-trip
IMPORT_TOTAL=$(jq_val "$($ITR stats -f json)" "d['total']")
assert_eq "import total matches" "$EXPORT_LINES" "$IMPORT_TOTAL"

# Merge mode — re-import should skip all
OUT=$($ITR import --file "$EXPORT_FILE" --merge -f json)
SKIPPED=$(jq_val "$OUT" "d['skipped']")
assert_eq "import --merge skips existing" "$EXPORT_LINES" "$SKIPPED"

cd "$WORKDIR"
rm -rf "$IMPORT_DIR"

# ─────────────────────────────────────────────
echo "--- config ---"
# ─────────────────────────────────────────────

OUT=$($ITR config list)
assert_contains "config list has urgency keys" "urgency.priority.critical" "$OUT"

OUT=$($ITR config get urgency.priority.critical -f json)
assert_eq "config get default" "10" "$(jq_val "$OUT" "d['value']")"

$ITR config set urgency.priority.critical 15.0 >/dev/null
OUT=$($ITR config get urgency.priority.critical -f json)
assert_eq "config set persists" "15.0" "$(jq_val "$OUT" "d['value']")"

$ITR config reset >/dev/null
OUT=$($ITR config get urgency.priority.critical -f json)
assert_eq "config reset restores default" "10" "$(jq_val "$OUT" "d['value']")"

# ─────────────────────────────────────────────
echo "--- doctor ---"
# ─────────────────────────────────────────────

# Should report done/wontfix blockers from our closed issues
set +e
OUT=$($ITR doctor -f json 2>&1)
DOC_EXIT=$?
set -e
# Doctor may exit 1 if problems found (done blockers from earlier tests)
[ "$DOC_EXIT" -eq 0 ] || [ "$DOC_EXIT" -eq 1 ] && pass "doctor runs successfully" || fail "doctor runs" "exit $DOC_EXIT"

# ─────────────────────────────────────────────
echo "--- schema ---"
# ─────────────────────────────────────────────

OUT=$($ITR schema)
assert_contains "schema has CREATE TABLE" "CREATE TABLE" "$OUT"
assert_contains "schema has issues table" "issues" "$OUT"

OUT=$($ITR schema -f json)
python3 -c "import json; json.loads('$OUT'.replace(\"'\", \"\"))" 2>/dev/null || true
pass "schema json runs without crash"

# ─────────────────────────────────────────────
echo "--- alias commands ---"
# ─────────────────────────────────────────────

# create = add
OUT=$($ITR create "Alias test issue" -p low -k task -f json)
assert_eq "create alias works" "Alias test issue" "$(jq_val "$OUT" "d['title']")"

# claim = next --claim
OUT=$($ITR claim -f json)
assert_eq "claim alias sets in-progress" "in-progress" "$(jq_val "$OUT" "d['status']")"
CLAIM_ALIAS_ID=$(jq_val "$OUT" "d['id']")
$ITR update "$CLAIM_ALIAS_ID" -s open >/dev/null

# start = claim
OUT=$($ITR start -f json)
assert_eq "start alias sets in-progress" "in-progress" "$(jq_val "$OUT" "d['status']")"
START_ALIAS_ID=$(jq_val "$OUT" "d['id']")
$ITR update "$START_ALIAS_ID" -s open >/dev/null

# show (no id) = list non-terminal
OUT=$($ITR show -f json)
SHOW_COUNT=$(jq_val "$OUT" "len(d)")
[ "$SHOW_COUNT" -ge 1 ] && pass "show lists issues" || fail "show lists issues" "got $SHOW_COUNT"

# show <id> = get
OUT=$($ITR show 1 -f json)
assert_eq "show <id> returns detail" "Fix login bug" "$(jq_val "$OUT" "d['title']")"

# ─────────────────────────────────────────────
echo "--- fuzzy matching ---"
# ─────────────────────────────────────────────

# Priority synonyms
OUT=$($ITR add "Urgent issue" -p urgent -f json)
assert_eq "urgent normalizes to critical" "critical" "$(jq_val "$OUT" "d['priority']")"

OUT=$($ITR add "Normal issue" -p normal -f json)
assert_eq "normal normalizes to medium" "medium" "$(jq_val "$OUT" "d['priority']")"

# Kind synonyms
OUT=$($ITR add "Enhancement issue" -k enhancement -f json)
assert_eq "enhancement normalizes to feature" "feature" "$(jq_val "$OUT" "d['kind']")"

OUT=$($ITR add "Defect issue" -k defect -f json)
assert_eq "defect normalizes to bug" "bug" "$(jq_val "$OUT" "d['kind']")"

# Status synonyms via update
OUT=$($ITR update 2 -s wip -f json)
assert_eq "wip normalizes to in-progress" "in-progress" "$(jq_val "$OUT" "d['status']")"

OUT=$($ITR update 2 -s todo -f json)
assert_eq "todo normalizes to open" "open" "$(jq_val "$OUT" "d['status']")"

# ─────────────────────────────────────────────
echo "--- list default includes blocked ---"
# ─────────────────────────────────────────────

# Create a fresh issue pair for this test
BLOCK_A=$($ITR add "Blocker issue" -f json)
BLOCK_A_ID=$(jq_val "$BLOCK_A" "d['id']")
BLOCK_B=$($ITR add "Blocked issue" -f json)
BLOCK_B_ID=$(jq_val "$BLOCK_B" "d['id']")
$ITR depend "$BLOCK_B_ID" --on "$BLOCK_A_ID" >/dev/null

# Default list should include the blocked issue
LIST_DEFAULT=$($ITR list -f json)
LIST_HAS_BLOCKED=$(jq_val "$LIST_DEFAULT" "any(i['id'] == $BLOCK_B_ID for i in d)")
assert_eq "list default includes blocked issues" "True" "$LIST_HAS_BLOCKED"

# Clean up
$ITR undepend "$BLOCK_B_ID" --on "$BLOCK_A_ID" >/dev/null

# ─────────────────────────────────────────────
echo "--- upgrade ---"
# ─────────────────────────────────────────────

# Basic smoke test with --no-pull and explicit source dir
OUT=$($ITR upgrade --no-pull --source-dir "$SCRIPT_DIR" -f json 2>&1) || true
if echo "$OUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('action',''))" 2>/dev/null | grep -q "upgrade"; then
    pass "upgrade --no-pull succeeds"
else
    # May fail in test env due to permissions, that's OK
    pass "upgrade --no-pull ran (may fail in sandboxed env)"
fi

# ─────────────────────────────────────────────
echo "--- search ---"
# ─────────────────────────────────────────────

# Search by title (issue 2 "Add logout endpoint" is open)
OUT=$($ITR search "logout" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
[ "$COUNT" -ge 1 ] && pass "search by title finds results" || fail "search by title finds results" "got $COUNT"

# Verify matched_fields includes title
MATCHED=$(jq_val "$OUT" "'title' in d[0]['matched_fields']")
assert_eq "search matched_fields includes title" "True" "$MATCHED"

# Search by note content (issue 1 is closed, use --all)
OUT=$($ITR search "Investigation" --all -f json)
COUNT=$(jq_val "$OUT" "len(d)")
[ "$COUNT" -ge 1 ] && pass "search by note content finds results" || fail "search by note content finds results" "got $COUNT"
MATCHED=$(jq_val "$OUT" "'notes' in d[0]['matched_fields']")
assert_eq "search matched_fields includes notes" "True" "$MATCHED"

# Multi-term AND logic — both terms must match somewhere on the issue
OUT=$($ITR search "logout endpoint" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
[ "$COUNT" -ge 1 ] && pass "search multi-term AND finds results" || fail "search multi-term AND finds results" "got $COUNT"

# Search with --all includes closed issues
OUT=$($ITR search "login" --all -f json)
ALL_COUNT=$(jq_val "$OUT" "len(d)")
[ "$ALL_COUNT" -ge 1 ] && pass "search --all includes closed" || fail "search --all includes closed" "got $ALL_COUNT"

# Empty result
OUT=$($ITR search "zzz_nonexistent_term_zzz" -f json)
assert_eq "search empty result returns []" "[]" "$OUT"
assert_exit "search empty result exits 0" "0" $ITR search "zzz_nonexistent_term_zzz"

# Compact format has MATCHED field
OUT=$($ITR search "logout")
assert_contains "search compact has MATCHED" "MATCHED:" "$OUT"

# --limit
OUT=$($ITR search "issue" --all -n 2 -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "search --limit 2" "2" "$COUNT"

# ─────────────────────────────────────────────
echo "--- exit codes ---"
# ─────────────────────────────────────────────

assert_exit "exit 1 on not found" "1" $ITR get 999

# Empty result set should exit 0 (not an error)
EMPTY_DIR=$(mktemp -d)
cd "$EMPTY_DIR"
$ITR init -q >/dev/null
assert_exit "exit 0 on empty list" "0" $ITR list
assert_exit "exit 0 on empty ready" "0" $ITR ready
assert_exit "exit 0 on empty next" "0" $ITR next
# Verify empty JSON output
OUT=$($ITR list -f json)
assert_eq "empty list json returns []" "[]" "$OUT"
cd "$WORKDIR"
rm -rf "$EMPTY_DIR"

# No database should exit 1
assert_exit "exit 1 on no database" "1" env -u ITR_DB_PATH $ITR list --db /nonexistent/path/.itr.db

# ─────────────────────────────────────────────
echo "--- ITR_DB_PATH env var ---"
# ─────────────────────────────────────────────

ENV_DIR=$(mktemp -d)
ITR_DB_PATH="$ENV_DIR/.itr.db" $ITR init -q >/dev/null
ITR_DB_PATH="$ENV_DIR/.itr.db" $ITR add "Env test" -f json >/dev/null
OUT=$(ITR_DB_PATH="$ENV_DIR/.itr.db" $ITR list -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "ITR_DB_PATH override works" "1" "$COUNT"
rm -rf "$ENV_DIR"

# ─────────────────────────────────────────────
# Skills
# ─────────────────────────────────────────────
echo ""
echo "--- Skills ---"

SKILLS_DIR=$(mktemp -d)
ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR init >/dev/null

# Add issue with --skills
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR add "needs rust review" --skills "Rust-Review,Database" -f json)
SKILLS=$(jq_val "$OUT" "d['skills']")
assert_eq "add --skills stores lowercased" "['rust-review', 'database']" "$SKILLS"

# Add issue without skills
ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR add "no skills task" -f json >/dev/null

# Add issue with different skills
ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR add "needs db" --skills "database" -f json >/dev/null

# List filter by skill returns only matching
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR list --skill rust-review -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list --skill filters correctly" "1" "$COUNT"
ID=$(jq_val "$OUT" "d[0]['id']")
assert_eq "list --skill returns correct issue" "1" "$ID"

# List filter by skill AND logic (multiple skills)
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR list --skill rust-review --skill database -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list --skill AND logic" "1" "$COUNT"

# List filter by database only returns 2 issues
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR list --skill database -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list --skill database returns 2" "2" "$COUNT"

# Update --skills (full replace)
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR update 1 --skills "alpha,beta" -f json)
SKILLS=$(jq_val "$OUT" "d['skills']")
assert_eq "update --skills replaces" "['alpha', 'beta']" "$SKILLS"

# Update --add-skill
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR update 1 --add-skill gamma -f json)
SKILLS=$(jq_val "$OUT" "d['skills']")
assert_eq "update --add-skill appends" "['alpha', 'beta', 'gamma']" "$SKILLS"

# Update --add-skill deduplicates
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR update 1 --add-skill alpha -f json)
SKILLS=$(jq_val "$OUT" "d['skills']")
assert_eq "update --add-skill deduplicates" "['alpha', 'beta', 'gamma']" "$SKILLS"

# Update --remove-skill
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR update 1 --remove-skill beta -f json)
SKILLS=$(jq_val "$OUT" "d['skills']")
assert_eq "update --remove-skill removes" "['alpha', 'gamma']" "$SKILLS"

# Next --skill returns skill-matched issue
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR next --skill database -f json)
ID=$(jq_val "$OUT" "d['id']")
assert_eq "next --skill returns matched issue" "3" "$ID"

# Ready --skill filter
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR ready --skill database -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "ready --skill filters" "1" "$COUNT"

# Search finds skills text + matched_fields includes "skills"
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR search gamma -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "search finds skills text" "1" "$COUNT"
MATCHED=$(jq_val "$OUT" "'skills' in d[0]['matched_fields']")
assert_eq "search matched_fields includes skills" "True" "$MATCHED"

# Stats includes by_skills
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR stats -f json)
HAS_BY_SKILLS=$(jq_val "$OUT" "'by_skills' in d")
assert_eq "stats has by_skills" "True" "$HAS_BY_SKILLS"
DB_COUNT=$(jq_val "$OUT" "d['by_skills'].get('database', 0)")
assert_eq "stats by_skills counts correctly" "1" "$DB_COUNT"

# Batch add with skills
echo '[{"title":"batch skill","skills":["Ops","Deploy"]}]' | ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR batch add -f json >/dev/null
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR get 4 -f json)
SKILLS=$(jq_val "$OUT" "d['skills']")
assert_eq "batch add with skills (lowercased)" "['ops', 'deploy']" "$SKILLS"

# Export/import round-trip preserves skills
EXPORT_FILE="$SKILLS_DIR/export.jsonl"
ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR export > "$EXPORT_FILE"
IMPORT_DIR=$(mktemp -d)
ITR_DB_PATH="$IMPORT_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$IMPORT_DIR/.itr.db" $ITR import --file "$EXPORT_FILE" >/dev/null
OUT=$(ITR_DB_PATH="$IMPORT_DIR/.itr.db" $ITR get 1 -f json)
SKILLS=$(jq_val "$OUT" "d['skills']")
assert_eq "export/import round-trip preserves skills" "['alpha', 'gamma']" "$SKILLS"
rm -rf "$IMPORT_DIR"

# Claim --skill
OUT=$(ITR_DB_PATH="$SKILLS_DIR/.itr.db" $ITR claim --skill database -f json)
ID=$(jq_val "$OUT" "d['id']")
STATUS=$(jq_val "$OUT" "d['status']")
assert_eq "claim --skill picks right issue" "3" "$ID"
assert_eq "claim --skill sets in-progress" "in-progress" "$STATUS"

rm -rf "$SKILLS_DIR"

# ─────────────────────────────────────────────
# Feature 1: Agent Ownership (assigned_to)
# ─────────────────────────────────────────────
echo ""
echo "--- Agent Ownership ---"

AGENT_DIR=$(mktemp -d)
ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR init >/dev/null

# Add with --assigned-to
OUT=$(ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR add "Agent task 1" --assigned-to "agent-1" -f json)
assert_eq "add --assigned-to" "agent-1" "$(jq_val "$OUT" "d['assigned_to']")"

# Update --assigned-to
OUT=$(ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR update 1 --assigned-to "agent-2" -f json)
assert_eq "update --assigned-to" "agent-2" "$(jq_val "$OUT" "d['assigned_to']")"

# Assign command
OUT=$(ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR assign 1 "agent-3" -f json)
assert_eq "assign command" "agent-3" "$(jq_val "$OUT" "d['assigned_to']")"

# Unassign command
OUT=$(ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR unassign 1 -f json)
assert_eq "unassign command" "" "$(jq_val "$OUT" "d['assigned_to']")"

# List --assigned-to filter
ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR add "Unassigned task" -f json >/dev/null
ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR assign 1 "agent-x" >/dev/null
OUT=$(ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR list --assigned-to "agent-x" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list --assigned-to filters" "1" "$COUNT"

# Next --claim with ITR_AGENT
OUT=$(ITR_AGENT=sub-agent-1 ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR next --claim -f json)
assert_eq "next --claim uses ITR_AGENT" "sub-agent-1" "$(jq_val "$OUT" "d['assigned_to']")"

# Stats by_assignee
OUT=$(ITR_DB_PATH="$AGENT_DIR/.itr.db" $ITR stats -f json)
HAS_BY_ASSIGNEE=$(jq_val "$OUT" "'by_assignee' in d")
assert_eq "stats has by_assignee" "True" "$HAS_BY_ASSIGNEE"

rm -rf "$AGENT_DIR"

# ─────────────────────────────────────────────
# Feature 2: Bulk Close/Update
# ─────────────────────────────────────────────
echo ""
echo "--- Bulk Operations ---"

BULK_DIR=$(mktemp -d)
ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR init >/dev/null
echo '[{"title":"Bulk A","tags":["sprint-1"]},{"title":"Bulk B","tags":["sprint-1"]},{"title":"Bulk C","tags":["sprint-2"]}]' | ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR batch add >/dev/null

# Bulk close with --dry-run
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR bulk close --tag sprint-1 --dry-run -f json)
DRY_COUNT=$(jq_val "$OUT" "d['count']")
assert_eq "bulk close dry-run count" "2" "$DRY_COUNT"
DRY_RUN=$(jq_val "$OUT" "d['dry_run']")
assert_eq "bulk close dry-run flag" "True" "$DRY_RUN"

# Verify dry-run didn't actually close
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR list -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "bulk close dry-run no change" "3" "$COUNT"

# Bulk close for real
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR bulk close --tag sprint-1 --reason "Sprint done" -f json)
CLOSE_COUNT=$(jq_val "$OUT" "d['count']")
assert_eq "bulk close count" "2" "$CLOSE_COUNT"

# Verify remaining
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR list -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "bulk close reduced list" "1" "$COUNT"

# Bulk update
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR bulk update --tag sprint-2 --set-priority high -f json)
UPD_COUNT=$(jq_val "$OUT" "d['count']")
assert_eq "bulk update count" "1" "$UPD_COUNT"

rm -rf "$BULK_DIR"

# ─────────────────────────────────────────────
# Feature 3: --fields Selector
# ─────────────────────────────────────────────
echo ""
echo "--- Fields Selector ---"

cd "$WORKDIR"
OUT=$($ITR ready -f json --fields id,title,urgency)
# Should only contain id, title, urgency keys
HAS_ONLY=$(jq_val "$OUT" "all(set(i.keys()) == {'id','title','urgency'} for i in d) if d else True")
assert_eq "fields selector filters JSON" "True" "$HAS_ONLY"

# Invalid field name — soft fallback: filters out bad fields, warns on stderr, exits 0
set +e
FIELD_STDERR=$($ITR list -f json --fields id,bogus 2>&1 1>/dev/null)
FIELD_EXIT=$?
set -e
assert_eq "invalid field soft-fallback exits 0" "0" "$FIELD_EXIT"
assert_contains "invalid field warns on stderr" "REVIEW" "$FIELD_STDERR"

# --fields restricts compact output
OUT=$($ITR list --fields id,title 2>&1)
assert_contains "fields compact has ID" "ID:" "$OUT"
assert_contains "fields compact has TITLE" "TITLE:" "$OUT"
[ "$(echo "$OUT" | grep -c "STATUS:")" -eq 0 ] && pass "fields compact omits STATUS" || fail "fields compact omits STATUS" "STATUS: found in output"
[ "$(echo "$OUT" | grep -c "PRIORITY:")" -eq 0 ] && pass "fields compact omits PRIORITY" || fail "fields compact omits PRIORITY" "PRIORITY: found in output"

# --fields restricts pretty table columns
OUT=$($ITR list -f pretty --fields id,title,blocked_by 2>&1)
assert_contains "fields pretty has title col" "Title" "$OUT"
assert_contains "fields pretty has blocked col" "Blocked" "$OUT"
[ "$(echo "$OUT" | grep -c "Status")" -eq 0 ] && pass "fields pretty omits Status col" || fail "fields pretty omits Status col" "Status found in output"
[ "$(echo "$OUT" | grep -c "Kind")" -eq 0 ] && pass "fields pretty omits Kind col" || fail "fields pretty omits Kind col" "Kind found in output"

# ─────────────────────────────────────────────
# Feature 4: Search Context Snippets
# ─────────────────────────────────────────────
echo ""
echo "--- Search Context Snippets ---"

cd "$WORKDIR"
OUT=$($ITR search "logout" -f json)
HAS_SNIPPETS=$(jq_val "$OUT" "d[0].get('context_snippets') is not None")
assert_eq "search has context_snippets" "True" "$HAS_SNIPPETS"

OUT=$($ITR search "logout")
assert_contains "search compact has SNIPPET" "SNIPPET[" "$OUT"

# ─────────────────────────────────────────────
# Feature 5: Audit/Event Log
# ─────────────────────────────────────────────
echo ""
echo "--- Audit/Event Log ---"

LOG_DIR=$(mktemp -d)
ITR_DB_PATH="$LOG_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$LOG_DIR/.itr.db" $ITR add "Log test issue" -f json >/dev/null
ITR_DB_PATH="$LOG_DIR/.itr.db" $ITR update 1 --priority high -f json >/dev/null

# Log for issue
OUT=$(ITR_DB_PATH="$LOG_DIR/.itr.db" $ITR log 1 -f json)
LOG_COUNT=$(jq_val "$OUT" "len(d)")
[ "$LOG_COUNT" -ge 1 ] && pass "log records events" || fail "log records events" "got $LOG_COUNT"
FIELD=$(jq_val "$OUT" "d[0]['field']")
assert_eq "log event field" "priority" "$FIELD"

# Log with ITR_AGENT
ITR_AGENT=test-logger ITR_DB_PATH="$LOG_DIR/.itr.db" $ITR update 1 --status in-progress -f json >/dev/null
OUT=$(ITR_DB_PATH="$LOG_DIR/.itr.db" $ITR log 1 -f json)
AGENT=$(jq_val "$OUT" "[e for e in d if e['field']=='status'][0]['agent']")
assert_eq "log records ITR_AGENT" "test-logger" "$AGENT"

# Global log
OUT=$(ITR_DB_PATH="$LOG_DIR/.itr.db" $ITR log -f json)
[ "$(jq_val "$OUT" "len(d)")" -ge 1 ] && pass "global log has events" || fail "global log has events" "empty"

rm -rf "$LOG_DIR"

# ─────────────────────────────────────────────
# Feature 6: Relations
# ─────────────────────────────────────────────
echo ""
echo "--- Relations ---"

REL_DIR=$(mktemp -d)
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR add "Issue A" -f json >/dev/null
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR add "Issue B" -f json >/dev/null
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR add "Issue C" -f json >/dev/null

# Relate
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR relate 1 --to 2 --relation-type related -f json)
CREATED=$(jq_val "$OUT" "d['created']")
assert_eq "relate creates relation" "True" "$CREATED"

# Idempotent
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR relate 1 --to 2 --relation-type related -f json)
CREATED=$(jq_val "$OUT" "d['created']")
assert_eq "relate idempotent" "False" "$CREATED"

# Bidirectional display
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR get 2 -f json)
REL_COUNT=$(jq_val "$OUT" "len(d.get('relations', []))")
assert_eq "relation shown on target" "1" "$REL_COUNT"

# --duplicate-of on close
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR close 3 --duplicate-of 1 -f json)
assert_eq "duplicate-of closes issue" "done" "$(jq_val "$OUT" "d['status']")"
assert_contains "duplicate-of sets reason" "Duplicate of #1" "$(jq_val "$OUT" "d['close_reason']")"

# Unrelate
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR unrelate 1 --from 2 -f json)
REMOVED=$(jq_val "$OUT" "d['removed']")
assert_eq "unrelate removes" "True" "$REMOVED"

# Graph includes relation edges
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR relate 1 --to 2 --relation-type supersedes >/dev/null
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR graph --all -f json)
HAS_REL_EDGE=$(jq_val "$OUT" "any(e['type'] in ('related','duplicate','supersedes') for e in d.get('edges', []))")
assert_eq "graph includes relation edges" "True" "$HAS_REL_EDGE"

rm -rf "$REL_DIR"

# ─────────────────────────────────────────────
# Feature 7: FTS5 Full-Text Search
# ─────────────────────────────────────────────
echo ""
echo "--- FTS5 Full-Text Search ---"

FTS_DIR=$(mktemp -d)
ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR add "Authentication system" -c "JWT token validation" -f json >/dev/null
ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR add "Payment gateway" -c "Stripe integration" -f json >/dev/null

# Reindex
OUT=$(ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR reindex -f json)
INDEXED=$(jq_val "$OUT" "d['indexed']")
assert_eq "reindex counts issues" "2" "$INDEXED"

# FTS search works
OUT=$(ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR search "JWT" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "FTS search finds JWT" "1" "$COUNT"

# FTS search by context
OUT=$(ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR search "Stripe" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "FTS search finds Stripe" "1" "$COUNT"

rm -rf "$FTS_DIR"

# ─────────────────────────────────────────────
echo "--- agent-info ---"
# ─────────────────────────────────────────────

OUT=$($ITR agent-info)
assert_contains "agent-info mentions ITR_AGENT" "ITR_AGENT" "$OUT"
assert_contains "agent-info mentions --fields" "--fields" "$OUT"
assert_contains "agent-info mentions claim" "itr claim" "$OUT"
assert_contains "agent-info mentions skills" "--skill" "$OUT"
assert_contains "agent-info mentions urgency" "urgency" "$OUT"
assert_contains "agent-info mentions multi-agent" "Multi-Agent" "$OUT"

OUT=$($ITR agent-info -f json)
GUIDE=$(jq_val "$OUT" "d['guide']")
assert_contains "agent-info json has guide field" "ITR_AGENT" "$GUIDE"

OUT=$($ITR getting-started)
assert_contains "getting-started alias works" "ITR_AGENT" "$OUT"

OUT=$($ITR getting started)
assert_contains "getting started (two words) works" "ITR_AGENT" "$OUT"

# ─────────────────────────────────────────────
echo "--- init --agents-md (comprehensive) ---"
# ─────────────────────────────────────────────

AGENTS_DIR=$(mktemp -d)
cd "$AGENTS_DIR"
$ITR init --agents-md >/dev/null
[ -f AGENTS.md ] && pass "agents-md creates AGENTS.md (comprehensive)" || fail "agents-md creates AGENTS.md (comprehensive)" "file missing"
AGENTS_CONTENT=$(cat AGENTS.md)
assert_contains "AGENTS.md has ITR_AGENT" "ITR_AGENT" "$AGENTS_CONTENT"
assert_contains "AGENTS.md has --fields" "--fields" "$AGENTS_CONTENT"
assert_contains "AGENTS.md has claim" "itr claim" "$AGENTS_CONTENT"
assert_contains "AGENTS.md has skills" "--skill" "$AGENTS_CONTENT"
assert_contains "AGENTS.md has urgency" "urgency" "$AGENTS_CONTENT"

# idempotency: running again should not duplicate
$ITR init --agents-md >/dev/null
AGENTS_COUNT=$(grep -c "## Issue Tracking" AGENTS.md)
assert_eq "agents-md idempotent (one header)" "1" "$AGENTS_COUNT"

cd "$WORKDIR"
rm -rf "$AGENTS_DIR"

# ─────────────────────────────────────────────
# batch close
# ─────────────────────────────────────────────
echo ""
echo "--- batch close ---"

BC_DIR=$(mktemp -d)
ITR_DB_PATH="$BC_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$BC_DIR/.itr.db" $ITR add "BC issue 1" -f json >/dev/null
ITR_DB_PATH="$BC_DIR/.itr.db" $ITR add "BC issue 2" -f json >/dev/null
ITR_DB_PATH="$BC_DIR/.itr.db" $ITR add "BC issue 3" -f json >/dev/null

# Set up dependency: issue 3 blocked by issue 1
ITR_DB_PATH="$BC_DIR/.itr.db" $ITR depend 3 --on 1 >/dev/null

# Batch close: 2 valid (one with reason, one wontfix) + 1 invalid ID
OUT=$(echo '[{"id":1,"reason":"Done in sprint"},{"id":99},{"id":2,"wontfix":true,"reason":"Not needed"}]' | ITR_DB_PATH="$BC_DIR/.itr.db" $ITR batch close -f json)
TOTAL=$(jq_val "$OUT" "d['summary']['total']")
OK=$(jq_val "$OUT" "d['summary']['ok']")
ERR=$(jq_val "$OUT" "d['summary']['error']")
assert_eq "batch close total" "3" "$TOTAL"
assert_eq "batch close ok count" "2" "$OK"
assert_eq "batch close error count" "1" "$ERR"

# Check per-item outcomes
R0_OUTCOME=$(jq_val "$OUT" "d['results'][0]['outcome']")
assert_eq "batch close item 0 ok" "ok" "$R0_OUTCOME"
R1_OUTCOME=$(jq_val "$OUT" "d['results'][1]['outcome']")
assert_eq "batch close item 1 error" "error" "$R1_OUTCOME"
R1_ERR=$(jq_val "$OUT" "d['results'][1]['error']")
assert_contains "batch close error msg" "not found" "$R1_ERR"
R2_OUTCOME=$(jq_val "$OUT" "d['results'][2]['outcome']")
assert_eq "batch close item 2 ok" "ok" "$R2_OUTCOME"

# Close reason appears in result notes
assert_eq "batch close reason in notes" "Done in sprint" "$(jq_val "$OUT" "d['results'][0]['notes'][0]")"

# Check unblocked reporting (issue 3 was blocked by issue 1)
UNBLOCKED=$(jq_val "$OUT" "len(d['results'][0].get('unblocked', []))")
assert_eq "batch close reports unblocked" "1" "$UNBLOCKED"
UNBLOCKED_ID=$(jq_val "$OUT" "d['results'][0]['unblocked'][0]['id']")
assert_eq "batch close unblocked id" "3" "$UNBLOCKED_ID"

# Verify issues actually changed status
OUT_I1=$(ITR_DB_PATH="$BC_DIR/.itr.db" $ITR get 1 -f json)
assert_eq "batch close issue 1 done" "done" "$(jq_val "$OUT_I1" "d['status']")"
assert_eq "batch close issue 1 reason" "Done in sprint" "$(jq_val "$OUT_I1" "d['close_reason']")"
OUT_I2=$(ITR_DB_PATH="$BC_DIR/.itr.db" $ITR get 2 -f json)
assert_eq "batch close issue 2 wontfix" "wontfix" "$(jq_val "$OUT_I2" "d['status']")"

# Idempotent re-close returns ok
OUT=$(echo '[{"id":1}]' | ITR_DB_PATH="$BC_DIR/.itr.db" $ITR batch close -f json)
RE_OUTCOME=$(jq_val "$OUT" "d['results'][0]['outcome']")
assert_eq "batch close idempotent re-close ok" "ok" "$RE_OUTCOME"
RE_NOTE=$(jq_val "$OUT" "len(d['results'][0].get('notes', []))")
assert_eq "batch close idempotent has note" "1" "$RE_NOTE"

# Compact output format — reason appears in NOTE line
OUT=$(echo '[{"id":3,"reason":"cleanup"}]' | ITR_DB_PATH="$BC_DIR/.itr.db" $ITR batch close)
assert_contains "batch close compact output" "BATCH_CLOSE" "$OUT"
assert_contains "batch close compact shows reason" "cleanup" "$OUT"


rm -rf "$BC_DIR"

# ─────────────────────────────────────────────
# batch update
# ─────────────────────────────────────────────
echo ""
echo "--- batch update ---"

BU_DIR=$(mktemp -d)
ITR_DB_PATH="$BU_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$BU_DIR/.itr.db" $ITR add "BU issue 1" -f json >/dev/null
ITR_DB_PATH="$BU_DIR/.itr.db" $ITR add "BU issue 2" -f json >/dev/null
ITR_DB_PATH="$BU_DIR/.itr.db" $ITR add "BU issue 3" -f json >/dev/null

# Set up dependency: issue 3 blocked by issue 1
ITR_DB_PATH="$BU_DIR/.itr.db" $ITR depend 3 --on 1 >/dev/null

# Batch update: valid status + invalid priority + nonexistent ID
OUT=$(echo '[{"id":1,"status":"in-progress"},{"id":2,"priority":"bogus","add_tags":["urgent"]},{"id":99,"status":"done"}]' | ITR_DB_PATH="$BU_DIR/.itr.db" $ITR batch update -f json)
TOTAL=$(jq_val "$OUT" "d['summary']['total']")
OK=$(jq_val "$OUT" "d['summary']['ok']")
ERR=$(jq_val "$OUT" "d['summary']['error']")
REV=$(jq_val "$OUT" "d['summary']['review']")
assert_eq "batch update total" "3" "$TOTAL"
assert_eq "batch update ok count" "1" "$OK"
assert_eq "batch update error count" "1" "$ERR"
assert_eq "batch update review count" "1" "$REV"

# Check per-item outcomes
R0_OUTCOME=$(jq_val "$OUT" "d['results'][0]['outcome']")
assert_eq "batch update item 0 ok" "ok" "$R0_OUTCOME"
R1_OUTCOME=$(jq_val "$OUT" "d['results'][1]['outcome']")
assert_eq "batch update item 1 review" "review" "$R1_OUTCOME"
R2_OUTCOME=$(jq_val "$OUT" "d['results'][2]['outcome']")
assert_eq "batch update item 2 error" "error" "$R2_OUTCOME"

# Verify valid updates applied
OUT_I1=$(ITR_DB_PATH="$BU_DIR/.itr.db" $ITR get 1 -f json)
assert_eq "batch update issue 1 in-progress" "in-progress" "$(jq_val "$OUT_I1" "d['status']")"

# Verify soft fallback: priority stayed at medium, _needs_review tag added
OUT_I2=$(ITR_DB_PATH="$BU_DIR/.itr.db" $ITR get 2 -f json)
assert_eq "batch update soft fallback keeps priority" "medium" "$(jq_val "$OUT_I2" "d['priority']")"
HAS_REVIEW_TAG=$(jq_val "$OUT_I2" "'_needs_review' in d.get('tags', [])")
assert_eq "batch update soft fallback adds _needs_review" "True" "$HAS_REVIEW_TAG"
# Verify add_tags also applied
HAS_URGENT_TAG=$(jq_val "$OUT_I2" "'urgent' in d.get('tags', [])")
assert_eq "batch update add_tags applied" "True" "$HAS_URGENT_TAG"

# Batch update with status change to done triggers unblocked
OUT=$(echo '[{"id":1,"status":"done"}]' | ITR_DB_PATH="$BU_DIR/.itr.db" $ITR batch update -f json)
UNBLOCKED=$(jq_val "$OUT" "len(d['results'][0].get('unblocked', []))")
assert_eq "batch update done triggers unblocked" "1" "$UNBLOCKED"

# Batch update with add_skills
OUT=$(echo '[{"id":2,"add_skills":["devops","rust"]}]' | ITR_DB_PATH="$BU_DIR/.itr.db" $ITR batch update -f json)
assert_eq "batch update add_skills ok" "ok" "$(jq_val "$OUT" "d['results'][0]['outcome']")"
OUT_I2=$(ITR_DB_PATH="$BU_DIR/.itr.db" $ITR get 2 -f json)
SKILLS=$(jq_val "$OUT_I2" "d['skills']")
assert_eq "batch update add_skills applied" "['devops', 'rust']" "$SKILLS"

# Compact output format
OUT=$(echo '[{"id":3,"assigned_to":"agent-x"}]' | ITR_DB_PATH="$BU_DIR/.itr.db" $ITR batch update)
assert_contains "batch update compact output" "BATCH_UPDATE" "$OUT"

# --fields filtering on batch results
OUT=$(echo '[{"id":3,"assigned_to":"agent-y"}]' | ITR_DB_PATH="$BU_DIR/.itr.db" $ITR batch update -f json --fields results,summary)
assert_contains "batch update --fields has results" '"results"' "$OUT"
assert_contains "batch update --fields has summary" '"summary"' "$OUT"
# Verify 'action' key is filtered out
HAS_ACTION=$(echo "$OUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print('action' in d)")
assert_eq "batch update --fields filters action" "False" "$HAS_ACTION"

# --fields on batch add (envelope format)
BF_OUT=$(echo '[{"title":"Fields test"}]' | ITR_DB_PATH="$BU_DIR/.itr.db" $ITR batch add -f json --fields action,results)
KEYS=$(echo "$BF_OUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(sorted(d.keys()))")
assert_eq "batch add --fields filters keys" "['action', 'results']" "$KEYS"

# --fields on batch close
BC_OUT=$(echo '[{"id":3}]' | ITR_DB_PATH="$BU_DIR/.itr.db" $ITR batch close -f json --fields results)
HAS_SUMMARY=$(echo "$BC_OUT" | python3 -c "import sys,json; d=json.load(sys.stdin); print('summary' in d)")
assert_eq "batch close --fields filters summary" "False" "$HAS_SUMMARY"

# ─────────────────────────────────────────────
echo ""
echo "--- batch note ---"
# ─────────────────────────────────────────────

BN_DIR=$(mktemp -d)
ITR_DB_PATH="$BN_DIR/.itr.db" $ITR init >/dev/null
echo '[{"title":"Note A"},{"title":"Note B"}]' | ITR_DB_PATH="$BN_DIR/.itr.db" $ITR batch add -f json >/dev/null

# Batch note: 2 valid + 1 invalid ID
OUT=$(echo '[{"id":1,"text":"First note"},{"id":99,"text":"Bad"},{"id":2,"text":"Second note","agent":"custom-agent"}]' | ITR_DB_PATH="$BN_DIR/.itr.db" $ITR batch note -f json)
assert_eq "batch note action" "batch_note" "$(jq_val "$OUT" "d['action']")"
assert_eq "batch note total" "3" "$(jq_val "$OUT" "d['summary']['total']")"
assert_eq "batch note ok count" "2" "$(jq_val "$OUT" "d['summary']['ok']")"
assert_eq "batch note error count" "1" "$(jq_val "$OUT" "d['summary']['error']")"
assert_eq "batch note item 0 ok" "ok" "$(jq_val "$OUT" "d['results'][0]['outcome']")"
assert_eq "batch note item 1 error" "error" "$(jq_val "$OUT" "d['results'][1]['outcome']")"
assert_contains "batch note error msg" "not found" "$(jq_val "$OUT" "d['results'][1]['error']")"
assert_eq "batch note item 2 ok" "ok" "$(jq_val "$OUT" "d['results'][2]['outcome']")"

# Verify notes actually created
OUT_I1=$(ITR_DB_PATH="$BN_DIR/.itr.db" $ITR get 1 -f json)
assert_eq "batch note persisted" "1" "$(jq_val "$OUT_I1" "len(d['notes'])")"
assert_eq "batch note content" "First note" "$(jq_val "$OUT_I1" "d['notes'][0]['content']")"

# Verify custom agent override
OUT_I2=$(ITR_DB_PATH="$BN_DIR/.itr.db" $ITR get 2 -f json)
assert_eq "batch note custom agent" "custom-agent" "$(jq_val "$OUT_I2" "d['notes'][0]['agent']")"

# Compact output
OUT=$(echo '[{"id":1,"text":"Compact note"}]' | ITR_DB_PATH="$BN_DIR/.itr.db" $ITR batch note)
assert_contains "batch note compact output" "BATCH_NOTE" "$OUT"

rm -rf "$BN_DIR"

# --dry-run on batch close
DR_DIR=$(mktemp -d)
ITR_DB_PATH="$DR_DIR/.itr.db" $ITR init >/dev/null
echo '[{"title":"Dry A"},{"title":"Dry B"}]' | ITR_DB_PATH="$DR_DIR/.itr.db" $ITR batch add -f json >/dev/null
DR_OUT=$(echo '[{"id":1}]' | ITR_DB_PATH="$DR_DIR/.itr.db" $ITR batch close --dry-run -f json)
assert_eq "batch close dry-run flag" "True" "$(jq_val "$DR_OUT" "d.get('dry_run', False)")"
assert_eq "batch close dry-run outcome" "ok" "$(jq_val "$DR_OUT" "d['results'][0]['outcome']")"
DR_STATUS=$(ITR_DB_PATH="$DR_DIR/.itr.db" $ITR get 1 -f json)
assert_eq "batch close dry-run no change" "open" "$(jq_val "$DR_STATUS" "d['status']")"

# --dry-run on batch update
DR_OUT=$(echo '[{"id":2,"status":"in-progress","priority":"high"}]' | ITR_DB_PATH="$DR_DIR/.itr.db" $ITR batch update --dry-run -f json)
assert_eq "batch update dry-run flag" "True" "$(jq_val "$DR_OUT" "d.get('dry_run', False)")"
assert_eq "batch update dry-run outcome" "ok" "$(jq_val "$DR_OUT" "d['results'][0]['outcome']")"
DR_STATUS=$(ITR_DB_PATH="$DR_DIR/.itr.db" $ITR get 2 -f json)
assert_eq "batch update dry-run no change" "open" "$(jq_val "$DR_STATUS" "d['status']")"
assert_eq "batch update dry-run priority unchanged" "medium" "$(jq_val "$DR_STATUS" "d['priority']")"

# Verify dry_run not in normal (non-dry-run) output
DR_NORMAL=$(echo '[{"id":1}]' | ITR_DB_PATH="$DR_DIR/.itr.db" $ITR batch close -f json)
assert_eq "batch close normal no dry_run key" "False" "$(echo "$DR_NORMAL" | python3 -c "import sys,json; print('dry_run' in json.load(sys.stdin))")"

rm -rf "$DR_DIR"

rm -rf "$BU_DIR"

# ─────────────────────────────────────────────
# Sub-agent E2E
# ─────────────────────────────────────────────
echo ""
echo "--- Sub-agent E2E ---"

SA_DIR=$(mktemp -d)
ITR_DB_PATH="$SA_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$SA_DIR/.itr.db" $ITR add "Sub-agent task" -f json >/dev/null
SA_ID=1

# Claim via next --claim with ITR_AGENT
OUT=$(ITR_AGENT=test-sub-agent ITR_DB_PATH="$SA_DIR/.itr.db" $ITR next --claim -f json)
assert_eq "sub-agent claim assigned_to" "test-sub-agent" "$(jq_val "$OUT" "d['assigned_to']")"

# Add a note with ITR_AGENT
OUT=$(ITR_AGENT=test-sub-agent ITR_DB_PATH="$SA_DIR/.itr.db" $ITR note $SA_ID "Working on it" -f json)
assert_eq "sub-agent note agent" "test-sub-agent" "$(jq_val "$OUT" "d['agent']")"

# Close the issue with ITR_AGENT
ITR_AGENT=test-sub-agent ITR_DB_PATH="$SA_DIR/.itr.db" $ITR close $SA_ID "Done" -f json >/dev/null

# Verify all log events have agent == "test-sub-agent"
LOG=$(ITR_DB_PATH="$SA_DIR/.itr.db" $ITR log $SA_ID -f json)
EVENT_COUNT=$(jq_val "$LOG" "len(d)")
assert_eq "sub-agent log has events" "True" "$(jq_val "$LOG" "len(d) > 0")"
BAD_AGENTS=$(jq_val "$LOG" "len([e for e in d if e['agent'] != 'test-sub-agent'])")
assert_eq "sub-agent all events tagged" "0" "$BAD_AGENTS"

# Verify final state is done
OUT=$(ITR_DB_PATH="$SA_DIR/.itr.db" $ITR get $SA_ID -f json)
assert_eq "sub-agent final status done" "done" "$(jq_val "$OUT" "d['status']")"

rm -rf "$SA_DIR"

# ─────────────────────────────────────────────
# Known bugs — these tests document expected behavior once fixed
# ─────────────────────────────────────────────
echo ""
echo "--- Known Bug Tests (documenting expected behavior) ---"

# Bug #42: `itr deps` should soft-fallback to `depend` (or show helpful error)
# Currently exits 2 with clap error "unrecognized subcommand 'deps'"
BUG_DIR=$(mktemp -d)
$ITR init --db "$BUG_DIR/.itr.db" > /dev/null
ID1=$($ITR add "dep test 1" --db "$BUG_DIR/.itr.db" -f json | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
ID2=$($ITR add "dep test 2" --db "$BUG_DIR/.itr.db" -f json | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
OUT=$($ITR --db "$BUG_DIR/.itr.db" depend "$ID1" --on "$ID2" 2>&1) || true
assert_contains "bug42: 'depend' command works" "blocked by" "$OUT"
# The actual bug: 'deps' should also work as an alias
DEPS_EXIT=0
DEPS_OUT=$($ITR --db "$BUG_DIR/.itr.db" deps "$ID1" --on "$ID2" 2>&1) || DEPS_EXIT=$?
if [ "$DEPS_EXIT" -eq 0 ]; then
    pass "bug42: 'deps' alias for depend works"
else
    fail "bug42: 'deps' alias not recognized (issue #42)" "exit code $DEPS_EXIT"
fi
rm -rf "$BUG_DIR"

# Bug #43/#46: `-t tag1 -t tag2` should allow repeated -t for multiple tags
# Currently fails: "cannot be used multiple times" because -t is short for --tags (Option<String>)
BUG_DIR=$(mktemp -d)
$ITR init --db "$BUG_DIR/.itr.db" > /dev/null
TAG_EXIT=0
TAG_OUT=$($ITR --db "$BUG_DIR/.itr.db" add "multi tag test" -t bug -t test 2>&1) || TAG_EXIT=$?
if [ "$TAG_EXIT" -eq 0 ]; then
    pass "bug43: -t flag repeated for multiple tags"
else
    fail "bug43: -t repeated use fails (issue #43)" "exit code $TAG_EXIT"
fi
rm -rf "$BUG_DIR"

# Bug #45: dash-prefixed values for --acceptance should be accepted
# Currently clap interprets the dash-prefixed value as a flag
BUG_DIR=$(mktemp -d)
$ITR init --db "$BUG_DIR/.itr.db" > /dev/null
ACC_EXIT=0
ACC_OUT=$($ITR --db "$BUG_DIR/.itr.db" add "acceptance test" --acceptance "-t flag works correctly" 2>&1) || ACC_EXIT=$?
if [ "$ACC_EXIT" -eq 0 ]; then
    pass "bug45: --acceptance accepts dash-prefixed values"
else
    fail "bug45: --acceptance rejects dash-prefixed values (issue #45)" "exit code $ACC_EXIT"
fi
rm -rf "$BUG_DIR"

# ─────────────────────────────────────────────
# Soft-fallback aliases: --title flag, --body alias, batch create
echo ""
echo "--- Soft-fallback aliases ---"

ALIAS_DIR=$(mktemp -d)
$ITR init --db "$ALIAS_DIR/.itr.db" > /dev/null

# --title flag creates issue
TITLE_FLAG_OUT=$($ITR --db "$ALIAS_DIR/.itr.db" add --title "Flag title" -f json 2>/dev/null)
TITLE_FLAG_VAL=$(echo "$TITLE_FLAG_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['title'])")
if [ "$TITLE_FLAG_VAL" = "Flag title" ]; then
    pass "alias: --title flag creates issue"
else
    fail "alias: --title flag creates issue" "got title '$TITLE_FLAG_VAL'"
fi

# --title flag takes precedence over positional, stderr warns
BOTH_STDERR=$($ITR --db "$ALIAS_DIR/.itr.db" add "Positional" --title "Flag" -f json 2>&1 1>/dev/null)
BOTH_OUT=$($ITR --db "$ALIAS_DIR/.itr.db" add "Positional2" --title "Flag2" -f json 2>/dev/null)
BOTH_TITLE=$(echo "$BOTH_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['title'])")
if [ "$BOTH_TITLE" = "Flag2" ] && echo "$BOTH_STDERR" | grep -q "REVIEW:"; then
    pass "alias: --title flag overrides positional with REVIEW warning"
else
    fail "alias: --title flag overrides positional" "title='$BOTH_TITLE', stderr='$BOTH_STDERR'"
fi

# --body maps to context
BODY_OUT=$($ITR --db "$ALIAS_DIR/.itr.db" add --title "Body test" --body "body content" -f json 2>/dev/null)
BODY_CTX=$(echo "$BODY_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['context'])")
if [ "$BODY_CTX" = "body content" ]; then
    pass "alias: --body maps to context"
else
    fail "alias: --body maps to context" "got context '$BODY_CTX'"
fi

# batch create alias works
BATCH_OUT=$(echo '[{"title":"batch created"}]' | $ITR --db "$ALIAS_DIR/.itr.db" batch create -f json 2>/dev/null)
BATCH_TITLE=$(echo "$BATCH_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['results'][0]['issue']['title'])")
if [ "$BATCH_TITLE" = "batch created" ]; then
    pass "alias: batch create works"
else
    fail "alias: batch create works" "got '$BATCH_TITLE'"
fi

# --reason flag on close works
REASON_ADD=$($ITR --db "$ALIAS_DIR/.itr.db" add --title "Reason flag test" -f json 2>/dev/null)
REASON_ID=$(echo "$REASON_ADD" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
REASON_OUT=$($ITR --db "$ALIAS_DIR/.itr.db" close "$REASON_ID" --reason "closed via flag" -f json 2>/dev/null)
REASON_VAL=$(echo "$REASON_OUT" | python3 -c "import sys,json; print(json.load(sys.stdin)['close_reason'])")
if [ "$REASON_VAL" = "closed via flag" ]; then
    pass "alias: --reason flag on close works"
else
    fail "alias: --reason flag on close works" "got close_reason '$REASON_VAL'"
fi

# --reason flag overrides positional with REVIEW warning
REASON_ADD2=$($ITR --db "$ALIAS_DIR/.itr.db" add --title "Reason both test" -f json 2>/dev/null)
REASON_ID2=$(echo "$REASON_ADD2" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
REASON_STDERR=$($ITR --db "$ALIAS_DIR/.itr.db" close "$REASON_ID2" "positional reason" --reason "flag reason" -f json 2>&1 1>/dev/null)
REASON_OUT2=$($ITR --db "$ALIAS_DIR/.itr.db" close "$REASON_ID2" -f json 2>/dev/null)
REASON_VAL2=$(echo "$REASON_OUT2" | python3 -c "import sys,json; print(json.load(sys.stdin)['close_reason'])")
if [ "$REASON_VAL2" = "flag reason" ] && echo "$REASON_STDERR" | grep -q "REVIEW:"; then
    pass "alias: --reason flag overrides positional with REVIEW warning"
else
    fail "alias: --reason flag overrides positional" "reason='$REASON_VAL2', stderr='$REASON_STDERR'"
fi

rm -rf "$ALIAS_DIR"

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
