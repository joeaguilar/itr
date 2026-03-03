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
- `itr bulk close` — Close all matching filters (--status, --priority, --kind, --tag, --skill, --assigned-to, --dry-run)
- `itr bulk update` — Update matching issues (--set-status, --set-priority, --add-tag, --dry-run)

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

Issues are ranked by a computed urgency score (never stored, always fresh). Default coefficients:
- `w_priority`: critical=4, high=3, medium=2, low=1
- `w_age`: 0.1 per day since creation
- `w_dependency`: +2 per issue blocked by this one
- `w_update_lag`: 0.05 per day since last update
- `w_blocker_bonus`: +5 if this issue blocks others

Override via `itr config set <key> <value>`. View breakdown with `itr get <ID> -f json` (urgency_breakdown field).

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
