use crate::error::ItrError;
use crate::models::{Event, Issue, Note, Relation};
use rusqlite::{params, Connection};
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

CREATE INDEX IF NOT EXISTS idx_issues_status ON issues(status);
CREATE INDEX IF NOT EXISTS idx_issues_priority ON issues(priority);
CREATE INDEX IF NOT EXISTS idx_issues_kind ON issues(kind);
CREATE INDEX IF NOT EXISTS idx_issues_parent ON issues(parent_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocked ON dependencies(blocked_id);
CREATE INDEX IF NOT EXISTS idx_dependencies_blocker ON dependencies(blocker_id);
CREATE INDEX IF NOT EXISTS idx_notes_issue ON notes(issue_id);

CREATE TRIGGER IF NOT EXISTS trg_issues_updated_at
    AFTER UPDATE ON issues
    FOR EACH ROW
BEGIN
    UPDATE issues SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
    WHERE id = OLD.id;
END;
";

pub fn find_db(override_path: Option<&str>) -> Result<PathBuf, ItrError> {
    // Check env var
    if let Ok(path) = env::var("ITR_DB_PATH") {
        return Ok(PathBuf::from(path));
    }

    // Check CLI override
    if let Some(p) = override_path {
        return Ok(PathBuf::from(p));
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

pub fn open_db(path: &Path) -> Result<Connection, ItrError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    migrate_add_skills(&conn)?;
    migrate_add_assigned_to(&conn)?;
    migrate_add_events(&conn)?;
    migrate_add_relations(&conn)?;
    try_create_fts(&conn);
    Ok(conn)
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
            );
            CREATE INDEX idx_events_issue ON events(issue_id);
            CREATE INDEX idx_events_created ON events(created_at);",
        )?;
    }
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
            );
            CREATE INDEX idx_relations_source ON relations(source_id);
            CREATE INDEX idx_relations_target ON relations(target_id);",
        )?;
    }
    Ok(())
}

pub fn init_db(path: &Path) -> Result<Connection, ItrError> {
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
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

    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(std::convert::AsRef::as_ref).collect();
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
        "title", "status", "priority", "kind", "context", "files", "tags",
        "skills", "acceptance", "close_reason", "assigned_to",
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

pub fn update_issue_parent(
    conn: &Connection,
    id: i64,
    parent_id: Option<i64>,
) -> Result<(), ItrError> {
    if !issue_exists(conn, id)? {
        return Err(ItrError::NotFound(id));
    }
    conn.execute(
        "UPDATE issues SET parent_id = ?1 WHERE id = ?2",
        params![parent_id, id],
    )?;
    Ok(())
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

pub fn remove_dependency(
    conn: &Connection,
    blocker_id: i64,
    blocked_id: i64,
) -> Result<(), ItrError> {
    if !issue_exists(conn, blocker_id)? {
        return Err(ItrError::NotFound(blocker_id));
    }
    if !issue_exists(conn, blocked_id)? {
        return Err(ItrError::NotFound(blocked_id));
    }
    conn.execute(
        "DELETE FROM dependencies WHERE blocker_id = ?1 AND blocked_id = ?2",
        params![blocker_id, blocked_id],
    )?;
    Ok(())
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
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(terms.len() * 8);

    // Each term must match at least one searchable field
    for term in terms {
        let pattern = format!("%{}%", term);
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
            " AND (i.title LIKE ?{} OR i.context LIKE ?{} OR i.acceptance LIKE ?{} OR i.close_reason LIKE ?{} OR i.tags LIKE ?{} OR i.files LIKE ?{} OR i.skills LIKE ?{} OR n.content LIKE ?{})",
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

    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(std::convert::AsRef::as_ref).collect();
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
    let params_ref: Vec<&dyn rusqlite::types::ToSql> =
        param_values.iter().map(std::convert::AsRef::as_ref).collect();
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

pub fn remove_relation(
    conn: &Connection,
    source_id: i64,
    target_id: i64,
) -> Result<bool, ItrError> {
    if !issue_exists(conn, source_id)? {
        return Err(ItrError::NotFound(source_id));
    }
    if !issue_exists(conn, target_id)? {
        return Err(ItrError::NotFound(target_id));
    }

    let deleted = conn.execute(
        "DELETE FROM relations WHERE source_id = ?1 AND target_id = ?2",
        params![source_id, target_id],
    )?;

    if deleted > 0 {
        record_event(
            conn,
            source_id,
            "relation_removed",
            &format!("{}", target_id),
            "",
        )?;
    }
    Ok(deleted > 0)
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

/// Attempt to create the FTS5 virtual table. Silently fails if FTS5 is not available.
fn try_create_fts(conn: &Connection) {
    let _ = conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS issues_fts USING fts5(
            title, context, acceptance, tags_text, files_text, skills_text, close_reason,
            content='', content_rowid=id
        );",
    );
}

/// Index a single issue into FTS. Called after insert/update.
pub fn fts_index_issue(conn: &Connection, issue: &Issue) {
    if !has_fts(conn) {
        return;
    }
    let tags_text = issue.tags.join(" ");
    let files_text = issue.files.join(" ");
    let skills_text = issue.skills.join(" ");

    // Delete old entry, then insert new
    let _ = conn.execute(
        "INSERT OR REPLACE INTO issues_fts(rowid, title, context, acceptance, tags_text, files_text, skills_text, close_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![issue.id, issue.title, issue.context, issue.acceptance, tags_text, files_text, skills_text, issue.close_reason],
    );
}

/// Rebuild the entire FTS index from scratch.
pub fn fts_rebuild(conn: &Connection) -> Result<(), ItrError> {
    // Drop and recreate
    let _ = conn.execute_batch("DROP TABLE IF EXISTS issues_fts;");
    try_create_fts(conn);

    if !has_fts(conn) {
        return Err(ItrError::InvalidValue {
            field: "fts5".to_string(),
            value: "unavailable".to_string(),
            valid: "SQLite must be compiled with FTS5 support".to_string(),
        });
    }

    let issues = all_issues(conn)?;
    for issue in &issues {
        fts_index_issue(conn, issue);
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
