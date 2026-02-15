use crate::error::ItrError;
use crate::models::{Issue, Note};
use rusqlite::{params, Connection};
use std::env;
use std::path::{Path, PathBuf};

const SCHEMA: &str = r#"
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
"#;

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
    Ok(conn)
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

pub fn insert_issue(
    conn: &Connection,
    title: &str,
    priority: &str,
    kind: &str,
    context: &str,
    files: &[String],
    tags: &[String],
    acceptance: &str,
    parent_id: Option<i64>,
) -> Result<Issue, ItrError> {
    let files_json = serde_json::to_string(files)?;
    let tags_json = serde_json::to_string(tags)?;

    conn.execute(
        "INSERT INTO issues (title, priority, kind, context, files, tags, acceptance, parent_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![title, priority, kind, context, files_json, tags_json, acceptance, parent_id],
    )?;

    let id = conn.last_insert_rowid();
    get_issue(conn, id)
}

pub fn get_issue(conn: &Connection, id: i64) -> Result<Issue, ItrError> {
    conn.query_row(
        "SELECT id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at
         FROM issues WHERE id = ?1",
        params![id],
        |row| {
            Ok(Issue {
                id: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                priority: row.get(3)?,
                kind: row.get(4)?,
                context: row.get(5)?,
                files: parse_json_array(row.get::<_, String>(6)?),
                tags: parse_json_array(row.get::<_, String>(7)?),
                acceptance: row.get(8)?,
                parent_id: row.get(9)?,
                close_reason: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        },
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

pub fn list_issues(
    conn: &Connection,
    statuses: &[String],
    priorities: &[String],
    kinds: &[String],
    tags: &[String],
    blocked_only: bool,
    include_blocked: bool,
    parent_id: Option<i64>,
    all: bool,
) -> Result<Vec<Issue>, ItrError> {
    let mut sql = String::from(
        "SELECT id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at FROM issues WHERE 1=1",
    );
    let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if !all {
        if !statuses.is_empty() {
            let placeholders: Vec<String> = statuses
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", param_values.len() + i + 1))
                .collect();
            sql.push_str(&format!(" AND status IN ({})", placeholders.join(",")));
            for s in statuses {
                param_values.push(Box::new(s.clone()));
            }
        } else {
            let p1 = param_values.len() + 1;
            let p2 = param_values.len() + 2;
            sql.push_str(&format!(" AND status IN (?{}, ?{})", p1, p2));
            param_values.push(Box::new("open".to_string()));
            param_values.push(Box::new("in-progress".to_string()));
        }
    }

    if !priorities.is_empty() {
        let placeholders: Vec<String> = priorities
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", param_values.len() + i + 1))
            .collect();
        sql.push_str(&format!(" AND priority IN ({})", placeholders.join(",")));
        for p in priorities {
            param_values.push(Box::new(p.clone()));
        }
    }

    if !kinds.is_empty() {
        let placeholders: Vec<String> = kinds
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", param_values.len() + i + 1))
            .collect();
        sql.push_str(&format!(" AND kind IN ({})", placeholders.join(",")));
        for k in kinds {
            param_values.push(Box::new(k.clone()));
        }
    }

    if let Some(pid) = parent_id {
        let p = param_values.len() + 1;
        sql.push_str(&format!(" AND parent_id = ?{}", p));
        param_values.push(Box::new(pid));
    }

    let params_ref: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;
    let issues: Vec<Issue> = stmt
        .query_map(params_ref.as_slice(), |row| {
            Ok(Issue {
                id: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                priority: row.get(3)?,
                kind: row.get(4)?,
                context: row.get(5)?,
                files: parse_json_array(row.get::<_, String>(6)?),
                tags: parse_json_array(row.get::<_, String>(7)?),
                acceptance: row.get(8)?,
                parent_id: row.get(9)?,
                close_reason: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    // Filter by tags (AND logic)
    let issues = if !tags.is_empty() {
        issues
            .into_iter()
            .filter(|i| tags.iter().all(|t| i.tags.contains(t)))
            .collect()
    } else {
        issues
    };

    // Filter by blocked status
    let issues = if blocked_only {
        issues
            .into_iter()
            .filter(|i| is_blocked(conn, i.id).unwrap_or(false))
            .collect()
    } else if !include_blocked && !all {
        issues
            .into_iter()
            .filter(|i| !is_blocked(conn, i.id).unwrap_or(false))
            .collect()
    } else {
        issues
    };

    Ok(issues)
}

pub fn update_issue_field(conn: &Connection, id: i64, field: &str, value: &str) -> Result<(), ItrError> {
    if !issue_exists(conn, id)? {
        return Err(ItrError::NotFound(id));
    }
    let sql = format!("UPDATE issues SET {} = ?1 WHERE id = ?2", field);
    conn.execute(&sql, params![value, id])?;
    Ok(())
}

pub fn update_issue_parent(conn: &Connection, id: i64, parent_id: Option<i64>) -> Result<(), ItrError> {
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

pub fn add_dependency(conn: &Connection, blocker_id: i64, blocked_id: i64) -> Result<bool, ItrError> {
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

pub fn remove_dependency(conn: &Connection, blocker_id: i64, blocked_id: i64) -> Result<(), ItrError> {
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
fn has_path(conn: &Connection, from_id: i64, to_id: i64) -> Result<bool, ItrError> {
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
        let mut stmt = conn.prepare(
            "SELECT blocked_id FROM dependencies WHERE blocker_id = ?1",
        )?;
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
    let mut stmt = conn.prepare(
        "SELECT blocker_id FROM dependencies WHERE blocked_id = ?1",
    )?;
    let ids: Vec<i64> = stmt
        .query_map(params![issue_id], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ids)
}

pub fn get_blocking(conn: &Connection, issue_id: i64) -> Result<Vec<i64>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT blocked_id FROM dependencies WHERE blocker_id = ?1",
    )?;
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
pub fn get_newly_unblocked(conn: &Connection, closed_id: i64) -> Result<Vec<(i64, String)>, ItrError> {
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
        .query_map(params![closed_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

// --- Notes ---

pub fn add_note(conn: &Connection, issue_id: i64, content: &str, agent: &str) -> Result<Note, ItrError> {
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
        |row| {
            Ok(Note {
                id: row.get(0)?,
                issue_id: row.get(1)?,
                content: row.get(2)?,
                agent: row.get(3)?,
                created_at: row.get(4)?,
            })
        },
    )
    .map_err(ItrError::Db)
}

pub fn get_notes(conn: &Connection, issue_id: i64) -> Result<Vec<Note>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT id, issue_id, content, agent, created_at FROM notes WHERE issue_id = ?1 ORDER BY created_at ASC",
    )?;
    let notes: Vec<Note> = stmt
        .query_map(params![issue_id], |row| {
            Ok(Note {
                id: row.get(0)?,
                issue_id: row.get(1)?,
                content: row.get(2)?,
                agent: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(notes)
}

pub fn count_notes(conn: &Connection, issue_id: i64) -> Result<i64, ItrError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM notes WHERE issue_id = ?1",
        params![issue_id],
        |row| row.get(0),
    )?;
    Ok(count)
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
        "SELECT id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at
         FROM issues ORDER BY id",
    )?;
    let issues: Vec<Issue> = stmt
        .query_map([], |row| {
            Ok(Issue {
                id: row.get(0)?,
                title: row.get(1)?,
                status: row.get(2)?,
                priority: row.get(3)?,
                kind: row.get(4)?,
                context: row.get(5)?,
                files: parse_json_array(row.get::<_, String>(6)?),
                tags: parse_json_array(row.get::<_, String>(7)?),
                acceptance: row.get(8)?,
                parent_id: row.get(9)?,
                close_reason: row.get(10)?,
                created_at: row.get(11)?,
                updated_at: row.get(12)?,
            })
        })?
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
    let mut stmt = conn.prepare(
        "SELECT id, issue_id, content, agent, created_at FROM notes ORDER BY id",
    )?;
    let notes: Vec<Note> = stmt
        .query_map([], |row| {
            Ok(Note {
                id: row.get(0)?,
                issue_id: row.get(1)?,
                content: row.get(2)?,
                agent: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(notes)
}
