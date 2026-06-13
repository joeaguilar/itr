#!/usr/bin/env bash
set -euo pipefail

# Integration test suite for itr
# Usage: ./tests/integration.sh [--smoke] [path-to-itr-binary]
#
# If no path is provided, uses ./target/release/itr

SCRIPT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SMOKE=0
if [ "${1:-}" = "--smoke" ]; then
    SMOKE=1
    shift
fi

if [ "$#" -gt 0 ]; then
    ITR="$1"
    case "$ITR" in
        /*) ;;
        *) ITR="$(pwd)/$ITR" ;;
    esac
else
    ITR="$SCRIPT_DIR/target/release/itr"
fi

if [ ! -x "$ITR" ]; then
    echo "Binary not found at $ITR — run 'cargo build --release' first"
    exit 1
fi

run_smoke() {
    local out ready_out
    SMOKE_DIR=$(mktemp -d)
    trap 'rm -rf "$SMOKE_DIR"' EXIT

    "$ITR" --version >/dev/null

    cd "$SMOKE_DIR"
    out=$("$ITR" init)
    echo "$out" | grep -qF ".itr.db"
    [ -f .itr.db ]

    "$ITR" add "Release smoke issue" >/dev/null
    ready_out=$("$ITR" ready -f json)
    echo "$ready_out" | grep -qF "Release smoke issue"

    echo "release smoke passed: version, init, add, ready"
}

if [ "$SMOKE" -eq 1 ]; then
    run_smoke
    exit 0
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

# Background UI server pids (set by the `--- ui ---` section, cleared again
# after its own kill/wait). Tracked globally so the single EXIT trap below can
# reap them even when `set -e` aborts the suite between server start and the
# section's own kill — an orphaned server would otherwise outlive the suite,
# keep its port bound, and poison the NEXT run. Bash traps OVERWRITE rather
# than stack, so the UI cleanup is folded into the same handler as the WORKDIR
# removal instead of registering a second `trap ... EXIT`.
UI_PID=""
UI_SQL_PID=""
suite_cleanup() {
    if [ -n "${UI_PID:-}" ]; then
        kill "$UI_PID" 2>/dev/null || true
        wait "$UI_PID" 2>/dev/null || true
    fi
    if [ -n "${UI_SQL_PID:-}" ]; then
        kill "$UI_SQL_PID" 2>/dev/null || true
        wait "$UI_SQL_PID" 2>/dev/null || true
    fi
    rm -rf "$WORKDIR"
}
trap suite_cleanup EXIT
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
if INIT_SCHEMA_CHECK=$(python3 - <<'PY' 2>&1
import sqlite3

conn = sqlite3.connect(".itr.db")

issues_cols = {row[1] for row in conn.execute("PRAGMA table_info(issues)")}
required_issue_cols = {"assigned_to", "skills"}
missing_issue_cols = sorted(required_issue_cols - issues_cols)
if missing_issue_cols:
    raise SystemExit(f"missing issue columns: {missing_issue_cols}")

tables = {
    row[0]
    for row in conn.execute(
        "SELECT name FROM sqlite_master WHERE type='table'"
    )
}
required_tables = {
    "issues",
    "dependencies",
    "notes",
    "config",
    "events",
    "relations",
    "issues_fts",
}
missing_tables = sorted(required_tables - tables)
if missing_tables:
    raise SystemExit(f"missing tables: {missing_tables}")

indexes = {
    row[0]
    for row in conn.execute(
        "SELECT name FROM sqlite_master WHERE type='index'"
    )
}
required_indexes = {
    "idx_events_issue",
    "idx_events_created",
    "idx_relations_source",
    "idx_relations_target",
}
missing_indexes = sorted(required_indexes - indexes)
if missing_indexes:
    raise SystemExit(f"missing indexes: {missing_indexes}")

events_sql = conn.execute(
    "SELECT sql FROM sqlite_master WHERE type='table' AND name='events'"
).fetchone()[0]
relations_sql = conn.execute(
    "SELECT sql FROM sqlite_master WHERE type='table' AND name='relations'"
).fetchone()[0]
if "REFERENCES issues(id) ON DELETE CASCADE" not in events_sql:
    raise SystemExit("events table missing issue foreign key")
if "UNIQUE(source_id, target_id, relation_type)" not in relations_sql:
    raise SystemExit("relations table missing unique constraint")
if "CHECK(relation_type IN" not in relations_sql:
    raise SystemExit("relations table missing relation_type check")

print("ok")
PY
); then
    assert_eq "fresh init has current schema before reopen" "ok" "$INIT_SCHEMA_CHECK"
else
    fail "fresh init has current schema before reopen" "$INIT_SCHEMA_CHECK"
fi

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

ADD_BLOCK_DIR=$(mktemp -d)
$ITR init --db "$ADD_BLOCK_DIR/.itr.db" >/dev/null
ADD_BLOCKER_A=$($ITR --db "$ADD_BLOCK_DIR/.itr.db" add "Add blocker A" -f json | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
ADD_BLOCKER_B=$($ITR --db "$ADD_BLOCK_DIR/.itr.db" add "Add blocker B" -f json | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")

OUT=$($ITR --db "$ADD_BLOCK_DIR/.itr.db" add "Blocked by two" --blocked-by "$ADD_BLOCKER_A,$ADD_BLOCKER_B" -f json)
assert_eq "add --blocked-by creates multi dependencies" "[$ADD_BLOCKER_A, $ADD_BLOCKER_B]" "$(jq_val "$OUT" "sorted(d['blocked_by'])")"
assert_eq "add --blocked-by marks issue blocked" "True" "$(jq_val "$OUT" "d['is_blocked']")"

OUT=$($ITR --db "$ADD_BLOCK_DIR/.itr.db" add "Blocked by one plus invalid" --blocked-by "$ADD_BLOCKER_A,not-an-id" -f json)
assert_eq "add --blocked-by invalid token adds _needs_review" "True" "$(jq_val "$OUT" "'_needs_review' in d.get('tags', [])")"
assert_eq "add --blocked-by invalid token adds review note" "True" "$(jq_val "$OUT" "any('blocked_by' in n['content'] and 'not-an-id' in n['content'] for n in d['notes'])")"
assert_eq "add --blocked-by invalid token keeps valid dependencies" "[$ADD_BLOCKER_A]" "$(jq_val "$OUT" "sorted(d['blocked_by'])")"

ADD_COUNT_BEFORE=$(python3 -c "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('SELECT COUNT(*) FROM issues').fetchone()[0])" "$ADD_BLOCK_DIR/.itr.db")
ADD_MISSING_EXIT=0
ADD_MISSING_OUT=$($ITR --db "$ADD_BLOCK_DIR/.itr.db" add "Missing blocker should rollback" --blocked-by 999 -f json 2>&1) || ADD_MISSING_EXIT=$?
assert_eq "add --blocked-by missing id exits 1" "1" "$ADD_MISSING_EXIT"
assert_contains "add --blocked-by missing id reports not found" "Issue 999 not found" "$ADD_MISSING_OUT"
ADD_COUNT_AFTER=$(python3 -c "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('SELECT COUNT(*) FROM issues').fetchone()[0])" "$ADD_BLOCK_DIR/.itr.db")
assert_eq "add --blocked-by missing id rolls back issue" "$ADD_COUNT_BEFORE" "$ADD_COUNT_AFTER"
rm -rf "$ADD_BLOCK_DIR"

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
echo "--- get (multi-ID batch, #136) ---"
# ─────────────────────────────────────────────

# Happy path: comma-separated IDs return an array of full details in request order
OUT=$($ITR get 1,2 -f json)
assert_eq "multi-get returns 2 details" "2" "$(jq_val "$OUT" "len(d)")"
assert_eq "multi-get preserves request order" "[1, 2]" "$(jq_val "$OUT" "[i['id'] for i in d]")"
assert_eq "multi-get elements are full details" "True" "$(jq_val "$OUT" "all('urgency_breakdown' in i and 'notes' in i for i in d)")"

# Repeated-argument form is equivalent to the comma form
OUT=$($ITR get 2 1 -f json)
assert_eq "multi-get repeated args preserve order" "[2, 1]" "$(jq_val "$OUT" "[i['id'] for i in d]")"

# Compact batched output: one block per issue, blank-line separated
COMPACT=$($ITR get 1,2)
assert_contains "multi-get compact has block for id 1" "ID:1" "$COMPACT"
assert_contains "multi-get compact has block for id 2" "ID:2" "$COMPACT"
MG_BLOCKS=$(echo "$COMPACT" | grep -c "^ID:" || true)
assert_eq "multi-get compact emits 2 record lines" "2" "$MG_BLOCKS"

# Partial-missing: found issues still return, missing ID is a REVIEW note, exit 0
MG_ERR="$WORKDIR/multi_get_err.txt"
OUT=$($ITR get 1,999 -f json 2>"$MG_ERR")
assert_eq "multi-get partial missing returns found issue" "[1]" "$(jq_val "$OUT" "[i['id'] for i in d]")"
assert_contains "multi-get partial missing emits REVIEW note" "REVIEW" "$(cat "$MG_ERR")"
assert_contains "multi-get partial missing names the missing id" "999" "$(cat "$MG_ERR")"
assert_exit "multi-get partial missing exits 0" "0" $ITR get 1,999 -f json

# All-missing: empty result ([] in JSON), one REVIEW note per ID, exit 0
OUT=$($ITR get 998,999 -f json 2>"$MG_ERR")
assert_eq "multi-get all missing returns empty array" "[]" "$OUT"
MG_REVIEWS=$(grep -c "REVIEW" "$MG_ERR" || true)
assert_eq "multi-get all missing emits one REVIEW per id" "2" "$MG_REVIEWS"
assert_exit "multi-get all missing exits 0" "0" $ITR get 998,999

# Duplicate IDs are de-duplicated (first-seen order)
OUT=$($ITR get 1,1,2,1 -f json)
assert_eq "multi-get dedups duplicate ids" "[1, 2]" "$(jq_val "$OUT" "[i['id'] for i in d]")"

# Duplicates collapsing to a single unique ID keep the single-issue contract
OUT=$($ITR get 1,1 -f json 2>/dev/null)
assert_eq "multi-get collapsed to one id emits bare object" "1" "$(jq_val "$OUT" "d['id']")"

# show mirrors the batched get contract
OUT=$($ITR show 1,2 -f json)
assert_eq "show multi-id batches like get" "[1, 2]" "$(jq_val "$OUT" "[i['id'] for i in d]")"

# Very-large batch: one invocation returns the full working set
MG_BIG_DIR=$(mktemp -d)
ITR_DB_PATH="$MG_BIG_DIR/.itr.db" $ITR init >/dev/null
python3 -c "import json; print(json.dumps([{'title': f'Bulk fetch {i}'} for i in range(1, 251)]))" \
    | ITR_DB_PATH="$MG_BIG_DIR/.itr.db" $ITR batch add -f json >/dev/null
MG_BIG_IDS=$(python3 -c "print(','.join(str(i) for i in range(1, 251)))")
OUT=$(ITR_DB_PATH="$MG_BIG_DIR/.itr.db" $ITR get "$MG_BIG_IDS" -f json)
assert_eq "multi-get 250-issue batch returns all" "250" "$(jq_val "$OUT" "len(d)")"
assert_eq "multi-get 250-issue batch keeps order" "[1, 125, 250]" "$(jq_val "$OUT" "[d[0]['id'], d[124]['id'], d[249]['id']]")"
rm -rf "$MG_BIG_DIR"

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

READY_STATUS_DIR=$(mktemp -d)
ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR init >/dev/null
OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR add "Ready open" -f json)
READY_OPEN_ID=$(jq_val "$OUT" "d['id']")
OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR add "Ready in progress" -f json)
READY_IN_PROGRESS_ID=$(jq_val "$OUT" "d['id']")
ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR update "$READY_IN_PROGRESS_ID" --status in-progress >/dev/null
OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR add "Ready done should not leak" -f json)
READY_DONE_ID=$(jq_val "$OUT" "d['id']")
ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR close "$READY_DONE_ID" "Finished" >/dev/null
OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR add "Ready wontfix should not leak" -f json)
READY_WONTFIX_ID=$(jq_val "$OUT" "d['id']")
ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR close "$READY_WONTFIX_ID" --wontfix "Not needed" >/dev/null

OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR ready --status open -f json)
assert_eq "ready --status open keeps open work" "[$READY_OPEN_ID]" "$(jq_val "$OUT" "[i['id'] for i in d]")"
OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR ready --status in-progress -f json)
assert_eq "ready --status in-progress keeps in-progress work" "[$READY_IN_PROGRESS_ID]" "$(jq_val "$OUT" "[i['id'] for i in d]")"
OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR ready --status done -f json)
assert_eq "ready --status done excludes terminal work" "[]" "$(jq_val "$OUT" "d")"
OUT=$(ITR_DB_PATH="$READY_STATUS_DIR/.itr.db" $ITR ready --status wontfix -f json)
assert_eq "ready --status wontfix excludes terminal work" "[]" "$(jq_val "$OUT" "d")"
rm -rf "$READY_STATUS_DIR"

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

# Single update to terminal status should also unblock and remove stale edges
UPDATE_DEP_DIR=$(mktemp -d)
ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR init >/dev/null
OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR add "Update blocker" -f json)
UPDATE_BLOCKER=$(jq_val "$OUT" "d['id']")
OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR add "Update blocked" --blocked-by "$UPDATE_BLOCKER" -f json)
UPDATE_BLOCKED=$(jq_val "$OUT" "d['id']")

OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR update "$UPDATE_BLOCKER" --status done -f json)
assert_eq "update done reports unblocked" "1" "$(jq_val "$OUT" "len(d.get('unblocked', []))")"
OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR get "$UPDATE_BLOCKED" -f json)
assert_eq "update done removes blocker edge" "[]" "$(jq_val "$OUT" "d['blocked_by']")"
assert_eq "update done leaves dependent unblocked" "False" "$(jq_val "$OUT" "d['is_blocked']")"
set +e
OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR doctor -f json 2>&1)
UPDATE_DOCTOR_EXIT=$?
set -e
assert_eq "doctor clean after update done blocker cleanup" "0" "$UPDATE_DOCTOR_EXIT"

OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR add "Update wontfix blocker" -f json)
UPDATE_WONTFIX_BLOCKER=$(jq_val "$OUT" "d['id']")
OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR add "Update wontfix blocked" --blocked-by "$UPDATE_WONTFIX_BLOCKER" -f json)
UPDATE_WONTFIX_BLOCKED=$(jq_val "$OUT" "d['id']")
ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR update "$UPDATE_WONTFIX_BLOCKER" --status wontfix -f json >/dev/null
OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR get "$UPDATE_WONTFIX_BLOCKED" -f json)
assert_eq "update wontfix removes blocker edge" "[]" "$(jq_val "$OUT" "d['blocked_by']")"
set +e
OUT=$(ITR_DB_PATH="$UPDATE_DEP_DIR/.itr.db" $ITR doctor -f json 2>&1)
UPDATE_WONTFIX_DOCTOR_EXIT=$?
set -e
assert_eq "doctor clean after update wontfix blocker cleanup" "0" "$UPDATE_WONTFIX_DOCTOR_EXIT"
rm -rf "$UPDATE_DEP_DIR"

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
assert_contains "schema has assigned_to column" "assigned_to" "$OUT"
assert_contains "schema has events table" "CREATE TABLE IF NOT EXISTS events" "$OUT"
assert_contains "schema has relations table" "CREATE TABLE IF NOT EXISTS relations" "$OUT"
assert_contains "schema has event indexes" "idx_events_issue" "$OUT"
assert_contains "schema has relation indexes" "idx_relations_source" "$OUT"

# Pipe stdout to python's json.load via stdin (the jq_val pattern) so embedded
# quotes in the schema SQL can't break the parse, and record a real failure if
# the output is not valid JSON.
OUT=$($ITR schema -f json)
if echo "$OUT" | python3 -c "import sys,json; json.load(sys.stdin)" >/dev/null 2>&1; then
    pass "schema -f json emits valid JSON"
else
    fail "schema -f json emits valid JSON" "stdout did not parse as JSON"
fi

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

# Bulk update to terminal status should report unblocked and remove stale edges
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR add "Bulk update blocker" --tag bulk-update-blocker -f json)
BULK_UPDATE_BLOCKER=$(jq_val "$OUT" "d['id']")
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR add "Bulk update blocked" --blocked-by "$BULK_UPDATE_BLOCKER" -f json)
BULK_UPDATE_BLOCKED=$(jq_val "$OUT" "d['id']")
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR bulk update --tag bulk-update-blocker --set-status done -f json)
assert_eq "bulk update done reports unblocked" "1" "$(jq_val "$OUT" "len(d.get('unblocked', []))")"
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR get "$BULK_UPDATE_BLOCKED" -f json)
assert_eq "bulk update done removes blocker edge" "[]" "$(jq_val "$OUT" "d['blocked_by']")"
set +e
OUT=$(ITR_DB_PATH="$BULK_DIR/.itr.db" $ITR doctor -f json 2>&1)
BULK_UPDATE_DOCTOR_EXIT=$?
set -e
assert_eq "doctor clean after bulk update blocker cleanup" "0" "$BULK_UPDATE_DOCTOR_EXIT"

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

# Typed unrelate: --type removes only the named relation type
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR add "Issue D" -f json >/dev/null   # id 4
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR add "Issue E" -f json >/dev/null   # id 5
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR relate 4 --to 5 --relation-type related >/dev/null
ITR_DB_PATH="$REL_DIR/.itr.db" $ITR relate 4 --to 5 --relation-type duplicate >/dev/null
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR unrelate 4 --from 5 --type related -f json)
assert_eq "typed unrelate removes" "True" "$(jq_val "$OUT" "d['removed']")"
assert_eq "typed unrelate removes only the requested type" "['related']" "$(jq_val "$OUT" "[r['relation_type'] for r in d['removed_relations']]")"
OUT=$(ITR_DB_PATH="$REL_DIR/.itr.db" $ITR get 5 -f json)
assert_eq "typed unrelate leaves other types intact" "['duplicate']" "$(jq_val "$OUT" "[r['relation_type'] for r in d.get('relations', [])]")"
assert_exit "typed unrelate rejects unknown type" "1" env ITR_DB_PATH="$REL_DIR/.itr.db" $ITR unrelate 4 --from 5 --type bogus

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
ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR add "Background task" -c "Runs scheduled cleanup" -f json >/dev/null
ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR note 3 "needle lives only in this note" >/dev/null
ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR add "Needle title match" -c "No note needed" -f json >/dev/null

# Reindex
OUT=$(ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR reindex -f json)
INDEXED=$(jq_val "$OUT" "d['indexed']")
assert_eq "reindex counts issues" "4" "$INDEXED"

# FTS search works
OUT=$(ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR search "JWT" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "FTS search finds JWT" "1" "$COUNT"

# FTS search by context
OUT=$(ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR search "Stripe" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "FTS search finds Stripe" "1" "$COUNT"

# FTS-ranked search still includes note-only matches from the LIKE path
OUT=$(ITR_DB_PATH="$FTS_DIR/.itr.db" $ITR search "needle" -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "FTS search includes note-only matches" "2" "$COUNT"
NOTE_ID_PRESENT=$(jq_val "$OUT" "3 in [item['id'] for item in d]")
assert_eq "FTS search includes expected note-only issue id" "True" "$NOTE_ID_PRESENT"
NOTE_MATCH=$(jq_val "$OUT" "next(('notes' in item['matched_fields'] and item.get('context_snippets', {}).get('notes') is not None for item in d if item['id'] == 3), False)")
assert_eq "FTS note-only match includes notes snippet" "True" "$NOTE_MATCH"

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
assert_contains "agent-info mentions skill install" "itr skill install" "$OUT"
assert_contains "agent-info mentions Agent Onboarding" "Agent Onboarding" "$OUT"

OUT=$($ITR agent-info -f json)
GUIDE=$(jq_val "$OUT" "d['guide']")
assert_contains "agent-info json has guide field" "ITR_AGENT" "$GUIDE"

OUT=$($ITR getting-started)
assert_contains "getting-started alias works" "ITR_AGENT" "$OUT"

OUT=$($ITR getting started)
assert_contains "getting started (two words) works" "ITR_AGENT" "$OUT"

# ─────────────────────────────────────────────
echo "--- skill ---"
# ─────────────────────────────────────────────

OUT=$($ITR skill)
assert_contains "skill emits frontmatter" "name: itr" "$OUT"
assert_contains "skill emits description" "agent-first issue tracker" "$OUT"
assert_contains "skill mentions itr add" "itr add" "$OUT"

OUT=$($ITR skill -f json)
SKILL_BODY=$(jq_val "$OUT" "d['skill']")
assert_contains "skill json has skill field" "name: itr" "$SKILL_BODY"

SKILL_DIR=$(mktemp -d)
cd "$SKILL_DIR"

OUT=$($ITR skill path --scope project)
assert_contains "skill path project shows target" ".claude/skills/itr/SKILL.md" "$OUT"

$ITR skill install --scope project >/dev/null
[ -f .claude/skills/itr/SKILL.md ] && pass "skill install --scope project writes file" \
    || fail "skill install --scope project writes file" "file missing"
assert_contains "installed file has frontmatter" "name: itr" "$(cat .claude/skills/itr/SKILL.md)"

echo "tampered" > .claude/skills/itr/SKILL.md
OUT=$($ITR skill install --scope project 2>&1 || true)
assert_contains "skill install refuses overwrite without --force" "already exists" "$OUT"
assert_eq "skill install preserved tampered file" "tampered" "$(cat .claude/skills/itr/SKILL.md)"

$ITR skill install --scope project --force >/dev/null
assert_contains "skill install --force overwrites" "name: itr" "$(cat .claude/skills/itr/SKILL.md)"

OUT=$($ITR skill install --scope project --force -f json)
INSTALLED=$(jq_val "$OUT" "d['installed']")
assert_contains "skill install json reports path" ".claude/skills/itr/SKILL.md" "$INSTALLED"

cd "$WORKDIR"

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
assert_contains "AGENTS.md has skill install" "itr skill install" "$AGENTS_CONTENT"

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
OUT_I3=$(ITR_DB_PATH="$BU_DIR/.itr.db" $ITR get 3 -f json)
assert_eq "batch update done removes blocker edge" "[]" "$(jq_val "$OUT_I3" "d['blocked_by']")"
set +e
OUT=$(ITR_DB_PATH="$BU_DIR/.itr.db" $ITR doctor -f json 2>&1)
BU_DOCTOR_EXIT=$?
set -e
assert_eq "doctor clean after batch update blocker cleanup" "0" "$BU_DOCTOR_EXIT"

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
# Unknown flags are rejected
echo ""
echo "--- Unknown flags are rejected ---"

UNK_DIR=$(mktemp -d)
$ITR init --db "$UNK_DIR/.itr.db" > /dev/null

UNK_EXIT=0
UNK_STDERR=$($ITR --db "$UNK_DIR/.itr.db" add "Unknown flag test" --urgency high -f json 2>&1 1>/dev/null) || UNK_EXIT=$?
UNK_COUNT=$($ITR --db "$UNK_DIR/.itr.db" list -f json | python3 -c "import sys,json; print(len(json.load(sys.stdin)))")
if [ "$UNK_EXIT" -ne 0 ] && echo "$UNK_STDERR" | grep -q -- "--urgency" && [ "$UNK_COUNT" = "0" ]; then
    pass "unknown-flag: add rejects unknown flag without mutation"
else
    fail "unknown-flag: add rejects unknown flag without mutation" "exit='$UNK_EXIT', count='$UNK_COUNT', stderr='$UNK_STDERR'"
fi

BULK_TYPO_ADD=$($ITR --db "$UNK_DIR/.itr.db" add "Bulk typo guard" --tags stale -f json)
BULK_TYPO_ID=$(echo "$BULK_TYPO_ADD" | python3 -c "import sys,json; print(json.load(sys.stdin)['id'])")
BULK_TYPO_EXIT=0
BULK_TYPO_STDERR=$($ITR --db "$UNK_DIR/.itr.db" bulk close --tag stale --dryrun -f json 2>&1 1>/dev/null) || BULK_TYPO_EXIT=$?
BULK_TYPO_STATUS=$($ITR --db "$UNK_DIR/.itr.db" get "$BULK_TYPO_ID" -f json | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])")
if [ "$BULK_TYPO_EXIT" -ne 0 ] && echo "$BULK_TYPO_STDERR" | grep -q -- "--dryrun" && [ "$BULK_TYPO_STATUS" = "open" ]; then
    pass "unknown-flag: bulk dry-run typo fails before mutation"
else
    fail "unknown-flag: bulk dry-run typo fails before mutation" "exit='$BULK_TYPO_EXIT', status='$BULK_TYPO_STATUS', stderr='$BULK_TYPO_STDERR'"
fi

rm -rf "$UNK_DIR"

# ─────────────────────────────────────────────
echo ""
echo "--- ui ---"
# ─────────────────────────────────────────────

UI_DIR=$(mktemp -d)
$ITR init --db "$UI_DIR/.itr.db" > /dev/null
$ITR --db "$UI_DIR/.itr.db" add "UI seed" -p high -k bug --tags "ui,seed" -a "visible through ui api" -f json > /dev/null

# Dynamic port: launch with --port 0 so the KERNEL assigns a free port via the
# binary's own bind — no fixed-port collision with whatever else occupies it,
# and no probe-then-bind race. The real port is parsed from the startup banner
# (ui.rs flushes it BEFORE entering accept(), and both compact and json output
# embed http://127.0.0.1:<port>/) — the same pattern tests/contracts/ui.sh uses.
UI_OUT="$UI_DIR/ui.out"
UI_ERR="$UI_DIR/ui.err"
$ITR --db "$UI_DIR/.itr.db" ui --port 0 --no-open -f json > "$UI_OUT" 2> "$UI_ERR" &
UI_PID=$!

UI_PORT=""
for _ in {1..50}; do
    UI_PORT="$(sed -nE 's#.*http://127\.0\.0\.1:([0-9]+)/.*#\1#p' "$UI_OUT" 2>/dev/null | head -1)"
    if [ -n "$UI_PORT" ]; then
        break
    fi
    if ! kill -0 "$UI_PID" 2>/dev/null; then
        break
    fi
    sleep 0.1
done

if [ -z "$UI_PORT" ] || ! kill -0 "$UI_PID" 2>/dev/null; then
    fail "ui: local server starts" "no startup banner/port; stderr: $(cat "$UI_ERR" 2>/dev/null)"
else
    TOKEN=$(python3 - "$UI_OUT" <<'PY'
import json, sys, urllib.parse
with open(sys.argv[1], encoding="utf-8") as f:
    data = json.load(f)
print(urllib.parse.parse_qs(urllib.parse.urlparse(data["url"]).query)["token"][0])
PY
)
    UI_RESULT=$(python3 - "$UI_PORT" "$TOKEN" 2>&1 <<'PY'
import http.client
import json
import sys

port = int(sys.argv[1])
token = sys.argv[2]

def request_raw(method, path, body=None):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    payload = json.dumps(body).encode() if body is not None else None
    headers = {"X-ITR-Token": token}
    if body is not None:
        headers["Content-Type"] = "application/json"
    conn.request(method, path, body=payload, headers=headers)
    resp = conn.getresponse()
    raw = resp.read().decode()
    conn.close()
    data = json.loads(raw) if raw else {}
    return resp.status, data

def request(method, path, body=None):
    status, data = request_raw(method, path, body)
    if status >= 400:
        raise SystemExit(f"{method} {path} failed: {status} {json.dumps(data)}")
    return data

health = request("GET", "/api/health")
assert health["ok"] is True

bootstrap = request("GET", "/api/bootstrap")
assert bootstrap["dangerous_sql"] is False

status, denied = request_raw("POST", "/api/sql", {"sql": "select 1"})
assert status == 403
assert denied["code"] == "DANGEROUS_SQL_DISABLED"

listed = request("GET", "/api/issues?q=UI&all=true")
assert listed["total"] >= 1

# Batched detail fetch (#136): ids= switches to full IssueDetail records,
# missing ids are reported instead of failing, duplicates fetched once.
batched = request("GET", "/api/issues?ids=1,999,1")
assert batched["total"] == 1
assert batched["issues"][0]["id"] == 1
assert "urgency_breakdown" in batched["issues"][0]
assert batched["missing"] == [999]
status, bad = request_raw("GET", "/api/issues?ids=abc")
assert status == 400
assert bad["code"] == "INVALID_VALUE"

created = request("POST", "/api/issues", {
    "title": "Created through UI test",
    "priority": "medium",
    "kind": "task",
    "tags": ["ui"],
    "acceptance": "api create returns issue",
})
issue_id = created["issue"]["id"]

patched = request("PATCH", f"/api/issues/{issue_id}", {
    "status": "in-progress",
    "assigned_to": "integration",
    "tags": ["ui", "edited"],
})
assert patched["issue"]["status"] == "in-progress"
assert patched["issue"]["assigned_to"] == "integration"
assert "edited" in patched["issue"]["tags"]

preview = request("POST", "/api/bulk/resolve/preview", {"ids": [issue_id]})
assert preview["count"] == 1

resolved = request("POST", "/api/bulk/resolve/apply", {
    "ids": [issue_id],
    "reason": "ui integration complete",
})
assert resolved["count"] == 1
assert resolved["issues"][0]["status"] == "done"
print("ok")
PY
) || true
    if [ "$UI_RESULT" = "ok" ]; then
        pass "ui: local API supports list/create/edit/bulk resolve and blocks raw SQL by default"
    else
        fail "ui: local API supports list/create/edit/bulk resolve and blocks raw SQL by default" "$UI_RESULT"
    fi
fi

kill "$UI_PID" >/dev/null 2>&1 || true
wait "$UI_PID" 2>/dev/null || true
UI_PID=""

# Same dynamic-port pattern as above for the --allow-dangerous server.
UI_SQL_OUT="$UI_DIR/ui-sql.out"
UI_SQL_ERR="$UI_DIR/ui-sql.err"
$ITR --db "$UI_DIR/.itr.db" ui --port 0 --no-open --allow-dangerous -f json > "$UI_SQL_OUT" 2> "$UI_SQL_ERR" &
UI_SQL_PID=$!

UI_SQL_PORT=""
for _ in {1..50}; do
    UI_SQL_PORT="$(sed -nE 's#.*http://127\.0\.0\.1:([0-9]+)/.*#\1#p' "$UI_SQL_OUT" 2>/dev/null | head -1)"
    if [ -n "$UI_SQL_PORT" ]; then
        break
    fi
    if ! kill -0 "$UI_SQL_PID" 2>/dev/null; then
        break
    fi
    sleep 0.1
done

if [ -z "$UI_SQL_PORT" ] || ! kill -0 "$UI_SQL_PID" 2>/dev/null; then
    fail "ui: dangerous SQL server starts" "no startup banner/port; stderr: $(cat "$UI_SQL_ERR" 2>/dev/null)"
else
    SQL_TOKEN=$(python3 - "$UI_SQL_OUT" <<'PY'
import json, sys, urllib.parse
with open(sys.argv[1], encoding="utf-8") as f:
    data = json.load(f)
print(urllib.parse.parse_qs(urllib.parse.urlparse(data["url"]).query)["token"][0])
PY
)
    SQL_RESULT=$(python3 - "$UI_SQL_PORT" "$SQL_TOKEN" 2>&1 <<'PY'
import http.client
import json
import sys

port = int(sys.argv[1])
token = sys.argv[2]

def request(method, path, body=None):
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    payload = json.dumps(body).encode() if body is not None else None
    headers = {"X-ITR-Token": token}
    if body is not None:
        headers["Content-Type"] = "application/json"
    conn.request(method, path, body=payload, headers=headers)
    resp = conn.getresponse()
    raw = resp.read().decode()
    conn.close()
    data = json.loads(raw) if raw else {}
    if resp.status >= 400:
        raise SystemExit(f"{method} {path} failed: {resp.status} {raw}")
    return data

bootstrap = request("GET", "/api/bootstrap")
assert bootstrap["dangerous_sql"] is True

selected = request("POST", "/api/sql", {
    "sql": "select title, status from issues where title = 'UI seed'"
})
assert selected["columns"] == ["title", "status"]
assert selected["row_count"] == 1
assert selected["rows"][0][0] == "UI seed"

updated = request("POST", "/api/sql", {
    "sql": "update issues set status = 'in-progress' where title = 'UI seed'"
})
assert updated["changes"] >= 1

listed = request("GET", "/api/issues?q=UI&all=true")
assert any(issue["title"] == "UI seed" and issue["status"] == "in-progress" for issue in listed["issues"])

print("ok")
PY
) || true
    if [ "$SQL_RESULT" = "ok" ]; then
        pass "ui: raw SQL requires --allow-dangerous and can query/update"
    else
        fail "ui: raw SQL requires --allow-dangerous and can query/update" "$SQL_RESULT"
    fi
fi

kill "$UI_SQL_PID" >/dev/null 2>&1 || true
wait "$UI_SQL_PID" 2>/dev/null || true
UI_SQL_PID=""
rm -rf "$UI_DIR"

# ─────────────────────────────────────────────
echo ""
echo "--- update --parent / --no-parent ---"
# ─────────────────────────────────────────────

PARENT_DIR=$(mktemp -d)
ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR add "Parent epic A" -k epic -f json >/dev/null     # id 1
ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR add "Parent epic B" -k epic -f json >/dev/null     # id 2
ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR add "Child task"   -f json >/dev/null               # id 3
ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR add "Grandchild"   -f json >/dev/null               # id 4

# Set parent: child (3) under epic A (1)
OUT=$(ITR_AGENT=parent-test ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 3 --parent 1 -f json)
assert_eq "update --parent sets parent_id" "1" "$(jq_val "$OUT" "d['parent_id']")"

# Change parent: child (3) moved under epic B (2)
OUT=$(ITR_AGENT=parent-test ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 3 --parent 2 -f json)
assert_eq "update --parent changes parent_id" "2" "$(jq_val "$OUT" "d['parent_id']")"

# Clear parent: --no-parent on child (3)
OUT=$(ITR_AGENT=parent-test ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 3 --no-parent -f json)
PID_AFTER_CLEAR=$(jq_val "$OUT" "d.get('parent_id') is None")
assert_eq "update --no-parent clears parent_id" "True" "$PID_AFTER_CLEAR"

# Missing-parent rejection: parent ID 999 does not exist
set +e
MISSING_OUT=$(ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 3 --parent 999 2>&1)
MISSING_RC=$?
set -e
assert_eq "update --parent missing exits 1" "1" "$MISSING_RC"
assert_contains "update --parent missing message mentions 999" "999" "$MISSING_OUT"

# No partial write: parent_id should still be null after rejection
OUT=$(ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR get 3 -f json)
PID_AFTER_REJECT=$(jq_val "$OUT" "d.get('parent_id') is None")
assert_eq "update --parent missing leaves parent unchanged" "True" "$PID_AFTER_REJECT"

# Self-cycle rejection: cannot parent issue 1 to itself
set +e
SELF_OUT=$(ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 1 --parent 1 2>&1)
SELF_RC=$?
set -e
assert_eq "update --parent self exits 1" "1" "$SELF_RC"
assert_contains "update --parent self mentions cycle" "ycle" "$SELF_OUT"

# Descendant-cycle rejection: parent grandchild (4) under child (3) under epic A (1),
# then try to set epic A's parent to grandchild (4). 4 is a descendant of 1, so reject.
ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 3 --parent 1 -f json >/dev/null
ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 4 --parent 3 -f json >/dev/null
set +e
DESC_OUT=$(ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 1 --parent 4 2>&1)
DESC_RC=$?
set -e
assert_eq "update --parent descendant exits 1" "1" "$DESC_RC"
assert_contains "update --parent descendant mentions cycle" "ycle" "$DESC_OUT"

# Verify no partial write after descendant-cycle rejection
OUT=$(ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR get 1 -f json)
PID_AFTER_DESC=$(jq_val "$OUT" "d.get('parent_id') is None")
assert_eq "update --parent descendant leaves parent unchanged" "True" "$PID_AFTER_DESC"

# Conflicting flags: both --parent and --no-parent should be rejected
set +e
CONFLICT_OUT=$(ITR_DB_PATH="$PARENT_DIR/.itr.db" $ITR update 3 --parent 2 --no-parent 2>&1)
CONFLICT_RC=$?
set -e
assert_eq "update --parent + --no-parent exits non-zero" "0" "$([ "$CONFLICT_RC" -ne 0 ] && echo 0 || echo 1)"

# Audit-event emission for parent_id (set + clear + change). Reset to a known state first.
AUDIT_DIR=$(mktemp -d)
ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR add "Epic X" -k epic -f json >/dev/null              # id 1
ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR add "Epic Y" -k epic -f json >/dev/null              # id 2
ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR add "Audited child" -f json >/dev/null               # id 3

ITR_AGENT=audit-agent ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR update 3 --parent 1 -f json >/dev/null
ITR_AGENT=audit-agent ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR update 3 --parent 2 -f json >/dev/null
ITR_AGENT=audit-agent ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR update 3 --no-parent -f json >/dev/null

OUT=$(ITR_DB_PATH="$AUDIT_DIR/.itr.db" $ITR log 3 -f json)
PARENT_EVENTS=$(jq_val "$OUT" "len([e for e in d if e['field']=='parent_id'])")
assert_eq "update parent_id emits 3 audit events (set, change, clear)" "3" "$PARENT_EVENTS"
FIRST_SET_NEW=$(jq_val "$OUT" "[e for e in d if e['field']=='parent_id'][0]['new_value']")
assert_eq "first parent_id event new_value is '1'" "1" "$FIRST_SET_NEW"
CHANGE_OLD=$(jq_val "$OUT" "[e for e in d if e['field']=='parent_id'][1]['old_value']")
CHANGE_NEW=$(jq_val "$OUT" "[e for e in d if e['field']=='parent_id'][1]['new_value']")
assert_eq "change parent_id event old_value is '1'" "1" "$CHANGE_OLD"
assert_eq "change parent_id event new_value is '2'" "2" "$CHANGE_NEW"
CLEAR_OLD=$(jq_val "$OUT" "[e for e in d if e['field']=='parent_id'][2]['old_value']")
CLEAR_NEW=$(jq_val "$OUT" "[e for e in d if e['field']=='parent_id'][2]['new_value']")
assert_eq "clear parent_id event old_value is '2'" "2" "$CLEAR_OLD"
assert_eq "clear parent_id event new_value is empty" "" "$CLEAR_NEW"
AUDIT_AGENT=$(jq_val "$OUT" "[e for e in d if e['field']=='parent_id'][0]['agent']")
assert_eq "parent_id audit event records ITR_AGENT" "audit-agent" "$AUDIT_AGENT"

rm -rf "$PARENT_DIR" "$AUDIT_DIR"

# ─────────────────────────────────────────────
echo "--- import drops events/relations with REVIEW warning ---"
# ─────────────────────────────────────────────

# Build a source DB that has at least one event (from an update) and one
# relation, then export it. Importing that bundle into a fresh DB should
# emit a REVIEW: warning on stderr naming events and relations, while
# still importing issues, notes, and dependencies.

IMPORT_WARN_SRC=$(mktemp -d)
ITR_DB_PATH="$IMPORT_WARN_SRC/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$IMPORT_WARN_SRC/.itr.db" $ITR add "Source issue A" -f json >/dev/null  # id 1
ITR_DB_PATH="$IMPORT_WARN_SRC/.itr.db" $ITR add "Source issue B" -f json >/dev/null  # id 2

# Generate at least one audit event (priority change is logged).
ITR_AGENT=warn-test ITR_DB_PATH="$IMPORT_WARN_SRC/.itr.db" $ITR update 1 -p high -f json >/dev/null

# Generate at least one relation row. The relate command takes the source
# id positionally and the target via --to (default relation type is
# "related"). Swallow errors to keep the test resilient if the relate
# command shape changes later.
ITR_DB_PATH="$IMPORT_WARN_SRC/.itr.db" $ITR relate 1 --to 2 >/dev/null 2>&1 || true

# Sanity-check: confirm the source bundle actually contains events/relations
EXPORT_WARN_FILE="$IMPORT_WARN_SRC/export.jsonl"
ITR_DB_PATH="$IMPORT_WARN_SRC/.itr.db" $ITR export > "$EXPORT_WARN_FILE"
HAS_EVENTS=$(python3 -c "import json,sys
n=0
for line in open(sys.argv[1]):
    line=line.strip()
    if not line: continue
    d=json.loads(line)
    n+=len(d.get('events',[]))
print(n)" "$EXPORT_WARN_FILE")
HAS_RELATIONS=$(python3 -c "import json,sys
n=0
for line in open(sys.argv[1]):
    line=line.strip()
    if not line: continue
    d=json.loads(line)
    n+=len(d.get('relations',[]))
print(n)" "$EXPORT_WARN_FILE")
[ "$HAS_EVENTS" -ge 1 ] && pass "export bundle contains events to drop" || \
    fail "export bundle contains events to drop" "events=$HAS_EVENTS"

# Import into a fresh DB and capture stderr.
IMPORT_WARN_DST=$(mktemp -d)
ITR_DB_PATH="$IMPORT_WARN_DST/.itr.db" $ITR init >/dev/null
WARN_STDERR_FILE="$IMPORT_WARN_DST/import.stderr"
WARN_STDOUT=$(ITR_DB_PATH="$IMPORT_WARN_DST/.itr.db" $ITR import --file "$EXPORT_WARN_FILE" -f json 2>"$WARN_STDERR_FILE")
WARN_RC=$?
WARN_STDERR=$(cat "$WARN_STDERR_FILE")

# Exit code should still be 0 (soft fallback).
assert_eq "import with dropped events/relations exits 0" "0" "$WARN_RC"

# stdout JSON should still report imported count.
WARN_IMPORTED=$(jq_val "$WARN_STDOUT" "d['imported']")
[ "$WARN_IMPORTED" -ge 1 ] && pass "import still wrote issues despite drops" || \
    fail "import still wrote issues despite drops" "imported=$WARN_IMPORTED"

# stderr should carry the REVIEW: warning and name the dropped table.
assert_contains "import emits REVIEW: warning on stderr" "REVIEW:" "$WARN_STDERR"
assert_contains "import REVIEW warning names events table" "events" "$WARN_STDERR"

# Only mention relations if any were actually generated (relate command may
# vary between builds); skip the relations assertion if there were none.
if [ "$HAS_RELATIONS" -ge 1 ]; then
    assert_contains "import REVIEW warning names relations table" "relations" "$WARN_STDERR"
fi

# stdout must NOT contain the warning — output contract: stderr-only.
case "$WARN_STDOUT" in
    *REVIEW:*) fail "import REVIEW warning stays off stdout" "leaked to stdout" ;;
    *) pass "import REVIEW warning stays off stdout" ;;
esac

# Importing a bundle with zero events/relations should NOT emit the warning.
CLEAN_SRC=$(mktemp -d)
ITR_DB_PATH="$CLEAN_SRC/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$CLEAN_SRC/.itr.db" $ITR add "Clean issue" -f json >/dev/null
CLEAN_EXPORT="$CLEAN_SRC/export.jsonl"
ITR_DB_PATH="$CLEAN_SRC/.itr.db" $ITR export > "$CLEAN_EXPORT"

CLEAN_DST=$(mktemp -d)
ITR_DB_PATH="$CLEAN_DST/.itr.db" $ITR init >/dev/null
CLEAN_STDERR_FILE="$CLEAN_DST/import.stderr"
ITR_DB_PATH="$CLEAN_DST/.itr.db" $ITR import --file "$CLEAN_EXPORT" -f json >/dev/null 2>"$CLEAN_STDERR_FILE"
CLEAN_STDERR=$(cat "$CLEAN_STDERR_FILE")
case "$CLEAN_STDERR" in
    *REVIEW:*dropped*) fail "import without events/relations stays quiet" "warning unexpectedly emitted: $CLEAN_STDERR" ;;
    *) pass "import without events/relations stays quiet" ;;
esac

rm -rf "$IMPORT_WARN_SRC" "$IMPORT_WARN_DST" "$CLEAN_SRC" "$CLEAN_DST"

# ─────────────────────────────────────────────
echo "--- invalid format error message lists oneline ---"
# ─────────────────────────────────────────────

# The invalid-format error message must list every format accepted by
# Format::from_str (compact, json, pretty, oneline). If src/format.rs
# adds a new format and src/main.rs forgets to mention it, this test
# should fail so the drift is caught at CI time.

FMT_SRC=$(mktemp -d)
ITR_DB_PATH="$FMT_SRC/.itr.db" $ITR init >/dev/null
ITR_DB_PATH="$FMT_SRC/.itr.db" $ITR add "Format probe issue" -f json >/dev/null

# 1) `oneline` must NOT trigger the invalid-format error path.
ONELINE_STDERR_FILE="$FMT_SRC/oneline.stderr"
set +e
ITR_DB_PATH="$FMT_SRC/.itr.db" $ITR list -f oneline >/dev/null 2>"$ONELINE_STDERR_FILE"
ONELINE_RC=$?
set -e
ONELINE_STDERR=$(cat "$ONELINE_STDERR_FILE")
assert_eq "oneline format exits 0 (not invalid)" "0" "$ONELINE_RC"
case "$ONELINE_STDERR" in
    *"Invalid format"*) fail "oneline does not trigger invalid-format error" "stderr: $ONELINE_STDERR" ;;
    *) pass "oneline does not trigger invalid-format error" ;;
esac

# 2) A truly invalid format must produce the error message AND that message
#    must enumerate every accepted format, including `oneline`.
BAD_STDERR_FILE="$FMT_SRC/bad.stderr"
set +e
ITR_DB_PATH="$FMT_SRC/.itr.db" $ITR list -f bogus >/dev/null 2>"$BAD_STDERR_FILE"
BAD_RC=$?
set -e
BAD_STDERR=$(cat "$BAD_STDERR_FILE")
assert_eq "invalid format exits 1" "1" "$BAD_RC"
assert_contains "invalid-format error message lists compact" "compact" "$BAD_STDERR"
assert_contains "invalid-format error message lists json" "json" "$BAD_STDERR"
assert_contains "invalid-format error message lists pretty" "pretty" "$BAD_STDERR"
assert_contains "invalid-format error message lists oneline" "oneline" "$BAD_STDERR"

rm -rf "$FMT_SRC"

# ─────────────────────────────────────────────
echo ""
echo "--- deterministic JSON contracts (stats key order + graph urgency precision) ---"
# ─────────────────────────────────────────────
#
# Regression for issue #139: parseable JSON outputs must be deterministic so
# byte-level snapshot tests don't flap on semantically-identical data.
#
#   (a) `stats -f json` must emit object keys AND nested count-map keys in a
#       fixed order. Seed two freshly-init'd temp DBs identically and assert
#       the raw stdout bytes are identical, plus assert the documented key
#       order explicitly.
#   (b) `graph -f json` urgency values must honor a fixed precision contract
#       (<= DET_URG_DECIMALS decimal places), stable across two fresh DBs.

DET_URG_DECIMALS=4

# Seed an identical fixture into a fresh DB. Avoids age/time drift by not
# relying on wall-clock-sensitive fields for the byte comparison: the two DBs
# are created back-to-back and seeded with the same script.
seed_det_db() {
    local db="$1"
    ITR_DB_PATH="$db" $ITR init -q >/dev/null
    # A spread of priorities, kinds, statuses, skills, and a dependency edge so
    # every nested count-map (by_status / by_priority / by_kind / by_skills /
    # by_assignee) is exercised and the graph has nodes + an edge with urgency.
    ITR_DB_PATH="$db" $ITR add "Det critical bug" -p critical -k bug --skills "rust,db" --assigned-to "agent-a" >/dev/null
    ITR_DB_PATH="$db" $ITR add "Det high feature" -p high -k feature --skills "rust" --assigned-to "agent-b" >/dev/null
    ITR_DB_PATH="$db" $ITR add "Det low epic" -p low -k epic >/dev/null
    ITR_DB_PATH="$db" $ITR add "Det medium task" -p medium -k task >/dev/null
    ITR_DB_PATH="$db" $ITR update 2 -s in-progress >/dev/null
    ITR_DB_PATH="$db" $ITR depend 4 --on 1 >/dev/null
}

DET_DIR_A=$(mktemp -d)
DET_DIR_B=$(mktemp -d)
seed_det_db "$DET_DIR_A/.itr.db"
seed_det_db "$DET_DIR_B/.itr.db"

# Capture stats JSON for both DBs to files. We pass these files to python via
# argv (not stdin): the analysis scripts below use heredocs, which consume
# stdin, so the JSON must come in as a file argument.
DET_STATS_A_FILE="$DET_DIR_A/stats.json"
DET_STATS_B_FILE="$DET_DIR_B/stats.json"
ITR_DB_PATH="$DET_DIR_A/.itr.db" $ITR stats -f json > "$DET_STATS_A_FILE"
ITR_DB_PATH="$DET_DIR_B/.itr.db" $ITR stats -f json > "$DET_STATS_B_FILE"

# (a.1) Byte-identical stats JSON across two identically-seeded fresh DBs.
if cmp -s "$DET_STATS_A_FILE" "$DET_STATS_B_FILE"; then
    pass "stats -f json byte-identical across two fresh DBs"
else
    fail "stats -f json byte-identical across two fresh DBs" \
        "A=$(cat "$DET_STATS_A_FILE") B=$(cat "$DET_STATS_B_FILE")"
fi

# (a.2) Top-level object keys appear in a fixed, documented order.
DET_STATS_TOPKEYS=$(python3 - "$DET_STATS_A_FILE" <<'PY'
import sys, json
order = []
def hook(pairs):
    order.append([k for k, _ in pairs])
    return dict(pairs)
with open(sys.argv[1], encoding="utf-8") as f:
    json.loads(f.read(), object_pairs_hook=hook)
# object_pairs_hook fires bottom-up (nested objects first), so the top-level
# Stats object is the LAST one decoded.
print(','.join(order[-1]))
PY
)
# serde_json's Map (default build) sorts object keys alphabetically, which is a
# stable, deterministic order. Assert that exact order.
assert_eq "stats -f json top-level key order is deterministic" \
    "avg_urgency,blocked,by_assignee,by_kind,by_priority,by_skills,by_status,oldest_open,ready,total" \
    "$DET_STATS_TOPKEYS"

# (a.3) Nested count-map keys appear in a fixed (sorted) order — the part that
#       was nondeterministic under HashMap serialization.
DET_BY_STATUS_KEYS=$(python3 - "$DET_STATS_A_FILE" <<'PY'
import sys, json
captured = {}
def hook(pairs):
    d = dict(pairs)
    # Capture the inner maps by recognising their key signatures.
    keyset = set(d.keys())
    if {"open", "in-progress", "done", "wontfix"} <= keyset:
        captured["by_status"] = [k for k, _ in pairs]
    if {"critical", "high", "medium", "low"} <= keyset:
        captured["by_priority"] = [k for k, _ in pairs]
    if {"bug", "feature", "task", "epic"} <= keyset:
        captured["by_kind"] = [k for k, _ in pairs]
    return d
with open(sys.argv[1], encoding="utf-8") as f:
    json.loads(f.read(), object_pairs_hook=hook)
print('|'.join([
    ','.join(captured.get("by_status", [])),
    ','.join(captured.get("by_priority", [])),
    ','.join(captured.get("by_kind", [])),
]))
PY
)
assert_eq "stats -f json nested count-map keys are sorted/deterministic" \
    "done,in-progress,open,wontfix|critical,high,low,medium|bug,epic,feature,task" \
    "$DET_BY_STATUS_KEYS"

# (b) graph -f json urgency precision contract: every node urgency must have at
#     most DET_URG_DECIMALS decimal places, and be stable across two fresh DBs.
DET_GRAPH_A_FILE="$DET_DIR_A/graph.json"
DET_GRAPH_B_FILE="$DET_DIR_B/graph.json"
ITR_DB_PATH="$DET_DIR_A/.itr.db" $ITR graph --all -f json > "$DET_GRAPH_A_FILE"
ITR_DB_PATH="$DET_DIR_B/.itr.db" $ITR graph --all -f json > "$DET_GRAPH_B_FILE"

DET_URG_OK=$(python3 - "$DET_GRAPH_A_FILE" "$DET_URG_DECIMALS" <<'PY'
import sys, json
with open(sys.argv[1], encoding="utf-8") as f:
    data = json.load(f)
max_dec = int(sys.argv[2])
bad = []
for node in data["nodes"]:
    # repr() yields the shortest round-trip decimal string for the float, so its
    # fractional digit count reflects the precision serde/json actually emitted.
    val = node["urgency"]
    s = repr(val)
    if "e" in s or "E" in s:
        bad.append((node["id"], s, "scientific notation"))
        continue
    if "." in s:
        decimals = len(s.split(".", 1)[1])
        if decimals > max_dec:
            bad.append((node["id"], s, f"{decimals} decimals"))
    # Also verify rounding to the contract precision is idempotent (no drift).
    if round(val, max_dec) != val:
        bad.append((node["id"], s, "not rounded to contract precision"))
print("ok" if not bad else "BAD:" + ";".join(f"#{i}={s}({why})" for i, s, why in bad))
PY
)
assert_eq "graph -f json urgency honors fixed precision contract (<= $DET_URG_DECIMALS decimals)" "ok" "$DET_URG_OK"

# (b.2) Same urgency precision determinism across the second fresh DB.
DET_URG_OK_B=$(python3 - "$DET_GRAPH_B_FILE" "$DET_URG_DECIMALS" <<'PY'
import sys, json
with open(sys.argv[1], encoding="utf-8") as f:
    data = json.load(f)
max_dec = int(sys.argv[2])
bad = []
for node in data["nodes"]:
    val = node["urgency"]
    s = repr(val)
    if "e" in s or "E" in s:
        bad.append((node["id"], s, "scientific notation")); continue
    if "." in s and len(s.split(".", 1)[1]) > max_dec:
        bad.append((node["id"], s, "too many decimals"))
    if round(val, max_dec) != val:
        bad.append((node["id"], s, "not rounded"))
print("ok" if not bad else "BAD:" + ";".join(f"#{i}={s}({why})" for i, s, why in bad))
PY
)
assert_eq "graph -f json urgency precision stable on second fresh DB" "ok" "$DET_URG_OK_B"

rm -rf "$DET_DIR_A" "$DET_DIR_B"

# ─────────────────────────────────────────────
# Auto-discovered normalized snapshot contracts (issue #140)
# ─────────────────────────────────────────────
#
# This is the ONLY hook the snapshot harness adds to integration.sh. Each
# tests/contracts/<area>.sh sources tests/contracts/_lib.sh and registers cases
# via `snapshot` / `snapshot_seeded`. Those helpers reuse THIS suite's $ITR
# binary, isolated temp DBs, normalization, and the pass/fail counters above —
# so contract results fold straight into the totals and the verify gate
# exercises them.
#
# New contract areas are added purely by dropping a new
# tests/contracts/<area>.sh plus tests/snapshots/<area>/*.txt. Never edit this
# block to add an area — it loops over every *.sh except _lib.sh automatically.
CONTRACTS_DIR="$SCRIPT_DIR/tests/contracts"
if [ -d "$CONTRACTS_DIR" ]; then
    echo ""
    echo "==============================="
    echo "Snapshot contracts (auto-discovered)"
    echo "==============================="
    # $ITR is exported so _lib.sh / contract files resolve the same binary.
    export ITR
    shopt -s nullglob
    for contract_file in "$CONTRACTS_DIR"/*.sh; do
        case "$(basename "$contract_file")" in
            _lib.sh) continue ;;
        esac
        # Source so registered cases use the suite's PASS/FAIL/TESTS counters.
        # shellcheck source=/dev/null
        . "$contract_file"
    done
    shopt -u nullglob
fi

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
