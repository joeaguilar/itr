# itr data-integrity review: create/mutate paths (read-only)

- Code: `/Users/josefaguilar/AI_Projects/itr` @ HEAD `d017daa` (binary on PATH reports `v3.3.0-2-g2e96f5d`; `d017daa` only syncs the manifest version).
- Repro DB: `scratchpad/repro-add/.itr.db` (fresh `itr init`). Race repro: `scratchpad/repro-race/`. I did not touch the repo's `.itr.db`. I read tracker text from a byte copy (`scratchpad/repro-copy.db`, opened with `sqlite3 -readonly`).
- UI evidence comes from live HTTP calls against `itr ui --no-open --port 48321` in the repro dir (driver scripts `repro-add/ui.py`, `t1.py`, `t2.py`). I killed the server afterwards.
- Scope: add (argv + `--stdin-json`), batch add/update/close/note, update, note, depend, relate, bulk close/update/note/depend/relate, the UI JSON API (create/PATCH/close/notes/deps/relations/bulk-resolve), the db.rs insert/update helpers, and normalize.rs/util.rs.

---

## Part 1: Known tracker issues verified

| # | Verdict | Evidence |
|---|---|---|
| **#222** update clobbers priority/kind | **STILL-OPEN** | `update.rs:183-189, 199-205`. `itr add "enum test" -p critical -k bug`, then `itr update 13 --priority bogus --kind nope` gives `medium task ['_needs_review']` with notes "defaulted to 'medium'/'task'". The same input through `batch update` on #14 keeps `critical\|bug` (notes "kept 'critical'"). |
| **#224** skill filter case | **STILL-OPEN** | Stored `["rust"]`. `list --skill Rust`, `next --skill Rust`, `ready --skill Rust`, `search skill --skill Rust` and `bulk note --skill Rust --dry-run` all return 0 matches. The same commands with `rust` match. Cause: `db.rs:653-661` compares raw values, and `bulk.rs:43` passes `skill` through without lowercasing. |
| **#228** UI PATCH reopens closed | **STILL-OPEN** | `ui.rs:983-1009`. Issue done/high/bug + `PATCH {"status":"bogus","priority":"nope","kind":"zzz"}` returns HTTP 200 and the row becomes `open\|medium\|task`, with no notes and no `_needs_review`. |
| **#229** add_dependency race | **STILL-OPEN, reproduced** | `db.rs:853-903` does check-then-insert under autocommit (single-ID `depend.rs:112`, UI `ui.rs:637`). In `repro-race/race.py` (150 rounds of concurrent `itr depend A --on B` and `itr depend B --on A`), **24/150 rounds left a 2-cycle A↔B in `dependencies`**. |
| **#237** add vs batch add drift | **STILL-OPEN** | `echo '{"title":"sj miss blk","blocked_by":[9999]}' \| itr add --stdin-json` gives `ERROR: Issue 9999 not found` and 0 rows. `batch add` with the same object gives outcome `review`, "blocked_by 9999 not found; dependency skipped", and the issue is created. The parity table (Part 3) shows more drift. |
| **#255** non-atomic note / UI create | **STILL-OPEN, and wider than filed** | `db.rs:1036-1059`: `add_note` runs the INSERT, then `record_event`. Single `itr note` (`note.rs:135`) and UI notes (`ui.rs:602-606`) call it without a tx. UI create: `POST {"title":"ui missing blocker","priority":"bogus","blocked_by":[9999]}` returns HTTP 404, yet issue #18 was committed with `_needs_review` and its REVIEW notes. **Also non-atomic (not named in #255):** single-ID `depend` and `relate` (INSERT, then event, as separate autocommits); `note update` (`note.rs:176-184`: event, then update) and `note delete` (`note.rs:150-153`: delete, then event); UI `PATCH/DELETE /api/notes/:id` (`ui.rs:608-632`); UI `POST .../dependencies` and `.../relations`. |
| **#258** UI create missing parent | **STILL-OPEN** | `POST {"title":"ui orphan","parent_id":9999}` returns HTTP 500 `Database error: FOREIGN KEY constraint failed`, `DB_ERROR`. |
| **#270** UI empty blocked-by | **STILL-OPEN** | `app.js:68-70` `parseIds("")` returns `[0]` (`Number("")===0`). `POST {"title":"ui blocked 0","blocked_by":[0]}` returns HTTP 404 "Issue 0 not found", **and issue #17 is committed anyway** (orphaned by #255). So every default-form UI create returns an error and still writes the issue. |
| #235 (batch update notes lack `REVIEW:`) | STILL-OPEN (seen incidentally) | Notes on #14 read `priority 'bogus' not recognized, kept 'critical'...` with no prefix. |
| #274 (UI status dropdown leaves blocker edges) | STILL-OPEN (seen incidentally) | After `PATCH {"status":"done"}` on a blocker, the `dependencies` row `(26,27)` remains. |
| #233 (single-ID self relate/depend) | PARTIAL | `itr relate 1 --to 1` is now rejected at the db layer (`db.rs:1424`). `itr depend 1 --on 1` still reports `Cycle detected: 1 -> ... -> 1` where the multi path emits a REVIEW skip. |

---

## Part 2: New findings

### AW-1 P2: Empty and whitespace-only titles are accepted everywhere except UI create
- **Where:** `add.rs:200-204` (rejects only a *missing* title), `add.rs:144-146`, `batch.rs:251-253` (raw title), `update.rs:208-211`, `batch.rs:596-599`, `ui.rs:956`/`1047-1060` (PATCH). Only `ui.rs:886-893` trims and rejects.
- **Repro:**
  - `itr add ""` creates id 1 with `title=''`.
  - `itr add "   "` creates id 2 with `'   '`.
  - `itr add --title ""` creates id 12.
  - `echo '{"title":""}' | itr add --stdin-json` creates id 4.
  - `batch add [{"title":""},{"title":"   "}]` returns ok/ok (ids 6, 7).
  - `itr update 11 --title ""` stores `''`.
  - `batch update {"title":"   "}` stores `'   '`.
  - UI `PATCH {"title":""}` returns 200 and stores `''`.
  - UI `POST {"title":"   "}` returns **400**.
  - Titles are never trimmed on CLI or JSON paths (`"  padded  "` is stored verbatim) but are trimmed on UI create. There is no length cap: a 100,000-char title via batch add returns `ok`.
- **Fix sketch:** add a shared `normalize_title(&str) -> Result<String, Note>` that trims. Empty after trim becomes the one hard `InvalidValue` on create. On update/PATCH, either reject or keep the current title with a REVIEW note, the same across all 6 paths. Consider a length cap with a REVIEW note.

### AW-2 P2: files/tags/skills element normalization differs in four ways
- **Where:** `util.rs:16-21` (`parse_comma_list`: trim, drop empties, **no dedupe**); `add.rs:77-78` and `batch.rs:240,257` (JSON `files`/`tags` stored raw); `util.rs:71-79` `apply_tags` (no trim, no empty filter; also used for files at `update.rs:255`); `batch.rs:616`; `bulk.rs:211-220` (raw `--add-tag`); `ui.rs:1082-1095` `clean_list` (trim, drop empties, dedupe).
- **Repro:** the same logical input goes through each path.
  - CLI `add --tags " A ,A,,a" --tag " " --skills "Rust, rust" --files "x.rs,x.rs"` gives tags `['A','A','a']`, skills `['rust','rust']`, files `['x.rs','x.rs']` (**duplicates kept**).
  - `add --stdin-json` / `batch add` with `tags [" A ","A","",""]`, `files [" x.rs ","x.rs",""]` gives tags `[' A ','A','','']` and files `[' x.rs ','x.rs','']` (**untrimmed, empty strings stored**). Skills give `['rust','rust']`.
  - UI create with the same payload gives tags `['A']`, skills `['rust']`, files `['a.rs']`.
  - `update --add-tag " "` appends `' '`. `batch update add_tags ["  z  "]` stores `'  z  '`. `itr bulk update --tag bulkme --add-tag ""` stores `["bulkme",""]`.
- **Impact:** empty and whitespace tags are unfilterable junk. `--tag A` misses `' A '`. Duplicate skills inflate `stats`.
- **Fix sketch:** add one `clean_list(values, lowercase)` in `util.rs` (move `ui.rs:1082`) and call it from every writer: `insert_issue` callers, `apply_tags`/`apply_skills` inputs, and bulk `--add-tag`. The REVIEW note on dropped empties is optional.

### AW-3 P2: Closing an already-closed issue differs by path (overwrite vs no-op)
- **Where:** `close.rs:215-235`, `bulk.rs:83-90`, `ui.rs:1097-1113` (always rewrite status and record an event) vs `batch.rs:432-445` (returns "Already <status>" and does nothing).
- **Repro:** `itr close 41`, then `itr close 41 --wontfix` changes status to `wontfix`. Then `echo '[{"id":41,"wontfix":true}]' \| itr batch close` returns `notes:["Already wontfix"]`. UI: two POSTs to `/close` (the second with `wontfix:true`) record events `open→done` and `done→wontfix`. `itr bulk close --status done` records `done→done` for every match.
- **Fix sketch:** pick one contract, preferably batch's idempotent "Already X" plus a REVIEW note when the requested terminal state differs, and put it in a shared `close_issue` used by all four paths.

### AW-4 P2: UI bulk resolve apply is not atomic
- **Where:** `ui.rs:551-566`. Each id is resolved in its own tx via `resolve_issue`, and `?` aborts mid-loop.
- **Repro:** `POST /api/bulk/resolve/apply {"ids":[<z1>, 99999]}` returns HTTP 404 "Issue 99999 not found", but `z1` is already `done`. The UI shows an error toast for a partially applied bulk action.
- **Fix sketch:** wrap the loop in a single tx and roll back on error, or match the CLI `close_many` soft fallback (skip missing ids with REVIEW notes and report per-id outcomes).

### AW-5 P2: A non-numeric parent in the UI detail field silently clears the parent
- **Where:** `app.js:245-247` (`Number(value)` gives `NaN`, and `JSON.stringify(NaN)` gives `null`) with `ui.rs:1015-1039` (`null` means clear, and an event is recorded).
- **Repro:** the child has parent 24. Typing `abc` or `#24` in the Parent field and blurring sends `{"parent_id":null}`. Evidence: I sent `PATCH {"parent_id":null}` directly, which JSON-equals what the blur sends, and parent became `None` with HTTP 200. The JS path is read from code.
- **Fix sketch:** in JS, send nothing (or show a toast) when `!Number.isInteger(n)`. On the server, accept numeric strings and fall back to "unchanged + REVIEW" for anything else.

### AW-6 P2: UI create silently drops unknown keys and the `parent` alias, and rejects string `blocked_by`
- **Where:** `ui.rs:52-76` `IssueCreateInput` (no `alias = "parent"`, no unknown-key scan, `blocked_by: Vec<i64>`). Compare `batch.rs:20-33,112-119` and `models.rs:129`.
- **Repro:**
  - `POST {"title":"ui alias","parent":1,"priorty":"high"}` returns 200 with `parent_id: None`, `priority: medium` and `tags: []`: no REVIEW, no `_needs_review`. The same object through `add --stdin-json` links parent 1 and adds a REVIEW note naming `priorty`.
  - `POST {"title":"ui str","blocked_by":["1"]}` returns 400 PARSE_ERROR, while stdin-json and batch accept `"1"`.
- **Impact:** this breaks the "never silently swallow input" rule and the #150/#165/#212 parity.
- **Fix sketch:** have UI create deserialize via `parse_add_item` and call `add::execute` (this also fixes #255, #258 and #270; see #236/#237).

### AW-7 P3: Batch add hard-aborts the whole batch on a trivially recoverable self or intra-batch `@N` reference
- **Where:** `batch.rs:280-305`, which feeds `db::add_dependency`, which returns `CycleDetected`, which propagates `Err(e) => return Err(e)`.
- **Repro:**
  - `[{"title":"self","blocked_by":["@0"]}]` gives `ERROR: Cycle detected: 39 -> ... -> 39` and 0 rows.
  - `[{"title":"c1","blocked_by":["@1"]},{"title":"c2","blocked_by":["@0"]}]` gives `ERROR: Cycle detected: 40 -> ... -> 39` and 0 rows. The error cites IDs that were rolled back and never exist.
  - For comparison, `depend 5,6 --on 5` and `bulk depend` skip the self-edge with a REVIEW note.
- **Fix sketch:** skip `@idx == own idx` with a REVIEW note (it is the same class as the multi-depend self skip). Keep real cycles hard, but report them by batch index rather than phantom IDs.

### AW-8 P3: Empty note text is accepted by 3 of 4 note paths, and error text contradicts behavior
- **Where:** `note.rs:127-135` (only `None` is rejected), `batch.rs:747`, `ui.rs:602-606`; `bulk.rs:418` rejects `""` but accepts `"   "`. The UI note also skips the `ITR_AGENT` fallback (`ui.rs:604` vs `note.rs:9-15`).
- **Repro:** `itr note 1 ""` creates NOTE:16 with content `''`. `batch note [{"id":1,"text":""}]` returns `ok`. UI `POST notes {"content":""}` returns 200. `itr bulk note "" --status open` returns `ERROR ... Valid: non-empty string`. `itr note 1 "   "` creates NOTE:18 `'   '`.
- **Fix sketch:** add a shared `validate_note_text` (trim, reject empty with the existing InvalidValue) in all four paths, and have the UI use `resolve_agent`.

### AW-9 P3: Reopening leaves a stale `close_reason`
- **Where:** `update.rs:158-164`, `batch.rs:544-550`, `bulk.rs:203-205`, `ui.rs:983-991`. None of these touch `close_reason` on a non-terminal status.
- **Repro:** `itr close 41 "reason1"`, then `itr update 41 --status open` shows `open 'reason1'`. UI PATCH `{"status":"open"}` on a closed issue keeps `close_reason='shipped'`. UI PATCH can also set `close_reason` on an open issue.
- **Fix sketch:** on a transition to open or in-progress, clear `close_reason` (recording an event), or move it into a note. Apply the same rule on every path.

### AW-10 P3: Enum normalizers do not trim or accept spaced forms
- **Where:** `normalize.rs:35-43, 62-70, 90-99`.
- **Repro:** `itr add "ws enum" -p " high " -k "Bug "` stores `medium task` with REVIEW notes. `itr update N --status "in progress"` keeps `open` with a REVIEW note. The same applies to every path that uses these functions (batch, bulk, UI, filters).
- **Fix sketch:** call `.trim()` before `to_lowercase()`, and add `"in progress"` to the status aliases.

### AW-11 P3: Relation type has no normalization, and symmetric links are stored twice
- **Where:** `relate.rs:8-17` (and its duplicate at `ui.rs:1492-1503`, #260); `db.rs:1418-1450`.
- **Repro:** `itr relate 1 --to 2 --type Duplicate` and `--type relates` are both hard `INVALID_VALUE`, whereas priority/kind/status accept case and synonyms with soft fallback. `itr relate 1 --to 2 --type related` followed by `itr relate 2 --to 1 --type related` stores 2 rows for one symmetric fact. A↔B `supersedes` in both directions (a contradiction) is also accepted.
- **Fix sketch:** add `normalize_relation_type` (lowercase; `dup`/`duplicates` map to `duplicate`, `relates`/`relates-to` to `related`) with the default-plus-REVIEW pattern. For `related`/`duplicate`, check the reverse pair before insert and return `exists`. Reject or warn on a reverse `supersedes`.

### AW-12 P3: `assigned_to`, `context` and `acceptance` are never trimmed, so padded assignees are unfilterable
- **Where:** `add.rs:247-253`, `batch.rs:262,608-611`, `update.rs:216-223`, `ui.rs:936-938,966-973`. UI create trims the title but not `assigned_to`.
- **Repro:** `itr add ... --assigned-to "  bob "`, `add --stdin-json`, `batch add` and UI create all store `'  bob '` (4 rows). `itr list --assigned-to bob` returns `[]`. `--acceptance "  "` is stored as `'  '`.
- **Fix sketch:** trim `assigned_to` on every write, since it is an identifier compared by equality. Whitespace-only `acceptance`/`context` can become `""`.

### AW-13 P3: `updated_at` ignores notes, deps and relations but bumps on no-op updates
- **Where:** the trigger `db.rs:~92-97` fires only on `UPDATE issues`. `add_note`, `add_dependency` and `add_relation` never touch `issues`. `update.rs:161,179,209`, `batch.rs:548-610` and `ui.rs:1056,1077,1029` record events and write even when old equals new.
- **Repro:**
  - Created 41, 42, 43 at 19:26:06. After 2 s: `itr note 41`, `itr depend 42 --on 43`, `itr relate 43 --to 41`. `updated_at` is unchanged on all three.
  - `itr update 41 --priority medium --status open --title ua` (all no-ops) records events `status open→open`, `priority medium→medium`, `title ua→ua` and bumps `updated_at`.
  - A UI no-op PATCH `{"status":"open","priority":"medium"}` adds 2 events.
- **Impact:** `list --sort updated` and staleness views misrank. The audit log fills with phantom changes.
- **Fix sketch:** skip the write and event when the value is unchanged (the `persist_list_field` already does this for lists; generalize it). Decide whether note, dep and relation activity should touch `updated_at` and apply that in the db helpers.

### AW-14 P3: Control characters, including NUL, are stored in titles and emitted raw
- **Where:** there is no sanitization on any writer (`db::insert_issue`, `update_issue_field`). The compact formatter escapes `\n`/`\t` (#156) but not other C0 bytes.
- **Repro:**
  - `itr add $'line1\nline2\ttab\x01ctl'` stores hex `...0A...09...01...`. `itr list | cat -v` prints `TITLE: line1\nline2\ttab^Actl`: the raw `0x01` reaches the terminal, and so would ESC (`0x1b`) sequences.
  - `batch add [{"title":"a\u0000b"}]` and `add --stdin-json` both store a 3-byte title. SQLite `length(title)=1`, because TEXT functions stop at NUL, while the JSON output shows `"a\u0000b"`.
- **Fix sketch:** reject NUL. Replace or strip other C0 controls except `\n`/`\t` in title, and escape them in compact/oneline output, with a REVIEW note on write.

### AW-15 P3: Comma-bearing tags/skills round-trip badly (singular flags do not split, UI does)
- **Where:** `add.rs:218-232` and `update.rs:270-274, 300-305` (`--tag`/`--skill` are not comma-split), JSON array paths, `bulk.rs:211`; `app.js:64,250` (`parseList` splits on `,` on every tags/skills blur).
- **Repro:**
  - `itr add "comma tag" --tag "x,y"` stores tags `['x,y']`, and `list --tag x` gives `[]`.
  - `itr add "skill comma" --skill "Go,Rust"` stores skills `['go,rust']`, which never matches `--skill go`.
  - Opening that issue in the UI and blurring the Tags field rewrites it as `['x','y']`: a silent mutation.
  - `bulk update --add-tag " x,y "` stores the literal `' x,y '`.
- **Fix sketch:** either split singular flags and JSON elements on `,` like the plural flags, or reject commas in elements with a REVIEW note. Do it in the shared `clean_list` from AW-2.

### AW-16 P3: `blocked_by`/`parent` token parsing is inconsistent inside the JSON paths
- **Where:** `batch.rs:92-105` (numeric strings are `trim()`ed, but `@N` is not); `models.rs:129` (`parent_id: Option<i64>`).
- **Repro:** `[{"title":"a0"},{"title":"a1","blocked_by":[" @0","@ 0"]}]` rejects both with REVIEW notes, while `" 5"` would be accepted. `{"title":"sj p str","parent_id":"1"}` through `add --stdin-json` gives a hard `JSON parse error: invalid type: string "1"`, while `blocked_by:["1"]` in the same payload is accepted. `blocked_by:[1.5]` becomes a REVIEW note, as expected.
- **Fix sketch:** trim before stripping `@`. Accept numeric-string `parent_id` through a custom deserializer, and fall back to parentless with a REVIEW note on garbage, as for a missing parent.

### AW-17 P3: UI PATCH silently ignores wrong-typed values
- **Where:** `ui.rs:1055` (`and_then(Value::as_str)`), `ui.rs:983,992,1001`.
- **Repro:** `PATCH {"title":null,"context":123}` returns HTTP 200, and nothing changes: no error, no REVIEW. `{"status":5}` is ignored the same way.
- **Fix sketch:** return per-field REVIEW notes, or a 400 listing the fields that were not applied, and report unknown keys too (as #212 does for batch).

---

## Part 3: Creation-path parity table

`add` = argv form. `sj` = `add --stdin-json`. `batch` = `batch add`. `UI` = `POST /api/issues`. All results were observed in the repro DB unless marked (code).

| Input | add (argv) | add --stdin-json | batch add | UI create |
|---|---|---|---|---|
| title `""` | **created** `''` | **created** `''` | **created** (ok) | 400 INVALID_VALUE |
| title `"   "` | created `'   '` | created (code-identical) | created `'   '` | 400 |
| title `"  padded  "` | stored untrimmed (code) | stored `'  padded  '` | untrimmed | **trimmed** `'ui pad'` |
| title with `\x01` / NUL | ctrl stored; NUL impossible via argv | NUL stored | NUL stored | (code) stored after trim |
| priority `" high "` | medium + REVIEW | medium + REVIEW (shared `execute`) | medium + REVIEW (code) | medium + REVIEW (code) |
| priority `bogus` | medium + REVIEW | medium + REVIEW | medium + REVIEW | medium + REVIEW (tag + note) |
| tags `[" A ","A","",""]` | (`--tags " A ,A,,"`) `['A','A']`, trimmed, not deduped | `[' A ','A','','']` raw | raw | `['A']` trimmed and deduped |
| skills `["Rust"," rust "]` | `['rust','rust']` | `['rust','rust']` | `['rust','rust']` | `['rust']` |
| files `[" x.rs ","x.rs",""]` | trimmed, dup kept | raw | raw | trimmed and deduped |
| assigned_to `"  bob "` | raw | raw | raw | raw |
| parent 9999 | parentless + REVIEW | parentless + REVIEW | parentless + REVIEW | **500 FK error** (#258) |
| key `parent` (alias) | n/a (`--parent`) | accepted | accepted | **silently dropped** |
| unknown key `priorty` | n/a | REVIEW + `_needs_review` | REVIEW | **silently dropped** |
| blocked_by 9999 | **hard error, rollback** | **hard error, rollback** | **created + REVIEW, edge skipped** (#237) | **404, issue and notes persisted** (#255) |
| blocked_by `"1"` (string) | parsed | accepted | accepted | **400 PARSE_ERROR** |
| blocked_by `junk` | REVIEW, ignored | REVIEW, ignored | REVIEW, skipped | 400 (type) / JS drops NaN silently |
| blocked_by empty field | `--blocked-by ""` gives `[]` | `[]` | `[]` | JS sends `[0]`: **404 + orphan issue** (#270) |
| blocked_by `0` / `-5` | hard NotFound | hard NotFound (code) | REVIEW skip (code) | 404 + orphan |
| blocked_by `@own` | n/a | REVIEW "only valid in batch" | **whole batch aborted** (AW-7) | n/a |
| atomic on failure | yes (one tx) | yes | yes (one tx, per-item soft) | **no** (autocommit per statement) |

Update-path mini-parity (unrecognized values):

| Input | update | batch update | bulk update | UI PATCH |
|---|---|---|---|---|
| priority `bogus` | **clobber → medium** (#222) | keep + note (no `REVIEW:`, #235) | keep + REVIEW | **clobber → medium, no note** |
| status `bogus` | keep + REVIEW | keep + note | keep + REVIEW | **→ open, no note** (#228) |
| title `""` | stored | stored | n/a | stored |
| status → done | edges cleaned | edges cleaned | edges cleaned | **edges kept** (#274) |
| no-op value | event + updated_at bump | event + bump | event + bump | event + bump |

---

## Areas checked with no defect found
- JSON-array columns are only ever written through `serde_json::to_string(&Vec<String>)` or `clean_list`, so no in-scope path writes a non-array or unparseable JSON (the `parse_json_array` silent-`[]` hazard is #242 and needs external corruption or dangerous SQL).
- The SQL CHECKs on status/priority/kind/relation_type are never the first line of defense on the scoped paths: every writer validates first. Columns without a CHECK (`close_reason`, `assigned_to`, `context`, `acceptance`, `title`) are free text by design, and their gaps are covered by AW-1, AW-9, AW-12 and AW-14.
- Parent cycles: `update_issue_parent` (`db.rs:789-820`) guards every caller, and batch update soft-falls-back.
- FTS stays in sync through triggers, plus `fts_index_issue` with a REVIEW note on failure.
- `update`, `batch *`, `bulk *` (except UI bulk apply) and UI PATCH/close each run in one transaction.
