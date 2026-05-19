# UI API Reference

This documents the current `itr ui` localhost API. These routes are UI
internals served by the `itr` binary on `127.0.0.1`; they are not a stable
remote service contract.

`itr ui` serves embedded static assets and a JSON API from the same process.
The browser receives a per-session token in the root URL. API callers must send
that token as `X-ITR-Token: <token>` or as a `token=<token>` query parameter.
Request bodies and responses are JSON unless noted.

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
| `sort` | `urgency` (default), `created`, `updated`, `id`, or `priority`. |
| `limit` | Maximum result count. |

`assigned_to` is matched as an exact string. Passing an empty string
(`?assigned_to=`) is treated as "no filter" rather than "match issues whose
assignee is the empty string" — to find unassigned issues, omit the parameter
and filter client-side, or use the raw SQL endpoint.

`sort=priority` sorts by the priority string in **ascending alphabetic order**,
with issue id as a tiebreaker. Because the four priority values do not sort
into severity order alphabetically, the actual sequence is:

1. `critical`
2. `high`
3. `low`
4. `medium`

This is unrelated to severity — high-severity issues should usually be
surfaced via the default `sort=urgency`.

Response:

```json
{
  "total": 1,
  "issues": [
    {
      "$ref": "IssueSummary"
    }
  ]
}
```

### `POST /api/sql`

Token required. Requires `itr ui --allow-dangerous`.

Request body:

```json
{
  "sql": "select id, title from issues limit 20"
}
```

Query responses include column names, rows as arrays, total rows stepped, a
truncation marker for displayed rows, and the connection change count:

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

Defaults: `priority` defaults to `medium`, `kind` defaults to `task`, strings
default to empty, arrays default to empty, and `parent_id` defaults to `null`.
Unknown priority or kind values are soft-normalized to defaults and add
`_needs_review` plus review notes.

Response:

```json
{
  "issue": {
    "$ref": "IssueDetail"
  }
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

`parent_id` must be an integer issue id or `null`. Invalid `status`,
`priority`, and `kind` values fall back to `open`, `medium`, and `task`.

Patching `status` to `done` or `wontfix` **does not** remove dependency edges
or report newly unblocked issues. It only updates the status field. To close
an issue and unblock its dependents in one step, use
`POST /api/issues/{id}/close` instead.

Patching `assigned_to` to an empty string clears the assignee for that issue;
this is distinct from the `assigned_to=` query parameter on
`GET /api/issues`, which treats an empty value as "no filter".

Response:

```json
{
  "issue": {
    "$ref": "IssueDetail"
  }
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

`agent` defaults to empty.

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
| `500` | `INTERNAL_ERROR`, `NO_DATABASE`, `DB_ERROR`, `IO_ERROR`, `UPGRADE_FAILED` |

`DANGEROUS_SQL_DISABLED` is returned by `POST /api/sql` when the server was
started without `--allow-dangerous`. Restart `itr ui --allow-dangerous` to
enable raw SQL for the new session.

Unknown API routes return `404` with `code: "NOT_FOUND"` after token
validation. Missing or invalid tokens currently use `400` with
`code: "INVALID_VALUE"`. The request body limit is 1,048,576 bytes.
