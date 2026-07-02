# Command Contracts

This document records stable command behavior for contributors. It is not clap
help. Keep stdout machine-consumable, preserve aliases, and treat stderr as the
place for diagnostics, hints, warnings, review notes, and progress.

For the wider context of how these commands hang together, see
[architecture.md](architecture.md) — module shape, DB discovery, and the
embedded UI server. For the philosophy behind the recoverable-input behavior
documented under **Soft Fallbacks** below, see
[soft_fallbacks.md](soft_fallbacks.md), which explains why `itr` defaults,
warns, and continues instead of hard-failing on normalizable input.

## Global Contract

All commands accept the global parser flags from `src/cli.rs`:

- `-f, --format`: `compact`, `json`, `pretty`, or `oneline`. Values are
  case-insensitive and surrounding whitespace is trimmed (issue #192), so
  `-f JSON` works. Default is `compact`. Unknown formats exit before handler
  dispatch.
- `--db`: database path override. `ITR_DB_PATH` takes precedence over `--db`
  for every command except `init`, which inverts the order — see
  [environment.md](environment.md#itr_db_path) for the full precedence rules.
- `--fields`: comma-separated field selector. It is stable for issue, list,
  search, and batch JSON outputs; for `stats`, `graph`, and `log` JSON outputs
  (top-level key filtering, issue #197); and for issue/list/search compact and
  list pretty/oneline outputs. On issue-list output the requested order is
  honored: oneline emits the selected fields tab-separated in the given order
  (list values join with `,`), pretty builds its columns from the given order,
  and compact orders fields within its record-line/labeled-line structure. A
  command/format combination with no field filtering (issue-detail pretty,
  search pretty/oneline, and the non-JSON modes of `stats`, `graph`, `log`,
  and `batch`) emits a `REVIEW:` note to stderr and prints the unfiltered
  output instead of silently swallowing the flag. When the filter is applied
  to JSON output, the surviving keys re-serialize in the requested `--fields`
  order (see **JSON Determinism And Snapshotting**).
- `-q, --quiet`: accepted globally for compatibility. Do not rely on it to
  change parseable stdout in current command contracts.

Commands with no database requirement: `init`, `agent-info`,
`getting-started`, `skill`, `schema`, and `upgrade`. Other commands open the
resolved SQLite database before dispatch.

## Stdout And Stderr

- Successful command data goes to stdout.
- Runtime errors go to stderr. In JSON mode, runtime errors are JSON objects
  with `error` and `code`; otherwise they are `ERROR: ...`.
- Soft-fallback review messages, hints, and progress go to stderr and should
  not corrupt stdout.
- Argument parse errors are clap errors and exit before command handlers.
- `show --all` may emit a hint to stderr before normal list output.
- `upgrade` progress is stderr in non-JSON mode.
- `ui` browser-open failures are `REVIEW:` messages on stderr.

## Exit Contract

- Success exits 0.
- Empty result sets are not errors and exit 0.
- Handler/runtime errors exit 1: not found, no database, parse errors,
  invalid hard-validation errors, dependency cycles, DB/IO errors, no filters
  for bulk operations, and failed upgrades.
- Clap parse errors use clap's exit behavior.
- Batch `add`, `close`, `update`, and `note` represent per-item failures
  (including malformed array items) in the batch result envelope and still
  exit 0 unless the top-level stdin payload fails to parse as a JSON array or
  a non-item handler error fails.
- `doctor` prints its report, then exits 1 only when problems remain after
  the run. A `--fix` invocation that repairs every detected problem exits 0;
  the report still lists the detected problems and the `FIXED:` actions.
  The remaining-problems failure is a diagnostic outcome, reported on stderr
  with code `DOCTOR_PROBLEMS_REMAIN` in JSON mode (not `INVALID_VALUE`).

## Empty Results

JSON empty results use `[]` on stdout. Non-JSON empty results use a terse
message or blank formatted output, depending on the command path.

Stable empty-result messages:

- `list`, `search`: `No matching issues found.`
- `get`/`show` with multiple IDs, all missing: `No matching issues found.`
  (JSON: `[]`), exit 0 — see **Issue Detail** for the batched contract.
- `ready`: `No ready issues found.`
- `next`, `claim`, `start` without an explicit ID: `No eligible issues found.`
- `log`: `No events found.`
- `search` with no terms: `No search terms provided.`

## Soft Fallbacks

These are the per-command surfaces of the project-wide pattern documented in
[soft_fallbacks.md](soft_fallbacks.md). Preserve them as non-fatal behavior:

- Priority synonyms normalize before validation: `urgent`, `p0`, `highest` to
  `critical`; `p1` to `high`; `p2`, `normal` to `medium`; `p3`, `lowest` to
  `low`.
- Kind synonyms normalize before validation: `enhancement`, `feat`, `story` to
  `feature`; `bugfix`, `defect` to `bug`; `chore`, `subtask` to `task`.
- Status synonyms normalize before validation: `todo`, `new`, `backlog` to
  `open`; `closed`, `resolved`, `fixed` to `done`; `cancelled`, `canceled` to
  `wontfix`; `wip`, `started`, `progress`, `in_progress`, `inprogress` to
  `in-progress`.
- `add` invalid priority/kind defaults to `medium`/`task`, adds
  `_needs_review`, creates an `itr` note, and exits 0.
- `update` invalid priority/kind defaults to `medium`/`task`, adds
  `_needs_review`, creates an `itr` note, and exits 0. An invalid status keeps
  the issue's current status (it is never reset to `open` — a typo must not
  reopen a closed issue), adds `_needs_review`, creates an `itr` note, and
  exits 0, matching `batch update` (#163).
- `batch add` invalid priority/kind defaults to `medium`/`task`, marks the item
  `review`, adds `_needs_review`, and exits 0.
- `batch update` invalid status/priority/kind keeps the existing value, marks
  the item `review`, adds `_needs_review`, and exits 0.
- `add` (CLI `--parent` and `--stdin-json` `parent_id`) and `batch add` with a
  nonexistent parent create the issue parentless, add `_needs_review`, and
  record a `REVIEW:` note instead of failing with a FOREIGN KEY error (#167).
- `batch add` skips unresolvable `blocked_by` entries (missing issue IDs,
  out-of-range or failed `@N` references, unparseable tokens) with a per-item
  `REVIEW:` note, and `add --stdin-json` skips non-parseable `blocked_by`
  entries the same way (#164). CLI `add --blocked-by <missing-id>` remains a
  hard `NOT_FOUND` that rolls back the whole add.
- Unrecognized JSON keys in `add --stdin-json` and `batch add` item payloads
  emit a `REVIEW:` note naming the keys instead of being silently dropped
  (#150).
- `update` accepts the replace-form list flags (`--files`/`--file`,
  `--tags`/`--tag`, `--skills`/`--skill`) together with the add/remove-form
  flags: the replacement is applied first, then the `--add-*`/`--remove-*`
  edits on top, with a `REVIEW:` warning on stderr instead of silently
  discarding the add/remove flags (#188). stdout stays parseable.
- Batched `get`/`show` (more than one unique ID, #136): missing IDs emit one
  `REVIEW:` note each on stderr while the found issues are still returned
  (exit 0); when every ID is missing, the standard empty result is printed
  (exit 0). Duplicate IDs are fetched once with a `REVIEW:` note; non-integer
  tokens are skipped with a `REVIEW:` note. A request with no parseable ID at
  all is a hard `INVALID_VALUE`. A single-ID request keeps the hard
  `NOT_FOUND` contract.
- Multi-ID mutating verbs (`close`, `note`, `relate`, `depend`) accept the
  same ID grammar as `get`/`show` — repeated arguments, comma lists, and
  inclusive `A-B` ranges — and run all writes in one transaction with per-ID
  soft fallback: a missing ID emits `REVIEW: id <N> not found; skipped` and
  the rest proceed; exit 0 if at least one ID succeeded, exit 1 if none did.
  An ID equal to `--to`/`--on` skips the self-edge with a `REVIEW:` note.
  A reversed range (`9-5`) recovers by swapping the bounds with a `REVIEW:`
  note; a range wider than 1000 IDs is rejected as an invalid token. Single-ID
  invocations keep the historical hard-error contracts. Dependency cycles
  remain hard errors and roll the whole invocation back. `claim` deliberately
  stays single-ID.
- Unknown `--fields` names emit `REVIEW:` to stderr and are ignored if absent
  from the output shape.
- `--fields` on a command/format combination that has no field filtering emits
  `REVIEW: --fields is not supported for ...` to stderr and prints the full,
  unfiltered output (exit 0).
- `skill install` refuses to overwrite an existing skill without `--force`,
  emits `REVIEW:` to stderr, preserves the file, and exits 0.
- Hidden compatibility flags `add --title` and `close --reason` take precedence
  over the positional value when both are provided and emit a `REVIEW:` warning
  to stderr.

## Compatibility Aliases

Visible aliases from `src/cli.rs` are part of the public contract:

| Alias | Canonical command or flag |
| --- | --- |
| `itr create` | `itr add` |
| `itr deps` | `itr depend` |
| `itr getting-started` | `itr agent-info` |
| `itr getting started` | preprocessed to `itr getting-started` |
| `itr start` | `itr claim` |
| `itr current` | `itr wip` |
| `itr batch create` | `itr batch add` |
| `itr list --tags` | `itr list --tag` |
| `itr relate --type` | `itr relate --relation-type` |
| `itr unrelate --type` | `itr unrelate --relation-type` |
| `itr bulk update --filter-status` | `itr bulk update --status` |
| `itr bulk update --filter-priority` | `itr bulk update --priority` |

Accepted hidden compatibility spellings:

- `itr add --body` as an alias for `--context`.
- `itr add --title` as a flag-form title.
- `itr close --reason` as a flag-form reason.

## Output Families

### Escaping In Line-Oriented Output

Compact, oneline, and compact event output are line-oriented contracts: one
logical field never spans more than one physical line, and a double-quoted
`"…"` token never contains an unescaped quote. To guarantee that, free-text
values (title, context, acceptance, close reason, note content and agent,
tags/files/skills, assignee, event old/new values, search snippets, unblocked
titles, batch item notes/errors) are backslash-escaped before being embedded:

| Character | Encoded as |
| --- | --- |
| backslash `\` | `\\` |
| newline (LF) | `\n` |
| carriage return (CR) | `\r` |
| tab | `\t` |
| `"` (only inside double-quoted tokens) | `\"` |

- Unquoted labeled values (`TITLE:`, `CONTEXT:`, `ACCEPTANCE:`,
  `CLOSE_REASON:`, `ASSIGNED:`, `SNIPPET[…]:`, …) and tab-separated oneline
  fields escape `\`, LF, CR, and tab.
- Double-quoted tokens (oneline titles, `OLDEST_OPEN: … "…"`, `NODE:… "…"`,
  `UNBLOCKED:… "…"`, batch `NOTE:`/`ERROR:` strings, event
  `OLD:"…"`/`NEW:"…"`) additionally escape `"`.
- Parsers recover the exact original value by reversing the escapes; because
  the backslash itself is escaped, the encoding is unambiguous.
- A hostile value embedding something like `\nID:777 STATUS:open …` can never
  forge a record line: the newline renders as the two-character sequence
  `\n`, so a compact-contract parser never sees a fabricated issue.
- Graphviz DOT output (`graph -f pretty`/`-f oneline`) uses DOT's own label
  escaping instead: `\` → `\\`, `"` → `\"`, and literal newlines become the
  DOT `\n` line-break escape, so emitted DOT always parses.

The shared helpers are `format::escape_line_value`,
`format::escape_quoted_value`, and the DOT-specific `escape_dot_label` in
`src/format.rs`. New line-oriented output must reuse them rather than invent
another encoding. JSON output needs none of this — serde escaping already
covers it.

### Issue Detail

Commands: `add`, `create`, `get`, `show <ID>`, `update`, `close`, `next`,
`claim`, `start`, `assign`, `unassign`.

- JSON is an `IssueDetail`: issue fields flattened with `urgency`,
  `blocked_by`, `blocks`, `is_blocked`, `notes`, optional
  `urgency_breakdown`, optional `children`, and optional `relations`. Close and
  terminal updates may add `unblocked`. `close` and `update` round-trip the
  detail through `serde_json::Value` to append `unblocked`; with the
  `preserve_order` serde_json feature this keeps serde struct field order with
  `unblocked` appended last (see **JSON Determinism And Snapshotting**).
  Multi-ID `close` emits a JSON array of these objects.
- Compact starts with `ID:<id> STATUS:<status> PRIORITY:<priority> KIND:<kind>
  URGENCY:<score>` and optional dependency tokens, followed by stable labeled
  lines such as `TAGS:`, `FILES:`, `SKILLS:`, `ASSIGNED:`, `TITLE:`,
  `CONTEXT:`, `ACCEPTANCE:`, `PARENT:`, `CLOSE_REASON:`, `CREATED:`,
  `UPDATED:`, and optional sections. Free-text values are escaped per
  **Escaping In Line-Oriented Output**, so each labeled line is exactly one
  physical line.
- Pretty is human text headed by `Issue #<id>: <title>`.
- Oneline currently uses the compact issue-detail formatter.
- **Batched retrieval (#136).** `get` and `show` accept multiple IDs as
  repeated arguments and/or comma-separated lists (`itr get 1,2,3`,
  `itr show 1 2 3`). With more than one unique ID the output is batched:
  JSON is an **array** of `IssueDetail` objects in request order;
  compact/oneline emit the per-issue compact blocks separated by one blank
  line (each block starts with its `ID:` record line, and the line-oriented
  escaping above keeps the separator unambiguous); pretty emits the per-issue
  pretty blocks separated by one blank line. With exactly one unique ID
  (including `itr get 1,1` after dedup) the output is byte-identical to the
  single-issue contract — a bare JSON object, no array wrapper. Missing-ID
  handling for the batched form is under **Soft Fallbacks**.

### Issue Lists

Commands: `list`, `ready`, `wip`, `current`, `show` without ID.

- JSON is an array of `IssueSummary`.
- Compact is one issue block per item, separated by a blank line.
- Pretty is a table with selected columns. Without `--fields` the columns are
  the historical default set (`#`, `Urg`, `Status`, `Pri`, `Kind`, `Assignee`,
  `Title`, `Blocked`); with `--fields` the columns are built from the
  requested names in the requested order (extra columns such as `tags`,
  `files`, `skills`, `created_at`, `updated_at`, `acceptance`, `is_blocked`
  become available). Empty pretty output is empty.
- Oneline is one tab-separated row per issue:
  `id status priority kind "title"` plus optional `assigned_to`. Titles and
  assignees are escaped per **Escaping In Line-Oriented Output**, so embedded
  tabs, newlines, and quotes never change the row or field count. With
  `--fields`, the row is instead the selected fields tab-separated in the
  requested order — list values join with `,`, free text is escaped, and an
  unknown field name renders as an empty cell so the column count stays
  stable.

### Search Results

Command: `search`.

- JSON is an array of `SearchResult` with `matched_fields` and optional
  `context_snippets`.
- Compact uses issue-summary labels plus `MATCHED:` and `SNIPPET[field]:`
  lines when snippets are present. Compact honors `--fields` like the
  issue-list compact formatter (issue #197).
- Pretty and oneline currently use the same table formatter; they have no
  field filtering, so `--fields` there emits a `REVIEW:` note.

### Stats And Summary

Commands: `stats`, `summary`.

- `stats -f json` is a `Stats` object with totals, status/priority/kind maps,
  blocked/ready counts, average urgency, skill and assignee maps, and optional
  oldest-open detail. Compact, pretty, and oneline share labeled compact lines.
  - **Deterministic JSON contract (issue #139).** `stats -f json` emits a
    byte-stable object: top-level keys are serialized in alphabetical order,
    every nested count map (`by_status`, `by_priority`, `by_kind`, `by_skills`,
    `by_assignee`) has its keys sorted alphabetically, and the nested
    `oldest_open` object's keys are likewise alphabetical (`days_old`, `id`,
    `title`). This holds even though the in-memory `Stats` buckets are
    `HashMap`s with per-process-randomized iteration order — the JSON is
    rebuilt at the serialization boundary
    (`format::stats_to_deterministic_json`). Snapshot harnesses MAY compare
    `stats -f json` byte-for-byte. The `avg_urgency` field follows the same
    fixed float-precision contract as graph urgency (below).
- `summary -f json` is a session summary object with counts, completion
  percent, oldest open issue, in-progress issues, ready issues, and recent
  events. Non-JSON modes share compact narrative lines beginning with
  `PROJECT:`.

### Graph

Command: `graph`.

- JSON is `{ "nodes": [...], "edges": [...] }`.
- Compact emits `NODE:` and `EDGE:` lines; quoted node titles are escaped per
  **Escaping In Line-Oriented Output**.
- Pretty emits Graphviz DOT; node label titles use DOT escaping (`\\`, `\"`,
  `\n`) so the output always parses.
- Oneline currently also emits Graphviz DOT.
- **Deterministic urgency precision (issue #139).** In `graph -f json`, each
  node's `urgency` is rounded to a fixed 4 decimal places at the serialization
  boundary (`format::graph_to_deterministic_json`). Urgency is computed fresh as
  an `f64`, so the raw value can carry trailing float noise (e.g.
  `9.00019212962963`); rounding pins the rendered precision without changing the
  underlying urgency *ranking* math. Snapshot harnesses MAY treat node
  `urgency` as byte-stable to 4 decimals; the same precision contract applies to
  `stats -f json`'s `avg_urgency`.
- **Field order (issue #179).** `graph -f json` is serialized straight from
  the serde structs, so it preserves declared field order: `nodes` before
  `edges`; node keys `id`, `title`, `status`, `urgency`, `is_blocked`; edge
  keys `from`, `to`, `type`. The urgency rounding above happens on the struct,
  not via a `serde_json::Value` round trip, so it never reorders keys.

### Events

Command: `log`.

- JSON is an array of `Event`.
- Compact emits `EVENT:<id> ISSUE:<id> FIELD:<field> OLD:"<old>" NEW:"<new>"
  [AGENT:<agent>] (<timestamp>)`. The old/new values are double-quoted and
  escaped per **Escaping In Line-Oriented Output**, so multi-word values and
  values containing literal ` NEW:`/` OLD:` tokens round-trip exactly.
- List-field changes (`tags`, `files`, `skills`) made through `update`,
  `batch update`, and `bulk update` record events whose `old_value` /
  `new_value` are the JSON-array encodings of the list (e.g. `["a","b"]`),
  including the auto-added `_needs_review` tag (#187). Unchanged lists record
  no event.
- Pretty and oneline share a table formatter.

### Batch Results

Commands: `batch add`, `batch create`, `batch close`, `batch update`,
`batch note`.

- JSON is a `BatchResult`: `action`, `results`, `summary`, and optional
  `dry_run`.
- Per-item `outcome` is usually `ok`, `error`, or `review`.
- A malformed item inside the top-level JSON array (wrong type, missing
  required key) becomes a per-item `outcome: "error"` result — `id` is taken
  from the item payload when present, else `0`, and `error` is
  `item <N>: <reason>` with `<N>` the zero-based array index — while the
  remaining items still process, and the command exits 0 (#164). Only a
  top-level payload that is not a JSON array (or is unparseable) is a hard
  `PARSE_ERROR` with exit 1.
- `batch add` accepts `parent` as an alias of `parent_id` in item payloads
  (#150). Unrecognized item keys mark the item `review` with a `REVIEW:` note
  naming them; they are never silently dropped.
- All four batch verbs support `--dry-run`. `batch add --dry-run` and
  `batch note --dry-run` run the exact same parse/validate/insert path inside
  a transaction that is rolled back instead of committed: per-item verdicts
  (including resolved priority/kind defaults and `@N` dependency resolution)
  match the real run byte-for-byte in outcome/notes while nothing is written —
  no issues, no notes, no audit events.
- Compact, pretty, and oneline share the compact envelope:
  `<ACTION>: <n> items (<ok> ok, <error> error, <review> review)` followed by
  per-item lines.

### Bulk Results

Commands: `bulk close`, `bulk update`, `bulk relate`, `bulk depend`,
`bulk note`.

- `bulk close`/`bulk update` JSON is a `BulkResult`: `action`, `count`, `ids`,
  optional `unblocked`, and `dry_run`.
- Compact, pretty, and oneline share `<ACTION>: <n> issues [ids]` with
  optional `(dry-run)` and `UNBLOCKED:` lines.
- `bulk relate`/`bulk depend`/`bulk note` take the same filter grammar
  (`--status`, `--priority`, `--kind`, `--tag`, `--skill`, `--assigned-to`;
  at least one required) and support `--dry-run` on all three. JSON is an
  ad-hoc envelope (`action`, `count`, `ids`, `dry_run`, plus `to`/
  `relation_type` or `on`); line output prints the planned/applied
  `RELATION:`/`DEPEND:`/`NOTE:` lines followed by the `BULK_*` summary line.
  A matched issue equal to `--to`/`--on` is skipped with a `REVIEW:` note.
  Validation is the single-verb code path run inside a transaction — a
  `--dry-run` rolls it back instead of committing, so dependency cycles fail
  identically in both modes and a dry run writes nothing (no rows, no audit
  events).
- An unrecognized `--set-status` or `--set-priority` value soft-falls: every
  matched issue keeps its current value for that field, a
  `REVIEW: <field> '<value>' not recognized; kept each issue's current
  <field>. Valid: ...` note is emitted to stderr, and the command still exits
  0 with the normal `BULK_UPDATE` envelope (count/ids reflect the filter
  match). A valid field in the same invocation is still applied. Never a
  CHECK-constraint `DB_ERROR`.

### Notes

Commands: `note`, `note-delete`, `note-update`.

- JSON is a `Note`.
- Compact, pretty, and oneline share `NOTE:<note_id> ISSUE:<issue_id> ...` for
  create/update and `DELETED NOTE:<note_id> ISSUE:<issue_id>` for delete.

### Other JSON Objects

- `init -f json`: `{ "action": "init", "path": ..., "created": bool }`.
- `depend -f json`: `{ "action": "depend", "blocked_id": ..., "blocker_id":
  ..., "created": bool }`.
- `undepend -f json`: `{ "action": "undepend", "blocked_id": ...,
  "blocker_id": ... }`, followed by an unblocked JSON array if applicable.
- `relate -f json`: `{ "source_id": ..., "target_id": ..., "relation_type":
  ..., "created": bool }`.
- `unrelate -f json`: `{ "source_id": ..., "target_id": ..., "removed": bool,
  "removed_relations": [{ "source_id": ..., "target_id": ...,
  "relation_type": ... }] }` — one entry per removed link, in stored
  direction (#186). With `--type` only links of that relation type are
  removed; without it every typed link between the pair is removed.
- `config get -f json`: `{ "key": ..., "value": ... }`.
- `config set -f json`: `{ "action": "set", "key": ..., "value": ... }`.
- `config reset -f json`: `{ "action": "reset" }`.
- `import -f json`: `{ "action": "import", "imported": n, "skipped": n }`.
- `doctor -f json`: `{ "problems": [...], "fixed": [...], "clean": bool }`.
  `problems` lists what was detected at the start of the run; `clean` reflects
  the post-fix state (true when nothing remains, matching exit 0).
- `ui -f json`: `{ "url": ..., "db_path": ..., "port": n }`.
- `agent-info -f json`: `{ "guide": ... }`.
- `skill -f json`: `{ "skill": ... }`.
- `skill install -f json`: `{ "installed": ... }`.
- `skill path -f json`: `{ "path": ... }`.
- `schema -f json`: `{ "schema": ... }`.
- `reindex -f json`: `{ "action": "reindex", "indexed": n }`.
- `upgrade -f json`: `{ "action": "upgrade", "old_version": ...,
  "new_version": ..., "source": ..., "binary": ..., "pulled": bool,
  "new_changes": bool }`.

`export` is intentionally governed by `--export-format`, not by `-f`: default
stdout is JSONL, and `--export-format json` stdout is a JSON array.

## JSON Determinism And Snapshotting

For byte-level snapshot testing of parseable (`-f json`) output, the following
fields have an explicit determinism contract rather than being compared
structurally:

| Output | Field(s) | Contract |
| --- | --- | --- |
| `stats -f json` | top-level object keys | Alphabetical key order (byte-stable). |
| `stats -f json` | `by_status`, `by_priority`, `by_kind`, `by_skills`, `by_assignee` | Nested count-map keys sorted alphabetically (byte-stable). |
| `stats -f json` | `oldest_open` | Nested keys alphabetical: `days_old`, `id`, `title` (byte-stable). |
| `stats -f json` | `avg_urgency` | Float rounded to 4 decimal places. |
| `graph -f json` | all object keys | Serde struct field order preserved: `nodes` before `edges`; node keys `id`, `title`, `status`, `urgency`, `is_blocked`; edge keys `from`, `to`, `type` (issue #179). |
| `graph -f json` | each node `urgency` | Float rounded to 4 decimal places. |
| `close -f json`, `update -f json` | whole detail object | Serde struct field order preserved; `unblocked` (when present) is appended last (the detail is augmented through a `serde_json::Value` round trip with `preserve_order`). |

All other struct-serialized JSON output preserves its serde-derived struct
field order, which is already deterministic. Two further deterministic
exceptions: the small ad-hoc objects under **Other JSON Objects** (`init`,
`depend`, `config`, `relate`, `doctor`, …) are built with `serde_json::json!`
and serialize in the key order written in the macro (insertion order, via the
`preserve_order` serde_json feature); and applying `--fields` to any JSON
output re-serializes the surviving keys in the requested `--fields` order.
List/array
element order follows the underlying query sort and is deterministic for a
fixed database state. A regression test (`tests/integration.sh`,
"deterministic JSON contracts") seeds two freshly created temp databases
identically and asserts `stats -f json` is byte-identical across them and that
`graph -f json` urgency honors the 4-decimal precision contract; the unit test
`format::tests::graph_json_preserves_serde_struct_field_order` pins the graph
key order.

## Normalized Output Snapshot Harness

Issue #140 adds a checked-in, auto-discovery snapshot harness so that output
changes (compact, JSON, pretty, stderr, and exit status) are reviewed
deliberately as git diffs against expected baselines. It is dependency-light
(pure Bash + `sed` + `diff`) and runs against the same `itr` binary the
integration suite uses, so `just verify` exercises it.

### Layout

```
tests/
  integration.sh                # auto-discovers and runs contract files at the end
  contracts/
    _lib.sh                     # shared harness library (sourced, never run)
    example.sh                  # example area (the harness self-proof)
    <area>.sh                   # one file per area; sources _lib.sh, registers cases
  snapshots/
    example/<case>.txt          # expected normalized snapshot per case
    <area>/<case>.txt
```

### Snapshot file format

Each `tests/snapshots/<area>/<case>.txt` is the normalized capture of one
command, with labeled sections so a diff pinpoints the channel that drifted:

```
$ itr <args...>
--- exit ---
<exit status>
--- stdout ---
<normalized stdout>
--- stderr ---
<normalized stderr>
```

### Normalizations applied (to both stdout and stderr)

| Entropy                             | Replaced with  |
| ----------------------------------- | -------------- |
| UTC ISO-8601 timestamps             | `<TS>`         |
| mktemp temp paths (per-case DB dir) | `<TMP>`        |
| `127.0.0.1:PORT` / `localhost:PORT` | `:<PORT>`      |
| UI session tokens (`token=…`, `X-ITR-Token:`) | `<TOKEN>` |
| version describe/dirty suffix (`itr v2.9.6-1-gdb7e324`) | `itr X.Y.Z` |

Keep snapshotted commands deterministic — the same determinism rules above
(sorted maps, fixed float precision, no un-normalized run-varying fields)
apply. If a command emits entropy the table does not cover, extend
`contract_normalize` in `tests/contracts/_lib.sh` rather than special-casing a
snapshot.

### How to add a new contract area

1. Create `tests/contracts/<area>.sh` that sources `_lib.sh` relative to
   itself and registers cases:

   ```bash
   #!/usr/bin/env bash
   CONTRACT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
   . "$CONTRACT_DIR/_lib.sh"

   echo ""
   echo "--- contract: <area> ---"

   # Each case runs in its own freshly-init'd temp DB. The literal `--`
   # separates harness positionals from the itr argv.
   snapshot <area> <case>                       -- <itr args...>
   snapshot <area> <case_with_stdin> '<stdin json>' -- batch add -f json

   # Seed fixtures first when the assertion needs existing issues:
   seed_<area>() { ITR_DB_PATH="$1" "$ITR" add "Fixture" >/dev/null 2>&1; }
   snapshot_seeded <area> <case> seed_<area>    -- get 1
   ```

2. Generate baselines, then review them in git:

   ```bash
   UPDATE_SNAPSHOTS=1 ./tests/integration.sh      # writes tests/snapshots/<area>/*.txt
   git diff tests/snapshots/<area>/                # eyeball the captured bytes
   ./tests/integration.sh                          # assert mode — must be green
   ```

3. Commit `tests/contracts/<area>.sh` and `tests/snapshots/<area>/*.txt`.
   **Never edit `tests/integration.sh`** — its end-of-suite loop discovers
   every `tests/contracts/*.sh` (except `_lib.sh`) automatically and folds the
   results into the suite totals.

On a mismatch the harness prints a labeled `diff -u` naming the command, args,
exit status, stdout, and stderr, then exits non-zero through the normal suite
reporting.

## Command Matrix

| Command | Input contract | Output contract |
| --- | --- | --- |
| `init` | Creates or opens the target `.itr.db`; `--agents-md` idempotently appends agent guidance. | Init object or `INIT: <path>`. |
| `add`, `create` | Positional title or `--stdin-json`; stores priority, kind, context, files, tags, skills, acceptance, blockers, parent, assignee. | Issue detail. |
| `list` | Filters issue summaries by status, priority, kind, tags, skills, blocked state, parent, assignee; sorts and limits. Default includes open and in-progress issues, including blocked. | Issue list. |
| `get` | Requires one or more issue IDs (repeated, comma-separated, or `A-B` ranges). | Single ID: issue detail or not-found error. Multiple IDs: batched issue details; missing IDs are stderr `REVIEW:` notes, exit 0. |
| `update` | Requires issue ID; replaces fields, appends/removes tags/files/skills, sets parent and assignee. | Issue detail, plus `unblocked` when terminal status unblocks work. |
| `close` | One or more issue IDs (repeated, comma-separated, or ranges); optional trailing reason, `--reason`, `--wontfix`, or `--duplicate-of`. | Single ID: issue detail; duplicate close also creates a duplicate relation. Multiple IDs: batched details in one transaction; missing IDs are stderr `REVIEW:` notes. |
| `note` | One or more issue IDs (repeated, comma-separated, or ranges) followed by the note text; `--agent` overrides `ITR_AGENT`. | Note, or one note per issue (JSON array / `NOTE:` lines) for multi-ID. |
| `note-delete` | Requires note ID. | Deleted note. |
| `note-update` | Requires note ID and new text. | Updated note. |
| `depend`, `deps` | One or more blocked issue IDs (repeated, comma-separated, or ranges) and `--on <blocker_id>`; detects cycles. | Depend object(s) or `DEPEND: <blocked> blocked by <blocker>` per edge. |
| `undepend` | Requires blocked issue ID and `--on <blocker_id>`. | Undepend object or `UNDEPEND: ...`, with optional unblocked notification. |
| `next` | Selects highest-urgency open, unblocked issue; can filter by skill or assignee; `--claim` sets in-progress and may assign agent. | Issue detail or empty result. |
| `ready` | Lists unblocked non-terminal issues; can filter by status, skill, assignee, and limit. | Issue list or empty result. |
| `batch add`, `batch create` | Reads JSON array of add objects from stdin; supports `blocked_by` integer IDs and `@N` intra-batch references; accepts `parent` as an alias of `parent_id`; `--dry-run` validates and previews without writing. | Batch result with issue details; transactional creation; malformed items become per-item errors. |
| `batch close` | Reads JSON array `{id, reason?, wontfix?}`; `--dry-run` previews. | Batch result with per-item outcomes and unblocked items. |
| `batch update` | Reads JSON array of update objects; `--dry-run` previews. | Batch result with per-item outcomes and unblocked items. |
| `batch note` | Reads JSON array `{id, text, agent?}`; item agent overrides `ITR_AGENT`; `--dry-run` previews. | Batch result. |
| `bulk close` | Requires at least one filter; closes all matches; `--dry-run` previews. | Bulk result. |
| `bulk update` | Requires at least one filter; applies shared status/priority/tag changes to all matches; `--dry-run` previews. | Bulk result. |
| `bulk relate` | Requires at least one filter and `--to <target_id>`; optional `--type`; `--dry-run` previews. Self-edges skipped with `REVIEW:`. | `RELATION:` lines plus `BULK_RELATE` summary, or JSON envelope. |
| `bulk depend` | Requires at least one filter and `--on <blocker_id>`; `--dry-run` previews; cycles are hard errors that roll everything back. Self-edges skipped with `REVIEW:`. | `DEPEND:` lines plus `BULK_DEPEND` summary, or JSON envelope. |
| `bulk note` | Requires at least one filter and note text; `--agent` overrides `ITR_AGENT`; `--dry-run` previews. | `NOTE:` lines plus `BULK_NOTE` summary, or JSON envelope. |
| `graph` | Emits dependency and relation graph; `--all` includes terminal issues. | Graph output. |
| `stats` | Reads all issues and current urgency config. | Stats output. |
| `summary` | Reads project counts, ready work, in-progress work, and recent events. | Summary output. |
| `export` | Reads all issues, notes, dependencies, events, and relations. | JSONL by default or JSON array with `--export-format json`. |
| `import` | Reads JSON array or JSONL from `--file` or stdin; `--merge` skips existing IDs. | Import object or `IMPORT: <imported> imported, <skipped> skipped`. |
| `doctor` | Checks orphaned deps, cycles, stale in-progress issues, empty epics, done blockers, and FTS health; `--fix` fixes safe issues. | Doctor report; exits 0 when clean or when `--fix` repaired every detected problem, 1 if problems remain after the run (stderr code `DOCTOR_PROBLEMS_REMAIN`). |
| `ui` | Binds a local HTTP UI to `127.0.0.1`; `--port 0` auto-selects; `--no-open` suppresses browser launch; `--allow-dangerous` enables the raw SQL UI/API. | UI URL and DB path, then serves until stopped. |
| `config list` | Reads effective config defaults plus overrides. | JSON object of key/value strings or `key=value` lines with `*` for custom values. |
| `config get` | Requires config key. | Config get object or `key=value`; unknown keys are errors. |
| `config set` | Requires key and value. | Config set object or `SET: key=value`. |
| `config reset` | Resets stored config overrides. | Config reset object or `CONFIG: Reset to defaults`. |
| `agent-info`, `getting-started`, `getting started` | No database; emits baked agent guide. | Guide text or guide JSON object. |
| `skill` | No subcommand emits baked skill text. | Skill text or skill JSON object. |
| `skill install` | Writes `SKILL.md` to user or project scope; refuses existing file without `--force`. | Installed path object or install line; existing-file refusal is stderr-only review. |
| `skill path` | Computes install target for scope without writing. | Path object or plain path. |
| `schema` | No database; emits compiled schema SQL string. | Schema text or schema JSON object. |
| `upgrade` | Finds source dir, optionally pulls, builds release, and installs over current executable. | Upgrade object or upgrade summary; progress on stderr. |
| `claim`, `start` | With ID, claims that issue; without ID, same selection as `next --claim`; optional skill/agent/assignee filters. | Issue detail or empty result. |
| `assign` | Requires issue ID and agent. | Issue detail with `assigned_to` set. |
| `unassign` | Requires issue ID. | Issue detail with `assigned_to` cleared. |
| `log` | Lists audit events globally or for one issue; supports limit, since, and agent filter. | Event list or empty result. |
| `relate` | One or more source IDs (repeated, comma-separated, or ranges), `--to <target_id>`, and relation type `duplicate`, `related`, or `supersedes`. | Relation object(s) or `RELATION:created|exists ...` per source. |
| `unrelate` | Requires source ID and `--from <target_id>`; optional `--type` (alias of `--relation-type`) limits removal to one relation type (`duplicate`, `related`, or `supersedes`), default removes every type between the pair. | Unrelate object or `RELATION:removed|not_found ...`. |
| `reindex` | Rebuilds FTS index. | Reindex object or `REINDEX: Rebuilt FTS index for <n> issues`. |
| `search` | Query terms use AND semantics across indexed/searchable fields; supports filters and limit. | Search results or empty result. |
| `wip`, `current` | Shorthand for in-progress issue list, including blocked issues. | Issue list. |
| `show` | With ID(s), same contract as `get` (including batched multi-ID retrieval); without ID, lists non-terminal issues including blocked; `--all` includes terminal issues. | Issue detail(s) or issue list. |

## Historical Baseline Output Diff (Developer Tool)

The snapshot harness above (issue #140) is the *gate*: it asserts current output
against checked-in baselines on every run of `tests/integration.sh`. Issue #145
adds a complementary **developer tool**, `tests/tools/baseline-diff.sh`, for a
different question: *how did the output standard change versus a released or
remote ref?* It is intentionally **not** part of the verify gate.

| Question | Use |
| --- | --- |
| "Did current output drift from its checked-in baseline?" | Snapshot harness (`tests/contracts/*.sh`), run by the gate. |
| "How does current output differ from `origin/main` / a tag / an old commit?" | `tests/tools/baseline-diff.sh` (on demand). |

When you deliberately change the CLI output contract (a new field, a reworded
compact line, a changed exit status), the workflow is:

1. Regenerate snapshot baselines and review them as a git diff
   (`UPDATE_SNAPSHOTS=1 ./tests/integration.sh`, then `git diff tests/snapshots/`).
   This keeps the *gate* honest.
2. Optionally run `tests/tools/baseline-diff.sh --baseline origin/main` to get a
   normalized, command-by-command report of the delta against the released ref —
   useful for changelog notes and for confirming the change matches intent.

The tool's contract:

- **Inputs.** `--baseline <ref>` (required) plus a current target: by default it
  builds the working tree, or `--target-binary <path>` / `--baseline-binary
  <path>` to use prebuilt binaries.
- **Isolation.** The baseline ref is built in a detached `git worktree` under a
  temp dir with an isolated `CARGO_TARGET_DIR`; it never touches the user's
  working tree, index, or HEAD, and cleans up on exit.
- **Dirty-tree guard.** It refuses (exit 3) on a dirty working tree unless
  `--allow-dirty` is given, because "current" is ambiguous with uncommitted
  changes.
- **Normalization.** It reuses `contract_normalize` from
  `tests/contracts/_lib.sh`, so runtime entropy (`<TS>`, `<TMP>`, `:<PORT>`,
  `<TOKEN>`, `itr X.Y.Z`) is stripped identically to the snapshot gate.
- **Report.** Plain text with a header (baseline ref + binary identities), a
  per-command summary table marking each command `SAME`/`DIFF` and flagging
  `*CHANGED*` exit statuses, and, for each changed command, a normalized
  `diff -u` plus an exit-status delta line.
- **Exit codes.** `0` ran successfully (differences are data, not failure);
  `2` usage/argument error; `3` refused dirty tree; `4` environment error (not a
  git repo, ref not found, build failed).

It is covered by the auto-discovered smoke test
`tests/contracts/baseline_tool.sh`, which the verify gate runs. The smoke test
exercises the tool's control flow (dirty guard, argument validation, happy-path
report structure, and a stub-vs-real diff proving changed commands / changed
exit status / unified diffs) without a slow full cross-ref build.
