# Backup, Import, And Export

`itr` stores project issue state in one SQLite database, `.itr.db`. That file is
the source of truth. Export/import is a portable snapshot format for migration,
review, and recovery workflows.

## What To Back Up

Back up `.itr.db` when you want an exact local copy of the tracker:

```bash
cp .itr.db .itr.db.backup
```

Use a direct file copy when:

- You are staying on the same project and SQLite file format.
- You want all SQLite state exactly as stored, including indexes and internal
  metadata.
- You need a quick rollback before a bulk operation.

Use export/import when:

- You want a text snapshot.
- You are moving data between machines or repositories.
- You want to inspect or transform data before restoring it.
- You want merge behavior instead of replacing an existing database file.

## Backing Up Safely (WAL Companion Files)

`itr` opens SQLite in WAL (Write-Ahead Logging) mode. When the database has
been opened for writes, SQLite creates two companion files next to
`.itr.db`:

- `.itr.db-wal` — the write-ahead log holding pending changes not yet folded
  into `.itr.db`.
- `.itr.db-shm` — the shared-memory index SQLite uses to coordinate readers
  and writers.

If you copy only `.itr.db` while a writer is active (most commonly an
`itr ui` session, but also any in-flight `add` / `update` / `close` /
`claim` / `bulk` / `batch` invocation), the snapshot can miss writes that
are still parked in `.itr.db-wal`. In the worst case the resulting copy is
internally consistent but stale, or — if a checkpoint is mid-flight —
opens dirty and triggers `DB_ERROR` on the next `itr` command.

### Safe-Backup Procedure

When a writer **may** be running (the common case for shared projects or
when `itr ui` is open):

1. Stop every active writer first. Quit `itr ui` and confirm no other
   `itr` command is in flight.
2. Run any read-only `itr` command (for example `itr stats`) once. Opening
   the database cleanly triggers a SQLite checkpoint that folds
   `.itr.db-wal` back into `.itr.db`.
3. Copy **all three** files together if any companion files are still
   present, so the snapshot is consistent even if a checkpoint did not
   fully drain:

   ```bash
   cp .itr.db      .itr.db.backup
   cp .itr.db-wal  .itr.db-wal.backup  2>/dev/null || true
   cp .itr.db-shm  .itr.db-shm.backup  2>/dev/null || true
   ```

   The `2>/dev/null || true` guards are there because the companion files
   may legitimately not exist after a clean checkpoint.

When you can guarantee no writer is running and no companion files exist,
copying `.itr.db` alone is sufficient:

```bash
cp .itr.db .itr.db.backup
```

### Exports Are Always Safe

`itr export` reads through SQLite, which serializes the read against any
in-flight writes. You can run `itr export > snapshot.jsonl` while `itr ui`
is open without risking a torn snapshot — the export will reflect a
consistent point-in-time view. Use export/import (described below) as the
preferred backup path when stopping writers is inconvenient.

### Do Not Commit Companion Files

Add `.itr.db-wal` and `.itr.db-shm` to `.gitignore`. They are local
runtime state, can contain data not yet merged into `.itr.db`, and
conflict in unhelpful ways across machines. See
[Troubleshooting → WAL Companion Files](troubleshooting.md#wal-companion-files-itrdb-wal-itrdb-shm)
for the full lifecycle, when each file appears, and when it is safe to
delete them manually.

## Export Formats

Default export is JSONL: one issue bundle per line.

```bash
itr export > itr-backup.jsonl
```

JSON array export is available for tools that prefer one document:

```bash
itr export --export-format json > itr-backup.json
```

Each exported item contains:

- `issue`: the full issue row, including status, priority, kind, context,
  files, tags, skills, acceptance, parent ID, assignee, close reason, and
  timestamps.
- `notes`: all notes for the issue.
- `blocked_by`: dependency blocker IDs for the issue.
- `events`: audit events for the issue.
- `relations`: issue relations visible from the issue.

The default JSONL format is easier to stream and diff line-by-line. The JSON
array format is easier to load into tools that expect a single JSON document.

## Import Behavior

Import accepts either JSONL or a JSON array. If `--file` is omitted, import reads
from stdin.

```bash
itr import --file itr-backup.jsonl
cat itr-backup.jsonl | itr import
itr import --file itr-backup.json --merge
```

Import preserves issue IDs and uses `INSERT OR REPLACE` for issue and note rows.
Dependencies are inserted with `INSERT OR IGNORE`.

`--merge` skips imported issues whose IDs already exist:

```bash
itr import --file itr-backup.jsonl --merge
```

Use `--merge` when restoring into a database that may already contain some of
the exported issue IDs. Without `--merge`, imported issues with matching IDs are
replaced.

## Round-Trip Expectations

Current import/export preserves:

- issues
- notes
- dependency blockers
- tags, files, skills, and assignees
- parent IDs and close reasons
- created and updated timestamps

The export data shape also includes events and relations. The current importer
does not restore those fields; use a direct `.itr.db` file copy when you need a
full-fidelity backup that includes audit history and relation rows. If import
support for events or relations changes, add round-trip tests and update this
section.

When an import bundle contains `events` or `relations` records, import drops
those rows but still writes the issue, notes, and dependency data. A single
`REVIEW:` warning is emitted on stderr naming the dropped tables and the total
number of dropped rows, for example:

```
REVIEW: import dropped data from unsupported tables: events (12 row(s)), relations (3 row(s)). Round-trip restore of audit history and relation rows is not implemented; use a direct .itr.db file copy for full-fidelity backups. See docs/backup-import-export.md.
```

The warning goes to stderr only — it does not change the exit code, the stdout
import summary, or the `imported` / `skipped` counts. If you need a backup that
preserves audit events and relations, use a direct `.itr.db` file copy as
described above.

## Backup Before Bulk Changes

Before large changes, take a file backup and an export snapshot:

```bash
cp .itr.db .itr.db.before-bulk
itr export > itr-before-bulk.jsonl
```

Preview bulk operations when available:

```bash
itr bulk close --tag cleanup --dry-run -f json
itr batch update --dry-run -f json < updates.json
```

## Restore From A File Copy

Stop any running `itr ui` session first, then replace the database. Also
remove any stale `.itr.db-wal` / `.itr.db-shm` companion files so SQLite
does not try to replay an outdated write-ahead log against the restored
database:

```bash
cp .itr.db .itr.db.bad
rm -f .itr.db-wal .itr.db-shm
cp .itr.db.before-bulk .itr.db
itr doctor
```

If you saved companion files alongside the backup (see
[Backing Up Safely](#backing-up-safely-wal-companion-files)), restore them
together with `.itr.db` instead of deleting:

```bash
cp .itr.db.before-bulk      .itr.db
cp .itr.db-wal.before-bulk  .itr.db-wal  2>/dev/null || true
cp .itr.db-shm.before-bulk  .itr.db-shm  2>/dev/null || true
itr doctor
```

If you keep backups outside the project, restore with `--db`:

```bash
itr --db /path/to/restored/.itr.db stats
```

## Restore From Export

Create a fresh database and import the snapshot:

```bash
mkdir /tmp/itr-restore-check
cd /tmp/itr-restore-check
itr init
itr import --file /path/to/itr-backup.jsonl
itr stats
itr doctor
```

To restore into an existing project database without replacing existing IDs:

```bash
itr import --file /path/to/itr-backup.jsonl --merge
```

## Verify A Backup

Run these checks after creating or restoring a backup:

```bash
itr stats -f json
itr ready -f json
itr doctor
itr export > /tmp/itr-verify.jsonl
python3 -c 'import json,sys; [json.loads(line) for line in open(sys.argv[1]) if line.strip()]' /tmp/itr-verify.jsonl
```

For JSON array exports:

```bash
python3 -c 'import json,sys; json.load(open(sys.argv[1]))' itr-backup.json
```

## Contributor Notes

- Keep export data structured through `ExportData` in `src/models.rs`.
- Use serde for all JSON parsing and writing.
- Preserve stdout as data only; diagnostics belong on stderr.
- Add integration coverage for every new exported field.
- If a new table references issues, decide whether export/import should preserve
  it and add round-trip tests.
