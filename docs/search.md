# Search

`itr search` returns issues whose text matches a free-form query. It works two
ways depending on the SQLite build and on what the query matches:

- **FTS5 path** when the optional `issues_fts` virtual table exists and the
  query matches at least one issue's indexed fields. Ranked, tokenized
  full-text search across issue fields, plus a separate LIKE pass that appends
  issues whose notes contain every term (notes are not indexed in
  `issues_fts`).
- **LIKE fallback** when FTS5 is unavailable or the FTS query matches no
  issue at all. Case-insensitive substring matching across the same fields
  plus note content.

Both paths share the same query syntax, the same filter flags, and the same
output shape — but they do **not** match exactly the same issues when a
query's terms are split between issue fields and notes. See the
[known limitation](#known-limitation-terms-split-between-issue-fields-and-notes)
below.

## What is indexed

The FTS5 virtual table `issues_fts` indexes one row per issue with these
columns:

- `title`
- `context`
- `acceptance`
- `tags_text` — the `tags` array as searchable tokens
- `files_text` — the `files` array as searchable tokens
- `skills_text` — the `skills` array as searchable tokens
- `close_reason`

Notes (`notes.content`) are intentionally not part of `issues_fts`. The search
command runs a separate LIKE pass over `notes.content` and merges note hits
into the result list — but only issues where **every** query term appears
within a single note (see below). The LIKE fallback path covers the same
eight fields (issue text columns plus notes) in a single SQL pass.

The `tags`, `files`, and `skills` arrays are stored as JSON in TEXT columns.
FTS sees the array elements as separate tokens (the tokenizer treats JSON
punctuation as separators), while the LIKE path matches against the raw JSON
text (so `LIKE '%refactor%'` will still hit a tag named `refactor`).

The index is kept fresh automatically: SQLite triggers reindex an issue on
insert, on delete, and on any update of a searchable column, covering every
write path including raw SQL, and `itr import` additionally indexes each
imported issue inside its transaction. Normal `itr add` / `itr update` /
`itr close` / `itr import` never require a manual rebuild — see
[When to run `itr reindex`](#when-to-run-itr-reindex).

See [docs/schema.md](schema.md) for the `issues_fts` definition.

## Query semantics

A search query is split on whitespace into terms. Terms are ANDed: every term
must match the same issue. There is no `OR` operator, no field-specific query
syntax (`title:foo`), and no phrase grouping beyond what a single
whitespace-free token already gives you. Use multi-word queries to narrow
results, not to broaden them.

*Where* each term is allowed to match depends on which code path runs.

### FTS path (the usual case)

1. **Field pass.** The query is rewritten as `"term1" AND "term2" AND ...`
   and run as an FTS5 MATCH. An issue matches when every term appears
   somewhere in its FTS-indexed fields. Different terms may hit different
   fields — `itr search "auth retry"` matches an issue with `auth` in `title`
   and `retry` in `tags` — but **all terms must be satisfied by issue fields
   alone**; notes do not participate in this pass.
2. **Note pass.** A LIKE scan over `notes.content` appends issues where every
   term appears **within a single note**. A note containing only some of the
   terms does not count, and terms spread across two different notes of the
   same issue do not count either.

An issue that needs a *mix* of the two passes — one term only in a field,
another term only in a note — is matched by neither pass. That is the known
limitation below.

### LIKE fallback

Runs only when `issues_fts` is missing (SQLite built without FTS5, or table
creation failed silently when the database was opened) or when the field pass
above matched zero issues. The query builds one
`(title LIKE %term% OR context LIKE %term% OR ... OR note content LIKE %term%)`
group per term, joined with `AND`, evaluated against each (issue, note) row
pair. Each term must therefore appear in an issue field **or in the same
single note**:

- `auth` in `title` plus `retry` only in a note **does** match on this path.
- Terms split across two different notes, with no field hit for one of them,
  still do not match.

### Known limitation: terms split between issue fields and notes

An issue whose match requires combining issue fields *and* a note — e.g.
`auth` only in the title and `retry` only in a note — is found **only when
the LIKE fallback runs**. As soon as any other issue satisfies the whole
query within its FTS-indexed fields, the FTS path is taken and the
cross-field-plus-note issue is silently missed.

Reproduced against the current binary:

```bash
itr add "auth login flow"                      # issue 1
itr note 1 "retry the token refresh on failure"
itr add "auth retry handler"                   # issue 2

itr search "auth retry"
# Returns only issue 2. Issue 1 (auth in title + retry in a note) is missed
# because issue 2 made the FTS field pass non-empty, and the note pass
# requires BOTH terms inside one of issue 1's notes.
```

With no FTS-complete competitor, the fallback runs and the same shape of
query works:

```bash
itr add "websocket reconnect logic"            # issue 3
itr note 3 "use exponential backoff between attempts"

itr search "websocket backoff"
# Returns issue 3 (title + note combined) — no other issue matched via FTS,
# so the LIKE fallback ran.
```

The FTS-vs-fallback decision is made **before** status filtering. A closed
issue that matches every term in its fields therefore also suppresses the
fallback: after `itr close 2 "..."`, `itr search "auth retry"` (default
statuses `open,in-progress`) returns nothing at all, even though open issue 1
still has `auth` in its title and `retry` in a note.

Workarounds: search for the single most distinctive term, or run one query
per term and intersect the resulting IDs yourself.

### Literal wildcard matching

LIKE wildcards in query terms are escaped, so `%` and `_` match only their
literal characters:

```bash
itr search "100%"     # matches "reach 100% branch coverage",
                      # NOT "100 percent done plan"
itr search "foo_bar"  # matches "tune foo_bar config",
                      # NOT "tune fooxbar config"
```

FTS5 tokenization would happily treat `100%` as the token `100`, but every
candidate result is re-checked with a literal case-insensitive substring test
per field; candidates whose text never literally contains any query term end
up with empty `matched_fields` and are dropped from the results.

## Examples

All examples below were reproduced against the current binary.

Both terms in one field:

```bash
itr search "auth retry"        # hits title "auth retry handler"
```

Terms in different issue fields of the same issue (`auth` in `title`, `retry`
in `tags`):

```bash
itr search "auth retry"        # also hits title "auth session hardening"
                               # tagged "retry"
```

Every term inside a single note:

```bash
itr search "gamma delta"       # hits an issue whose one note says
                               # "gamma and delta discussed in one note"
```

Restrict by kind and status (values are normalized, see
[Filters](#filters)):

```bash
itr search "auth retry" --kind bug
itr search "frobnicate" --status wip     # wip -> in-progress
```

Search across closed issues too:

```bash
itr search "auth retry" --all
```

Filter by skill (AND logic, all skills required) and assignee:

```bash
itr search "migration" --skill rust --skill sql --assigned-to alice
```

Cap the result count and emit JSON for downstream tooling:

```bash
itr search "migration" -n 1 -f json
```

## FTS5 vs LIKE fallback decision logic

The dispatch lives in `src/commands/search.rs::run_core`:

1. If `db::has_fts(conn)` is true, run `db::fts_search(conn, query)` (the
   field pass).
2. If the field pass returns at least one ID — **before** any status,
   priority, or kind filtering — keep the FTS path: post-filter those IDs,
   then append note matches from `db::search_note_issue_ids` (every term in
   one note, same filters applied).
3. If the field pass returns zero IDs, fall back to the full LIKE scan
   (`db::search_issue_ids`) over issue fields plus notes. This also guards
   against a stale index — a row missing from `issues_fts` can still be found
   via LIKE as long as no other issue matches via FTS.
4. If `has_fts` is false (SQLite built without FTS5, or table creation failed
   silently at open time), the LIKE scan is the only path.

Whatever ID set the search produced then goes through the literal
`matched_fields` check (dropping FTS token-only false positives), the
`--skill` / `--assigned-to` filters, urgency-based sorting, and `--limit`.

Empty results print `No matching issues found.` (or `[]` in JSON mode) and
exit 0 (see [command contracts](command-contracts.md)).

## Filters

- `--status`, `--priority`, and `--kind` accept the same synonyms as the
  write paths and normalize them before filtering: `wip` → `in-progress`,
  `closed` → `done`, `urgent` → `critical`, `defect` → `bug`, and so on.
  Recognized synonyms normalize silently; a value that is still unrecognized
  after normalization emits a `REVIEW:` note on stderr and matches nothing
  (exit 0, empty result).
- `--status` defaults to `open,in-progress`.
- `--all` includes every status (`done` and `wontfix` too). Note that `--all`
  disables status filtering entirely — an explicit `--status` passed
  alongside `--all` is ignored.
- `--skill` may repeat; all listed skills must be present on the issue (AND
  logic). `--assigned-to` requires an exact assignee match. Both apply after
  the text search.
- `-n`/`--limit` truncates after sorting.

## When to run `itr reindex`

`itr reindex` drops and rebuilds `issues_fts` (and its sync triggers) from
the current `issues` rows. The index is normally self-maintaining — triggers
cover every SQL write path, including raw SQL and the UI's dangerous SQL
mode, and `itr import` indexes as it writes — so reindex is a recovery tool:

- `itr doctor` reports a stale FTS row count (`fts_stale`) and you prefer the
  manual rebuild over `itr doctor --fix` (which performs the same rebuild).
- Search results look stale or wrong even though the LIKE fallback would find
  them — this usually means FTS returned a non-empty (but incomplete) set, so
  the fallback never kicked in.
- A migration adds or changes a searchable column.

You do **not** need to reindex after normal `itr add` / `itr update` /
`itr close` / `itr import`. Updates take effect immediately: renaming an
issue makes the new title searchable and removes the old title's tokens in
the same write. A legacy `issues_fts` table from an older itr build is
detected and rebuilt automatically the next time the database is opened.

If `itr reindex` exits with `INVALID_VALUE` on `fts5`, the bundled SQLite in
that environment was built without FTS5. Search still works via the LIKE
fallback; nothing else degrades. See
[docs/troubleshooting.md](troubleshooting.md#search-misses-expected-results)
for diagnostics.

## Output

`itr search` returns a list of `SearchResult` entries (see `src/models.rs`),
sorted by urgency descending. Each entry includes:

- The issue summary fields (`id`, `title`, `status`, `priority`, `kind`,
  `urgency`, `is_blocked`, `blocked_by`, `tags`, `files`, `skills`,
  `acceptance`, `assigned_to`).
- `matched_fields` — which fields contributed the hit (`title`, `context`,
  `acceptance`, `tags`, `files`, `skills`, `notes`, `close_reason`).
  Computed with a literal case-insensitive substring check per field; a
  candidate with no literal match in any field (e.g. an FTS token match like
  `100` for the query `100%`) is dropped, so every returned result carries at
  least one matched field.
- `context_snippets` — short `...prefix **match** suffix...` excerpts per
  matched text field, computed locally in Rust (not from FTS5 `snippet()`),
  so they look the same on either code path. For `tags`, `files`, and
  `skills` the snippet is the matched array element itself.

The global `--fields` flag filters which keys appear in both `json` and
`compact` output (e.g. `itr search "auth" --fields id,title`); `pretty`
output does not support it and warns on stderr.

## Related

- [docs/schema.md](schema.md) — `issues_fts` table definition and the
  `try_create_fts` soft-fallback.
- [docs/troubleshooting.md](troubleshooting.md#search-misses-expected-results)
  — recovering from stale search results and missing FTS5 builds.
- [docs/command-contracts.md](command-contracts.md) — global rules for stdout,
  stderr, and exit codes that `itr search` follows.
