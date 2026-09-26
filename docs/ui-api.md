# UI API Reference

This documents the current `itr ui` localhost API. These routes are UI
internals served by the `itr` binary on `127.0.0.1`; they are not a stable
remote service contract.

`itr ui` serves embedded static assets and a JSON API from the same process.
The browser receives a per-session token in the root URL. API callers must send
that token as `X-ITR-Token: <token>` or as a `token=<token>` query parameter.
Request bodies and responses are JSON unless noted.

Every mutating route (create, PATCH, close, notes, dependencies, relations,
bulk resolve) runs in a single `BEGIN IMMEDIATE` transaction: a failure at any
step rolls the whole request back, including its audit events, and concurrent
CLI writers wait on the lock instead of failing with `database is locked`.

Raw SQL is disabled unless the server starts with `itr ui --allow-dangerous`.
When disabled, `POST /api/sql` returns `403` with
`DANGEROUS_SQL_DISABLED`.

## Session Token

The token is generated once per `itr ui` process at startup by reading 24
random bytes from SQLite's `randomblob(24)` and lowercase-hex-encoding them,
producing a 48-character hexadecimal string (e.g. `a1b2c3...` of length 48).
The token is bound to the running process: it is never persisted to disk, and
every restart of `itr ui` mints a fresh token. There is no rotation, refresh,
or revocation API — kill and restart the server to invalidate the current
token. The token is emitted in the startup URL on stdout (and as the `url`
field in `--format json` mode); copy it from there to drive the API directly.

## Response Headers

Every response (static assets, API JSON, and error JSON) includes the
following headers in addition to `Content-Type`, `Content-Length`, and
`Connection: close`:

| Header | Value | Purpose |
| --- | --- | --- |
| `X-Content-Type-Options` | `nosniff` | Disables browser MIME sniffing of response bodies. |
| `Referrer-Policy` | `no-referrer` | Prevents the browser from leaking the token-bearing URL via the `Referer` header. |

## Static Assets

| Method | Path | Token | Response |
| --- | --- | --- | --- |
| `GET` | `/` | Required | Embedded `index.html`. |
| `GET` | `/assets/app.css` | No | Embedded CSS, `text/css`. |
| `GET` | `/assets/app.js` | No | Embedded JS, `application/javascript`. |

## Common Shapes

`IssueSummary`:

```json
{
  "id": 1,
  "title": "string",
  "status": "open",
  "priority": "medium",
  "kind": "task",
  "urgency": 0.0,
  "is_blocked": false,
  "blocked_by": [2],
  "tags": ["tag"],
  "files": ["path"],
  "skills": ["skill"],
  "acceptance": "string",
  "assigned_to": "string",
  "created_at": "string",
  "updated_at": "string"
}
```

`IssueDetail` is an `IssueSummary`-like issue object with full editable fields
and related data:

```json
{
  "id": 1,
  "title": "string",
  "status": "open",
  "priority": "medium",
  "kind": "task",
  "context": "string",
  "files": ["path"],
  "tags": ["tag"],
  "skills": ["skill"],
  "acceptance": "string",
  "parent_id": null,
  "assigned_to": "string",
  "close_reason": "string",
  "created_at": "string",
  "updated_at": "string",
  "urgency": 0.0,
  "blocked_by": [2],
  "blocks": [3],
  "is_blocked": false,
  "notes": [
    {
      "id": 10,
      "issue_id": 1,
      "content": "string",
      "agent": "string",
      "created_at": "string"
    }
  ],
  "urgency_breakdown": {
    "components": [["component", 0.0]]
  },
  "children": [
    {
      "$ref": "IssueSummary"
    }
  ],
  "relations": [
    {
      "id": 20,
      "source_id": 1,
      "target_id": 4,
      "relation_type": "related",
      "created_at": "string"
    }
  ]
}
```

`children` is present on UI issue detail responses. `relations` is omitted only
when empty in serializers that skip empty vectors.

## Routes

### `GET /api/health`

Token required. No request body.

Response:

```json
{
  "ok": true,
  "db_path": "/path/to/.itr.db",
  "version": "string"
}
```

### `GET /api/bootstrap`

Token required. No request body.

Response:

```json
{
  "db_path": "/path/to/.itr.db",
  "version": "string",
  "statuses": ["open", "in-progress", "done", "wontfix"],
  "priorities": ["critical", "high", "medium", "low"],
  "kinds": ["bug", "feature", "task", "epic"],
  "dangerous_sql": false,
  "stats": {
    "total": 0,
    "active": 0,
    "done": 0,
    "wontfix": 0,
    "blocked": 0,
    "ready": 0
  }
}
```

`blocked` and `ready` partition `active` (open + in-progress) with the same
definition `itr stats` uses: a done or wontfix issue is never counted as
blocked, even if an open issue still has an edge to it.

### `GET /api/issues`

Token required. No request body.

Query parameters:

| Name | Meaning |
| --- | --- |
| `q` | Whitespace-separated search terms matched against issue text, lists, and notes. |
| `status` | Comma-separated statuses. |
| `priority` | Comma-separated priorities. |
| `kind` | Comma-separated kinds. |
| `tag` | Comma-separated tags, all required. |
| `tag_any` | Comma-separated tags, any accepted. |
| `skill` | Comma-separated skills, all required. |
| `assigned_to` | Exact assignee filter. |
| `ready` | Boolean: `1`, `true`, `yes`, or `on`. Excludes blocked and closed issues. |
| `blocked` | Boolean: only blocked issues. |
| `all` | Boolean: include closed issues when no status filter is set. |
| `sort` | Column to sort by: `urgency` (default), `id`, `status`, `priority`, `kind`, `title`, `tags`, `assignee`, `created`, `updated`, or `blocked`. |
| `dir` | `asc` or `desc`. Omitted (or unrecognized) means each column's natural default. |
| `limit` | Maximum result count. |

`assigned_to` is matched as an exact string. Passing an empty string
(`?assigned_to=`) is treated as "no filter" rather than "match issues whose
assignee is the empty string" — to find unassigned issues, omit the parameter
and filter client-side, or use the raw SQL endpoint.

Unknown `sort` and `dir` values are soft fallbacks: the listing comes back
sorted by the default (`urgency`, highest first) rather than erroring.

Sorting notes:

- `priority` and `status` sort **semantically**, not alphabetically:
  `critical → high → medium → low`, and `in-progress → open → done → wontfix`.
- `urgency`, `created`, `updated`, and `blocked` default to `desc` (highest /
  newest / blocked first); every other column defaults to `asc`.
- Issue id ascending is the tiebreaker in both directions, so rows with equal
  values keep a stable order instead of flipping with the direction.
- An empty `assignee` sorts after populated ones in ascending order.

#### Search by issue number

When `q` names an issue by number — `42`, `#42`, `id:42`, or `ID=42`, as the
first such token anywhere in the query — that issue is placed first in
`issues` and reported as `pinned_id`. The pin is applied **after** sorting and
**before** `limit`, and it ignores the other filters: naming a closed issue by
number surfaces it even though the default listing hides closed issues. If no
issue has that id, `pinned_id` is `null` and the number is treated as ordinary
search text.

Response:

```json
{
  "total": 1,
  "pinned_id": null,
  "issues": [
    {
      "$ref": "IssueSummary"
    }
  ]
}
```

#### Batched detail mode: `GET /api/issues?ids=1,2,3`

Passing the `ids` query parameter (comma-separated integer issue ids)
switches this route into a batched **detail** fetch: instead of filtered
`IssueSummary` records it returns one full `IssueDetail` per requested id, in
request order — the same objects `GET /api/issues/{id}` returns, in a single
round trip. All other query parameters are ignored in this mode.

- Duplicate ids are fetched once (first occurrence wins).
- Ids that are valid integers but do not exist are reported in `missing`
  instead of failing the batch; the response is still `200`.
- A non-integer token in `ids` is a `400` with `INVALID_VALUE`, consistent
  with path-segment id parsing on the per-issue routes.

Response:

```json
{
  "total": 2,
  "issues": [
    {
      "$ref": "IssueDetail"
    }
  ],
  "missing": [999]
}
```

`total` counts the returned issues, not the requested ids. The plain list
response above never contains a `missing` key, so callers can distinguish the
two modes.

### `POST /api/sql`

Token required. Requires `itr ui --allow-dangerous`.

Request body:

```json
{
  "sql": "select id, title from issues limit 20"
}
```

Query responses include column names, rows as arrays, total rows stepped, a
truncation marker for displayed rows, and `changes`: the number of rows the
submitted statement(s) inserted, updated, or deleted at top level. Rows touched
by triggers (the `updated_at` touch, full-text index sync) are not counted, and
a multi-statement batch reports the sum across its statements:

```json
{
  "columns": ["id", "title"],
  "rows": [[1, "Example"]],
  "row_count": 1,
  "truncated": false,
  "changes": 0
}
```

Statements that do not return columns run through SQLite batch execution and
return no rows:

```json
{
  "columns": [],
  "rows": [],
  "row_count": 0,
  "truncated": false,
  "changes": 1
}
```

Only the first 500 result rows are retained in the response. The statement is
still stepped to completion so mutating statements with returned rows are not
partially applied because of response truncation.

### `POST /api/issues`

Token required.

Request body:

```json
{
  "title": "required non-empty string",
  "priority": "medium",
  "kind": "task",
  "context": "string",
  "files": ["path"],
  "tags": ["tag"],
  "skills": ["skill"],
  "acceptance": "string",
  "parent_id": null,
  "assigned_to": "string",
  "blocked_by": [2]
}
```

The body is parsed and inserted by the same code as
`itr add --stdin-json`, so the two paths behave identically:

- Defaults: `priority` defaults to `medium`, `kind` defaults to `task`, strings
  default to empty, arrays default to empty, and `parent_id` defaults to `null`.
- `parent` is accepted as an alias for `parent_id`.
- `blocked_by` entries may be integers or numeric strings (`[2, "3"]`). Other
  tokens are skipped with a REVIEW note.
- Unknown keys are not silently dropped: each produces a REVIEW note.
- Unknown priority or kind values fall back to the defaults with a REVIEW
  note.
- A `parent_id` that does not exist creates the issue without a parent and
  adds a REVIEW note (not a `500` foreign-key error).
- A `blocked_by` id that does not exist is a `404 NOT_FOUND`. The whole create
  rolls back, so no issue, notes, or edges are left behind.

Any REVIEW note tags the issue `_needs_review` and is stored as an `itr` note.

The UI adds some normalization on top. The title is trimmed, and an empty or
whitespace-only title is a `400 INVALID_VALUE`. `files`, `tags`, and `skills`
are trimmed and deduplicated, and `skills` are lowercased. A body that is not
UTF-8 JSON, or that has a wrong-typed field such as `"title": 5`, is a `400`.

Response. `review_notes` lists the REVIEW notes this create produced, and is
empty for a clean create:

```json
{
  "issue": {
    "$ref": "IssueDetail"
  },
  "review_notes": ["REVIEW: parent 9999 not found; issue created without a parent"]
}
```

### `GET /api/issues/{id}`

Token required. No request body.

Response:

```json
{
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `PATCH /api/issues/{id}`

Token required.

Request body is a partial object. Supported fields:

```json
{
  "title": "string",
  "context": "string",
  "acceptance": "string",
  "assigned_to": "string",
  "close_reason": "string",
  "status": "open",
  "priority": "medium",
  "kind": "task",
  "files": ["path"],
  "tags": ["tag"],
  "skills": ["skill"],
  "parent_id": null
}
```

Type checking runs before any write. The request is a `400 INVALID_VALUE` that
names every offending field, and applies nothing, when any of these hold:

- A text or enum field is not a string (`{"title": null, "context": 123}`).
- `title` is empty or whitespace-only. Titles are trimmed, as on create.
- `files`, `tags`, or `skills` is not an array of strings.
- `parent_id` is not an integer, a numeric string such as `"12"`, or `null`.
  `null` clears the parent, and anything else, such as `"abc"`, never clears it.

`parent` is accepted as an alias for `parent_id`. Other unknown keys are
ignored with a REVIEW note.

Invalid `status`, `priority`, and `kind` values follow the CLI update soft
fallback. The current value is **kept**, a REVIEW note is added, and the issue
is tagged `_needs_review`. An unrecognized status never reopens a closed
issue.

Patching `status` to `done` or `wontfix` behaves like `itr close`. It removes
dependency edges where this issue was the blocker and reports the newly
unblocked issues in `unblocked`.

Patching `assigned_to` to an empty string clears the assignee for that issue;
this is distinct from the `assigned_to=` query parameter on
`GET /api/issues`, which treats an empty value as "no filter".

Response:

```json
{
  "issue": {
    "$ref": "IssueDetail"
  },
  "unblocked": [
    {
      "id": 2,
      "title": "string"
    }
  ],
  "review_notes": []
}
```

### `POST /api/issues/{id}/close`

Token required.

Request body:

```json
{
  "reason": "string",
  "wontfix": false
}
```

`wontfix: true` resolves to status `wontfix`; otherwise status `done`.
Non-empty `reason` is stored as `close_reason`. Closing removes dependency
edges where the resolved issue was the blocker and reports newly unblocked
issues.

Response:

```json
{
  "issue": {
    "$ref": "IssueDetail"
  },
  "unblocked": [
    {
      "id": 2,
      "title": "string"
    }
  ]
}
```

### `POST /api/issues/{id}/notes`

Token required.

Request body:

```json
{
  "content": "string",
  "agent": "string"
}
```

`content` must be non-empty after trimming. Otherwise the request is a
`400 INVALID_VALUE`. As with `itr note`, an empty `agent` falls back to the
`ITR_AGENT` environment variable of the `itr ui` process, and then to empty.
The note and its `note_added` audit event commit together.

Response:

```json
{
  "note": {
    "id": 10,
    "issue_id": 1,
    "content": "string",
    "agent": "string",
    "created_at": "string"
  },
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `PATCH /api/notes/{id}`

Token required.

Request body:

```json
{
  "content": "string",
  "agent": "string"
}
```

Only `content` is persisted; `agent` is accepted by the input shape but ignored.
`content` must be non-empty after trimming (`400 INVALID_VALUE`). The update
and its `note_updated` event commit together, and a delete commits together
with its `note_deleted` event.

Response:

```json
{
  "note": {
    "id": 10,
    "issue_id": 1,
    "content": "string",
    "agent": "string",
    "created_at": "string"
  },
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `DELETE /api/notes/{id}`

Token required. No request body.

Response:

```json
{
  "note": {
    "id": 10,
    "issue_id": 1,
    "content": "string",
    "agent": "string",
    "created_at": "string"
  },
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `POST /api/issues/{id}/dependencies`

Token required.

Request body:

```json
{
  "blocker_id": 2
}
```

Adds an edge where `blocker_id` blocks `{id}`.

Response:

```json
{
  "created": true,
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `DELETE /api/issues/{id}/dependencies/{blocker_id}`

Token required. No request body.

Removes the edge where `{blocker_id}` blocks `{id}`.

Response:

```json
{
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `POST /api/issues/{id}/relations`

Token required.

Request body:

```json
{
  "target_id": 2,
  "relation_type": "related"
}
```

`relation_type` defaults to `related`. Valid values are `duplicate`,
`related`, and `supersedes`.

Response:

```json
{
  "created": true,
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `DELETE /api/issues/{id}/relations/{target_id}`

Token required. No request body.

Removes the outbound relation from `{id}` to `{target_id}`.

Response:

```json
{
  "removed": true,
  "issue": {
    "$ref": "IssueDetail"
  }
}
```

### `POST /api/bulk/resolve/preview`

Token required.

Request body:

```json
{
  "ids": [1, 2],
  "reason": "string",
  "wontfix": false
}
```

Only `ids` and `wontfix` affect preview output. `reason` is accepted by the
shared input shape and ignored.

Response:

```json
{
  "count": 2,
  "issues": [
    {
      "$ref": "IssueSummary"
    }
  ],
  "target_status": "done"
}
```

`target_status` is `wontfix` when `wontfix` is true.

### `POST /api/bulk/resolve/apply`

Token required.

Request body:

```json
{
  "ids": [1, 2],
  "reason": "string",
  "wontfix": false
}
```

Applies the same close behavior as `POST /api/issues/{id}/close` to each id.
The request is all-or-nothing. Every id is checked before anything is written,
and all resolves share one transaction. A single missing id is a
`404 NOT_FOUND`, and several missing ids are a `400 INVALID_VALUE` that lists
them. In both cases no issue is resolved. Duplicate ids are resolved once.

Response:

```json
{
  "count": 2,
  "issues": [
    {
      "$ref": "IssueDetail"
    }
  ],
  "unblocked": [
    {
      "id": 3,
      "title": "string"
    }
  ]
}
```

## Errors

All API errors are JSON:

```json
{
  "error": "human-readable message",
  "code": "ERROR_CODE"
}
```

HTTP status mapping:

| Status | Codes |
| --- | --- |
| `400` | `BAD_REQUEST`, `INVALID_VALUE`, `PARSE_ERROR`, `NO_FILTERS` |
| `403` | `DANGEROUS_SQL_DISABLED` |
| `404` | `NOT_FOUND` |
| `409` | `CYCLE_DETECTED` |
| `500` | `INTERNAL_ERROR`, `NO_DATABASE`, `DB_ERROR`, `IO_ERROR`, `UPGRADE_FAILED`, `NEWER_SCHEMA` |

`DANGEROUS_SQL_DISABLED` is returned by `POST /api/sql` when the server was
started without `--allow-dangerous`. Restart `itr ui --allow-dangerous` to
enable raw SQL for the new session.

Unknown API routes return `404` with `code: "NOT_FOUND"` after token
validation. Missing or invalid tokens currently use `400` with
`code: "INVALID_VALUE"`. The request body limit is 1,048,576 bytes.
