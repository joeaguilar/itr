#!/usr/bin/env bash
# tests/contracts/example.sh
#
# Example contract area (issue #140). Proves the snapshot harness works
# end-to-end against the same `itr` binary the integration suite uses.
#
# This file is auto-discovered and sourced by tests/integration.sh. For local
# iteration you can run it directly:
#   ITR=./target/release/itr bash tests/contracts/example.sh
# or regenerate baselines for just this area:
#   UPDATE_SNAPSHOTS=1 ITR=./target/release/itr bash tests/contracts/example.sh
#
# It sources _lib.sh relative to its own location, so it works regardless of
# the caller's working directory.

CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tests/contracts/_lib.sh
. "$CONTRACT_DIR/_lib.sh"

echo ""
echo "--- contract: example (snapshot harness self-proof) ---"

# 1) Version output — exercises the version describe/dirty-suffix normalization
#    (`itr v2.9.6-1-gdb7e324`, `itr 0.1.0+abc-dirty`, … all -> `itr X.Y.Z`).
snapshot example version -- --version

# 2) Schema dump — large, fully deterministic, no runtime entropy.
snapshot example schema -- schema

# 3) Empty list on a fresh DB — JSON empty-set contract (exit 0, stdout `[]`).
snapshot example empty_list -- list -f json

# 4) init + add + get flow. `add` then `get` exercises timestamp normalization
#    (CREATED/UPDATED -> <TS>) on a freshly-seeded issue.
seed_example_issue() {
    local db="$1"
    ITR_DB_PATH="$db" "$ITR" add "Hello world" -p high -k bug -c "some context" -a "tests pass" >/dev/null 2>&1
    # Pin created_at to an ancient date (issue #151) so the urgency `age`
    # component saturates to a constant -- get_issue's URGENCY line and breakdown
    # are then timing-independent. See contract_pin_created_at in _lib.sh.
    contract_pin_created_at "$db" 1
}
snapshot_seeded example get_issue seed_example_issue -- get 1

# 5) The compact `add` output itself. The CREATED:<id> line carries no
#    timestamp, so it is stable as-is — confirms a stdout-only, exit-0 command
#    snapshots clean on a freshly-initialized DB.
snapshot example add -- add "Snapshot me" -p medium -k task
