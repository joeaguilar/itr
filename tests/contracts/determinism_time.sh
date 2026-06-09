#!/usr/bin/env bash
# tests/contracts/determinism_time.sh
#
# Timing-independence regression test (issue #151).
#
# This is the test that would have CAUGHT the urgency snapshot drift: the
# urgency `age` component is wall-clock dependent
#   config.age * clamp(days_since(created_at) / 10, 0, 1)
# so an urgency-bearing fixture READ in a later second than it was CREATED
# produces a different urgency TOTAL (a hair above the same-second value), which
# in turn flips whether `age` appears in compact output. With ancient-pinned
# created_at, age saturates to a constant and the output is byte-identical no
# matter how much wall-clock time passes between seeding and reading.
#
# Strategy: seed an urgency-bearing fixture, capture its NORMALIZED urgency
# output, sleep >1s, capture again, and assert byte-identical.
#   - PINNED fixture (contract_pin_created_at): constant -> the assertion PASSES.
#     This runs in the normal suite (pass-after evidence).
#   - UNPINNED fixture (same-second created_at): drifts -> the assertion FAILS.
#     This is the fail-before evidence; it is gated behind
#     ITR_DETERMINISM_PROVE_DRIFT so it never breaks the normal suite.
#
# Auto-discovered and sourced by tests/integration.sh. Run standalone:
#   ITR=./target/release/itr bash tests/contracts/determinism_time.sh
# Prove the drift the fix prevents (expected-to-fail demonstration):
#   ITR_DETERMINISM_PROVE_DRIFT=1 ITR=./target/release/itr \
#     bash tests/contracts/determinism_time.sh

CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=tests/contracts/_lib.sh
. "$CONTRACT_DIR/_lib.sh"

echo ""
echo "--- contract: determinism_time (urgency timing-independence, #151) ---"

# Capture the normalized urgency output of `get 1 -f json` for a freshly-built
# isolated DB seeded by the given seed function, twice, separated by a >1s
# sleep, and compare. Records pass/fail through the suite helpers.
#
# Args: <case-label> <seed_fn>
_assert_urgency_timing_independent() {
    local label="$1" seed_fn="$2"

    local tmpdir db
    tmpdir="$(mktemp -d)"
    db="$tmpdir/.itr.db"
    ITR_DB_PATH="$db" "$ITR" init -q >/dev/null 2>&1 || true
    "$seed_fn" "$db" >/dev/null 2>&1 || true

    # First capture (effectively "same second" as the seed).
    local first second
    first="$(ITR_DB_PATH="$db" "$ITR" get 1 -f json 2>/dev/null | contract_normalize "$tmpdir")"

    # Let the wall clock advance past a full second so days_since() changes for
    # any non-saturated age component. 1.5s comfortably clears one tick; fall
    # back to a 2s integer sleep if sub-second sleep is unavailable.
    perl -e 'select(undef,undef,undef,1.5)' 2>/dev/null || sleep 2

    second="$(ITR_DB_PATH="$db" "$ITR" get 1 -f json 2>/dev/null | contract_normalize "$tmpdir")"

    rm -rf "$tmpdir"

    if [ "$first" = "$second" ]; then
        pass "determinism_time/$label (urgency byte-identical across >1s)"
        return 0
    else
        fail "determinism_time/$label" \
            "urgency output drifted across a >1s gap:
--- first ---
$first
--- second ---
$second"
        return 1
    fi
}

# Pass-after seed: one open high/bug issue, created_at PINNED to an ancient date
# so age saturates to a constant. This is the case that runs in the suite.
seed_pinned() {
    ITR_DB_PATH="$1" "$ITR" add "Timing fixture" -p high -k bug -c "ctx" -a "acc" >/dev/null 2>&1
    contract_pin_created_at "$1" 1
}

# Fail-before seed: NO pin, so created_at is "now" and age drifts the moment the
# wall clock ticks past the seeding second.
seed_unpinned() {
    ITR_DB_PATH="$1" "$ITR" add "Timing fixture" -p high -k bug -c "ctx" -a "acc" >/dev/null 2>&1
}

# Pass-after: the pinned fixture is timing-independent (runs in the suite).
_assert_urgency_timing_independent pinned seed_pinned

# Fail-before demonstration: only when explicitly requested, prove the UNPINNED
# (old) fixture drifts and would fail this very assertion. Gated so the normal
# suite stays green; flip the env var to reproduce the original bug on demand.
if [ "${ITR_DETERMINISM_PROVE_DRIFT:-0}" = "1" ]; then
    echo "  (ITR_DETERMINISM_PROVE_DRIFT=1: the next assertion is EXPECTED to FAIL)"
    _assert_urgency_timing_independent unpinned_expected_fail seed_unpinned
fi
