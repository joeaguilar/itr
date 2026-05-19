# AGENTS.md

Guidance for Codex and other coding agents working in this repository.

## Project

`itr` is a Rust, single-binary, SQLite-backed issue tracker for AI coding agents. It is local-first: `.itr.db` is the source of truth, there is no daemon requirement, no auth system, and no required external runtime.

The project also includes a human-facing local browser editor via `itr ui`. The UI is served from the same Rust binary, binds to `127.0.0.1`, uses embedded vanilla assets, and talks to a localhost JSON API.

## Commands

Use `itr` directly from `PATH`; do not use full paths like `~/.cargo/bin/itr` or `./target/release/itr` in docs or agent guidance.

Common checks:

```bash
cargo build
cargo check
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
cargo deny check
./tests/integration.sh ./target/debug/itr
```

`cargo deny check` runs license, advisory, and ban policy checks against
`deny.toml`; CI runs it on every push, so add or audit any new dependency
locally before opening a PR.

The integration script normalizes explicit binary paths before switching into
its temp directory, so the debug-binary command above works from the repo root.

Local UI:

```bash
itr ui
itr ui --db path/to/.itr.db
itr ui --port 8787 --no-open
itr ui --allow-dangerous --no-open
```

In sandboxed environments, UI tests may need permission to bind/connect to `127.0.0.1`.
`--allow-dangerous` enables raw SQL against the opened SQLite database and should
be treated as full database access.

Install and update:

```bash
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/joeaguilar/itr/main/install.sh | bash
curl -fsSL https://raw.githubusercontent.com/joeaguilar/itr/main/install.sh | bash -s -- --update

# Windows (PowerShell)
iwr -useb https://raw.githubusercontent.com/joeaguilar/itr/main/install.ps1 | iex
iex "& { $(iwr -useb https://raw.githubusercontent.com/joeaguilar/itr/main/install.ps1) } -Update"

# From source (any platform)
cargo install --path . --force
```

Check `install.sh` and `install.ps1` before answering install or update questions.
Both installers behave symmetrically: the release installer detects an existing
`itr` on `PATH` and replaces it in place (Unix uses `--update` / `update`;
Windows uses `-Update` or the positional `update`). Source installs use
`cargo install --path . --force`.

Claude Code skill:

```bash
itr skill                 # emit the SKILL.md body to stdout
itr skill path            # show where `itr skill install` would write
itr skill install         # write SKILL.md to the user-scope skills dir
itr skill install --project  # write to the current project's .claude/skills/
itr skill install --force # overwrite an existing SKILL.md
```

`itr skill install` is a soft fallback: refusing to overwrite an existing file
emits a `REVIEW:` note on stderr and still exits 0. The skill body itself is
`include_str!`'d from `skills/itr/SKILL.md`, so edits there require a rebuild.

## Code Map

- `src/cli.rs` defines clap commands and flags.
- `src/main.rs` resolves the DB path and dispatches command handlers.
- `src/db.rs` owns SQLite schema, migrations, and DB helpers.
- `src/models.rs` contains serializable data structs.
- `src/commands/` contains command handlers.
- `src/commands/ui.rs` serves the local browser UI and JSON API.
- `src/ui_assets/` contains embedded HTML/CSS/JS for `itr ui`; rebuild after editing these files.
- `src/format.rs` owns compact, JSON, pretty, and oneline output.
- `tests/integration.sh` is the main test suite.

## UI Rules

Keep `itr ui` dependency-light and portable. Do not add Node, Electron, Tauri, an async runtime, or a frontend build step unless explicitly requested.

All mutating UI API routes should reuse Rust DB helpers and preserve audit/event behavior where appropriate. The normal UI must not expose hard issue deletion in v1; prune-style workflows mean previewed bulk resolve or cleanup tagging. Raw SQL is only available behind `itr ui --allow-dangerous`.

The UI should stay dense and operational: table-first search/filter, direct detail editing, notes, dependencies, relations, and bulk resolve.

## Issue Tracking

This repo uses `itr` itself for project issues. Before filing an issue, search for duplicates with `itr search`. Prefer `-f json` for machine-readable output.

Set `ITR_AGENT=<name>` when claiming, noting, or closing work if attribution matters.

## Style

Follow existing Rust style and project constraints:

- stdout is parseable data; stderr is errors/warnings.
- Preserve soft-fallback behavior for recoverable bad input.
- Use `rusqlite` with bundled SQLite; avoid system SQLite assumptions.
- Avoid unrelated refactors while touching command handlers or DB code.
- Keep generated or embedded assets ASCII unless the surrounding file already requires otherwise.
