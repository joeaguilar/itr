# Contributing to itr

This repository is a Rust, single-binary, SQLite-backed issue tracker built for
AI coding agents. Keep contributions local-first, deterministic, dependency-light,
and friendly to both CLI automation and the embedded localhost UI.

## Project Invariants

- `itr` is one Rust binary. Do not add a daemon, required service, auth system,
  external database, Node toolchain, desktop shell, or async runtime unless the
  project explicitly decides to change direction.
- `.itr.db` is the source of truth. The app discovers it by `--db`,
  `ITR_DB_PATH`, or walking up from the current directory.
- stdout is parseable data. stderr is for errors, warnings, progress, and
  `REVIEW:` notes.
- Commands never prompt interactively. Every workflow must be scriptable.
- Empty result sets are successful results. They exit 0 and print `[]` in JSON
  mode.
- All timestamps stored by the application are UTC ISO 8601 strings.
- The UI is served by `itr ui`, binds to `127.0.0.1`, serves embedded vanilla
  assets, and talks to the same Rust DB helpers through a localhost JSON API.
- The UI does not hard-delete issues. Cleanup flows should resolve, wontfix, or
  tag issues for review.
- Use `itr` directly in docs and guidance. Do not document `~/.cargo/bin/itr`,
  `./target/release/itr`, or other full binary paths as normal usage.

## Setup

The repository pins the Rust toolchain in `rust-toolchain.toml`.

```bash
rustup show
cargo build
```

Optional helpers:

```bash
just --list
just check
just test-debug
just verify
```

`just` is convenient but not required. CI uses plain Cargo commands plus the
shell integration suite.

## Verification

Run the smallest check that covers your change while developing. Before a broad
change or PR, use the full gate:

```bash
cargo build
cargo check
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
./tests/integration.sh ./target/debug/itr
```

For release-style validation:

```bash
cargo build --release
./tests/integration.sh
cargo deny check
```

Local UI testing may need permission to bind and connect to `127.0.0.1` in
sandboxed environments.

## Repository Map

- `src/cli.rs` defines clap commands, subcommands, flags, aliases, and help.
- `src/main.rs` preprocesses a small number of args, resolves the database, and
  dispatches to command handlers.
- `src/db.rs` owns schema SQL, migrations, SQLite helpers, FTS helpers,
  dependency logic, event logging, and database discovery.
- `src/models.rs` contains serializable data structs used by commands, formats,
  batch operations, graph output, stats, events, and export/import.
- `src/commands/` contains command handlers. Most files export a `run` function
  that receives a `&rusqlite::Connection`, command args, and `Format`.
- `src/commands/mod.rs` contains shared command helpers for building issue
  summaries/details, urgency sorting, and unblocked notifications.
- `src/format.rs` owns compact, JSON, pretty, and oneline output, plus
  `--fields` filtering.
- `src/normalize.rs` owns fuzzy normalization and validation for priority,
  kind, and status.
- `src/urgency.rs` computes urgency fresh from current state. Urgency scores
  are not stored.
- `src/util.rs` contains small shared parsing/list helpers and focused unit
  tests.
- `src/commands/ui.rs` is the embedded localhost HTTP server and JSON API.
- `src/ui_assets/` contains embedded `index.html`, `app.css`, and `app.js`.
  Rebuild the binary after editing these files.
- `tests/integration.sh` is the main end-to-end test suite.
- `skills/itr/SKILL.md` is embedded by `src/commands/skill.rs`.
- `src/agent_docs.rs`, `AGENTS.md`, `CLAUDE.md`, and `README.md` should stay in
  sync when command behavior or agent workflow changes.
- `.github/workflows/ci.yml` runs format, clippy, cargo-deny, release build,
  and integration tests.
- `.github/workflows/auto-version.yml` tags `feat:` and `fix:` commits on
  `main`; `.github/workflows/release.yml` builds release archives and checksums.

## Adding Or Changing Commands

Follow the existing path:

1. Add or adjust clap definitions in `src/cli.rs`.
2. Add dispatch in `src/main.rs::run_command`.
3. Implement the handler in `src/commands/<name>.rs`.
4. Export the module in `src/commands/mod.rs`.
5. Add output formatting in `src/format.rs` if the command emits a new data
   shape.
6. Add or update serializable structs in `src/models.rs` when JSON output or
   stdin input needs a stable schema.
7. Add integration coverage in `tests/integration.sh`.
8. Update `README.md`, `CLAUDE.md`, `AGENTS.md`, `src/agent_docs.rs`, and
   `skills/itr/SKILL.md` if user or agent workflow changes.

Command handlers should return `Result<(), ItrError>` and print their final
output directly. Keep command-specific parsing near the handler unless it is
shared by multiple commands.

## Rust Style

- Use Rust 2021 and the pinned toolchain.
- Let `rustfmt` decide layout. Do not hand-format against it.
- Keep `cargo clippy --all-targets -- -D warnings` clean.
- The clippy policy lives in `Cargo.toml`. The project enables `all` and
  `pedantic`, allows noisy lints that do not fit the codebase, and denies
  `dbg_macro`.
- Prefer explicit, direct code over large abstractions. The existing style is
  straightforward command dispatch, small helper functions, and concrete data
  structs.
- Use `?` and `ItrError` for fallible paths. Avoid panics for user input,
  database state, request bodies, and filesystem state.
- `unwrap_or_default`, `unwrap_or(false)`, and `unwrap_or_else` are acceptable
  for deliberate soft-fallback behavior. Do not hide unrecoverable corruption or
  programmer errors behind defaults.
- Keep comments sparse and useful. Use comments to explain behavior, invariants,
  compatibility, or non-obvious control flow.
- Use existing helper APIs before adding new ones: `util::*`, `normalize::*`,
  `commands::build_issue_summary`, `commands::build_issue_detail`,
  `format::*`, and `db::*`.
- Avoid unrelated refactors while touching command handlers or DB code.
- Keep generated or embedded assets ASCII unless the surrounding file already
  requires non-ASCII.

## Data Models And Serialization

- Put externally visible structs in `src/models.rs`.
- Derive `Serialize` and `Deserialize` when a struct crosses JSON input/output
  boundaries.
- Use `#[serde(default)]` for backward-compatible additions to input or stored
  data shapes.
- Use `#[serde(skip_serializing_if = "...")]` to keep JSON output compact when
  fields are optional or empty.
- Preserve stable field names. If a JSON field must use a reserved word or
  different external name, use serde attributes such as `#[serde(rename = ...)]`.
- Keep `IssueDetail` flattened through its `issue` field unless changing the
  public JSON contract intentionally.

## Output Contract

Every command must preserve these rules:

- stdout contains data only: compact blocks, JSON, pretty tables/DOT, or empty
  success output.
- stderr contains errors, warnings, hints, progress, and `REVIEW:` messages.
- `-f json` always emits valid JSON.
- The default `compact` format is optimized for agents and should stay
  token-efficient.
- `pretty` is for humans; it can use tables or DOT graph output.
- `oneline` is tab-oriented where supported and must stay easy to parse.
- Do not print explanatory prose around JSON.
- Empty JSON result sets should print `[]`.
- If adding fields that should work with `--fields`, update `VALID_FIELDS` in
  `src/format.rs` and cover compact, pretty, and JSON behavior as applicable.

## Errors And Soft Fallbacks

The project intentionally prefers soft fallback for recoverable bad input.

Use hard errors for:

- missing database
- missing issue or note IDs
- database errors
- JSON parse errors
- dependency cycles
- unsafe bulk operations without filters
- invalid system capability or environment state

Prefer soft fallback for:

- unknown priority, kind, or status when a safe default exists
- synonym and alias handling
- partial user input where valid parts can still be applied
- existing idempotent state, such as re-adding a dependency or re-closing an
  already terminal issue
- non-critical UI conveniences, such as failing to open the browser

Soft fallback style:

- Normalize before validating with `normalize::*`.
- Emit a `REVIEW:` warning to stderr when the user should revise an invocation.
- Add `_needs_review` and an `itr` note when stored issue data was defaulted and
  needs human or agent review.
- Keep partial valid input rather than rejecting the whole operation when the
  command can safely continue.
- Never silently swallow suspicious input.

`ItrError` in `src/error.rs` owns machine-readable error codes and exit codes.
At present all hard errors exit 1. Clap usage errors may exit before reaching
`ItrError`; when possible, prefer aliases or hidden compatibility flags for
common agent mistakes.

## Database Rules

- Use `rusqlite` with bundled SQLite. Do not assume system SQLite features.
- Keep schema and migrations in `src/db.rs`.
- Enable WAL and foreign keys on every opened connection.
- Add migrations as idempotent helpers called from `open_db`.
- Store `files`, `tags`, and `skills` as JSON arrays in TEXT columns. Use
  `serde_json` to serialize and parse them.
- Use `params!` or generated placeholders for SQL values. Do not interpolate
  user data into SQL.
- If dynamically choosing a column name, validate it against an allowlist first.
  `update_issue_field` is the reference pattern.
- Preserve event/audit behavior for mutating operations. Status, priority, kind,
  title, context, acceptance, assignment, tags, skills, notes, dependencies,
  relations, and close reason changes should record events where existing
  behavior does.
- Re-index FTS after mutating searchable fields.
- Dependency insertion must remain idempotent and must reject cycles.
- Closing an issue should clean stale blocker edges and report newly unblocked
  work.

## CLI Behavior

- Keep flags and aliases compatible with existing scripts and agent guidance.
- Prefer `Option<T>` for optional single values and `Vec<T>` for repeatable
  flags.
- For comma-separated values, use `util::parse_comma_list` or
  `util::parse_comma_list_lower`.
- Skills are normalized to lowercase. Tags and files preserve case.
- `ITR_AGENT` is the fallback agent identity for claims, notes, and event logs
  where supported.
- `ITR_DB_PATH` overrides database discovery. `--db` also overrides discovery.
- Bulk commands must require at least one filter when they apply one change to
  many issues.
- Batch commands read JSON arrays from stdin and should return per-item
  outcomes instead of failing the whole batch when individual items are missing
  or invalid and continuation is safe.

## Formatting Code

`src/format.rs` is the single place for output presentation. Add formatting
helpers there instead of scattering string formats through commands, except for
small command acknowledgements that already follow existing patterns.

Formatting conventions:

- Compact issue output uses uppercase keys such as `ID:`, `STATUS:`, `TITLE:`,
  `TAGS:`, `FILES:`, `SKILLS:`, and `UNBLOCKED:`.
- Lists are sorted deterministically, usually by urgency descending or ID.
- Pretty tables use fixed-width truncation and must be UTF-8 safe.
- JSON output should use serde structs when possible. `serde_json::json!` is
  fine for small acknowledgement envelopes.
- Graph pretty output is DOT.

## Local UI Style

Keep `itr ui` dense, operational, and dependency-free.

- No Node, build step, Electron, Tauri, frontend framework, or async runtime.
- HTML, CSS, and JS live in `src/ui_assets/` and are embedded with
  `include_str!`.
- Rust routes live in `src/commands/ui.rs` and should reuse DB helpers rather
  than duplicating persistence logic.
- Mutating API routes should preserve audit events and the same soft-fallback
  semantics as CLI commands.
- The UI should remain table-first: fast filters, direct detail editing, notes,
  dependencies, relations, and previewed bulk resolve.
- Use the per-session token in the URL and `X-ITR-Token` header for API calls.
- Keep API responses JSON and errors structured with `error` and `code`.
- Escape user-visible dynamic HTML in `app.js`.
- Keep layout compact and responsive. Avoid marketing pages, decorative shells,
  and heavy visual assets.

## Testing Style

`tests/integration.sh` is the main suite and should cover user-visible behavior.

- Use `set -euo pipefail`.
- Keep tests isolated with `mktemp -d` and `trap` cleanup.
- Use the script's assertion helpers (`assert_eq`, `assert_contains`,
  `assert_exit`) instead of ad hoc checks.
- Parse JSON with `python3 -c` and `json.load`; do not add a `jq` dependency.
- Test stdout, stderr, and exit codes where behavior matters.
- Cover all output formats affected by a change, especially `-f json`.
- Add tests for aliases, soft-fallback warnings, `_needs_review`, audit events,
  `--fields`, and empty result behavior when touching those surfaces.
- UI API changes should be covered by the localhost test block or a similarly
  isolated HTTP test.
- Focused Rust unit tests are appropriate for pure helpers such as UTF-8-safe
  truncation, list parsing, and list mutation.

Known-bug tests at the end of `tests/integration.sh` document behavior the
project wants. If you fix one, update the test from "known bug" shape into a
normal passing assertion.

## Documentation

Update docs with behavior changes, not just code.

- `README.md` is the public user guide.
- `CLAUDE.md` is detailed agent/developer guidance and should explain command
  flow, architecture, and project philosophy.
- `AGENTS.md` is concise guidance for coding agents working in this repository.
- `src/agent_docs.rs` is embedded into `itr agent-info` and `itr init
  --agents-md`.
- `skills/itr/SKILL.md` is embedded into `itr skill`; rebuild after editing.
- `CHANGELOG.md` records user-facing release history and upgrade notes.
- `docs/architecture.md` explains the high-level system shape.
- `docs/command-contracts.md` records stable CLI behavior.
- `docs/schema.md` documents SQLite schema and migration rules.
- `docs/ui-api.md` documents the localhost UI API.
- `docs/security.md` documents the local UI security model.
- `docs/backup-import-export.md` documents backup and data portability.
- `docs/troubleshooting.md` documents common recovery flows.
- `docs/testing.md` documents test conventions.
- `docs/limitations.md` documents constraints, compatibility coverage, and future
  directions.
- `docs/soft_fallbacks.md` explains the philosophy behind recoverable errors.
- Keep command examples current and always invoke the CLI as `itr`.

If you change flags, aliases, output fields, workflow recommendations, urgency
scoring, UI behavior, skill installation, or agent onboarding, check all of the
above for drift.

## Dependencies

Keep dependencies minimal. Current runtime dependencies are intentionally small:

- `clap` with derive for CLI parsing
- `rusqlite` with bundled SQLite
- `serde` and `serde_json`
- `chrono`
- `thiserror`

Before adding a dependency, prefer the standard library or an existing crate
already in the tree. If a dependency is justified, update `Cargo.toml`,
`Cargo.lock`, and make sure `cargo deny check` passes under `deny.toml`.

## Issue Tracking

This repository uses `itr` for its own issues.

Before filing work, search for duplicates:

```bash
itr search "short query" -f json
```

Set attribution when claiming, noting, or closing:

```bash
export ITR_AGENT=<your-name>
itr claim
itr note <ID> "What changed"
itr close <ID> "Verified with cargo check and integration tests"
```

Use clear acceptance criteria and include relevant files, tags, skills, and
dependencies when creating issues. The conventions used for writing those
issues (title shape, required acceptance lines, tag vocabulary, body
structure) live in [STORY_STYLE.md](STORY_STYLE.md). `/sprint` and `/blitz`
read that file in Phase 0, so keep new issues consistent with it.

## Commit Messages

Commit subjects on `main` drive the auto-versioning workflow
(`.github/workflows/auto-version.yml`), so the prefix matters:

- `feat: <subject>` — produces a minor version bump (new user-visible
  capability).
- `fix: <subject>` — produces a patch version bump (bug fix or regression
  repair).
- `<type>!: <subject>` — produces a major version bump (breaking change to a
  CLI flag, JSON shape, exit-code contract, schema, or UI API). Any type
  works as long as it ends with `!`.
- Other prefixes (`docs:`, `chore:`, `refactor:`, `test:`, `ci:`, etc.) are
  accepted as plain commits and do not bump the version.
- Append `[skip version]` anywhere in the subject to bypass auto-tagging on a
  commit that would otherwise produce a tag.

Keep subjects short, imperative, and scoped to a single logical change. When a
change touches a specific subsystem, prefer a scoped prefix such as
`feat(ui):`, `fix(db):`, or `docs(testing):` — the auto-version workflow keys
off the type prefix, not the optional scope.

## Releases And Versioning

- `build.rs` sets `ITR_VERSION` from `git describe --tags --always --dirty`,
  falling back to the Cargo package version.
- CI runs on pushes and pull requests to `main`.
- Pushes to `main` can create version tags through `.github/workflows/auto-version.yml`.
  See **Commit Messages** above for the prefix contract that drives auto-tagging.
- Tags matching `v*` trigger `.github/workflows/release.yml`, which builds
  Linux, macOS, and Windows archives plus SHA256 files.
- Install scripts download release assets and verify checksums when available.
- Rerunning `install.sh` should update the active `itr` on `PATH`; keep
  installer behavior and README install/update guidance in sync.

## Related Docs

Cross-references that contributors usually want next:

- [STORY_STYLE.md](STORY_STYLE.md) — issue/story writing conventions used by
  `/sprint` and `/blitz`.
- [docs/urgency.md](docs/urgency.md) — urgency formula, full coefficient
  table, and a worked example for `itr next` / `itr ready` ordering.
- [docs/migrations.md](docs/migrations.md) — step-by-step walkthrough for
  adding a column or table to the SQLite schema, with worked case studies.

## Pull Request Checklist

Before handing off a change, confirm the relevant items:

- The change preserves stdout/stderr and exit-code contracts.
- JSON output remains valid and stable.
- Soft-fallback behavior is preserved for recoverable input problems.
- Mutating commands and UI routes record audit events where expected.
- DB migrations are idempotent.
- FTS is updated or rebuilt when searchable fields change.
- Embedded UI or skill/doc asset changes are reflected by rebuilding.
- `README.md`, `CLAUDE.md`, `AGENTS.md`, `src/agent_docs.rs`, and
  `skills/itr/SKILL.md` are updated when workflow behavior changes.
- Tests cover the changed behavior.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`,
  and the integration suite pass for the affected surface.
