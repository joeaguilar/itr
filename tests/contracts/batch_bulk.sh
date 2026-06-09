#!/usr/bin/env bash
# tests/contracts/batch_bulk.sh
#
# Snapshot contract for the batch / bulk / import / export output surfaces
# (issue #143). These are the highest-value commands for agents: batch and
# bulk return structured per-item results and soft-fallback summaries, and
# import/export define the round-trip data shape. This file locks down the
# normal, dry-run, partial-error, soft-fallback, and idempotent output shapes.
#
# Auto-discovered and sourced by tests/integration.sh. Run standalone with:
#   ITR=./target/release/itr bash tests/contracts/batch_bulk.sh
# Regenerate just this area's baselines with:
#   UPDATE_SNAPSHOTS=1 ITR=./target/release/itr bash tests/contracts/batch_bulk.sh
#
# DETERMINISM NOTES
#   - Every case runs in its own fresh temp DB (harness contract). IDs are
#     sequential from 1, so a fixed seed order yields stable IDs.
#   - Timestamps, temp paths, ports, tokens, and version strings are
#     auto-normalized by _lib.sh::contract_normalize. The batch-add JSON case
#     embeds full issue detail incl. created_at/updated_at -> <TS> and an
#     urgency score that is deterministic on an age-0 fresh issue (same pattern
#     the example area already relies on for `get`).
#   - import --file cases use a FIXED path under /tmp whose directory name
#     contains "tmp" so the normalizer collapses it to <TMP>; the seed fn
#     writes the fixture to that same path before the asserted command runs.

CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tests/contracts/_lib.sh
. "$CONTRACT_DIR/_lib.sh"

echo ""
echo "--- contract: batch_bulk ---"

# Fixed, normalizer-friendly paths for the import --file cases. Each directory
# name contains "tmp", so /tmp/<...>tmp<...>/import.jsonl collapses to
# <TMP>/import.jsonl in both the command header and any error message. The two
# file cases use DISTINCT directories (never shared) so there is zero cross-case
# state — the success case writes its fixture, the missing case points at a dir
# that is never created.
BB_IMPORT_FILE="/tmp/itr-batchbulk-tmp-importfile/import.jsonl"
BB_IMPORT_MISSING="/tmp/itr-batchbulk-tmp-importmissing/import.jsonl"

# ──────────────────────────────────────────────────────────────────────────
# Seed helpers. Each receives the per-case DB path as $1.
# ──────────────────────────────────────────────────────────────────────────
seed_two() {
    ITR_DB_PATH="$1" "$ITR" add "One" >/dev/null 2>&1
    ITR_DB_PATH="$1" "$ITR" add "Two" >/dev/null 2>&1
}

seed_closed_one() {
    ITR_DB_PATH="$1" "$ITR" add "One" >/dev/null 2>&1
    ITR_DB_PATH="$1" "$ITR" close 1 >/dev/null 2>&1
}

# Three issues: #1/#2 high, #3 low. Lets bulk filters select a stable subset.
seed_mix() {
    ITR_DB_PATH="$1" "$ITR" add "High one" -p high >/dev/null 2>&1
    ITR_DB_PATH="$1" "$ITR" add "High two" -p high >/dev/null 2>&1
    ITR_DB_PATH="$1" "$ITR" add "Low one" -p low >/dev/null 2>&1
}

# Blocker #1 (high) blocks dependent #2; closing #1 unblocks #2.
seed_dep() {
    ITR_DB_PATH="$1" "$ITR" add "Blocker" -p high >/dev/null 2>&1
    ITR_DB_PATH="$1" "$ITR" add "Dependent" >/dev/null 2>&1
    ITR_DB_PATH="$1" "$ITR" depend 2 --on 1 >/dev/null 2>&1
}

# One pre-existing issue (#1). Used by the import --merge stdin case (the merge
# payload arrives on stdin, so no on-disk fixture is needed here).
seed_one() {
    ITR_DB_PATH="$1" "$ITR" add "Existing one" -p high >/dev/null 2>&1
}

# One pre-existing issue (#1) plus a two-issue JSONL fixture written to the
# DEDICATED import-file directory. Also clears the missing-file directory so the
# import_file_missing case is guaranteed to point at a nonexistent path. These
# two directories are never shared, eliminating any cross-case path leakage.
seed_one_and_import_file() {
    ITR_DB_PATH="$1" "$ITR" add "Existing one" -p high >/dev/null 2>&1
    rm -rf "$(dirname "$BB_IMPORT_MISSING")"
    mkdir -p "$(dirname "$BB_IMPORT_FILE")"
    cat >"$BB_IMPORT_FILE" <<'EOF'
{"id":1,"title":"Dup","priority":"high","kind":"bug","status":"open","context":"","files":[],"tags":[],"skills":[],"acceptance":"","blocked_by":[],"blocks":[],"parent_id":null,"children":[],"assigned_to":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","close_reason":"","notes":[],"urgency":0.0,"relations":[]}
{"id":2,"title":"FromFile","priority":"low","kind":"task","status":"open","context":"","files":[],"tags":[],"skills":[],"acceptance":"","blocked_by":[],"blocks":[],"parent_id":null,"children":[],"assigned_to":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","close_reason":"","notes":[],"urgency":0.0,"relations":[]}
EOF
}

# Reusable JSONL stdin payloads for import-from-stdin cases. One issue per line.
IMPORT_JSONL='{"id":1,"title":"Imported A","priority":"high","kind":"bug","status":"open","context":"","files":[],"tags":[],"skills":[],"acceptance":"","blocked_by":[],"blocks":[],"parent_id":null,"children":[],"assigned_to":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","close_reason":"","notes":[],"urgency":0.0,"relations":[]}'
IMPORT_JSONL_TWO='{"id":1,"title":"Dup","priority":"high","kind":"bug","status":"open","context":"","files":[],"tags":[],"skills":[],"acceptance":"","blocked_by":[],"blocks":[],"parent_id":null,"children":[],"assigned_to":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","close_reason":"","notes":[],"urgency":0.0,"relations":[]}
{"id":2,"title":"Imported B","priority":"low","kind":"task","status":"open","context":"","files":[],"tags":[],"skills":[],"acceptance":"","blocked_by":[],"blocks":[],"parent_id":null,"children":[],"assigned_to":null,"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","close_reason":"","notes":[],"urgency":0.0,"relations":[]}'

# ──────────────────────────────────────────────────────────────────────────
# BATCH ADD / CREATE — stdin JSON array; all-ok, soft-fallback, alias.
# ──────────────────────────────────────────────────────────────────────────

# Compact per-item ok shape: summary line + one OK line per created issue.
snapshot batch_bulk batch_add_compact \
    '[{"title":"A","priority":"high"},{"title":"B"}]' \
    -- batch add

# Full structured JSON shape (per-item ok + embedded issue detail). Timestamps
# normalize to <TS>; urgency is deterministic on an age-0 fresh DB.
snapshot batch_bulk batch_add_json \
    '[{"title":"A","priority":"high"},{"title":"B"}]' \
    -- batch add -f json

# Soft-fallback: unrecognized priority+kind default to medium/task and emit
# REVIEW notes; outcome is "review", summary review=1. (Captures CURRENT
# behavior; see issue #150 re: a known batch-add parent-field bug — not fixed.)
snapshot batch_bulk batch_add_softfallback \
    '[{"title":"C","priority":"bogus","kind":"nonsense"}]' \
    -- batch add -f json

# `batch create` is a visible alias for `batch add`; action stays "batch_add".
snapshot batch_bulk batch_create_alias \
    '[{"title":"X"}]' \
    -- batch create

# ──────────────────────────────────────────────────────────────────────────
# BATCH CLOSE — per-item reasons; ok / error / idempotent / dry-run.
# ──────────────────────────────────────────────────────────────────────────

# Partial error: #1 closes ok (reason echoed), #99 missing -> error outcome.
snapshot_seeded batch_bulk batch_close_partial seed_two \
    '[{"id":1,"reason":"shipped"},{"id":99}]' \
    -- batch close

# Dry-run: both report ok, nothing committed; label gets (DRY-RUN).
snapshot_seeded batch_bulk batch_close_dryrun seed_two \
    '[{"id":1,"reason":"shipped"},{"id":2}]' \
    -- batch close --dry-run

# Idempotent: re-closing an already-closed issue is ok with "Already done".
snapshot_seeded batch_bulk batch_close_idempotent seed_closed_one \
    '[{"id":1,"reason":"again"}]' \
    -- batch close

# ──────────────────────────────────────────────────────────────────────────
# BATCH UPDATE — per-item changes; ok / review / error / dry-run.
# ──────────────────────────────────────────────────────────────────────────

# Mixed outcomes: #1 ok, #2 review (bogus priority kept), #99 error (missing).
snapshot_seeded batch_bulk batch_update_partial seed_two \
    '[{"id":1,"status":"in-progress","priority":"high"},{"id":2,"priority":"bogus"},{"id":99,"status":"done"}]' \
    -- batch update

# Dry-run: both ok, no mutation; label gets (DRY-RUN).
snapshot_seeded batch_bulk batch_update_dryrun seed_two \
    '[{"id":1,"status":"done"},{"id":2,"title":"Renamed"}]' \
    -- batch update --dry-run

# ──────────────────────────────────────────────────────────────────────────
# BATCH NOTE — [{id, text, agent?}]; ok / error.
# ──────────────────────────────────────────────────────────────────────────

# #1 note ok (content echoed), #99 missing -> error outcome.
snapshot_seeded batch_bulk batch_note_partial seed_two \
    '[{"id":1,"text":"first note","agent":"alice"},{"id":99,"text":"orphan"}]' \
    -- batch note

# ──────────────────────────────────────────────────────────────────────────
# BULK CLOSE — filter-based; dry-run / real / json / unblocked / no-filter.
# ──────────────────────────────────────────────────────────────────────────

# Dry-run: matches #1,#2 by priority; (dry-run) suffix, no mutation.
snapshot_seeded batch_bulk bulk_close_dryrun seed_mix \
    -- bulk close --priority high --dry-run

# Real mutation: matches #1,#2 and closes them with a reason.
snapshot_seeded batch_bulk bulk_close_real seed_mix \
    -- bulk close --priority high --reason "batch shipped"

# JSON shape for the bulk result envelope (action/count/ids/unblocked/dry_run).
snapshot_seeded batch_bulk bulk_close_json seed_mix \
    -- bulk close --priority high -f json

# Closing the blocker emits an UNBLOCKED line for the freed dependent.
snapshot_seeded batch_bulk bulk_close_unblocked seed_dep \
    -- bulk close --priority high --reason done

# No filters -> hard error on stderr, exit 1 (NoFilters is unrecoverable).
snapshot batch_bulk bulk_close_nofilters \
    -- bulk close

# ──────────────────────────────────────────────────────────────────────────
# BULK UPDATE — filter-based; dry-run / real.
# ──────────────────────────────────────────────────────────────────────────

# Dry-run: matches #1,#2; (dry-run) suffix, no mutation.
snapshot_seeded batch_bulk bulk_update_dryrun seed_mix \
    -- bulk update --priority high --set-status in-progress --dry-run

# Real mutation: matches #3 (low), bumps priority + adds a tag.
snapshot_seeded batch_bulk bulk_update_real seed_mix \
    -- bulk update --priority low --set-priority medium --add-tag triaged

# ──────────────────────────────────────────────────────────────────────────
# EXPORT — jsonl (default) and json.
# ──────────────────────────────────────────────────────────────────────────

# JSONL: one issue object per line. created_at/updated_at -> <TS>.
snapshot_seeded batch_bulk export_jsonl seed_mix \
    -- export

# JSON: single pretty object with issues/dependencies/notes arrays.
snapshot_seeded batch_bulk export_json seed_mix \
    -- export --export-format json

# ──────────────────────────────────────────────────────────────────────────
# IMPORT — stdin jsonl, stdin merge (skip existing), --file, --file missing.
# ──────────────────────────────────────────────────────────────────────────

# Replace mode from stdin JSONL: one issue imported, mode: replace.
snapshot batch_bulk import_jsonl_stdin \
    "$IMPORT_JSONL" \
    -- import

# Merge mode from stdin: #1 already present (seeded) -> skipped, #2 imported.
snapshot_seeded batch_bulk import_merge_stdin seed_one \
    "$IMPORT_JSONL_TWO" \
    -- import --merge

# --file replace from an on-disk fixture (#1 dup overwrites, #2 new): 2 imported.
# Path collapses to <TMP>/import.jsonl via the normalizer.
snapshot_seeded batch_bulk import_file seed_one_and_import_file \
    -- import --file /tmp/itr-batchbulk-tmp-importfile/import.jsonl

# --file pointing at a missing path -> hard error on stderr, exit 1. The
# directory is cleared by seed_one_and_import_file (run by the prior case), and
# this case never recreates it, so the read genuinely fails. Path collapses to
# <TMP>/import.jsonl via the normalizer.
snapshot batch_bulk import_file_missing \
    -- import --file /tmp/itr-batchbulk-tmp-importmissing/import.jsonl
