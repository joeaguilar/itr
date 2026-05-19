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
just ci
```

`./tests/integration.sh` defaults to `./target/release/itr`, so build release
first. CI runs format check, clippy, cargo-deny, release build, and the release
integration suite.

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
   `/api/health`, issue list/create/edit, and bulk resolve preview/apply.
8. Always `kill`, `wait`, and remove the temp directory.

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
