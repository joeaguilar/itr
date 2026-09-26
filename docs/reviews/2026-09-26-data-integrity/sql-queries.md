# itr SQL correctness review

- Scope: every SQL statement in `src/db.rs`, `src/commands/*.rs`, and `src/urgency.rs`.
- Code at HEAD `d017daa`. `itr` on PATH reports `v3.3.0-2-g2e96f5d`. Bundled SQLite: libsqlite3-sys 0.28 (SQLite 3.45.x), so `contentless_delete=1` is supported.
- The review was read-only. The repo and its `.itr.db` were not touched. All reproductions ran in fresh DBs under `scratchpad/repro-sql/{a,b,c,conc}` with `ITR_DB_PATH` set.

---

## 1. Verified known issues

| Issue | Verdict | Evidence |
|---|---|---|
| **#240** summary builder swallows dependency-query errors | **STILL-OPEN** (the scope is wider than the ticket says) | `src/commands/mod.rs:64-66` still has `get_blockers(..).unwrap_or_default()`, `get_blocking(..).unwrap_or_default()` and `is_blocked(..).unwrap_or(false)`. The same `is_blocked(..).unwrap_or(false)` pattern also appears in `db.rs:667,672` (the `list_issues` blocked filter, which also drives `next`/`claim` candidate selection, so a query error makes a blocked issue claimable), `stats.rs:44`, `summary.rs:83`, `graph.rs:30`, and `search.rs:139-140`. `urgency.rs:285-303` and `:324` fall back to a REVIEW note, and `UrgencyConfig::load_key` (`urgency.rs:106`) silently ignores DB errors. The fix should cover every one of these sites, not just `mod.rs`. |
| **#241** `open_db` never reconciles SCHEMA objects | **STILL-OPEN** | Repro in `b/`: `DROP TRIGGER trg_issues_updated_at; DROP INDEX idx_issues_status; DROP TRIGGER issues_fts_au;`, then `itr list` and `itr update 1 --title y`. Afterwards `sqlite_master` still lacks all three objects, and `itr doctor` prints `DOCTOR: All clean` with exit 0. There is an extra angle: `fts_needs_work` (`db.rs:292`) only checks that the `issues_fts` table exists or is legacy, so missing FTS **triggers** are never recreated either. `try_create_fts` also discards trigger-creation failures (`db.rs:1606`, `let _ =`). Doctor's FTS check is a row count only (`doctor.rs:165-178`). |
| **#244** redundant queries; `--limit` applied after building summaries | **STILL-OPEN** | `list.rs:17-29` builds every summary and then calls `truncate(n)`. `ready.rs:26`, `search.rs:196` and UI `ui.rs:1221` do the same. `is_blocked` still runs twice per summary (`urgency.rs:296` and `mod.rs:66`). The search FTS post-filter still makes up to three `get_issue` calls per candidate (`search.rs:99-122`) plus a fourth at `:136`. |
| **#247** multi-statement dangerous SQL has two different behaviours | **STILL-OPEN** | Repro in `b/` (`itr ui --allow-dangerous`). `SELECT 1; UPDATE issues SET title='mutated' WHERE id=1;` returns `changes:0` and the title is unchanged, so the UPDATE is silently dropped. `UPDATE ... ; SELECT 1;` returns `changes:1` and the title becomes `m2`. `run_sql` (`ui.rs:779-837`) still runs without a transaction. |
| **#251** duplicated filter and exec blocks in the search helpers | **STILL-OPEN** | The status/priority/kind block and the params/prepare/collect tail are still byte-identical at `db.rs:1157-1183` and `db.rs:1209-1234`. |
| **#275** one literal term's match hides another term's false positive | **STILL-OPEN** | Repro in `a/`: `itr add "auth 100"; itr search "auth 100%" -f json` returns `[(6,'auth 100',['title'])]`. There is a second trigger for the same bug: the LIKE path matches JSON punctuation in `tags`/`files`/`skills` (`files='[]'` always contains `[`). So `itr search "plain ["` returns issue 1 ("plain issue") even though no field contains `[`. `search.rs:148` still tests only `matched_fields.is_empty()`. |

---

## 2. New findings

### SQ-1 · P1 · DEFERRED read-then-write transactions fail with "database is locked" under concurrent writers

**Where**
- `unchecked_transaction()` (DEFERRED) is used, with a read as the first statement, at:
  - `close.rs:116,225`
  - `add.rs:125`
  - `batch.rs:200,388,511,727`
  - `bulk.rs:82,200,275,351,428`
  - `note.rs:85`
  - `relate.rs:73`
  - `depend.rs:59`
  - `ui.rs:953,1106`
- Only `claim_issue` (`db.rs:749`) and migrations use `TransactionBehavior::Immediate`.

**SQL**

```sql
BEGIN DEFERRED;
SELECT ... FROM issues WHERE id=?;
INSERT INTO events ...;
UPDATE issues ...;
COMMIT;
```

**Defect.** In WAL mode, a DEFERRED transaction takes a read snapshot at its first SELECT. If it later tries to write after another connection has committed, SQLite returns `SQLITE_BUSY_SNAPSHOT` **without calling the busy handler**. The `busy_timeout=5000` in `open_schema_db` therefore does not help, and parallel agents get hard failures.

**Repro** (`conc/`, 80 issues)

```
python: 80 parallel `itr close <id> done` (distinct ids)
-> close failures: 4   ['ERROR: Database error: database is locked', ...]
   status: done|76, open|4
3 rounds of 40 parallel `itr close a b` (multi-id): failures 1, 6, 2
Control: 80 parallel `itr update <id> --priority high` -> 0 failures
         (its tx starts with a write); 80 parallel `itr claim <id>` -> 0 (IMMEDIATE)
```

**Fix.** Add a `db::write_tx(conn)` helper that returns `Transaction::new_unchecked(conn, TransactionBehavior::Immediate)`, and use it for every mutating transaction. Alternatively, retry on `SQLITE_BUSY` / `BUSY_SNAPSHOT`. IMMEDIATE is the right fix because it also removes the stale-read window (see SQ-12).

---

### SQ-2 · P2 · Race in single-ID `itr depend` can create a dependency cycle

**Where.** `depend.rs:112` calls `db::add_dependency(conn, …)` in autocommit mode, with no transaction. `add_dependency` (`db.rs:853-900`) runs this sequence:

```sql
SELECT COUNT(*) > 0 FROM dependencies WHERE blocker_id=?1 AND blocked_id=?2;
SELECT blocked_id FROM dependencies WHERE blocker_id=?1;   -- has_path BFS
INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?1, ?2);
INSERT INTO events ...;
```

**Defect.** The cycle check and the insert are separate autocommit statements. Two concurrent opposing `depend` calls can both pass `has_path` and both insert, producing the cycle the command promises to reject with a hard error. Both issues are then permanently blocked and never appear in `next` or `ready`. Doctor reports the cycle but cannot fix it.

**Repro** (`conc/`, 200 issues, 100 opposing pairs launched simultaneously)

```
Counter({'ok': 101, 'Cycle detected': 99})      # one pair: both succeeded
sqlite3: reciprocal edges = 1
itr doctor -> PROBLEM: [circular_dependency] Cycle: 1 -> ... -> 2 / 2 -> ... -> 1
```

**Fix.** Wrap `add_dependency` (and the multi-ID and batch callers) in an IMMEDIATE transaction so the check and the insert happen under the write lock. The edge insert and the event insert then also become atomic.

---

### SQ-3 · P2 · UI stats count closed issues as "blocked", so "ready" is under-reported

**Where.** `ui.rs:1464-1489` (`stats_value`, sent to the UI via `/api/bootstrap`).

**SQL.** `db::is_blocked` is called for **every** issue:

```sql
SELECT COUNT(*) FROM dependencies d JOIN issues i ON d.blocker_id=i.id
WHERE d.blocked_id=?1 AND i.status NOT IN ('done','wontfix')
```

**Defect.** `blocked` includes done and wontfix issues whose blocker is still open. Then `ready = active - blocked` subtracts terminal issues from the active count. The CLI `stats.rs:43` correctly counts only non-terminal issues.

**Repro** (`a/`): run `itr depend 8 --on 7` and then `itr close 8`. The edge 7→8 remains because only edges where the *closed* issue is the blocker get cleaned up.

```
CLI stats : {'blocked': 0, 'ready': 7}
UI  stats : {'active': 7, 'done': 1, 'blocked': 1, 'ready': 6}
```

**Fix.** Skip `done`/`wontfix` issues before calling `is_blocked`, as `stats.rs` does. Better, compute the count in one SQL aggregate:

```sql
SELECT COUNT(DISTINCT d.blocked_id) FROM dependencies d
JOIN issues b ON b.id=d.blocker_id
JOIN issues x ON x.id=d.blocked_id
WHERE b.status NOT IN ('done','wontfix')
  AND x.status NOT IN ('done','wontfix')
```

---

### SQ-4 · P2 · UI `PATCH status` diverges from every CLI close path: no edge cleanup, and an unknown status reopens the issue

**Where.** `ui.rs:985-992` (`patch_issue`). This is reachable from the detail-panel status `<select>` (`app.js:239-240`).

**SQL.**

```sql
UPDATE issues SET status = ?1 WHERE id = ?2
```

Unlike `close.rs:157-158`, `update.rs:385-387`, `bulk.rs:91-101`, `batch.rs:464-465,679-680` and UI `resolve_issue`, it never runs `get_newly_unblocked` / `DELETE FROM dependencies WHERE blocker_id=?1`.

**Defect.** Setting status to done or wontfix through PATCH leaves stale blocker edges behind, so doctor reports a `done_blocker`. It also reports no unblocked issues. An unrecognised status falls back to `"open"`, which force-reopens a closed issue. The CLI keeps the current status here (#163).

**Repro** (`a/`, UI on port 0)

```
itr depend 10 --on 9;  PATCH /api/issues/9 {"status":"done"}
 -> sqlite dependencies still has 9|10 ; issue 10 blocked_by:[9] is_blocked:False
 -> itr doctor: PROBLEM: [done_blocker] Done/wontfix issue 9 still blocks issue 10 (exit 1)
PATCH /api/issues/9 {"status":"bogus"} -> status: open   (was done)
```

**Related, P3.** UI list filters (`ui.rs:1159-1171`, `query_list`) compare `status`, `priority` and `kind` raw, with no `normalize_*_filters`. As a result `?status=wip` silently matches nothing, which is inconsistent with #168 in the CLI.

**Fix.** Route terminal transitions through the same helper as `update.rs`: record the event, update the row, call `get_newly_unblocked`, then `remove_blocker_edges`. Keep the old status on a validation failure and add a REVIEW note. Normalise UI filter values.

---

### SQ-5 · P2 · `--skill` filters are case-sensitive, but skills are stored lowercased

**Where**
- `db.rs:654-661` (`list_issues`: `i.skills.contains(s)`), which feeds `list`, `next`/`claim` and `ready`
- `search.rs:176-183`
- `ui.rs:1174`

Writes lowercase skills (`update.rs` `parse_comma_list_lower`, `ui.rs clean_list(…, true)`, add). Filter values are never lowercased.

**Repro** (`b/`)

```
itr add "case test" -t UI --skill Rust   -> stored skills ["rust"], tags ["UI"]
itr list --skill Rust -> []          itr list --skill rust -> [#2]
itr next --skill Rust -> []          (agent gets "No eligible issues")
itr list --tag ui -> []              (tags are case-preserving; P3 by itself)
```

**Fix.** Lowercase and trim skill filter values once, when the `ListFilter` or search arguments are built, matching the write path. Consider case-insensitive tag matching too, or emit a REVIEW note when a differently-cased tag exists.

---

### SQ-6 · P2 · Search results depend on unrelated issues: FTS token matching vs LIKE substring matching

**Where.** `search.rs:91-94` together with `db::fts_search` (`db.rs:1665-1685`). The LIKE fallback runs only when FTS returns zero IDs.

**SQL.** The FTS path runs:

```sql
SELECT rowid FROM issues_fts WHERE issues_fts MATCH '"auth"' ORDER BY rank
```

This is token-exact. The fallback uses `title LIKE '%auth%' ESCAPE '\'`, which is a substring match.

**Defect.** The same query returns different issues depending on whether *some other* issue contains the exact token. This is separate from the documented notes limitation (#148): it affects single terms and single fields. The docs describe "substring" semantics only for the fallback path and never say that adding an issue can remove a result.

**Repro** (`a/`)

```
itr add "authentication overhaul"; itr search auth   -> #3 authentication overhaul
itr add "auth token bug";          itr search auth   -> #4 only (#3 vanished)
```

**Related, P3.** LIKE is ASCII-only case-insensitive, while FTS `unicode61` folds Unicode case and diacritics and the post-filter uses Unicode `to_lowercase`. So non-ASCII case matching also depends on which path runs.

**Fix.** Always union the FTS candidates with the LIKE candidates (the literal post-filter already guards precision). Alternatively, use FTS prefix queries (`"auth"*`) plus the literal post-filter. Either way, document the result.

---

### SQ-7 · P2 · `itr log --since` compares raw strings, so non-canonical input misfilters or silently returns nothing

**Where.** `db.rs:1389-1391`. The `since` value is passed through raw from `cli.rs:505-507` via `log.rs:44`.

**SQL.**

```sql
... WHERE created_at >= ?N ORDER BY created_at DESC, id DESC LIMIT ?M
```

**Defect.** The comparison is lexicographic TEXT against `YYYY-MM-DDTHH:MM:SSZ`. A space separator (`' ' < 'T'`), a timezone offset, or an unparsable value like `yesterday` all misfilter silently. The soft-fallback rule says to warn instead.

**Repro** (`c/`, events at 08:00Z and 20:00Z on 2026-01-05)

```
--since '2026-01-05T12:00:00Z'       -> [20:00Z]                 (correct)
--since '2026-01-05 12:00:00'        -> [08:00Z, 20:00Z]         (wrong: includes 08:00)
--since '2026-01-05T12:00:00+05:00'  -> [20:00Z]                 (wrong: 07:00Z cutoff, 08:00 dropped)
--since yesterday                    -> []  exit 0, no REVIEW
```

**Fix.** Parse `since` with chrono, accepting RFC 3339, date-only and space forms. Re-emit it in the canonical `%Y-%m-%dT%H:%M:%SZ` form. Emit a REVIEW note and ignore the filter when it cannot be parsed.

---

### SQ-8 · P3 · "blocked" and "ready" aggregates mean different things in stats, summary and the UI

- `db::is_blocked` is the single definition used everywhere, and it correctly ignores done/wontfix blockers. The aggregates built on top of it differ:
  - `stats.rs:43-50` counts open **and** in-progress issues.
  - `summary.rs:62-86` counts only `open`, so in-progress blocked issues fall into neither bucket. It also puts wontfix into `done` and into `completion_pct`.
  - UI stats has the bug described in SQ-3.
- **Repro** (`c/`): with A (open) blocking B (in-progress):
  ```
  stats   : {'blocked': 1, 'ready': 1}
  summary : {'open': 1, 'in_progress': 1, 'blocked': 0, 'ready': 1}
  ```
- **Fix.** Pick one definition, for example "active = open ∪ in-progress; blocked/ready partition active". Share it through a single SQL aggregate. Label wontfix separately.

---

### SQ-9 · P3 · Ordering depends on the query plan: no tiebreak on second-resolution timestamps, or no ORDER BY at all

**Missing tiebreak:**
- `db.rs:1063` `get_notes ... ORDER BY created_at ASC`
- `db.rs:1327` `get_events_for_issue ... ORDER BY created_at ASC`
- `db.rs:1344,1351` `get_recent_events ... ORDER BY created_at DESC LIMIT ?`, used by `summary` "last 5"
- `db.rs:1512` `get_relations ... ORDER BY created_at ASC`

**No ORDER BY:**
- `db.rs:1131` `search_issue_ids` (`SELECT DISTINCT`; urgency ties then follow scan order)
- `db.rs:1199` `search_note_issue_ids`
- `db.rs:1289` `all_dependencies` (export and graph edge order)
- `db.rs:961,969` `get_blockers` / `get_blocking` (the `blocked_by` / `blocks` arrays)

`get_events_filtered` already does this correctly with `created_at DESC, id DESC`.

**Repro** (`b/`). Two relations with the same `created_at` on issue 3. The MULTI-INDEX OR plan emits rows from the source index first:

```
relations order: [(2, 3, 5), (1, 4, 3)]    # id 2 before id 1
```

Notes happen to come out in id order today (the index scan feeds the temp B-tree), but nothing guarantees that.

**Fix.** Append `, id` to every `ORDER BY created_at`, and add `ORDER BY` to the ID and edge queries.

---

### SQ-10 · P3 · `add_dependency` accepts a done or wontfix blocker, creating an immediate doctor problem

- **Where:** `db.rs:853-900`. The blocker's status is never checked.
- **Repro** (`c/`):
  ```
  itr close 3; itr depend 4 --on 3 -> DEPEND: 4 blocked by 3 (exit 0)
  get 4 -> blocked_by:[3] is_blocked:False
  itr doctor -> PROBLEM: [done_blocker] Done/wontfix issue 3 still blocks issue 4
  ```
- **Fix.** Emit a REVIEW note and skip the edge, which is the soft fallback. Or, if edges on terminal issues are meant to be allowed, make doctor and `blocked_by` agree.

---

### SQ-11 · P3 · Dangerous-SQL `changes` counts changes made inside triggers

- **Where:** `ui.rs:789-797,839-841`, which uses `SELECT total_changes()` deltas. `total_changes` includes rows changed by trigger programs: the `updated_at` touch and the FTS delete and insert.
- **Repro** (`c/`, a normal DB with triggers):
  ```
  UPDATE issues SET priority='high' WHERE id=1  -> "changes": 2
  UPDATE issues SET title='A renamed' WHERE id=1 -> "changes": 12
  ```
- **Fix.** Use `conn.changes()` (`sqlite3_changes`, top-level only) per statement and sum across statements. Fold this into the #247 rework.

---

### SQ-12 · P3 · Multi-statement mutations that are not atomic, or that read before their transaction starts

- **`assign.rs:8-15,18-30`.** Three autocommit writes: event, `UPDATE assigned_to`, then a note. The `old_issue` read happens outside any transaction. A mid-sequence failure leaves partial state behind.
- **`next.rs:181-183` (`claim_by_id`, in-progress and unassigned).** `record_event` followed by `update_issue_field` in autocommit, without a CAS. Two agents can both be told "recorded assignment to X", and the last write wins.
- **`close.rs:67-70`.** Single-ID `--duplicate-of` commits `add_relation` and its event **before** the close transaction. If the close then fails (for example SQ-1's BUSY), an orphan duplicate relation is left behind.
- **`update.rs:151` vs `:154`.** `old_issue` is read before `unchecked_transaction()`, so status, priority, title and other events can record a stale `old_value` under concurrency. List fields are re-read inside the transaction; scalars are not.
- **`db.rs:762` (`claim_issue` CAS).** The guard is only `AND status='open'`. An issue that becomes blocked between `list_issues` and the claim (`next.rs:40-65`) is still claimed. Add `AND NOT EXISTS (SELECT 1 FROM dependencies d JOIN issues b ON b.id=d.blocker_id WHERE d.blocked_id=issues.id AND b.status NOT IN ('done','wontfix'))` for the claim-next path.
- **Fix.** Use one IMMEDIATE transaction per command (see SQ-1), and read `old_*` values inside it.

---

### SQ-13 · P3 · The `updated_at` trigger has no change guard, and non-row mutations never touch `updated_at`

- **Where:** `db.rs:86-92`.
  ```sql
  AFTER UPDATE ON issues FOR EACH ROW BEGIN UPDATE issues SET updated_at=... WHERE id=OLD.id; END
  ```
- **What is correct.** The trigger is not recursive, because `recursive_triggers` is off by default. Its inner UPDATE does not fire `issues_fts_au`, because that trigger is limited to `UPDATE OF title, ...`.
- **Defect 1.** It fires on no-op updates. The UI autosaves on every field blur (`app.js:242-251`), and `patch_string_field` / `patch_array_field` (`ui.rs:1047-1080`) write and record an event unconditionally. Tabbing through the form therefore bumps `updated_at` and writes events for fields that did not change.
- **Defect 2.** Adding notes, dependencies or relations never bumps `updated_at`. `doctor`'s `stale_in_progress` (`doctor.rs:264-277`, `julianday(updated_at)`) can therefore flag an actively-noted issue as stale.
- **Fix.** Either skip unchanged fields in the UI patch helpers, or add `WHEN OLD.title IS NOT NEW.title OR ...` to the trigger. Decide whether note, dependency and relation writes should touch the parent row.

---

### SQ-14 · P3 · Redundant index

- `idx_dependencies_blocker ON dependencies(blocker_id)` (`db.rs:79`) duplicates the leading column of the `PRIMARY KEY (blocker_id, blocked_id)` autoindex.
- It is harmless but adds write cost. Drop it in a future migration; once #241 is fixed, that migration would actually reach existing databases.

---

## 3. Items checked and found correct

- **Injection.** The only `format!`-interpolated identifiers are:
  - `update_issue_field` (`db.rs:710`), checked against the closed `VALID_COLUMNS` allowlist
  - `append_in_clause` column names, which are all string literals at the call sites
  - `PRAGMA user_version = {SCHEMA_VERSION}`, a constant

  Every user value is bound as a parameter.
- **LIKE escaping.** `escape_like` escapes `\`, `%` and `_`, and every LIKE carries `ESCAPE '\'` (`db.rs:1149,1205`). Unit tests exist.
- **FTS5 MATCH.** Each whitespace term is wrapped in double quotes with internal `"` doubled, then joined with `AND`. Operators, column filters and `*` cannot be injected. A term with no tokens (for example `%`) yields an empty phrase and zero rows, not an error, and the code falls back to LIKE.
- **IN-clause builder.** Every caller guards the empty case: status falls back to `open, in-progress`, and priorities and kinds are skipped when empty. Placeholder numbering (`param_values.len()+i+1`, and `base+1..8` in search) is correct.
- **JSON-array filters.** `list`/`next`/`ready`/`bulk` tag and skill filters are exact element matches in Rust, so tag `ui` does not match `build-ui`. LIKE on raw JSON happens only in search; see #275 for the punctuation case.
- **NULL semantics.** There is no `= NULL`. `remove_relation` uses `(?3 IS NULL OR relation_type = ?3)` correctly. `parent_id` is NULL-able (FK `ON DELETE SET NULL`), `assigned_to` is `NOT NULL DEFAULT ''` in both SCHEMA and the migration, and events record `""` for "none". The two representations are consistent.
- **CHECK constraints vs `normalize.rs`.** Status, priority and kind vocabularies match exactly in both directions. `relation_type` matches `relate.rs::validate_relation_type` and `ui.rs::validate_relation_type`. `dependencies` has `CHECK (blocker_id != blocked_id)`.
- **FK and cascade.** Notes, events, dependencies and relations all use `ON DELETE CASCADE`. `import.rs` uses UPDATE rather than REPLACE so the cascade does not fire. Import runs with `defer_foreign_keys=ON`.
- **FTS triggers.** The `contentless_delete=1` delete-then-insert pattern is idempotent. `fts_index_issue` (space-joined arrays) and the triggers (raw JSON) produce identical unicode61 tokens.
- **Division by zero.** `stats` `avg_urgency` and `summary` `completion_pct` are both guarded.
- **Blocked semantics.** Blocked status is computed only in `db::is_blocked`, `blocks_active_issues` and `get_newly_unblocked`. All three exclude done/wontfix blockers, so a done or wontfix blocker blocks nothing anywhere. The inconsistencies are only in the aggregates (SQ-3, SQ-8) and in which paths clean up edges (SQ-4, SQ-10).

---

## 4. Inventory: `row_to_*` readers and the SELECTs that feed them

Reader column order:

- **`row_to_issue`** (`db.rs:529`): id, title, status, priority, kind, context, files, tags, skills, acceptance, parent_id, close_reason, created_at, updated_at, assigned_to
- **`row_to_note`** (`db.rs:549`): id, issue_id, content, agent, created_at
- **`row_to_event`** (`db.rs:559`): id, issue_id, field, old_value, new_value, agent, created_at
- **`row_to_relation`** (`db.rs:571`): id, source_id, target_id, relation_type, created_at

| Reader | Caller | SELECT site | Columns selected | Verdict |
|---|---|---|---|---|
| row_to_issue | `get_issue` | db.rs:486 | id…assigned_to (15, exact order) | ✅ |
| row_to_issue | `list_issues` | db.rs:586 | same 15 | ✅ |
| row_to_issue | `all_issues` | db.rs:1279 | same 15 | ✅ |
| row_to_note | `add_note` | db.rs:1054 | id, issue_id, content, agent, created_at | ✅ |
| row_to_note | `get_notes` | db.rs:1063 | same | ✅ |
| row_to_note | `get_note` | db.rs:1073 | same | ✅ |
| row_to_note | `all_notes` | db.rs:1299 | same | ✅ |
| row_to_event | `get_events_for_issue` | db.rs:1326 | id, issue_id, field, old_value, new_value, agent, created_at | ✅ |
| row_to_event | `get_recent_events` (since) | db.rs:1343 | same | ✅ |
| row_to_event | `get_recent_events` | db.rs:1350 | same | ✅ |
| row_to_event | `get_events_filtered` | db.rs:1381 | same | ✅ |
| row_to_relation | `remove_relation` | db.rs:1482 | id, source_id, target_id, relation_type, created_at | ✅ |
| row_to_relation | `get_relations` | db.rs:1510 | same | ✅ |
| row_to_relation | `all_relations` | db.rs:1522 | same | ✅ |

Ad-hoc positional mappers and bound-parameter lists:

| Site | Columns → mapping | Verdict |
|---|---|---|
| `claim_issue` db.rs:752 | status, assigned_to → (0, 1) | ✅ |
| `get_newly_unblocked` db.rs:1006 | i.id, i.title → (0, 1) | ✅ |
| `config_list` db.rs:1260 | key, value | ✅ |
| `doctor` find_orphaned_deps, find_done_blockers (:223, :293) | blocker_id, blocked_id | ✅ |
| `doctor` find_stuck_in_progress :265 | id, title, days → (0, 1, 2) | ✅ |
| `doctor` find_empty_epics :280 | id, title | ✅ |
| `import` insert_issue_row :230 | 15 columns; parent_id bound as `?15`, close_reason `?11`, created_at `?12`, updated_at `?13`, assigned_to `?14`, matching the params! order | ✅ |
| `import` replace_issue_row :265 | same numbering | ✅ |
| `insert_issue` db.rs:473 | 10 columns ↔ ?1..?10 in order | ✅ |

**No column-order mismatches were found.**

---

## Repro workspace

Everything is under `/private/tmp/claude-501/-Users-josefaguilar-AI-Projects-itr/503a22a5-a49b-4aee-8a77-faa00e2c47c9/scratchpad/repro-sql/`:

- `a/`: search, #275, UI stats, UI PATCH
- `b/`: #241, #247, case filters, ordering
- `c/`: aggregates, `--since`, done blocker, `changes`
- `conc/`: concurrency, SQ-1 and SQ-2

All UI servers started during the review have been stopped.
