#!/usr/bin/env bash
# tests/contracts/core.sh
#
# Core issue-workflow output contracts (issue #142). Snapshots the normalized
# stdout/stderr/exit of the everyday itr commands so an unintended change to any
# of them surfaces as a labeled unified diff in the integration suite.
#
# Auto-discovered and sourced by tests/integration.sh. Run standalone with:
#   ITR=./target/release/itr bash tests/contracts/core.sh
# Regenerate just this area's baselines:
#   UPDATE_SNAPSHOTS=1 ITR=./target/release/itr bash tests/contracts/core.sh
#
# Determinism notes (relied on here):
#   - All UTC ISO-8601 timestamps normalize to <TS> (CREATED/UPDATED, note
#     created_at, log TS, search created_at, summary recent activity).
#   - The per-case temp DB path normalizes to <TMP>.
#   - Per issue #139, `stats -f json` key order and `graph` urgency precision
#     are deterministic, so they are snapshotted byte-for-byte.
#
# Coverage: each case runs in its own fresh temp DB (the harness re-inits per
# call), so adding a command snapshot never perturbs unrelated expected output.

CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tests/contracts/_lib.sh
. "$CONTRACT_DIR/_lib.sh"

echo ""
echo "--- contract: core ---"

# ──────────────────────────────────────────────────────────────────────────
# Seed helpers. Kept deterministic so snapshots stay stable. Each receives the
# isolated DB path as $1 and writes through ITR_DB_PATH.
# ──────────────────────────────────────────────────────────────────────────

# Two plain issues: 1 = high/bug, 2 = low/task. Used by most read/edit cases.
#
# created_at is pinned to an ancient instant (issue #151) so the urgency `age`
# component saturates to the constant `urgency.age` coefficient on every run,
# making every urgency-bearing snapshot fed by this seed timing-independent.
# See contract_pin_created_at in _lib.sh for the full rationale. (summary's raw
# days_old is collapsed to <DAYS> by the normalizer, so the ancient pin does not
# leak a drifting day count into the summary snapshots.)
seed_core() {
    ITR_DB_PATH="$1" "$ITR" add "Fixture issue" -p high -k bug -c "ctx" -a "acc" >/dev/null 2>&1
    ITR_DB_PATH="$1" "$ITR" add "Another" -p low -k task >/dev/null 2>&1
    contract_pin_created_at "$1" 1
    contract_pin_created_at "$1" 2
}

# Same two issues, plus one note (id 1) on issue 1. Used by note-update /
# note-delete cases that need an existing note.
seed_core_noted() {
    seed_core "$1"
    ITR_DB_PATH="$1" "$ITR" note 1 "original note" --agent seed >/dev/null 2>&1
}

# Two issues with a dependency edge: issue 2 blocked by issue 1. Used by
# undepend / deps-read cases.
seed_core_dep() {
    seed_core "$1"
    ITR_DB_PATH="$1" "$ITR" depend 2 --on 1 >/dev/null 2>&1
}

# Two issues with a relation edge: 1 related-to 2. Used by unrelate.
seed_core_rel() {
    seed_core "$1"
    ITR_DB_PATH="$1" "$ITR" relate 1 --to 2 >/dev/null 2>&1
}

# Issue 1 driven to in-progress so it appears in wip/current and produces a
# status_changed event for the log.
seed_core_wip() {
    seed_core "$1"
    ITR_DB_PATH="$1" "$ITR" update 1 -s in-progress >/dev/null 2>&1
}

# Issue 1 assigned to an agent. Used by unassign.
seed_core_assigned() {
    seed_core "$1"
    ITR_DB_PATH="$1" "$ITR" assign 1 agent-x >/dev/null 2>&1
}

# ──────────────────────────────────────────────────────────────────────────
# init — fresh DB (the harness already pre-inits, so this is the idempotent
# re-init path). Covers compact + json + the json created=false contract.
# ──────────────────────────────────────────────────────────────────────────
snapshot core init_compact    -- init
snapshot core init_json       -- init -f json

# ──────────────────────────────────────────────────────────────────────────
# add / create — compact, json, soft-fallback (bad priority → medium +
# _needs_review note on stderr), and the `create` alias.
# ──────────────────────────────────────────────────────────────────────────
snapshot core add_compact     -- add "New work" -p high -k bug -c "context here" -a "done when green"
snapshot core add_json        -- add "New work" -p medium -k task -f json
snapshot core add_soft_priority -- add "Bad priority" -p notarealpriority -f json
snapshot core create_alias    -- create "Via create alias" -p low -k feature -f json

# ──────────────────────────────────────────────────────────────────────────
# list — empty json, seeded compact/json/pretty/oneline, and --fields projection.
# ──────────────────────────────────────────────────────────────────────────
snapshot core list_empty_json  -- list -f json
snapshot_seeded core list_compact seed_core -- list
snapshot_seeded core list_json    seed_core -- list -f json
snapshot_seeded core list_pretty  seed_core -- list -f pretty
snapshot_seeded core list_oneline seed_core -- list -f oneline
snapshot_seeded core list_fields  seed_core -- list -f json --fields id,title,priority

# ──────────────────────────────────────────────────────────────────────────
# get / show — seeded detail in compact + json, the `show <id>` alias, and the
# not-found error contract (non-zero exit + stderr).
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core get_compact seed_core -- get 1
snapshot_seeded core get_json    seed_core -- get 1 -f json
snapshot_seeded core show_id     seed_core -- show 1 -f json
snapshot_seeded core show_list   seed_core -- show -f json
snapshot         core get_notfound -- get 999
snapshot         core get_notfound_json -- get 999 -f json

# ──────────────────────────────────────────────────────────────────────────
# update — status change (compact + json) and status soft-fallback.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core update_compact seed_core -- update 1 -s in-progress
snapshot_seeded core update_json    seed_core -- update 1 -s in-progress -f json
snapshot_seeded core update_soft_status seed_core -- update 1 -s notastatus -f json

# ──────────────────────────────────────────────────────────────────────────
# close — done (compact + json) and --wontfix.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core close_compact  seed_core -- close 1 "Fixed it"
snapshot_seeded core close_json     seed_core -- close 1 "Fixed it" -f json
snapshot_seeded core close_wontfix  seed_core -- close 1 --wontfix "Not doing this" -f json

# ──────────────────────────────────────────────────────────────────────────
# note / note-update / note-delete — compact + json, plus not-found error.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core note_compact      seed_core       -- note 1 "Investigating" --agent worker
snapshot_seeded core note_json         seed_core       -- note 1 "Investigating" --agent worker -f json
snapshot_seeded core note_update_compact seed_core_noted -- note-update 1 "Edited content"
snapshot_seeded core note_update_json    seed_core_noted -- note-update 1 "Edited content" -f json
snapshot_seeded core note_delete_compact seed_core_noted -- note-delete 1
snapshot_seeded core note_delete_json    seed_core_noted -- note-delete 1 -f json
snapshot         core note_notfound    -- note 999 "nope"

# ──────────────────────────────────────────────────────────────────────────
# depend / deps / undepend — dependency edges, compact + json, alias, cycle.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core depend_compact  seed_core     -- depend 2 --on 1
snapshot_seeded core depend_json     seed_core     -- depend 2 --on 1 -f json
snapshot_seeded core deps_alias      seed_core     -- deps 2 --on 1
snapshot_seeded core undepend_compact seed_core_dep -- undepend 2 --on 1
snapshot_seeded core undepend_json    seed_core_dep -- undepend 2 --on 1 -f json
snapshot_seeded core depend_cycle    seed_core_dep -- depend 1 --on 2

# ──────────────────────────────────────────────────────────────────────────
# ready / next / claim — work-selection commands, compact + json + empty.
# ──────────────────────────────────────────────────────────────────────────
snapshot         core ready_empty    -- ready -f json
snapshot_seeded core ready_compact   seed_core -- ready
snapshot_seeded core ready_json      seed_core -- ready -f json
snapshot_seeded core next_compact    seed_core -- next
snapshot_seeded core next_json       seed_core -- next -f json
snapshot_seeded core claim_json      seed_core -- claim -f json

# ──────────────────────────────────────────────────────────────────────────
# assign / unassign — ownership, compact + json.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core assign_compact   seed_core          -- assign 1 agent-x
snapshot_seeded core assign_json      seed_core          -- assign 1 agent-x -f json
snapshot_seeded core unassign_compact seed_core_assigned -- unassign 1
snapshot_seeded core unassign_json    seed_core_assigned -- unassign 1 -f json

# ──────────────────────────────────────────────────────────────────────────
# wip / current — in-progress view, empty + seeded, compact + json + alias.
# ──────────────────────────────────────────────────────────────────────────
snapshot         core wip_empty     -- wip -f json
snapshot_seeded core wip_compact    seed_core_wip -- wip
snapshot_seeded core wip_json       seed_core_wip -- wip -f json
snapshot_seeded core current_alias  seed_core_wip -- current -f json

# ──────────────────────────────────────────────────────────────────────────
# stats / summary — health views, compact + json. stats json is deterministic
# (issue #139). summary embeds recent-activity timestamps that normalize.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core stats_compact   seed_core -- stats
snapshot_seeded core stats_json      seed_core -- stats -f json
snapshot_seeded core summary_compact seed_core -- summary
snapshot_seeded core summary_json    seed_core -- summary -f json

# ──────────────────────────────────────────────────────────────────────────
# search — compact + json, empty result (exit 0, []).
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core search_compact seed_core -- search Fixture
snapshot_seeded core search_json    seed_core -- search Fixture -f json
snapshot_seeded core search_empty   seed_core -- search zzznotfoundzzz -f json

# ──────────────────────────────────────────────────────────────────────────
# graph — compact, json (deterministic urgency precision, #139), pretty DOT.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core graph_compact seed_core_dep -- graph
snapshot_seeded core graph_json    seed_core_dep -- graph -f json
snapshot_seeded core graph_pretty  seed_core_dep -- graph -f pretty

# ──────────────────────────────────────────────────────────────────────────
# log — audit history, compact + json (timestamps normalize), and empty.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core log_compact seed_core_wip -- log 1
snapshot_seeded core log_json    seed_core_wip -- log 1 -f json
snapshot_seeded core log_empty   seed_core     -- log -f json

# ──────────────────────────────────────────────────────────────────────────
# relate / unrelate — issue relations, compact + json.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core relate_compact   seed_core     -- relate 1 --to 2
snapshot_seeded core relate_json      seed_core     -- relate 1 --to 2 --type supersedes -f json
snapshot_seeded core unrelate_compact seed_core_rel -- unrelate 1 --from 2
snapshot_seeded core unrelate_json    seed_core_rel -- unrelate 1 --from 2 -f json

# ──────────────────────────────────────────────────────────────────────────
# reindex — FTS rebuild, compact + json.
# ──────────────────────────────────────────────────────────────────────────
snapshot_seeded core reindex_compact seed_core -- reindex
snapshot_seeded core reindex_json    seed_core -- reindex -f json
