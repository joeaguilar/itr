# Search

`itr search` returns issues whose text matches a free-form query. It works two
ways depending on the SQLite build:

- **FTS5 path** when the optional `issues_fts` virtual table exists. Ranked,
  tokenized full-text search across issue fields, plus a separate LIKE scan over
  notes (which are not indexed in `issues_fts`).
- **LIKE fallback** when FTS5 is unavailable or returns no rows. Case-insensitive
  substring matching across the same fields plus note content.

Both paths share the same query syntax, the same filter flags, and the same
output shape, so callers do not need to branch on which path ran.

## What is indexed

The FTS5 virtual table `issues_fts` indexes one row per issue with these
columns:

- `title`
- `context`
- `acceptance`
- `tags_text` — space-joined `tags` array
- `files_text` — space-joined `files` array
- `skills_text` — space-joined `skills` array
- `close_reason`

Notes (`notes.content`) are intentionally not part of `issues_fts`. The search
command runs a separate LIKE pass over `notes.content` and merges note hits into
the result list. The LIKE fallback path covers the same eight fields (issue
text columns plus notes) in a single SQL pass.

The `tags`, `files`, and `skills` arrays are stored as JSON in TEXT columns; FTS
sees them as space-separated tokens, while the LIKE path matches against the
raw JSON text (so `LIKE '%refactor%'` will still hit a tag named `refactor`).

See [docs/schema.md](schema.md) for the full `issues_fts` definition and the
indexing trigger points (insert, update, reindex).

## AND-by-default query semantics

A search query is split on whitespace into terms. **Every term must match
somewhere on the same issue.** The matching field can differ from one term to
the next — `itr search "auth token"` will return an issue whose `title`
contains `auth` and whose `notes` contain `token`, even though neither field
holds both words.

This holds on both code paths:

- FTS5 builds a quoted `"term1" AND "term2" AND ...` MATCH expression.
- LIKE builds one `(title LIKE %term% OR context LIKE %term% OR ...)` group per
  term, joined with `AND`, so each term must appear in at least one indexed
  field or in a note.

There is no `OR` operator, no field-specific query syntax (`title:foo`), and no
phrase grouping beyond what a single whitespace-free token already gives you.
Use multi-word queries to narrow results, not to broaden them.

### Examples

Find an open issue mentioning both `urgency` and `coefficient` anywhere:

```bash
itr search "urgency coefficient"
```

Mixed-field hit — `auth` in `tags`, `retry` in a `note`, `oauth` in `context`:

```bash
itr search "auth retry oauth"
```

Restrict to bugs touching the UI layer:

```bash
itr search "token expired" --kind bug --status open
```

Search across closed issues too:

```bash
itr search "wontfix-rationale" --all
```

Filter by skill (AND logic, all skills required) and assignee:

```bash
itr search "migration" --skill rust --skill sql --assigned-to alice
```

Cap the result count and emit JSON for downstream tooling:

```bash
itr search "flaky" -n 5 -f json
```

## FTS5 vs LIKE fallback decision logic

The dispatch lives in `src/commands/search.rs::run`:

1. If `db::has_fts(conn)` is true, run `db::fts_search(conn, query)`.
2. If FTS returns at least one ID, use those IDs and append note-only matches
   from a LIKE scan against `notes.content`.
3. If FTS returns zero IDs, fall back to the full LIKE scan
   (`db::search_issue_ids`) over both issue fields and notes. This guards
   against a stale index — a recent insert that has not been indexed yet still
   shows up via LIKE.
4. If `has_fts` is false (SQLite built without FTS5, or table creation failed
   silently at `open_db` time), the LIKE scan is the only path.

Post-filtering (`--status`, `--priority`, `--kind`, `--skill`, `--assigned-to`,
`--limit`) and urgency-based sorting run identically on whatever ID set the
search produced. By default, `--status` defaults to `open,in-progress`; pass
`--all` to include `done` and `wontfix`.

Empty results return exit 0 (see [command contracts](command-contracts.md)).

## When to run `itr reindex`

`itr reindex` drops and rebuilds `issues_fts` from the current `issues` rows.
Run it when:

- You import issues with `itr import` or any bulk SQL path that bypasses the
  per-write `fts_index_issue` hook.
- A migration adds or changes a searchable column.
- `itr doctor` reports stale FTS row counts (and you prefer the manual rebuild
  over `itr doctor --fix`).
- Search results look stale even though the LIKE fallback finds them — this
  usually means FTS returned a non-empty (but incomplete) set, so the fallback
  never kicked in.

You do **not** need to reindex after normal `itr add` / `itr update` /
`itr close`; those write paths call `db::fts_index_issue` on every searchable
mutation.

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
- `context_snippets` — short `...prefix **match** suffix...` excerpts per
  matched field, computed locally in Rust (not from FTS5 `snippet()`), so they
  look the same on either code path.

## Related

- [docs/schema.md](schema.md) — `issues_fts` table definition, migrations, and
  the `try_create_fts` soft-fallback.
- [docs/troubleshooting.md](troubleshooting.md#search-misses-expected-results)
  — recovering from stale search results and missing FTS5 builds.
- [docs/command-contracts.md](command-contracts.md) — global rules for stdout,
  stderr, and exit codes that `itr search` follows.
