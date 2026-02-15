# itr — Agent-First Issue Tracker CLI

A local, zero-config issue tracker built for AI coding agents. SQLite-backed, single binary, no daemon, no network, no auth.

```
cargo install --path .
itr init
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

### Quick install (recommended)

```bash
./install.sh
```

The installation script will:
- Build the release binary
- Offer installation to `~/.cargo/bin`, `/usr/local/bin`, or a custom location
- Verify the binary is in your PATH
- Guide you through any additional setup

To uninstall later:

```bash
./uninstall.sh
```

### From source (any platform)

```bash
cargo install --path .
```

### Build manually

```bash
git clone https://github.com/joeaguilar/itr
cd itr
cargo build --release
# Binary at ./target/release/itr
```

### Nix

```bash
nix profile install https://github.com/joeaguilar/itr
# or in a dev shell:
nix develop
```

**Requirements**: Rust 2021 edition. No system dependencies — SQLite is compiled from source via the `bundled` feature.

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

## Output Formats

All commands support `--format` / `-f` with three modes:

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

## Commands

### Core Workflow

| Command | Description |
|---------|-------------|
| `itr init` | Create `.itr.db` in the current directory |
| `itr add <TITLE>` | Create a new issue |
| `itr list` | List issues (default: open/in-progress, unblocked, by urgency) |
| `itr get <ID>` | Full detail for one issue |
| `itr update <ID>` | Modify issue fields |
| `itr close <ID> [REASON]` | Close an issue as done |
| `itr note <ID> <TEXT>` | Append a note to an issue |

### Dependencies

| Command | Description |
|---------|-------------|
| `itr depend <ID> --on <ID>` | Mark an issue as blocked by another |
| `itr undepend <ID> --on <ID>` | Remove a dependency |
| `itr graph` | Output the dependency graph (JSON or DOT format) |

### Agent Workflow

| Command | Description |
|---------|-------------|
| `itr next` | Single highest-urgency unblocked open issue |
| `itr next --claim` | Same, but atomically sets it to in-progress |
| `itr ready` | All unblocked non-terminal issues, sorted by urgency |
| `itr batch add` | Bulk-create issues from JSON array on stdin |

### Project Management

| Command | Description |
|---------|-------------|
| `itr stats` | Counts by status/priority/kind, blocked ratio, average urgency |
| `itr doctor` | Integrity checks (orphaned deps, stuck issues, cycles) |
| `itr doctor --fix` | Auto-fix safe issues |
| `itr config list` | Show all urgency coefficients |
| `itr config set <KEY> <VALUE>` | Tune urgency scoring |
| `itr export` | Export all data as JSONL (or `--export-format json`) |
| `itr import --file <PATH>` | Import from JSONL/JSON (supports `--merge`) |
| `itr schema` | Dump the database schema SQL |

### Global Flags

```
-f, --format <FORMAT>    Output format: compact|json|pretty [default: compact]
    --db <PATH>          Override database path
-q, --quiet              Suppress non-essential output
```

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

`@0` refers to the first issue in the batch, `@1` to the second, etc. The entire batch is transactional — if any issue fails validation, none are created.

## Urgency Scoring

Every issue has a computed urgency score that drives `itr next` and `itr ready`. The score is never stored — it's always computed fresh from current state.

```
urgency = priority + kind + blocking + blocked + age + in_progress + acceptance + notes
```

### Default Coefficients

| Factor | Value | Description |
|--------|-------|-------------|
| `priority.critical` | 10.0 | |
| `priority.high` | 6.0 | |
| `priority.medium` | 3.0 | |
| `priority.low` | 1.0 | |
| `blocking` | 8.0 | Issue blocks other active issues |
| `blocked` | -10.0 | Issue is blocked (deprioritize) |
| `age` | 2.0 | Scales linearly over 10 days (0→2.0) |
| `in_progress` | 4.0 | Already started — finish it |
| `has_acceptance` | 1.0 | Has testable acceptance criteria |
| `kind.bug` | 2.0 | Bugs get a boost |
| `kind.epic` | -2.0 | Epics are containers, not direct work |
| `notes_count` | 0.5 | More notes = more investigated (max 3.0) |

### Customize

```bash
itr config set urgency.priority.critical 15.0
itr config set urgency.kind.bug 5.0
itr config list       # see all values
itr config reset      # restore defaults
```

## Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Error (not found, validation, DB error, cycle detected) |
| 2 | Empty result set (query succeeded but no matches) |

Exit code 2 lets agents distinguish "nothing found" from "something broke."

## Database

`itr` stores everything in a single `.itr.db` SQLite file. It finds the database by walking up from the current directory, or you can set `ITR_DB_PATH` to point at a specific file.

```bash
# Use env var
export ITR_DB_PATH=/path/to/.itr.db

# Or CLI flag
itr list --db /path/to/.itr.db
```

### Schema

Four tables: `issues`, `dependencies`, `notes`, `config`. Run `itr schema` to see the full SQL.

## Agent Integration

### CLAUDE.md / AGENTS.md

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

## License

MIT
