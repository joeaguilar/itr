# Architecture

`itr` is a local-first issue tracker for coding agents. The implementation is a
single Rust binary backed by SQLite. There is no daemon, required service,
external database, auth system, or frontend build step.

## Runtime Shape

```text
CLI args
  -> src/cli.rs
  -> src/main.rs
  -> src/commands/*
  -> src/db.rs
  -> .itr.db

itr ui
  -> src/commands/ui.rs
  -> embedded src/ui_assets/*
  -> localhost JSON API
  -> src/db.rs
  -> .itr.db
```

All commands are synchronous. The UI server uses `std::net::TcpListener` and
serves one local HTTP request at a time.

## CLI Dispatch

`src/cli.rs` defines the clap parser, command enum, subcommands, flags, and
visible aliases.

`src/main.rs`:

1. preprocesses the two-word `getting started` alias into `getting-started`;
2. parses global format and field filters;
3. handles no-database commands (`init`, `schema`, `skill`, `agent-info`,
   `upgrade`);
4. resolves `.itr.db` for all database-backed commands;
5. dispatches to `run_command`.

Most command handlers live in `src/commands/<name>.rs` and expose `run(...) ->
Result<(), ItrError>`. Handlers print their final output directly and return
errors to the shared error handler.

Shared command helpers live in `src/commands/mod.rs`:

- `build_issue_summary` — borrowing wrapper that clones the `Issue`;
- `build_issue_summary_owned` — owned variant for callers with `Vec<Issue>` via
  `into_iter()`, avoiding the per-field clone storm;
- `build_issue_detail`;
- `HasUrgency` trait — abstracts the urgency score lookup so collections of
  `IssueSummary`, `SearchResult`, or other scored types share one sort path;
- `sort_by_urgency_desc` — generic over `HasUrgency`;
- `print_detail_with_unblocked`.

## Key Modules

Small leaf modules that don't deserve their own architectural section but are
load-bearing for the rest of the codebase:

- **`src/util.rs`** — pure helpers shared across command handlers: comma-list
  parsing (`parse_comma_list`, `parse_comma_list_lower`), tag/skill set edits
  (`apply_tags`, `apply_skills`), and the `days_since` ISO-date helper used by
  the urgency age factor. All helpers follow the soft-fallback rule: malformed
  input degrades to an empty list or `0.0` rather than erroring. Unit-tested
  in-file under `#[cfg(test)]`.
- **`src/agent_docs.rs`** — a single `AGENT_DOCS` const string surfaced by
  `itr agent-info` (alias `getting-started`). It teaches agents the standard
  claim/note/close workflow and the full command reference. Keep its examples
  in sync with the actual CLI when commands change — there is no automated
  drift check.

## Database Layer

`src/db.rs` owns persistence:

- schema SQL for initial database creation;
- idempotent migrations called from `open_db` (including `migrate_add_skills`
  which adds the `skills TEXT` column on existing databases);
- SQLite connection setup with WAL and foreign keys;
- issue, note, dependency, config, event, relation, and FTS helpers;
- skills helpers — the `skills` column is read, written, filtered (AND logic in
  `list`), and indexed in the FTS `skills_text` field alongside title/context;
- cycle-check helpers — `has_path` (BFS over dependency blocker edges) and
  `is_self_or_descendant` (BFS over `parent_id` edges to block parent-cycle
  creation);
- database discovery by `ITR_DB_PATH`, `--db`, or walk-up search.

Command handlers should reuse DB helpers instead of writing duplicate SQL. When
a helper accepts a dynamic column name, it must validate the name against an
allowlist before building SQL.

Detailed schema guidance lives in [schema.md](schema.md).

## Models

`src/models.rs` contains the serializable shapes shared by command handlers,
formatters, batch operations, export/import, graph output, stats, events, and
the UI API.

Patterns:

- use serde derives for JSON boundaries;
- use `#[serde(default)]` for backward-compatible additions;
- use `#[serde(skip_serializing_if = ...)]` for optional or empty output;
- keep public field names stable unless a contract change is intentional.

## Output Layer

`src/format.rs` is the presentation boundary. It owns:

- compact agent-oriented output;
- JSON output and `--fields` filtering;
- pretty tables;
- oneline output where supported;
- DOT graph output in pretty graph mode;
- batch and unblocked notifications.

The output contract is:

- stdout is parseable data or an empty success result;
- stderr is errors, warnings, hints, progress, and `REVIEW:` notes;
- JSON mode always emits valid JSON;
- empty list-like JSON results print `[]`.

Detailed command contracts live in [command-contracts.md](command-contracts.md).

## Errors And Soft Fallback

`src/error.rs` defines `ItrError`, error codes, exit codes, and `handle_error`.
Hard errors currently exit 1.

Recoverable bad input should usually use soft fallback:

- normalize near-valid values first;
- use safe defaults when the command can continue;
- emit `REVIEW:` diagnostics;
- add `_needs_review` and an `itr` note when stored issue data was defaulted.

`src/normalize.rs` is the central place for priority, kind, and status synonym
normalization.

More background lives in [soft_fallbacks.md](soft_fallbacks.md).

## Urgency

Urgency is computed, not stored. `src/urgency.rs` loads coefficients from the
`config` table with hardcoded defaults, then scores current issue state.

Inputs include:

- priority;
- kind;
- whether the issue blocks active work;
- whether the issue is blocked;
- age;
- in-progress status;
- acceptance criteria;
- note count.

Command handlers compute urgency when building summaries or details. Config
changes take effect immediately because scores are recomputed fresh.

## Local UI

`itr ui` lives in `src/commands/ui.rs`.

It:

- resolves the same database as CLI commands;
- binds to `127.0.0.1`;
- creates a per-session token;
- serves embedded `src/ui_assets/index.html`, `app.css`, and `app.js`;
- exposes a localhost JSON API for issue editing;
- reuses DB helpers for mutations.

The UI intentionally stays dependency-light: no Node, frontend framework,
desktop shell, async runtime, or build step. After editing `src/ui_assets/*`,
rebuild the Rust binary because assets are embedded with `include_str!`.

Route details live in [ui-api.md](ui-api.md). Security behavior lives in
[security.md](security.md).

## Integration Tests

`tests/integration.sh` is the main behavior suite. It builds isolated temp
databases and exercises user-visible CLI and UI API behavior.

Coverage includes:

- init, add, list, get, update, close;
- notes, dependencies, relations, graph;
- ready, next, claim, assignment;
- batch and bulk operations;
- fields filtering;
- search and FTS reindexing;
- export/import round trips;
- agent-info and skill output;
- local UI API smoke flow;
- expected exit codes and empty-result behavior.

Focused Rust unit tests exist for pure helpers where a shell integration test
would be too broad or awkward.

Testing conventions live in [testing.md](testing.md).

## Documentation Boundaries

- [../CONTRIBUTING.md](../CONTRIBUTING.md): contributor workflow and style.
- [schema.md](schema.md): database schema and migration rules.
- [command-contracts.md](command-contracts.md): CLI command behavior.
- [ui-api.md](ui-api.md): local UI API reference.
- [testing.md](testing.md): test practices.
- [backup-import-export.md](backup-import-export.md): data portability.
- [troubleshooting.md](troubleshooting.md): recovery guidance.
- [roadmap.md](roadmap.md): constraints, known limitations, and roadmap.
