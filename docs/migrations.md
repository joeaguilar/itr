# Migrations: history, mechanism, and process

This is the contributor guide for changing the SQLite schema behind `itr`. It
covers three things:

- what has shipped so far (the ledger);
- how an existing `.itr.db` is brought forward today (the mechanism);
- the step-by-step process for each kind of change, taken from how past
  migrations were actually done.

The per-table reference for the *current* shape lives in
[`docs/schema.md`](schema.md). All schema and migration code lives in
[`src/db.rs`](../src/db.rs).

## Migration ledger

Every schema change that has shipped, oldest first. "First release" is the
earliest tag by creation date that contains the commit. The `v0.1.0` tag is out
of sequence: it was created on 2026-03-07, after v1.0.0 through v2.1.1, so
ignore it when reading release order.

| # | Date | Commit | First release | Change | How existing databases were brought forward | Follow-ups |
|---|---|---|---|---|---|---|
| 0 | 2026-02-14 | `8704df7` | v1.0.0 | **Baseline.** Tables `issues`, `dependencies`, `notes` and `config`. Seven indexes (`idx_issues_status/priority/kind/parent`, `idx_dependencies_blocked/blocker`, `idx_notes_issue`). Trigger `trg_issues_updated_at`. CHECK vocabularies for status, priority and kind. | Nothing needed. `init_db` was `execute_batch(SCHEMA)`. | The CHECK vocabularies have never changed since. The pragmas moved out of SCHEMA into `SCHEMA_PRAGMAS` in `aa4dd4c`. |
| 1 | 2026-02-14 | `88a29b2` | v1.0.0 | DB file renamed from `.nit.db` to `.itr.db`, and `NIT_DB_PATH` to `ITR_DB_PATH`. Not a DDL change. | Nothing needed: it landed the same day as init, before any tag. | — |
| 2 | 2026-03-02 | `20309ec` | v1.2.0 | `issues.skills TEXT NOT NULL DEFAULT '[]'`, in SCHEMA and as `migrate_add_skills`. | `migrate_add_skills` ran on every `open_db`: a column probe, then `ALTER TABLE ADD COLUMN`. Migrated files get the column *after* `updated_at`. | The probe was rewritten as `has_issue_column` in `aa4dd4c`. |
| 3 | 2026-03-02 | `b620fe7` | v2.0.0 | `issues.assigned_to` (`migrate_add_assigned_to`); the `events` table and 2 indexes (`migrate_add_events`); the `relations` table with CHECK, UNIQUE and 2 indexes (`migrate_add_relations`); FTS5 `issues_fts` (`content='', content_rowid=id`, no triggers) created by `try_create_fts`. None of this was in SCHEMA. | Four helpers plus `try_create_fts`, run from `open_db` only. The FTS table started **empty**, so old issues stayed unsearchable until `itr reindex`. A fresh `itr init` produced the old shape until the next open (#116). | `b68d42e` (#116) and `2dfb37e` (#152/#161). |
| 4 | 2026-05-18 | `b68d42e` (#116) | v2.10.0 | SCHEMA gains `assigned_to`, `events`, `relations` and their 4 indexes. `migrate_current_schema()` groups the helpers. `init_db` runs SCHEMA, the helpers and FTS setup. The event and relation helpers re-issue their indexes with `IF NOT EXISTS`. | Same helpers. SCHEMA itself still only ran on `init`, which is the origin of #241. | #241, fixed in row 11. |
| 5 | 2026-06-09 | `2dfb37e` (#152, #161) | v2.10.1 | **FTS redesign.** `FTS_CREATE` is `fts5(..., content='', contentless_delete=1)`, three sync triggers (`issues_fts_ai/ad/au`), `fts_is_legacy()` detection, delete-then-insert reindexing, and `busy_timeout=5000`. | Automatic on `open_db`: detect the legacy table, drop it, recreate it, repopulate it from `issues`. | #204 (docs). #264: a pre-v2.10.1 binary's `reindex` recreated the legacy table under the new triggers, and every later UPDATE failed. |
| 6 | 2026-06-09 | `2dfb37e` (#149) | v2.10.1 | Data fix in the repo's own `.itr.db` only: 18 children re-pointed to real `parent_id` links. | By hand with `itr update --parent`. **No code data migration has ever shipped.** | — |
| 7 | 2026-09-05 | `1e607a3` (#264) | v3.3.0 | **Schema generation.** `SCHEMA_VERSION = 1` is stored in `PRAGMA user_version`, and the writer release in `config.last_writer_version`. `NEWER_SCHEMA` refuses files with a higher generation. | `stamp_schema_version` stamps after migrating. Files from older releases read as generation 0 and are raised to 1. | `efb6023`, `aa4dd4c` |
| 8 | 2026-09-05 | `efb6023` | v3.3.0 | Writer stamp normalised to the release core (`vX.Y.Z`), so dev builds do not dirty a git-tracked DB. | n/a | — |
| 9 | 2026-09-05 | `aa4dd4c` (#264 hardening) | v3.3.0 | `open_schema_db` adds a lock-free zero-write fast path; a `BEGIN IMMEDIATE` slow path that re-checks the generation, migrates, repairs FTS and stamps in one transaction; `ReadOnlyNeedsMigration`; the `SCHEMA_FINGERPRINT` test. | Same helpers, now transactional. | — |
| 10 | 2026-09-26 | `9fa0f19` | v3.3.1 | `UPDATED_AT_TRIGGER` const plus `suspend_/restore_updated_at_trigger`, so import can write `updated_at` verbatim. No shape change. | n/a | A second copy of the trigger DDL; `updated_at_trigger_const_matches_schema` keeps the two in sync. |
| 11 | 2026-09-26 | (#241) | unreleased | **SCHEMA reconcile.** The migration transaction re-runs the idempotent SCHEMA after the column migrations. The expected object set is derived from SCHEMA, and a missing FTS sync trigger triggers an FTS rebuild. `doctor` gains schema checks. `ReadOnlyNeedsMigration` gets its own code, `READONLY_NEEDS_MIGRATION`. | Automatic on the next writable open whenever any expected table, column, index or trigger is missing. `SCHEMA_VERSION` stays 1, because the fresh DDL did not change. | — |

Things that have **never** happened:

- a table rebuild;
- a CHECK-vocabulary change (new status, priority, kind or relation values have
  always come in as synonyms in `normalize.rs`);
- `DROP COLUMN` or a rename;
- a data backfill in code.

The playbooks below for those cases are written from SQLite's rules and the
constraints of the current mechanism, not from precedent.

**Two physical shapes.** Migrated files carry `skills` and `assigned_to` at the
end of `issues`, while fresh files have them in declaration order. All itr SQL
uses explicit column lists, so this is harmless. Never write `SELECT *` or
positional `INSERT INTO issues VALUES (...)` against `issues`.

## The current mechanism

### Constants in `src/db.rs`

| Constant | Role |
|---|---|
| `SCHEMA` | The full current shape: six tables, 11 indexes, `trg_issues_updated_at`, all `IF NOT EXISTS`. It is executed on `init` and, since #241, on **every** slow-path open. |
| `SCHEMA_PRAGMAS` | `journal_mode=WAL; foreign_keys=ON`. It is only printed by `itr schema`; `open_db` sets the pragmas itself. |
| `FTS_CREATE`, `FTS_TRIGGERS`, `FTS_DROP` | The optional FTS5 index and its three sync triggers. |
| `UPDATED_AT_TRIGGER` | Byte-for-byte copy of the trigger in SCHEMA, used by import. A unit test keeps the two equal. |
| `SCHEMA_VERSION` | The generation this binary understands, stored in `PRAGMA user_version`. Currently `1`. |
| `WRITER_VERSION_KEY` | `last_writer_version`, a reserved config row naming the release that last opened the file. `config set` rejects it and `config reset` keeps it. |

### Open flow

`open_db` and `init_db` both call `open_schema_db`:

```rust
fn open_schema_db(path: &Path, initialize: bool) -> Result<Connection, ItrError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;")?;
    check_schema_version(&conn)?;
    if !initialize {
        let missing = missing_schema_objects(&conn)?;
        if missing.is_empty() && !schema_needs_stamp(&conn)? {
            // Deliberately skip journal_mode=WAL: this path must perform zero
            // writes, and any database itr created is already WAL.
            return Ok(conn);
        }
        if conn.is_readonly(rusqlite::DatabaseName::Main)? {
            if missing.iter().any(MissingSchemaObject::is_structural) {
                return Err(ItrError::ReadOnlyNeedsMigration);
            }
            return Ok(conn);
        }
    }
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    migrate_and_stamp_schema(&conn, initialize)?;
    Ok(conn)
}
```

1. **Generation guard.** `check_schema_version` reads `PRAGMA user_version`. If
   it is above `SCHEMA_VERSION`, the open fails with `NEWER_SCHEMA`, naming the
   recorded writer. This happens before WAL is set and before any write, so a
   refused file keeps its bytes and its journal mode.
2. **Fast path (zero writes, no lock).** The open returns straight away when
   both of these hold:
   - `missing_schema_objects` is empty: every expected table, column, index and
     trigger exists, and the FTS index exists, is not legacy, and has all three
     sync triggers;
   - the stamps are current: `user_version == SCHEMA_VERSION` and
     `last_writer_version == writer_stamp()`.

   `up_to_date_open_performs_no_write` checks this byte for byte.
3. **Read-only handle.** Only a missing **table or column** is fatal
   (`READONLY_NEEDS_MIGRATION`), because reads would hit "no such column". A
   missing index, trigger or FTS repair, or a stale stamp, is left for the next
   writable open, and reads carry on. `itr doctor` reports these gaps
   meanwhile.
4. **Slow path.** It sets `journal_mode=WAL`, then runs everything in one
   `BEGIN IMMEDIATE` transaction:

```rust
fn migrate_and_stamp_schema(conn: &Connection, initialize: bool) -> Result<(), ItrError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    check_schema_version(&tx)?;          // re-check under the write lock
    if initialize {
        tx.execute_batch(SCHEMA)?;       // a new file has no tables to probe
    }
    migrate_current_schema(&tx)?;        // column/table helpers, in landing order
    tx.execute_batch(SCHEMA)?;           // reconcile: every index/trigger in SCHEMA
    try_create_fts(&tx);                 // create/repair FTS; errors swallowed
    stamp_schema_version(&tx)?;          // raise user_version, record writer
    tx.commit()?;
    Ok(())
}
```

- **The generation is re-checked under the lock.** Another process may have
  upgraded the file between step 1 and `BEGIN IMMEDIATE`
  (`schema_generation_is_rechecked_under_write_lock`).
- **The reconcile must run after the column migrations.** SCHEMA is
  idempotent, but an index on a migrated column fails on an old file with "no
  such column" if SCHEMA runs first. That was reproduced with a hypothetical
  `CREATE INDEX ... ON issues(assigned_to)`.
- **`stamp_schema_version` only ever raises `user_version`.** It writes the
  writer row only when it differs, and it swallows `SQLITE_READONLY` because
  the stamp is advisory.
- **The first writable open by each new release always takes the slow path,**
  because the writer stamp changes. That is intended: it guarantees every
  release re-runs the reconcile at least once.

`itr doctor --fix` reaches the same transaction through
`db::reconcile_schema(conn)`.

### The expected object set

`missing_schema_objects` compares the file with `expected_schema()`. That set is
derived once per process by executing `SCHEMA`, `FTS_CREATE` and `FTS_TRIGGERS`
against a private in-memory database and reading back `sqlite_master` and
`pragma_table_info`. There is no hand-kept list, so declaring an object in
SCHEMA is enough for the fast path, the read-only check and `doctor` to know
about it. Each missing object is reported with one of these kinds:

| Kind | Meaning | Structural (fatal read-only)? |
|---|---|---|
| `table` | A SCHEMA table is absent. | yes |
| `column` | A SCHEMA column is absent (`issues.skills`). | yes |
| `index` / `trigger` | A SCHEMA index or trigger is absent, or an FTS sync trigger is absent. | no |
| `fts_index` | This build has FTS5 but `issues_fts` is absent. | no |
| `legacy_fts_index` | `issues_fts` has the pre-v2.10.1 shape. | no |

What the set does **not** see:

- a changed CHECK constraint, column type, default or trigger body on an object
  whose name already exists;
- the FTS column list.

Changes like those need their own detector (see "FTS shape change" and
"CHECK-vocabulary change" below).

### FTS setup and repair

```rust
fn try_create_fts(conn: &Connection) {
    if fts_is_legacy(conn) || fts_triggers_missing(conn) {
        let _ = conn.execute_batch(FTS_DROP);
    }
    let existed = has_fts(conn);
    if conn.execute_batch(FTS_CREATE).is_err() || !has_fts(conn) {
        return; // FTS5 unavailable; search uses the LIKE fallback.
    }
    let _ = conn.execute_batch(FTS_TRIGGERS);
    if !existed {
        // populate from all_issues()
    }
}
```

- `fts_is_legacy` is a substring check for `contentless_delete` in the stored
  `CREATE VIRTUAL TABLE` SQL. It is the only FTS shape detector.
- If any sync trigger is missing, rows written in the meantime may be stale, so
  the index is dropped and rebuilt rather than just getting its triggers back
  (`writable_open_restores_fts_triggers_and_reindexes`).
- FTS is optional. If `FTS_CREATE` fails, search falls back to LIKE, and the
  expected set does not require FTS objects.
- `itr reindex` and the `fts_stale` fix in `doctor --fix` call `fts_rebuild`,
  which does the same drop, recreate and repopulate.

### What older binaries do

- **v3.3.0 and later** refuse any file with a higher generation (`NEWER_SCHEMA`).
  A long-running `itr ui` checks only at startup.
- **Before v3.3.0** binaries cannot see the stamp. In #264, v2.10.0's `reindex`
  recreated the legacy FTS table under the v2.10.1 triggers, and every later
  `update` failed with `cannot DELETE from contentless fts5 table`. The next
  open by a current binary self-heals: the legacy detector rebuilds the index.
- **There is no downgrade path.** Once a newer binary bumps the generation,
  older v3.3+ binaries refuse the file. Treat every bump as a one-way door.

### Read-only databases

A read-only copy opens fine unless it is missing a table or column. An
un-migrated copy fails with `READONLY_NEEDS_MIGRATION` ("database is read-only
and needs migration; reopen it writable"). Make it writable once, or open a
writable copy, and the first open migrates it. See
[troubleshooting](troubleshooting.md#error-code-reference).

## Process by change type

Every change type ends with the [PR checklist](#pr-checklist). The
[`SCHEMA_VERSION` rules](#when-to-bump-schema_version) apply throughout.

### Add a column

Precedent: `skills` (row 2) and `assigned_to` (row 3).

1. **Declare it in SCHEMA**, in the `CREATE TABLE`. A new `NOT NULL` column
   needs a `DEFAULT`, because `ALTER TABLE ADD COLUMN` cannot add a `NOT NULL`
   column without one. Use `DEFAULT ''` for text and `DEFAULT '[]'` for
   JSON-array columns.
2. **Write the helper and append it to `migrate_current_schema`:**

   ```rust
   fn migrate_add_skills(conn: &Connection) -> Result<(), ItrError> {
       if !has_issue_column(conn, "skills")? {
           conn.execute_batch("ALTER TABLE issues ADD COLUMN skills TEXT NOT NULL DEFAULT '[]';")?;
       }
       Ok(())
   }
   ```

   The helper must be a no-op when the column already exists, which is always
   the case on `init`. `ALTER TABLE ADD COLUMN` cannot add `PRIMARY KEY`,
   `UNIQUE`, or a non-constant default such as `strftime(...)`. Needing any of
   those means a table rebuild.
3. **Detection is automatic.** The derived expected set sees the new SCHEMA
   column, so the fast path and read-only check pick it up. You no longer edit
   a `schema_needs_migration` list.
4. **Add an entry to `STRUCTURAL_MIGRATION_GAPS`** in the `db.rs` tests (for
   example `"ALTER TABLE issues DROP COLUMN <col>;"`). Both the writable-open
   and read-only-refusal tests then cover it.
5. **Bump `SCHEMA_VERSION` and update `SCHEMA_FINGERPRINT`.** The fingerprint
   test fails until you do.
6. **Thread the column through the code:**
   - `models::Issue`, with `#[serde(default)]`;
   - `row_to_issue` and every SELECT list;
   - `insert_issue`;
   - the `update_issue_field` allowlist;
   - `format.rs` (all four list renderers plus `VALID_FIELDS`);
   - the UI;
   - `record_event` calls if the field is audited mutable state.
7. **Update import.** `src/commands/import.rs` `insert_issue_row` and
   `replace_issue_row` use explicit column lists, so a new column is silently
   reset to its default on import unless you add it to both. Export needs no
   change, because it serialises `Issue`. See
   [Export/import compatibility](#exportimport-compatibility).
8. **If it is searchable,** this is also an FTS shape change (below).

### Add a table

Precedent: `events` and `relations` (rows 3 and 4).

1. Declare the table and its indexes in SCHEMA (`IF NOT EXISTS`). Declare
   foreign keys with `ON DELETE` deliberately; every issue-scoped table so far
   uses `ON DELETE CASCADE`.
2. Write a `migrate_add_<table>` helper guarded by `has_schema_table`, and
   append it to `migrate_current_schema`. Strictly speaking, the SCHEMA
   reconcile would now create a missing table on its own. Keep the helper
   anyway:
   - it documents the migration;
   - it is the place to seed or backfill rows on first creation;
   - it keeps the ledger explicit.
3. Add a `DROP TABLE <table>;` gap to `STRUCTURAL_MIGRATION_GAPS`. Bump
   `SCHEMA_VERSION` and update the fingerprint.
4. Add DB helpers, a model struct, and a `row_to_*` function; commands should
   not issue ad-hoc SQL.
5. If the rows are issue-scoped, add them to `ExportData` with
   `#[serde(default)]`, read them in `export.rs`, and write them in
   `import.rs`. `replace_issue_row` must also clear them for a replaced issue.
   If the table is not issue-scoped, `ExportData` is the wrong place; document
   that it is not exported.

### Add an index or a trigger

Precedent: the seven base indexes and `trg_issues_updated_at` (row 0) reached
existing files only through `init` until #241.

1. Declare it in SCHEMA with `IF NOT EXISTS`. That is the whole migration:
   - the derived expected set notices it is missing on existing files;
   - the reconcile in `migrate_and_stamp_schema` creates it on the next
     writable open;
   - `doctor` reports it until then.

   Do not add a separate helper.
2. The index or trigger may reference a migrated column. That works, because
   the reconcile runs after `migrate_current_schema`.
3. A **changed** trigger body or index definition under an existing name is
   *not* detected, because `IF NOT EXISTS` keeps the old one. Give the new
   version a new name and `DROP ... IF EXISTS` the old name in a helper, or
   write an explicit SQL-text detector like `fts_is_legacy`. If the trigger
   is also written somewhere else, as `trg_issues_updated_at` is in
   `UPDATED_AT_TRIGGER`, update that copy too; the unit test enforces it.
4. The fingerprint test fails. See the rules below. A new plain index is the
   one change that may keep the generation.

### FTS shape change

Precedent: the v2.10.1 `contentless_delete` redesign (row 5) and the #264
incident.

1. Change `FTS_CREATE`, `FTS_TRIGGERS` (they insert every indexed column), and
   the `INSERT` in `fts_index_issue` together.
2. **Extend the legacy detector.** `CREATE VIRTUAL TABLE IF NOT EXISTS` keeps
   the old column set on existing files. `fts_is_legacy` must recognise the
   previous shape from its stored SQL, for example with a marker only the new
   DDL contains. The existing path then drops, recreates and repopulates it
   automatically inside the migration transaction. Never tell users to run
   `itr reindex` for this; the #264 design comment explicitly wants it
   automatic.
3. **Bump `SCHEMA_VERSION`.** An older binary would recreate its own FTS shape
   under your triggers (#264).
4. Add a unit test in the style of `fts_legacy_contentless_table_is_migrated`,
   starting from the previous shape. Consider adding a fixture of the previous
   shape (see "Testing requirements").

### CHECK-vocabulary change or table rebuild (never done yet)

This covers new status, priority, kind or `relation_type` values, a type
change, dropping or renaming a column, or adding a constraint.

**First ask whether a synonym is enough.** Until now, vocabulary growth has
gone through `normalize.rs` synonyms that map onto the fixed CHECK sets.

If the stored vocabulary really has to change, SQLite has no
`ALTER ... CHECK`. Use the
[12-step rebuild](https://www.sqlite.org/lang_altertable.html#otheralter):

1. `PRAGMA foreign_keys=OFF` (**outside** any transaction).
2. `BEGIN IMMEDIATE`.
3. Save the table's index and trigger SQL:
   `SELECT sql FROM sqlite_master WHERE tbl_name='issues' AND type IN ('index','trigger')`.
4. `CREATE TABLE new_issues (...)` in the new shape.
5. `INSERT INTO new_issues (<explicit columns>) SELECT <explicit columns> FROM issues`,
   mapping values where the vocabulary changed.
6. `DROP TABLE issues`.
7. `ALTER TABLE new_issues RENAME TO issues`.
8. Recreate the indexes and triggers. In itr, the SCHEMA reconcile and
   `try_create_fts` do this: dropping `issues` drops `trg_issues_updated_at`
   and the three FTS triggers, and the FTS index is then rebuilt because its
   triggers were missing.
9. Recreate any views (itr has none).
10. `PRAGMA foreign_key_check`. If it returns any row, roll back.
11. `COMMIT`.
12. `PRAGMA foreign_keys=ON`.

Constraints specific to itr:

- **The rebuild cannot run inside today's migration transaction as-is.**
  `open_schema_db` sets `foreign_keys=ON` before `BEGIN IMMEDIATE`, and
  `PRAGMA foreign_keys` is a silent no-op inside a transaction. With foreign
  keys on, `DROP TABLE issues` performs an implicit `DELETE FROM issues`, and
  `ON DELETE CASCADE` wipes `notes`, `dependencies`, `events` and `relations`.
  `defer_foreign_keys` does not help, because cascade actions are not
  deferred. The migration path needs a pre-transaction step:
  - detect "needs rebuild";
  - set `foreign_keys=OFF`;
  - run the transaction, re-checking under the lock that the rebuild is still
    needed;
  - run `foreign_key_check` before commit;
  - restore `foreign_keys=ON` in every exit path, including errors.
- **Detection.** The derived expected set only compares names and columns, so
  write an explicit probe on the stored `CREATE TABLE` SQL, in the style of
  `fts_is_legacy`.
- **Keep `sqlite_sequence`.** `issues` uses `AUTOINCREMENT`. Copy the old
  `sqlite_sequence.seq` for the table onto the rebuilt one, so IDs of deleted
  issues are never reused.
- **Always bump `SCHEMA_VERSION`.** An older binary would reject or mishandle
  the new values.
- **Add a fixture of the pre-rebuild shape,** and test that a populated file
  keeps every note, dependency, event and relation.

### Data backfill (never done yet)

1. Put the backfill in `migrate_current_schema`, after the structural helpers,
   so it runs inside the same write-locked transaction as the stamp.
2. Make it idempotent by predicate (`UPDATE ... WHERE <not yet backfilled>`),
   or gate it on the generation with
   `if read_user_version(conn)? < N { ... }`. The stamp raises `user_version`
   to N in the same transaction, so a generation-gated backfill runs exactly
   once.
3. **Bump `SCHEMA_VERSION`.** A backfill changes no DDL, so the fingerprint
   test will not remind you. The bump is also what sends existing files down
   the slow path: `schema_needs_stamp` sees `user_version != SCHEMA_VERSION`.
4. If the backfill changes user-visible field values, consider recording
   `events` rows with a recognisable agent (for example `itr-migration`), so
   `itr log` explains the change.
5. Backfills touch `issues`, which fires `trg_issues_updated_at`. Decide
   whether `updated_at` should move; if not, suspend the trigger in the same
   transaction the way import does.

## When to bump `SCHEMA_VERSION`

Bump it for **any change an older binary at the same generation would
mishandle**:

- a new column or table (older binaries would not write it, and their `init`
  would create the old shape);
- an FTS shape change (#264);
- a CHECK or vocabulary change, or a table rebuild;
- a data backfill;
- a new or changed trigger (it changes write semantics for every binary that
  touches the file).

The `fresh_schema_matches_generation_fingerprint` unit test pairs
`SCHEMA_VERSION` with an FNV-1a hash of the fresh `sqlite_master` DDL, so any
DDL change fails until you update `SCHEMA_FINGERPRINT`. That forces a
deliberate decision.

- **The only accepted non-bump is a new plain index.** It is invisible to
  older binaries. Since #241 it reaches existing files through the reconcile,
  and those binaries' own opens tolerate it. Say so in the PR if you update the
  fingerprint without bumping.
- **Backfills and changed trigger bodies under an existing name do not change
  the fresh DDL,** so the test cannot catch them. Bump by hand.

Consequences of a bump:

- Files are stamped on their next writable open. From then on, every v3.3+
  binary with a lower generation refuses them with `NEWER_SCHEMA`.
- Pre-v3.3.0 binaries remain blind to the stamp.
- Write an upgrade note in `CHANGELOG.md` that names the new generation.
- Update the `SCHEMA_VERSION` doc comment to say what the new generation
  contains.

## Export/import compatibility

- **Export has no version or generation field.** `itr export` writes a JSON
  array (or JSONL) of `ExportData`. Compatibility rests entirely on serde
  defaults.
- **New fields must default.** `Issue.skills`, `Issue.assigned_to`,
  `ExportData.events` and `ExportData.relations` carry `#[serde(default)]`, so
  exports from before those fields import cleanly. Every new field needs the
  same attribute.
- **Unknown fields are ignored.** No struct uses `deny_unknown_fields`, so a
  newer export imported by an older binary silently drops the fields that
  binary does not know. There is no downgrade guarantee.
- **Import writes explicit column lists.** `insert_issue_row` and
  `replace_issue_row` in `src/commands/import.rs` name every column. A new
  `issues` column must be added to both, or import resets it to its default.
  New issue-scoped tables need a write path in import, and a clear path in
  `replace_issue_row`.
- **Import writes `updated_at` verbatim.** It suspends `trg_issues_updated_at`
  inside its transaction (`suspend_/restore_updated_at_trigger`). Changing that
  trigger means changing `UPDATED_AT_TRIGGER` too.
- **Import indexes FTS explicitly** with `fts_index_issue`. A new searchable
  column must be included there.
- **A file copy is not an export.** Copying `.itr.db` carries its generation. A
  copy from a newer binary is refused by an older one, and an old copy is
  migrated (one-way) by the first writable open. Prefer `itr export` for
  anything that may be restored by a different itr release.

## Testing requirements

Every schema change needs coverage for each of these; the existing tests to
extend are named.

1. **Fresh init.**
   - `INIT_SCHEMA_CHECK` in `tests/integration.sh` asserts the tables,
     columns and indexes that a new `itr init` must have.
   - `fresh_schema_matches_generation_fingerprint` pins the DDL.
   - `expected_schema_is_derived_from_schema_and_fts_ddl` pins the derived
     set.
2. **Upgrade from old releases.**
   - `tests/fixtures/schema-v1.0-oldest.sql` is the verbatim v1.0 DDL plus
     sample rows. `tests/fixtures/schema-v2.0-legacy-fts.sql` is the v2.0 to
     v2.10.0 shape, including a stale legacy FTS index; apply it on top of the
     first.
   - They are loaded by `old_release_fixtures_upgrade_to_the_complete_schema`
     (unit) and by the "schema migrations: old-release fixtures" section of
     `tests/integration.sh`. Those checks cover:
     - data intact;
     - the generation stamped;
     - the object set equal to a fresh init;
     - FTS backfilled;
     - `doctor` clean;
     - `add` and `update` working;
     - a second open changing nothing.
   - **Fixtures are historical: never edit their DDL.** When a release leaves
     a *new* on-disk shape that a later migration must handle (a rebuild, an
     FTS change), add a new fixture for it built from that release's DDL, and
     add it to both loops.
   - A new SCHEMA column or table that lacks its `migrate_*` helper fails
     these tests.
3. **Idempotency and zero writes.**
   - `up_to_date_open_performs_no_write` and
     `up_to_date_open_succeeds_under_write_lock` cover the fast path.
   - `writable_open_restores_schema_only_objects` checks that a repaired file
     then opens with zero writes.
   - The integration "second open changes nothing" check compares the schema
     fingerprint and the file's md5.
   - Running `itr init` twice proves nothing: the second run is a fast-path
     open.
4. **Structural gaps.**
   - Add to `STRUCTURAL_MIGRATION_GAPS`.
   - `writable_open_applies_each_structural_migration` and
     `readonly_db_needing_structural_migration_is_refused` then cover the new
     step on both writable and read-only handles.
5. **Read-only opens.**
   - `readonly_db_missing_index_or_trigger_opens`
   - `readonly_db_with_fts_only_differences_opens`
   - `readonly_db_with_different_writer_opens`
   - the integration read-only block (chmod 444; skipped when running as root)
6. **Newer-generation refusal.**
   - `newer_schema_is_refused_before_migrating`
   - `schema_generation_is_rechecked_under_write_lock`
7. **FTS.** `writable_open_repairs_fts_with_current_stamp`,
   `writable_open_restores_fts_triggers_and_reindexes`, and
   `fts_legacy_contentless_table_is_migrated`.

Run the full gate: `gatr run --tag ci -- just ci`.

## PR checklist

- [ ] SCHEMA declares the change (`IF NOT EXISTS`; `NOT NULL` columns have a
      constant `DEFAULT`).
- [ ] A new column or table has a `migrate_*` helper, appended to
      `migrate_current_schema`, that is a no-op on a fresh file.
- [ ] Indexes and triggers are declared in SCHEMA only (the reconcile applies
      them), and any changed body gets a new name or an explicit detector.
- [ ] An FTS change updates `FTS_CREATE`, `FTS_TRIGGERS`, `fts_index_issue`
      *and* the legacy detector.
- [ ] A rebuild or CHECK change handles `foreign_keys=OFF` outside the
      transaction, runs `foreign_key_check`, and preserves `sqlite_sequence`.
- [ ] `SCHEMA_VERSION` bumped (or a plain-index-only exception stated),
      `SCHEMA_FINGERPRINT` updated, and the `SCHEMA_VERSION` doc comment
      updated.
- [ ] `STRUCTURAL_MIGRATION_GAPS` extended for new columns and tables.
- [ ] Old-release fixture tests pass; a new fixture was added if this release
      leaves a new on-disk shape.
- [ ] Models (`#[serde(default)]`), `row_to_*`, SELECT lists, insert/update
      helpers, `update_issue_field` allowlist, and `format.rs` (all four list
      renderers, `VALID_FIELDS`) updated.
- [ ] `import.rs` `insert_issue_row`/`replace_issue_row` (and
      `ExportData`/`export.rs` for new tables) updated.
- [ ] `record_event` coverage for new audited mutable state.
- [ ] Docs: this ledger (a new row), `docs/schema.md`, and
      `docs/backup-import-export.md` if the export shape changed.
- [ ] `CHANGELOG.md` upgrade note: the new generation, and that older v3.3+
      binaries will refuse migrated files.
- [ ] `gatr run --tag ci -- just ci` is green.

## Related

- [docs/schema.md](schema.md): the per-table reference for the current shape.
- [docs/backup-import-export.md](backup-import-export.md): the export format
  that `ExportData` defines.
- [docs/troubleshooting.md](troubleshooting.md#error-code-reference):
  `NEWER_SCHEMA` and `READONLY_NEEDS_MIGRATION`.
- [docs/testing.md](testing.md): integration-suite conventions.
- [src/db.rs](../src/db.rs): the source of truth.
