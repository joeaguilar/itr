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
use std::path::Path;
use std::process::Command;

const INDEX_HTML: &str = include_str!("../ui_assets/index.html");
const APP_CSS: &str = include_str!("../ui_assets/app.css");
const APP_JS: &str = include_str!("../ui_assets/app.js");
const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_SQL_ROWS: usize = 500;

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

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                if let Err(err) = handle_stream(&mut stream, conn, db_path, &token, allow_dangerous)
                {
                    let response = error_response(500, &err.to_string(), "INTERNAL_ERROR");
                    let _ = write_response(&mut stream, response);
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

    Ok(())
}

fn session_token(conn: &Connection) -> Result<String, ItrError> {
    Ok(conn.query_row("SELECT lower(hex(randomblob(24)))", [], |row| row.get(0))?)
}

fn handle_stream(
    stream: &mut TcpStream,
    conn: &Connection,
    db_path: &Path,
    token: &str,
    allow_dangerous: bool,
) -> Result<(), ItrError> {
    let response = match read_request(stream) {
        Ok(request) => match route_request(&request, conn, db_path, token, allow_dangerous) {
            Ok(response) => response,
            Err(err) => error_response_for_itr(err),
        },
        Err(err) => error_response(400, &err.to_string(), "BAD_REQUEST"),
    };
    write_response(stream, response)?;
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest, ItrError> {
    let mut reader = BufReader::new(stream);
    let mut start_line = String::new();
    reader.read_line(&mut start_line)?;
    if start_line.trim().is_empty() {
        return Err(ItrError::InvalidValue {
            field: "request".to_string(),
            value: String::new(),
            valid: "HTTP request line".to_string(),
        });
    }

    let mut parts = start_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default();
    if method.is_empty() || target.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "request".to_string(),
            value: start_line.trim().to_string(),
            valid: "METHOD path HTTP/version".to_string(),
        });
    }

    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
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
        return Err(ItrError::InvalidValue {
            field: "body".to_string(),
            value: content_length.to_string(),
            valid: format!("at most {} bytes", MAX_BODY_BYTES),
        });
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
    let mut out = Vec::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    i += 3;
                } else {
                    out.push(bytes[i]);
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
            let issues = list_issue_summaries(conn, &request.query)?;
            json_response(json!({
                "total": issues.len(),
                "issues": issues,
            }))
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
            db::remove_dependency(conn, blocker_id, id)?;
            json_response(json!({ "issue": issue_detail(conn, id)? }))
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
            let removed = db::remove_relation(conn, id, target_id)?;
            json_response(json!({
                "removed": removed,
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
    if supplied.is_some_and(|value| value == token) {
        return Ok(());
    }
    Err(ItrError::InvalidValue {
        field: "token".to_string(),
        value: String::new(),
        valid: "current UI session token".to_string(),
    })
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
    let old_issue = db::get_issue(conn, id)?;

    patch_string_field(conn, id, patch, "title", "title", &old_issue.title)?;
    patch_string_field(conn, id, patch, "context", "context", &old_issue.context)?;
    patch_string_field(
        conn,
        id,
        patch,
        "acceptance",
        "acceptance",
        &old_issue.acceptance,
    )?;
    patch_string_field(
        conn,
        id,
        patch,
        "assigned_to",
        "assigned_to",
        &old_issue.assigned_to,
    )?;
    patch_string_field(
        conn,
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
        db::record_event(conn, id, "status", &old_issue.status, &status)?;
        db::update_issue_field(conn, id, "status", &status)?;
    }
    if let Some(value) = patch.get("priority").and_then(Value::as_str) {
        let priority = normalize::normalize_priority(value);
        let priority = match validate_priority(&priority) {
            Ok(()) => priority,
            Err(_) => "medium".to_string(),
        };
        db::record_event(conn, id, "priority", &old_issue.priority, &priority)?;
        db::update_issue_field(conn, id, "priority", &priority)?;
    }
    if let Some(value) = patch.get("kind").and_then(Value::as_str) {
        let kind = normalize::normalize_kind(value);
        let kind = match validate_kind(&kind) {
            Ok(()) => kind,
            Err(_) => "task".to_string(),
        };
        db::record_event(conn, id, "kind", &old_issue.kind, &kind)?;
        db::update_issue_field(conn, id, "kind", &kind)?;
    }

    patch_array_field(conn, id, patch, "files", &old_issue.files, false)?;
    patch_array_field(conn, id, patch, "tags", &old_issue.tags, false)?;
    patch_array_field(conn, id, patch, "skills", &old_issue.skills, true)?;

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
            conn,
            id,
            "parent_id",
            &old_issue
                .parent_id
                .map(|value| value.to_string())
                .unwrap_or_default(),
            &parent_id.map(|value| value.to_string()).unwrap_or_default(),
        )?;
        db::update_issue_parent(conn, id, parent_id)?;
    }

    issue_detail(conn, id)
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
    let status = if wontfix { "wontfix" } else { "done" };
    let old_issue = db::get_issue(conn, id)?;
    db::record_event(conn, id, "status", &old_issue.status, status)?;
    db::update_issue_field(conn, id, "status", status)?;
    if !reason.trim().is_empty() {
        db::record_event(conn, id, "close_reason", &old_issue.close_reason, reason)?;
        db::update_issue_field(conn, id, "close_reason", reason)?;
    }
    let unblocked = db::get_newly_unblocked(conn, id)?;
    db::remove_blocker_edges(conn, id)?;
    Ok(json!({
        "issue": issue_detail(conn, id)?,
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
        409 => "Conflict",
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
