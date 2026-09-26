use crate::error::ItrError;
use crate::models::{Event, Issue, Note, Relation};
use crate::sanitize;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};
use std::env;
use std::path::{Path, PathBuf};

const SCHEMA_PRAGMAS: &str = r"
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;
";

const SCHEMA: &str = r"
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

/// The `updated_at` touch trigger exactly as `SCHEMA` declares it (a unit
/// test keeps the two in sync). Restore paths that must write `updated_at`
/// verbatim drop it via `suspend_updated_at_trigger` and put it back with
/// `restore_updated_at_trigger` inside the same transaction.
pub const UPDATED_AT_TRIGGER: &str = "CREATE TRIGGER IF NOT EXISTS trg_issues_updated_at
    AFTER UPDATE ON issues
    FOR EACH ROW
BEGIN
    UPDATE issues SET updated_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
    WHERE id = OLD.id;
END;";

/// Drop `trg_issues_updated_at` so the caller can set `updated_at` to an
/// exact value (for example when restoring an export). Always pair with
/// [`restore_updated_at_trigger`] before the enclosing transaction commits.
pub fn suspend_updated_at_trigger(conn: &Connection) -> Result<(), ItrError> {
    conn.execute_batch("DROP TRIGGER IF EXISTS trg_issues_updated_at")?;
    Ok(())
}

/// Recreate `trg_issues_updated_at` after [`suspend_updated_at_trigger`].
pub fn restore_updated_at_trigger(conn: &Connection) -> Result<(), ItrError> {
    conn.execute_batch(UPDATED_AT_TRIGGER)?;
    Ok(())
}

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

/// Resolve a DB address (from `--db` or `ITR_DB_PATH`) to a `.itr.db` file.
///
/// If `path` is an existing **directory**, resolve to `<path>/.itr.db` — a
/// control plane can point at a project root without hand-appending the
/// suffix. Otherwise the path is used verbatim (a `.itr.db` file, or any
/// filename the caller chose). This is address→file mapping only; it does not
/// check that the resulting file exists and never creates anything, so it is
/// shared by both `find_db` (open) and `itr init` (create).
pub fn db_path_for(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_dir() {
        p.join(".itr.db")
    } else {
        PathBuf::from(path)
    }
}

/// Resolve an explicit DB override (`--db` flag or `ITR_DB_PATH` env var).
///
/// Precedence is unified across every command: an explicit `--db` flag wins
/// over an ambient `ITR_DB_PATH`, which wins over the walk-up finder. A
/// control plane sets `ITR_DB_PATH` for its own tracker, then passes `--db
/// <project>` per call to address another project's tracker — the explicit
/// flag must not be silently shadowed by the ambient env (matches `itr init`
/// and the documented rationale in docs/environment.md).
///
/// Returns `None` when no usable override is present — an empty-string `--db`
/// or `ITR_DB_PATH` is treated as unset — so the caller falls through to the
/// walk-up finder. Both flag and env accept a directory (resolved via
/// [`db_path_for`]) or a file path.
///
/// A directory with no `.itr.db`, or a nonexistent path, is rejected with
/// `NoDatabase` instead of letting `Connection::open` create an empty junk
/// file that the walk-up finder would forever discover as a broken database
/// (#160). The offending path is named on stderr because the `NoDatabase`
/// variant carries no payload.
fn resolve_override_db(
    env_path: Option<&str>,
    cli_path: Option<&str>,
) -> Option<Result<PathBuf, ItrError>> {
    let (path, source) = match (cli_path, env_path) {
        (Some(p), _) if !p.is_empty() => (p, "--db"),
        (_, Some(p)) if !p.is_empty() => (p, "ITR_DB_PATH"),
        _ => return None,
    };
    let p = Path::new(path);
    if p.is_dir() {
        let candidate = p.join(".itr.db");
        if candidate.exists() {
            return Some(Ok(candidate));
        }
        eprintln!(
            "ERROR: {source} points to '{path}', a directory with no .itr.db. Run 'itr init --db {path}' to create it."
        );
        return Some(Err(ItrError::NoDatabase));
    }
    if p.exists() {
        Some(Ok(PathBuf::from(path)))
    } else {
        eprintln!(
            "ERROR: {source} points to '{path}', which does not exist. Run 'itr init --db {path}' to create it."
        );
        Some(Err(ItrError::NoDatabase))
    }
}

/// Schema generation this binary understands, stored in `PRAGMA user_version`.
/// Bump it for any change an older binary would mishandle: a new
/// `migrate_current_schema` step, an FTS design change, any DDL change that
/// fails `fresh_schema_matches_generation_fingerprint`, or a data migration
/// (see docs/migrations.md). A file stamped with a higher generation was
/// written by a newer itr, and `open_db` refuses it: an out-of-date install
/// must never rewrite (e.g. `itr reindex`) a schema it does not understand.
/// Generation 1 is the shape reached in v2.10.1 (skills, `assigned_to`,
/// events, relations, and the `contentless_delete=1` FTS index with its sync
/// triggers); the stamp itself first shipped in v3.3.0, so files last opened
/// by an older release carry 0.
pub const SCHEMA_VERSION: i32 = 1;

/// `config` key recording the itr release that last opened the database.
/// Surfaced in the `NewerSchema` error so the message can name the writer.
pub const WRITER_VERSION_KEY: &str = "last_writer_version";

/// The release core of this binary's version (`v3.2.0`, never
/// `v3.2.0-5-gabc1234-dirty`), so dev builds do not rewrite a git-tracked
/// database on every open.
pub fn writer_stamp() -> String {
    normalize_writer_stamp(env!("ITR_VERSION"))
}

fn normalize_writer_stamp(full: &str) -> String {
    let core = full.strip_prefix('v').unwrap_or(full);
    let core = core.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<_> = core.split('.').collect();
    let valid = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    format!(
        "v{}",
        if valid {
            core
        } else {
            env!("CARGO_PKG_VERSION")
        }
    )
}

/// Begin a write transaction that takes the `SQLite` write lock up front
/// (`BEGIN IMMEDIATE`).
///
/// Every read-then-write mutation must use this rather than
/// `Connection::unchecked_transaction` (which is `BEGIN DEFERRED`). In WAL
/// mode a deferred transaction pins its read snapshot at the first SELECT;
/// if another connection commits before this one's first write, the upgrade
/// to a write lock fails with `SQLITE_BUSY_SNAPSHOT` *without* consulting
/// `busy_timeout`, so parallel agents see "database is locked". Taking the
/// lock at BEGIN makes the busy handler wait instead, and guarantees the
/// pre-reads cannot go stale before the writes land.
pub fn write_tx(conn: &Connection) -> Result<Transaction<'_>, ItrError> {
    Ok(Transaction::new_unchecked(
        conn,
        TransactionBehavior::Immediate,
    )?)
}

pub fn open_db(path: &Path) -> Result<Connection, ItrError> {
    open_schema_db(path, false)
}

fn open_schema_db(path: &Path, initialize: bool) -> Result<Connection, ItrError> {
    let conn = Connection::open(path)?;
    // busy_timeout makes concurrent writers (e.g. parallel `itr claim`) wait
    // for the write lock instead of failing immediately with SQLITE_BUSY.
    conn.execute_batch("PRAGMA busy_timeout=5000; PRAGMA foreign_keys=ON;")?;
    check_schema_version(&conn)?;
    if !initialize {
        let missing = missing_schema_objects(&conn)?;
        if missing.is_empty() && !schema_needs_stamp(&conn)? {
            // Deliberately skip journal_mode=WAL: this path must perform zero
            // writes, and any database itr created is already WAL.
            return Ok(conn);
        }
        // Read-only handles cannot migrate or stamp, but can still serve
        // reads. In particular, BEGIN IMMEDIATE itself would fail on them.
        if conn.is_readonly(rusqlite::DatabaseName::Main)? {
            // Only a missing table or column breaks reads. Missing indexes,
            // triggers, FTS repairs and the advisory stamps wait for the next
            // writable open (`itr doctor` reports them meanwhile).
            if missing.iter().any(MissingSchemaObject::is_structural) {
                return Err(ItrError::ReadOnlyNeedsMigration);
            }
            return Ok(conn);
        }
    }
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    migrate_and_stamp_schema(&conn, initialize)?;
    Ok(conn)
}

/// Advisory stamp work (generation pragma and writer config row).
fn schema_needs_stamp(conn: &Connection) -> Result<bool, ItrError> {
    Ok(read_user_version(conn)? != SCHEMA_VERSION
        || config_get(conn, WRITER_VERSION_KEY)? != Some(writer_stamp()))
}

/// Every schema object this binary expects, derived by executing `SCHEMA`
/// and the FTS DDL against a private in-memory database. Deriving it keeps
/// the fast-path check, the read-only check and `itr doctor` from drifting
/// away from `SCHEMA`: adding a table, column, index or trigger there is
/// enough for existing databases to be detected as needing reconciliation.
struct ExpectedSchema {
    /// Every table declared in `SCHEMA` with its column names.
    tables: Vec<(String, Vec<String>)>,
    /// `(type, name)` of every index and trigger declared in `SCHEMA`.
    objects: Vec<(String, String)>,
    /// Whether this build can create the FTS5 index at all.
    fts_available: bool,
    /// The sync triggers declared in `FTS_TRIGGERS`.
    fts_triggers: Vec<String>,
}

fn expected_schema() -> &'static ExpectedSchema {
    static EXPECTED: std::sync::OnceLock<ExpectedSchema> = std::sync::OnceLock::new();
    EXPECTED.get_or_init(|| {
        derive_expected_schema().expect("SCHEMA must apply to an empty in-memory database")
    })
}

fn derive_expected_schema() -> rusqlite::Result<ExpectedSchema> {
    fn names(conn: &Connection, kinds: &str) -> rusqlite::Result<Vec<(String, String)>> {
        let mut stmt = conn.prepare(&format!(
            r"SELECT type, name FROM sqlite_master
              WHERE type IN ({kinds}) AND name NOT LIKE 'sqlite\_%' ESCAPE '\'
              ORDER BY type, name"
        ))?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect()
    }

    let mem = Connection::open_in_memory()?;
    mem.execute_batch(SCHEMA)?;
    let mut tables = Vec::new();
    for (_, table) in names(&mem, "'table'")? {
        let columns = table_columns(&mem, &table)?;
        tables.push((table, columns));
    }
    let objects = names(&mem, "'index', 'trigger'")?;
    let fts_available = mem.execute_batch(FTS_CREATE).is_ok();
    let fts_triggers = if fts_available {
        mem.execute_batch(FTS_TRIGGERS)?;
        names(&mem, "'trigger'")?
            .into_iter()
            .map(|(_, name)| name)
            .filter(|name| !objects.iter().any(|(_, known)| known == name))
            .collect()
    } else {
        Vec::new()
    };
    Ok(ExpectedSchema {
        tables,
        objects,
        fts_available,
        fts_triggers,
    })
}

fn table_columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT name FROM pragma_table_info(?1)")?;
    let rows = stmt.query_map(params![table], |row| row.get(0))?;
    rows.collect()
}

/// A schema object this binary expects but the database lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingSchemaObject {
    /// `table`, `column`, `index`, `trigger`, `fts_index` (the search index
    /// is absent) or `legacy_fts_index` (it has the pre-v2.10.1 shape).
    pub kind: &'static str,
    /// Object name; columns are `table.column`.
    pub name: String,
}

impl MissingSchemaObject {
    /// Missing tables and columns break reads; everything else only degrades
    /// write-side behavior (a stale `updated_at`, a slower query, stale FTS).
    pub fn is_structural(&self) -> bool {
        matches!(self.kind, "table" | "column")
    }
}

impl std::fmt::Display for MissingSchemaObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            "fts_index" => write!(f, "search index {} is missing", self.name),
            "legacy_fts_index" => write!(f, "search index {} has the pre-v2.10.1 shape", self.name),
            kind => write!(f, "{} {} is missing", kind, self.name),
        }
    }
}

/// Compare the database against [`expected_schema`]. Only reads
/// `sqlite_master` and `pragma_table_info`, so it is safe on read-only
/// handles and on the zero-write fast path.
pub fn missing_schema_objects(conn: &Connection) -> Result<Vec<MissingSchemaObject>, ItrError> {
    let expected = expected_schema();
    let mut present = std::collections::HashSet::new();
    {
        let mut stmt = conn.prepare_cached(
            "SELECT type, name FROM sqlite_master WHERE type IN ('table', 'index', 'trigger')",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            present.insert(row?);
        }
    }
    let has = |kind: &str, name: &str| present.contains(&(kind.to_string(), name.to_string()));

    let mut missing = Vec::new();
    for (table, columns) in &expected.tables {
        if !has("table", table) {
            missing.push(MissingSchemaObject {
                kind: "table",
                name: table.clone(),
            });
            continue;
        }
        let actual = table_columns(conn, table)?;
        for column in columns.iter().filter(|c| !actual.contains(c)) {
            missing.push(MissingSchemaObject {
                kind: "column",
                name: format!("{table}.{column}"),
            });
        }
    }
    for (kind, name) in &expected.objects {
        if !has(kind, name) {
            missing.push(MissingSchemaObject {
                kind: if kind == "index" { "index" } else { "trigger" },
                name: name.clone(),
            });
        }
    }
    if expected.fts_available {
        if !has("table", "issues_fts") {
            missing.push(MissingSchemaObject {
                kind: "fts_index",
                name: "issues_fts".to_string(),
            });
        } else if fts_is_legacy(conn) {
            missing.push(MissingSchemaObject {
                kind: "legacy_fts_index",
                name: "issues_fts".to_string(),
            });
        } else {
            for name in expected.fts_triggers.iter().filter(|n| !has("trigger", n)) {
                missing.push(MissingSchemaObject {
                    kind: "trigger",
                    name: name.clone(),
                });
            }
        }
    }
    Ok(missing)
}

fn migrate_and_stamp_schema(conn: &Connection, initialize: bool) -> Result<(), ItrError> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    // Another writer may have advanced the generation since the first check.
    // Hold the write lock until migrations and both advisory stamps finish.
    check_schema_version(&tx)?;
    if initialize {
        // A brand-new file has no tables for the column probes to inspect.
        tx.execute_batch(SCHEMA)?;
    }
    migrate_current_schema(&tx)?;
    // Reconcile (#241): SCHEMA is idempotent (`IF NOT EXISTS` throughout), so
    // re-running it lands every index and trigger it declares on existing
    // databases, not just fresh ones. It must run after the column
    // migrations: an index on a migrated column would fail on an old file
    // with "no such column" if SCHEMA ran first.
    tx.execute_batch(SCHEMA)?;
    try_create_fts(&tx);
    stamp_schema_version(&tx)?;
    tx.commit()?;
    Ok(())
}

/// Bring an already-open writable connection up to this binary's schema:
/// the same migrate + reconcile + FTS repair + stamp transaction that
/// `open_db` runs. Used by `itr doctor --fix`.
pub fn reconcile_schema(conn: &Connection) -> Result<(), ItrError> {
    migrate_and_stamp_schema(conn, false)
}

/// The schema generation stamped in `PRAGMA user_version`.
pub fn schema_generation(conn: &Connection) -> Result<i32, ItrError> {
    read_user_version(conn)
}

fn read_user_version(conn: &Connection) -> Result<i32, ItrError> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Refuse a database with a higher schema generation before WAL setup and
/// again under the write lock. No itr migration or command mutation is applied.
fn check_schema_version(conn: &Connection) -> Result<(), ItrError> {
    let db = read_user_version(conn)?;
    if db > SCHEMA_VERSION {
        let written_by = config_get(conn, WRITER_VERSION_KEY)
            .ok()
            .flatten()
            .map_or_else(
                || "an unknown itr release".to_string(),
                |v| format!("itr {v}"),
            );
        return Err(ItrError::NewerSchema {
            db,
            supported: SCHEMA_VERSION,
            written_by,
            current: env!("ITR_VERSION").to_string(),
        });
    }
    Ok(())
}

/// Record this binary's schema generation and version after a successful
/// open. Both writes are skipped when already current. The stamp is advisory:
/// read-only databases must not fail an open just to refresh it.
fn stamp_schema_version(conn: &Connection) -> Result<(), ItrError> {
    let result = (|| {
        if read_user_version(conn)? < SCHEMA_VERSION {
            conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION};"))?;
        }
        let current = writer_stamp();
        if config_get(conn, WRITER_VERSION_KEY)?.as_deref() != Some(current.as_str()) {
            config_set(conn, WRITER_VERSION_KEY, &current)?;
        }
        Ok(())
    })();
    match result {
        Err(ItrError::Db(rusqlite::Error::SqliteFailure(err, _)))
            if err.code == rusqlite::ErrorCode::ReadOnly =>
        {
            Ok(())
        }
        result => result,
    }
}

fn migrate_current_schema(conn: &Connection) -> Result<(), ItrError> {
    migrate_add_skills(conn)?;
    migrate_add_assigned_to(conn)?;
    migrate_add_events(conn)?;
    migrate_add_relations(conn)?;
    Ok(())
}

fn has_issue_column(conn: &Connection, column: &str) -> Result<bool, ItrError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('issues') WHERE name = ?1)",
        params![column],
        |row| row.get(0),
    )?)
}

fn has_schema_table(conn: &Connection, table: &str) -> Result<bool, ItrError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1)",
        params![table],
        |row| row.get(0),
    )?)
}

fn migrate_add_skills(conn: &Connection) -> Result<(), ItrError> {
    if !has_issue_column(conn, "skills")? {
        conn.execute_batch("ALTER TABLE issues ADD COLUMN skills TEXT NOT NULL DEFAULT '[]';")?;
    }
    Ok(())
}

fn migrate_add_assigned_to(conn: &Connection) -> Result<(), ItrError> {
    if !has_issue_column(conn, "assigned_to")? {
        conn.execute_batch("ALTER TABLE issues ADD COLUMN assigned_to TEXT NOT NULL DEFAULT '';")?;
    }
    Ok(())
}

fn migrate_add_events(conn: &Connection) -> Result<(), ItrError> {
    if !has_schema_table(conn, "events")? {
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
    if !has_schema_table(conn, "relations")? {
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
    open_schema_db(path, true)
}

pub fn get_schema_sql() -> &'static str {
    static FULL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FULL.get_or_init(|| format!("{SCHEMA_PRAGMAS}{SCHEMA}"))
        .as_str()
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
    // Last line of defense: every creation path (add, batch, UI, bulk
    // helpers) stores the same cleaned shape. See `sanitize`.
    let title = require_title(title)?;
    let context = sanitize::clean_text(context);
    let acceptance = sanitize::clean_text(acceptance);
    let assigned_to = sanitize::clean_assignee(assigned_to);
    let files_json = serde_json::to_string(&sanitize::clean_list(files))?;
    let tags_json = serde_json::to_string(&sanitize::clean_list(tags))?;
    let skills_json = serde_json::to_string(&sanitize::clean_skills(skills))?;

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

/// Clean a title and reject it when nothing is left: a title is the one
/// field with no meaningful default.
pub fn require_title(raw: &str) -> Result<String, ItrError> {
    let title = sanitize::clean_title(raw);
    if title.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "title".to_string(),
            value: raw.to_string(),
            valid: "non-empty text".to_string(),
        });
    }
    Ok(title)
}

/// Clean note content and reject it when nothing is left.
fn require_note_content(raw: &str) -> Result<String, ItrError> {
    let content = sanitize::clean_text(raw);
    if content.trim().is_empty() {
        return Err(ItrError::InvalidValue {
            field: "content".to_string(),
            value: raw.to_string(),
            valid: "non-empty string".to_string(),
        });
    }
    Ok(content)
}

/// Clean one column value for [`update_issue_field`]. JSON-array columns
/// must hold a JSON array of strings; anything else is rejected rather than
/// written, because readers could not parse it back.
fn clean_field_value(field: &str, value: &str) -> Result<String, ItrError> {
    Ok(match field {
        "title" => require_title(value)?,
        "context" | "acceptance" | "close_reason" => sanitize::clean_text(value),
        "assigned_to" => sanitize::clean_assignee(value),
        "files" | "tags" | "skills" => {
            let items: Vec<String> =
                serde_json::from_str(value).map_err(|_| ItrError::InvalidValue {
                    field: field.to_string(),
                    value: value.to_string(),
                    valid: "a JSON array of strings".to_string(),
                })?;
            let cleaned = if field == "skills" {
                sanitize::clean_skills(&items)
            } else {
                sanitize::clean_list(&items)
            };
            serde_json::to_string(&cleaned)?
        }
        _ => value.to_string(),
    })
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

/// Decode a JSON-array TEXT column (`files`, `tags`, `skills`).
///
/// Every writer stores a JSON array of strings, but a hand-edited or
/// corrupted row must never be silently read back as `[]` (that is how an
/// export would lose it for good, #242). Salvage what can be salvaged and
/// say so on stderr:
/// - an array of non-string scalars keeps each element as its JSON text;
/// - anything else that is not blank is kept verbatim as a single element.
fn parse_json_array(issue_id: i64, column: &str, raw: String) -> Vec<String> {
    if let Ok(items) = serde_json::from_str::<Vec<String>>(&raw) {
        return items;
    }
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let salvaged: Vec<String> = match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(serde_json::Value::Array(items)) => items
            .into_iter()
            .map(|item| match item {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            })
            .collect(),
        _ => vec![raw.clone()],
    };
    eprintln!(
        "REVIEW: issue #{issue_id} column '{column}' is not a JSON array of strings ({raw}); \
         read as {salvaged:?}. Rewrite it (e.g. `itr update {issue_id} --{column} ...`) to repair the row"
    );
    salvaged
}

/// Read a TEXT column without failing the whole query on one bad cell.
///
/// Invalid UTF-8 or a BLOB (only reachable through raw SQL or file
/// corruption) is decoded lossily with a `REVIEW:` note naming the row, so
/// `list` / `export` still work and the damage is visible instead of fatal.
fn text_col(
    row: &rusqlite::Row,
    idx: usize,
    table: &str,
    column: &str,
) -> rusqlite::Result<String> {
    use rusqlite::types::ValueRef;
    let (text, problem) = match row.get_ref(idx)? {
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) => return Ok(s.to_string()),
            Err(_) => (String::from_utf8_lossy(bytes).into_owned(), "invalid UTF-8"),
        },
        ValueRef::Blob(bytes) => (String::from_utf8_lossy(bytes).into_owned(), "a BLOB"),
        ValueRef::Null => (String::new(), "NULL"),
        ValueRef::Integer(n) => return Ok(n.to_string()),
        ValueRef::Real(f) => return Ok(f.to_string()),
    };
    let row_id: i64 = row.get(0).unwrap_or_default();
    eprintln!("REVIEW: {table} row {row_id} column '{column}' holds {problem}; read as {text:?}");
    Ok(text)
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
    let id: i64 = row.get(0)?;
    let t = |idx: usize, column: &str| text_col(row, idx, "issues", column);
    Ok(Issue {
        id,
        title: t(1, "title")?,
        status: t(2, "status")?,
        priority: t(3, "priority")?,
        kind: t(4, "kind")?,
        context: t(5, "context")?,
        files: parse_json_array(id, "files", t(6, "files")?),
        tags: parse_json_array(id, "tags", t(7, "tags")?),
        skills: parse_json_array(id, "skills", t(8, "skills")?),
        acceptance: t(9, "acceptance")?,
        parent_id: row.get(10)?,
        close_reason: t(11, "close_reason")?,
        created_at: t(12, "created_at")?,
        updated_at: t(13, "updated_at")?,
        assigned_to: t(14, "assigned_to")?,
    })
}

fn row_to_note(row: &rusqlite::Row) -> rusqlite::Result<Note> {
    let t = |idx: usize, column: &str| text_col(row, idx, "notes", column);
    Ok(Note {
        id: row.get(0)?,
        issue_id: row.get(1)?,
        content: t(2, "content")?,
        agent: t(3, "agent")?,
        created_at: t(4, "created_at")?,
    })
}

fn row_to_event(row: &rusqlite::Row) -> rusqlite::Result<Event> {
    let t = |idx: usize, column: &str| text_col(row, idx, "events", column);
    Ok(Event {
        id: row.get(0)?,
        issue_id: row.get(1)?,
        field: t(2, "field")?,
        old_value: t(3, "old_value")?,
        new_value: t(4, "new_value")?,
        agent: t(5, "agent")?,
        created_at: t(6, "created_at")?,
    })
}

fn row_to_relation(row: &rusqlite::Row) -> rusqlite::Result<Relation> {
    Ok(Relation {
        id: row.get(0)?,
        source_id: row.get(1)?,
        target_id: row.get(2)?,
        relation_type: text_col(row, 3, "relations", "relation_type")?,
        created_at: text_col(row, 4, "relations", "created_at")?,
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
    let value = clean_field_value(field, value)?;
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
    let tx = write_tx(conn)?;
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
    // Dependency edges are mutations like any other: record an audit event
    // on the blocked issue so `itr log` shows who blocked it and when (#35
    // lesson — every mutation path records its own event).
    record_event(
        conn,
        blocked_id,
        "dependency_added",
        "",
        &blocker_id.to_string(),
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
    if deleted > 0 {
        record_event(
            conn,
            blocked_id,
            "dependency_removed",
            &blocker_id.to_string(),
            "",
        )?;
    }
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
    let content = require_note_content(content)?;
    let agent = sanitize::clean_line(agent);
    let content = content.as_str();
    conn.execute(
        "INSERT INTO notes (issue_id, content, agent) VALUES (?1, ?2, ?3)",
        params![issue_id, content, agent],
    )?;
    let id = conn.last_insert_rowid();
    // Mirror note_deleted/note_updated: adding a note is an audited mutation
    // too, so multi-ID and bulk note operations show up in `itr log`.
    record_event(conn, issue_id, "note_added", "", content)?;
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
    let content = require_note_content(content)?;
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
    conn.execute(
        "DELETE FROM config WHERE key != ?1",
        params![WRITER_VERSION_KEY],
    )?;
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

/// True when `issues_fts` exists but at least one of its sync triggers does
/// not. Rows written while a trigger was missing left the index stale.
fn fts_triggers_missing(conn: &Connection) -> bool {
    has_fts(conn)
        && expected_schema().fts_triggers.iter().any(|name| {
            !conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='trigger' AND name = ?1)",
                    params![name],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        })
}

/// Attempt to create the FTS5 virtual table and its sync triggers. Silently
/// does nothing if FTS5 is unavailable (search falls back to LIKE). A legacy
/// stale-token index, or an index that lost any sync trigger, is dropped and
/// rebuilt in place so rows written in the meantime are reindexed.
fn try_create_fts(conn: &Connection) {
    if fts_is_legacy(conn) || fts_triggers_missing(conn) {
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
    conn.execute_batch(get_schema_sql()).expect("apply schema");
    migrate_current_schema(&conn).expect("apply migrations");
    try_create_fts(&conn);
    stamp_schema_version(&conn).expect("stamp schema version");
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
    fn updated_at_trigger_const_matches_schema() {
        assert!(
            SCHEMA.contains(UPDATED_AT_TRIGGER),
            "UPDATED_AT_TRIGGER drifted from the trigger declared in SCHEMA"
        );
    }

    #[test]
    fn suspend_and_restore_updated_at_trigger_round_trip() {
        let conn = test_conn();
        let issue = add(&conn, "trigger toggle");
        suspend_updated_at_trigger(&conn).unwrap();
        conn.execute(
            "UPDATE issues SET updated_at = '2020-01-02T00:00:00Z' WHERE id = ?1",
            [issue.id],
        )
        .unwrap();
        assert_eq!(
            get_issue(&conn, issue.id).unwrap().updated_at,
            "2020-01-02T00:00:00Z"
        );
        restore_updated_at_trigger(&conn).unwrap();
        update_issue_field(&conn, issue.id, "title", "touched").unwrap();
        assert_ne!(
            get_issue(&conn, issue.id).unwrap().updated_at,
            "2020-01-02T00:00:00Z",
            "trigger must re-stamp updated_at once restored"
        );
    }

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

    fn schema_test_db_path(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "itr-schema-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".itr.db");
        (dir, path)
    }

    fn user_version(conn: &Connection) -> i32 {
        read_user_version(conn).unwrap()
    }

    fn writer_version(conn: &Connection) -> Option<String> {
        config_get(conn, WRITER_VERSION_KEY).unwrap()
    }

    #[test]
    fn init_stamps_schema_generation_and_writer() {
        let (dir, path) = schema_test_db_path("init");
        let conn = init_db(&path).unwrap();
        assert_eq!(user_version(&conn), SCHEMA_VERSION);
        assert_eq!(writer_version(&conn), Some(writer_stamp()));
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unstamped_db_is_stamped_on_open() {
        let (dir, path) = schema_test_db_path("unstamped");
        {
            // A file written before the guard existed: generation 0, no writer.
            let conn = init_db(&path).unwrap();
            conn.execute_batch("PRAGMA user_version = 0;").unwrap();
            conn.execute(
                "DELETE FROM config WHERE key = ?1",
                params![WRITER_VERSION_KEY],
            )
            .unwrap();
        }
        let conn = open_db(&path).unwrap();
        assert_eq!(user_version(&conn), SCHEMA_VERSION);
        assert_eq!(writer_version(&conn), Some(writer_stamp()));
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn newer_schema_is_refused_before_migrating() {
        let (dir, path) = schema_test_db_path("newer");
        {
            let conn = init_db(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION + 1))
                .unwrap();
            config_set(&conn, WRITER_VERSION_KEY, "v99.0.0").unwrap();
            // If migrations ran on the refused file they would recreate this.
            conn.execute_batch("DROP TABLE events;").unwrap();
            conn.execute_batch("PRAGMA journal_mode=DELETE;").unwrap();
        }

        let err = open_db(&path).unwrap_err();
        match &err {
            ItrError::NewerSchema { db, supported, .. } => {
                assert_eq!(*db, SCHEMA_VERSION + 1);
                assert_eq!(*supported, SCHEMA_VERSION);
            }
            other => panic!("expected NewerSchema, got {other:?}"),
        }
        assert_eq!(err.error_code(), "NEWER_SCHEMA");
        let msg = err.to_string();
        assert!(msg.contains("itr v99.0.0"), "names the writer: {msg}");
        assert!(
            msg.contains(env!("ITR_VERSION")),
            "names this binary: {msg}"
        );
        assert!(
            msg.contains(r".\install.ps1 -Update"),
            "tells how to update: {msg}"
        );
        assert_eq!(msg.lines().count(), 1, "error must stay on one line");
        assert!(matches!(init_db(&path), Err(ItrError::NewerSchema { .. })));

        // No itr migration or command mutation was applied; WAL was not enabled.
        let raw = Connection::open(&path).unwrap();
        assert_eq!(user_version(&raw), SCHEMA_VERSION + 1);
        assert_eq!(writer_version(&raw).as_deref(), Some("v99.0.0"));
        let journal_mode: String = raw
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(journal_mode, "delete");
        let has_events: bool = raw
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='events'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!has_events, "migrations must not run on a refused database");
        drop(raw);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn up_to_date_open_performs_no_write() {
        for journal_mode in ["WAL", "DELETE"] {
            let (dir, path) = schema_test_db_path("current");
            let conn = init_db(&path).unwrap();
            conn.execute_batch(&format!("PRAGMA journal_mode={journal_mode};"))
                .unwrap();
            drop(conn);
            let before = std::fs::read(&path).unwrap();
            drop(open_db(&path).unwrap());
            let after = std::fs::read(&path).unwrap();
            assert_eq!(
                before, after,
                "opening a current database must not modify it ({journal_mode})"
            );
            assert!(
                !path.with_extension("db-wal").exists(),
                "no WAL sidecar after a clean read-only open"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn writer_stamp_normalizes_release_core() {
        for full in [
            "v3.2.0",
            "v3.2.0-8-gabc1234",
            "v3.2.0-8-gabc1234-dirty",
            "3.2.0+abc1234",
            "3.2.0+abc1234-dirty",
            "3.2.0",
        ] {
            assert_eq!(normalize_writer_stamp(full), "v3.2.0", "{full}");
        }
        for full in [
            "", "abc1234", "v3.2", "3.2.0.1", "3.x.0", "3..0", " 3.2.0", "vv3.2.0",
        ] {
            assert_eq!(
                normalize_writer_stamp(full),
                format!("v{}", env!("CARGO_PKG_VERSION")),
                "{full}"
            );
        }
    }

    #[test]
    fn config_reset_preserves_writer_stamp() {
        let conn = test_conn();
        config_set(&conn, "default_priority", "high").unwrap();
        config_set(&conn, WRITER_VERSION_KEY, "v99.0.0").unwrap();
        config_reset(&conn).unwrap();
        assert_eq!(
            config_list(&conn).unwrap(),
            vec![(WRITER_VERSION_KEY.to_string(), "v99.0.0".to_string())]
        );
    }

    #[test]
    fn readonly_db_with_different_writer_opens() {
        let (dir, path) = schema_test_db_path("readonly");
        let conn = init_db(&path).unwrap();
        config_set(&conn, WRITER_VERSION_KEY, "v0.0.1").unwrap();
        // Cover read-only files that would otherwise require a WAL mode write.
        conn.execute_batch("PRAGMA journal_mode=DELETE;").unwrap();
        drop(conn);

        let permissions = std::fs::metadata(&path).unwrap().permissions();
        let mut readonly = permissions.clone();
        readonly.set_readonly(true);
        std::fs::set_permissions(&path, readonly).unwrap();
        let opened = open_db(&path);
        // Restore the original permissions even if open failed (Windows cleanup).
        std::fs::set_permissions(&path, permissions).unwrap();
        let conn = opened.unwrap();
        assert_eq!(user_version(&conn), SCHEMA_VERSION);
        if conn.is_readonly(rusqlite::DatabaseName::Main).unwrap() {
            assert_eq!(
                writer_version(&conn).as_deref(),
                Some("v0.0.1"),
                "read-only open must preserve the existing writer stamp"
            );
        } else {
            assert_eq!(
                writer_version(&conn),
                Some(writer_stamp()),
                "writable open must update the writer stamp to this binary's stamp"
            );
        }
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // Each historical migration must be detected even when the stamp is current.
    const STRUCTURAL_MIGRATION_GAPS: [&str; 4] = [
        "ALTER TABLE issues DROP COLUMN skills;",
        "ALTER TABLE issues DROP COLUMN assigned_to;",
        "DROP TABLE events;",
        "DROP TABLE relations;",
    ];

    #[test]
    fn writable_open_applies_each_structural_migration() {
        for gap in STRUCTURAL_MIGRATION_GAPS {
            let (dir, path) = schema_test_db_path("migrate");
            let conn = init_db(&path).unwrap();
            conn.execute_batch(FTS_DROP).unwrap();
            conn.execute_batch(gap).unwrap();
            drop(conn);

            let conn = open_db(&path).unwrap();
            assert!(has_issue_column(&conn, "skills").unwrap(), "{gap}");
            assert!(has_issue_column(&conn, "assigned_to").unwrap(), "{gap}");
            assert!(has_schema_table(&conn, "events").unwrap(), "{gap}");
            assert!(has_schema_table(&conn, "relations").unwrap(), "{gap}");
            assert!(has_fts(&conn));
            drop(conn);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    fn readonly_uri(path: &Path) -> PathBuf {
        // URI mode enforces a read-only handle even when tests run as root.
        PathBuf::from(format!(
            "file:{}?mode=ro",
            path.to_string_lossy().replace('\\', "/")
        ))
    }

    #[test]
    fn readonly_db_needing_structural_migration_is_refused() {
        for gap in STRUCTURAL_MIGRATION_GAPS {
            let (dir, path) = schema_test_db_path("readonly-migration");
            let conn = init_db(&path).unwrap();
            conn.execute_batch(FTS_DROP).unwrap();
            conn.execute_batch(gap).unwrap();
            conn.execute_batch("PRAGMA user_version=0; PRAGMA journal_mode=DELETE;")
                .unwrap();
            drop(conn);
            let before = std::fs::read(&path).unwrap();

            let err = open_db(&readonly_uri(&path)).unwrap_err();
            assert!(
                matches!(err, ItrError::ReadOnlyNeedsMigration),
                "{gap}: {err}"
            );
            assert_eq!(err.error_code(), "READONLY_NEEDS_MIGRATION");
            assert_eq!(
                err.to_string(),
                "database is read-only and needs migration; reopen it writable"
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn readonly_db_with_fts_only_differences_opens() {
        for legacy in [false, true] {
            let (dir, path) = schema_test_db_path("readonly-fts");
            let conn = init_db(&path).unwrap();
            conn.execute_batch(FTS_DROP).unwrap();
            if legacy {
                conn.execute_batch(
                    "CREATE VIRTUAL TABLE issues_fts USING fts5(title, content='');",
                )
                .unwrap();
            }
            conn.execute_batch("PRAGMA user_version=0; PRAGMA journal_mode=DELETE;")
                .unwrap();
            config_set(&conn, WRITER_VERSION_KEY, "v0.0.1").unwrap();
            drop(conn);
            let before = std::fs::read(&path).unwrap();

            let conn = open_db(&readonly_uri(&path)).unwrap();
            assert!(conn.is_readonly(rusqlite::DatabaseName::Main).unwrap());
            assert_eq!(user_version(&conn), 0);
            assert_eq!(writer_version(&conn).as_deref(), Some("v0.0.1"));
            assert_eq!(has_fts(&conn), legacy);
            assert_eq!(fts_is_legacy(&conn), legacy);
            drop(conn);
            assert_eq!(std::fs::read(&path).unwrap(), before);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn writable_open_repairs_fts_with_current_stamp() {
        for legacy in [false, true] {
            let (dir, path) = schema_test_db_path("repair-fts");
            let conn = init_db(&path).unwrap();
            let issue = add(&conn, "searchable migration");
            conn.execute_batch(FTS_DROP).unwrap();
            if legacy {
                conn.execute_batch(
                    "CREATE VIRTUAL TABLE issues_fts USING fts5(title, content='');",
                )
                .unwrap();
            }
            drop(conn);

            let conn = open_db(&path).unwrap();
            assert!(has_fts(&conn));
            assert!(!fts_is_legacy(&conn));
            assert_eq!(fts_search(&conn, "searchable").unwrap(), vec![issue.id]);
            drop(conn);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn readonly_stamp_writes_are_advisory() {
        for generation in [0, SCHEMA_VERSION] {
            let conn = test_conn();
            config_set(&conn, WRITER_VERSION_KEY, "v0.0.1").unwrap();
            conn.execute_batch(&format!(
                "PRAGMA user_version={generation}; PRAGMA query_only=ON;"
            ))
            .unwrap();
            stamp_schema_version(&conn).unwrap();
            assert_eq!(user_version(&conn), generation);
            assert_eq!(writer_version(&conn).as_deref(), Some("v0.0.1"));
        }
    }

    #[test]
    fn up_to_date_open_succeeds_under_write_lock() {
        let (dir, path) = schema_test_db_path("busy-open");
        drop(init_db(&path).unwrap());
        let other = Connection::open(&path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let before = std::fs::read(&path).unwrap();
        let wal_path = path.with_extension("db-wal");
        let wal_before = std::fs::read(&wal_path).unwrap();

        let conn = open_db(&path).expect("current readers must not wait for a writer");
        assert_eq!(user_version(&conn), SCHEMA_VERSION);
        assert_eq!(writer_version(&conn), Some(writer_stamp()));
        let changes: i64 = conn
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(changes, 0);
        drop(conn);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(std::fs::read(&wal_path).unwrap(), wal_before);
        other.execute_batch("ROLLBACK;").unwrap();
        drop(other);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_generation_is_rechecked_under_write_lock() {
        for initialize in [false, true] {
            let (dir, path) = schema_test_db_path("recheck");
            let conn = init_db(&path).unwrap();
            check_schema_version(&conn).unwrap();
            // A concurrent writer advances the generation after our first check.
            let other = Connection::open(&path).unwrap();
            other
                .execute_batch(&format!(
                    "BEGIN IMMEDIATE; PRAGMA user_version={}; DROP TABLE events; COMMIT;",
                    SCHEMA_VERSION + 1
                ))
                .unwrap();
            assert!(matches!(
                migrate_and_stamp_schema(&conn, initialize),
                Err(ItrError::NewerSchema { .. })
            ));
            assert_eq!(user_version(&conn), SCHEMA_VERSION + 1);
            assert!(conn.prepare("SELECT * FROM events").is_err());
            drop(other);
            drop(conn);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    fn object_names(conn: &Connection) -> Vec<(String, String)> {
        let mut stmt = conn
            .prepare(
                r"SELECT type, name FROM sqlite_master
                  WHERE type IN ('table', 'index', 'trigger')
                    AND name NOT LIKE 'sqlite\_%' ESCAPE '\'
                  ORDER BY type, name",
            )
            .unwrap();
        stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn has_trigger(conn: &Connection, name: &str) -> bool {
        object_names(conn)
            .iter()
            .any(|(kind, n)| kind == "trigger" && n == name)
    }

    #[test]
    fn expected_schema_is_derived_from_schema_and_fts_ddl() {
        let expected = expected_schema();
        let tables: Vec<&str> = expected.tables.iter().map(|(t, _)| t.as_str()).collect();
        for table in [
            "config",
            "dependencies",
            "events",
            "issues",
            "notes",
            "relations",
        ] {
            assert!(tables.contains(&table), "{table}");
        }
        let issue_columns = &expected
            .tables
            .iter()
            .find(|(t, _)| t == "issues")
            .unwrap()
            .1;
        assert!(issue_columns.contains(&"skills".to_string()));
        assert!(issue_columns.contains(&"assigned_to".to_string()));
        assert_eq!(
            expected.objects.len(),
            12,
            "11 indexes + trg_issues_updated_at"
        );
        assert!(expected
            .objects
            .contains(&("trigger".to_string(), "trg_issues_updated_at".to_string())));
        assert!(expected.fts_available);
        assert_eq!(
            expected.fts_triggers,
            vec!["issues_fts_ad", "issues_fts_ai", "issues_fts_au"]
        );
        let conn = init_db(Path::new(":memory:")).unwrap();
        assert_eq!(missing_schema_objects(&conn).unwrap(), vec![]);
    }

    // #241: objects declared only in SCHEMA must reach existing databases,
    // even when the generation and writer stamps are already current.
    #[test]
    fn writable_open_restores_schema_only_objects() {
        let (dir, path) = schema_test_db_path("reconcile");
        let fresh = {
            let conn = init_db(&path).unwrap();
            let names = object_names(&conn);
            conn.execute_batch(
                "DROP INDEX idx_issues_status;
                 DROP INDEX idx_notes_issue;
                 DROP INDEX idx_events_issue;
                 DROP TRIGGER trg_issues_updated_at;",
            )
            .unwrap();
            let missing = missing_schema_objects(&conn).unwrap();
            assert_eq!(missing.len(), 4, "{missing:?}");
            assert!(!missing.iter().any(MissingSchemaObject::is_structural));
            names
        };

        let conn = open_db(&path).unwrap();
        assert_eq!(object_names(&conn), fresh);
        let issue = add(&conn, "touch me");
        conn.execute(
            "UPDATE issues SET updated_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            params![issue.id],
        )
        .unwrap();
        update_issue_field(&conn, issue.id, "priority", "high").unwrap();
        assert_ne!(
            get_issue(&conn, issue.id).unwrap().updated_at,
            "2020-01-01T00:00:00Z",
            "restored trigger must stamp updated_at"
        );
        drop(conn);

        // The repaired file now takes the zero-write fast path.
        let before = std::fs::read(&path).unwrap();
        let wal_before = std::fs::read(path.with_extension("db-wal")).ok();
        let conn = open_db(&path).unwrap();
        let changes: i64 = conn
            .query_row("SELECT total_changes()", [], |row| row.get(0))
            .unwrap();
        assert_eq!(changes, 0);
        drop(conn);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(
            std::fs::read(path.with_extension("db-wal")).ok(),
            wal_before
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // e3 in the migration review: a dropped FTS sync trigger was invisible to
    // the fast path, and rows written meanwhile stayed stale forever.
    #[test]
    fn writable_open_restores_fts_triggers_and_reindexes() {
        let (dir, path) = schema_test_db_path("fts-triggers");
        let (kept, gone) = {
            let conn = init_db(&path).unwrap();
            let kept = add(&conn, "alpha widget");
            let gone = add(&conn, "doomed gizmo");
            conn.execute_batch("DROP TRIGGER issues_fts_au; DROP TRIGGER issues_fts_ad;")
                .unwrap();
            // Raw writers (e.g. `itr ui --allow-dangerous`) bypass fts_index_issue.
            conn.execute(
                "UPDATE issues SET title = 'omega widget' WHERE id = ?1",
                params![kept.id],
            )
            .unwrap();
            conn.execute("DELETE FROM issues WHERE id = ?1", params![gone.id])
                .unwrap();
            assert_eq!(fts_search(&conn, "alpha").unwrap(), vec![kept.id]);
            assert_eq!(fts_search(&conn, "gizmo").unwrap(), vec![gone.id]);
            (kept, gone)
        };

        let conn = open_db(&path).unwrap();
        for trigger in &expected_schema().fts_triggers {
            assert!(has_trigger(&conn, trigger), "{trigger}");
        }
        assert!(fts_search(&conn, "alpha").unwrap().is_empty());
        assert_eq!(fts_search(&conn, "omega").unwrap(), vec![kept.id]);
        assert!(
            fts_search(&conn, "gizmo").unwrap().is_empty(),
            "entry for deleted issue {} must be gone",
            gone.id
        );
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM issues_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fts_count, 1);
        drop(conn);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readonly_db_missing_index_or_trigger_opens() {
        let (dir, path) = schema_test_db_path("readonly-objects");
        let conn = init_db(&path).unwrap();
        let issue = add(&conn, "readable");
        conn.execute_batch(
            "DROP INDEX idx_issues_priority;
             DROP TRIGGER trg_issues_updated_at;
             DROP TRIGGER issues_fts_ai;
             PRAGMA journal_mode=DELETE;",
        )
        .unwrap();
        drop(conn);
        let before = std::fs::read(&path).unwrap();

        let conn = open_db(&readonly_uri(&path)).unwrap();
        assert!(conn.is_readonly(rusqlite::DatabaseName::Main).unwrap());
        assert_eq!(get_issue(&conn, issue.id).unwrap().title, "readable");
        assert_eq!(missing_schema_objects(&conn).unwrap().len(), 3);
        drop(conn);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    const OLDEST_FIXTURE: &str = include_str!("../tests/fixtures/schema-v1.0-oldest.sql");
    const LEGACY_FTS_FIXTURE: &str = include_str!("../tests/fixtures/schema-v2.0-legacy-fts.sql");

    // Real historical shapes, not a fresh DB with pieces subtracted. If a new
    // SCHEMA column or table lacks its migrate_* helper, this fails because the
    // old file never reaches the expected object set.
    #[test]
    fn old_release_fixtures_upgrade_to_the_complete_schema() {
        let fresh = object_names(&init_db(Path::new(":memory:")).unwrap());
        for (name, fixtures) in [
            ("v1.0", vec![OLDEST_FIXTURE]),
            ("v2.0", vec![OLDEST_FIXTURE, LEGACY_FTS_FIXTURE]),
        ] {
            let (dir, path) = schema_test_db_path(name);
            {
                let raw = Connection::open(&path).unwrap();
                for sql in &fixtures {
                    raw.execute_batch(sql).unwrap();
                }
                assert_eq!(user_version(&raw), 0);
            }

            let conn = open_db(&path).unwrap();
            assert_eq!(missing_schema_objects(&conn).unwrap(), vec![], "{name}");
            assert_eq!(object_names(&conn), fresh, "{name}");
            assert_eq!(user_version(&conn), SCHEMA_VERSION, "{name}");
            assert_eq!(writer_version(&conn), Some(writer_stamp()), "{name}");
            let issues = all_issues(&conn).unwrap();
            assert_eq!(issues.len(), 4, "{name}");
            assert_eq!(get_issue(&conn, 2).unwrap().parent_id, Some(1), "{name}");
            assert_eq!(
                config_get(&conn, "urgency.priority.high")
                    .unwrap()
                    .as_deref(),
                Some("7")
            );
            assert_eq!(fts_search(&conn, "sprocket").unwrap(), vec![2], "{name}");
            assert_eq!(fts_search(&conn, "manual").unwrap(), vec![4], "{name}");
            assert!(fts_search(&conn, "cogwheel").unwrap().is_empty(), "{name}");
            drop(conn);

            let before = std::fs::read(&path).unwrap();
            drop(open_db(&path).unwrap());
            assert_eq!(std::fs::read(&path).unwrap(), before, "{name}: second open");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    // Pair the stable FNV-1a fingerprint with its schema generation.
    const SCHEMA_FINGERPRINT: (i32, u64) = (1, 0xc22d_a286_ef3a_52f4);

    #[test]
    fn fresh_schema_matches_generation_fingerprint() {
        let conn = init_db(Path::new(":memory:")).unwrap();
        let mut stmt = conn
            .prepare(
                r"SELECT sql FROM sqlite_master
             WHERE type IN ('table', 'index', 'trigger', 'view') AND sql IS NOT NULL
               AND NOT (type='table' AND name LIKE 'issues\_fts\_%' ESCAPE '\')
               AND name NOT LIKE 'sqlite\_%' ESCAPE '\'",
            )
            .unwrap();
        let mut definitions: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|sql| {
                sql.unwrap()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();
        // Keep the virtual table and FTS triggers, excluding SQLite-generated DDL.
        definitions.sort_unstable();
        let fingerprint = definitions
            .join("\n")
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
                (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
            });
        assert_eq!(
            (SCHEMA_VERSION, fingerprint),
            SCHEMA_FINGERPRINT,
            "schema changed: bump SCHEMA_VERSION in src/db.rs and update SCHEMA_FINGERPRINT (observed {fingerprint:#018x}) (or the bundled SQLite changed its generated shadow-table DDL)"
        );
    }

    #[test]
    fn fts_legacy_contentless_table_is_migrated() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(get_schema_sql()).unwrap();
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
    fn empty_cli_override_falls_through() {
        // Empty --db is "unset": with no env either, resolution defers (None)
        // to the walk-up finder rather than erroring or opening a temp db.
        assert!(resolve_override_db(None, Some("")).is_none());
        assert!(resolve_override_db(Some(""), Some("")).is_none());
    }

    #[test]
    fn cli_override_wins_over_env() {
        // P2: an explicit --db must beat an ambient ITR_DB_PATH so a control
        // plane can keep env on its own tracker and target another per call.
        let dir = std::env::temp_dir().join(format!(
            "itr-cli-wins-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let a = dir.join("A");
        let b = dir.join("B");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let a_db = a.join(".itr.db");
        let b_db = b.join(".itr.db");
        drop(init_db(&a_db).unwrap());
        drop(init_db(&b_db).unwrap());

        let resolved =
            resolve_override_db(Some(a_db.to_str().unwrap()), Some(b_db.to_str().unwrap()));
        assert!(
            matches!(resolved, Some(Ok(ref p)) if *p == b_db),
            "--db must win over ITR_DB_PATH, got {resolved:?}"
        );
        // Empty --db still yields the env, not the walk-up.
        let env_wins = resolve_override_db(Some(a_db.to_str().unwrap()), Some(""));
        assert!(matches!(env_wins, Some(Ok(ref p)) if *p == a_db));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_override_resolves_to_itr_db_inside() {
        // P1: a directory address opens <dir>/.itr.db; a file address is
        // used verbatim; an empty dir (no .itr.db) is rejected without
        // creating a file.
        let dir = std::env::temp_dir().join(format!(
            "itr-dir-override-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(".itr.db");
        drop(init_db(&db_path).unwrap());

        // Directory form → <dir>/.itr.db
        let from_dir = resolve_override_db(None, Some(dir.to_str().unwrap()));
        assert!(matches!(from_dir, Some(Ok(ref p)) if *p == db_path));
        // db_path_for is the shared address→file mapping.
        assert_eq!(db_path_for(dir.to_str().unwrap()), db_path);
        assert_eq!(
            db_path_for(db_path.to_str().unwrap()),
            db_path,
            "a file path is used verbatim"
        );

        // A sibling directory with no .itr.db is rejected, nothing created.
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let rejected = resolve_override_db(None, Some(empty.to_str().unwrap()));
        assert!(matches!(rejected, Some(Err(ItrError::NoDatabase))));
        assert!(
            std::fs::metadata(empty.join(".itr.db")).is_err(),
            "a directory with no .itr.db must not have one created"
        );
        let _ = std::fs::remove_dir_all(&dir);
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

    // --- Write choke points and read-side salvage (review 2026-09-26) ---

    #[test]
    fn insert_issue_stores_the_cleaned_shape() {
        let conn = test_conn();
        let files = vec![" a.rs ".to_string(), "a.rs".to_string(), String::new()];
        let tags = vec!["A".to_string(), "A".to_string(), " ".to_string()];
        let skills = vec!["Rust".to_string(), " rust ".to_string()];
        let issue = insert_issue(
            &conn,
            "  title\u{1b}[31m\n  ",
            "medium",
            "task",
            "ctx\u{0}\r\n",
            &files,
            &tags,
            &skills,
            "",
            None,
            "  bob ",
        )
        .unwrap();
        assert_eq!(issue.title, "title[31m");
        assert_eq!(issue.context, "ctx");
        assert_eq!(issue.files, vec!["a.rs"]);
        assert_eq!(issue.tags, vec!["A"]);
        assert_eq!(issue.skills, vec!["rust"]);
        assert_eq!(issue.assigned_to, "bob");
    }

    #[test]
    fn empty_title_and_note_are_rejected_at_the_db_layer() {
        let conn = test_conn();
        let err = insert_issue(
            &conn,
            " \n ",
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
        .unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { ref field, .. } if field == "title"));
        let issue = add(&conn, "real");
        assert!(update_issue_field(&conn, issue.id, "title", "   ").is_err());
        assert_eq!(get_issue(&conn, issue.id).unwrap().title, "real");
        assert!(add_note(&conn, issue.id, "  \n ", "a").is_err());
        let note = add_note(&conn, issue.id, "ok", "a").unwrap();
        assert!(update_note(&conn, note.id, "").is_err());
    }

    #[test]
    fn update_issue_field_rejects_non_array_list_values() {
        let conn = test_conn();
        let issue = add(&conn, "lists");
        for bad in ["[\"a\", broken", "\"rust\"", "[1,2]", "{}"] {
            assert!(
                update_issue_field(&conn, issue.id, "tags", bad).is_err(),
                "{bad} must not be stored"
            );
        }
        update_issue_field(&conn, issue.id, "skills", "[\"Go\",\"go\",\" \"]").unwrap();
        assert_eq!(get_issue(&conn, issue.id).unwrap().skills, vec!["go"]);
    }

    /// #242: a malformed JSON-array cell is salvaged, never read back as [].
    #[test]
    fn malformed_list_cells_are_salvaged_not_dropped() {
        assert_eq!(
            parse_json_array(1, "tags", "[\"a\",\"b\"]".into()),
            vec!["a", "b"]
        );
        assert_eq!(parse_json_array(1, "files", "[1,2]".into()), vec!["1", "2"]);
        assert_eq!(
            parse_json_array(1, "skills", "\"rust\"".into()),
            vec!["\"rust\""]
        );
        assert_eq!(
            parse_json_array(1, "tags", "[\"keepme\", broken".into()),
            vec!["[\"keepme\", broken"]
        );
        assert!(parse_json_array(1, "tags", "  ".into()).is_empty());
    }

    /// IE-10: one corrupt cell no longer aborts every read of the table.
    #[test]
    fn invalid_utf8_and_blob_cells_read_lossily() {
        let conn = test_conn();
        let issue = add(&conn, "bytes");
        conn.execute(
            "UPDATE issues SET context = CAST(x'61ff62' AS TEXT) WHERE id = ?1",
            params![issue.id],
        )
        .unwrap();
        let note = add_note(&conn, issue.id, "x", "a").unwrap();
        conn.execute(
            "UPDATE notes SET content = x'00ff' WHERE id = ?1",
            params![note.id],
        )
        .unwrap();
        let read = get_issue(&conn, issue.id).unwrap();
        assert_eq!(read.context, "a\u{fffd}b");
        assert_eq!(all_issues(&conn).unwrap().len(), 1);
        assert_eq!(get_notes(&conn, issue.id).unwrap().len(), 1);
    }
}
