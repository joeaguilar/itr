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

There are no unit tests yet — only the shell-based integration test suite in `tests/integration.sh`. The test suite uses `python3 -c` with `json.load` for JSON parsing (not `jq`).

## Architecture

### Core Flow

`main.rs` parses CLI args via clap derive macros (`cli.rs`), resolves the database path, and dispatches to command handlers. Two commands (`init`, `schema`) don't need an existing database; all others require one.

### Key Modules

- **`cli.rs`** — Clap `#[derive(Parser)]` definitions for all commands and subcommands. Adding a new command means: add variant to `Commands` enum here, add match arm in `main.rs::run_command`, create handler in `src/commands/`.
- **`db.rs`** — All SQLite operations. Contains the schema as a const string, CRUD functions for issues/notes/dependencies/config, cycle detection via BFS, and the walk-up `.itr.db` finder. This is the largest file (~600 lines).
- **`models.rs`** — All data structs (`Issue`, `Note`, `IssueDetail`, `IssueSummary`, `BatchAddInput`, `GraphOutput`, `Stats`, `ExportData`). Uses `serde` derive for JSON serialization. `IssueDetail` uses `#[serde(flatten)]` on its `issue` field.
- **`urgency.rs`** — Urgency scoring engine. Scores are never stored — always computed fresh from current state. `UrgencyConfig` loads coefficients from the `config` table with hardcoded defaults. The `compute_urgency_with_breakdown` function returns both the score and a component breakdown.
- **`format.rs`** — Output formatting for three modes: `compact` (token-efficient default), `json`, `pretty` (human tables/DOT graphs). Each data type has its own `format_*` function.
- **`error.rs`** — `ItrError` enum with `thiserror` derive. Maps each variant to an exit code (all are 1) and a machine-readable error code. `handle_error` prints to stderr (JSON in json mode) and exits. `exit_empty` exits with code 2 for empty result sets.

### Command Handlers (`src/commands/`)

Each file exports a `run()` function that takes `&Connection`, command-specific args, and `Format`. Commands return `Result<(), ItrError>` and print to stdout directly. The `depend.rs` module also exports `run_undepend`. The `config.rs` module exports `run_list`, `run_get`, `run_set`, `run_reset`.

### Database

Four tables: `issues`, `dependencies`, `notes`, `config`. Schema defined as a const in `db.rs`. WAL mode, foreign keys enabled. `files` and `tags` are JSON arrays stored in TEXT columns. DB is found by walking up from cwd, or via `ITR_DB_PATH` env var or `--db` flag.

### Exit Codes

- 0: success
- 1: error (not found, validation, DB error, cycle)
- 2: empty result set (query OK but no matches)

### Output Contract

stdout is always parseable data (or empty). stderr is always errors. No interactive prompts ever. All timestamps are UTC ISO 8601.

## Dependencies

Minimal: `clap` (derive), `rusqlite` (bundled SQLite), `serde`/`serde_json`, `chrono`, `thiserror`. No async, no network crates.
