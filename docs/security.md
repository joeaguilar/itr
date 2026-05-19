# Localhost UI security model

`itr ui` is a local-first browser editor for a local `.itr.db` file. It is not a
remote multi-user service. It has no user accounts, no role model, no TLS setup,
no daemon, and no network auth system beyond a per-process UI token.

## Network binding

The UI server binds to `127.0.0.1` only. It is intended to be reachable from the
same machine that started `itr ui`, not from the LAN or the public internet.

Do not put `itr ui` behind a reverse proxy, port forward, SSH tunnel, sharing
service, or container port mapping unless you are deliberately expanding the
trust boundary. The UI API can create and mutate issues in the selected database.

In sandboxed environments, starting or testing the UI may require permission to
bind or connect to `127.0.0.1`. A localhost permission failure is a sandbox
policy issue, not an `itr` authentication failure.

## Session token

Each `itr ui` process creates a fresh session token with SQLite
`lower(hex(randomblob(24)))`. The token is random process-local state and is not
stored as a user credential.

The startup URL includes the token:

```text
http://127.0.0.1:<port>/?token=<token>
```

The browser UI reads `token` from the URL and sends it on API requests as:

```text
X-ITR-Token: <token>
```

Server behavior:

- `GET /` requires the current token in the query string or `X-ITR-Token`.
- API routes require the current token in `X-ITR-Token` or the `token` query
  parameter.
- Two static asset routes are served without token checks: exactly
  `/assets/app.css` and `/assets/app.js`. The `/assets/` prefix is not a
  wildcard — every other path under `/assets/...` falls through to the
  token-protected dynamic router and returns `404` for unknown routes.
- Missing or wrong tokens are rejected before route-specific API behavior runs.
- Possession of the current token is authorization for that UI process.

This is bearer-token protection for a localhost tool. It is not a login session,
not an identity system, and not durable authentication.

See `docs/ui-api.md` for the full route inventory and `docs/architecture.md` for
where the UI server fits in the overall command dispatch.

## Local machine access

The localhost bind prevents direct network access from other machines, but any
process that can connect to `127.0.0.1:<port>` on the same machine can reach the
listener. The token is the practical protection against blind local requests.

Anyone who can see or recover the startup URL can use the UI while the process is
running. Treat the full URL as sensitive.

## URL, history, and logs

Because the token is in the startup URL, it may appear in places that record or
display URLs:

- browser history and address bars
- terminal scrollback or command logs that captured `itr ui` output
- screenshots, screen shares, recordings, or pasted support logs
- copied links, bookmarks, or chat messages

The UI sends `Referrer-Policy: no-referrer`, which reduces accidental referrer
leakage from pages served by `itr ui`. It does not protect a token that is copied,
logged, screenshotted, bookmarked, or otherwise shared.

Stop and restart `itr ui` to rotate the token.

## CSRF and cross-origin scope

The UI token makes blind CSRF impractical: a different site should not know the
random per-process token, and normal app requests use the non-simple
`X-ITR-Token` header.

The server does not check `Origin` or `Referer` headers on any request, and it
does not emit `Access-Control-Allow-Origin`. Cross-origin protection relies on
two browser behaviors:

- Custom request headers like `X-ITR-Token` are non-simple, so a cross-origin
  `fetch` from another web page triggers a CORS preflight. The `itr ui` server
  does not answer `OPTIONS` with a permissive CORS response, so the preflight
  fails and the browser blocks the real request before the token is ever sent.
- Without the token, requests that do reach the server (for example, a simple
  cross-origin `GET` to `/api/...` with no custom header) are rejected by the
  token check.

The server does not expose a CORS allowlist for other origins. Browser JavaScript
from another origin should not be able to read API responses or complete normal
custom-header API calls.

This is still a practical localhost boundary, not a hardened browser security
product. Browsers can send some cross-origin traffic to localhost, and if the
token leaks, the token becomes the authorization material. Do not browse with or
share a leaked UI URL and assume cross-origin rules will save it.

## Database file access

The `.itr.db` file is the source of truth. The UI token only protects the UI HTTP
API for the current process. It does not encrypt the database or protect the file
from local filesystem access.

Anyone with read access to `.itr.db` can inspect issue data outside the UI.
Anyone with write access can modify it outside the UI and bypass UI token checks.
Use normal OS filesystem permissions, workspace permissions, backups, and disk
security for database protection.

## Dangerous SQL Mode

`itr ui --allow-dangerous` enables a raw SQL editor and `POST /api/sql`. This is
full SQLite access to the opened database, including schema changes and data
loss operations. It is intentionally off by default, and the API returns `403`
when the flag is absent. SQL executed this way bypasses normal command helpers,
including validation, soft-fallback behavior, and audit/event recording.

Use this mode only for short local maintenance sessions. Take a backup first
when experimenting with mutating SQL:

```bash
cp .itr.db .itr.db.backup
itr ui --allow-dangerous --no-open
```

## Request body size limit

The UI server enforces a 1 MiB (`1_048_576` bytes) cap on request bodies. The
limit is read from `Content-Length` before any bytes are read off the socket,
and requests that declare a larger body are rejected with a `400` before the
body is consumed. This is a deliberate denial-of-service mitigation: a runaway
or malicious client cannot make the localhost listener allocate an unbounded
buffer or block a worker on a slow upload.

The cap applies to every route the server accepts a body for, including
`POST /api/issues`, note creation, dependency/relation writes, bulk resolve, and
the raw SQL endpoint when `--allow-dangerous` is on. Issue payloads in normal
use are well under this limit; the cap exists for safety, not for sizing.

## Response headers

Every response from `itr ui` (HTML, static assets, JSON, and error bodies)
includes two hardening headers:

- `X-Content-Type-Options: nosniff` — prevents browsers from second-guessing
  the declared `Content-Type` on UI responses, so a stored value that ends up
  in a response cannot be re-interpreted as a script or stylesheet.
- `Referrer-Policy: no-referrer` — pages served by `itr ui` do not send a
  `Referer` header on outgoing navigations or sub-resource requests, which
  reduces accidental token leakage when the tokenized URL is the current page.

These headers are emitted unconditionally by the response writer and apply to
both `200` and error responses. They harden how the browser treats the UI; they
do not authenticate requests.

## Operational guidance

Use `itr ui` for short local editing sessions. Prefer `--no-open` in remote,
sandboxed, CI, or shared-terminal contexts. Avoid exposing the port outside
localhost. Avoid sharing the tokenized URL. Stop the process when the editing
session is done.
