# Schema and migrations

`src/db.rs` is the source of truth for SQLite schema, migrations, DB helpers,
FTS, dependency cycle checks, and event logging. `src/models.rs` is the public
JSON shape layered over the stored rows.

The live schema is the base `SCHEMA` string plus the idempotent helpers called
from `open_db`.

## Connection setup

Every normal DB-backed command opens the database through `db::open_db(path)`.
That function:

- opens the SQLite connection;
- runs `PRAGMA journal_mode=WAL`;
- runs `PRAGMA foreign_keys=ON`;
- runs idempotent migrations;
- attempts to create the optional FTS5 table.

`init_db(path)` only executes the base `SCHEMA`. Existing databases receive
migrations when they are later opened through `open_db`. `itr schema` prints the
base `SCHEMA` string, not the migration-expanded runtime schema.

## Tables

### `issues`

Primary issue records.

Important columns:

- `id`: integer primary key, autoincrement.
- `title`: required text.
- `status`: required text, default `open`; checked against `open`,
  `in-progress`, `done`, `wontfix`.
- `priority`: required text, default `medium`; checked against `critical`,
  `high`, `medium`, `low`.
- `kind`: required text, default `task`; checked against `bug`, `feature`,
  `task`, `epic`.
- `context`: required text, default empty.
- `files`: required text, default `[]`; JSON array encoded in TEXT.
- `tags`: required text, default `[]`; JSON array encoded in TEXT.
- `skills`: required text, default `[]`; JSON array encoded in TEXT. Also
  present as the `migrate_add_skills` migration for older databases.
- `acceptance`: required text, default empty.
- `parent_id`: optional self-reference to `issues(id)`, `ON DELETE SET NULL`.
- `close_reason`: required text, default empty.
- `created_at`: UTC ISO 8601 text from SQLite `strftime`.
- `updated_at`: UTC ISO 8601 text from SQLite `strftime`.
- `assigned_to`: required text, default empty; added by `migrate_add_assigned_to`.

Indexes:

- `idx_issues_status`
- `idx_issues_priority`
- `idx_issues_kind`
- `idx_issues_parent`

Trigger:

- `trg_issues_updated_at` updates `updated_at` after any issue update.

Model:

- Rows map to `models::Issue`.
- `files`, `tags`, and `skills` are parsed with `serde_json`; invalid stored
  JSON arrays soft-fallback to empty vectors.

### `dependencies`

Directed blocking edges between issues.

Important columns:

- `blocker_id`: issue that blocks work; required FK to `issues(id)`,
  `ON DELETE CASCADE`.
- `blocked_id`: issue that is blocked; required FK to `issues(id)`,
  `ON DELETE CASCADE`.
- `created_at`: UTC ISO 8601 text from SQLite `strftime`.

Constraints and indexes:

- Primary key is `(blocker_id, blocked_id)`.
- `CHECK (blocker_id != blocked_id)` rejects self-dependencies.
- `idx_dependencies_blocked` supports blocker lookup for an issue.
- `idx_dependencies_blocker` supports blocked-work lookup for an issue.

Behavior:

- `add_dependency` treats an existing edge as success and returns `false`.
- Before insert, `add_dependency` rejects cycles. It checks whether `blocked_id`
  already reaches `blocker_id` by following `blocker_id -> blocked_id` edges.
- `is_blocked` only counts blockers whose status is not `done` or `wontfix`.
- Closing an issue removes dependency edges where the closed issue was the
  blocker, after computing newly unblocked issues.
- `doctor --fix` can remove orphaned dependency rows and done/wontfix blockers.

### `notes`

Append-only-ish issue notes, with update/delete support.

Important columns:

- `id`: integer primary key, autoincrement.
- `issue_id`: required FK to `issues(id)`, `ON DELETE CASCADE`.
- `content`: required text.
- `agent`: required text, default empty.
- `created_at`: UTC ISO 8601 text from SQLite `strftime`.

Indexes:

- `idx_notes_issue`

Model:

- Rows map to `models::Note`.

### `config`

String key/value storage for local configuration.

Important columns:

- `key`: text primary key.
- `value`: required text.

Behavior:

- `config_set` uses `INSERT OR REPLACE`.
- Urgency configuration is loaded from this table, with hardcoded defaults when
  keys are absent or invalid.

### `events`

Audit log table, added by `migrate_add_events`.

Important columns:

- `id`: integer primary key, autoincrement.
- `issue_id`: required FK to `issues(id)`, `ON DELETE CASCADE`.
- `field`: required text identifying the changed field or action.
- `old_value`: required text, default empty.
- `new_value`: required text, default empty.
- `agent`: required text, default empty; populated from `ITR_AGENT`.
- `created_at`: UTC ISO 8601 text from SQLite `strftime`.

Indexes:

- `idx_events_issue`
- `idx_events_created`

Behavior:

- `record_event` is explicit; there are no DB triggers for audit rows.
- Command handlers must call `record_event` around mutating operations that need
  audit coverage.
- Current audited actions include issue field changes, close reason/status
  changes, assignment changes, note update/delete, relation add/remove, and bulk
  variants. New mutating workflows should preserve or extend this behavior.
- Note creation and dependency edge changes currently do not record events unless
  a command layer adds one.

### `relations`

Typed non-blocking issue relationships, added by `migrate_add_relations`.

Important columns:

- `id`: integer primary key, autoincrement.
- `source_id`: required FK to `issues(id)`, `ON DELETE CASCADE`.
- `target_id`: required FK to `issues(id)`, `ON DELETE CASCADE`.
- `relation_type`: required text checked against `duplicate`, `related`,
  `supersedes`.
- `created_at`: UTC ISO 8601 text from SQLite `strftime`.

Constraints and indexes:

- `UNIQUE(source_id, target_id, relation_type)` makes relation insertion
  idempotent.
- `idx_relations_source`
- `idx_relations_target`

Behavior:

- Self-relations are rejected in `add_relation` before insert.
- Re-adding the same relation returns `false`.
- Removing a relation deletes all rows for `(source_id, target_id)`, regardless
  of `relation_type`.
- Add/remove operations record audit events on `source_id`.

### `issues_fts`

Optional FTS5 virtual table for issue search.

Columns:

- `title`
- `context`
- `acceptance`
- `tags_text`
- `files_text`
- `skills_text`
- `close_reason`

Behavior:

- Created by `try_create_fts` from `open_db`.
- Creation failure is ignored so itr can run with SQLite builds that lack FTS5.
- `content=''` and `content_rowid=id` make this a contentless index managed by
  application code.
- `fts_index_issue` writes one row per issue using `rowid = issue.id`.
- Indexing joins `tags`, `files`, and `skills` arrays with spaces.
- Issue insert and searchable field updates call `fts_index_issue`.
- `search` uses FTS when `has_fts` is true. If FTS returns no IDs, it falls back
  to the LIKE search path in case the index is stale.
- Without FTS, search uses LIKE over issue text fields and note content.
- Note content is not in `issues_fts`; note-only matches are found only through
  the LIKE fallback path.
- `reindex` rebuilds FTS; `doctor` reports stale FTS row counts and can rebuild
  them with `--fix`.
- See [docs/search.md](search.md) for query semantics, the FTS5/LIKE dispatch,
  and when to run `itr reindex`.

## JSON-in-TEXT fields

`files`, `tags`, and `skills` are stored as JSON arrays in TEXT columns. Use
`serde_json::to_string` when writing and `serde_json::from_str` when reading.

Rules:

- Store arrays, not comma-separated strings.
- Keep defaults as `'[]'`.
- Preserve empty arrays as valid data.
- Filtering by tags and skills currently happens after row load in Rust.
- Search indexes these fields through their joined text forms.

## Migration rules

See [docs/migrations.md](migrations.md) for the contributor walkthrough on
adding a column or a new table, with worked case studies from the existing
migrations.

All migrations live in `src/db.rs` and are wired from `open_db`:

1. `migrate_add_skills`
2. `migrate_add_assigned_to`
3. `migrate_add_events`
4. `migrate_add_relations`
5. `try_create_fts`

Migrations must be idempotent:

- Check for a column with `PRAGMA table_info(table)` before `ALTER TABLE`.
- Check `sqlite_master` before creating a migrated table.
- Use `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, and unique
  constraints where they fit.
- Use defaults for new NOT NULL columns so old rows remain valid.
- Do not rely on a global schema version table unless one is added deliberately.
- Do not assume migration order except the order in `open_db`.
- Keep migration SQL compatible with bundled SQLite through `rusqlite`.

When adding a column:

- Add it to the base `SCHEMA` when new databases should have it immediately.
- Add an idempotent migration helper for existing databases.
- Wire the helper in `open_db`.
- Update `row_to_issue`, SELECT lists, INSERT/UPDATE helpers, and affected
  command handlers.
- Update `src/models.rs` and add `#[serde(default)]` for backward-compatible
  JSON input/output where appropriate.
- Add the field to formatting and `--fields` allowlists if it is user-visible.
- Re-index FTS after writes if the field is searchable.
- Record audit events if the field is mutable user-facing state.

When adding a table:

- Add the base table and indexes to `SCHEMA`.
- Add an idempotent migration helper for existing databases.
- Wire the helper in `open_db`.
- Use foreign keys and `ON DELETE` behavior deliberately.
- Add DB helper functions rather than issuing ad hoc SQL from commands.
- Add model structs for JSON-facing rows.
- Preserve stdout/stderr and soft-fallback behavior in commands.

## SQL safety

- Use `params!` or generated placeholders for SQL values.
- If a column name is dynamic, validate it against an allowlist first.
  `update_issue_field` is the reference pattern.
- Keep stdout parseable and emit warnings/errors to stderr from command layers.
- Avoid hard issue deletion from UI workflows; prefer resolve, wontfix, or
  cleanup tags.
