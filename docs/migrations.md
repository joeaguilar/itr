# Migrations: Schema Change How-To

This is the contributor walkthrough for changing the SQLite schema that backs
`itr`. The reference for the *current* shape of the database lives in
[`docs/schema.md`](schema.md) — this doc covers the *process* of getting a new
column or table into the wild without breaking existing `.itr.db` files.

All schema and migration code lives in
[`src/db.rs`](../src/db.rs). The base `SCHEMA` string at the top of that file
is the shape a freshly-created database starts with, and every idempotent
`migrate_*` helper wired into `migrate_current_schema` is what brings older
databases up to that shape when they are reopened.

## Guiding rules

Before touching the schema, internalize these invariants — they apply to every
migration in the tree:

- **Migrations must be idempotent.** Reopening the same database five times in
  a row must produce zero side effects after the first run. Always probe before
  you mutate: `PRAGMA table_info(<table>)` for columns, `sqlite_master` for
  tables.
- **Use `CREATE … IF NOT EXISTS`** for tables, indexes, and triggers when
  possible. Use a `sqlite_master` probe when `IF NOT EXISTS` is not enough
  (for example, when you also need to update existing data conditionally).
- **New NOT NULL columns must have a default.** Old rows already exist and
  will not be backfilled by `ALTER TABLE` otherwise. The standard pattern is
  `TEXT NOT NULL DEFAULT ''` or `TEXT NOT NULL DEFAULT '[]'` for JSON-array
  columns.
- **Do not assume migration order beyond what `open_db` wires.** Each helper
  must be safe regardless of what state the DB starts in — fresh-from-init
  (the base `SCHEMA` already applied) or any older version.
- **No global schema version table** unless the project explicitly adds one.
  The "is this column/table there yet?" probe is the version check.
- **Stick to bundled SQLite features.** `rusqlite` bundles its own SQLite,
  but FTS5 may or may not be enabled on a given build — see how
  `try_create_fts` swallows failure for the pattern when a feature is optional.
- **Mirror new schema into the base `SCHEMA` string** when fresh databases
  should also get it. The migration helper is for existing databases; the
  base `SCHEMA` is for `init_db`. Both run on init, so the helper still has
  to be a no-op when the base `SCHEMA` already applied the change.

## The eight steps

The order below is the recommended path for any schema-shape change. Steps 1–4
get the data into SQLite; steps 5–8 make sure the rest of the binary actually
sees it.

1. **Pick a migration name.** Match the existing convention:
   `migrate_add_<thing>`. Examples in tree:
   - [`migrate_add_skills`](../src/db.rs) (adds a column to `issues`)
   - [`migrate_add_assigned_to`](../src/db.rs) (adds a column to `issues`)
   - [`migrate_add_events`](../src/db.rs) (adds a new table)
   - [`migrate_add_relations`](../src/db.rs) (adds a new table)
2. **Write the idempotent migration helper** in `src/db.rs`. Probe first,
   then `ALTER`/`CREATE`. See the two worked examples below.
3. **Wire it into `migrate_current_schema`.** Append a call after the last
   existing migration in `open_db`'s migration sequence. Order matters only
   in that later migrations may depend on earlier ones — keep them
   independent when you can.
4. **Update the base `SCHEMA` const** so a freshly initialized database
   already has the column or table. Without this, `init_db` would create a
   database that immediately needs the migration to run.
5. **Update `models.rs` + DB helpers.** Add the field to the relevant struct
   (use `#[serde(default)]` for backward-compatible JSON shapes), then update
   `row_to_*`, SELECT lists, INSERT/UPDATE helpers, and any
   `update_issue_field` allowlists.
6. **Update `ExportData` and import/export round-trips** when the change adds
   a *new table* whose rows are tied to an issue. `models::ExportData`
   carries `notes`, `blocked_by`, `events`, and `relations` per issue —
   anything else issue-scoped that should survive a backup needs its own
   field there, plus matching reads in `src/commands/export.rs` and writes
   in `src/commands/import.rs`. New *columns* on `issues` flow through
   automatically via `ExportData.issue: Issue`.
7. **Touch the rest of the surface.** Walk through:
   - `src/format.rs` — add the field to compact/json/pretty output and to
     `VALID_FIELDS` if `--fields` should accept it.
   - `src/commands/` — any handler that needs to read or mutate the field.
   - `src/commands/ui.rs` and `src/ui_assets/` — the localhost UI if the
     field is user-visible.
   - `src/agent_docs.rs`, `skills/itr/SKILL.md`, `README.md`, `AGENTS.md`,
     `CLAUDE.md` — only if agent workflow changes.
   - FTS: call `fts_index_issue` after writes if the new column is searchable,
     and add the column to the FTS5 virtual table in `try_create_fts` plus
     the insert in `fts_index_issue`.
   - Audit: call `record_event` from mutating command handlers if the new
     field is user-facing mutable state.
8. **Add an idempotency test on an empty database.** The integration suite
   already verifies fresh-init schema shape (see the `INIT_SCHEMA_CHECK`
   block in `tests/integration.sh`). Mirror that pattern for your new
   column/table, and reopen the DB twice in a row to confirm the migration
   is a no-op on the second open. Then verify the field reads back through
   the relevant `models.rs` / `db.rs` helper — for a new column, that
   usually means `itr add` followed by `itr get <ID> -f json` and asserting
   the field is present.

## Worked example: adding a column (`migrate_add_skills`)

The `skills` column on `issues` is the canonical "add a column" migration in
the tree. It's a JSON-array TEXT column with a `[]` default so old rows are
valid the moment the migration runs.

### Step 1: name the migration

The function is named `migrate_add_skills` — same pattern as the others, no
prefix, no version number, just `migrate_add_<column>`.

### Step 2: write the idempotent helper

```rust
fn migrate_add_skills(conn: &Connection) -> Result<(), ItrError> {
    let has_skills: bool = conn
        .prepare("PRAGMA table_info(issues)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|col| col.as_deref() == Ok("skills"));
    if !has_skills {
        conn.execute_batch("ALTER TABLE issues ADD COLUMN skills TEXT NOT NULL DEFAULT '[]';")?;
    }
    Ok(())
}
```

Key points:

- `PRAGMA table_info(issues)` returns one row per column; column **name** is
  at index `1`.
- The probe runs every time `open_db` runs — the `ALTER` only fires when the
  column is missing. That is what makes the helper safe to re-run.
- `NOT NULL DEFAULT '[]'` means existing rows materialize the empty-array
  default automatically; no backfill loop needed.

### Step 3: wire into `migrate_current_schema`

```rust
fn migrate_current_schema(conn: &Connection) -> Result<(), ItrError> {
    migrate_add_skills(conn)?;
    migrate_add_assigned_to(conn)?;
    migrate_add_events(conn)?;
    migrate_add_relations(conn)?;
    Ok(())
}
```

`migrate_add_skills` is first because it landed first historically. Place new
migrations at the end of the chain.

### Step 4: mirror into the base `SCHEMA`

The `issues` table definition in the `SCHEMA` const already includes:

```sql
skills          TEXT NOT NULL DEFAULT '[]',
```

So `init_db` produces databases that don't actually need the migration — but
the migration still runs on every `open_db` and is a no-op when the column is
already there.

### Step 5: thread through `models.rs` and DB helpers

- `models::Issue` gained a `pub skills: Vec<String>` field.
- `row_to_issue` reads it via `parse_json_array(row.get::<_, String>(8)?)`.
- The SELECT list in `get_issue` (and every other issue read) includes
  `skills` in the column order.
- `insert_issue` accepts `skills: &[String]`, serializes it to JSON, and
  passes it through `params!`.
- `update_issue_field` is the dynamic-column allowlist; the column name is
  validated there before being interpolated.

### Step 6: ExportData

No change needed — `skills` lives on `Issue`, and `ExportData.issue: Issue`
already round-trips it through JSON.

### Step 7: surrounding surface

- `format.rs` gained a `SKILLS:` line in compact mode and a `skills` entry
  in `VALID_FIELDS`.
- `try_create_fts` includes `skills_text`, and `fts_index_issue` joins the
  `skills` array with spaces and writes it into the FTS row.
- `list`, `search`, `next`, `ready`, `claim`, and `ui` all gained a
  `--skills` filter that narrows on the parsed array.
- `util::parse_comma_list_lower` normalizes skills to lowercase on the way
  in.

### Step 8: integration coverage

The integration suite has a dedicated `--skills` block (search
`tests/integration.sh` for `--skills "Rust-Review,Database"`) that asserts:

- the field is stored lowercased,
- the JSON output contains the field,
- filtering by skill returns the right rows.

`INIT_SCHEMA_CHECK` near the top of `tests/integration.sh` also asserts the
column is present on a fresh `itr init`.

## Worked example: adding a table (`migrate_add_events`)

The `events` table is the canonical "add a table" migration. It is
issue-scoped (one row per audit event, FK to `issues(id)`), gets its own
indexes, and ships with full round-trip support through `ExportData`.

### Step 1: name the migration

`migrate_add_events` — singular `migrate_add_<table>`, matching the existing
helpers.

### Step 2: write the idempotent helper

```rust
fn migrate_add_events(conn: &Connection) -> Result<(), ItrError> {
    let has_table: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='events'",
        [],
        |row| row.get(0),
    )?;
    if !has_table {
        conn.execute_batch(
            "CREATE TABLE events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                field       TEXT NOT NULL,
                old_value   TEXT NOT NULL DEFAULT '',
                new_value   TEXT NOT NULL DEFAULT '',
                agent       TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_events_issue ON events(issue_id);
         CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at);",
    )?;
    Ok(())
}
```

Notice the pattern:

- The `sqlite_master` probe guards the `CREATE TABLE`. We could use
  `CREATE TABLE IF NOT EXISTS` here too — the probe is helpful when a future
  migration needs to also seed or migrate existing rows on first creation.
- Indexes use `IF NOT EXISTS` unconditionally — they're safe to re-create on
  every open.
- `ON DELETE CASCADE` ties events to issues so cleanup is automatic.
- `created_at` uses the same `strftime('%Y-%m-%dT%H:%M:%SZ', 'now')` UTC ISO
  8601 pattern as every other timestamp in the schema.

### Step 3: wire into `migrate_current_schema`

```rust
migrate_add_events(conn)?;
```

Added after `migrate_add_assigned_to` and before `migrate_add_relations` —
the order it landed.

### Step 4: mirror into the base `SCHEMA`

The full `CREATE TABLE events (...)` definition is duplicated in the base
`SCHEMA` const, with `CREATE TABLE IF NOT EXISTS` and the same indexes
underneath. New databases get the table on `init_db`; the migration is a
no-op on second open.

### Step 5: thread through `models.rs` and DB helpers

- `models::Event` was added with `#[derive(Debug, Clone, Serialize, Deserialize)]`.
- `row_to_event` mirrors the column order in the SELECT lists.
- `record_event(conn, issue_id, field, old, new)` is the writer; every
  command handler that mutates an audited field calls it.
- `get_events_for_issue` returns events sorted by `created_at`.

### Step 6: ExportData round-trip

This is the step you can't skip when adding a new issue-scoped table.
`ExportData` was widened:

```rust
pub struct ExportData {
    pub issue: Issue,
    pub notes: Vec<Note>,
    pub blocked_by: Vec<i64>,
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub relations: Vec<Relation>,
}
```

Key choices:

- `#[serde(default)]` on `events` and `relations` makes older JSON exports
  (which predate these tables) still parse — they import as empty vectors.
- `src/commands/export.rs` now calls `db::get_events_for_issue` for each
  issue and stuffs the result into the `events` field.
- `src/commands/import.rs` reads `events` back out when restoring a row.

If the new table is **not** issue-scoped (e.g. a new top-level config-like
table), `ExportData` is the wrong place — design a separate top-level
export entry, or skip export support and document it as such.

### Step 7: surrounding surface

- A new `events` command + handler in `src/commands/events.rs` for listing
  audit rows.
- Every mutating command in `src/commands/` was audited for whether it
  needs a `record_event` call; the bulk-edit and close paths got special
  attention.
- The UI exposes events on the issue detail panel.
- Doctor commands gained no events-specific repair; events are append-only.

### Step 8: integration coverage

`tests/integration.sh` exercises the events table from two angles:

- The `INIT_SCHEMA_CHECK` block at the top asserts the `events` table, the
  `idx_events_issue` and `idx_events_created` indexes, and the FK clause
  (`REFERENCES issues(id) ON DELETE CASCADE`) all exist on a fresh `itr
  init`.
- Behavioral tests further down assert that mutating commands write event
  rows with the expected `field`, `old_value`, `new_value`, and `agent`
  values.

To prove idempotency directly: run `$ITR init`, then `$ITR list` (which
calls `open_db` and triggers the migrations a second time), and confirm
SQLite did not error and the schema is unchanged. The existing reopen test
pattern after `init` already covers this — search for the second `OUT=$($ITR
init)` invocation near the top of `tests/integration.sh`.

## Checklist

Before you call a schema change done, walk this list:

- [ ] New `migrate_*` function probes before mutating and is safe to run on
      any prior state.
- [ ] Wired into `migrate_current_schema` after the previous migration.
- [ ] Base `SCHEMA` const updated so `init_db` produces the same shape.
- [ ] `models.rs` carries the new field/struct with appropriate `serde`
      attributes for backward-compatible JSON.
- [ ] `row_to_issue` / new `row_to_*` reads the column at the right index.
- [ ] SELECT lists in `db.rs` include the new column everywhere `Issue` is
      loaded.
- [ ] `INSERT` / `UPDATE` helpers accept and write the field.
- [ ] `update_issue_field` allowlist includes the column name if it should
      be field-updatable.
- [ ] `ExportData` (and `export.rs` / `import.rs`) round-trips new
      issue-scoped tables; `#[serde(default)]` lets old exports still parse.
- [ ] `format.rs` renders the field in compact/json/pretty and
      `VALID_FIELDS` accepts it.
- [ ] `try_create_fts` and `fts_index_issue` include the column if it's
      searchable; existing DBs need `itr reindex` for the FTS table to be
      rebuilt with the new columns.
- [ ] `record_event` called from any handler that mutates a user-facing
      audited field.
- [ ] Integration test asserts the column/table is present on a fresh
      `itr init` and that the field reads back through `itr get -f json`.
- [ ] `tests/integration.sh` and unit tests in `src/util.rs` / `src/format.rs`
      cover any new parsing/validation behavior.
- [ ] `docs/schema.md` updated with the new column/table and any new
      indexes, constraints, or behaviors.
- [ ] `docs/backup-import-export.md` updated if the export shape changed.
- [ ] `CHANGELOG.md` notes the schema change and any upgrade implications
      (almost always zero, because migrations are automatic on the next
      `open_db`).

If a step doesn't apply (e.g. the field isn't user-visible, isn't
searchable, isn't audited), leave it unchecked — the list is a prompt, not
a gate.

## Related

- [docs/schema.md](schema.md) — the live shape of the database and the
  per-table behavior reference.
- [docs/backup-import-export.md](backup-import-export.md) — the export
  format that `ExportData` defines.
- [docs/testing.md](testing.md) — the conventions the integration suite
  follows.
- [src/db.rs](../src/db.rs) — the source of truth for schema and migrations.
