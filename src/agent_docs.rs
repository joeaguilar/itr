pub const AGENT_DOCS: &str = r#"## Issue Tracking

This project uses `itr` for issue tracking. Always use `itr` directly (it is on your PATH).
Do NOT use full paths like ~/.cargo/bin/itr or ./target/release/itr.

### Setup

Set `ITR_AGENT=<your-name>` in your environment to identify yourself for claims, notes, and audit log entries.
Use `-f json` for all machine-parseable output. Use `--fields id,title,urgency,status` to reduce token usage.

### Standard Workflow

```
itr claim --agent $ITR_AGENT   # Claim highest-urgency unblocked issue
itr get <ID> -f json           # Read full detail (acceptance criteria, context, files)
# ... do the work ...
itr note <ID> "what I did"     # Record progress before ending session
itr close <ID> "reason"        # Close when done
```

### Command Reference

**Discovery:**
- `itr ready` — List unblocked, non-terminal issues sorted by urgency
- `itr next` — Get single highest-urgency unblocked issue
- `itr next --claim` / `itr claim` — Claim it (set in-progress + assign)
- `itr search "<query>"` — Full-text search across all fields
- `itr list` — List issues with filtering (--status, --priority, --kind, --tag, --skill, --assigned-to)
- `itr get <ID>` — Full detail for a single issue
- `itr show` — Alias: no args = list, with ID = get
- `itr stats` — Project health summary
- `itr graph` — Dependency graph (DOT format in pretty mode)

**CRUD:**
- `itr add "<title>"` — Create issue (-p priority, -k kind, -c context, --tags, --skills, --files, -a acceptance, --blocked-by, --parent, --assigned-to)
- `itr update <ID>` — Update fields (--status, --priority, --title, --context, --add-tag, --remove-tag, --add-skill, --remove-skill, --add-file, --remove-file)
- `itr close <ID> ["reason"]` — Close (--wontfix, --duplicate-of)

**Notes & Audit:**
- `itr note <ID> "text"` — Append timestamped note (--agent for attribution)
- `itr log [ID]` — View event history (--limit, --since)

**Dependencies & Relations:**
- `itr depend <ID> --on <ID>` — Add blocker
- `itr undepend <ID> --on <ID>` — Remove blocker
- `itr relate <ID> --to <ID> --type duplicate|related|supersedes` — Create relation
- `itr unrelate <ID> --from <ID>` — Remove relation

**Bulk Operations:**
- `itr batch add` — Bulk-create from JSON array on stdin
- `itr batch close` — Bulk-close from JSON array on stdin (per-issue reasons, soft fallback)
- `itr batch update` — Bulk-update from JSON array on stdin (per-issue changes, soft fallback)
- `itr bulk close` — Close all matching filters (--reason, --wontfix, --status, --priority, --kind, --tag, --skill, --assigned-to, --dry-run)
- `itr bulk update` — Update matching issues (--set-status, --set-priority, --add-tag, --dry-run)

Prefer `batch close`/`batch update` when you need per-issue control. Prefer `bulk close`/`bulk update` when a single filter covers all targets.

**Assignment:**
- `itr assign <ID> <agent>` — Assign issue to agent
- `itr unassign <ID>` — Unassign issue
- `itr claim` — Claim next (alias for `next --claim`)

**Maintenance:**
- `itr init [--agents-md]` — Create database (optionally write AGENTS.md)
- `itr schema` — Print database schema
- `itr agent-info` — Print this guide
- `itr doctor [--fix]` — Database integrity checks
- `itr config list|get|set|reset` — Per-project configuration
- `itr export [--export-format json|jsonl]` / `itr import [--file, --merge]` — Data portability
- `itr reindex` — Rebuild full-text search index
- `itr upgrade` — Rebuild itr from source

### Token Reduction

Use `--fields` to select only the fields you need (JSON mode only):
```
itr list -f json --fields id,title,urgency,status
itr ready -f json --fields id,title,priority
```
Valid fields: id, title, status, priority, kind, created, updated, context, files, tags, skills, acceptance, parent, assigned_to, urgency, blocked_by, notes, relations.

### Urgency Scoring

Issues are ranked by a computed urgency score (never stored, always fresh). Components:
- `urgency.priority.critical`=10, `urgency.priority.high`=6, `urgency.priority.medium`=3, `urgency.priority.low`=1
- `urgency.kind.bug`=2, `urgency.kind.feature`=0, `urgency.kind.task`=0, `urgency.kind.epic`=-2
- `urgency.blocking`=8 (blocks other active issues), `urgency.blocked`=-10 (blocked by others)
- `urgency.age`=2 (scaled by days/10, capped at 1.0)
- `urgency.in_progress`=4, `urgency.has_acceptance`=1, `urgency.notes_count`=0.5

Override via `itr config set <key> <value>`. View breakdown with `itr get <ID> -f json` (urgency_breakdown field).
View all config keys: `itr config list`.

### Skills Filtering

Add skills to issues to match agent capabilities:
```
itr add "Migrate DB" --skills "sql,devops"
itr ready --skill sql              # Only issues needing sql
itr claim --skill rust --skill sql # Issues needing both
```

### Multi-Agent Patterns

- Each agent should set `ITR_AGENT` to a unique name
- Use `itr claim --agent myname` to atomically claim work
- Use `--assigned-to myname` to filter your own issues
- Handoff: `itr assign <ID> other-agent` + `itr note <ID> "handing off because..."`

### Error Handling

- Exit 0: success (including empty result sets — empty array `[]` in JSON)
- Exit 1: error (not found, validation, DB error, cycle detection)
- stdout: always parseable data (or empty). stderr: always errors. No interactive prompts ever.
- All timestamps are UTC ISO 8601.
"#;
