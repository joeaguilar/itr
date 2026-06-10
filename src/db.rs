use crate::error::ItrError;
use crate::models::{Event, Issue, Note, Relation};
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use std::env;
use std::path::{Path, PathBuf};

const SCHEMA: &str = r"
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

CREATE TABLE IF NOT EXISTS issues (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    title           TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open'
                    CHECK (status IN ('open', 'in-progress', 'done', 'wontfix')),
    priority        TEXT NOT NULL DEFAULT 'medium'
                    CHECK (priority IN ('critical', 'high', 'medium', 'low')),
    kind            TEXT NOT NULL DEFAULT 'task'
                    CHECK (kind IN ('bug', 'feature', 'task', 'epic')),
    context         TEXT NOT NULL DEFAULT '',
    files           TEXT NOT NULL DEFAULT '[]',
    tags            TEXT NOT NULL DEFAULT '[]',
    skills          TEXT NOT NULL DEFAULT '[]',
    acceptance      TEXT NOT NULL DEFAULT '',
    parent_id       INTEGER REFERENCES issues(id) ON DELETE SET NULL,
    close_reason    TEXT NOT NULL DEFAULT '',
    assigned_to     TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS dependencies (
    blocker_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    blocked_id      INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (blocker_id, blocked_id),
    CHECK (blocker_id != blocked_id)
);

CREATE TABLE IF NOT EXISTS notes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id        INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    content         TEXT NOT NULL,
    agent           TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS config (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS events (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    issue_id        INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    field           TEXT NOT NULL,
    old_value       TEXT NOT NULL DEFAULT '',
    new_value       TEXT NOT NULL DEFAULT '',
    agent           TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS relations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    source_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    target_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    relation_type   TEXT NOT NULL CHECK(relation_type IN ('duplicate', 'related', 'supersedes')),
    created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    UNIQUE(source_id, target_id, relation_type)
);

CREATE INDEX IF NOT EXISTS idx_issues_status ON issues(status);
CREATE INDEX IF NOT EXISTS idx_issues_priority ON issues(priority);
CREATE INDEX IF NOT EXISTS idx_issues_kind ON issues(kind);
CREATE INDEX IF NOT EXISTS idx_issues_parent ON issues(parent_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocked ON dependencies(blocked_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocker ON dependencies(blocker_id);
CREATE INDEX IF NOT EXISTS idx_notes_issue ON notes(issue_id);
CREATE INDEX IF NOT EXISTS idx_events_issue ON events(issue_id);
CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at);
CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_id);
CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_id);

CREATE TRIGGER IF NOT EXISTS trg_issues_updated_at
    AFTER UPDATE ON issues
    FOR EACH ROW
BEGIN
    UPDATE issues SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
    WHERE id = OLD.id;
END;
";

pub fn find_db(override_path: Option<&str>) -> Result<PathBuf, ItrError> {
    // Explicit overrides (ITR_DB_PATH, then --db) are validated before use.
    let env_path = env::var("ITR_DB_PATH").ok();
    if let Some(resolved) = resolve_override_db(env_path.as_deref(), override_path) {
        return resolved;
    }

    // Walk up from cwd
    let mut dir = env::current_dir().map_err(ItrError::Io)?;
    loop {
        let candidate = dir.join(".itr.db");
        if candidate.exists() {
            return Ok(candidate);
        }
        if !dir.pop() {
            return Err(ItrError::NoDatabase);
        }
    }
}

/// Resolve an explicit DB override (`ITR_DB_PATH` env var or `--db` flag).
///
/// Returns `None` when no usable override is present — an empty-string
/// `ITR_DB_PATH` is treated as unset — so the caller falls through to the
/// walk-up finder. A nonexistent override path is rejected with `NoDatabase`
/// instead of letting `Connection::open` create an empty junk file that the
/// walk-up finder would forever discover as a broken database (#160). The
/// offending path is named on stderr because the `NoDatabase` variant
/// carries no payload.
fn resolve_override_db(
    env_path: Option<&str>,
    cli_path: Option<&str>,
) -> Option<Result<PathBuf, ItrError>> {
    let (path, source) = match (env_path, cli_path) {
        (Some(p), _) if !p.is_empty() => (p, "ITR_DB_PATH"),
        (_, Some(p)) => (p, "--db"),
        _ => return None,
    };
    if path.is_empty() {
        eprintln!("ERROR: {source} is set to an empty path; no database opened.");
        return Some(Err(ItrError::NoDatabase));
    }
    if Path::new(path).exists() {
        Some(Ok(PathBuf::from(path)))
    } else {
        eprintln!(
            "ERROR: {source} points to '{path}', which does not exist. Run 'itr init --db {path}' to create it."
        );
        Some(Err(ItrError::NoDatabase))
    }
}

pub fn open_db(path: &Path) -> Result<Connection, ItrError> {
    let conn = Connection::open(path)?;
    // busy_timeout makes concurrent writers (e.g. parallel `itr claim`) wait
    // for the write lock instead of failing immediately with SQLITE_BUSY.
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
    )?;
    migrate_current_schema(&conn)?;
    try_create_fts(&conn);
    Ok(conn)
}

fn migrate_current_schema(conn: &Connection) -> Result<(), ItrError> {
    migrate_add_skills(conn)?;
    migrate_add_assigned_to(conn)?;
    migrate_add_events(conn)?;
    migrate_add_relations(conn)?;
    Ok(())
}

fn migrate_add_skills(conn: &Connection) -> Result<(), ItrError> {
    let has_skills: bool = conn
        .prepare("PRAGMA table_info(issues)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|col| col.as_deref() == Ok("skills"));
    if !has_skills {
        conn.execute_batch("ALTER TABLE issues ADD COLUMN skills TEXT NOT NULL DEFAULT '[]';")?;
    }
    Ok(())
}

fn migrate_add_assigned_to(conn: &Connection) -> Result<(), ItrError> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(issues)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|col| col.as_deref() == Ok("assigned_to"));
    if !has_col {
        conn.execute_batch("ALTER TABLE issues ADD COLUMN assigned_to TEXT NOT NULL DEFAULT '';")?;
    }
    Ok(())
}

fn migrate_add_events(conn: &Connection) -> Result<(), ItrError> {
    let has_table: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='events'",
        [],
        |row| row.get(0),
    )?;
    if !has_table {
        conn.execute_batch(
            "CREATE TABLE events (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                issue_id    INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                field       TEXT NOT NULL,
                old_value   TEXT NOT NULL DEFAULT '',
                new_value   TEXT NOT NULL DEFAULT '',
                agent       TEXT NOT NULL DEFAULT '',
                created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
            );",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_events_issue ON events(issue_id);
         CREATE INDEX IF NOT EXISTS idx_events_created ON events(created_at);",
    )?;
    Ok(())
}

fn migrate_add_relations(conn: &Connection) -> Result<(), ItrError> {
    let has_table: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='relations'",
        [],
        |row| row.get(0),
    )?;
    if !has_table {
        conn.execute_batch(
            "CREATE TABLE relations (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                source_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                target_id       INTEGER NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
                relation_type   TEXT NOT NULL CHECK(relation_type IN ('duplicate', 'related', 'supersedes')),
                created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
                UNIQUE(source_id, target_id, relation_type)
            );",
        )?;
    }
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_relations_source ON relations(source_id);
         CREATE INDEX IF NOT EXISTS idx_relations_target ON relations(target_id);",
    )?;
    Ok(())
}

pub fn init_db(path: &Path) -> Result<Connection, ItrError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    migrate_current_schema(&conn)?;
    try_create_fts(&conn);
    Ok(conn)
}

pub fn get_schema_sql() -> &'static str {
    SCHEMA
}

// --- Issue CRUD ---

#[allow(clippy::too_many_arguments)]
pub fn insert_issue(
    conn: &Connection,
    title: &str,
    priority: &str,
    kind: &str,
    context: &str,
    files: &[String],
    tags: &[String],
    skills: &[String],
    acceptance: &str,
    parent_id: Option<i64>,
    assigned_to: &str,
) -> Result<Issue, ItrError> {
    let files_json = serde_json::to_string(files)?;
    let tags_json = serde_json::to_string(tags)?;
    let skills_json = serde_json::to_string(skills)?;

    conn.execute(
        "INSERT INTO issues (title, priority, kind, context, files, tags, skills, acceptance, parent_id, assigned_to)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![title, priority, kind, context, files_json, tags_json, skills_json, acceptance, parent_id, assigned_to],
    )?;

    let id = conn.last_insert_rowid();
    let issue = get_issue(conn, id)?;
    fts_index_issue(conn, &issue);
    Ok(issue)
}

pub fn get_issue(conn: &Connection, id: i64) -> Result<Issue, ItrError> {
    conn.query_row(
        "SELECT id, title, status, priority, kind, context, files, tags, skills, acceptance, parent_id, close_reason, created_at, updated_at, assigned_to
         FROM issues WHERE id = ?1",
        params![id],
        row_to_issue,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => ItrError::NotFound(id),
        other => ItrError::Db(other),
    })
}

pub fn issue_exists(conn: &Connection, id: i64) -> Result<bool, ItrError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM issues WHERE id = ?1",
        params![id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn parse_json_array(s: String) -> Vec<String> {
    serde_json::from_str(&s).unwrap_or_default()
}

/// Append an `AND column IN (?, ?, ...)` clause to the SQL string,
/// pushing values into `param_values`. Returns the number of placeholders added.
fn append_in_clause(
    sql: &mut String,
    param_values: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    column: &str,
    values: &[String],
) {
    let placeholders: Vec<String> = values
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", param_values.len() + i + 1))
        .collect();
    sql.push_str(&format!(" AND {} IN ({})", column, placeholders.join(",")));
    for v in values {
        param_values.push(Box::new(v.clone()));
    }
}

fn row_to_issue(row: &rusqlite::Row) -> rusqlite::Result<Issue> {
    Ok(Issue {
        id: row.get(0)?,
        title: row.get(1)?,
        status: row.get(2)?,
        priority: row.get(3)?,
        kind: row.get(4)?,
        context: row.get(5)?,
        files: parse_json_array(row.get::<_, String>(6)?),
        tags: parse_json_array(row.get::<_, String>(7)?),
        skills: parse_json_array(row.get::<_, String>(8)?),
        acceptance: row.get(9)?,
        parent_id: row.get(10)?,
        close_reason: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        assigned_to: row.get(14)?,
    })
}

fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    Ok(Note {
        id: row.get(0)?,
        issue_id: row.get(1)?,
        content: row.get(2)?,
        agent: row.get(3)?,
        created_at: row.get(4)?,
    })
}

fn row_to_event(row: &rusqlite::Row) -> rusqlite::Result<Event> {
    Ok(Event {
        id: row.get(0)?,
        issue_id: row.get(1)?,
        field: row.get(2)?,
        old_value: row.get(3)?,
        new_value: row.get(4)?,
        agent: row.get(5)?,
        created_at: row.get(6)?,
    })
}

fn row_to_relation(row: &rusqlite::Row) -> rusqlite::Result<Relation> {
    Ok(Relation {
        id: row.get(0)?,
        source_id: row.get(1)?,
        target_id: row.get(2)?,
        relation_type: row.get(3)?,
        created_at: row.get(4)?,
    })
}

pub fn list_issues(
    conn: &Connection,
    filter: &crate::models::ListFilter,
) -> Result<Vec<Issue>, ItrError> {
    let mut sql = String::from(
        "SELECT id, title, status, priority, kind, context, files, tags, skills, acceptance, parent_id, close_reason, created_at, updated_at, assigned_to FROM issues WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if !filter.all {
        if filter.statuses.is_empty() {
            let defaults = vec!["open".to_string(), "in-progress".to_string()];
            append_in_clause(&mut sql, &mut param_values, "status", &defaults);
        } else {
            append_in_clause(&mut sql, &mut param_values, "status", &filter.statuses);
        }
    }

    if !filter.priorities.is_empty() {
        append_in_clause(&mut sql, &mut param_values, "priority", &filter.priorities);
    }

    if !filter.kinds.is_empty() {
        append_in_clause(&mut sql, &mut param_values, "kind", &filter.kinds);
    }

    if let Some(pid) = filter.parent_id {
        let p = param_values.len() + 1;
        sql.push_str(&format!(" AND parent_id = ?{}", p));
        param_values.push(Box::new(pid));
    }

    if let Some(ref agent) = filter.assigned_to {
        let p = param_values.len() + 1;
        sql.push_str(&format!(" AND assigned_to = ?{}", p));
        param_values.push(Box::new(agent.clone()));
    }

    // Deterministic base order: without an ORDER BY, SQLite is free to return
    // rows in index-scan order, which makes in-memory stable sorts (urgency
    // ties, priority ties) and unsorted callers nondeterministic (#171).
    sql.push_str(" ORDER BY id");

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let issues: Vec<Issue> = stmt
        .query_map(params_ref.as_slice(), row_to_issue)?
        .collect::<Result<Vec<_>, _>>()?;

    // Filter by tags (AND logic)
    let issues = if filter.tags.is_empty() {
        issues
    } else {
        issues
            .into_iter()
            .filter(|i| filter.tags.iter().all(|t| i.tags.contains(t)))
            .collect()
    };

    // Filter by tag_any (OR logic)
    let issues = if filter.tag_any.is_empty() {
        issues
    } else {
        issues
            .into_iter()
            .filter(|i| filter.tag_any.iter().any(|t| i.tags.contains(t)))
            .collect()
    };

    // Filter by skills (AND logic)
    let issues = if filter.skills.is_empty() {
        issues
    } else {
        issues
            .into_iter()
            .filter(|i| filter.skills.iter().all(|s| i.skills.contains(s)))
            .collect()
    };

    // Filter by blocked status
    let issues = if filter.blocked_only {
        issues
            .into_iter()
            .filter(|i| is_blocked(conn, i.id).unwrap_or(false))
            .collect()
    } else if !filter.include_blocked && !filter.all {
        issues
            .into_iter()
            .filter(|i| !is_blocked(conn, i.id).unwrap_or(false))
            .collect()
    } else {
        issues
    };

    Ok(issues)
}

pub fn update_issue_field(
    conn: &Connection,
    id: i64,
    field: &str,
    value: &str,
) -> Result<(), ItrError> {
    const VALID_COLUMNS: &[&str] = &[
        "title",
        "status",
        "priority",
        "kind",
        "context",
        "files",
        "tags",
        "skills",
        "acceptance",
        "close_reason",
        "assigned_to",
    ];
    if !VALID_COLUMNS.contains(&field) {
        return Err(ItrError::InvalidValue {
            field: "column".to_string(),
            value: field.to_string(),
            valid: VALID_COLUMNS.join(", "),
        });
    }
    if !issue_exists(conn, id)? {
        return Err(ItrError::NotFound(id));
    }
    let sql = format!("UPDATE issues SET {} = ?1 WHERE id = ?2", field);
    conn.execute(&sql, params![value, id])?;

    // Re-index FTS for searchable fields
    match field {
        "title" | "context" | "acceptance" | "tags" | "files" | "skills" | "close_reason" => {
            if let Ok(issue) = get_issue(conn, id) {
                fts_index_issue(conn, &issue);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Result of an atomic claim attempt (see [`claim_issue`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The compare-and-swap won: the issue moved `open` -> `in-progress`
    /// (and was assigned, when an agent was supplied).
    Claimed { prior_assigned_to: String },
    /// The issue was not `open` (already claimed, done, or wontfix).
    /// Nothing was modified; the observed state is returned for reporting.
    NotOpen { status: String, assigned_to: String },
}

/// Atomically claim an issue: transition `open` -> `in-progress` and record
/// the assignment in a single transaction.
///
/// The UPDATE is guarded with `AND status = 'open'` (compare-and-swap), so a
/// concurrent claimer that already won leaves this call with 0 affected rows
/// and a `NotOpen` outcome instead of silently stealing the issue. The
/// transaction starts IMMEDIATE so the pre-read of status/assignee is made
/// under the write lock and cannot go stale before the UPDATE.
pub fn claim_issue(
    conn: &Connection,
    id: i64,
    agent: Option<&str>,
) -> Result<ClaimOutcome, ItrError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let (status, assigned_to): (String, String) = tx
        .query_row(
            "SELECT status, assigned_to FROM issues WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => ItrError::NotFound(id),
            other => ItrError::Db(other),
        })?;

    let rows = tx.execute(
        "UPDATE issues SET status = 'in-progress' WHERE id = ?1 AND status = 'open'",
        params![id],
    )?;
    if rows == 0 {
        // Lost the race (or the issue is closed); leave everything untouched.
        return Ok(ClaimOutcome::NotOpen {
            status,
            assigned_to,
        });
    }

    record_event(&tx, id, "status", &status, "in-progress")?;
    if let Some(name) = agent {
        if name != assigned_to {
            record_event(&tx, id, "assigned_to", &assigned_to, name)?;
            tx.execute(
                "UPDATE issues SET assigned_to = ?1 WHERE id = ?2",
                params![name, id],
            )?;
        }
    }
    tx.commit()?;
    Ok(ClaimOutcome::Claimed {
        prior_assigned_to: assigned_to,
    })
}

pub fn update_issue_parent(
    conn: &Connection,
    id: i64,
    parent_id: Option<i64>,
) -> Result<(), ItrError> {
    if !issue_exists(conn, id)? {
        return Err(ItrError::NotFound(id));
    }
    // Guard at the db layer so every caller (CLI update, UI PATCH, future
    // writers) gets the same parent-cycle protection (#159). Parent cycles
    // are one of the few designated hard errors: any parent-chain traversal
    // would loop forever on a self/descendant parent.
    if let Some(pid) = parent_id {
        if !issue_exists(conn, pid)? {
            return Err(ItrError::NotFound(pid));
        }
        if is_self_or_descendant(conn, id, pid)? {
            return Err(ItrError::CycleDetected(format!(
                "parent_id: {} cannot be parent of {} (creates cycle)",
                pid, id
            )));
        }
    }
    conn.execute(
        "UPDATE issues SET parent_id = ?1 WHERE id = ?2",
        params![parent_id, id],
    )?;
    Ok(())
}

/// Check if `candidate` is `id` itself or any descendant of `id` via `parent_id` edges.
/// Used to prevent parent-cycle creation when setting `id`'s parent to `candidate`.
/// Reuses the BFS pattern from `has_path` (dependency cycle detection).
pub fn is_self_or_descendant(conn: &Connection, id: i64, candidate: i64) -> Result<bool, ItrError> {
    if id == candidate {
        return Ok(true);
    }
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(id);

    while let Some(current) = queue.pop_front() {
        if !visited.insert(current) {
            continue;
        }
        // Follow: which issues have `current` as their parent? (descendants of `current`)
        let mut stmt = conn.prepare("SELECT id FROM issues WHERE parent_id = ?1")?;
        let children: Vec<i64> = stmt
            .query_map(params![current], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for child in children {
            if child == candidate {
                return Ok(true);
            }
            if !visited.contains(&child) {
                queue.push_back(child);
            }
        }
    }
    Ok(false)
}

// --- Dependencies ---

pub fn add_dependency(
    conn: &Connection,
    blocker_id: i64,
    blocked_id: i64,
) -> Result<bool, ItrError> {
    if !issue_exists(conn, blocker_id)? {
        return Err(ItrError::NotFound(blocker_id));
    }
    if !issue_exists(conn, blocked_id)? {
        return Err(ItrError::NotFound(blocked_id));
    }

    // Check for existing
    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM dependencies WHERE blocker_id = ?1 AND blocked_id = ?2",
        params![blocker_id, blocked_id],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(false); // idempotent
    }

    // Cycle check: would adding blocker_id->blocked_id create a cycle?
    // Check if blocked_id can already reach blocker_id via existing "blocks" edges.
    // If so, adding this edge would create a cycle.
    if has_path(conn, blocked_id, blocker_id)? {
        return Err(ItrError::CycleDetected(format!(
            "{} -> ... -> {}",
            blocked_id, blocker_id
        )));
    }

    conn.execute(
        "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?1, ?2)",
        params![blocker_id, blocked_id],
    )?;
    Ok(true)
}

/// Removes the `blocked_id <- blocker_id` edge. Returns whether an edge
/// actually existed, so callers can distinguish a real removal from a no-op
/// instead of reporting phantom state changes (#191).
pub fn remove_dependency(
    conn: &Connection,
    blocker_id: i64,
    blocked_id: i64,
) -> Result<bool, ItrError> {
    if !issue_exists(conn, blocker_id)? {
        return Err(ItrError::NotFound(blocker_id));
    }
    if !issue_exists(conn, blocked_id)? {
        return Err(ItrError::NotFound(blocked_id));
    }
    let deleted = conn.execute(
        "DELETE FROM dependencies WHERE blocker_id = ?1 AND blocked_id = ?2",
        params![blocker_id, blocked_id],
    )?;
    Ok(deleted > 0)
}

/// Check if there's a path from `from_id` to `to_id` following blocker edges.
/// i.e., `from_id` is blocked by X, X is blocked by Y, ... eventually reaches `to_id`.
pub fn has_path(conn: &Connection, from_id: i64, to_id: i64) -> Result<bool, ItrError> {
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(from_id);

    while let Some(current) = queue.pop_front() {
        if current == to_id {
            return Ok(true);
        }
        if !visited.insert(current) {
            continue;
        }
        // Follow: what does `current` block? (current is a blocker_id, find blocked_ids)
        let mut stmt = conn.prepare("SELECT blocked_id FROM dependencies WHERE blocker_id = ?1")?;
        let blocked: Vec<i64> = stmt
            .query_map(params![current], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for b in blocked {
            if !visited.contains(&b) {
                queue.push_back(b);
            }
        }
    }
    Ok(false)
}

pub fn get_blockers(conn: &Connection, issue_id: i64) -> Result<Vec<i64>, ItrError> {
    let mut stmt = conn.prepare("SELECT blocker_id FROM dependencies WHERE blocked_id = ?1")?;
    let ids: Vec<i64> = stmt
        .query_map(params![issue_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

pub fn get_blocking(conn: &Connection, issue_id: i64) -> Result<Vec<i64>, ItrError> {
    let mut stmt = conn.prepare("SELECT blocked_id FROM dependencies WHERE blocker_id = ?1")?;
    let ids: Vec<i64> = stmt
        .query_map(params![issue_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

pub fn is_blocked(conn: &Connection, issue_id: i64) -> Result<bool, ItrError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dependencies d
         JOIN issues i ON d.blocker_id = i.id
         WHERE d.blocked_id = ?1
         AND i.status NOT IN ('done', 'wontfix')",
        params![issue_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

pub fn blocks_active_issues(conn: &Connection, issue_id: i64) -> Result<bool, ItrError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM dependencies d
         JOIN issues i ON d.blocked_id = i.id
         WHERE d.blocker_id = ?1
         AND i.status NOT IN ('done', 'wontfix')",
        params![issue_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Get issues that become unblocked when `closed_id` is resolved.
pub fn get_newly_unblocked(
    conn: &Connection,
    closed_id: i64,
) -> Result<Vec<(i64, String)>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.title FROM issues i
         JOIN dependencies d ON d.blocked_id = i.id
         WHERE d.blocker_id = ?1
         AND i.status NOT IN ('done', 'wontfix')
         AND NOT EXISTS (
             SELECT 1 FROM dependencies d2
             JOIN issues i2 ON d2.blocker_id = i2.id
             WHERE d2.blocked_id = i.id
             AND d2.blocker_id != ?1
             AND i2.status NOT IN ('done', 'wontfix')
         )",
    )?;
    let results: Vec<(i64, String)> = stmt
        .query_map(params![closed_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

/// Remove all dependency edges where the given issue is the blocker.
/// Called on close to auto-clean stale edges so `doctor --fix` isn't needed.
pub fn remove_blocker_edges(conn: &Connection, blocker_id: i64) -> Result<usize, ItrError> {
    let count = conn.execute(
        "DELETE FROM dependencies WHERE blocker_id = ?1",
        params![blocker_id],
    )?;
    Ok(count)
}

// --- Notes ---

pub fn add_note(
    conn: &Connection,
    issue_id: i64,
    content: &str,
    agent: &str,
) -> Result<Note, ItrError> {
    if !issue_exists(conn, issue_id)? {
        return Err(ItrError::NotFound(issue_id));
    }
    conn.execute(
        "INSERT INTO notes (issue_id, content, agent) VALUES (?1, ?2, ?3)",
        params![issue_id, content, agent],
    )?;
    let id = conn.last_insert_rowid();
    conn.query_row(
        "SELECT id, issue_id, content, agent, created_at FROM notes WHERE id = ?1",
        params![id],
        row_to_note,
    )
    .map_err(ItrError::Db)
}

pub fn get_notes(conn: &Connection, issue_id: i64) -> Result<Vec<Note>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT id, issue_id, content, agent, created_at FROM notes WHERE issue_id = ?1 ORDER BY created_at ASC",
    )?;
    let notes: Vec<Note> = stmt
        .query_map(params![issue_id], row_to_note)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(notes)
}

pub fn get_note(conn: &Connection, note_id: i64) -> Result<Note, ItrError> {
    conn.query_row(
        "SELECT id, issue_id, content, agent, created_at FROM notes WHERE id = ?1",
        params![note_id],
        row_to_note,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => ItrError::NotFound(note_id),
        other => ItrError::Db(other),
    })
}

pub fn delete_note(conn: &Connection, note_id: i64) -> Result<Note, ItrError> {
    let note = get_note(conn, note_id)?;
    conn.execute("DELETE FROM notes WHERE id = ?1", params![note_id])?;
    Ok(note)
}

pub fn update_note(conn: &Connection, note_id: i64, content: &str) -> Result<Note, ItrError> {
    let _existing = get_note(conn, note_id)?;
    conn.execute(
        "UPDATE notes SET content = ?1 WHERE id = ?2",
        params![content, note_id],
    )?;
    get_note(conn, note_id)
}

pub fn count_notes(conn: &Connection, issue_id: i64) -> Result<i64, ItrError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM notes WHERE issue_id = ?1",
        params![issue_id],
        |row| row.get(0),
    )?;
    Ok(count)
}

// --- Search ---

/// Escape SQL LIKE wildcards (`%`, `_`) and the escape character itself so a
/// user-supplied term matches only literal occurrences. Must be paired with
/// `ESCAPE '\'` on the LIKE clause.
fn escape_like(term: &str) -> String {
    term.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub fn search_issue_ids(
    conn: &Connection,
    terms: &[String],
    statuses: &[String],
    priorities: &[String],
    kinds: &[String],
    all: bool,
) -> Result<Vec<i64>, ItrError> {
    if terms.is_empty() {
        return Ok(vec![]);
    }

    let mut sql = String::from(
        "SELECT DISTINCT i.id FROM issues i LEFT JOIN notes n ON n.issue_id = i.id WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> =
        Vec::with_capacity(terms.len() * 8);

    // Each term must match at least one searchable field
    for term in terms {
        let pattern = format!("%{}%", escape_like(term));
        let base = param_values.len();
        let p1 = base + 1;
        let p2 = base + 2;
        let p3 = base + 3;
        let p4 = base + 4;
        let p5 = base + 5;
        let p6 = base + 6;
        let p7 = base + 7;
        let p8 = base + 8;
        sql.push_str(&format!(
            " AND (i.title LIKE ?{} ESCAPE '\\' OR i.context LIKE ?{} ESCAPE '\\' OR i.acceptance LIKE ?{} ESCAPE '\\' OR i.close_reason LIKE ?{} ESCAPE '\\' OR i.tags LIKE ?{} ESCAPE '\\' OR i.files LIKE ?{} ESCAPE '\\' OR i.skills LIKE ?{} ESCAPE '\\' OR n.content LIKE ?{} ESCAPE '\\')",
            p1, p2, p3, p4, p5, p6, p7, p8
        ));
        for _ in 0..8 {
            param_values.push(Box::new(pattern.clone()));
        }
    }

    // Status filter
    if !all {
        if statuses.is_empty() {
            let defaults = vec!["open".to_string(), "in-progress".to_string()];
            append_in_clause(&mut sql, &mut param_values, "i.status", &defaults);
        } else {
            append_in_clause(&mut sql, &mut param_values, "i.status", statuses);
        }
    }

    if !priorities.is_empty() {
        append_in_clause(&mut sql, &mut param_values, "i.priority", priorities);
    }

    if !kinds.is_empty() {
        append_in_clause(&mut sql, &mut param_values, "i.kind", kinds);
    }

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let ids: Vec<i64> = stmt
        .query_map(params_ref.as_slice(), |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

pub fn search_note_issue_ids(
    conn: &Connection,
    terms: &[String],
    statuses: &[String],
    priorities: &[String],
    kinds: &[String],
    all: bool,
) -> Result<Vec<i64>, ItrError> {
    if terms.is_empty() {
        return Ok(vec![]);
    }

    let mut sql = String::from(
        "SELECT DISTINCT i.id FROM notes n JOIN issues i ON i.id = n.issue_id WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(terms.len());

    for term in terms {
        let p = param_values.len() + 1;
        sql.push_str(&format!(" AND n.content LIKE ?{} ESCAPE '\\'", p));
        param_values.push(Box::new(format!("%{}%", escape_like(term))));
    }

    if !all {
        if statuses.is_empty() {
            let defaults = vec!["open".to_string(), "in-progress".to_string()];
            append_in_clause(&mut sql, &mut param_values, "i.status", &defaults);
        } else {
            append_in_clause(&mut sql, &mut param_values, "i.status", statuses);
        }
    }

    if !priorities.is_empty() {
        append_in_clause(&mut sql, &mut param_values, "i.priority", priorities);
    }

    if !kinds.is_empty() {
        append_in_clause(&mut sql, &mut param_values, "i.kind", kinds);
    }

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let ids: Vec<i64> = stmt
        .query_map(params_ref.as_slice(), |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

// --- Config ---

pub fn config_get(conn: &Connection, key: &str) -> Result<Option<String>, ItrError> {
    match conn.query_row(
        "SELECT value FROM config WHERE key = ?1",
        params![key],
        |row| row.get::<_, String>(0),
    ) {
        Ok(val) => Ok(Some(val)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(ItrError::Db(e)),
    }
}

pub fn config_set(conn: &Connection, key: &str, value: &str) -> Result<(), ItrError> {
    conn.execute(
        "INSERT OR REPLACE INTO config (key, value) VALUES (?1, ?2)",
        params![key, value],
    )?;
    Ok(())
}

pub fn config_list(conn: &Connection) -> Result<Vec<(String, String)>, ItrError> {
    let mut stmt = conn.prepare("SELECT key, value FROM config ORDER BY key")?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn config_reset(conn: &Connection) -> Result<(), ItrError> {
    conn.execute("DELETE FROM config", [])?;
    Ok(())
}

// --- All issues (for export, stats, etc.) ---

pub fn all_issues(conn: &Connection) -> Result<Vec<Issue>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, status, priority, kind, context, files, tags, skills, acceptance, parent_id, close_reason, created_at, updated_at, assigned_to
         FROM issues ORDER BY id",
    )?;
    let issues: Vec<Issue> = stmt
        .query_map([], row_to_issue)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(issues)
}

pub fn all_dependencies(conn: &Connection) -> Result<Vec<(i64, i64)>, ItrError> {
    let mut stmt = conn.prepare("SELECT blocker_id, blocked_id FROM dependencies")?;
    let deps: Vec<(i64, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(deps)
}

#[allow(dead_code)]
pub fn all_notes(conn: &Connection) -> Result<Vec<Note>, ItrError> {
    let mut stmt =
        conn.prepare("SELECT id, issue_id, content, agent, created_at FROM notes ORDER BY id")?;
    let notes: Vec<Note> = stmt
        .query_map([], row_to_note)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(notes)
}

// --- Events (Audit Log) ---

pub fn record_event(
    conn: &Connection,
    issue_id: i64,
    field: &str,
    old_value: &str,
    new_value: &str,
) -> Result<(), ItrError> {
    let agent = env::var("ITR_AGENT").unwrap_or_default();
    conn.execute(
        "INSERT INTO events (issue_id, field, old_value, new_value, agent)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![issue_id, field, old_value, new_value, agent],
    )?;
    Ok(())
}

pub fn get_events_for_issue(conn: &Connection, issue_id: i64) -> Result<Vec<Event>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT id, issue_id, field, old_value, new_value, agent, created_at
         FROM events WHERE issue_id = ?1 ORDER BY created_at ASC",
    )?;
    let events: Vec<Event> = stmt
        .query_map(params![issue_id], row_to_event)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(events)
}

pub fn get_recent_events(
    conn: &Connection,
    limit: usize,
    since: Option<&str>,
) -> Result<Vec<Event>, ItrError> {
    let (sql, param_values): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
        if let Some(since_ts) = since {
            (
                "SELECT id, issue_id, field, old_value, new_value, agent, created_at
                 FROM events WHERE created_at >= ?1 ORDER BY created_at DESC LIMIT ?2"
                    .to_string(),
                vec![Box::new(since_ts.to_string()), Box::new(limit as i64)],
            )
        } else {
            (
                "SELECT id, issue_id, field, old_value, new_value, agent, created_at
                 FROM events ORDER BY created_at DESC LIMIT ?1"
                    .to_string(),
                vec![Box::new(limit as i64)],
            )
        };
    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let events: Vec<Event> = stmt
        .query_map(params_ref.as_slice(), row_to_event)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(events)
}

/// Fetch events with every filter applied in SQL before the limit (#170).
///
/// Returns the newest matching events first. Filters: optional issue scope,
/// optional `created_at >= since`, optional exact agent match. Filtering
/// before `LIMIT` is the point — limiting first would hide older matching
/// events behind newer non-matching ones.
pub fn get_events_filtered(
    conn: &Connection,
    issue_id: Option<i64>,
    limit: usize,
    since: Option<&str>,
    agent: Option<&str>,
) -> Result<Vec<Event>, ItrError> {
    let mut sql = String::from(
        "SELECT id, issue_id, field, old_value, new_value, agent, created_at FROM events",
    );
    let mut clauses: Vec<String> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    if let Some(id) = issue_id {
        values.push(Box::new(id));
        clauses.push(format!("issue_id = ?{}", values.len()));
    }
    if let Some(ts) = since {
        values.push(Box::new(ts.to_string()));
        clauses.push(format!("created_at >= ?{}", values.len()));
    }
    if let Some(name) = agent {
        values.push(Box::new(name.to_string()));
        clauses.push(format!("agent = ?{}", values.len()));
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    values.push(Box::new(limit as i64));
    sql.push_str(&format!(
        " ORDER BY created_at DESC, id DESC LIMIT ?{}",
        values.len()
    ));

    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        values.iter().map(std::convert::AsRef::as_ref).collect();
    let mut stmt = conn.prepare(&sql)?;
    let events: Vec<Event> = stmt
        .query_map(params_ref.as_slice(), row_to_event)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(events)
}

// --- Relations ---

pub fn add_relation(
    conn: &Connection,
    source_id: i64,
    target_id: i64,
    relation_type: &str,
) -> Result<bool, ItrError> {
    if source_id == target_id {
        return Err(ItrError::InvalidValue {
            field: "relation".to_string(),
            value: "self".to_string(),
            valid: "source and target must be different issues".to_string(),
        });
    }
    if !issue_exists(conn, source_id)? {
        return Err(ItrError::NotFound(source_id));
    }
    if !issue_exists(conn, target_id)? {
        return Err(ItrError::NotFound(target_id));
    }

    let exists: bool = conn.query_row(
        "SELECT COUNT(*) > 0 FROM relations WHERE source_id = ?1 AND target_id = ?2 AND relation_type = ?3",
        params![source_id, target_id, relation_type],
        |row| row.get(0),
    )?;
    if exists {
        return Ok(false);
    }

    conn.execute(
        "INSERT INTO relations (source_id, target_id, relation_type) VALUES (?1, ?2, ?3)",
        params![source_id, target_id, relation_type],
    )?;

    record_event(
        conn,
        source_id,
        "relation_added",
        "",
        &format!("{}:{}", relation_type, target_id),
    )?;
    Ok(true)
}

/// Removes relations between a pair of issues, matching the pair in EITHER
/// direction — `get_relations` displays both directions, so unrelate must
/// accept the pair however the caller saw it (#186). An optional
/// `relation_type` filter limits removal to one type; with `None`, every
/// typed link between the pair is removed. Returns the removed relations so
/// callers can report exactly what was deleted (type + stored direction).
pub fn remove_relation(
    conn: &Connection,
    issue_id: i64,
    other_id: i64,
    relation_type: Option<&str>,
) -> Result<Vec<Relation>, ItrError> {
    if !issue_exists(conn, issue_id)? {
        return Err(ItrError::NotFound(issue_id));
    }
    if !issue_exists(conn, other_id)? {
        return Err(ItrError::NotFound(other_id));
    }

    let mut stmt = conn.prepare(
        "SELECT id, source_id, target_id, relation_type, created_at
         FROM relations
         WHERE ((source_id = ?1 AND target_id = ?2)
             OR (source_id = ?2 AND target_id = ?1))
           AND (?3 IS NULL OR relation_type = ?3)
         ORDER BY id",
    )?;
    let matched: Vec<Relation> = stmt
        .query_map(params![issue_id, other_id, relation_type], row_to_relation)?
        .collect::<Result<Vec<_>, _>>()?;

    for relation in &matched {
        conn.execute("DELETE FROM relations WHERE id = ?1", params![relation.id])?;
        // Mirror relation_added's `type:target` value so the audit log keeps
        // enough detail to reconstruct exactly which typed link was removed.
        record_event(
            conn,
            relation.source_id,
            "relation_removed",
            &format!("{}:{}", relation.relation_type, relation.target_id),
            "",
        )?;
    }
    Ok(matched)
}

pub fn get_relations(conn: &Connection, issue_id: i64) -> Result<Vec<Relation>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_id, target_id, relation_type, created_at
         FROM relations WHERE source_id = ?1 OR target_id = ?1
         ORDER BY created_at ASC",
    )?;
    let relations: Vec<Relation> = stmt
        .query_map(params![issue_id], row_to_relation)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(relations)
}

pub fn all_relations(conn: &Connection) -> Result<Vec<Relation>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT id, source_id, target_id, relation_type, created_at
         FROM relations ORDER BY id",
    )?;
    let relations: Vec<Relation> = stmt
        .query_map([], row_to_relation)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(relations)
}

// --- FTS5 Full-Text Search ---

pub fn has_fts(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='issues_fts'",
        [],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// FTS5 table definition. `contentless_delete=1` (`SQLite >= 3.43`) allows rows
/// to be removed by rowid alone, without knowing the previously indexed
/// values. That keeps delete-then-insert reindexing correct for every writer,
/// including `INSERT OR REPLACE` (which bypasses delete triggers when
/// recursive triggers are off, as they are by default).
const FTS_CREATE: &str = "CREATE VIRTUAL TABLE IF NOT EXISTS issues_fts USING fts5(
    title, context, acceptance, tags_text, files_text, skills_text, close_reason,
    content='', contentless_delete=1
);";

/// Triggers that keep `issues_fts` in sync with every write path to `issues`,
/// including raw SQL writers that never call `fts_index_issue` (e.g. the UI's
/// dangerous SQL mode). The JSON-array columns (tags/files/skills) are indexed
/// as their raw JSON text; the default unicode61 tokenizer treats punctuation
/// as separators, so the tokens are identical to space-joined values.
/// `UPDATE OF` is restricted to searchable columns so the `updated_at`
/// touch trigger and status-only updates skip the reindex.
const FTS_TRIGGERS: &str = "
CREATE TRIGGER IF NOT EXISTS issues_fts_ai AFTER INSERT ON issues BEGIN
    DELETE FROM issues_fts WHERE rowid = new.id;
    INSERT INTO issues_fts(rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
    VALUES (new.id, new.title, new.context, new.acceptance, new.tags, new.files, new.skills, new.close_reason);
END;
CREATE TRIGGER IF NOT EXISTS issues_fts_ad AFTER DELETE ON issues BEGIN
    DELETE FROM issues_fts WHERE rowid = old.id;
END;
CREATE TRIGGER IF NOT EXISTS issues_fts_au AFTER UPDATE OF title, context, acceptance, tags, files, skills, close_reason ON issues BEGIN
    DELETE FROM issues_fts WHERE rowid = old.id;
    INSERT INTO issues_fts(rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
    VALUES (new.id, new.title, new.context, new.acceptance, new.tags, new.files, new.skills, new.close_reason);
END;
";

const FTS_DROP: &str = "
DROP TRIGGER IF EXISTS issues_fts_ai;
DROP TRIGGER IF EXISTS issues_fts_ad;
DROP TRIGGER IF EXISTS issues_fts_au;
DROP TABLE IF EXISTS issues_fts;
";

/// Returns true if an `issues_fts` table exists but predates the
/// `contentless_delete` + trigger design. The legacy contentless table could
/// not remove a rowid's old tokens, so updates left stale terms searchable.
fn fts_is_legacy(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='issues_fts'",
        [],
        |row| row.get::<_, String>(0),
    )
    .map(|sql| !sql.contains("contentless_delete"))
    .unwrap_or(false)
}

/// Attempt to create the FTS5 virtual table and its sync triggers. Silently
/// does nothing if FTS5 is unavailable (search falls back to LIKE). A legacy
/// stale-token index is dropped and rebuilt in place.
fn try_create_fts(conn: &Connection) {
    if fts_is_legacy(conn) {
        let _ = conn.execute_batch(FTS_DROP);
    }
    let existed = has_fts(conn);
    if conn.execute_batch(FTS_CREATE).is_err() || !has_fts(conn) {
        return; // FTS5 unavailable; search uses the LIKE fallback.
    }
    let _ = conn.execute_batch(FTS_TRIGGERS);
    if !existed {
        // Fresh or migrated index: populate from current issues.
        if let Ok(issues) = all_issues(conn) {
            for issue in &issues {
                fts_index_issue(conn, issue);
            }
        }
    }
}

/// (Re)index a single issue into FTS. The triggers installed by
/// `try_create_fts` already cover every SQL write path; this remains the
/// public entry point for callers that want to force a row's entry (e.g.
/// import, reindex). Delete-then-insert is idempotent because
/// `contentless_delete=1` removes by rowid without needing old values.
pub fn fts_index_issue(conn: &Connection, issue: &Issue) {
    if !has_fts(conn) {
        return;
    }
    let tags_text = issue.tags.join(" ");
    let files_text = issue.files.join(" ");
    let skills_text = issue.skills.join(" ");

    let result = conn
        .execute("DELETE FROM issues_fts WHERE rowid = ?1", params![issue.id])
        .and_then(|_| {
            conn.execute(
                "INSERT INTO issues_fts(rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![issue.id, issue.title, issue.context, issue.acceptance, tags_text, files_text, skills_text, issue.close_reason],
            )
        });
    if let Err(e) = result {
        eprintln!(
            "REVIEW: failed to update search index for issue #{}: {} (run `itr reindex` to rebuild)",
            issue.id, e
        );
    }
}

/// Rebuild the entire FTS index from scratch.
pub fn fts_rebuild(conn: &Connection) -> Result<(), ItrError> {
    // Drop table and triggers, then recreate; try_create_fts repopulates
    // the fresh index from the issues table.
    conn.execute_batch(FTS_DROP)?;
    try_create_fts(conn);

    if !has_fts(conn) {
        return Err(ItrError::InvalidValue {
            field: "fts5".to_string(),
            value: "unavailable".to_string(),
            valid: "SQLite must be compiled with FTS5 support".to_string(),
        });
    }
    Ok(())
}

/// Search using FTS5 MATCH. Returns issue IDs sorted by rank.
pub fn fts_search(conn: &Connection, query: &str) -> Result<Vec<i64>, ItrError> {
    // Escape FTS5 special characters and build OR query for each term
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Ok(vec![]);
    }

    // Build FTS5 query: each term is quoted, joined with AND
    let fts_query: String = terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");

    let mut stmt =
        conn.prepare("SELECT rowid FROM issues_fts WHERE issues_fts MATCH ?1 ORDER BY rank")?;
    let ids: Vec<i64> = stmt
        .query_map(params![fts_query], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

/// Open a fresh in-memory database with the full schema, migrations, and FTS
/// applied. Shared test fixture for unit tests across command modules.
#[cfg(test)]
pub(crate) fn open_test_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    conn.execute_batch(SCHEMA).expect("apply schema");
    migrate_current_schema(&conn).expect("apply migrations");
    try_create_fts(&conn);
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = open_test_db();
        assert!(has_fts(&conn), "bundled SQLite must support FTS5");
        conn
    }

    fn add(conn: &Connection, title: &str) -> Issue {
        insert_issue(
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
        .unwrap()
    }

    // --- #152: FTS staleness on field updates ---

    #[test]
    fn fts_update_title_removes_stale_tokens() {
        let conn = test_conn();
        let issue = add(&conn, "alpha widget");
        assert_eq!(fts_search(&conn, "widget").unwrap(), vec![issue.id]);

        update_issue_field(&conn, issue.id, "title", "gadget beta").unwrap();

        assert!(
            fts_search(&conn, "widget").unwrap().is_empty(),
            "old title token must not be searchable after update"
        );
        assert!(fts_search(&conn, "alpha").unwrap().is_empty());
        assert_eq!(fts_search(&conn, "gadget").unwrap(), vec![issue.id]);
        assert_eq!(fts_search(&conn, "beta").unwrap(), vec![issue.id]);
    }

    #[test]
    fn fts_reflects_updates_to_all_searchable_fields() {
        let cases = [
            ("context", "oldctx", "newctx", "oldctx", "newctx"),
            ("acceptance", "oldacc", "newacc", "oldacc", "newacc"),
            (
                "close_reason",
                "oldreason",
                "newreason",
                "oldreason",
                "newreason",
            ),
            ("tags", r#"["oldtag"]"#, r#"["newtag"]"#, "oldtag", "newtag"),
            (
                "files",
                r#"["src/oldfile.rs"]"#,
                r#"["src/newfile.rs"]"#,
                "oldfile",
                "newfile",
            ),
            (
                "skills",
                r#"["oldskill"]"#,
                r#"["newskill"]"#,
                "oldskill",
                "newskill",
            ),
        ];
        for (field, old_value, new_value, old_term, new_term) in cases {
            let conn = test_conn();
            let issue = add(&conn, "plain title");
            update_issue_field(&conn, issue.id, field, old_value).unwrap();
            assert_eq!(
                fts_search(&conn, old_term).unwrap(),
                vec![issue.id],
                "field {field} should be indexed"
            );

            update_issue_field(&conn, issue.id, field, new_value).unwrap();
            assert!(
                fts_search(&conn, old_term).unwrap().is_empty(),
                "stale {field} token must not be searchable after update"
            );
            assert_eq!(fts_search(&conn, new_term).unwrap(), vec![issue.id]);
        }
    }

    #[test]
    fn fts_stays_fresh_on_direct_sql_update() {
        // Writers that bypass the db helpers (e.g. UI dangerous SQL mode)
        // are covered by the sync triggers.
        let conn = test_conn();
        let issue = add(&conn, "trigger coverage check");
        conn.execute(
            "UPDATE issues SET title = 'zebra crossing' WHERE id = ?1",
            params![issue.id],
        )
        .unwrap();

        assert!(fts_search(&conn, "coverage").unwrap().is_empty());
        assert_eq!(fts_search(&conn, "zebra").unwrap(), vec![issue.id]);
    }

    #[test]
    fn fts_insert_or_replace_reindexes() {
        // The import path uses INSERT OR REPLACE, whose implicit delete does
        // not fire delete triggers; the insert trigger's delete-by-rowid
        // must still clear lingering tokens.
        let conn = test_conn();
        let issue = add(&conn, "original tokens here");
        conn.execute(
            "INSERT OR REPLACE INTO issues (id, title) VALUES (?1, 'replacement text')",
            params![issue.id],
        )
        .unwrap();

        assert!(fts_search(&conn, "original").unwrap().is_empty());
        assert_eq!(fts_search(&conn, "replacement").unwrap(), vec![issue.id]);
    }

    #[test]
    fn fts_delete_removes_entry_and_count_stays_in_sync() {
        let conn = test_conn();
        let a = add(&conn, "first issue");
        let b = add(&conn, "second issue");
        conn.execute("DELETE FROM issues WHERE id = ?1", params![a.id])
            .unwrap();

        assert!(fts_search(&conn, "first").unwrap().is_empty());
        assert_eq!(fts_search(&conn, "second").unwrap(), vec![b.id]);
        // doctor's fts_stale check compares row counts; deletes must not skew it.
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM issues_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fts_count, 1);
    }

    #[test]
    fn fts_legacy_contentless_table_is_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA).unwrap();
        migrate_current_schema(&conn).unwrap();
        // Recreate the legacy index design and its stale-token failure mode.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE issues_fts USING fts5(
                title, context, acceptance, tags_text, files_text, skills_text, close_reason,
                content='', content_rowid=id
            );",
        )
        .unwrap();
        conn.execute("INSERT INTO issues (title) VALUES ('legacy alpha')", [])
            .unwrap();
        let id = conn.last_insert_rowid();
        conn.execute(
            "INSERT OR REPLACE INTO issues_fts(rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
             VALUES (?1, 'legacy alpha', '', '', '', '', '', '')",
            params![id],
        )
        .unwrap();
        conn.execute(
            "UPDATE issues SET title = 'fresh beta' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO issues_fts(rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
             VALUES (?1, 'fresh beta', '', '', '', '', '', '')",
            params![id],
        )
        .unwrap();
        // Legacy bug: the old token is still searchable.
        assert_eq!(fts_search(&conn, "legacy").unwrap(), vec![id]);

        // Opening the DB migrates and rebuilds the index.
        try_create_fts(&conn);
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='issues_fts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(sql.contains("contentless_delete"));
        assert!(fts_search(&conn, "legacy").unwrap().is_empty());
        assert_eq!(fts_search(&conn, "fresh").unwrap(), vec![id]);
        // And the new triggers keep it fresh from now on.
        conn.execute(
            "UPDATE issues SET title = 'final gamma' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        assert!(fts_search(&conn, "fresh").unwrap().is_empty());
        assert_eq!(fts_search(&conn, "gamma").unwrap(), vec![id]);
    }

    // --- #182: LIKE wildcard escaping ---

    #[test]
    fn escape_like_escapes_wildcards() {
        assert_eq!(escape_like("100%"), "100\\%");
        assert_eq!(escape_like("snake_case"), "snake\\_case");
        assert_eq!(escape_like("back\\slash"), "back\\\\slash");
        assert_eq!(escape_like("plain"), "plain");
    }

    #[test]
    fn search_percent_matches_only_literal() {
        let conn = test_conn();
        let _fast = add(&conn, "make it 100x faster");
        let pct = add(&conn, "reach 100% coverage");
        let terms = vec!["100%".to_string()];
        let ids = search_issue_ids(&conn, &terms, &[], &[], &[], true).unwrap();
        assert_eq!(ids, vec![pct.id], "'100%' must not match '100x'");
    }

    #[test]
    fn search_underscore_matches_only_literal() {
        let conn = test_conn();
        let snake = add(&conn, "rename snake_case helper");
        let _other = add(&conn, "rename snakeXcase helper");
        let terms = vec!["snake_case".to_string()];
        let ids = search_issue_ids(&conn, &terms, &[], &[], &[], true).unwrap();
        assert_eq!(ids, vec![snake.id], "'_' must not act as a wildcard");
    }

    #[test]
    fn search_backslash_matches_literally() {
        let conn = test_conn();
        let bs = add(&conn, "fix back\\slash handling");
        let _other = add(&conn, "fix backXslash handling");
        let terms = vec!["back\\slash".to_string()];
        let ids = search_issue_ids(&conn, &terms, &[], &[], &[], true).unwrap();
        assert_eq!(ids, vec![bs.id]);
    }

    #[test]
    fn note_search_percent_matches_only_literal() {
        let conn = test_conn();
        let a = add(&conn, "first plain");
        let b = add(&conn, "second plain");
        add_note(&conn, a.id, "this is 100x faster", "").unwrap();
        add_note(&conn, b.id, "hit 100% done", "").unwrap();
        let terms = vec!["100%".to_string()];
        let ids = search_note_issue_ids(&conn, &terms, &[], &[], &[], true).unwrap();
        assert_eq!(ids, vec![b.id], "'100%' must not match a '100x' note");
    }

    // --- #154 / #172: atomic compare-and-swap claim ---

    fn events_for(conn: &Connection, id: i64, field: &str) -> Vec<Event> {
        get_events_for_issue(conn, id)
            .unwrap()
            .into_iter()
            .filter(|e| e.field == field)
            .collect()
    }

    #[test]
    fn claim_issue_transitions_open_and_assigns_atomically() {
        let conn = test_conn();
        let issue = add(&conn, "claim me");

        let outcome = claim_issue(&conn, issue.id, Some("agent-a")).unwrap();
        assert_eq!(
            outcome,
            ClaimOutcome::Claimed {
                prior_assigned_to: String::new()
            }
        );

        let after = get_issue(&conn, issue.id).unwrap();
        assert_eq!(after.status, "in-progress");
        assert_eq!(after.assigned_to, "agent-a");
        // Both the status transition and the assignment are audit-logged.
        assert_eq!(events_for(&conn, issue.id, "status").len(), 1);
        assert_eq!(events_for(&conn, issue.id, "assigned_to").len(), 1);
    }

    #[test]
    fn claim_issue_refuses_done_issue_without_mutation() {
        let conn = test_conn();
        let issue = add(&conn, "already finished");
        update_issue_field(&conn, issue.id, "status", "done").unwrap();

        let outcome = claim_issue(&conn, issue.id, Some("agent-a")).unwrap();
        assert_eq!(
            outcome,
            ClaimOutcome::NotOpen {
                status: "done".to_string(),
                assigned_to: String::new()
            }
        );

        let after = get_issue(&conn, issue.id).unwrap();
        assert_eq!(after.status, "done", "claim must not resurrect done issues");
        assert_eq!(after.assigned_to, "");
        assert!(events_for(&conn, issue.id, "status").is_empty());
        assert!(events_for(&conn, issue.id, "assigned_to").is_empty());
    }

    #[test]
    fn claim_issue_reports_current_holder_of_in_progress_issue() {
        let conn = test_conn();
        let issue = add(&conn, "someone else's work");
        assert!(matches!(
            claim_issue(&conn, issue.id, Some("agent-a")).unwrap(),
            ClaimOutcome::Claimed { .. }
        ));

        // A second claimer loses the CAS and learns who holds the issue.
        let outcome = claim_issue(&conn, issue.id, Some("agent-b")).unwrap();
        assert_eq!(
            outcome,
            ClaimOutcome::NotOpen {
                status: "in-progress".to_string(),
                assigned_to: "agent-a".to_string()
            }
        );
        let after = get_issue(&conn, issue.id).unwrap();
        assert_eq!(after.assigned_to, "agent-a", "loser must not steal");
    }

    #[test]
    fn claim_issue_missing_id_is_not_found() {
        let conn = test_conn();
        assert!(matches!(
            claim_issue(&conn, 999, None),
            Err(ItrError::NotFound(999))
        ));
    }

    #[test]
    fn concurrent_claims_yield_distinct_winners() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};

        const N: usize = 6;
        let dir = std::env::temp_dir().join(format!(
            "itr-claim-race-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("race.itr.db");

        let ids: Vec<i64> = {
            let conn = init_db(&db_path).unwrap();
            (0..N)
                .map(|i| add(&conn, &format!("racy issue {i}")).id)
                .collect()
        };

        let barrier = Arc::new(Barrier::new(N));
        let mut handles = Vec::new();
        for n in 0..N {
            let barrier = Arc::clone(&barrier);
            let db_path = db_path.clone();
            let ids = ids.clone();
            handles.push(std::thread::spawn(move || -> Option<i64> {
                let conn = open_db(&db_path).unwrap();
                let agent = format!("racer-{n}");
                barrier.wait();
                // Mirrors claim-next: walk candidates, CAS each, keep the win.
                for &id in &ids {
                    if let ClaimOutcome::Claimed { .. } =
                        claim_issue(&conn, id, Some(&agent)).unwrap()
                    {
                        return Some(id);
                    }
                }
                None
            }));
        }

        let winners: Vec<Option<i64>> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let claimed: HashSet<i64> = winners.iter().filter_map(|w| *w).collect();
        assert_eq!(
            claimed.len(),
            N,
            "each claimer must win a distinct issue, got {winners:?}"
        );
        assert_eq!(claimed, ids.iter().copied().collect::<HashSet<_>>());

        let conn = open_db(&db_path).unwrap();
        for &id in &ids {
            let issue = get_issue(&conn, id).unwrap();
            assert_eq!(issue.status, "in-progress");
            assert!(issue.assigned_to.starts_with("racer-"));
            // Exactly one status transition per issue: no double claims.
            assert_eq!(events_for(&conn, id, "status").len(), 1);
        }
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #171: list_issues has a deterministic base order ---

    #[test]
    fn list_issues_returns_rows_in_id_order() {
        let conn = test_conn();
        let a = add(&conn, "first").id;
        let b = add(&conn, "second").id;
        let c = add(&conn, "third").id;
        // Mixed statuses so a status-index scan would group rows by status
        // instead of id without the explicit ORDER BY.
        update_issue_field(&conn, a, "status", "in-progress").unwrap();
        update_issue_field(&conn, c, "status", "in-progress").unwrap();

        let filter = crate::models::ListFilter {
            statuses: vec!["in-progress".to_string(), "open".to_string()],
            include_blocked: true,
            ..crate::models::ListFilter::default()
        };
        let ids: Vec<i64> = list_issues(&conn, &filter)
            .unwrap()
            .iter()
            .map(|i| i.id)
            .collect();
        assert_eq!(ids, vec![a, b, c], "base order must be id ascending");
    }

    // --- #159: parent-cycle guard enforced in the db layer ---

    #[test]
    fn update_issue_parent_rejects_self_parent() {
        let conn = test_conn();
        let issue = add(&conn, "self parent");
        assert!(
            matches!(
                update_issue_parent(&conn, issue.id, Some(issue.id)),
                Err(ItrError::CycleDetected(_))
            ),
            "setting an issue as its own parent must be a cycle error"
        );
        assert_eq!(get_issue(&conn, issue.id).unwrap().parent_id, None);
    }

    #[test]
    fn update_issue_parent_rejects_descendant_parent() {
        let conn = test_conn();
        let root = add(&conn, "root").id;
        let child = add(&conn, "child").id;
        let grandchild = add(&conn, "grandchild").id;
        update_issue_parent(&conn, child, Some(root)).unwrap();
        update_issue_parent(&conn, grandchild, Some(child)).unwrap();

        assert!(
            matches!(
                update_issue_parent(&conn, root, Some(grandchild)),
                Err(ItrError::CycleDetected(_))
            ),
            "a descendant must not become its ancestor's parent"
        );
        assert_eq!(get_issue(&conn, root).unwrap().parent_id, None);

        // Legal re-parenting still works after the rejected attempt.
        update_issue_parent(&conn, grandchild, Some(root)).unwrap();
        assert_eq!(get_issue(&conn, grandchild).unwrap().parent_id, Some(root));
    }

    #[test]
    fn update_issue_parent_rejects_missing_parent() {
        let conn = test_conn();
        let issue = add(&conn, "orphan");
        assert!(matches!(
            update_issue_parent(&conn, issue.id, Some(999)),
            Err(ItrError::NotFound(999))
        ));
    }

    // --- #186: unrelate is direction-aware and type-aware ---

    #[test]
    fn remove_relation_matches_reverse_direction() {
        let conn = test_conn();
        let a = add(&conn, "rel a").id;
        let b = add(&conn, "rel b").id;
        add_relation(&conn, a, b, "related").unwrap();

        // The pair is passed in the opposite direction from how it is stored;
        // get_relations shows both sides, so removal must match both too.
        let removed = remove_relation(&conn, b, a, None).unwrap();
        assert_eq!(removed.len(), 1, "reverse-direction pair must match");
        assert_eq!(removed[0].source_id, a);
        assert_eq!(removed[0].target_id, b);
        assert_eq!(removed[0].relation_type, "related");
        assert!(get_relations(&conn, a).unwrap().is_empty());
    }

    #[test]
    fn remove_relation_type_filter_leaves_other_types_intact() {
        let conn = test_conn();
        let a = add(&conn, "typed a").id;
        let b = add(&conn, "typed b").id;
        add_relation(&conn, a, b, "related").unwrap();
        add_relation(&conn, a, b, "duplicate").unwrap();

        let removed = remove_relation(&conn, a, b, Some("duplicate")).unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].relation_type, "duplicate");

        let remaining = get_relations(&conn, a).unwrap();
        assert_eq!(remaining.len(), 1, "other typed links must survive");
        assert_eq!(remaining[0].relation_type, "related");
    }

    #[test]
    fn remove_relation_without_filter_reports_every_removed_link() {
        let conn = test_conn();
        let a = add(&conn, "multi a").id;
        let b = add(&conn, "multi b").id;
        add_relation(&conn, a, b, "related").unwrap();
        add_relation(&conn, b, a, "duplicate").unwrap();

        let removed = remove_relation(&conn, a, b, None).unwrap();
        let mut types: Vec<&str> = removed.iter().map(|r| r.relation_type.as_str()).collect();
        types.sort_unstable();
        assert_eq!(
            types,
            vec!["duplicate", "related"],
            "unfiltered removal must report every typed link in both directions"
        );
        assert!(get_relations(&conn, a).unwrap().is_empty());
    }

    #[test]
    fn remove_relation_no_match_returns_empty() {
        let conn = test_conn();
        let a = add(&conn, "lonely a").id;
        let b = add(&conn, "lonely b").id;
        assert!(remove_relation(&conn, a, b, None).unwrap().is_empty());

        add_relation(&conn, a, b, "related").unwrap();
        assert!(
            remove_relation(&conn, a, b, Some("duplicate"))
                .unwrap()
                .is_empty(),
            "type filter that matches nothing must remove nothing"
        );
        assert_eq!(get_relations(&conn, a).unwrap().len(), 1);

        assert!(matches!(
            remove_relation(&conn, a, 999, None),
            Err(ItrError::NotFound(999))
        ));
    }

    // --- #191: remove_dependency reports whether an edge existed ---

    #[test]
    fn remove_dependency_reports_whether_an_edge_existed() {
        let conn = test_conn();
        let blocker = add(&conn, "blocker").id;
        let blocked = add(&conn, "blocked").id;

        assert!(
            !remove_dependency(&conn, blocker, blocked).unwrap(),
            "removing a nonexistent edge must report a no-op"
        );

        add_dependency(&conn, blocker, blocked).unwrap();
        assert!(remove_dependency(&conn, blocker, blocked).unwrap());
        assert!(
            !remove_dependency(&conn, blocker, blocked).unwrap(),
            "second removal of the same edge must report a no-op"
        );
    }

    // --- #160: explicit DB overrides are validated, never auto-created ---

    fn missing_db_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "itr-no-such-{tag}-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn nonexistent_cli_override_is_rejected_without_creating_a_file() {
        let path = missing_db_path("cli");
        let resolved = resolve_override_db(None, Some(path.to_str().unwrap()));
        assert!(
            matches!(resolved, Some(Err(ItrError::NoDatabase))),
            "a missing --db path must be NO_DATABASE, got {resolved:?}"
        );
        assert!(
            std::fs::metadata(&path).is_err(),
            "the failed resolution must not leave a junk file on disk"
        );
    }

    #[test]
    fn nonexistent_env_override_is_rejected_without_creating_a_file() {
        let path = missing_db_path("env");
        let resolved = resolve_override_db(Some(path.to_str().unwrap()), None);
        assert!(
            matches!(resolved, Some(Err(ItrError::NoDatabase))),
            "a missing ITR_DB_PATH must be NO_DATABASE, got {resolved:?}"
        );
        assert!(
            std::fs::metadata(&path).is_err(),
            "the failed resolution must not leave a junk file on disk"
        );
    }

    #[test]
    fn empty_env_override_falls_through_to_walk_up() {
        // Empty ITR_DB_PATH is "unset": resolution must defer (None), not
        // open a SQLite temp database.
        assert!(resolve_override_db(Some(""), None).is_none());
        // ...and an empty env var must not mask a real --db override.
        let dir = std::env::temp_dir().join(format!(
            "itr-empty-env-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(".itr.db");
        drop(init_db(&db_path).unwrap());
        let resolved = resolve_override_db(Some(""), Some(db_path.to_str().unwrap()));
        assert!(matches!(resolved, Some(Ok(p)) if p == db_path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_cli_override_is_rejected_not_a_temp_db() {
        assert!(matches!(
            resolve_override_db(None, Some("")),
            Some(Err(ItrError::NoDatabase))
        ));
    }

    #[test]
    fn existing_override_path_resolves() {
        let dir = std::env::temp_dir().join(format!(
            "itr-existing-override-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(".itr.db");
        drop(init_db(&db_path).unwrap());

        let from_env = resolve_override_db(Some(db_path.to_str().unwrap()), None);
        assert!(matches!(from_env, Some(Ok(ref p)) if *p == db_path));
        let from_cli = resolve_override_db(None, Some(db_path.to_str().unwrap()));
        assert!(matches!(from_cli, Some(Ok(ref p)) if *p == db_path));
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- #170: event filters apply in SQL before the limit ---

    fn insert_event_at(conn: &Connection, issue_id: i64, agent: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO events (issue_id, field, old_value, new_value, agent, created_at)
             VALUES (?1, 'status', 'open', 'in-progress', ?2, ?3)",
            params![issue_id, agent, created_at],
        )
        .unwrap();
    }

    #[test]
    fn get_events_filtered_applies_since_to_issue_scope() {
        let conn = test_conn();
        let id = add(&conn, "since target").id;
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-03-01T00:00:00Z");

        let events =
            get_events_filtered(&conn, Some(id), 50, Some("2026-02-01T00:00:00Z"), None).unwrap();
        assert_eq!(events.len(), 1, "--since must filter issue-scoped events");
        assert_eq!(events[0].created_at, "2026-03-01T00:00:00Z");

        let future =
            get_events_filtered(&conn, Some(id), 50, Some("2099-01-01T00:00:00Z"), None).unwrap();
        assert!(future.is_empty(), "a future --since must yield no events");
    }

    #[test]
    fn get_events_filtered_applies_agent_before_limit() {
        let conn = test_conn();
        let id = add(&conn, "agent target").id;
        // alice's event is older than the 3 newest events overall.
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-02T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-03T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-04T00:00:00Z");

        let events = get_events_filtered(&conn, None, 3, None, Some("alice")).unwrap();
        assert_eq!(
            events.len(),
            1,
            "agent filter must run before LIMIT, not after"
        );
        assert_eq!(events[0].agent, "alice");
    }

    #[test]
    fn get_events_filtered_limits_to_newest_matches() {
        let conn = test_conn();
        let id = add(&conn, "limit target").id;
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-02T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-03T00:00:00Z");

        let events = get_events_filtered(&conn, Some(id), 2, None, None).unwrap();
        let stamps: Vec<&str> = events.iter().map(|e| e.created_at.as_str()).collect();
        assert_eq!(
            stamps,
            vec!["2026-01-03T00:00:00Z", "2026-01-02T00:00:00Z"],
            "limit must keep the newest matches, newest first"
        );
    }
}
