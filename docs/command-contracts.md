# Command Contracts

This document records stable command behavior for contributors. It is not clap
help. Keep stdout machine-consumable, preserve aliases, and treat stderr as the
place for diagnostics, hints, warnings, review notes, and progress.

## Global Contract

All commands accept the global parser flags from `src/cli.rs`:

- `-f, --format`: `compact`, `json`, `pretty`, or `oneline`. Default is
  `compact`. Unknown formats exit before handler dispatch.
- `--db`: database path override. For commands that require a database,
  `ITR_DB_PATH` currently takes precedence over `--db`, then walk-up search is
  used. `init` uses `--db`, then `ITR_DB_PATH`, then `./.itr.db`.
- `--fields`: comma-separated field selector. It is stable for issue, list,
  search, and batch JSON outputs, and for issue/list compact and list pretty
  outputs where the formatter supports field checks.
- `-q, --quiet`: accepted globally for compatibility. Do not rely on it to
  change parseable stdout in current command contracts.

Commands with no database requirement: `init`, `agent-info`,
`getting-started`, `skill`, `schema`, and `upgrade`. Other commands open the
resolved SQLite database before dispatch.

## Stdout And Stderr

- Successful command data goes to stdout.
- Runtime errors go to stderr. In JSON mode, runtime errors are JSON objects
  with `error` and `code`; otherwise they are `ERROR: ...`.
- Soft-fallback review messages, hints, and progress go to stderr and should
  not corrupt stdout.
- Argument parse errors are clap errors and exit before command handlers.
- `show --all` may emit a hint to stderr before normal list output.
- `upgrade` progress is stderr in non-JSON mode.
- `ui` browser-open failures are `REVIEW:` messages on stderr.

## Exit Contract

- Success exits 0.
- Empty result sets are not errors and exit 0.
- Handler/runtime errors exit 1: not found, no database, parse errors,
  invalid hard-validation errors, dependency cycles, DB/IO errors, no filters
  for bulk operations, and failed upgrades.
- Clap parse errors use clap's exit behavior.
- Batch `close`, `update`, and `note` represent per-item failures in the batch
  result envelope and still exit 0 unless stdin parsing or a non-item handler
  error fails.
- `doctor` prints its report, then exits 1 when problems remain.

## Empty Results

JSON empty results use `[]` on stdout. Non-JSON empty results use a terse
message or blank formatted output, depending on the command path.

Stable empty-result messages:

- `list`, `search`: `No matching issues found.`
- `ready`: `No ready issues found.`
- `next`, `claim`, `start` without an explicit ID: `No eligible issues found.`
- `log`: `No events found.`
- `search` with no terms: `No search terms provided.`

## Soft Fallbacks

Preserve these as non-fatal behavior:

- Priority synonyms normalize before validation: `urgent`, `p0`, `highest` to
  `critical`; `p1` to `high`; `p2`, `normal` to `medium`; `p3`, `lowest` to
  `low`.
- Kind synonyms normalize before validation: `enhancement`, `feat`, `story` to
  `feature`; `bugfix`, `defect` to `bug`; `chore`, `subtask` to `task`.
- Status synonyms normalize before validation: `todo`, `new`, `backlog` to
  `open`; `closed`, `resolved`, `fixed` to `done`; `cancelled`, `canceled` to
  `wontfix`; `wip`, `started`, `progress`, `in_progress`, `inprogress` to
  `in-progress`.
- `add` invalid priority/kind defaults to `medium`/`task`, adds
  `_needs_review`, creates an `itr` note, and exits 0.
- `update` invalid status/priority/kind defaults to `open`/`medium`/`task`,
  adds `_needs_review`, creates an `itr` note, and exits 0.
- `batch add` invalid priority/kind defaults to `medium`/`task`, marks the item
  `review`, adds `_needs_review`, and exits 0.
- `batch update` invalid status/priority/kind keeps the existing value, marks
  the item `review`, adds `_needs_review`, and exits 0.
- Unknown `--fields` names emit `REVIEW:` to stderr and are ignored if absent
  from the output shape.
- `skill install` refuses to overwrite an existing skill without `--force`,
  emits `REVIEW:` to stderr, preserves the file, and exits 0.
- Hidden compatibility flags `add --title` and `close --reason` take precedence
  over the positional value when both are provided and emit a `REVIEW:` warning
  to stderr.

## Compatibility Aliases

Visible aliases from `src/cli.rs` are part of the public contract:

| Alias | Canonical command or flag |
| --- | --- |
| `itr create` | `itr add` |
| `itr deps` | `itr depend` |
| `itr getting-started` | `itr agent-info` |
| `itr getting started` | preprocessed to `itr getting-started` |
| `itr start` | `itr claim` |
| `itr current` | `itr wip` |
| `itr batch create` | `itr batch add` |
| `itr list --tags` | `itr list --tag` |
| `itr relate --type` | `itr relate --relation-type` |
| `itr bulk update --filter-status` | `itr bulk update --status` |
| `itr bulk update --filter-priority` | `itr bulk update --priority` |

Accepted hidden compatibility spellings:

- `itr add --body` as an alias for `--context`.
- `itr add --title` as a flag-form title.
- `itr close --reason` as a flag-form reason.

## Output Families

### Issue Detail

Commands: `add`, `create`, `get`, `show <ID>`, `update`, `close`, `next`,
`claim`, `start`, `assign`, `unassign`.

- JSON is an `IssueDetail`: issue fields flattened with `urgency`,
  `blocked_by`, `blocks`, `is_blocked`, `notes`, optional
  `urgency_breakdown`, optional `children`, and optional `relations`. Close and
  terminal updates may add `unblocked`.
- Compact starts with `ID:<id> STATUS:<status> PRIORITY:<priority> KIND:<kind>
  URGENCY:<score>` and optional dependency tokens, followed by stable labeled
  lines such as `TAGS:`, `FILES:`, `SKILLS:`, `ASSIGNED:`, `TITLE:`,
  `CONTEXT:`, `ACCEPTANCE:`, `PARENT:`, `CLOSE_REASON:`, `CREATED:`,
  `UPDATED:`, and optional sections.
- Pretty is human text headed by `Issue #<id>: <title>`.
- Oneline currently uses the compact issue-detail formatter.

### Issue Lists

Commands: `list`, `ready`, `wip`, `current`, `show` without ID.

- JSON is an array of `IssueSummary`.
- Compact is one issue block per item, separated by a blank line.
- Pretty is a table with selected columns. Empty pretty output is empty.
- Oneline is one tab-separated row per issue:
  `id status priority kind "title"` plus optional `assigned_to`.

### Search Results

Command: `search`.

- JSON is an array of `SearchResult` with `matched_fields` and optional
  `context_snippets`.
- Compact uses issue-summary labels plus `MATCHED:` and `SNIPPET[field]:`
  lines when snippets are present.
- Pretty and oneline currently use the same table formatter.

### Stats And Summary

Commands: `stats`, `summary`.

- `stats -f json` is a `Stats` object with totals, status/priority/kind maps,
  blocked/ready counts, average urgency, skill and assignee maps, and optional
  oldest-open detail. Compact, pretty, and oneline share labeled compact lines.
- `summary -f json` is a session summary object with counts, completion
  percent, oldest open issue, in-progress issues, ready issues, and recent
  events. Non-JSON modes share compact narrative lines beginning with
  `PROJECT:`.

### Graph

Command: `graph`.

- JSON is `{ "nodes": [...], "edges": [...] }`.
- Compact emits `NODE:` and `EDGE:` lines.
- Pretty emits Graphviz DOT.
- Oneline currently also emits Graphviz DOT.

### Events

Command: `log`.

- JSON is an array of `Event`.
- Compact emits `EVENT:<id> ISSUE:<id> FIELD:<field> ...`.
- Pretty and oneline share a table formatter.

### Batch Results

Commands: `batch add`, `batch create`, `batch close`, `batch update`,
`batch note`.

- JSON is a `BatchResult`: `action`, `results`, `summary`, and optional
  `dry_run`.
- Per-item `outcome` is usually `ok`, `error`, or `review`.
- Compact, pretty, and oneline share the compact envelope:
  `<ACTION>: <n> items (<ok> ok, <error> error, <review> review)` followed by
  per-item lines.

### Bulk Results

Commands: `bulk close`, `bulk update`.

- JSON is a `BulkResult`: `action`, `count`, `ids`, optional `unblocked`, and
  `dry_run`.
- Compact, pretty, and oneline share `<ACTION>: <n> issues [ids]` with
  optional `(dry-run)` and `UNBLOCKED:` lines.

### Notes

Commands: `note`, `note-delete`, `note-update`.

- JSON is a `Note`.
- Compact, pretty, and oneline share `NOTE:<note_id> ISSUE:<issue_id> ...` for
  create/update and `DELETED NOTE:<note_id> ISSUE:<issue_id>` for delete.

### Other JSON Objects

- `init -f json`: `{ "action": "init", "path": ..., "created": bool }`.
- `depend -f json`: `{ "action": "depend", "blocked_id": ..., "blocker_id":
  ..., "created": bool }`.
- `undepend -f json`: `{ "action": "undepend", "blocked_id": ...,
  "blocker_id": ... }`, followed by an unblocked JSON array if applicable.
- `relate -f json`: `{ "source_id": ..., "target_id": ..., "relation_type":
  ..., "created": bool }`.
- `unrelate -f json`: `{ "source_id": ..., "target_id": ..., "removed": bool
  }`.
- `config get -f json`: `{ "key": ..., "value": ... }`.
- `config set -f json`: `{ "action": "set", "key": ..., "value": ... }`.
- `config reset -f json`: `{ "action": "reset" }`.
- `import -f json`: `{ "action": "import", "imported": n, "skipped": n }`.
- `doctor -f json`: `{ "problems": [...], "fixed": [...], "clean": bool }`.
- `ui -f json`: `{ "url": ..., "db_path": ..., "port": n }`.
- `agent-info -f json`: `{ "guide": ... }`.
- `skill -f json`: `{ "skill": ... }`.
- `skill install -f json`: `{ "installed": ... }`.
- `skill path -f json`: `{ "path": ... }`.
- `schema -f json`: `{ "schema": ... }`.
- `reindex -f json`: `{ "action": "reindex", "indexed": n }`.
- `upgrade -f json`: `{ "action": "upgrade", "old_version": ...,
  "new_version": ..., "source": ..., "binary": ..., "pulled": bool,
  "new_changes": bool }`.

`export` is intentionally governed by `--export-format`, not by `-f`: default
stdout is JSONL, and `--export-format json` stdout is a JSON array.

## Command Matrix

| Command | Input contract | Output contract |
| --- | --- | --- |
| `init` | Creates or opens the target `.itr.db`; `--agents-md` idempotently appends agent guidance. | Init object or `INIT: <path>`. |
| `add`, `create` | Positional title or `--stdin-json`; stores priority, kind, context, files, tags, skills, acceptance, blockers, parent, assignee. | Issue detail. |
| `list` | Filters issue summaries by status, priority, kind, tags, skills, blocked state, parent, assignee; sorts and limits. Default includes open and in-progress issues, including blocked. | Issue list. |
| `get` | Requires issue ID. | Issue detail or not-found error. |
| `update` | Requires issue ID; replaces fields, appends/removes tags/files/skills, sets parent and assignee. | Issue detail, plus `unblocked` when terminal status unblocks work. |
| `close` | Requires issue ID; optional reason, `--wontfix`, or `--duplicate-of`. | Issue detail; duplicate close also creates a duplicate relation. |
| `note` | Requires issue ID and text; `--agent` overrides `ITR_AGENT`. | Note. |
| `note-delete` | Requires note ID. | Deleted note. |
| `note-update` | Requires note ID and new text. | Updated note. |
| `depend`, `deps` | Requires blocked issue ID and `--on <blocker_id>`; detects cycles. | Depend object or `DEPEND: <blocked> blocked by <blocker>`. |
| `undepend` | Requires blocked issue ID and `--on <blocker_id>`. | Undepend object or `UNDEPEND: ...`, with optional unblocked notification. |
| `next` | Selects highest-urgency open, unblocked issue; can filter by skill or assignee; `--claim` sets in-progress and may assign agent. | Issue detail or empty result. |
| `ready` | Lists unblocked non-terminal issues; can filter by status, skill, assignee, and limit. | Issue list or empty result. |
| `batch add`, `batch create` | Reads JSON array of add objects from stdin; supports `blocked_by` integer IDs and `@N` intra-batch references. | Batch result with issue details; transactional creation. |
| `batch close` | Reads JSON array `{id, reason?, wontfix?}`; `--dry-run` previews. | Batch result with per-item outcomes and unblocked items. |
| `batch update` | Reads JSON array of update objects; `--dry-run` previews. | Batch result with per-item outcomes and unblocked items. |
| `batch note` | Reads JSON array `{id, text, agent?}`; item agent overrides `ITR_AGENT`. | Batch result. |
| `bulk close` | Requires at least one filter; closes all matches; `--dry-run` previews. | Bulk result. |
| `bulk update` | Requires at least one filter; applies shared status/priority/tag changes to all matches; `--dry-run` previews. | Bulk result. |
| `graph` | Emits dependency and relation graph; `--all` includes terminal issues. | Graph output. |
| `stats` | Reads all issues and current urgency config. | Stats output. |
| `summary` | Reads project counts, ready work, in-progress work, and recent events. | Summary output. |
| `export` | Reads all issues, notes, dependencies, events, and relations. | JSONL by default or JSON array with `--export-format json`. |
| `import` | Reads JSON array or JSONL from `--file` or stdin; `--merge` skips existing IDs. | Import object or `IMPORT: <imported> imported, <skipped> skipped`. |
| `doctor` | Checks orphaned deps, cycles, stale in-progress issues, empty epics, done blockers, and FTS health; `--fix` fixes safe issues. | Doctor report; exits 1 if problems remain. |
| `ui` | Binds a local HTTP UI to `127.0.0.1`; `--port 0` auto-selects; `--no-open` suppresses browser launch; `--allow-dangerous` enables the raw SQL UI/API. | UI URL and DB path, then serves until stopped. |
| `config list` | Reads effective config defaults plus overrides. | JSON object of key/value strings or `key=value` lines with `*` for custom values. |
| `config get` | Requires config key. | Config get object or `key=value`; unknown keys are errors. |
| `config set` | Requires key and value. | Config set object or `SET: key=value`. |
| `config reset` | Resets stored config overrides. | Config reset object or `CONFIG: Reset to defaults`. |
| `agent-info`, `getting-started`, `getting started` | No database; emits baked agent guide. | Guide text or guide JSON object. |
| `skill` | No subcommand emits baked skill text. | Skill text or skill JSON object. |
| `skill install` | Writes `SKILL.md` to user or project scope; refuses existing file without `--force`. | Installed path object or install line; existing-file refusal is stderr-only review. |
| `skill path` | Computes install target for scope without writing. | Path object or plain path. |
| `schema` | No database; emits compiled schema SQL string. | Schema text or schema JSON object. |
| `upgrade` | Finds source dir, optionally pulls, builds release, and installs over current executable. | Upgrade object or upgrade summary; progress on stderr. |
| `claim`, `start` | With ID, claims that issue; without ID, same selection as `next --claim`; optional skill/agent/assignee filters. | Issue detail or empty result. |
| `assign` | Requires issue ID and agent. | Issue detail with `assigned_to` set. |
| `unassign` | Requires issue ID. | Issue detail with `assigned_to` cleared. |
| `log` | Lists audit events globally or for one issue; supports limit, since, and agent filter. | Event list or empty result. |
| `relate` | Requires source ID, `--to <target_id>`, and relation type `duplicate`, `related`, or `supersedes`. | Relation object or `RELATION:created|exists ...`. |
| `unrelate` | Requires source ID and `--from <target_id>`. | Unrelate object or `RELATION:removed|not_found ...`. |
| `reindex` | Rebuilds FTS index. | Reindex object or `REINDEX: Rebuilt FTS index for <n> issues`. |
| `search` | Query terms use AND semantics across indexed/searchable fields; supports filters and limit. | Search results or empty result. |
| `wip`, `current` | Shorthand for in-progress issue list, including blocked issues. | Issue list. |
| `show` | With ID, same contract as `get`; without ID, lists non-terminal issues including blocked; `--all` includes terminal issues. | Issue detail or issue list. |
