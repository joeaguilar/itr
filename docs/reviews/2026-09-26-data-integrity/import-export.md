# itr import/export data-integrity review

Binary: `itr v3.3.0-2-g2e96f5d` (matches HEAD 2e96f5d). This was a read-only review: no repo files were edited and the repo `.itr.db` was not touched.
Scope: `src/commands/import.rs`, `src/commands/export.rs`, `models::{ExportData, Issue, Note, Event, Relation}`, and the `db.rs` helpers they call.
Repro workspace: `scratchpad/repro-import/`. `fresh.sh <name>` creates a fresh DB at `db-<name>/.itr.db`. `mk.py` has the payload builders `issue(id, **kw)` and `item(issue, notes, blocked_by, events, relations, **extra)`. The test payloads are `t*.jsonl`. `rt/` is the rich source DB and `rt2/` is its re-import.

Snippets below use `$S` for the repro dir and `mkpayload` as shorthand for a `python3 -c` script that imports `mk` and prints `json.dumps(item(...))` lines.

---

## Part A: Verified known issues

### #223 Import collision destroys notes, edges, and child links. Status: PARTIAL (the core bug is fixed)
- **Fixed:** `replace_issue_row` (import.rs:258-298) now uses `UPDATE ... WHERE id` instead of `INSERT OR REPLACE`, so no cascade fires.
  Repro: issues 1-4 with `2 -b 1`, `3 --parent 2`, `4 -b 2`, and a note on 2. Re-import only the issue-2 line (`$S/only2.jsonl`). Result: `2 blocked_by [1] notes 1`, `3 parent 2`, `4 blocked_by [2]`. Children and outgoing edges survive.
- **Still open against the acceptance criteria:**
  1. The REVIEW note never counts the collateral rows it removes. It says only "notes, audit events, incoming blockers, and relations now match the payload". Repro: import `only2b.jsonl` (issue 2 with `notes:[]`, `blocked_by:[]`). The existing note and the 1→2 edge disappear, and the only signal is the generic "replaced 1" note.
  2. Replace also deletes relations to DB issues that are not in the payload (`DELETE FROM relations WHERE source_id=?1 OR target_id=?1`, import.rs:293-296). No count is reported.
  3. No integration test covers the collision case. `tests/integration.sh:659` only asserts the imported count.
- **Also open:** the import writes no audit event for any of these changes. That part is tracked in #246.

### #230 Normalize status, priority, and kind during import. Status: PARTIAL
- **Still open:** the enum columns go straight into SQL (import.rs:229-249, 264-286), and the schema CHECK constraints (db.rs:16-21) reject them.
  ```
  $ mkpayload: issue 1 ok; issue 2 priority="urgent"   # t1.jsonl
  $ itr import --file t1.jsonl
  ERROR: Database error: CHECK constraint failed: priority IN ('critical', 'high', 'medium', 'low')   exit=1
  $ sqlite3 db "select count(*) from issues"  ->  0     # the valid issue 1 is lost too
  $ status="Open" (case only)  ->  CHECK constraint failed: status IN (...)  exit=1
  ```
  `normalize::normalize_{priority,kind,status}` is never called on this path.
- **Fixed:** the ticket's `let _ = tx.execute(...)` for dependency inserts is gone. Insert results are now counted (import.rs:163-166).
- **New related gap:** a bad `relation_type` does not fail at all. It is silently discarded. See IE-2.

### #242 parse_json_array silently defaults to `[]`. Status: STILL-OPEN, and the export path amplifies it
- `db.rs:506-508` still has `serde_json::from_str(&s).unwrap_or_default()`.
  ```
  itr add a -t keep; itr add b; itr add c
  sqlite3 db "update issues set tags='[\"keepme\", broken' where id=1;
              update issues set files='[1,2]' where id=2;
              update issues set skills='\"rust\"' where id=3;"
  itr export > pja.jsonl     -> exit 0, stderr EMPTY
  pja.jsonl: 1 tags [] / 2 files [] / 3 skills []
  itr doctor --format json   -> {"problems":[],"clean":true}
  ```
  The backup records the lossy view with no warning. Importing that backup makes the loss permanent.

### #269 Export is not a consistent snapshot. Status: STILL-OPEN
- `export.rs:7-22` runs `all_issues`, then per-issue `get_notes`, `get_blockers`, `get_events_for_issue`, and `get_relations`, with no enclosing read transaction.
- Repro: 400 issues, each with one note whose content equals its status. `writer.py` flips every status and every note content together in one transaction, in a loop.
  ```
  snap1.jsonl bundles 400 torn 187
  snap2.jsonl bundles 400 torn 190
  snap3.jsonl bundles 400 torn 187
  snap4.jsonl bundles 400 torn 194      (writer: 6856 txns in 6s)
  ```
- `docs/backup-import-export.md` ("Exports Are Always Safe") is still false.

### #272 Import accepts parent and dependency cycles. Status: STILL-OPEN
- A 3-node cycle (`t9.jsonl`: 1→2→3→1 on both `parent_id` and `blocked_by`) imports cleanly: `{"imported":3,"dependencies":3,"dropped_references":0}`, exit 0.
  - `itr ready` returns "No ready issues found".
  - `itr doctor` reports 3 `circular_dependency` problems, all `fixable:false`, and does not mention the parent cycle.
  - `itr list --parent 1` returns issue 3.
- `itr update 1 --parent 1` is refused ("Cycle detected"), so import is the only path that bypasses the check.

---

## Part B: New findings

### IE-1 · P1 · import.rs:113-136 (no ID range check), db.rs:14 (AUTOINCREMENT)
**Defect:** importing an issue whose ID is `i64::MAX` makes every later `itr add` fail, even after the row is deleted. Separately, negative and zero IDs are accepted.
```
$ mkpayload: issue(9223372036854775807) > t5.jsonl
$ itr import --file t5.jsonl                 -> IMPORT: 1 imported ... exit 0
$ itr add "after huge"                       -> ERROR: Database error: database or disk is full   exit 1
$ sqlite3 db "PRAGMA foreign_keys=ON; delete from issues"
$ itr add "after delete"                     -> ERROR: Database error: database or disk is full   (sqlite_sequence stays at 9223372036854775807)
$ itr doctor --format json                   -> {"problems":[],"clean":true}
```
Negative and zero IDs (`t4.jsonl`: -5, 0, 1000) import with exit 0. The sequence advances correctly afterwards (the next add gets 1001, so there is no collision). But `itr get -5` fails with clap's "unexpected argument '-5'", so the ID only works after `--`, and options must come before it (`itr update -- -5 --status done` fails).
**Fix:** in pass 1, refuse any `id < 1` or `id > MAX_IMPORT_ID` (for example 2^53), skipping the item with a REVIEW note. Add a doctor check for `sqlite_sequence` near `i64::MAX`.

### IE-2 · P2 · import.rs:195-204
**Defect:** `INSERT OR IGNORE INTO relations` also suppresses CHECK violations. A bad or unnormalized `relation_type` is silently dropped: it is not counted in `dropped_references` and gets no REVIEW note.
```
$ t1c.jsonl: issue 1 relations=[{source 1, target 2, relation_type "dupe", created_at "x"}], issue 2
$ itr import --file t1c.jsonl
IMPORT: 2 imported ... relations 0        exit 0, stderr empty
```
**Fix:** normalize and validate `relation_type` first (default to `related` with a REVIEW note). Replace `OR IGNORE` with `INSERT ... ON CONFLICT(source_id,target_id,relation_type) DO NOTHING`, which only covers uniqueness conflicts. The same applies to `INSERT OR IGNORE INTO dependencies` at import.rs:164.

### IE-3 · P2 · import.rs:109-144 (duplicate IDs within one payload)
**Defect:** duplicate issue IDs in one payload produce a merged issue. Scalar fields come from the last copy, while notes, blockers, and events are the union of all copies. The counts and REVIEW note also misreport what happened.
```
$ t6.jsonl: issue 2; issue 1 "first copy" notes=[A] blocked_by=[2]; issue 1 "second copy" status=done notes=[B]
$ itr import --file t6.jsonl
REVIEW: import replaced 1 existing issue(s) ...            # nothing pre-existed
IMPORT: 3 imported, 0 skipped, 1 replaced (notes 2, dependencies 1 ...)    # only 2 issues exist
$ itr get 1  ->  "second copy" done blocked_by [2] notes ['note from first', 'note from second']
$ same file with --merge  ->  "first copy" open, "1 skipped" (the first copy wins silently)
```
**Fix:** run a pre-pass that detects duplicate `issue.id` values. Keep one copy deterministically (last for replace, first for merge), drop the other copies' child rows, and report `duplicate_ids` in a REVIEW note and in the JSON output.

### IE-4 · P2 · models.rs:19-38, 345-354; import.rs:323-331
**Defect:** unknown or misspelled fields are silently ignored because there is no unknown-key detection. The data is lost and the import exits 0. `add --stdin-json` emits REVIEW notes for unrecognized fields, so import breaks both the "never silently swallow input" rule and cross-path consistency.
```
$ t7.jsonl issue 2 with "parent":1, "assignee":"bob", "tag":["zzz"], and bundle keys "blockedby":[1], "note":[...]
$ itr import --file t7.jsonl  -> IMPORT: 2 imported ... dependencies 0   exit 0, stderr empty
$ sqlite3: 2|||[]   (no parent, no assignee, no tags); dependencies=0
```
**Fix:** deserialize each record to `serde_json::Value` first and compare its keys against the known sets for the bundle and for the issue. Emit a REVIEW note per unknown key, and treat `parent` as an alias of `parent_id`, as batch add does.

### IE-5 · P2 · import.rs:320-331; models.rs:19-38, 345-354
**Defect:** parsing is all-or-nothing, and JSONL errors point to the wrong place. Each JSONL line is parsed separately, so serde always reports "line 1", and the record index is never given. Many fields are also required with no default (`context`, `acceptance`, `close_reason`, `created_at`, `updated_at`, `files`, `tags`, `notes`, `blocked_by`). One malformed record aborts the whole file.
```
$ t8.jsonl: 4 records, record 3 has no "notes" key
ERROR: JSON parse error: missing field `notes` at line 1 column 315    exit 1   (the problem is on line 3; 0 of 4 imported)
$ missing "context" -> ERROR: ... missing field `context` ...
$ {"issue":{"id":"7"}} -f json -> {"error":"JSON parse error: invalid type: string \"7\", expected i64 at line 1 column 18","code":"PARSE_ERROR"}
```
**Fix:** use `.enumerate()` and `map_err` so errors carry the real line number. Add `#[serde(default)]` to the optional text, array, and timestamp fields, with default timestamps set to now plus a REVIEW note. If atomic restore is intended, keep the rollback but name the failing record.

### IE-6 · P2 · import.rs:151-204, 229-249 (timestamps are never validated)
**Defect:** `created_at` and `updated_at` on issues, notes, events, and relations accept any string. Downstream time logic then quietly produces wrong results.
```
$ t3.jsonl: issue1 created_at="garbage", updated_at=""; note/event created_at "not-a-date"/"zzzz";
            issue2 created_at="2026-01-01 00:00:00", updated_at="...+05:00"; issue3 year 2999
IMPORT: 4 imported ... exit 0
itr list: #2 urgency 3.0 vs #4 (valid 2020 date) 5.0     -> util::days_since returns 0, so the age component is dropped
itr log --since 1d  -> returns the "zzzz" event           (string comparison: "zzzz" >= any date)
itr log             -> "zzzz" event sorts as newest permanently
itr doctor          -> {"problems":[],"clean":true}
```
**Fix:** parse every timestamp with chrono. Accept RFC 3339 and convert it to `%Y-%m-%dT%H:%M:%SZ` UTC. On failure, use now plus a REVIEW note, and optionally warn on future dates. Add a doctor check for malformed timestamps.

### IE-7 · P2 · import.rs:213-219 (the list columns bypass the normalization add applies)
**Defect:** `add` trims and lowercases skills and drops empty entries (add.rs:79-84, 223-232; `util::parse_comma_list*`). Import stores the arrays verbatim, so skill filters miss imported issues.
```
$ t2.jsonl issue 4: tags ["A","A","",""," x "], skills ["Rust","RUST"," Go "], files ["","a.rs","a.rs"]
stored verbatim: ["Rust","RUST"," Go "] ...
$ itr list --skill rust  -> "No matching issues found."
```
`assigned_to` is not trimmed either (`"  bob\n"` is stored as-is).
**Fix:** route imported `files`, `tags`, and `skills` through the same helpers as add (trim, drop empties, lowercase and dedupe skills). Emit a REVIEW note when anything changes.

### IE-8 · P2 · import.rs:187-205
**Defect:** a bundle can carry relations that don't involve its own issue. Under `--merge` these relations modify existing issues that were skipped. Replace mode can inject relations between any two DB issues the same way.
```
$ DB has #1 "one", #2 "two". mrel.jsonl: issue 1 (skipped by --merge); issue 3 with relations=[1 -> 2 duplicate]
$ itr import --merge --file mrel.jsonl -> imported 1, skipped 1, relations 1
$ itr get 1 -> relations [{source 1, target 2, duplicate}]     # the "kept" issue was modified
```
**Fix:** only accept a relation if `source_id == issue.id || target_id == issue.id`, and count the rest as dropped. Consider skipping relations whose other endpoint was merge-skipped, unless the link is new.

### IE-9 · P3 · export.rs:12 (blocked_by only), db.rs:1063, 1327, 1344, 1351, 1512 (ORDER BY created_at with no id tiebreaker)
**Defect:** export → import → export is not identical. The known losses:
(a) `dependencies.created_at` is never exported, so it resets to import time (`4|1|…19:21:36Z` becomes `4|1|…19:21:44Z`).
(b) Event and relation row IDs are renumbered per issue. The global order of same-second events changes, so `itr log` output differs after the round-trip. `diff` of the two logs shows issue 2's `status`, `close_reason`, and `note_added` rows moving.
(c) Per-issue notes, events, and relations are ordered only by second-granularity `created_at`, so the intra-second order is not deterministic.
(d) `relations` in `sqlite_sequence` rises to 4 for 2 rows, because each ignored mirrored insert still consumes an ID.
Issues, notes (content), unicode, newlines, quotes, parent links, and closed issues all round-trip correctly (the `issues` and `notes` tables were identical).
**Fix:** export blocker edges with their `created_at` (for example `blocked_by_edges: [{id, created_at}]`, keeping `blocked_by` for compatibility). Add `, id ASC` tiebreakers. In import pass 2, insert events globally in (`created_at`, source `id`) order.

### IE-10 · P3 · db.rs:529-547 (row_to_issue), 549-557 (row_to_note), export.rs:7-14
**Defect:** a single corrupt cell (invalid UTF-8 text or a BLOB) aborts the entire export, and the error does not name the issue. This makes the backup path unusable in exactly the situations where it is needed.
```
$ sqlite3 db "update issues set context=CAST(x'61ff62' AS TEXT) where id=2"
$ itr export -> ERROR: Database error: Conversion error from type Text at index: 5, invalid utf-8 ...  exit 1   (itr list fails the same way)
$ notes.content = x'00ff' (BLOB) -> ERROR: Database error: Invalid column type Blob at index: 2, name: content
```
**Fix:** read TEXT columns as `ValueRef`, lossily convert non-UTF-8 or BLOB values, and emit a REVIEW note naming the table, id, and column.

### IE-11 · P3 · export.rs:24-34
**Defect:** unknown export formats silently fall back to JSONL. `--export-format JSON` (uppercase) and `--export-format csv` both produce JSONL with no warning. The global `-f json` / `-f pretty` is ignored and also produces JSONL.
**Fix:** match case-insensitively, emit a REVIEW note on unknown values, and either treat `-f json` as an alias or warn that it has no effect.

### IE-12 · P3 · import.rs:320-331
**Defect:** the format sniffing is fragile. A UTF-8 BOM (common from Windows editors and PowerShell `>`) makes both JSON and JSONL fail with "expected value at line 1 column 1". A single pretty-printed object (not wrapped in an array) fails with "EOF while parsing an object at line 1 column 1".
**Fix:** use `input.trim_start_matches('\u{feff}')`. If the input starts with `{`, try parsing the whole document as one `ExportData` before falling back to line-by-line parsing.

### IE-13 · P3 · import.rs:229-249 and 264-286 (status and close_reason), 158-167 (closed blockers)
**Defect:** import can create states that the normal paths never produce:
- `status:"open"` with a `close_reason`.
- `status:"done"` with no reason.
- Blocker edges from a done issue. Close paths auto-remove these, but here doctor flags `done_blocker` immediately after the import.
Duplicate `blocked_by` entries are deduplicated silently, which is benign.
```
$ issue 1 done; issue 2 blocked_by [1,1,1] -> dependencies 1; itr doctor -> "Done/wontfix issue 1 still blocks issue 2" (fixable)
```
**Fix:** after pass 2, run the same invariant cleanup as close (`remove_blocker_edges` for closed blockers). Emit a REVIEW note for status and close_reason mismatches.

### IE-14 · P3 · export.rs (whole file); cli.rs:388 ("Export the full database")
**Defect:** the `config` table is neither exported nor imported, so custom urgency coefficients are lost on restore.
```
$ itr config set urgency.priority.high 99; itr export | grep -c 99 -> 0
```
**Fix:** add an optional config section to the export and import, or change the help text and docs to say config is excluded.

### IE-15 · P3 · shared with add, not import-specific · import.rs:229-249
**Defect:** empty, whitespace-only, and control-character titles (including NUL, ESC, and BEL) are stored and written raw to stdout. `itr list --format oneline | cat -v` shows `a^@b^[[31mred^G`, which allows terminal escape injection from an imported file. `itr add ""` and `itr add $'a\e[31m'` are accepted too, so the fix belongs in the shared creation path (#237).
**Fix:** use one validator for all creation paths: trim, and fall back to "(untitled)" plus a REVIEW note when empty. Strip or escape C0 controls on output in compact, oneline, and pretty modes.

### IE-16 · P3 · code-read only, not reproduced · import.rs:62, 115
**Defect:** `db::issue_exists(...).unwrap_or(false)` turns a DB error into "does not exist". In `KnownIssues::contains`, a transient DB error would drop a valid parent, blocker, or relation and report it as a *dangling reference*. In pass 1 it would attempt an INSERT that then fails with a misleading PK error.
**Fix:** propagate with `?` (`contains` returns `Result<bool, ItrError>`).

---

## Round-trip summary (rt → rt2)
Source DB: 5 issues (an epic with unicode, quotes, tabs, newlines, and a backslash; parent links; one done and one wontfix issue; 2 notes with newlines and CJK; 2 relations; 1 open dependency; 11 events).
- The `issues` and `notes` tables are identical.
- `events` rows and `relations` rows are renumbered.
- `dependencies.created_at` changes (IE-9).
- The two exports differ only in note, event, and relation `id` values. The ordering of issue 4's relations changes too.
- FTS: `itr search Alpha` and `itr search back` find the imported issue. The FTS row count equals the issue count, and doctor reports clean.
- AUTOINCREMENT after importing ID 1000 is correct: the next add gets 1001.
- The `updated_at` values in the payload are preserved verbatim, because the trigger is suspended and restored inside the transaction. The trigger is present after commit.
