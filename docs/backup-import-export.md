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

Stop any running `itr ui` session first, then replace the database:

```bash
cp .itr.db .itr.db.bad
cp .itr.db.before-bulk .itr.db
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
