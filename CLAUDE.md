# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

`itr` is a local, zero-config issue tracker CLI built for AI coding agents. Single Rust binary, SQLite-backed (`.itr.db`), no daemon, no network, no auth. Primary consumers are AI agents, not humans.

## Build & Test

```bash
cargo build --release          # Binary at ./target/release/itr
cargo install --path .         # Install to ~/.cargo/bin
./tests/integration.sh         # Run full integration test suite (requires release build)
./tests/integration.sh ./target/debug/itr  # Run against debug build
```

A `justfile` provides shortcuts (requires [just](https://github.com/casey/just)):

```bash
just test          # release build + integration tests
just test-debug    # debug build + integration tests
just lint          # cargo clippy --all-targets -- -D warnings
just fmt           # cargo fmt --all
just verify        # release + lint + test + fmt-check (full pre-push validation)
just ci            # fmt-check + lint + test
```

There are no unit tests yet — only the shell-based integration test suite in `tests/integration.sh`. The test suite uses `python3 -c` with `json.load` for JSON parsing (not `jq`).

## Architecture

### Core Flow

`main.rs` parses CLI args via clap derive macros (`cli.rs`), resolves the database path, and dispatches to command handlers. Three commands (`init`, `schema`, `upgrade`) don't need an existing database; all others require one.

Always invoke as `itr` (on PATH). Never use full binary paths like `~/.cargo/bin/itr` or `./target/release/itr`.

### Key Modules

- **`cli.rs`** — Clap `#[derive(Parser)]` definitions for all commands and subcommands. Adding a new command means: add variant to `Commands` enum here, add match arm in `main.rs::run_command`, create handler in `src/commands/`.
- **`db.rs`** — All SQLite operations. Contains the schema as a const string, CRUD functions for issues/notes/dependencies/config, cycle detection via BFS, and the walk-up `.itr.db` finder. This is the largest file (~730 lines).
- **`models.rs`** — All data structs (`Issue`, `Note`, `IssueDetail`, `IssueSummary`, `BatchAddInput`, `GraphOutput`, `Stats`, `ExportData`, `SearchResult`, `UrgencyBreakdown`). Uses `serde` derive for JSON serialization. `IssueDetail` uses `#[serde(flatten)]` on its `issue` field.
- **`urgency.rs`** — Urgency scoring engine. Scores are never stored — always computed fresh from current state. `UrgencyConfig` loads coefficients from the `config` table with hardcoded defaults. The `compute_urgency_with_breakdown` function returns both the score and a component breakdown.
- **`format.rs`** — Output formatting for three modes: `compact` (token-efficient default), `json`, `pretty` (human tables/DOT graphs). Each data type has its own `format_*` function.
- **`normalize.rs`** — Fuzzy matching for priority/kind/status values. Normalizes synonyms (e.g., `urgent`→`critical`, `wip`→`in-progress`). Called before validation in add, update, and batch commands.
- **`error.rs`** — `ItrError` enum with `thiserror` derive. Maps each variant to an exit code (all are 1) and a machine-readable error code. `handle_error` prints to stderr (JSON in json mode) and exits. `print_empty` prints empty results to stdout and returns normally (exit 0).

### Command Handlers (`src/commands/`)

Each file exports a `run()` function that takes `&Connection`, command-specific args, and `Format`. Commands return `Result<(), ItrError>` and print to stdout directly. The `depend.rs` module also exports `run_undepend`. The `config.rs` module exports `run_list`, `run_get`, `run_set`, `run_reset`. Several CLI commands are aliases that dispatch to existing handlers: `show <ID>` → `get::run`, `show` (no ID) → `list::run`, `claim`/`start` → `next::run` with `claim=true`, `create` → `add`.

### Database

Four tables: `issues`, `dependencies`, `notes`, `config`. Schema defined as a const in `db.rs`. WAL mode, foreign keys enabled. `files`, `tags`, and `skills` are JSON arrays stored in TEXT columns. DB is found by walking up from cwd, or via `ITR_DB_PATH` env var or `--db` flag. The `skills` field represents agent capabilities required for an issue, and can be used to filter in `list`, `search`, `next`, `ready`, and `claim` commands.

### Exit Codes

- 0: success (including empty result sets)
- 1: error (not found, validation, DB error, cycle)

### Output Contract

stdout is always parseable data (or empty). stderr is always errors. No interactive prompts ever. All timestamps are UTC ISO 8601.

## Soft Fallbacks Philosophy

This project follows a **soft fallback** approach to error handling. Hard errors should be reserved for truly unrecoverable situations (DB corruption, missing database). For everything else, prefer graceful recovery:

- **Default to safe values** when input is unrecognized (e.g., unknown priority → `"medium"` with a `REVIEW:` note on stderr). The pattern in `add.rs`/`update.rs` for priority/kind normalization is the reference implementation.
- **Warn, don't fail** — emit `REVIEW:` notes to stderr and continue with a reasonable default rather than exiting with error code 1.
- **Suggest corrections** — when a value doesn't match, suggest the closest valid option ("did you mean...?").
- **Accept partial valid input** — e.g., if `--fields` contains one bad field name, filter it out and process the valid ones instead of rejecting the entire request.
- **Use the right error type** — `InvalidValue` is for user-supplied bad values, not system capability issues or diagnostic reports.
- **Never silently swallow input** — if a flag consumes a value the user likely intended for another argument, detect and warn.

When adding new validation, ask: "Can this recover with a default?" If yes, do that and append a review note. If no (e.g., cycle detection, missing DB), then a hard error is appropriate.

## Dependencies

Minimal: `clap` (derive), `rusqlite` (bundled SQLite), `serde`/`serde_json`, `chrono`, `thiserror`. No async, no network crates.
