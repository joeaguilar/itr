use super::{build_issue_detail, build_issue_summary, sort_by_urgency_desc};
use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::{IssueDetail, IssueSummary};
use crate::normalize::{self, validate_kind, validate_priority, validate_status};
use crate::urgency::UrgencyConfig;
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

const INDEX_HTML: &str = include_str!("../ui_assets/index.html");
const APP_CSS: &str = include_str!("../ui_assets/app.css");
const APP_JS: &str = include_str!("../ui_assets/app.js");
const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_SQL_ROWS: usize = 500;
/// Cap on the HTTP request line (method + target + version) in bytes.
const MAX_REQUEST_LINE_BYTES: usize = 8_192;
/// Cap on a single header line in bytes.
const MAX_HEADER_LINE_BYTES: usize = 8_192;
/// Cap on the number of header lines per request.
const MAX_HEADER_COUNT: usize = 100;
/// Socket read/write timeout per accepted connection. Generous so slow CI
/// machines and human browsers never trip it, but bounded so one stalled
/// connection cannot wedge the (serial) accept loop forever.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

#[derive(Debug, Deserialize)]
struct IssueCreateInput {
    title: String,
    #[serde(default)]
    priority: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    context: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    acceptance: String,
    #[serde(default)]
    parent_id: Option<i64>,
    #[serde(default)]
    assigned_to: String,
    #[serde(default)]
    blocked_by: Vec<i64>,
}

#[derive(Debug, Deserialize)]
struct CloseInput {
    #[serde(default)]
    reason: String,
    #[serde(default)]
    wontfix: bool,
}

#[derive(Debug, Deserialize)]
struct NoteInput {
    content: String,
    #[serde(default)]
    agent: String,
}

#[derive(Debug, Deserialize)]
struct DependencyInput {
    blocker_id: i64,
}

#[derive(Debug, Deserialize)]
struct RelationInput {
    target_id: i64,
    #[serde(default = "default_relation_type")]
    relation_type: String,
}

#[derive(Debug, Deserialize)]
struct BulkResolveInput {
    ids: Vec<i64>,
    #[serde(default)]
    reason: String,
    #[serde(default)]
    wontfix: bool,
}

#[derive(Debug, Deserialize)]
struct SqlInput {
    sql: String,
}

fn default_relation_type() -> String {
    "related".to_string()
}

pub fn run(
    conn: &Connection,
    db_path: &Path,
    port: u16,
    no_open: bool,
    once: bool,
    allow_dangerous: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let token = session_token(conn)?;
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let addr = listener.local_addr()?;
    let url = format!("http://{}:{}/?token={}", addr.ip(), addr.port(), token);

    if fmt.is_json() {
        println!(
            "{}",
            json!({
                "url": url,
                "db_path": db_path.display().to_string(),
                "port": addr.port(),
            })
        );
    } else {
        println!("UI: {}", url);
        println!("DB: {}", db_path.display());
    }
    std::io::stdout().flush()?;

    if allow_dangerous {
        eprintln!(
            "REVIEW: raw SQL UI is enabled for {}. Treat this session as full database access.",
            db_path.display()
        );
    }

    if !no_open && !once {
        if let Err(err) = open_browser(&url) {
            eprintln!("REVIEW: could not open browser: {}", err);
        }
    }

    serve(
        &listener,
        conn,
        db_path,
        &token,
        allow_dangerous,
        once,
        IO_TIMEOUT,
        addr.port(),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn serve(
    listener: &TcpListener,
    conn: &Connection,
    db_path: &Path,
    token: &str,
    allow_dangerous: bool,
    once: bool,
    io_timeout: Duration,
    port: u16,
) {
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                // Bound both directions so one stalled connection cannot
                // wedge the serial accept loop indefinitely.
                let _ = stream.set_read_timeout(Some(io_timeout));
                let _ = stream.set_write_timeout(Some(io_timeout));
                // Isolate per-connection handling: a panic while parsing or
                // routing one request must not abort the accept loop.
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    handle_stream(&mut stream, conn, db_path, token, allow_dangerous, port)
                }));
                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(err)) => {
                        let response = error_response(500, &err.to_string(), "INTERNAL_ERROR");
                        let _ = write_response(&mut stream, response);
                    }
                    Err(_) => {
                        eprintln!("REVIEW: UI request handler panicked; connection dropped");
                        let response =
                            error_response(500, "request handler panicked", "INTERNAL_ERROR");
                        let _ = write_response(&mut stream, response);
                    }
                }
                if once {
                    break;
                }
            }
            Err(err) => {
                eprintln!("REVIEW: UI request failed: {}", err);
                if once {
                    break;
                }
            }
        }
    }
}

fn session_token(conn: &Connection) -> Result<String, ItrError> {
    Ok(conn.query_row("SELECT lower(hex(randomblob(24)))", [], |row| row.get(0))?)
}

/// Errors raised while reading a request off the socket, before routing.
#[derive(Debug)]
enum RequestError {
    /// Socket-level failure (including read timeouts).
    Io(std::io::Error),
    /// Malformed request -> 400.
    Bad(String),
    /// Request line / headers exceed the configured caps -> 431.
    TooLarge(String),
}

impl From<std::io::Error> for RequestError {
    fn from(err: std::io::Error) -> Self {
        RequestError::Io(err)
    }
}

fn handle_stream(
    stream: &mut TcpStream,
    conn: &Connection,
    db_path: &Path,
    token: &str,
    allow_dangerous: bool,
    port: u16,
) -> Result<(), ItrError> {
    let response = match read_request(stream) {
        Ok(request) => match host_rejection(&request, port) {
            Some(rejection) => rejection,
            None => match route_request(&request, conn, db_path, token, allow_dangerous) {
                Ok(response) => response,
                Err(err) => error_response_for_itr(err),
            },
        },
        Err(RequestError::Bad(message)) => error_response(400, &message, "BAD_REQUEST"),
        Err(RequestError::TooLarge(message)) => error_response(431, &message, "REQUEST_TOO_LARGE"),
        Err(RequestError::Io(err))
            if matches!(
                err.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            error_response(408, "request read timed out", "REQUEST_TIMEOUT")
        }
        Err(RequestError::Io(err)) => error_response(400, &err.to_string(), "BAD_REQUEST"),
    };
    write_response(stream, response)?;
    Ok(())
}

/// Result of one bounded line read.
enum LineRead {
    /// A full line (terminator included) within the byte limit.
    Line(String),
    /// Clean EOF before any byte of this line arrived.
    Eof,
    /// The line exceeded the byte limit; reading stopped without buffering it.
    TooLong,
}

/// Reads a single `\n`-terminated line without ever buffering more than
/// `limit` bytes, unlike `BufRead::read_line` which allocates unboundedly.
fn read_line_limited<R: BufRead>(reader: &mut R, limit: usize) -> std::io::Result<LineRead> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let (consumed, done) = {
            let buf = reader.fill_buf()?;
            if buf.is_empty() {
                return Ok(if line.is_empty() {
                    LineRead::Eof
                } else {
                    LineRead::Line(String::from_utf8_lossy(&line).to_string())
                });
            }
            match buf.iter().position(|&b| b == b'\n') {
                Some(pos) => {
                    if line.len() + pos + 1 > limit {
                        return Ok(LineRead::TooLong);
                    }
                    line.extend_from_slice(&buf[..=pos]);
                    (pos + 1, true)
                }
                None => {
                    if line.len() + buf.len() > limit {
                        return Ok(LineRead::TooLong);
                    }
                    line.extend_from_slice(buf);
                    (buf.len(), false)
                }
            }
        };
        reader.consume(consumed);
        if done {
            return Ok(LineRead::Line(String::from_utf8_lossy(&line).to_string()));
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, RequestError> {
    let mut reader = BufReader::new(stream);
    let start_line = match read_line_limited(&mut reader, MAX_REQUEST_LINE_BYTES)? {
        LineRead::Line(line) => line,
        LineRead::Eof => return Err(RequestError::Bad("empty HTTP request".to_string())),
        LineRead::TooLong => {
            return Err(RequestError::TooLarge(format!(
                "request line exceeds {} bytes",
                MAX_REQUEST_LINE_BYTES
            )))
        }
    };
    if start_line.trim().is_empty() {
        return Err(RequestError::Bad(
            "empty HTTP request line (expected: METHOD path HTTP/version)".to_string(),
        ));
    }

    let mut parts = start_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default();
    if method.is_empty() || target.is_empty() {
        return Err(RequestError::Bad(format!(
            "malformed request line: {} (expected: METHOD path HTTP/version)",
            start_line.trim()
        )));
    }

    let mut headers = HashMap::new();
    let mut header_count = 0usize;
    loop {
        let line = match read_line_limited(&mut reader, MAX_HEADER_LINE_BYTES)? {
            LineRead::Line(line) => line,
            LineRead::Eof => break,
            LineRead::TooLong => {
                return Err(RequestError::TooLarge(format!(
                    "header line exceeds {} bytes",
                    MAX_HEADER_LINE_BYTES
                )))
            }
        };
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        header_count += 1;
        if header_count > MAX_HEADER_COUNT {
            return Err(RequestError::TooLarge(format!(
                "more than {} request headers",
                MAX_HEADER_COUNT
            )));
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let content_length = headers
        .get("content-length")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return Err(RequestError::Bad(format!(
            "request body of {} bytes exceeds maximum {} bytes",
            content_length, MAX_BODY_BYTES
        )));
    }

    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let (path, query) = parse_target(target);
    Ok(HttpRequest {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn parse_target(target: &str) -> (String, HashMap<String, String>) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    (url_decode(path), parse_query(query))
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        values.insert(url_decode(key), url_decode(value));
    }
    values
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                // Decode on raw bytes only: slicing `input` as a &str here can
                // panic when the offset lands inside a multibyte UTF-8 char.
                let decoded = bytes.get(i + 1..i + 3).and_then(|hex| {
                    let hex = std::str::from_utf8(hex).ok()?;
                    u8::from_str_radix(hex, 16).ok()
                });
                if let Some(value) = decoded {
                    out.push(value);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).to_string()
}

fn route_request(
    request: &HttpRequest,
    conn: &Connection,
    db_path: &Path,
    token: &str,
    allow_dangerous: bool,
) -> Result<HttpResponse, ItrError> {
    match (request.method.as_str(), request.path.as_str()) {
        // Test-only route used to verify that a panic in one connection's
        // handler cannot abort the accept loop. Compiled out of release/debug
        // binaries; only exists under `cargo test`.
        #[cfg(test)]
        ("GET", "/__test/panic") => panic!("deliberate test panic"),
        ("GET", "/") => {
            require_token(request, token)?;
            Ok(html_response(INDEX_HTML))
        }
        ("GET", "/assets/app.css") => Ok(response(200, "text/css; charset=utf-8", APP_CSS)),
        ("GET", "/assets/app.js") => Ok(response(
            200,
            "application/javascript; charset=utf-8",
            APP_JS,
        )),
        ("GET", "/api/health") => {
            require_token(request, token)?;
            json_response(json!({
                "ok": true,
                "db_path": db_path.display().to_string(),
                "version": env!("ITR_VERSION"),
            }))
        }
        ("GET", "/api/bootstrap") => {
            require_token(request, token)?;
            json_response(json!({
                "db_path": db_path.display().to_string(),
                "version": env!("ITR_VERSION"),
                "statuses": ["open", "in-progress", "done", "wontfix"],
                "priorities": ["critical", "high", "medium", "low"],
                "kinds": ["bug", "feature", "task", "epic"],
                "dangerous_sql": allow_dangerous,
                "stats": stats_value(conn)?,
            }))
        }
        ("GET", "/api/issues") => {
            require_token(request, token)?;
            if let Some(raw_ids) = request.query.get("ids") {
                // Batched detail fetch (#136): `?ids=1,2,3` switches the
                // response from summaries to full IssueDetail records. Other
                // filter parameters are ignored in this mode.
                let (details, missing) = batched_issue_details(conn, raw_ids)?;
                json_response(json!({
                    "total": details.len(),
                    "issues": details,
                    "missing": missing,
                }))
            } else {
                let issues = list_issue_summaries(conn, &request.query)?;
                json_response(json!({
                    "total": issues.len(),
                    "issues": issues,
                }))
            }
        }
        ("POST", "/api/sql") => {
            require_token(request, token)?;
            if !allow_dangerous {
                return Ok(error_response(
                    403,
                    "Raw SQL requires starting itr ui with --allow-dangerous",
                    "DANGEROUS_SQL_DISABLED",
                ));
            }
            let input: SqlInput = parse_body(request)?;
            json_response(run_sql(conn, &input.sql)?)
        }
        ("POST", "/api/issues") => {
            require_token(request, token)?;
            let input: IssueCreateInput = parse_body(request)?;
            let detail = create_issue(conn, input)?;
            json_response(json!({ "issue": detail }))
        }
        ("POST", "/api/bulk/resolve/preview") => {
            require_token(request, token)?;
            let input: BulkResolveInput = parse_body(request)?;
            let issues = selected_issue_summaries(conn, &input.ids)?;
            json_response(json!({
                "count": issues.len(),
                "issues": issues,
                "target_status": if input.wontfix { "wontfix" } else { "done" },
            }))
        }
        ("POST", "/api/bulk/resolve/apply") => {
            require_token(request, token)?;
            let input: BulkResolveInput = parse_body(request)?;
            let mut resolved = Vec::new();
            let mut unblocked = Vec::new();
            for id in input.ids {
                let result = resolve_issue(conn, id, &input.reason, input.wontfix)?;
                if let Some(items) = result.get("unblocked").and_then(Value::as_array) {
                    unblocked.extend(items.iter().cloned());
                }
                resolved.push(result["issue"].clone());
            }
            json_response(json!({
                "count": resolved.len(),
                "issues": resolved,
                "unblocked": unblocked,
            }))
        }
        _ => route_dynamic(request, conn, token),
    }
}

fn route_dynamic(
    request: &HttpRequest,
    conn: &Connection,
    token: &str,
) -> Result<HttpResponse, ItrError> {
    require_token(request, token)?;
    let segments: Vec<&str> = request
        .path
        .trim_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();

    match (request.method.as_str(), segments.as_slice()) {
        ("GET", ["api", "issues", id]) => {
            let id = parse_id(id, "id")?;
            json_response(json!({ "issue": issue_detail(conn, id)? }))
        }
        ("PATCH", ["api", "issues", id]) => {
            let id = parse_id(id, "id")?;
            let patch: Value = parse_body(request)?;
            let detail = patch_issue(conn, id, &patch)?;
            json_response(json!({ "issue": detail }))
        }
        ("POST", ["api", "issues", id, "close"]) => {
            let id = parse_id(id, "id")?;
            let input: CloseInput = parse_body(request)?;
            json_response(resolve_issue(conn, id, &input.reason, input.wontfix)?)
        }
        ("POST", ["api", "issues", id, "notes"]) => {
            let id = parse_id(id, "id")?;
            let input: NoteInput = parse_body(request)?;
            let note = db::add_note(conn, id, &input.content, &input.agent)?;
            json_response(json!({ "note": note, "issue": issue_detail(conn, id)? }))
        }
        ("PATCH", ["api", "notes", id]) => {
            let id = parse_id(id, "id")?;
            let input: NoteInput = parse_body(request)?;
            let old_note = db::get_note(conn, id)?;
            db::record_event(
                conn,
                old_note.issue_id,
                "note_updated",
                &old_note.content,
                &input.content,
            )?;
            let note = db::update_note(conn, id, &input.content)?;
            json_response(json!({
                "note": note,
                "issue": issue_detail(conn, old_note.issue_id)?,
            }))
        }
        ("DELETE", ["api", "notes", id]) => {
            let id = parse_id(id, "id")?;
            let note = db::delete_note(conn, id)?;
            db::record_event(conn, note.issue_id, "note_deleted", &note.content, "")?;
            json_response(json!({
                "note": note,
                "issue": issue_detail(conn, note.issue_id)?,
            }))
        }
        ("POST", ["api", "issues", id, "dependencies"]) => {
            let id = parse_id(id, "id")?;
            let input: DependencyInput = parse_body(request)?;
            let created = db::add_dependency(conn, input.blocker_id, id)?;
            json_response(json!({
                "created": created,
                "issue": issue_detail(conn, id)?,
            }))
        }
        ("DELETE", ["api", "issues", id, "dependencies", blocker_id]) => {
            let id = parse_id(id, "id")?;
            let blocker_id = parse_id(blocker_id, "blocker_id")?;
            let removed = db::remove_dependency(conn, blocker_id, id)?;
            json_response(json!({
                "removed": removed,
                "issue": issue_detail(conn, id)?,
            }))
        }
        ("POST", ["api", "issues", id, "relations"]) => {
            let id = parse_id(id, "id")?;
            let input: RelationInput = parse_body(request)?;
            validate_relation_type(&input.relation_type)?;
            let created = db::add_relation(conn, id, input.target_id, &input.relation_type)?;
            json_response(json!({
                "created": created,
                "issue": issue_detail(conn, id)?,
            }))
        }
        ("DELETE", ["api", "issues", id, "relations", target_id]) => {
            let id = parse_id(id, "id")?;
            let target_id = parse_id(target_id, "target_id")?;
            let removed = db::remove_relation(conn, id, target_id, None)?;
            json_response(json!({
                "removed": !removed.is_empty(),
                "issue": issue_detail(conn, id)?,
            }))
        }
        _ => Ok(error_response(404, "Route not found", "NOT_FOUND")),
    }
}

fn require_token(request: &HttpRequest, token: &str) -> Result<(), ItrError> {
    let supplied = request
        .headers
        .get("x-itr-token")
        .or_else(|| request.query.get("token"));
    if supplied.is_some_and(|value| constant_time_eq(value, token)) {
        return Ok(());
    }
    Err(ItrError::InvalidValue {
        field: "token".to_string(),
        value: String::new(),
        valid: "current UI session token".to_string(),
    })
}

/// Compares two strings in time independent of where they first differ.
/// Accumulates XOR differences over `max(len_a, len_b)` bytes (reading 0 past
/// the end of the shorter input) and folds the length difference into the
/// result, so neither a byte mismatch nor a length mismatch exits early.
/// Closes the theoretical timing side channel of `==` on session tokens.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let byte_a = a.get(i).copied().unwrap_or(0);
        let byte_b = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(byte_a ^ byte_b);
    }
    diff == 0
}

/// Returns true when the Host header names this loopback server: `127.0.0.1`,
/// `localhost`, or `[::1]`, each optionally suffixed with the bound port.
/// Anything else — e.g. an attacker-controlled hostname that resolves to
/// 127.0.0.1 (DNS rebinding) — is rejected before routing.
fn host_allowed(host: &str, port: u16) -> bool {
    let host = host.trim();
    let (name, host_port) = if host.starts_with('[') {
        // Bracketed IPv6 literal: `[::1]` or `[::1]:port`.
        let Some(end) = host.find(']') else {
            return false;
        };
        let remainder = &host[end + 1..];
        let port_part = if remainder.is_empty() {
            None
        } else {
            match remainder.strip_prefix(':') {
                Some(part) => Some(part),
                None => return false,
            }
        };
        (&host[..=end], port_part)
    } else {
        match host.rsplit_once(':') {
            Some((name, part)) => (name, Some(part)),
            None => (host, None),
        }
    };
    let name_allowed = name.eq_ignore_ascii_case("127.0.0.1")
        || name.eq_ignore_ascii_case("localhost")
        || name.eq_ignore_ascii_case("[::1]");
    match host_port {
        None => name_allowed,
        Some(part) => name_allowed && part.parse::<u16>().ok() == Some(port),
    }
}

/// Builds the 4xx rejection for a request whose Host header is missing or not
/// a loopback name. Runs before routing and before the token check: a valid
/// token must not rescue a cross-origin (DNS-rebinding style) request. A
/// missing Host header is also rejected — every real browser sends one, so
/// only non-browser HTTP/1.0-style probes lose access.
fn host_rejection(request: &HttpRequest, port: u16) -> Option<HttpResponse> {
    match request.headers.get("host") {
        None => Some(error_response(
            400,
            "missing Host header (the itr UI only answers 127.0.0.1, localhost, or [::1])",
            "HOST_NOT_ALLOWED",
        )),
        Some(host) if !host_allowed(host, port) => Some(error_response(
            403,
            &format!(
                "Host '{}' is not allowed (the itr UI only answers 127.0.0.1, localhost, or [::1])",
                host
            ),
            "HOST_NOT_ALLOWED",
        )),
        Some(_) => None,
    }
}

fn parse_body<T: serde::de::DeserializeOwned>(request: &HttpRequest) -> Result<T, ItrError> {
    Ok(serde_json::from_slice(&request.body)?)
}

fn parse_id(value: &str, field: &str) -> Result<i64, ItrError> {
    value.parse::<i64>().map_err(|_| ItrError::InvalidValue {
        field: field.to_string(),
        value: value.to_string(),
        valid: "integer issue id".to_string(),
    })
}

fn run_sql(conn: &Connection, sql: &str) -> Result<Value, ItrError> {
    let sql = sql.trim();
    if sql.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "sql".to_string(),
            value: String::new(),
            valid: "non-empty SQL statement".to_string(),
        });
    }

    let before_changes = total_changes(conn)?;
    let mut statement = conn.prepare(sql)?;
    let column_count = statement.column_count();

    if column_count == 0 {
        drop(statement);
        conn.execute_batch(sql)?;
        let changes = total_changes(conn)?.saturating_sub(before_changes);
        return Ok(json!({
            "columns": [],
            "rows": [],
            "row_count": 0,
            "truncated": false,
            "changes": changes,
        }));
    }

    let columns: Vec<String> = statement
        .column_names()
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    let mut rows = statement.query([])?;
    let mut result_rows = Vec::new();
    let mut row_count = 0_i64;
    let mut truncated = false;

    while let Some(row) = rows.next()? {
        if result_rows.len() < MAX_SQL_ROWS {
            let mut values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                values.push(sql_value_to_json(row.get_ref(index)?));
            }
            result_rows.push(Value::Array(values));
        } else {
            truncated = true;
        }
        row_count += 1;
    }

    let changes = total_changes(conn)?.saturating_sub(before_changes);
    Ok(json!({
        "columns": columns,
        "rows": result_rows,
        "row_count": row_count,
        "truncated": truncated,
        "changes": changes,
    }))
}

fn total_changes(conn: &Connection) -> Result<i64, ItrError> {
    Ok(conn.query_row("SELECT total_changes()", [], |row| row.get(0))?)
}

fn sql_value_to_json(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => Value::String(String::from_utf8_lossy(value).to_string()),
        ValueRef::Blob(value) => Value::String(format!("x'{}'", hex_encode(value))),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn issue_detail(conn: &Connection, id: i64) -> Result<IssueDetail, ItrError> {
    let issue = db::get_issue(conn, id)?;
    let config = UrgencyConfig::load(conn);
    let mut detail = build_issue_detail(conn, issue, &config)?;
    detail.children = Some(
        db::list_issues(
            conn,
            &crate::models::ListFilter {
                parent_id: Some(id),
                all: true,
                include_blocked: true,
                ..crate::models::ListFilter::default()
            },
        )?
        .iter()
        .map(|issue| build_issue_summary(conn, issue, &config))
        .collect(),
    );
    detail.relations = db::get_relations(conn, id)?;
    Ok(detail)
}

fn create_issue(conn: &Connection, input: IssueCreateInput) -> Result<IssueDetail, ItrError> {
    let title = input.title.trim();
    if title.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "title".to_string(),
            value: String::new(),
            valid: "non-empty string".to_string(),
        });
    }

    let priority = normalize::normalize_priority(input.priority.as_deref().unwrap_or("medium"));
    let kind = normalize::normalize_kind(input.kind.as_deref().unwrap_or("task"));
    let mut tags = clean_list(input.tags, false);
    let skills = clean_list(input.skills, true);
    let files = clean_list(input.files, false);
    let mut review_notes = Vec::new();

    let priority = match validate_priority(&priority) {
        Ok(()) => priority,
        Err(_) => {
            review_notes.push(format!(
                "REVIEW: priority '{}' not recognized, defaulted to 'medium'. Valid: critical, high, medium, low",
                priority
            ));
            "medium".to_string()
        }
    };
    let kind = match validate_kind(&kind) {
        Ok(()) => kind,
        Err(_) => {
            review_notes.push(format!(
                "REVIEW: kind '{}' not recognized, defaulted to 'task'. Valid: bug, feature, task, epic",
                kind
            ));
            "task".to_string()
        }
    };

    if !review_notes.is_empty() && !tags.contains(&"_needs_review".to_string()) {
        tags.push("_needs_review".to_string());
    }

    let issue = db::insert_issue(
        conn,
        title,
        &priority,
        &kind,
        &input.context,
        &files,
        &tags,
        &skills,
        &input.acceptance,
        input.parent_id,
        &input.assigned_to,
    )?;

    for note in review_notes {
        db::add_note(conn, issue.id, &note, "itr")?;
    }
    for blocker_id in input.blocked_by {
        db::add_dependency(conn, blocker_id, issue.id)?;
    }

    issue_detail(conn, issue.id)
}

fn patch_issue(conn: &Connection, id: i64, patch: &Value) -> Result<IssueDetail, ItrError> {
    // Single transaction: a failure on any field rolls back the whole patch.
    let tx = conn.unchecked_transaction()?;
    let old_issue = db::get_issue(&tx, id)?;

    patch_string_field(&tx, id, patch, "title", "title", &old_issue.title)?;
    patch_string_field(&tx, id, patch, "context", "context", &old_issue.context)?;
    patch_string_field(
        &tx,
        id,
        patch,
        "acceptance",
        "acceptance",
        &old_issue.acceptance,
    )?;
    patch_string_field(
        &tx,
        id,
        patch,
        "assigned_to",
        "assigned_to",
        &old_issue.assigned_to,
    )?;
    patch_string_field(
        &tx,
        id,
        patch,
        "close_reason",
        "close_reason",
        &old_issue.close_reason,
    )?;

    if let Some(value) = patch.get("status").and_then(Value::as_str) {
        let status = normalize::normalize_status(value);
        let status = match validate_status(&status) {
            Ok(()) => status,
            Err(_) => "open".to_string(),
        };
        db::record_event(&tx, id, "status", &old_issue.status, &status)?;
        db::update_issue_field(&tx, id, "status", &status)?;
    }
    if let Some(value) = patch.get("priority").and_then(Value::as_str) {
        let priority = normalize::normalize_priority(value);
        let priority = match validate_priority(&priority) {
            Ok(()) => priority,
            Err(_) => "medium".to_string(),
        };
        db::record_event(&tx, id, "priority", &old_issue.priority, &priority)?;
        db::update_issue_field(&tx, id, "priority", &priority)?;
    }
    if let Some(value) = patch.get("kind").and_then(Value::as_str) {
        let kind = normalize::normalize_kind(value);
        let kind = match validate_kind(&kind) {
            Ok(()) => kind,
            Err(_) => "task".to_string(),
        };
        db::record_event(&tx, id, "kind", &old_issue.kind, &kind)?;
        db::update_issue_field(&tx, id, "kind", &kind)?;
    }

    patch_array_field(&tx, id, patch, "files", &old_issue.files, false)?;
    patch_array_field(&tx, id, patch, "tags", &old_issue.tags, false)?;
    patch_array_field(&tx, id, patch, "skills", &old_issue.skills, true)?;

    if let Some(parent_value) = patch.get("parent_id") {
        let parent_id = if parent_value.is_null() {
            None
        } else {
            Some(
                parent_value
                    .as_i64()
                    .ok_or_else(|| ItrError::InvalidValue {
                        field: "parent_id".to_string(),
                        value: parent_value.to_string(),
                        valid: "integer issue id or null".to_string(),
                    })?,
            )
        };
        db::record_event(
            &tx,
            id,
            "parent_id",
            &old_issue
                .parent_id
                .map(|value| value.to_string())
                .unwrap_or_default(),
            &parent_id.map(|value| value.to_string()).unwrap_or_default(),
        )?;
        db::update_issue_parent(&tx, id, parent_id)?;
    }

    let detail = issue_detail(&tx, id)?;
    tx.commit()?;
    Ok(detail)
}

fn patch_string_field(
    conn: &Connection,
    id: i64,
    patch: &Value,
    json_name: &str,
    db_name: &str,
    old_value: &str,
) -> Result<(), ItrError> {
    if let Some(value) = patch.get(json_name).and_then(Value::as_str) {
        db::record_event(conn, id, db_name, old_value, value)?;
        db::update_issue_field(conn, id, db_name, value)?;
    }
    Ok(())
}

fn patch_array_field(
    conn: &Connection,
    id: i64,
    patch: &Value,
    field: &str,
    old_values: &[String],
    lowercase: bool,
) -> Result<(), ItrError> {
    let Some(value) = patch.get(field) else {
        return Ok(());
    };
    let values: Vec<String> = serde_json::from_value(value.clone())?;
    let values = clean_list(values, lowercase);
    let old_json = serde_json::to_string(old_values)?;
    let new_json = serde_json::to_string(&values)?;
    db::record_event(conn, id, field, &old_json, &new_json)?;
    db::update_issue_field(conn, id, field, &new_json)?;
    Ok(())
}

fn clean_list(values: Vec<String>, lowercase: bool) -> Vec<String> {
    let mut cleaned = Vec::new();
    for value in values {
        let value = if lowercase {
            value.trim().to_lowercase()
        } else {
            value.trim().to_string()
        };
        if !value.is_empty() && !cleaned.contains(&value) {
            cleaned.push(value);
        }
    }
    cleaned
}

fn resolve_issue(
    conn: &Connection,
    id: i64,
    reason: &str,
    wontfix: bool,
) -> Result<Value, ItrError> {
    // Single transaction (mirrors `itr close`): a mid-resolve failure leaves
    // the issue fully unchanged — no stray events, status flip, or lost edges.
    let status = if wontfix { "wontfix" } else { "done" };
    let tx = conn.unchecked_transaction()?;
    let old_issue = db::get_issue(&tx, id)?;
    db::record_event(&tx, id, "status", &old_issue.status, status)?;
    db::update_issue_field(&tx, id, "status", status)?;
    if !reason.trim().is_empty() {
        db::record_event(&tx, id, "close_reason", &old_issue.close_reason, reason)?;
        db::update_issue_field(&tx, id, "close_reason", reason)?;
    }
    let unblocked = db::get_newly_unblocked(&tx, id)?;
    db::remove_blocker_edges(&tx, id)?;
    let detail = issue_detail(&tx, id)?;
    tx.commit()?;
    Ok(json!({
        "issue": detail,
        "unblocked": unblocked
            .into_iter()
            .map(|(uid, title)| json!({ "id": uid, "title": title }))
            .collect::<Vec<_>>(),
    }))
}

fn list_issue_summaries(
    conn: &Connection,
    query: &HashMap<String, String>,
) -> Result<Vec<IssueSummary>, ItrError> {
    let config = UrgencyConfig::load(conn);
    let all = query_bool(query, "all");
    let ready = query_bool(query, "ready");
    let blocked_only = query_bool(query, "blocked");
    let terms: Vec<String> = query
        .get("q")
        .map(|q| q.split_whitespace().map(str::to_lowercase).collect())
        .unwrap_or_default();
    let statuses = query_list(query, "status");
    let priorities = query_list(query, "priority");
    let kinds = query_list(query, "kind");
    let tags = query_list(query, "tag");
    let tag_any = query_list(query, "tag_any");
    let skills = query_list(query, "skill");
    let assigned_to = query.get("assigned_to").filter(|s| !s.is_empty());

    let mut summaries = Vec::new();
    for issue in db::all_issues(conn)? {
        if !all && statuses.is_empty() && issue.status != "open" && issue.status != "in-progress" {
            continue;
        }
        if !statuses.is_empty() && !statuses.contains(&issue.status) {
            continue;
        }
        if !priorities.is_empty() && !priorities.contains(&issue.priority) {
            continue;
        }
        if !kinds.is_empty() && !kinds.contains(&issue.kind) {
            continue;
        }
        if !tags.is_empty() && !tags.iter().all(|tag| issue.tags.contains(tag)) {
            continue;
        }
        if !tag_any.is_empty() && !tag_any.iter().any(|tag| issue.tags.contains(tag)) {
            continue;
        }
        if !skills.is_empty() && !skills.iter().all(|skill| issue.skills.contains(skill)) {
            continue;
        }
        if assigned_to.is_some_and(|agent| issue.assigned_to != *agent) {
            continue;
        }

        let is_blocked = db::is_blocked(conn, issue.id)?;
        if blocked_only && !is_blocked {
            continue;
        }
        if ready && (is_blocked || issue.status == "done" || issue.status == "wontfix") {
            continue;
        }
        if !terms.is_empty() && !issue_matches_terms(conn, &issue, &terms)? {
            continue;
        }

        summaries.push(build_issue_summary(conn, &issue, &config));
    }

    match query.get("sort").map_or("urgency", String::as_str) {
        "created" => summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
        "updated" => summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
        "id" => summaries.sort_by(|a, b| a.id.cmp(&b.id)),
        "priority" => summaries.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.id.cmp(&b.id))),
        _ => sort_by_urgency_desc(&mut summaries),
    }

    if let Some(limit) = query.get("limit").and_then(|v| v.parse::<usize>().ok()) {
        summaries.truncate(limit);
    }
    Ok(summaries)
}

/// Resolve a comma-separated `ids` query value into full issue details
/// (#136). Duplicate IDs are fetched once (first-seen order); IDs that are
/// valid integers but not in the database are reported in the returned
/// `missing` list instead of failing the whole batch. A non-integer token is
/// a hard 400 `INVALID_VALUE`, consistent with path-segment ID parsing on the
/// other routes.
fn batched_issue_details(
    conn: &Connection,
    raw_ids: &str,
) -> Result<(Vec<IssueDetail>, Vec<i64>), ItrError> {
    let mut ids: Vec<i64> = Vec::new();
    for token in raw_ids.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let id = parse_id(token, "ids")?;
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut details = Vec::with_capacity(ids.len());
    let mut missing = Vec::new();
    for id in ids {
        match issue_detail(conn, id) {
            Ok(detail) => details.push(detail),
            Err(ItrError::NotFound(_)) => missing.push(id),
            Err(err) => return Err(err),
        }
    }
    Ok((details, missing))
}

fn selected_issue_summaries(conn: &Connection, ids: &[i64]) -> Result<Vec<IssueSummary>, ItrError> {
    let config = UrgencyConfig::load(conn);
    ids.iter()
        .map(|id| db::get_issue(conn, *id).map(|issue| build_issue_summary(conn, &issue, &config)))
        .collect()
}

fn issue_matches_terms(
    conn: &Connection,
    issue: &crate::models::Issue,
    terms: &[String],
) -> Result<bool, ItrError> {
    let notes = db::get_notes(conn, issue.id)?;
    let haystacks = [
        issue.title.as_str(),
        issue.context.as_str(),
        issue.acceptance.as_str(),
        issue.close_reason.as_str(),
        issue.assigned_to.as_str(),
    ];
    Ok(terms.iter().all(|term| {
        haystacks
            .iter()
            .any(|value| value.to_lowercase().contains(term))
            || issue
                .tags
                .iter()
                .chain(issue.files.iter())
                .chain(issue.skills.iter())
                .any(|value| value.to_lowercase().contains(term))
            || notes
                .iter()
                .any(|note| note.content.to_lowercase().contains(term))
    }))
}

fn query_bool(query: &HashMap<String, String>, key: &str) -> bool {
    query
        .get(key)
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
}

fn query_list(query: &HashMap<String, String>, key: &str) -> Vec<String> {
    query
        .get(key)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn stats_value(conn: &Connection) -> Result<Value, ItrError> {
    let issues = db::all_issues(conn)?;
    let total = issues.len();
    let open = issues
        .iter()
        .filter(|issue| issue.status == "open" || issue.status == "in-progress")
        .count();
    let done = issues.iter().filter(|issue| issue.status == "done").count();
    let wontfix = issues
        .iter()
        .filter(|issue| issue.status == "wontfix")
        .count();
    let mut blocked = 0;
    for issue in &issues {
        if db::is_blocked(conn, issue.id)? {
            blocked += 1;
        }
    }
    Ok(json!({
        "total": total,
        "active": open,
        "done": done,
        "wontfix": wontfix,
        "blocked": blocked,
        "ready": open.saturating_sub(blocked),
    }))
}

fn validate_relation_type(value: &str) -> Result<(), ItrError> {
    if matches!(value, "duplicate" | "related" | "supersedes") {
        Ok(())
    } else {
        Err(ItrError::InvalidValue {
            field: "relation_type".to_string(),
            value: value.to_string(),
            valid: "duplicate, related, supersedes".to_string(),
        })
    }
}

fn html_response(body: &str) -> HttpResponse {
    response(200, "text/html; charset=utf-8", body)
}

fn json_response(value: Value) -> Result<HttpResponse, ItrError> {
    Ok(HttpResponse {
        status: 200,
        content_type: "application/json; charset=utf-8",
        body: serde_json::to_vec(&value)?,
    })
}

fn response(status: u16, content_type: &'static str, body: &str) -> HttpResponse {
    HttpResponse {
        status,
        content_type,
        body: body.as_bytes().to_vec(),
    }
}

fn error_response_for_itr(err: ItrError) -> HttpResponse {
    let status = match err {
        ItrError::NotFound(_) => 404,
        ItrError::InvalidValue { .. } | ItrError::Parse(_) | ItrError::NoFilters => 400,
        ItrError::CycleDetected(_) => 409,
        ItrError::NoDatabase | ItrError::Db(_) | ItrError::Io(_) | ItrError::UpgradeFailed(_) => {
            500
        }
    };
    let code = err.error_code();
    error_response(status, &err.to_string(), code)
}

fn error_response(status: u16, message: &str, code: &str) -> HttpResponse {
    HttpResponse {
        status,
        content_type: "application/json; charset=utf-8",
        body: json!({
            "error": message,
            "code": code,
        })
        .to_string()
        .into_bytes(),
    }
}

fn write_response(stream: &mut TcpStream, response: HttpResponse) -> Result<(), ItrError> {
    let status_text = match response.status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        409 => "Conflict",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\n\r\n",
        response.status,
        status_text,
        response.content_type,
        response.body.len()
    )?;
    stream.write_all(&response.body)?;
    Ok(())
}

fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Instant;

    const TEST_TOKEN: &str = "itr-ui-test-token";

    /// Starts the real serve loop on an ephemeral port in a detached thread.
    /// Only routes that never touch the database are exercised, so a bare
    /// in-memory connection is sufficient.
    fn spawn_test_server(io_timeout: Duration) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let conn = Connection::open_in_memory().expect("open in-memory db");
            serve(
                &listener,
                &conn,
                Path::new(":memory:"),
                TEST_TOKEN,
                false,
                false,
                io_timeout,
                addr.port(),
            );
        });
        addr
    }

    fn send_raw(addr: SocketAddr, request: &[u8]) -> String {
        let mut stream = TcpStream::connect(addr).expect("connect to test server");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("set client read timeout");
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .expect("set client write timeout");
        stream.write_all(request).expect("write request");
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
        response
    }

    fn health_check(addr: SocketAddr) -> String {
        send_raw(
            addr,
            format!(
                "GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ITR-Token: {}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN
            )
            .as_bytes(),
        )
    }

    #[test]
    fn url_decode_survives_percent_before_multibyte_char() {
        // Regression (#155): slicing the &str at byte offsets after '%' used
        // to panic with "byte index 4 is not a char boundary".
        assert_eq!(url_decode("/%\u{20ac}"), "/%\u{20ac}");
        assert_eq!(url_decode("%\u{e9}"), "%\u{e9}");
        assert_eq!(url_decode("%"), "%");
        assert_eq!(url_decode("%4"), "%4");
        assert_eq!(url_decode("a%\u{20ac}b"), "a%\u{20ac}b");
    }

    #[test]
    fn url_decode_decodes_valid_sequences() {
        assert_eq!(url_decode("a+b%20c%2Fd"), "a b c/d");
        assert_eq!(url_decode("%41%42"), "AB");
        assert_eq!(url_decode("%zz"), "%zz");
        assert_eq!(url_decode("plain"), "plain");
    }

    #[test]
    fn read_line_limited_caps_line_length() {
        // An unterminated 10k "line" must stop at the cap, not buffer it all.
        let mut cursor = std::io::Cursor::new(vec![b'a'; 10_000]);
        assert!(matches!(
            read_line_limited(&mut cursor, 100),
            Ok(LineRead::TooLong)
        ));

        let mut cursor = std::io::Cursor::new(b"hello\r\nworld".to_vec());
        match read_line_limited(&mut cursor, 100) {
            Ok(LineRead::Line(line)) => assert_eq!(line, "hello\r\n"),
            _ => panic!("expected a terminated line within the limit"),
        }

        let mut cursor = std::io::Cursor::new(Vec::new());
        assert!(matches!(
            read_line_limited(&mut cursor, 100),
            Ok(LineRead::Eof)
        ));
    }

    #[test]
    fn malformed_percent_path_gets_response_and_server_survives() {
        // Regression (#155): 'GET /%<multibyte>' used to panic during request
        // parsing (before the token check) and kill the whole UI server.
        let addr = spawn_test_server(Duration::from_secs(5));
        let first = send_raw(
            addr,
            "GET /%\u{20ac} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n".as_bytes(),
        );
        assert!(
            first.starts_with("HTTP/1.1 "),
            "expected an HTTP error response, got: {:?}",
            first
        );
        let second = health_check(addr);
        assert!(
            second.starts_with("HTTP/1.1 200"),
            "server did not survive malformed percent path: {:?}",
            second
        );
        assert!(second.contains("\"ok\":true"));
    }

    #[test]
    fn handler_panic_does_not_terminate_accept_loop() {
        let addr = spawn_test_server(Duration::from_secs(5));
        let panicked = send_raw(
            addr,
            b"GET /__test/panic HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
        );
        assert!(
            panicked.contains("500") || panicked.is_empty(),
            "unexpected response to panicking route: {:?}",
            panicked
        );
        let health = health_check(addr);
        assert!(
            health.starts_with("HTTP/1.1 200"),
            "accept loop died after a handler panic: {:?}",
            health
        );
    }

    #[test]
    fn stalled_connection_does_not_wedge_server() {
        // Regression (#174): a connection that advertises a body but never
        // sends it used to block the serial accept loop forever. A short
        // timeout keeps this test fast; production uses IO_TIMEOUT.
        let addr = spawn_test_server(Duration::from_millis(500));
        let mut stalled = TcpStream::connect(addr).expect("connect stalled client");
        stalled
            .write_all(b"POST /api/sql HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 100\r\n\r\n")
            .expect("write partial request");
        // Give the server time to accept the stalled connection first.
        std::thread::sleep(Duration::from_millis(100));

        let started = Instant::now();
        let response = health_check(addr);
        assert!(
            response.starts_with("HTTP/1.1 200"),
            "stalled connection wedged the server: {:?}",
            response
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "health check took too long: {:?}",
            started.elapsed()
        );
        drop(stalled);
    }

    #[test]
    fn oversized_request_line_returns_431() {
        let addr = spawn_test_server(Duration::from_secs(5));
        let long_path = "a".repeat(MAX_REQUEST_LINE_BYTES + 64);
        let request = format!("GET /{} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", long_path);
        let response = send_raw(addr, request.as_bytes());
        assert!(
            response.starts_with("HTTP/1.1 431"),
            "expected 431 for oversized request line, got: {:?}",
            response
        );
        let health = health_check(addr);
        assert!(
            health.starts_with("HTTP/1.1 200"),
            "server died after oversized request line: {:?}",
            health
        );
    }

    #[test]
    fn constant_time_eq_matches_equality_semantics() {
        // Equality.
        assert!(constant_time_eq("", ""));
        assert!(constant_time_eq("a", "a"));
        assert!(constant_time_eq(TEST_TOKEN, TEST_TOKEN));
        // Inequality at the first byte and at the last byte: the fixed-time
        // loop must report both without depending on mismatch position.
        assert!(!constant_time_eq("xbcdef", "abcdef"));
        assert!(!constant_time_eq("abcdex", "abcdef"));
        // Length mismatches, including prefixes and empty inputs, must not
        // short-circuit and must compare unequal.
        assert!(!constant_time_eq("abc", "ab"));
        assert!(!constant_time_eq("ab", "abc"));
        assert!(!constant_time_eq("", "a"));
        assert!(!constant_time_eq("a", ""));
        assert!(!constant_time_eq(TEST_TOKEN, &format!("{}x", TEST_TOKEN)));
    }

    #[test]
    fn host_allowed_accepts_only_loopback_names() {
        let port = 8377;
        for host in [
            "127.0.0.1",
            "127.0.0.1:8377",
            "localhost",
            "localhost:8377",
            "LocalHost:8377",
            "[::1]",
            "[::1]:8377",
        ] {
            assert!(host_allowed(host, port), "expected allow: {:?}", host);
        }
        for host in [
            "evil.example.com",
            "evil.example.com:8377",
            "127.0.0.1:9999",
            "localhost:9999",
            "[::1]:9999",
            "127.0.0.1.evil.com",
            "localhost.evil.com",
            "sub.localhost",
            "",
            "[::1",
            "[::1]x",
        ] {
            assert!(!host_allowed(host, port), "expected reject: {:?}", host);
        }
    }

    #[test]
    fn cross_origin_host_is_rejected_despite_valid_token() {
        // Regression (#193): a request with Host: evil.example.com and a
        // valid token used to be served (200) — the DNS-rebinding
        // precondition. It must now be rejected before routing.
        let addr = spawn_test_server(Duration::from_secs(5));
        let rejected = send_raw(
            addr,
            format!(
                "GET /api/health HTTP/1.1\r\nHost: evil.example.com\r\nX-ITR-Token: {}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN
            )
            .as_bytes(),
        );
        assert!(
            rejected.starts_with("HTTP/1.1 403"),
            "expected 403 for non-loopback Host, got: {:?}",
            rejected
        );
        assert!(rejected.contains("HOST_NOT_ALLOWED"));

        // The legitimate browser shape (loopback Host with the bound port,
        // token in the query string) keeps working.
        let browser = send_raw(
            addr,
            format!(
                "GET /?token={} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN,
                addr.port()
            )
            .as_bytes(),
        );
        assert!(
            browser.starts_with("HTTP/1.1 200"),
            "browser-style loopback request broke: {:?}",
            browser
        );
    }

    #[test]
    fn missing_host_header_is_rejected() {
        // Regression (#193): host validation is reject-by-default; a request
        // without any Host header (HTTP/1.0-style probe) gets a 4xx even with
        // a valid token. Real browsers always send Host, so the printed URL
        // is unaffected.
        let addr = spawn_test_server(Duration::from_secs(5));
        let response = send_raw(
            addr,
            format!(
                "GET /api/health HTTP/1.1\r\nX-ITR-Token: {}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN
            )
            .as_bytes(),
        );
        assert!(
            response.starts_with("HTTP/1.1 400"),
            "expected 400 for missing Host header, got: {:?}",
            response
        );
        assert!(response.contains("HOST_NOT_ALLOWED"));
        // Server stays healthy for allowed hosts afterwards.
        let health = health_check(addr);
        assert!(
            health.starts_with("HTTP/1.1 200"),
            "server died after Host rejection: {:?}",
            health
        );
    }

    #[test]
    fn too_many_headers_returns_431() {
        let addr = spawn_test_server(Duration::from_secs(5));
        let mut request = String::from("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1\r\n");
        for i in 0..(MAX_HEADER_COUNT + 10) {
            request.push_str(&format!("X-Filler-{}: {}\r\n", i, i));
        }
        request.push_str("\r\n");
        let response = send_raw(addr, request.as_bytes());
        assert!(
            response.starts_with("HTTP/1.1 431"),
            "expected 431 for too many headers, got: {:?}",
            response
        );
        let health = health_check(addr);
        assert!(
            health.starts_with("HTTP/1.1 200"),
            "server died after header flood: {:?}",
            health
        );
    }

    fn test_db() -> Connection {
        db::init_db(Path::new(":memory:")).expect("init in-memory db")
    }

    /// Like `spawn_test_server`, but with the full schema applied and one
    /// issue seeded (id 1), so DB-backed routes can be exercised over HTTP.
    fn spawn_seeded_test_server() -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            let conn = test_db();
            insert_test_issue(&conn, "seeded issue");
            serve(
                &listener,
                &conn,
                Path::new(":memory:"),
                TEST_TOKEN,
                false,
                false,
                Duration::from_secs(5),
                addr.port(),
            );
        });
        addr
    }

    fn insert_test_issue(conn: &Connection, title: &str) -> i64 {
        db::insert_issue(
            conn,
            title,
            "medium",
            "task",
            "",
            &[],
            &[],
            &[],
            "",
            None,
            "",
        )
        .expect("insert issue")
        .id
    }

    #[test]
    fn patch_issue_failure_rolls_back_already_applied_fields() {
        let conn = test_db();
        let id = insert_test_issue(&conn, "before");

        // parent_id is patched LAST; a non-integer value fails there after
        // title and status writes have already been issued.
        let patch = json!({ "title": "after", "status": "done", "parent_id": "bogus" });
        let result = patch_issue(&conn, id, &patch);
        assert!(result.is_err(), "invalid parent_id must propagate");

        // All-or-nothing: earlier fields of the same patch must be rolled back.
        let issue = db::get_issue(&conn, id).expect("get issue");
        assert_eq!(issue.title, "before", "title write must be rolled back");
        assert_eq!(issue.status, "open", "status write must be rolled back");
        let events = db::get_events_for_issue(&conn, id).expect("events");
        assert!(
            events.is_empty(),
            "recorded events must be rolled back, got: {:?}",
            events
        );
    }

    #[test]
    fn ui_patch_self_parent_is_rejected_with_conflict() {
        // Regression (#159): PATCH /api/issues/1 with parent_id 1 used to be
        // accepted, making the issue its own child. The db-layer guard must
        // reject it on the UI path with a cycle error, matching the CLI.
        let addr = spawn_seeded_test_server();
        let body = r#"{"parent_id":1}"#;
        let request = format!(
            "PATCH /api/issues/1 HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ITR-Token: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            TEST_TOKEN,
            body.len(),
            body
        );
        let response = send_raw(addr, request.as_bytes());
        assert!(
            response.starts_with("HTTP/1.1 409"),
            "expected 409 for self-parent PATCH, got: {:?}",
            response
        );
        assert!(
            response.contains("CYCLE_DETECTED"),
            "expected CYCLE_DETECTED error code, got: {:?}",
            response
        );

        // The rejected patch must not be persisted.
        let get = send_raw(
            addr,
            format!(
                "GET /api/issues/1 HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ITR-Token: {}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN
            )
            .as_bytes(),
        );
        assert!(
            get.starts_with("HTTP/1.1 200"),
            "issue fetch after rejected patch failed: {:?}",
            get
        );
        assert!(
            get.contains("\"parent_id\":null"),
            "self-parent must not be persisted: {:?}",
            get
        );
    }

    #[test]
    fn patch_issue_rejects_descendant_parent_and_rolls_back() {
        // Regression (#159), descendant flavor: re-parenting an issue under
        // its own child must fail and roll back every field of the patch.
        let conn = test_db();
        let parent = insert_test_issue(&conn, "parent");
        let child = insert_test_issue(&conn, "child");
        patch_issue(&conn, child, &json!({ "parent_id": parent })).expect("legal parent set");

        let result = patch_issue(
            &conn,
            parent,
            &json!({ "title": "renamed", "parent_id": child }),
        );
        assert!(
            matches!(result, Err(ItrError::CycleDetected(_))),
            "descendant parent must be a cycle error, got: {:?}",
            result
        );
        let issue = db::get_issue(&conn, parent).expect("get issue");
        assert_eq!(
            issue.parent_id, None,
            "cycle-creating parent must not persist"
        );
        assert_eq!(
            issue.title, "parent",
            "earlier fields of the failed patch must roll back"
        );
    }

    #[test]
    fn resolve_issue_failure_leaves_issue_fully_unchanged() {
        let conn = test_db();
        let blocker = insert_test_issue(&conn, "blocker");
        let blocked = insert_test_issue(&conn, "blocked");
        db::add_dependency(&conn, blocker, blocked).expect("add dependency");

        // Inject a failure at the LAST write step (dependency cleanup), so
        // the status/close_reason writes have already been issued.
        conn.execute_batch(
            "CREATE TRIGGER fail_dep_cleanup BEFORE DELETE ON dependencies
             BEGIN SELECT RAISE(ABORT, 'injected mid-resolve failure'); END;",
        )
        .expect("create failure trigger");

        let result = resolve_issue(&conn, blocker, "all done", false);
        assert!(result.is_err(), "injected failure must propagate");

        let issue = db::get_issue(&conn, blocker).expect("get issue");
        assert_eq!(issue.status, "open", "status flip must be rolled back");
        assert_eq!(issue.close_reason, "", "close_reason must be rolled back");
        let events = db::get_events_for_issue(&conn, blocker).expect("events");
        assert!(
            events.is_empty(),
            "recorded events must be rolled back, got: {:?}",
            events
        );
        assert_eq!(
            db::get_blockers(&conn, blocked).expect("blockers"),
            vec![blocker],
            "dependency edge must be retained"
        );
    }

    // --- Batched issue fetch: GET /api/issues?ids=... (#136) ---

    #[test]
    fn batched_issue_details_dedups_and_reports_missing() {
        let conn = test_db();
        let a = insert_test_issue(&conn, "first");
        let b = insert_test_issue(&conn, "second");

        let (details, missing) =
            batched_issue_details(&conn, &format!("{b},{a},{b},999")).expect("batched fetch");
        assert_eq!(
            details.iter().map(|d| d.issue.id).collect::<Vec<_>>(),
            vec![b, a],
            "request order preserved, duplicates fetched once"
        );
        assert_eq!(missing, vec![999]);
        assert!(
            details.iter().all(|d| d.children.is_some()),
            "batched records are the same full UI IssueDetail as the per-issue route"
        );
    }

    #[test]
    fn batched_issue_details_rejects_non_integer_token() {
        let conn = test_db();
        insert_test_issue(&conn, "only");
        let err = batched_issue_details(&conn, "1,abc").unwrap_err();
        assert!(
            matches!(err, ItrError::InvalidValue { .. }),
            "malformed ids token must be INVALID_VALUE, got: {:?}",
            err
        );
    }

    #[test]
    fn issues_route_with_ids_serves_batched_details_over_http() {
        // The batched form must not disturb the existing GET /api/issues
        // list route: same path, switched by the `ids` query parameter.
        let addr = spawn_seeded_test_server();
        let batched = send_raw(
            addr,
            format!(
                "GET /api/issues?ids=1,999,1 HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ITR-Token: {}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN
            )
            .as_bytes(),
        );
        assert!(
            batched.starts_with("HTTP/1.1 200"),
            "batched fetch failed: {:?}",
            batched
        );
        assert!(batched.contains("\"total\":1"));
        assert!(batched.contains("\"missing\":[999]"));
        assert!(
            batched.contains("urgency_breakdown"),
            "batched issues must be full details: {:?}",
            batched
        );

        // Plain list route is unchanged (summaries, no `missing` key).
        let listed = send_raw(
            addr,
            format!(
                "GET /api/issues HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ITR-Token: {}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN
            )
            .as_bytes(),
        );
        assert!(
            listed.starts_with("HTTP/1.1 200"),
            "list broke: {:?}",
            listed
        );
        assert!(!listed.contains("\"missing\""));

        // Malformed token in ids -> 400, server stays up.
        let bad = send_raw(
            addr,
            format!(
                "GET /api/issues?ids=abc HTTP/1.1\r\nHost: 127.0.0.1\r\nX-ITR-Token: {}\r\nConnection: close\r\n\r\n",
                TEST_TOKEN
            )
            .as_bytes(),
        );
        assert!(
            bad.starts_with("HTTP/1.1 400"),
            "expected 400 for malformed ids, got: {:?}",
            bad
        );
        let health = health_check(addr);
        assert!(health.starts_with("HTTP/1.1 200"));
    }
}
