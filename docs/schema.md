# Schema and migrations

`src/db.rs` is the source of truth for SQLite schema, migrations, DB helpers,
FTS, dependency cycle checks, and event logging. `src/models.rs` is the public
JSON shape layered over the stored rows.

The live schema is the `SCHEMA` string, which declares every table, column,
index and trigger, plus the optional FTS5 index (`FTS_CREATE`/`FTS_TRIGGERS`).
A writable open brings any older file up to that shape. The file's schema
generation is stored in `PRAGMA user_version` (`SCHEMA_VERSION`, currently 1).

## Connection setup

Every normal DB-backed command opens the database through `db::open_db(path)`,
which calls `open_schema_db`. That function:

- opens the connection and sets `busy_timeout=5000` and `foreign_keys=ON`;
- refuses a file whose `user_version` is above `SCHEMA_VERSION`
  (`NEWER_SCHEMA`) before any write;
- **fast path:** returns with zero writes when every expected table, column,
  index and trigger is present (derived from `SCHEMA` and the FTS DDL) and the
  generation and writer stamps are current. `journal_mode=WAL` is not re-set
  here, because it persists in the file;
- **read-only handle:** fails with `READONLY_NEEDS_MIGRATION` only if a table
  or column is missing; otherwise it serves reads and leaves index, trigger,
  FTS and stamp work for the next writable open;
- **slow path:** sets WAL, then in one `BEGIN IMMEDIATE` transaction it
  re-checks the generation, runs `migrate_current_schema`, re-runs the
  idempotent `SCHEMA` (the reconcile that lands every index and trigger),
  creates or repairs FTS, and stamps `user_version` and `last_writer_version`.

`init_db(path)` runs the same slow path, but executes `SCHEMA` first because a
new file has no tables. `itr schema` prints `SCHEMA_PRAGMAS` plus `SCHEMA`: the
complete table, column, index and trigger set, but not the FTS objects or the
generation. See [docs/migrations.md](migrations.md#the-current-mechanism) for
the full flow.

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
- `skills`: required text, default `[]`; JSON array encoded in TEXT. Older
  databases get it from `migrate_add_skills`.
- `acceptance`: required text, default empty.
- `parent_id`: optional self-reference to `issues(id)`, `ON DELETE SET NULL`.
- `close_reason`: required text, default empty.
- `created_at`: UTC ISO 8601 text from SQLite `strftime`.
- `updated_at`: UTC ISO 8601 text from SQLite `strftime`.
- `assigned_to`: required text, default empty; older databases get it from
  `migrate_add_assigned_to`.

Migrated databases carry `skills` and `assigned_to` at the end of the table
(after `updated_at`); fresh ones have them in the order above. All itr SQL
names its columns, so the physical order does not matter; never use
`SELECT *` or positional inserts against `issues`.

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
- `last_writer_version` is reserved. It records the release core (`vX.Y.Z`)
  of the itr that last opened the file writable, and `NEWER_SCHEMA` quotes it.
  `config set` rejects it, and `config reset` keeps it.

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

Optional FTS5 virtual table for issue search, declared with `content=''` and
`contentless_delete=1` (requires SQLite >= 3.43; the bundled build qualifies).
`contentless_delete` lets rows be removed by rowid alone, without knowing the
previously indexed values, which keeps the delete-then-insert reindex pattern
correct for every writer.

Columns (rows use `rowid = issues.id`):

- `title`
- `context`
- `acceptance`
- `tags_text`
- `files_text`
- `skills_text`
- `close_reason`

Sync triggers:

Three triggers on `issues` keep the index fresh for every SQL write path,
including raw SQL writers that never call `fts_index_issue` (for example the
UI's dangerous SQL mode):

- `issues_fts_ai` (`AFTER INSERT`): delete-then-insert the new row's entry.
- `issues_fts_ad` (`AFTER DELETE`): delete the row's entry.
- `issues_fts_au` (`AFTER UPDATE OF title, context, acceptance, tags, files,
  skills, close_reason`): delete-then-insert. Restricting `UPDATE OF` to the
  searchable columns means the `trg_issues_updated_at` touch trigger and
  status-only updates skip the reindex.

The triggers index the `tags`, `files`, and `skills` columns as their raw JSON
text; the default unicode61 tokenizer treats punctuation as separators, so the
tokens are identical to the space-joined values written by `fts_index_issue`.

Creation and legacy migration:

- `try_create_fts` runs inside the slow-path migration transaction of
  `open_db` and `init_db`. It creates the table and triggers idempotently, and
  populates the index from existing issues when the table is newly created.
  The fast path counts a missing, legacy, or trigger-less index as missing
  schema, so a writable open always repairs it.
- If any sync trigger is missing, rows written in the meantime may be stale,
  so `try_create_fts` drops and rebuilds the whole index instead of only
  recreating the trigger.
- Creation failure is ignored so itr can run with SQLite builds that lack FTS5;
  search then uses the LIKE fallback.
- One-time auto-migration: if an existing `issues_fts` predates the
  `contentless_delete` + trigger design (`fts_is_legacy` checks the stored
  `CREATE` SQL for `contentless_delete`), `try_create_fts` drops the legacy
  table and triggers, then recreates and repopulates. The legacy contentless
  table could not delete a rowid's old tokens, so updates left stale terms
  searchable.

Manual indexing:

- `fts_index_issue` remains the public per-issue entry point
  (delete-then-insert by rowid, arrays joined with spaces); `insert_issue`,
  searchable-field updates, and `import` still call it even though the triggers
  already cover those writes. Failures emit a `REVIEW:` warning to stderr
  instead of failing the command.
- `fts_rebuild` (used by `reindex` and `doctor --fix`) drops the table and
  triggers, then calls `try_create_fts` to recreate and repopulate; it errors
  only when FTS5 is unavailable.

Search behavior:

- `search` uses FTS when `has_fts` is true. If FTS returns no IDs, it falls back
  to the LIKE search path in case the index is stale.
- Without FTS, search uses LIKE over issue text fields and note content.
- Note content is not in `issues_fts`; note-only matches are found only through
  the LIKE fallback path.
- `doctor` reports the index as stale (`fts_stale`) when the FTS row count
  differs from the issue count, and rebuilds it with `--fix`. It also reports
  `missing_schema_object` (any expected table, column, index, trigger or FTS
  object is missing) and `stale_schema_generation` (`user_version` below
  `SCHEMA_VERSION`). Both are fixable on a writable handle by the same
  reconcile transaction `open_db` runs; on a read-only handle they are not
  fixable.
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

[docs/migrations.md](migrations.md) is the authoritative guide. It holds the
ledger of every shipped schema change, the current open and migrate mechanism,
per-change-type playbooks, rules for when to bump `SCHEMA_VERSION`, and the
PR checklist. In short:

- The slow-path transaction in `migrate_and_stamp_schema` runs, in order:
  1. `migrate_current_schema`: `migrate_add_skills`, `migrate_add_assigned_to`,
     `migrate_add_events`, `migrate_add_relations`;
  2. the `SCHEMA` reconcile;
  3. `try_create_fts`, which also rebuilds a legacy or trigger-less index;
  4. `stamp_schema_version`.
- Columns and tables need a `migrate_*` helper plus the `SCHEMA` declaration.
  Indexes and triggers only need the `SCHEMA` declaration.
- The generation guard is `PRAGMA user_version`. Bump `SCHEMA_VERSION` for any
  change an older binary would mishandle; the `SCHEMA_FINGERPRINT` unit test
  forces the decision for DDL changes.
- Keep migration SQL compatible with bundled SQLite through `rusqlite`.
- New issue columns must also be added to import's explicit column lists
  (`insert_issue_row`, `replace_issue_row`).

## SQL safety

- Use `params!` or generated placeholders for SQL values.
- If a column name is dynamic, validate it against an allowlist first.
  `update_issue_field` is the reference pattern.
- Keep stdout parseable and emit warnings/errors to stderr from command layers.
- Avoid hard issue deletion from UI workflows; prefer resolve, wontfix, or
  cleanup tags.
