# Testing guide

This project keeps tests local, dependency-light, and shell-friendly. Prefer
checks that run against the same `itr` binary a contributor will use.

## Fast checks

Run these before opening a change:

```bash
cargo check
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
./tests/integration.sh ./target/debug/itr
```

Equivalent `just` recipes:

```bash
just check
just fmt-check
just lint
just build
just test-debug
```

`just test-debug` builds `./target/debug/itr` and passes that path to the
integration suite. Use it while iterating.

When you pass an explicit binary path, `tests/integration.sh` normalizes it
before switching into its isolated temp directory, so repo-relative invocations
such as `./tests/integration.sh ./target/debug/itr` are stable.

Use `cargo fmt --all` or `just fmt` to rewrite formatting. Use
`cargo fmt --all -- --check` or `just fmt-check` to verify formatting without
modifying files.

## Release and CI checks

Run the release-path checks before merging broad CLI, database, or UI changes:

```bash
cargo build --release
./tests/integration.sh
cargo deny check
```

Equivalent `just` recipes:

```bash
just release
just test
just deny
just verify
just ci
```

`./tests/integration.sh` defaults to `./target/release/itr`, so build release
first. CI runs format check, clippy, cargo-deny, release build, and the release
integration suite.

`just verify` is the full pre-push gate: it chains the release build, clippy,
the release integration suite, the format check, and `cargo deny check`. Prefer
it before opening a PR or handing a change off to another agent.

## Integration harness conventions

`tests/integration.sh` is the main end-to-end suite. It is a Bash harness with
`set -euo pipefail` and these helpers:

- `pass DESC` and `fail DESC REASON` record result lines.
- `assert_eq DESC EXPECTED ACTUAL` compares strings exactly.
- `assert_contains DESC NEEDLE HAYSTACK` checks fixed-string containment.
- `assert_exit DESC EXPECTED COMMAND...` checks an exit code while discarding
  stdout and stderr.
- `jq_val JSON PY_EXPR` parses JSON with `python3` and prints `PY_EXPR`.

Do not add a `jq` dependency. Parse JSON with `python3`, either through
`jq_val "$OUT" "d['field']"` or a small heredoc script when the assertion needs
more setup.

The suite creates an isolated working directory:

```bash
WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT
cd "$WORKDIR"
```

Feature sections may create extra `mktemp -d` directories. Use `--db` or
`ITR_DB_PATH` to point each scenario at its own `.itr.db`, then remove the
directory after the section. Avoid relying on repository state.

## Normalized output snapshot harness

`tests/integration.sh` auto-discovers and runs a checked-in snapshot harness
that asserts normalized command output against expected baselines (issue #140).
It is dependency-light (Bash + `sed` + `diff`) and runs against the same `$ITR`
binary the suite already resolves, so `just verify` exercises it.

```
tests/contracts/_lib.sh      # shared harness library (sourced, never run)
tests/contracts/<area>.sh    # one file per area; sources _lib.sh, registers cases
tests/snapshots/<area>/*.txt # expected normalized snapshot per case
```

Each case runs an `itr` command in its own freshly-initialized temp database,
captures stdout, stderr, and exit status, normalizes runtime entropy, and
compares against `tests/snapshots/<area>/<case>.txt`. Normalized entropy:
UTC ISO-8601 timestamps (`<TS>`), mktemp temp paths (`<TMP>`), localhost ports
(`:<PORT>`), UI session tokens (`<TOKEN>`), and version describe/dirty suffixes
(`itr X.Y.Z`). On mismatch the harness prints a labeled unified diff naming the
command, args, exit status, stdout, and stderr, then fails through the normal
suite reporting.

Register cases with the `snapshot` / `snapshot_seeded` helpers:

```bash
# tests/contracts/<area>.sh
CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$CONTRACT_DIR/_lib.sh"

snapshot <area> version            -- --version
snapshot <area> empty_list         -- list -f json
snapshot <area> batch '<json>'     -- batch add -f json

seed_<area>() { ITR_DB_PATH="$1" "$ITR" add "Fixture" >/dev/null 2>&1; }
snapshot_seeded <area> get seed_<area> -- get 1
```

Generate or update baselines (then review the diff in git):

```bash
UPDATE_SNAPSHOTS=1 ./tests/integration.sh   # (re)writes tests/snapshots/**
git diff tests/snapshots/                     # eyeball captured bytes
./tests/integration.sh                        # assert mode — must be green
```

Adding a new area is purely additive: drop a `tests/contracts/<area>.sh` plus
`tests/snapshots/<area>/*.txt`. Do **not** edit `tests/integration.sh` for a
new area — its end-of-suite loop discovers every `tests/contracts/*.sh`
(excluding `_lib.sh`) automatically and folds results into the suite totals.
See `docs/command-contracts.md` for the full snapshot file format and the
how-to-add-a-contract reference.

## Testing output and exits

stdout is parseable command data. stderr is for warnings and errors. Tests
should assert that split explicitly:

```bash
OUT=$($ITR list -f json)
COUNT=$(jq_val "$OUT" "len(d)")
assert_eq "list count" "1" "$COUNT"
```

For expected stderr with a successful command, redirect stdout away:

```bash
STDERR=$($ITR list -f json --fields id,bogus 2>&1 1>/dev/null)
assert_contains "invalid field warns" "REVIEW" "$STDERR"
```

For expected nonzero exits, temporarily disable `set -e`:

```bash
set +e
OUT=$($ITR get 999 -f json 2>&1)
EXIT=$?
set -e
assert_eq "missing issue exits 1" "1" "$EXIT"
assert_contains "missing issue explains failure" "not found" "$OUT"
```

Use `assert_exit` when only the code matters.

## UI localhost API tests

Add UI API coverage in the `--- ui ---` section of `tests/integration.sh`.
Keep it localhost-only and browserless:

1. Create a temp DB and seed issues with the CLI.
2. Start `itr ui` with `--port PORT --no-open -f json`, redirecting stdout and
   stderr to temp files, and keep `$UI_PID`.
3. Wait for the JSON startup line, then verify the process is still alive.
4. Parse the auth token from the emitted URL with `python3` and `urllib.parse`.
5. Use `python3` `http.client.HTTPConnection("127.0.0.1", port, timeout=5)`.
6. Send `X-ITR-Token` on every API request and `Content-Type:
   application/json` for JSON bodies.
7. Assert API behavior, not DOM behavior. Current coverage exercises
   `/api/health`, `/api/bootstrap` (config and capability surface),
   issue list/create/edit, bulk resolve preview/apply, and `/api/sql`
   (only when the server was started with `--allow-dangerous`).
8. Always `kill`, `wait`, and remove the temp directory.

The `/api/sql` coverage starts a second `itr ui --allow-dangerous` server in
the same suite section so the dangerous-mode contract stays exercised without
leaking into the default UI run.

Sandboxed runs may need permission to bind or connect to `127.0.0.1`.

## Known-bug tests

Known-bug tests belong in the marked `Known Bug Tests` section. They document
expected behavior once a filed bug is fixed.

When adding one:

- Include the issue number in the comment and assertion name.
- State the current broken behavior in a comment.
- Run the command, capture its exit, and call `pass` only if the desired
  behavior already works.
- Call `fail` with the issue number when the known bug still reproduces.

These tests still count as failures and make the suite exit 1. Do not weaken or
skip them to make a run green. When fixing the bug, keep the coverage and turn
the conditional into a direct assertion if possible.

## Unit tests

Use focused Rust unit tests for pure helpers that do not need a process, a
database, or shell setup. Put them beside the helper under `#[cfg(test)]`.

Good examples:

- `src/util.rs` tests comma parsing, tag edits, skill edits, and date parsing.
- `src/format.rs` tests formatting helpers and UTF-8-safe truncation
  regressions.

Run all unit tests with:

```bash
cargo test
```

Run focused tests while iterating:

```bash
cargo test util::tests::parse_comma_list_basic
cargo test format::tests::pretty_list_with_em_dash_title_does_not_panic
```

Prefer unit tests for deterministic pure logic, then add integration coverage
when behavior crosses CLI parsing, SQLite state, output format contracts, audit
events, or the localhost UI API.

## Historical baseline output-diff tool

`tests/tools/baseline-diff.sh` is a **developer tool**, not part of the default
verify gate. It compares the CLI output matrix of a historical git ref (e.g.
`origin/main`, a release tag, or an old commit such as `db7e324`) against a
current target, and emits a report listing changed commands, changed exit
statuses, and normalized unified diffs.

Reach for it when you **deliberately change the CLI output standard** and want
to review the full delta against a released or remote ref — the one-off
"compare origin to the worktree" exploration, made repeatable. Do **not** put it
in CI: building an old ref is a full release build and can be slow, and the
day-to-day "did output drift from its checked-in baseline" job is already owned
by the [normalized snapshot harness](#normalized-output-snapshot-harness).

| Use the snapshot gate (`tests/contracts/*.sh`) | Use the baseline-diff tool |
| --- | --- |
| Every commit / `just verify` / CI. | On demand, when changing the output standard. |
| Asserts output vs. **checked-in** baselines. | Compares output vs. a **historical git ref**. |
| Fast, isolated temp DBs, no build of other refs. | Builds the baseline ref in an isolated worktree. |
| Fails the build on any drift. | Never fails on drift — drift is the report. |
| Source of truth for "is current output correct". | Review aid for "how did output change vs. release". |

How it stays safe:

- It **refuses to run on a dirty working tree** (guarded by
  `git status --porcelain`) unless `--allow-dirty` is passed, because the
  "current" target is ambiguous with uncommitted changes (exit 3).
- The baseline ref is checked out into a **detached `git worktree`** under a
  temp dir with an isolated `CARGO_TARGET_DIR`, built there, and removed on
  exit. The user's working tree, index, and HEAD are never mutated.
- It reuses the snapshot harness's `contract_normalize` (sourced from
  `tests/contracts/_lib.sh`) so entropy stripping matches the gate exactly.

Typical invocation (clean tree, building both sides):

```bash
# Compare the current working tree against origin/main.
tests/tools/baseline-diff.sh --baseline origin/main --out /tmp/itr-delta.txt

# Compare against a tag, reusing a prebuilt current binary.
tests/tools/baseline-diff.sh --baseline v2.9.6 \
  --target-binary ./target/release/itr --out /tmp/itr-delta.txt
```

Run `tests/tools/baseline-diff.sh --help` for the full flag list
(`--baseline`, `--target-binary`, `--baseline-binary`, `--out`,
`--allow-dirty`, `--skip-baseline-build`, `--keep-temp`).

The tool ships with an auto-discovered smoke test,
`tests/contracts/baseline_tool.sh`, that `just verify` runs. The smoke test
deliberately exercises the tool's **control flow** — the dirty-tree guard,
argument validation, the `--allow-dirty` happy path, and a stub-vs-real diff
that proves changed commands, a changed exit status, and a unified diff are
rendered — using the suite's resolved binary on both sides via the
explicit-binary flags, so it stays fast and never performs a full cross-ref
build.
