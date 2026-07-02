# itr — Agent-First Issue Tracker CLI

[![CI](https://github.com/joeaguilar/itr/actions/workflows/ci.yml/badge.svg)](https://github.com/joeaguilar/itr/actions/workflows/ci.yml)

A local, zero-config issue tracker built for AI coding agents. SQLite-backed, single binary, no daemon, no network, no auth.

```
cargo install --path .
itr init
itr skill install     # optional: brief Claude Code agents on itr
itr add "Fix auth bug" -p high -k bug --tags "auth,security" --files "src/auth.rs"
itr ready -f json
```

## Why itr?

AI coding agents need persistent memory across sessions. `itr` gives them a local issue database they can read from, write to, and reason about — without any setup, configuration, or network access.

- **Agent-first**: compact output format minimizes token usage; JSON mode for structured data
- **Zero config**: one `.itr.db` file, no daemon, no git hooks, no YAML
- **Deterministic**: sorted output, consistent exit codes, no interactive prompts
- **Composable**: stdout is always parseable data, stderr is always errors
- **Fast**: single-digit millisecond operations on SQLite

## Install

No Rust toolchain required — prebuilt binaries are published on every release for macOS (Intel + Apple Silicon), Linux (x86_64 glibc/musl, aarch64), and Windows (x86_64 + arm64).

On x86_64 Linux the installer downloads the **fully-static musl build by default** so it runs on any distro regardless of glibc version. The glibc artifact is still published if you'd rather grab it manually from the [Releases page](https://github.com/joeaguilar/itr/releases/latest).

### macOS / Linux

```bash
curl -fsSL https://raw.githubusercontent.com/joeaguilar/itr/main/install.sh | bash
```

The script auto-detects your platform, downloads the matching tarball from the latest GitHub Release, verifies its SHA256 checksum, and installs to an existing `itr` location on `PATH`, `~/.cargo/bin` if it is already on `PATH`, or `~/.local/bin`.

To update an existing install, rerun the installer:

```bash
curl -fsSL https://raw.githubusercontent.com/joeaguilar/itr/main/install.sh | bash -s -- --update
```

Environment overrides:

| Variable | Effect |
| -------- | ------ |
| `ITR_VERSION` | Pin a specific tag (e.g. `v0.1.0`). Defaults to latest. |
| `ITR_INSTALL_DIR` | Install directory. Defaults to the active `itr` on `PATH`, `~/.cargo/bin`, or `~/.local/bin`. |
| `ITR_FROM_SOURCE=1` | Skip download, build with cargo (must be run from a cloned repo). |

See [docs/environment.md](docs/environment.md) for the full list (including `ITR_REPO` and runtime variables).

### Windows

```powershell
iwr -useb https://raw.githubusercontent.com/joeaguilar/itr/main/install.ps1 | iex
```

Installs `itr.exe` into `%LOCALAPPDATA%\Programs\itr` and adds that directory to your user PATH. Use `-Version`, `-InstallDir`, or `-Repo` parameters to override defaults.

### Manual download

Grab a release archive for your platform from [GitHub Releases](https://github.com/joeaguilar/itr/releases/latest), verify the bundled `.sha256`, extract it, and drop `itr` (or `itr.exe`) anywhere on your `PATH`.

### From source

If you'd rather build locally — or no prebuilt binary exists for your target — you'll need a Rust toolchain (2021 edition or newer; SQLite is compiled from source via the `bundled` feature).

```bash
cargo install --git https://github.com/joeaguilar/itr   # remote
# or
git clone https://github.com/joeaguilar/itr && cd itr && cargo install --path .
```

### Nix

```bash
nix profile install https://github.com/joeaguilar/itr
# or in a dev shell:
nix develop
```

### Uninstall

```bash
./uninstall.sh   # macOS / Linux; removes itr from common install dirs
```

On Windows, delete `%LOCALAPPDATA%\Programs\itr\itr.exe` and remove that directory from your user PATH.

## Quick Start

```bash
# Initialize in your project root
itr init

# Create issues
itr add "Fix login timeout" -p high -k bug -c "Users report 30s hangs"
itr add "Add rate limiting" -p medium -k feature --tags "api,security"
itr add "Write integration tests" -p low -k task -a "cargo test integration passes"

# Set up dependencies
itr depend 3 --on 1    # issue 3 blocked by issue 1

# See what's ready to work on
itr ready

# Grab the top task
itr next --claim

# Log progress
itr note 1 "Investigated timeout — root cause is connection pool exhaustion"

# Close with reason
itr close 1 "Fixed pool size in config, verified with load test"

# Check project health
itr stats
```

## Documentation

- [Architecture](docs/architecture.md) - CLI flow, DB boundaries, formatting,
  UI server, and tests.
- [Command contracts](docs/command-contracts.md) - stable command behavior,
  aliases, output formats, and exit rules.
- [Schema and migrations](docs/schema.md) - SQLite tables, migrations, FTS, and
  audit/event behavior.
- [Search](docs/search.md) - what `itr search` indexes, AND-by-default query
  semantics, FTS5 vs LIKE fallback, and when to run `itr reindex`.
- [Urgency scoring](docs/urgency.md) - the full coefficient table, per-component
  math, and worked examples for `itr next` / `itr ready` ordering.
- [UI API](docs/ui-api.md) - localhost JSON API used by `itr ui`.
- [Security model](docs/security.md) - localhost binding, UI token behavior, and
  local trust boundaries.
- [Backup, import, and export](docs/backup-import-export.md) - `.itr.db`
  backups, JSONL/JSON export, import, and recovery checks.
- [Environment variables](docs/environment.md) - canonical reference for every
  `ITR_*` variable read by the CLI, installers, and upgrade.
- [Troubleshooting](docs/troubleshooting.md) - install, PATH, database, UI,
  search, and upgrade recovery.
- [Testing](docs/testing.md) - contributor test commands and integration-suite
  conventions.
- [Known limitations and roadmap](docs/limitations.md) - intentional constraints,
  compatibility coverage, and future directions.
- [Changelog](CHANGELOG.md) - release history and upgrade notes.

## Output Formats

All commands support `--format` / `-f` with four modes:

### compact (default)

Token-efficient, one issue per block. Optimized for AI agents.

```
ID:1 STATUS:open PRIORITY:high KIND:bug URGENCY:17.0 BLOCKS:3
TAGS:auth,security
FILES:src/auth.rs
TITLE: Fix login timeout
ACCEPTANCE: cargo test auth passes
```

### json

Full structured output. Always valid JSON.

```bash
itr get 1 -f json
```

```json
{
  "id": 1,
  "title": "Fix login timeout",
  "status": "open",
  "priority": "high",
  "kind": "bug",
  "urgency": 17.0,
  "blocked_by": [],
  "blocks": [3],
  "is_blocked": false,
  "notes": [],
  "files": ["src/auth.rs"],
  "tags": ["auth", "security"]
}
```

### pretty

Human-readable table format.

```
   # |   Urg | Status      | Pri      | Kind    | Title                                    | Blocked
-----|-------|-------------|----------|---------|------------------------------------------|--------
   1 |  17.0 | open        | high     | bug     | Fix login timeout                        |
   2 |  11.0 | open        | medium   | feature | Add rate limiting                        |
```

### oneline

Tab-separated single line per issue. Useful for piping into `cut`, `awk`,
or shell loops.

```
1	open	high	bug	"Fix login timeout"
2	open	medium	feature	"Add rate limiting"
```

Columns are `id`, `status`, `priority`, `kind`, `"title"`, and `assignee`
(only emitted when set).

## Commands

Every variant of the CLI is grouped below. Subcommands of `batch`, `bulk`,
`config`, and `skill` are listed under their respective parent tables.

### Core Workflow

| Command | Description |
|---------|-------------|
| `itr init` | Create `.itr.db` in the current directory (`--agents-md` appends instructions to `AGENTS.md`) |
| `itr add <TITLE>` | Create a new issue (alias: `itr create`) |
| `itr list` | List issues (default: open/in-progress, unblocked, by urgency) |
| `itr get <ID>...` | Full detail for one or more issues (`1 2 3`, `1,2,3`, or ranges `5-8`) |
| `itr update <ID>` | Modify issue fields |
| `itr close <ID>... [REASON]` | Close one or more issues as done (`12,14,17`, ranges `5-8`; `--reason`, `--wontfix`, `--duplicate-of <ID>`) |
| `itr show` | All non-terminal issues; `itr show <ID>...` aliases `itr get` |
| `itr wip` / `itr current` | Show in-progress issues (shorthand for `list -s in-progress`) |
| `itr ui` | Start a localhost browser UI for issue editing |

### Notes

| Command | Description |
|---------|-------------|
| `itr note <ID>... <TEXT>` | Append a timestamped note to one or more issues (`55 56 57`, `1,2,3`, or ranges `5-8`) |
| `itr note-update <NOTE_ID> <TEXT>` | Replace a note's content |
| `itr note-delete <NOTE_ID>` | Delete a note by ID |

### Dependencies & Relations

| Command | Description |
|---------|-------------|
| `itr depend <ID>... --on <ID>` | Mark one or more issues as blocked by another (alias: `itr deps`; multi-ID and ranges) |
| `itr undepend <ID> --on <ID>` | Remove a dependency |
| `itr relate <ID>... --to <ID> --type related\|duplicate\|supersedes` | Relate one or more issues to a target (e.g. `itr relate 124-132 --to 53`) |
| `itr unrelate <ID> --from <ID>` | Remove a relation between two issues |
| `itr graph` | Output the dependency graph (JSON or DOT format) |

### Agent Workflow

| Command | Description |
|---------|-------------|
| `itr next` | Single highest-urgency unblocked open issue |
| `itr next --claim` | Same, but atomically sets it to in-progress |
| `itr claim` / `itr start` | Alias for `itr next --claim` (accepts optional explicit `<ID>`; deliberately single-ID — claiming is one-at-a-time) |
| `itr ready` | All unblocked non-terminal issues, sorted by urgency |
| `itr assign <ID> <AGENT>` | Assign an issue to an agent |
| `itr unassign <ID>` | Clear an issue's assignee |
| `itr search "<QUERY>"` | Full-text search across all fields (see [docs/search.md](docs/search.md)) |
| `itr reindex` | Rebuild the full-text search index |
| `itr log [ID]` | View event history (audit log); omit `ID` for recent activity across all issues |

### Bulk Operations

Which one do I want? **`batch`** reads a JSON array on stdin so each item
carries its own changes (explicit list, per-item values); **`bulk`** applies
the same change to every issue matching CLI filters (filter-based, one shared
value). For an explicit list of IDs with one shared change, the mutating
verbs themselves also take multiple IDs — `itr close 12,14,17 "reason"`,
`itr relate 124-132 --to 53` — no loop or JSON needed.

| Command | Description |
|---------|-------------|
| `itr batch add` | Bulk-create issues from JSON array on stdin (alias: `itr batch create`; `--dry-run` validates without writing) |
| `itr batch close` | Bulk-close issues from JSON array on stdin (per-issue reasons; `--dry-run`) |
| `itr batch update` | Bulk-update issues from JSON array on stdin (per-issue changes incl. `parent_id`/`parent`; `null` or `no_parent: true` clears the parent; `--dry-run`) |
| `itr batch note` | Bulk-add notes from JSON array `[{id, text, agent?}]` on stdin (`--dry-run`) |
| `itr bulk close` | Close every issue matching `--status/--priority/--kind/--tag/--skill/--assigned-to` (`--reason`, `--wontfix`, `--dry-run`) |
| `itr bulk update` | Update fields (`--set-status`, `--set-priority`, `--add-tag`) on every issue matching filters (`--dry-run`) |
| `itr bulk relate` | Relate every issue matching filters to `--to <ID>` (`--type`, `--dry-run`; self-edges skipped) |
| `itr bulk depend` | Block every issue matching filters on `--on <ID>` (`--dry-run`; self-edges skipped, cycles hard-error) |
| `itr bulk note` | Append the same note to every issue matching filters (`--agent`, `--dry-run`) |

### Project Management

| Command | Description |
|---------|-------------|
| `itr stats` | Counts by status/priority/kind, blocked ratio, average urgency |
| `itr summary` | Project narrative for session start (combines stats + ready + recent activity) |
| `itr doctor` | Integrity checks (orphaned deps, stuck issues, cycles) |
| `itr doctor --fix` | Auto-fix safe issues |
| `itr export` | Export all data as JSONL (or `--export-format json`) |
| `itr import --file <PATH>` | Import from JSONL/JSON (supports `--merge`) |
| `itr schema` | Dump the database schema SQL |
| `itr upgrade` | Rebuild and reinstall itr from source (`--no-pull`, `--source-dir <PATH>`) |

### Configuration

| Command | Description |
|---------|-------------|
| `itr config list` | Show all settings (urgency coefficients and other tunables) |
| `itr config get <KEY>` | Print a single config value |
| `itr config set <KEY> <VALUE>` | Tune urgency scoring or other settings |
| `itr config reset` | Restore all defaults |

### Agent Onboarding

| Command | Description |
|---------|-------------|
| `itr agent-info` / `itr getting-started` | Print the full agent usage guide (no database required) |
| `itr skill` | Print the embedded `SKILL.md` to stdout (composable) |
| `itr skill install` | Install the Claude Code skill (`--scope user\|project`, `--force`) |
| `itr skill path` | Print the install target path without writing (`--scope user\|project`) |

## itr ui

```bash
itr ui
itr ui --db path/to/.itr.db
itr ui --port 8787 --no-open
itr ui --allow-dangerous --no-open
```

Starts a local web UI bound to `127.0.0.1`. It serves embedded assets from the
`itr` binary and uses a per-session token in the browser URL for API requests.
The UI supports search/filter, add, edit, close/wontfix, notes,
dependencies, relations, previewed bulk resolve workflows, and a raw SQL editor
only when started with `--allow-dangerous`. Without that flag, the raw SQL API is
disabled. Dangerous mode can read or mutate any table in the SQLite database.
See [UI API](docs/ui-api.md) for route details and [Security model](docs/security.md)
for the localhost trust boundary.

### Global Flags

These flags are accepted by every subcommand.

| Flag | Description |
|------|-------------|
| `-f, --format <FORMAT>` | Output format: `compact` (default), `json`, `pretty`, `oneline` |
| `--db <PATH>` | Override database path (skips the walk-up search). Lower precedence than `ITR_DB_PATH` for everything except `itr init`, where the CLI flag wins |
| `--fields <LIST>` | Comma-separated list of fields to include in output — all four formats (e.g. `--fields id,title,urgency`). Output honors the requested order: `oneline` emits the selected fields as tab-separated columns (script-ready TSV), `pretty` builds its table columns from the list, and JSON re-serializes the surviving keys in the given order. Soft-fallback on typos: unknown field names emit a `REVIEW:` note on stderr and are simply omitted from the output |
| `-q, --quiet` | Suppress non-essential output |

Valid `--fields` names (mirrors the serialized JSON shape; unknown entries are
warned about and dropped):

```
id, title, status, priority, kind, context, files, tags, skills, acceptance,
parent_id, assigned_to, close_reason, created_at, updated_at, urgency,
blocked_by, blocks, is_blocked, notes, urgency_breakdown, children,
matched_fields, unblocked, context_snippets, relations,
action, results, summary, outcome, error, total, ok, review, dry_run
```

The first block applies to issues; the second block covers batch/bulk result
envelopes and search-result metadata.

## itr add

```bash
# Basic
itr add "Fix the bug"

# Full options
itr add "Fix auth timeout" \
  -p critical \
  -k bug \
  -c "Users see 30s hangs on login" \
  --tags "auth,performance" \
  --files "src/auth.rs,src/pool.rs" \
  -a "cargo test auth::timeout passes" \
  --blocked-by "1,2" \
  --parent 5

# From JSON on stdin (avoids shell escaping)
echo '{"title":"Fix bug","priority":"high","kind":"bug","context":"long text..."}' \
  | itr add --stdin-json
```

**Fields**: `title` (required), `priority` (critical/high/medium/low), `kind` (bug/feature/task/epic), `context`, `files`, `tags`, `acceptance`, `blocked-by`, `parent`.

**Fuzzy matching**: Synonyms are normalized automatically — `urgent`→`critical`, `enhancement`→`feature`, `wip`→`in-progress`, etc. Truly invalid values are accepted with a `_needs_review` tag and defaulted to safe values.

## itr list

```bash
itr list                          # open + in-progress, unblocked, by urgency
itr list --all                    # all statuses
itr list -s open                  # only open
itr list -k bug -p critical       # bugs with critical priority
itr list --tag auth               # issues tagged 'auth'
itr list --blocked                # only blocked issues
itr list --include-blocked        # include blocked in results
itr list --parent 5               # children of epic #5
itr list --sort id -n 10          # by id, limit 10
```

## itr batch add

Bulk-create issues from a JSON array. Supports `@N` references for intra-batch dependencies.

```bash
cat <<'EOF' | itr batch add
[
  {"title": "Fix SQL injection", "priority": "critical", "kind": "bug"},
  {"title": "Add parameterized queries", "priority": "high"},
  {"title": "Write security tests", "blocked_by": ["@0", "@1"]}
]
EOF
```

`@0` refers to the first issue in the batch, `@1` to the second, etc. The batch runs in one transaction with per-item soft fallback: a malformed item becomes a per-item `error` result while the valid items are still created, and recoverable problems (unknown keys, unrecognized priority/kind, missing parent or `blocked_by` target) create the issue anyway with a `REVIEW:` note and the `_needs_review` tag. Validate a payload without writing anything using `--dry-run` — it runs the exact same parse/validate path and reports the same per-item verdicts, including the resolved priority/kind defaults:

```bash
itr batch add --dry-run < sprint1-stories.json   # per-item verdicts, nothing written
```

## Urgency Scoring

Every issue has a computed urgency score that drives `itr next` and `itr ready`. The score is never stored — it's always computed fresh from current state.

```
urgency = priority + kind + blocking + blocked + age + in_progress + acceptance + notes
```

Tune the coefficients per-project with `itr config set` (e.g. `itr config set urgency.priority.critical 15.0`); `itr config list` shows every key and `itr config reset` restores defaults.

See [docs/urgency.md](docs/urgency.md) for the full coefficient table, per-component formulas, and a worked example.

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success (including empty result sets) |
| 1 | Error (not found, validation, DB error, cycle detected) |

Empty results are not errors — `itr list` with no matches exits 0 and outputs `[]` in JSON mode.

## Database

`itr` stores everything in a single `.itr.db` SQLite file. It finds the database by walking up from the current directory, or you can set `ITR_DB_PATH` to point at a specific file (see [docs/environment.md](docs/environment.md) for the full precedence rules).

```bash
# Use env var
export ITR_DB_PATH=/path/to/.itr.db

# Or CLI flag
itr list --db /path/to/.itr.db
```

### Schema

Four tables: `issues`, `dependencies`, `notes`, `config`. Run `itr schema` to see the full SQL.

## Environment Variables

`itr` reads a small set of environment variables. They are all optional — every
behavior they enable also has an explicit CLI flag or sensible default.

| Variable | Purpose |
|----------|---------|
| `ITR_DB_PATH` | Override the `.itr.db` location. Wins over `--db` for every command except `itr init` (where `--db` wins). |
| `ITR_AGENT` | Default agent identity for claims, notes, and audit-log entries. |

See [docs/environment.md](docs/environment.md) for the full list, scopes,
precedence rules, and the installer-side variables (`ITR_VERSION`,
`ITR_INSTALL_DIR`, `ITR_FROM_SOURCE`, `ITR_REPO`, `ITR_SOURCE_DIR`).

## Agent Integration

### CLAUDE.md / AGENTS.md

Always invoke as `itr` (on PATH). Never use full binary paths like `~/.cargo/bin/itr` or `./target/release/itr`.

```bash
itr init --agents-md   # appends instructions to AGENTS.md
```

Or manually add to your `CLAUDE.md`:

```markdown
## Issue Tracking

This project uses `itr` for issue tracking. Before starting work, run `itr ready -f json`
to find the next actionable task. After completing work, run `itr close <ID> "reason"`.
File discovered issues with `itr add`. Always run `itr note <ID> "summary"` before ending a session.
```

### Claude Code skill

`itr` ships a Claude Code skill that auto-fires when an agent detects an issue-filing intent and points it at `itr agent-info` as the source of truth. The skill content is baked into the binary, so this works the same whether you installed via the curl/PowerShell scripts, prebuilt tarballs, or `cargo install`.

```bash
itr skill install                        # ~/.claude/skills/itr/SKILL.md (user scope)
itr skill install --scope project        # ./.claude/skills/itr/SKILL.md (project scope)
itr skill                                # print SKILL.md to stdout (composable)
itr skill path                           # show install target without writing
```

`install` refuses to overwrite an existing file without `--force` (soft fallback: emits a `REVIEW:` note to stderr and exits 0). Re-run with `--force` after `itr upgrade` to pick up any new conventions baked into a newer binary.

### Typical Agent Session

```bash
# 1. Understand the landscape
itr stats -f json
itr ready -f json

# 2. Grab top task
ISSUE=$(itr next --claim -f json)
ID=$(echo "$ISSUE" | jq -r '.id')

# 3. Do the work...

# 4. Log what happened
itr note "$ID" "Refactored auth module. All tests pass." --agent "claude-session-001"

# 5. Close it
itr close "$ID" "Implemented in commit abc123"

# 6. File anything discovered
itr add "Edge case in token refresh" -p high -k bug --files "src/auth/refresh.rs"

# 7. Show what's next
itr ready -f json --limit 5
```

## Testing

```bash
cargo build --release
./tests/integration.sh
```

The integration test suite covers init, add, list, get, update, close, notes, dependencies (including cycle detection), next, ready, batch add, graph, stats, export/import round-trip, config, doctor, exit codes, and environment variable overrides.
See [Testing](docs/testing.md) for contributor test conventions.

## License

MIT
