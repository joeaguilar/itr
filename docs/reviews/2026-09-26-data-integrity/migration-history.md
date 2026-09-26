# itr migrations: history, current mechanism, doc drift, gaps

Read-only git archaeology of `/Users/josefaguilar/AI_Projects/itr` at HEAD `d017daa` (v3.3.1). Experiments used the
installed `itr v3.3.0-2-g2e96f5d` (its db.rs is identical to HEAD; `d017daa` only bumps the manifest), plus an `itr`
built from the `v2.10.0` source (`git archive v2.10.0`, built in scratch). All experiment DBs live in
`scratchpad/repro-migrations/`. The repo's `.itr.db` was only read, through a `mode=ro` `.backup` copy
(`repro-migrations/repo-copy.db`).

Artifacts next to this report:
- `oldest-schema-8704df7.sql`: the SCHEMA const from the first commit, verbatim. It is byte-identical through v1.1.0,
  and it is exactly what `itr init` ran in v1.0.0 and v1.1.0, because `init_db` was just `execute_batch(SCHEMA)`.
- `v2.0-v2.10.0-legacy-shape.sql`: the DDL that the v2.0.0 through v2.10.0 `open_db` applied on top of the oldest
  schema, including the legacy FTS table. The migration DDL is copied verbatim from `v2.0.0:src/db.rs`.
- `repro-migrations/ddl.py`: a Python copy of the `SCHEMA_FINGERPRINT` algorithm (FNV-1a over the sorted,
  whitespace-normalised `sqlite_master` DDL, with FTS shadow tables and `sqlite_*` excluded).

A note on release attribution: the `v0.1.0` tag is out of sequence. It was created on 2026-03-07 and points at
`f080ef0`, after v1.0.0 through v2.1.1 already existed. "First release" below therefore means the earliest tag *by
creation date* that contains the commit (`git tag --contains <sha> --sort=creatordate | head -1`).

---

## (a) Chronological migration ledger

| # | Date | Commit | First release | Change | How existing DBs were brought forward | Follow-up fixes |
|---|---|---|---|---|---|---|
| 0 | 2026-02-14 | `8704df7` feat: init | v1.0.0 | **Baseline.** Tables `issues`, `dependencies`, `notes`, `config`. Seven indexes (`idx_issues_status/priority/kind/parent`, `idx_dependencies_blocked/blocker`, `idx_notes_issue`). Trigger `trg_issues_updated_at`. CHECK vocabularies for status, priority and kind. `PRAGMA journal_mode=WAL; foreign_keys=ON` were embedded in SCHEMA. | None needed. `init_db` = `execute_batch(SCHEMA)`; `open_db` only set the two pragmas. | The pragmas were split out of SCHEMA into `SCHEMA_PRAGMAS` in `aa4dd4c`. The CHECK vocabularies have **never** changed (pickaxe on `CHECK (status IN` / `kind IN` / `priority IN` only hits `8704df7`). |
| 1 | 2026-02-14 | `88a29b2` refactor: rename to itr | v1.0.0 | DB file `.nit.db` became `.itr.db`, and `NIT_DB_PATH` became `ITR_DB_PATH`. This is not a DDL change. | None. It happened the same day as init and before any tag, so no shipped DB used `.nit.db`. | — |
| 2 | 2026-03-02 | `20309ec` feat: skills | v1.2.0 | `issues.skills TEXT NOT NULL DEFAULT '[]'`. Added to SCHEMA between `tags` and `acceptance`, **and** added as `migrate_add_skills` (a `PRAGMA table_info` probe, then `ALTER TABLE … ADD COLUMN`), called from `open_db`. | **Named helper `migrate_add_skills`** on every `open_db`. ALTER appends the column, so migrated DBs have `skills` *after* `updated_at`, while fresh DBs have it after `tags` (see §e). | The probe was rewritten to use `has_issue_column` in `aa4dd4c`. CHANGELOG v1.2.0 misdescribes this release as "Claude Code skill support". `itr skill` actually arrived in `671bed9` on 2026-05-16. |
| 3 | 2026-03-02 | `b620fe7` feat: multi-agent support | v2.0.0 | (a) `issues.assigned_to TEXT NOT NULL DEFAULT ''` via `migrate_add_assigned_to`. **Not added to SCHEMA.** (b) `events` table plus `idx_events_issue` and `idx_events_created` via `migrate_add_events` (not in SCHEMA). (c) `relations` table with the `CHECK(relation_type IN ('duplicate','related','supersedes'))` constraint, `UNIQUE(source_id,target_id,relation_type)` and two indexes, via `migrate_add_relations` (not in SCHEMA). (d) FTS5 `issues_fts` created as `content='', content_rowid=id` by `try_create_fts`, with **no triggers**. Rust code indexed it with `INSERT OR REPLACE`, and the result was ignored with `let _`. | Four named helpers plus `try_create_fts`, all run from `open_db` only. The index DDL ran inside the `if !has_table` branch, so a missing index was never repaired. The FTS table was **created empty and never back-filled**: existing issues stayed invisible to FTS until `itr reindex` or `doctor --fix` (doctor's count check flagged it). `init_db` still ran only SCHEMA, so a fresh `init` produced the old shape until the next command opened it (#116). | `b68d42e` (#116): SCHEMA mirror, `init_db` runs migrations, and indexes are re-issued with `IF NOT EXISTS`. `2dfb37e` (#152/#161): the FTS redesign. `aa4dd4c`: probes rewritten and the work moved into a transaction. |
| 4 | 2026-05-18 | `b68d42e` "Fix code review blitz issues" (#116) | v2.10.0 | SCHEMA gains `assigned_to` (placed after `close_reason`), `events`, `relations` and four indexes. `migrate_current_schema()` is introduced to group the four helpers. `migrate_add_events` and `migrate_add_relations` now **always** run `CREATE INDEX IF NOT EXISTS`, which makes them self-healing for their own indexes. `init_db` now runs SCHEMA, then `migrate_current_schema`, then `try_create_fts`. Integration `INIT_SCHEMA_CHECK` added. | Same helpers as before. A fresh `init` now emits the current shape. **SCHEMA itself was still never re-executed on `open_db`**, which is the origin of #241. | #241 (open). The SCHEMA-only objects (7 base indexes and the trigger) still never reach existing DBs. |
| 5 | 2026-06-09 | `2dfb37e` 58-issue blitz (#152, #161) | v2.10.1 | **FTS redesign.** `FTS_CREATE` = `fts5(… content='', contentless_delete=1)` (needs SQLite ≥ 3.43; bundled). Three sync triggers `issues_fts_ai/ad/au` (`FTS_TRIGGERS`). `FTS_DROP` also drops the triggers. `fts_is_legacy()` does a substring check for `contentless_delete` in the stored SQL. `try_create_fts` drops a legacy table, recreates it, and **populates from `all_issues` when the table is newly created**. `fts_index_issue` switches to delete-then-insert and surfaces failures as `REVIEW:`. Import indexes rows (#161). `PRAGMA busy_timeout=5000` added to `open_db`. | **Auto-migration on `open_db`**: detect legacy, drop, recreate, repopulate. There was no transaction until v3.3.0. No CHANGELOG upgrade note was written for this release (v2.10.x through v3.3.1 all sit under "Unreleased"). | #204 (docs/schema.md FTS section). #264: a pre-v2.10.1 binary's `reindex` recreates the legacy table under the new triggers, and every later UPDATE then fails. `aa4dd4c` moved the FTS repair into the `BEGIN IMMEDIATE` transaction and added the `fts_needs_work` gate. |
| 6 | 2026-06-09 | `2dfb37e` (#149) | v2.10.1 | **Tracker-data** migration in the repo's own `.itr.db`: 18 children of epic #95 were re-pointed from a note-index to real `parent_id` links. `doctor --fix` also removed six stale done-blocker edges. | Performed by hand with `itr update --parent` and committed as the `.itr.db` snapshot. **No code data migration has ever shipped.** | — |
| 7 | 2026-09-05 | `1e607a3` (#264) | v3.3.0 | **Schema generation.** `SCHEMA_VERSION = 1` is stored in `PRAGMA user_version`, and the writer release is stored in `config.last_writer_version` (a new managed config row). `NewerSchema`/`NEWER_SCHEMA` refuses files with a higher generation before any migration runs. | Stamped by `stamp_schema_version` after migrations on any open where the values differ. Files older than the guard read as generation 0 and are upgraded to 1. | `efb6023`, `aa4dd4c` |
| 8 | 2026-09-05 | `efb6023` | v3.3.0 | The writer stamp is normalised to `vX.Y.Z` (release core), so dev builds do not dirty a git-tracked DB. The error message is now a single line. | n/a | — |
| 9 | 2026-09-05 | `aa4dd4c` (#264 hardening) | v3.3.0 | `open_db` restructured into `open_schema_db`. It adds a lock-free, zero-write fast path; a slow path that takes `BEGIN IMMEDIATE`, re-checks the generation, then migrates, repairs FTS and stamps in **one transaction**; `ReadOnlyNeedsMigration`; `SCHEMA_PRAGMAS` split out; `has_issue_column`/`has_schema_table` probes; the `SCHEMA_FINGERPRINT` test; `config reset` keeps and `config set` rejects `last_writer_version`. | Same helpers, now transactional. | — |
| 10 | 2026-09-26 | `9fa0f19` (import fix) | v3.3.1 | `UPDATED_AT_TRIGGER` const (a unit test keeps it equal to the text in SCHEMA). `suspend_/restore_updated_at_trigger` DROP and CREATE the trigger **at runtime inside the import transaction** so exported `updated_at` survives verbatim. No shape change. | n/a | This adds a second copy of the trigger DDL that must be kept in sync; the test enforces it. |

Things that have **never** happened: a table-rebuild migration, a CHECK-vocabulary change, `DROP COLUMN`, a
rename, or a data backfill in code. Vocabulary growth has always gone through `normalize.rs` synonyms that map
onto the fixed CHECK sets. There is no rebuild machinery. One consequence to note for future work: the slow path
runs inside `BEGIN IMMEDIATE` with `foreign_keys=ON` already set (`db.rs:253`), and
`PRAGMA foreign_keys=OFF` is a no-op inside a transaction. The standard SQLite 12-step table rebuild therefore
cannot be dropped into `migrate_current_schema` as-is.

The repo's own `.itr.db` shows the ledger is complete. Its DDL fingerprint is `0x35990ee6b722076c`, which exactly
equals the fingerprint of an oldest-schema DB after the current `itr` upgrades it (§e). No schema object exists that
this ledger does not explain.

---

## (b) Current mechanism (HEAD `src/db.rs`)

**Constants**
- `SCHEMA_PRAGMAS` (`db.rs:7-10`): `journal_mode=WAL; foreign_keys=ON`. It is only printed by `itr schema` via
  `get_schema_sql` (`db.rs:446-450`). It is never executed by open or init.
- `SCHEMA` (`db.rs:12-94`): six tables, 11 indexes and `trg_issues_updated_at`, all `IF NOT EXISTS`. It is
  **executed only when `initialize == true`**, at `db.rs:304-306`.
- `UPDATED_AT_TRIGGER` (`db.rs:99-105`), with `suspend_/restore_updated_at_trigger` (`db.rs:110-119`). The only
  runtime caller is import (`import.rs:104,208`).
- `SCHEMA_VERSION: i32 = 1` (`db.rs:207-214`). Its doc comment says "Bump it whenever `migrate_current_schema` gains
  a step or the FTS design changes".
- `WRITER_VERSION_KEY = "last_writer_version"` (`db.rs:218`). `writer_stamp()` and `normalize_writer_stamp()`
  (`db.rs:223-243`) produce `vX.Y.Z` from `ITR_VERSION` and fall back to `CARGO_PKG_VERSION`.
- FTS: `FTS_CREATE` (`db.rs:1547-1550`), `FTS_TRIGGERS` (`1559-1573`), `FTS_DROP` (`1575-1580`), `fts_is_legacy`
  (`1585-1594`), `try_create_fts` (`1598-1615`), `fts_index_issue` (`1622-1645`), `fts_rebuild` (`1648-1662`).

**Open flow.** `open_db` (`db.rs:245-247`) and `init_db` (`db.rs:442-444`) both call `open_schema_db(path, initialize)`
(`db.rs:249-276`):
1. `Connection::open`, then `PRAGMA busy_timeout=5000; foreign_keys=ON` (`:251-253`).
2. `check_schema_version` (`:254`, body `:319-337`). If `user_version > SCHEMA_VERSION`, it returns
   `NewerSchema{db, supported, written_by (from config, or "an unknown itr release"), current}`. This happens
   **before** WAL is set and before any write, so a refused file keeps its journal mode and bytes. This was verified
   in e6: `list`, `init` and `doctor --fix` all exit 1 with `NEWER_SCHEMA` and leave the md5 unchanged.
3. **Fast path** (`:255-263`). If this is not init, `schema_needs_migration` is false (`:279-284`: `skills`,
   `assigned_to`, `events` and `relations` exist), `schema_needs_stamp` is false (`:287-290`: `user_version ==
   SCHEMA_VERSION` and the writer equals `writer_stamp()`), and `fts_needs_work` is false (`:292-296`: `issues_fts`
   exists and is not legacy), the connection is returned with **zero writes and no WAL pragma**.
4. **Read-only handle** (`:264-271`). If structural work is pending, it returns `ReadOnlyNeedsMigration`, which
   maps to error code `DB_ERROR` (`error.rs:71`). Otherwise the connection is returned as-is (stamp and FTS repair are
   skipped). This was verified in e4 (a legacy-FTS DB opened read-only works and the file is unchanged) and in e5 (an
   oldest DB opened read-only gets "database is read-only and needs migration; reopen it writable" and exit 1).
5. **Slow path.** `PRAGMA journal_mode=WAL` (`:273`), then `migrate_and_stamp_schema` (`:298-311`). That function
   takes `BEGIN IMMEDIATE`, runs `check_schema_version` again under the lock, runs SCHEMA if `initialize`, then
   `migrate_current_schema` (`:363-369`: `migrate_add_skills`, `migrate_add_assigned_to`, `migrate_add_events`,
   `migrate_add_relations`, in that order), then `try_create_fts` (errors swallowed), then `stamp_schema_version`
   (`:342-361`), then commits.
6. `stamp_schema_version` only **raises** `user_version`; it never lowers it. It writes the writer row only when that
   row differs. `SQLITE_READONLY` is swallowed because the stamp is advisory.

**Which binary writes what.** Every writable slow-path open records the release core as the writer. Because the
writer changes with every release, the **first writable open by each new release always takes the slow path**, even
when the generation is unchanged. `config reset` keeps the key (`db.rs:1267-1273`). `config set` rejects it
(`config.rs:108-114`), and `config list` labels it (`config.rs:39`). In the UI API, both `NewerSchema` and
`ReadOnlyNeedsMigration` map to HTTP 500 (`ui.rs:1531-1534`).

**What an older binary does on a newer DB:**
- Binaries at or after v3.3.0 refuse any file with a higher generation.
- Binaries before v3.3.0 cannot see the stamp. I reproduced the #264 incident with a real v2.10.0 build against a
  generation-1 DB (e7). `list` works. `reindex` recreates `issues_fts` in the legacy `content_rowid=id` shape while
  the v3 triggers survive. The next `update` fails with `Database error: cannot DELETE from contentless fts5 table:
  issues_fts`. The old binary leaves `user_version=1` and the writer row alone. The next open by the current binary
  self-heals: `fts_is_legacy` triggers a drop, recreate and repopulate, after which writes succeed.

**`init` and `schema` (#116).**
- `itr init` on a new path runs `init_db`, which executes SCHEMA, the migrations, FTS setup and the stamp. On an
  **existing** path it calls `open_db` (`init.rs:25-31`), so SCHEMA is *not* re-run. I verified in e8 that re-init
  does not restore a dropped index or trigger.
- `itr schema` prints `SCHEMA_PRAGMAS + SCHEMA` (`schema.rs:6-19`). It reflects the migrated **columns and tables**
  (#116), but it omits `issues_fts`, the three FTS triggers and the `user_version` generation (checked by diffing
  against a fresh DB's `sqlite_master`). It also lists the columns in fresh-DB order.

**FTS optionality and reindex.**
- `try_create_fts` ignores a failed `CREATE VIRTUAL TABLE`, and search then falls back to LIKE.
- If FTS5 is unavailable, `fts_needs_work` is permanently true, so **every** writable open takes `BEGIN IMMEDIATE`
  and retries creation. This follows from `db.rs:292-296`; I did not test it because the bundled build has FTS5.
- `itr reindex` and `doctor --fix` (for `fts_stale`) call `fts_rebuild`: drop the table and triggers, recreate,
  repopulate. It errors only when FTS5 is missing, and it reuses `InvalidValue` for that (#253, open).
- The fast path only checks whether the FTS *table* exists and is not legacy. It does **not** check the triggers.
  e3: a dropped `issues_fts_au` is not restored by fast-path opens, and a raw-SQL title update then leaves the old
  token matchable while `doctor` reports "All clean". The slow path does restore the trigger (`FTS_TRIGGERS` uses
  `IF NOT EXISTS`). CLI writes still stay fresh because `update` also calls `fts_index_issue`.

**`doctor` and the schema.** Doctor has six checks (`doctor.rs:112-163`): orphaned dependencies, dependency cycles,
issues stuck in-progress, empty epics, done blockers, and FTS row count. It has **no** check for schema shape,
generation, index or trigger presence, `integrity_check` or `foreign_key_check`.

**Tests guarding the mechanism** (unit tests, `db.rs` `mod tests`):
- `init_stamps_schema_generation_and_writer` (1897)
- `unstamped_db_is_stamped_on_open` (1907)
- `newer_schema_is_refused_before_migrating` (1927)
- `up_to_date_open_performs_no_write` (1982)
- `writer_stamp_normalizes_release_core` (2005)
- `config_reset_preserves_writer_stamp` (2028)
- `readonly_db_with_different_writer_opens` (2040)
- `writable_open_applies_each_structural_migration` (2083), which uses `STRUCTURAL_MIGRATION_GAPS` (2075-2080)
- `readonly_db_needing_structural_migration_is_refused` (2111)
- `readonly_db_with_fts_only_differences_opens` (2138)
- `writable_open_repairs_fts_with_current_stamp` (2168)
- `readonly_stamp_writes_are_advisory` (2192)
- `up_to_date_open_succeeds_under_write_lock` (2207)
- `schema_generation_is_rechecked_under_write_lock` (2232)
- `fresh_schema_matches_generation_fingerprint` (2258-2294; `(1, 0xc22d_a286_ef3a_52f4)`)
- `fts_legacy_contentless_table_is_migrated` (2297)
- `updated_at_trigger_const_matches_schema` (1729)

---

## (c) Stale or wrong documentation (exact quotes)

### docs/migrations.md
Last touched in `85cd626` on 2026-05-19, which predates both the FTS redesign and #264.

1. L33-34: "**No global schema version table** unless the project explicitly adds one. The "is this column/table
   there yet?" probe is the version check." **Wrong since v3.3.0.** `PRAGMA user_version` holds the generation
   (`SCHEMA_VERSION`), `NEWER_SCHEMA` refuses newer files, and the fingerprint test forces a bump.
2. L19-22 and L125-128 recommend `PRAGMA table_info(<table>)` probes, and the worked examples (L110-120, L211-236)
   show the old inline `prepare("PRAGMA table_info(issues)")…any()` and `SELECT COUNT(*) > 0 FROM sqlite_master`
   code. HEAD uses `has_issue_column` (`pragma_table_info`) and `has_schema_table` (`db.rs:371-385`, `387-440`). The
   code blocks no longer match the source.
3. L57-58: "Append a call after the last existing migration in `open_db`'s migration sequence." `open_db` no longer
   holds a sequence. Since `aa4dd4c`, a new structural migration must **also** be added to `schema_needs_migration`
   (`db.rs:279-284`); otherwise the fast path and read-only handles skip it. It should also be added to
   `STRUCTURAL_MIGRATION_GAPS`, `SCHEMA_VERSION` should be bumped, and `SCHEMA_FINGERPRINT` updated. None of these
   steps is mentioned.
4. L127-128: "The probe runs every time `open_db` runs". **False since `aa4dd4c`.** Up-to-date opens return before
   any migration helper runs.
5. L83-85: "FTS: call `fts_index_issue` after writes if the new column is searchable, and add the column to the FTS5
   virtual table in `try_create_fts` plus the insert in `fts_index_issue`." The table DDL now lives in `FTS_CREATE`,
   and indexing is trigger-driven (`FTS_TRIGGERS`). Changing an FTS column also needs a **new legacy detector**:
   `fts_is_legacy` only looks for the `contentless_delete` substring, so `CREATE … IF NOT EXISTS` would silently
   keep the old column set on existing DBs.
6. L351-353: "existing DBs need `itr reindex` for the FTS table to be rebuilt with the new columns". This is a
   manual step that the #264 design comment ("bump when the FTS design changes") implies should be automatic. The
   doc should say to extend `fts_is_legacy` and bump the generation.
7. L73-74: "New *columns* on `issues` flow through automatically via `ExportData.issue: Issue`." **Half true.** Export
   does, but import writes explicit column lists (`import.rs:230` `insert_issue_row`, plus `replace_issue_row`), so a
   new column is silently dropped on import unless both are edited. `models::Issue` also needs `#[serde(default)]`
   for old exports; `skills` and `assigned_to` have it.
8. L305: "A new `events` command + handler in `src/commands/events.rs`". **That file does not exist.** Events are
   surfaced by `itr log` (`src/commands/log.rs`, `cli.rs:497`).
9. L270: "`record_event(conn, issue_id, field, old, new)`". The name is fine, but the agent comes from `ITR_AGENT`
   inside the function (`db.rs:1308-1315`). Minor.
10. L325-329 suggests proving idempotency with `$ITR init` then `$ITR list`. At HEAD the second open takes the
    **fast path**, so it proves nothing about the migration helpers.
11. L363-365: CHANGELOG "upgrade implications (almost always zero, because migrations are automatic on the next
    `open_db`)". This is no longer true:
    - A generation bump is a **one-way door**: every binary from v3.3.0 on at a lower generation refuses the file.
    - Read-only copies of un-migrated DBs fail with `ReadOnlyNeedsMigration`.
    - Pre-v3.3.0 binaries are blind to the stamp.
12. It is missing the process for data migrations, table rebuilds (CHECK changes), FTS redesigns, SCHEMA-only
    objects (indexes and triggers, #241), and old-DB fixture testing.

### docs/schema.md
Last touched in `2dfb37e` on 2026-06-09, which predates #264.

1. L7-8: "The live schema is the base `SCHEMA` string plus the idempotent helpers called from `open_db`." This is
   inaccurate for existing DBs, because SCHEMA is never applied to them (#241).
2. L12-19, the `open_db` list: "runs `PRAGMA journal_mode=WAL`; runs `PRAGMA foreign_keys=ON`; runs idempotent
   migrations; attempts to create the optional FTS5 table…". **Stale.** It omits `busy_timeout`, the generation
   check and newer-DB refusal, the zero-write fast path (which skips WAL, migrations and FTS), read-only handling, the
   `BEGIN IMMEDIATE` transaction and the stamping.
3. L22-23: "`itr schema` prints the base `SCHEMA` string, not the migration-expanded runtime schema." Since #116 the
   SCHEMA const includes all migrated columns and tables. The real omissions are FTS, the triggers and the
   generation.
4. L44-45 and L51 treat `skills` and `assigned_to` as helper-added. They don't mention that migrated DBs have both
   columns **at the end** of the table (a different physical order from fresh DBs).
5. L229-231: "`try_create_fts` runs from `open_db` and `init_db`". Since `aa4dd4c` it only runs on the slow path, and
   a dropped FTS trigger is not repaired on the fast path (e3).
6. L259-260: doctor "reports the index as stale when the FTS row count differs". This is accurate, but it should say
   that this is the **only** schema-adjacent check.
7. L283-290: "All migrations live in `src/db.rs` and are wired from `open_db`: 1…5". This misses
   `migrate_and_stamp_schema` and `stamp_schema_version`.
8. L299: "Do not rely on a global schema version table unless one is added deliberately." **One was added (#264).**
9. L300: "Do not assume migration order except the order in `open_db`." There is no order in `open_db` any more.
10. L307 and L322: "Wire the helper in `open_db`." Wrong location. It also misses `schema_needs_migration`,
    `SCHEMA_VERSION` and the fingerprint.
11. The `config` section (L119-132) does not mention the reserved `last_writer_version` key.

### docs/backup-import-export.md
Updated on 2026-09-26, but schema compatibility is absent.

- It never mentions that export JSON carries **no schema or format version**, or that a file copy of a
  generation-N DB cannot be opened by an older generation's binary (`NEWER_SCHEMA`).
- It never says that restoring an old `.itr.db` copy is auto-migrated by the first writable open. This is one-way:
  the same file will then refuse older binaries.
- Minor: L54-56 says "Run any read-only `itr` command … Opening the database cleanly triggers a SQLite checkpoint".
  Since `aa4dd4c`, up-to-date opens do no writes. The checkpoint comes from SQLite's last-connection close, not from
  itr, so the wording is loose but mostly right.

### Other docs
- **README.md:469**: "Four tables: `issues`, `dependencies`, `notes`, `config`." **Stale since v2.0.0.** There are six
  tables plus `issues_fts`.
- **CONTRIBUTING.md:224**: "Add migrations as idempotent helpers called from `open_db`." This is incomplete (see
  migrations.md #3).
- **CONTRIBUTING.md:223**: "Enable WAL and foreign keys on every opened connection." Stale. The fast path
  deliberately skips `journal_mode=WAL`; WAL persists in the file.
- **docs/architecture.md:78-80**: "idempotent migrations called from `open_db`". Same issue; no generation guard is
  mentioned.
- **CLAUDE.md:69** (project): accurate on tables and columns, but it says nothing about `user_version`, the
  generation or `NEWER_SCHEMA`.
- **docs/troubleshooting.md:357-366**: documents `NEWER_SCHEMA`, but **`ReadOnlyNeedsMigration` ("database is
  read-only and needs migration; reopen it writable", code `DB_ERROR`) is documented nowhere** (grep across docs,
  README and CHANGELOG found no hits).
- **CHANGELOG.md**:
  - v1.2.0 says "Added: Claude Code skill support". It was actually the `skills` column and `migrate_add_skills`.
  - v2.0.0 ("Multi-agent support") lists none of its four schema additions or the FTS table.
  - The FTS redesign (v2.10.1) has no upgrade note.
  - Everything from v2.10.0 to v3.3.1, including the generation guard shipped in v3.3.0, is still under
    "## Unreleased". No release section records "generation 1 first appears in v3.3.0".
- **`db.rs:212-213`** (code comment): "Generation 1 is the v3.2 schema". The generation-1 *shape* actually dates
  from v2.10.1 (FTS redesign); the structural tables and columns date from v2.0.0. Only the *stamp* is new in v3.3.0.

---

## (d) Process gaps and test gaps

### Process gaps

**G1 (#241, still reproduces at HEAD).** SCHEMA-only objects never reach existing DBs.
- Experiment e2 dropped `idx_issues_status`, `idx_events_issue` and `trg_issues_updated_at`, then ran `itr list`.
  - On the fast path, nothing was restored.
  - On the slow path (forced with `user_version=0` and the writer row removed), only `idx_events_issue` came back,
    because `migrate_add_events` re-issues its own indexes.
- In both paths `updated_at` then stays frozen through `itr update`, while `doctor` prints "All clean" and exits 0.
  `itr init` on the existing file, `reindex` and `doctor --fix` don't repair it either (e8).
- The #264 fingerprint test makes this **worse in a subtle way**. Adding an index or trigger to SCHEMA fails the
  fingerprint test, which pushes the developer to bump `SCHEMA_VERSION`. Existing DBs then get stamped with the new
  generation **without** the new object, because `migrate_and_stamp_schema` only runs SCHEMA when `initialize` is
  true. The stamp then claims a shape the file does not have.
- A manual repair works and is safe on both physical shapes: `itr schema | sqlite3 .itr.db` restores everything and
  makes the fingerprint equal to a fresh DB.
- **Ordering constraint for the fix:** SCHEMA must run *after* `migrate_current_schema`. Running the current SCHEMA
  first on an oldest DB succeeds today, but a future index on a migrated column fails. Tested:
  `CREATE INDEX … ON issues(assigned_to)` gives "no such column: assigned_to".

**G2.** `schema_needs_migration` is a hand-maintained list that duplicates `migrate_current_schema`. Forgetting to
add a new probe has these effects:
- Read-only handles open un-migrated files and then hit "no such column" errors instead of `ReadOnlyNeedsMigration`.
- Dev builds that share a release core skip the migration until `SCHEMA_VERSION` or the writer changes.

Writable opens by a new release still run it once, because the writer stamp differs. Nothing enforces keeping the
two lists in sync, apart from the manual `STRUCTURAL_MIGRATION_GAPS` list.

**G3.** The fingerprint only covers **fresh** DDL, so the following changes would not force a generation bump:
- data or backfill migrations
- behavioural changes to trigger bodies that do not change DDL (they do change DDL, so they are covered)
- anything that exists only on migrated DBs

Migrated DBs legitimately carry a different fingerprint because of column order (`0x35990ee6b722076c` vs
`0xc22da286ef3a52f4`, §e), so "generation N" already means two physical shapes. This is harmless today (all SQL uses
explicit column lists), but it matters for any future "assert shape equals fresh" check, and for `SELECT *` in the
dangerous-SQL UI.

**G4.** FTS design changes have no general detector. `fts_is_legacy` is a single-purpose substring check, and
`fts_needs_work` does not look at the triggers (e3). A future FTS change (a new column, a tokenizer change) needs
new detection code plus a generation bump. It is documented nowhere.

**G5.** There is no table-rebuild or CHECK-change playbook. Any vocabulary change (status, kind, priority,
relation_type) needs the SQLite 12-step rebuild, which is incompatible with running inside the current
`BEGIN IMMEDIATE` with `foreign_keys=ON` (see §a). It has never been needed, but it is the most likely next hard
migration.

**G6.** There is no data-migration convention: nothing covers where a backfill lives, idempotency markers,
generation gating, or recording it in `events`.

**G7.** Downgrade and old-binary story:
- Pre-v3.3.0 binaries are blind to the generation stamp. This is reproduced (e7): v2.10.0 `reindex` bricks writes
  until a current binary reopens the file.
- There is no downgrade path, and export JSON has no version field, so an export from a newer generation imports
  into an older binary with unknown fields silently ignored by serde.
- A long-running `itr ui` checks the generation only once, at startup, because it holds a single connection. If a
  newer binary upgrades the file mid-session, the old UI keeps writing.

**G8.** `doctor` does no schema verification: no expected-object set, no `user_version` sanity check, no
`integrity_check` or `foreign_key_check`. #241 already suggests "have `doctor` assert the expected `sqlite_master`
object set".

**G9.** `itr schema` is not the live schema. It omits FTS objects and the generation, so it cannot serve as a
contributor's "expected objects" oracle without adding those.

**G10.** CHANGELOG and release discipline for schema changes is missing: no release section records which release
introduced which generation, and past schema releases were either misdescribed or undocumented (§c).

### Test gaps

- **No test opens a DB created by an old release.** All migration unit tests start from `init_db` (the current fresh
  shape) and *subtract* pieces (`STRUCTURAL_MIGRATION_GAPS`: drop a column or table; `FTS_DROP`; a synthetic legacy
  FTS table). That simulates "missing X" but never:
  - the real v1.0.0 through v1.1.0 shape (no skills or assigned_to columns and no events or relations tables all at
    once, with migrated column order);
  - the real v2.0.0 through v2.10.0 shape (legacy FTS plus migration-formatted DDL);
  - a populated old DB with notes, dependencies and config rows.
- The integration suite (`tests/integration.sh:143-207`, `INIT_SCHEMA_CHECK`) checks only a **fresh** `itr init`:
  the `skills`/`assigned_to` columns, the six tables plus `issues_fts`, the four event/relation indexes, and the
  events FK and relations UNIQUE/CHECK constraints. It does **not** check the seven base indexes,
  `trg_issues_updated_at`, the FTS triggers or `user_version`. The "second `itr init`" at L210 is a fast-path open
  and does not exercise migrations.
- `tests/integration.sh:706-726` (`--- schema ---`) only greps `itr schema` output. The snapshot contracts
  (`tests/contracts/help.sh:124-125`, `example.sh:28`) pin the schema text.
- There is no integration or CLI-level test for `NEWER_SCHEMA`, `ReadOnlyNeedsMigration` or the `last_writer_version`
  stamp. All of these are unit-level only.
- There is no test that a migrated DB equals a fresh DB modulo column order. Such a test would have caught #241.
- There is no test for SCHEMA-only object repair (#241) or for the dropped-FTS-trigger fast-path gap (e3).

---

## (e) Old-DB upgrade experiments (current `itr v3.3.0-2-g2e96f5d`)

| Exp | Setup | Command(s) | Result |
|---|---|---|---|
| **e1 oldest** | `oldest-schema-8704df7.sql` via sqlite3, plus three issues (epic, child with `parent_id`, done issue with `close_reason`), one dependency, one note, and config `urgency.priority.high`. `user_version=0`. | `itr --db … list --all -f json` | Exit 0. All rows are returned with `skills:[]` and `assigned_to:""`. After the run: `user_version=1`, `last_writer_version=v3.3.0`, `events` and `relations` tables plus four indexes, `issues_fts` created as `contentless_delete=1` **and populated (3 rows)**, three FTS triggers, the config row preserved. Column order: `…,created_at,updated_at,skills,assigned_to`. |
| e1 cont. | same DB | `doctor`; `search widget`; `search sprocket` (note-only); `add --skills rust`; `update 1 --title …`; search for the old and new tokens; `events` table; list twice with md5 | `doctor` exits 1, reporting only the genuine `done_blocker` data problem (no schema problem is reported, and none exists). Search finds title and note hits. The add stores skills. After the retitle the old token returns nothing and the new token returns [1]. The update wrote an event row. A second open leaves the md5 **identical** (fast path). |
| e1 fingerprint | `ddl.py` fresh vs e1 | — | Only the `issues` CREATE differs, in column order. The fresh fingerprint `0xc22da286ef3a52f4` matches the test constant. Migrated gives `0x35990ee6b722076c`, and **the repo's real `.itr.db` also gives `0x35990ee6b722076c`**. |
| e1 round-trip | `export` from e1, then `import` into a fresh `init` DB, then export again | — | Imported 4, notes 1, dependencies 1, events 1, `dropped_references` 0. The re-export is identical (ignoring child row ids). |
| **e4 legacy FTS (v2.0 through v2.10.0 shape)** | oldest + `v2.0-v2.10.0-legacy-shape.sql`. The legacy FTS table holds a stale token ('legacy'), and one issue was never indexed ('delta'). | chmod 444 and journal DELETE, then `list`; then chmod 644 and `list` | **Read-only:** exit 0, file unchanged, `user_version` stays 0 (FTS-only differences are allowed). **Writable:** `user_version=1`, the FTS table is recreated as `contentless_delete=1`, 'legacy' no longer matches, 'fresh' gives [1], 'delta' gives [2] (backfilled), and the triggers are installed. The fingerprint equals the e1 and repo value. |
| **e5 read-only oldest** | oldest shape, journal DELETE, chmod 444 | `list`, `list -f json` | Exit 1: `ERROR: database is read-only and needs migration; reopen it writable` with JSON code `DB_ERROR`. The file is unchanged. |
| **e6 newer generation** | fresh DB with `user_version=2`, writer `v9.0.0`, journal DELETE | `list`, `list -f json`, `init`, `doctor --fix` | All exit 1 with `NEWER_SCHEMA`: "Database schema generation 2 was written by itr v9.0.0; this itr (v3.3.0-2-g2e96f5d) only supports generation 1. Update itr: …". The md5 is unchanged and the journal mode stays `delete`. `itr schema` still works (it needs no DB). |
| **e7 old binary on a new DB (#264 repro)** | fresh generation-1 DB from current itr; `itr` built from v2.10.0 source | v2.10.0: `list`, `reindex`, `update`, `add`; then current `list` and `update` | v2.10.0 `list` works. `reindex` recreates `content_rowid=id` (legacy) while the v3 triggers remain. `update` fails: `Database error: cannot DELETE from contentless fts5 table: issues_fts`. `add` happened to succeed. `user_version` and the writer are untouched. The current binary's next open migrates FTS back, and writes succeed. |
| **e2 #241** | fresh DB; drop `idx_issues_status`, `idx_events_issue` and `trg_issues_updated_at` (fast path), plus a variant with `user_version=0` (slow path) | `list`, `update -p high`, `doctor` | Fast path restores nothing. Slow path restores only `idx_events_issue`. In both, `updated_at` stays frozen at the backdated value after `itr update`, and doctor prints "All clean" with exit 0. |
| **e3 FTS trigger** | fresh DB; drop `issues_fts_au` | `list` (fast path), raw SQL `UPDATE title`, FTS MATCH, `doctor` | The fast path does not restore the trigger. The stale token still matches in FTS, and doctor reports "All clean". The slow path (`user_version=0`) and `reindex` do restore it. CLI `update` stays fresh because it calls `fts_index_issue`. |
| **e8 repair paths** | fresh DB; drop `idx_issues_priority` and the trigger; `user_version=0` | `init` (existing file), `reindex`, `doctor --fix`, then `itr schema \| sqlite3` | Only `itr schema \| sqlite3` restores them, after which the fingerprint equals fresh. Piping it onto a migrated-order DB is a harmless no-op. |
| **e10/e11 ordering** | oldest shape | `itr schema \| sqlite3` first; then the same plus a hypothetical `CREATE INDEX … ON issues(assigned_to)` | Current SCHEMA-first succeeds on the oldest DB. With the hypothetical index it fails with "no such column: assigned_to", which means the #241 fix must run SCHEMA after `migrate_current_schema`. |

**Conclusion on upgrades.** Every historical on-disk shape I could reconstruct upgrades cleanly and losslessly to
generation 1 on the first writable open, including FTS backfill and legacy-FTS replacement. The weak points are:
- objects that exist only in SCHEMA (#241);
- FTS trigger drift on the fast path;
- read-only copies (a hard failure, and undocumented);
- pre-v3.3.0 binaries (blind to the generation);
- the lack of any fixture-based regression test that keeps these results true for the next migration.
