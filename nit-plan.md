# `nit` — Agent-First Issue Tracker CLI

## Project Overview

Build a Rust CLI tool called `nit` (nitpick) that manages a local SQLite-backed issue database. The primary consumers are AI coding agents (e.g., Claude Code), not humans. Every design decision should optimize for machine-parseable output, minimal token overhead, stdin/stdout composability, and deterministic behavior.

The tool lives in a project directory as a single `.nit.db` file and requires zero configuration, no daemon, no network, and no authentication. Distribution targets NixOS first via a flake, with standard `cargo install` as the fallback.

---

## Competitive Landscape & Design Rationale

Several tools exist in this space. `nit` draws lessons from each while carving out a distinct niche.

### Beads / br (Steve Yegge)

Beads is the closest prior art — a git-backed issue tracker designed for AI coding agents. It uses SQLite locally with JSONL export for git-friendly collaboration. Key concepts `nit` borrows: the `ready` command for surfacing unblocked work, the philosophy that agents should kill sessions and resume from tracker state, and the idea that issue trackers serve as "persistent memory" across agent sessions.

Where `nit` diverges: Beads is a complex system (the Go version is 276K lines, the Rust port 20K) with daemon processes, git hooks, JSONL sync layers, and collision-resolution logic. `nit` is deliberately minimal — no daemon, no sync layer, no git integration by default. The database IS the source of truth. If you want git tracking, commit the `.nit.db` file or export explicitly. This is a philosophical choice: agents working on a single machine don't need distributed sync, and the complexity of sync introduces more failure modes than it solves for the solo-agent use case.

### git-bug

A distributed bug tracker that stores data as git objects (not files). Elegant architecture with bridges to GitHub/GitLab. However, it's optimized for human workflows with TUI/WebUI interfaces. Its bridge concept is interesting — `nit` could eventually support export to GitHub Issues — but the core design is too human-centric for agent-first usage.

### Taskwarrior

The most mature CLI task manager. Its urgency scoring system is brilliant — a polynomial that combines priority, age, blocking status, due date, and tags into a single numeric score. `nit` adopts a simplified version of this for its `next` command. Taskwarrior also pioneered UDAs (User Defined Attributes) which inspire `nit`'s flexible metadata approach. However, Taskwarrior predates the agent era and has no concept of machine-parseable output by default, dependency graphs, or batch operations.

### Claude Code Tasks (native)

Anthropic's built-in task system for Claude Code uses DAG-based dependencies, filesystem persistence, and cross-session state. It's tightly coupled to Claude Code's runtime. `nit` serves as an agent-agnostic alternative that any coding agent (Claude Code, Codex, Amp, Cursor) can use via CLI, and that persists independently of any agent's session management.

### Ditz, ticgit, git-issue

Older distributed trackers that store issues as files in git repos. Mostly dead projects. The lesson from their collective failure: storing issues as individual files in a repo creates merge conflicts, clutters commit history, and scales poorly. SQLite avoids all of these problems.

---

## Architecture

### Crate Layout

```
nit/
├── Cargo.toml
├── flake.nix
├── src/
│   ├── main.rs              # Entry point, CLI arg parsing
│   ├── cli.rs               # Clap command/subcommand definitions
│   ├── db.rs                # SQLite connection, migrations, schema
│   ├── models.rs            # Issue, Note, Dependency structs
│   ├── urgency.rs           # Urgency scoring engine
│   ├── commands/
│   │   ├── mod.rs
│   │   ├── init.rs           # Initialize database
│   │   ├── add.rs            # Create issues
│   │   ├── list.rs           # Query/filter issues
│   │   ├── get.rs            # Single issue detail
│   │   ├── update.rs         # Modify issue fields
│   │   ├── close.rs          # Close/resolve issues with reason
│   │   ├── note.rs           # Append agent notes
│   │   ├── depend.rs         # Add/remove dependencies
│   │   ├── next.rs           # Priority queue pick (urgency-scored)
│   │   ├── ready.rs          # List all actionable work
│   │   ├── batch.rs          # Bulk operations via JSON
│   │   ├── graph.rs          # Dependency graph output
│   │   ├── stats.rs          # Project health summary
│   │   ├── export.rs         # JSONL/JSON export
│   │   ├── import.rs         # JSONL/JSON import
│   │   ├── doctor.rs         # Database integrity checks
│   │   └── schema.rs         # Dump DB schema
│   ├── format.rs             # Output formatting (compact, json, pretty)
│   └── error.rs              # Unified error types
```

### Dependencies (Cargo.toml)

```toml
[package]
name = "nit"
version = "0.1.0"
edition = "2021"
description = "Agent-first issue tracker CLI"
license = "MIT"

[dependencies]
clap = { version = "4", features = ["derive"] }
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
```

- `rusqlite` with `bundled` — compiles SQLite from source, zero system dependency
- `clap` with `derive` — declarative CLI definition
- `serde`/`serde_json` — serialization for JSON I/O
- `chrono` — timestamps
- `thiserror` — ergonomic error types

No runtime dependencies. No async. No daemon. No network. The binary is fully self-contained.

---

## Database Schema

File: `.nit.db` in the project root (located by walking up from cwd until found, or created by `nit init`).

```sql
-- Run on init. Use WAL mode for better concurrent read performance.
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS issues (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    title           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'in-progress', 'done', 'wontfix')),
    priority        TEXT NOT NULL DEFAULT 'medium'
                    CHECK (priority IN ('critical', 'high', 'medium', 'low')),
    kind            TEXT NOT NULL DEFAULT 'task'
                    CHECK (kind IN ('bug', 'feature', 'task', 'epic')),
    context         TEXT NOT NULL DEFAULT '',
    files           TEXT NOT NULL DEFAULT '[]',    -- JSON array of file paths
    tags            TEXT NOT NULL DEFAULT '[]',     -- JSON array of strings
    acceptance      TEXT NOT NULL DEFAULT '',       -- criteria for "done"
    parent_id       INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    close_reason    TEXT NOT NULL DEFAULT '',       -- why it was closed
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS dependencies (
    blocker_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    blocked_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id != blocked_id)
);

CREATE TABLE IF NOT EXISTS notes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id        INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    content         TEXT NOT NULL,
    agent           TEXT NOT NULL DEFAULT '',       -- agent/session identifier
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS config (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);

-- Indexes for common query patterns
CREATE INDEX IF NOT EXISTS idx_issues_status ON issues(status);
CREATE INDEX IF NOT EXISTS idx_issues_priority ON issues(priority);
CREATE INDEX IF NOT EXISTS idx_issues_kind ON issues(kind);
CREATE INDEX IF NOT EXISTS idx_issues_parent ON issues(parent_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocked ON dependencies(blocked_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocker ON dependencies(blocker_id);
CREATE INDEX IF NOT EXISTS idx_notes_issue ON notes(issue_id);

-- Trigger to auto-update updated_at
CREATE TRIGGER IF NOT EXISTS trg_issues_updated_at
    AFTER UPDATE ON issues
    FOR EACH ROW
BEGIN
    UPDATE issues SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
    WHERE id = OLD.id;
END;
```

### Schema Design Notes

- `files` and `tags` are stored as JSON arrays in TEXT columns. This avoids join tables for what are essentially metadata lists. SQLite's `json_each()` can query into them if needed.
- `acceptance` is freeform text. Agents can write machine-checkable conditions here (e.g., `test::auth::token_rotation passes`, `file:src/auth.rs exists`).
- `notes` is an append-only log. Notes are never edited or deleted — they form a history of agent interactions with the issue.
- Terminal statuses are `done` and `wontfix`. An issue is "blocking" others only while in a non-terminal status.
- `kind` field distinguishes between bugs, features, tasks, and epics. Epics can serve as parent containers via `parent_id`.
- `parent_id` enables lightweight epic/subtask hierarchies without a separate table.
- `close_reason` captures why something was closed — essential for future agents to understand decisions.
- `config` table stores per-project settings (urgency coefficients, default priority, etc.).

---

## DB Location Strategy

Implement a walk-up search in `db.rs`:

1. Start from the current working directory.
2. Look for `.nit.db` in each directory.
3. Walk up to parent until found or filesystem root is reached.
4. If not found, commands other than `init` return exit code 1 with message: `No .nit.db found. Run 'nit init' to create one.`

Also support an environment variable override: `NIT_DB_PATH` — if set, use that path directly (skip walk-up). This is useful for agents that want to operate on a specific database, or for CI pipelines.

---

## Urgency Scoring Engine

Inspired by Taskwarrior's polynomial urgency model, adapted for agent workflows. Urgency is a computed float that combines multiple factors into a single sortable score. This powers both `nit next` and `nit ready`.

### Default Coefficients

```
urgency.priority.critical    = 10.0
urgency.priority.high        =  6.0
urgency.priority.medium      =  3.0
urgency.priority.low         =  1.0
urgency.blocking             =  8.0   # blocking other tasks
urgency.blocked              = -10.0  # blocked by other tasks
urgency.age                  =  2.0   # scales with age, max 10 days
urgency.has_acceptance       =  1.0   # has testable acceptance criteria
urgency.kind.bug             =  2.0   # bugs get slight boost
urgency.kind.feature         =  0.0
urgency.kind.task            =  0.0
urgency.kind.epic            = -2.0   # epics are containers, not direct work
urgency.in_progress          =  4.0   # already started, finish it
urgency.notes_count          =  0.5   # per note, max 3.0 — more context = more investigated
```

### Calculation

```
urgency = Σ (coefficient × factor)
```

Where each factor is either 0.0 or 1.0 (binary) except:
- `age`: `min(1.0, days_since_created / 10.0)` — linearly scales from 0 to 1 over 10 days
- `notes_count`: `min(1.0, notes / 6.0)` — diminishing returns
- `blocking`: 1.0 if the issue blocks at least one other active issue
- `blocked`: 1.0 if the issue is blocked by any active issue

### Configurability

Coefficients are stored in the `config` table and can be modified:

```bash
nit config set urgency.priority.critical 15.0
nit config get urgency.priority.critical
nit config list                              # show all config
nit config reset                             # restore defaults
```

This lets teams tune the scoring to match their workflow. An agent doing mostly bug triage might boost `urgency.kind.bug`, while a feature-heavy sprint might boost `urgency.kind.feature`.

---

## Output Formats

Implement in `format.rs`. Three modes controlled by `--format` / `-f` flag:

### `compact` (default)

Optimized for minimal token usage. One issue per block, blank-line separated. Fields on labeled lines, multi-value fields comma-separated.

```
ID:7 STATUS:open PRIORITY:high KIND:bug URGENCY:24.5 BLOCKED_BY:3,5
TAGS:auth,security
FILES:src/auth.rs,src/middleware.ts
TITLE: Refactor auth middleware to support token rotation
ACCEPTANCE: test::auth::token_rotation_test passes
```

For `list` output, issues are separated by blank lines. This format is trivially parseable with line-by-line string splitting.

### `json`

Full structured output. Single issues as objects, lists as arrays. Always valid JSON.

```json
{
  "id": 7,
  "title": "Refactor auth middleware to support token rotation",
  "status": "open",
  "priority": "high",
  "kind": "bug",
  "urgency": 24.5,
  "context": "Current auth middleware doesn't handle token rotation...",
  "files": ["src/auth.rs", "src/middleware.ts"],
  "tags": ["auth", "security"],
  "acceptance": "test::auth::token_rotation_test passes",
  "parent_id": null,
  "blocked_by": [3, 5],
  "blocks": [12],
  "is_blocked": true,
  "notes": [
    {
      "id": 1,
      "content": "Investigated token refresh flow...",
      "agent": "claude-code-session-abc",
      "created_at": "2025-02-14T10:30:00Z"
    }
  ],
  "created_at": "2025-02-14T09:00:00Z",
  "updated_at": "2025-02-14T10:30:00Z"
}
```

### `pretty`

Human-readable table format. Only used for occasional human inspection.

```
 #  | Urg  | Status      | Pri    | Kind | Title                                  | Blocked
----|------|-------------|--------|------|----------------------------------------|--------
 7  | 24.5 | open        | high   | bug  | Refactor auth middleware to support..  | 3, 5
 12 | 18.2 | open        | medium | task | Add rate limiting to API gateway        |
```

---

## CLI Command Reference

All commands defined via clap derive macros in `cli.rs`.

### Global Flags

```
--format, -f <FORMAT>    Output format: compact|json|pretty [default: compact]
--db <PATH>              Override database path (skips walk-up search)
--quiet, -q              Suppress non-essential output (only emit data or errors)
```

### `nit init`

Create `.nit.db` in the current directory. If one already exists, print its path and exit 0 (idempotent).

Optionally generates a snippet for `AGENTS.md` / `CLAUDE.md`:

```bash
nit init                    # create db
nit init --agents-md        # also append nit instructions to AGENTS.md
```

The `--agents-md` flag appends a block like:

```markdown
## Issue Tracking

This project uses `nit` for issue tracking. Before starting work, run `nit ready -f json`
to find the next actionable task. After completing work, run `nit close <ID> "reason"`.
File discovered issues with `nit add`. Always run `nit note <ID> "summary"` before ending a session.
```

**Output (compact):** `INIT: /path/to/.nit.db`
**Output (json):** `{"action": "init", "path": "/path/to/.nit.db", "created": true}`
**Exit codes:** 0 success, 1 error

### `nit add <TITLE>`

Create a new issue. Returns the created issue.

**Flags:**
```
<TITLE>                         Required positional argument (or --stdin-json)
--priority, -p <PRIORITY>       critical|high|medium|low [default: medium]
--kind, -k <KIND>               bug|feature|task|epic [default: task]
--context, -c <TEXT>             Freeform context/description
--files <FILES>                  Comma-separated file paths
--tags, -t <TAGS>                Comma-separated tags
--acceptance, -a <TEXT>          Acceptance criteria
--blocked-by, -b <IDS>          Comma-separated issue IDs this depends on
--parent <ID>                    Parent epic ID
--stdin-json                     Read a single JSON issue object from stdin
```

**Stdin JSON mode:** When `--stdin-json` is passed, read a JSON object from stdin with fields matching the schema. This avoids shell escaping issues for long context strings.

```bash
echo '{"title":"Fix auth bug","priority":"high","kind":"bug","context":"Stack trace:\n...long text...","files":["src/auth.rs"],"tags":["bug","auth"]}' | nit add --stdin-json
```

**Exit codes:** 0 success (prints created issue), 1 error

### `nit list`

List issues with filtering. Default: open and in-progress issues that are NOT blocked, sorted by urgency descending.

**Flags:**
```
--all                    Include all statuses (open, in-progress, done, wontfix)
--status, -s <STATUS>    Filter by status (repeatable: -s open -s in-progress)
--priority, -p <PRI>     Filter by priority (repeatable)
--kind, -k <KIND>        Filter by kind (repeatable)
--tag <TAG>              Filter by tag (repeatable, AND logic)
--blocked                Only show blocked issues
--include-blocked        Include blocked issues in results (default excludes them)
--parent <ID>            Show children of an epic
--sort <FIELD>           Sort by: urgency|priority|created|updated|id [default: urgency]
--limit, -n <N>          Max results
```

**Exit codes:** 0 results found, 2 no results match filter

### `nit get <ID>`

Get full detail for a single issue, including notes, dependency info, urgency breakdown, and children (if epic).

**Compact format includes all fields, notes appended, urgency breakdown:**
```
ID:7 STATUS:open PRIORITY:high KIND:bug URGENCY:24.5 BLOCKED_BY:3,5 BLOCKS:12
TAGS:auth,security
FILES:src/auth.rs,src/middleware.ts
TITLE: Refactor auth middleware to support token rotation
CONTEXT: Current auth middleware doesn't handle...
ACCEPTANCE: test::auth::token_rotation_test passes
CREATED: 2025-02-14T09:00:00Z
UPDATED: 2025-02-14T10:30:00Z
--- URGENCY BREAKDOWN ---
priority.high=6.0 blocking=8.0 kind.bug=2.0 age=1.8 has_acceptance=1.0 notes=0.5
--- NOTES ---
[2025-02-14T10:30:00Z] (claude-code-session-abc) Investigated token refresh flow...
```

**Exit codes:** 0 found, 1 not found

### `nit update <ID>`

Update one or more fields on an issue. Only specified fields are changed.

**Flags:**
```
<ID>                          Required issue ID
--status, -s <STATUS>         New status
--priority, -p <PRIORITY>     New priority
--kind, -k <KIND>             New kind
--title <TITLE>               New title
--context, -c <TEXT>          Replace context
--files <FILES>               Replace files list (comma-separated)
--tags, -t <TAGS>             Replace tags list (comma-separated)
--acceptance, -a <TEXT>       Replace acceptance criteria
--parent <ID>                 Set parent epic
--add-tag <TAG>               Append a tag (repeatable)
--remove-tag <TAG>            Remove a tag (repeatable)
--add-file <FILE>             Append a file (repeatable)
--remove-file <FILE>          Remove a file (repeatable)
```

When status changes to a terminal state (`done`/`wontfix`), check and report any issues that become unblocked as a result.

**Output:** The updated issue (same format as `get`), followed by unblocked notifications:
```
UNBLOCKED:12 "Add rate limiting to API gateway"
UNBLOCKED:15 "Deploy auth service v2"
```

**Exit codes:** 0 success, 1 issue not found or invalid field value

### `nit close <ID> [REASON]`

Shorthand for `nit update <ID> --status done`. The optional reason is stored in `close_reason`.

```bash
nit close 7 "Implemented in commit abc123, all tests pass"
```

If `REASON` is omitted and stdin is not a TTY, read from stdin.

Also supports `--wontfix` flag to close as wontfix instead of done:

```bash
nit close 7 --wontfix "Superseded by issue 12"
```

Reports unblocked issues, same as `update`.

**Exit codes:** 0 success, 1 not found

### `nit note <ID> <TEXT>`

Append a note to an issue's log.

**Flags:**
```
<ID>                    Issue ID
<TEXT>                   Note content (or omit and pipe via stdin)
--agent <NAME>           Agent/session identifier [default: ""]
```

Supports stdin: `echo "long note content" | nit note 7 --agent claude-session-xyz`

If `<TEXT>` is omitted and stdin is not a TTY, read from stdin.

**Exit codes:** 0 success, 1 issue not found

### `nit depend <ID> --on <ID>`

Add a dependency. Issue `<ID>` becomes blocked by `--on <ID>`.

**Validation:**
- Both issues must exist
- Cannot self-reference
- Check for cycles via BFS before inserting
- **Idempotent:** if the dependency already exists, exit 0 silently

**Output (compact):** `DEPEND: 7 blocked by 3`
**Exit codes:** 0 success, 1 error (not found, cycle detected)

### `nit undepend <ID> --on <ID>`

Remove a dependency. **Idempotent:** if it doesn't exist, exit 0. Reports unblocked issues if this was the last blocker.

**Exit codes:** 0 success, 1 issue not found

### `nit next`

Return the single highest-urgency unblocked issue in `open` status (not `in-progress`). This is the "what should I work on" command.

Uses the urgency scoring engine. Returns the top result.

Optionally auto-claim: `nit next --claim` returns the issue AND sets it to `in-progress` atomically.

**Exit codes:** 0 issue returned, 2 no eligible issues

### `nit ready`

List ALL unblocked, non-terminal issues sorted by urgency. This is the "work queue" — everything an agent could pick up right now. Unlike `next` which returns one, `ready` returns the full prioritized queue.

**Flags:**
```
--limit, -n <N>          Max results [default: unlimited]
--status <STATUS>        Filter within ready set [default: open]
```

This is the command agents should call at session start to understand the full picture.

**Exit codes:** 0 results found, 2 no eligible issues

### `nit batch add`

Bulk-create issues from a JSON array on stdin.

```bash
cat issues.json | nit batch add
```

Input format: JSON array of issue objects (same shape as `add --stdin-json`).

```json
[
  {"title": "Fix login timeout", "priority": "high", "kind": "bug", "tags": ["bug"]},
  {"title": "Add logout endpoint", "priority": "medium", "blocked_by": [1]},
  {"title": "Write auth tests", "blocked_by": ["@0", "@1"], "acceptance": "cargo test auth passes"}
]
```

**Transactional:** all issues are created in a single SQLite transaction. If any fail validation, none are created.

**Note on `blocked_by` in batch:** references can be to existing issue IDs or to other issues in the same batch by their array index prefixed with `@` (e.g., `"blocked_by": ["@0"]` means blocked by the first issue in this batch). This allows filing interdependent issues atomically.

**Output:** JSON array of created issues with their assigned IDs.

**Exit codes:** 0 all created, 1 validation error (none created)

### `nit graph`

Output the dependency graph.

**Flags:**
```
--format, -f <FORMAT>    compact|json|dot [default inherits global, dot also available]
--all                    Include resolved issues [default: only non-terminal]
```

**JSON format:**
```json
{
  "nodes": [
    {"id": 7, "title": "Refactor auth...", "status": "open", "urgency": 24.5, "is_blocked": true},
    {"id": 3, "title": "Update token lib...", "status": "in-progress", "urgency": 18.2, "is_blocked": false}
  ],
  "edges": [
    {"from": 3, "to": 7, "type": "blocks"}
  ]
}
```

**DOT format** (for Graphviz visualization):
```dot
digraph nit {
  rankdir=LR;
  3 [label="3: Update token lib..." shape=box]
  7 [label="7: Refactor auth..." shape=box style=filled fillcolor=gray]
  3 -> 7
}
```

**Exit codes:** 0 success

### `nit stats`

Project health summary. Shows counts by status, priority, kind, blocked/unblocked ratio, average urgency, and oldest open issue. Designed to give an agent (or human) a quick snapshot of project state.

**JSON output:**
```json
{
  "total": 24,
  "by_status": {"open": 12, "in-progress": 3, "done": 8, "wontfix": 1},
  "by_priority": {"critical": 2, "high": 5, "medium": 10, "low": 7},
  "by_kind": {"bug": 6, "feature": 8, "task": 9, "epic": 1},
  "blocked": 4,
  "ready": 11,
  "avg_urgency": 14.3,
  "oldest_open": {"id": 1, "title": "Setup CI pipeline", "days_old": 12}
}
```

**Exit codes:** 0 success

### `nit export`

Export the full database to a portable format.

```bash
nit export > backup.jsonl          # JSONL, one issue per line (default)
nit export --format json > all.json  # Single JSON array
```

Export includes all issues (including closed), notes, and dependencies. This is the interchange format for backup, migration, and sharing.

**Exit codes:** 0 success

### `nit import`

Import issues from JSONL or JSON.

```bash
cat backup.jsonl | nit import
nit import --file backup.jsonl
nit import --file backup.jsonl --merge   # skip existing IDs instead of erroring
```

**Transactional.** `--merge` mode skips issues whose IDs already exist rather than failing.

**Exit codes:** 0 success, 1 error

### `nit doctor`

Run integrity checks on the database and report problems.

Checks:
- Orphaned dependencies (referencing deleted issues)
- Circular dependency detection (full graph scan)
- Issues stuck in `in-progress` for more than N days (configurable, default 3)
- Epics with no children
- Issues with `done` status but still listed as blockers

```bash
nit doctor            # report only
nit doctor --fix      # auto-fix what's safe (remove orphaned deps, etc.)
```

**Exit codes:** 0 all clean, 1 problems found (2 if --fix couldn't resolve)

### `nit config`

Manage per-project configuration stored in the `config` table.

```bash
nit config list                              # show all settings
nit config get urgency.priority.critical     # get one value
nit config set urgency.priority.critical 15.0  # set a value
nit config reset                             # restore all defaults
```

**Exit codes:** 0 success, 1 key not found

### `nit schema`

Dump the current database schema as SQL. Useful for agent self-documentation.

**Exit codes:** 0 success

---

## Exit Code Convention

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | Error (not found, validation failure, DB error, cycle detected) |
| 2    | Empty result set (no matching issues — distinct from error) |

Exit code 2 is critical for agents. It lets them distinguish "the query worked but found nothing" from "something went wrong."

---

## Error Output

All errors go to **stderr** as structured messages. Stdout is reserved exclusively for data output.

**Error format (stderr):**
```
ERROR: Issue 99 not found
ERROR: Cycle detected: 7 -> 3 -> 7
ERROR: Invalid status 'pending'. Valid: open, in-progress, done, wontfix
```

With `--format json`, errors on stderr are also JSON:
```json
{"error": "Issue 99 not found", "code": "NOT_FOUND"}
```

Error codes for JSON mode: `NOT_FOUND`, `INVALID_VALUE`, `CYCLE_DETECTED`, `DB_ERROR`, `PARSE_ERROR`, `NO_DATABASE`, `STALE_IN_PROGRESS`

---

## Behavioral Contracts

These invariants must always hold:

1. **Stdout is always parseable data or empty.** Never prompts, never progress bars, never decorative text on stdout.
2. **Idempotent where possible.** `init` on existing db = no-op. `depend` on existing dependency = no-op. `undepend` on non-existent dependency = no-op.
3. **No interactive prompts.** Never ask for confirmation. Never require TTY. Every command completes with a single invocation.
4. **Deterministic ordering.** Lists are always sorted by the documented sort order. Agents can rely on the first item being highest urgency.
5. **Atomic writes.** All multi-row operations use transactions. The database is never in a half-updated state.
6. **Timestamps are always UTC ISO 8601.** No local time, no ambiguous formats.
7. **JSON output is always valid JSON.** Even error states in JSON mode produce valid JSON on stderr.
8. **Blank input fields are empty strings, not null.** `context`, `acceptance`, `agent`, `close_reason` default to `""`. `files` and `tags` default to `[]`. Agents never need null-checking.

---

## Implementation Notes

### Error Handling (`error.rs`)

Use `thiserror` to define a unified error enum:

```rust
#[derive(Debug, thiserror::Error)]
pub enum NitError {
    #[error("Issue {0} not found")]
    NotFound(i64),

    #[error("Cycle detected: {0}")]
    CycleDetected(String),

    #[error("Invalid value for {field}: '{value}'. Valid: {valid}")]
    InvalidValue { field: String, value: String, valid: String },

    #[error("No .nit.db found. Run 'nit init' to create one.")]
    NoDatabase,

    #[error("Database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

Map each variant to an exit code and a machine-readable error code for JSON output.

### Cycle Detection (`depend.rs`)

When adding dependency `blocked_id -> blocker_id`:

```
fn has_path(conn, from: i64, to: i64) -> bool:
    // BFS from `from` following blocked_by edges
    // If `to` is reachable, adding this edge would create a cycle
    queue = [from]
    visited = {}
    while queue not empty:
        current = queue.pop()
        if current == to: return true
        for each blocker of current:
            if blocker not in visited:
                visited.add(blocker)
                queue.push(blocker)
    return false
```

Before inserting `(blocker_id=B, blocked_id=A)`, check `has_path(conn, B, A)`. If true, the edge `A -> B` combined with the existing path `B -> ... -> A` forms a cycle.

### Urgency Computation (`urgency.rs`)

Urgency is always computed on read, never stored. This ensures it's always fresh with respect to current state (e.g., an issue that just became unblocked gets its score recalculated immediately).

```rust
pub fn compute_urgency(issue: &Issue, config: &UrgencyConfig, conn: &Connection) -> f64 {
    let mut score = 0.0;

    // Priority
    score += match issue.priority.as_str() {
        "critical" => config.priority_critical,
        "high" => config.priority_high,
        "medium" => config.priority_medium,
        "low" => config.priority_low,
        _ => 0.0,
    };

    // Kind
    score += match issue.kind.as_str() {
        "bug" => config.kind_bug,
        "feature" => config.kind_feature,
        "task" => config.kind_task,
        "epic" => config.kind_epic,
        _ => 0.0,
    };

    // Blocking others
    if blocks_active_issues(conn, issue.id) {
        score += config.blocking;
    }

    // Blocked by others
    if is_blocked(conn, issue.id) {
        score += config.blocked; // negative coefficient
    }

    // Age factor (0.0 to 1.0, maxing at 10 days)
    let age_days = days_since(issue.created_at);
    let age_factor = (age_days as f64 / 10.0).min(1.0);
    score += config.age * age_factor;

    // In-progress boost
    if issue.status == "in-progress" {
        score += config.in_progress;
    }

    // Has acceptance criteria
    if !issue.acceptance.is_empty() {
        score += config.has_acceptance;
    }

    // Notes count
    let notes = count_notes(conn, issue.id);
    let notes_factor = (notes as f64 / 6.0).min(1.0);
    score += config.notes_count * notes_factor;

    score
}
```

### Unblock Notification

After updating an issue to `done`/`wontfix`, query for newly unblocked issues:

```sql
SELECT i.id, i.title FROM issues i
JOIN dependencies d ON d.blocked_id = i.id
WHERE d.blocker_id = ?
AND i.status NOT IN ('done', 'wontfix')
AND NOT EXISTS (
    SELECT 1 FROM dependencies d2
    JOIN issues i2 ON d2.blocker_id = i2.id
    WHERE d2.blocked_id = i.id
    AND i2.status NOT IN ('done', 'wontfix')
);
```

### Batch Add with Internal References (`batch.rs`)

For `blocked_by` fields in batch input, parse each element:
- If it's a number, treat as an existing issue ID
- If it starts with `@`, treat as a batch-internal index (e.g., `@0` = first issue in the array)

Implementation: insert all issues first (in order) to get their IDs, then insert all dependencies in a second pass, resolving `@N` references to the actual assigned IDs. All within a single transaction.

---

## Testing Strategy

### Unit Tests

Each command module should have unit tests that operate on an in-memory SQLite database (`:memory:`). Key test cases:

- **add:** creates issue, verify all fields, verify defaults, verify kind/parent
- **list:** filtering by status, priority, kind, tag; blocked exclusion; urgency sort order
- **depend:** basic dependency, idempotent re-add, cycle detection (direct, 3-node, deep)
- **update/close:** status transitions trigger unblock notifications, close_reason stored
- **batch:** internal references resolve correctly, transaction rolls back on any validation failure
- **next/ready:** urgency scoring produces correct ordering, exit 2 when all blocked/done
- **urgency:** coefficient changes via config affect scoring
- **doctor:** detects orphaned deps, stuck in-progress, circular deps
- **export/import:** round-trip preserves all data

### Integration Tests

Shell-based tests that invoke the compiled binary and verify stdout/stderr/exit codes:

```bash
#!/bin/bash
set -e
cd $(mktemp -d)

# Init
nit init
[ -f .nit.db ]

# Add and retrieve
ID=$(nit add "Test issue" -p high -k bug -f json | jq -r '.id')
[ "$(nit get "$ID" -f json | jq -r '.priority')" = "high" ]
[ "$(nit get "$ID" -f json | jq -r '.kind')" = "bug" ]

# Dependency and blocking
ID1=$(nit add "First" -f json | jq -r '.id')
ID2=$(nit add "Second" --blocked-by "$ID1" -f json | jq -r '.id')
[ "$(nit get "$ID2" -f json | jq -r '.is_blocked')" = "true" ]

# Cycle detection
! nit depend "$ID1" --on "$ID2" 2>/dev/null
[ $? -eq 1 ]

# Ready shows only unblocked
READY_COUNT=$(nit ready -f json | jq '. | length')
[ "$READY_COUNT" -ge 1 ]

# Close with reason, check unblock
nit close "$ID1" "Done in commit abc123"
[ "$(nit get "$ID2" -f json | jq -r '.is_blocked')" = "false" ]
[ "$(nit get "$ID1" -f json | jq -r '.close_reason')" = "Done in commit abc123" ]

# Stats
[ "$(nit stats -f json | jq '.by_status.done')" = "1" ]

# Export/import round-trip
nit export > /tmp/nit-export.jsonl
cd $(mktemp -d)
nit init
nit import --file /tmp/nit-export.jsonl
[ "$(nit stats -f json | jq '.total')" -ge 1 ]

echo "All tests passed"
```

---

## Build & Distribution

### Cargo Build

```bash
cargo build --release
# Binary at target/release/nit
```

### Nix Flake

```nix
{
  description = "nit - agent-first issue tracker";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
  };

  outputs = { self, nixpkgs, rust-overlay }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
        in f pkgs
      );
    in {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "nit";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
        };
      });

      # Quick dev shell
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          buildInputs = with pkgs; [ cargo rustc rust-analyzer clippy rustfmt ];
        };
      });
    };
}
```

Install via `nix profile install github:user/nit` or add to system config. Dev shell via `nix develop`.

---

## Future Directions (v0.2+)

Features deliberately deferred from v0.1 to keep scope tight, but architecturally accounted for:

### MCP Server Mode

Add a `nit mcp` subcommand that runs nit as an MCP (Model Context Protocol) server over stdio. This would let Claude Code (and other MCP-compatible agents) interact with nit via structured tool calls instead of CLI subprocess invocations, reducing overhead and improving reliability.

The Rust MCP SDK (`rmcp` crate) supports this. Each nit command becomes an MCP tool:

```
nit_add, nit_list, nit_get, nit_update, nit_close,
nit_note, nit_next, nit_ready, nit_depend, nit_graph, nit_stats
```

This is architecturally cheap to add because the command implementations already return structured data — the MCP layer just serializes it differently.

### Git Sync (optional, opt-in)

Add `nit sync` that exports to JSONL and optionally commits to git. This would follow the Beads model but remain explicitly opt-in. The core design principle stays: the SQLite db is the source of truth, JSONL is just a portable snapshot.

### Bridge to GitHub Issues

`nit bridge github --repo user/repo` to sync issues bidirectionally with GitHub Issues. Useful for projects that want agent-local tracking but human-visible issues on GitHub.

### Templates

`nit template add bug-report '{"kind":"bug","priority":"high","tags":["bug"],"acceptance":"no regression in test suite"}'`

Then: `nit add "Login fails on Safari" --template bug-report`

Reduces boilerplate for common issue patterns.

### Hooks

Post-create, post-close, post-update hooks that run shell commands. E.g., auto-run tests when an issue is closed, or notify a webhook. Similar to git hooks, stored in `.nit/hooks/`.

---

## Agent Usage Examples

### Session start: understand the landscape

```bash
# What's the overall state?
nit stats -f json

# What can I work on right now?
nit ready -f json

# Grab the top task
ISSUE=$(nit next --claim -f json)
ID=$(echo "$ISSUE" | jq -r '.id')
FILES=$(echo "$ISSUE" | jq -r '.files | join(" ")')
echo "Working on: $(echo "$ISSUE" | jq -r '.title')"
```

### Filing issues from an audit

```bash
cat <<'EOF' | nit batch add
[
  {"title": "SQL injection in /api/users endpoint", "priority": "critical", "kind": "bug",
   "context": "Parameter 'id' in UserController.getUser() is concatenated directly into SQL query at line 47 of src/controllers/user.rs",
   "files": ["src/controllers/user.rs"], "tags": ["security", "bug"]},
  {"title": "Add parameterized queries to DB layer", "priority": "high", "kind": "task",
   "context": "Prerequisite for fixing SQL injection bugs. Need to refactor db::query() to accept params.",
   "files": ["src/db.rs"], "tags": ["security", "refactor"]},
  {"title": "Add SQL injection regression tests", "priority": "medium", "kind": "task",
   "blocked_by": ["@0", "@1"], "tags": ["security", "testing"],
   "acceptance": "cargo test security::sql_injection passes"}
]
EOF
```

### Agent work loop

```bash
# Get next task
ISSUE=$(nit next -f json)
if [ $? -eq 2 ]; then
  echo "No work available"
  exit 0
fi

ID=$(echo "$ISSUE" | jq -r '.id')

# Claim it
nit update "$ID" -s in-progress

# ... agent does work ...

# Log progress
nit note "$ID" "Refactored query builder. All tests pass." --agent "claude-session-001"

# Mark done with reason
nit close "$ID" "Implemented parameterized queries, verified with integration tests"
```

### Landing the plane (session end)

```bash
# Log what happened this session
nit note "$CURRENT_ID" "Session ending. Completed auth refactor. Tests pass. \
Discovered edge case with token expiry - filed as issue." --agent "claude-session-001"

# File any discovered work
nit add "Handle token expiry edge case in refresh flow" -p high -k bug \
  --context "During auth refactor, found that tokens within 30s of expiry cause race condition" \
  --files "src/auth/refresh.rs" --tags "auth,race-condition"

# Show what's next for the follow-up agent
nit ready -f json --limit 5
```

### Querying context before starting work

```bash
# Full picture on an issue
nit get 7 -f json

# Everything tagged 'auth' including blocked work
nit list --tag auth --include-blocked -f json

# Dependency graph to understand relationships
nit graph -f json

# Health check
nit doctor
```
