use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use rusqlite::{params, Connection};

/// Machine-readable code reported on stderr when problems remain after a
/// doctor run. Remaining problems are a diagnostic outcome, not a bad
/// user-supplied value, so this deliberately does not reuse `ItrError`
/// codes like `INVALID_VALUE` (see src/error.rs).
const PROBLEMS_REMAIN_CODE: &str = "DOCTOR_PROBLEMS_REMAIN";

/// Problem kind for an expected table, column, index, trigger or FTS object
/// that the database lacks.
const MISSING_SCHEMA_OBJECT: &str = "missing_schema_object";

/// Problem kind for `PRAGMA user_version` below `db::SCHEMA_VERSION`.
const STALE_SCHEMA_GENERATION: &str = "stale_schema_generation";

pub fn run(conn: &Connection, fix: bool, fmt: Format) -> Result<(), ItrError> {
    let report = diagnose(conn, fix)?;

    // Output
    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "problems": report.problems.iter().map(|p| serde_json::json!({
                    "kind": p.kind,
                    "message": p.message,
                    "fixable": p.fixable,
                })).collect::<Vec<_>>(),
                "fixed": report.fixed,
                "clean": report.remaining.is_empty(),
            });
            println!("{}", out);
        }
        _ => {
            if report.problems.is_empty() {
                println!("DOCTOR: All clean");
            } else {
                for p in &report.problems {
                    let fix_marker = if p.fixable { " [fixable]" } else { "" };
                    println!("PROBLEM: [{}]{} {}", p.kind, fix_marker, p.message);
                }
                for f in &report.fixed {
                    println!("FIXED: {}", f);
                }
            }
        }
    }

    // Exit contract: 0 when nothing remains after this run (including when
    // --fix repaired every detected problem); 1 only when problems remain.
    if let Some(msg) = failure_message(&report, fix) {
        if fmt.is_json() {
            eprintln!(
                "{}",
                serde_json::json!({ "error": msg, "code": PROBLEMS_REMAIN_CODE })
            );
        } else {
            eprintln!("ERROR: {}", msg);
        }
        std::process::exit(1);
    }
    Ok(())
}

struct DoctorReport {
    /// Problems detected at the start of the run.
    problems: Vec<Problem>,
    /// Human-readable descriptions of repairs applied (`--fix` only).
    fixed: Vec<String>,
    /// Problems still present after any repairs were applied.
    remaining: Vec<Problem>,
}

fn diagnose(conn: &Connection, fix: bool) -> Result<DoctorReport, ItrError> {
    let problems = detect_problems(conn)?;
    let fixed = if fix {
        apply_fixes(conn, &problems)?
    } else {
        Vec::new()
    };
    // Re-scan after repairs so the exit code reflects what actually remains.
    let remaining = if fixed.is_empty() {
        problems.clone()
    } else {
        detect_problems(conn)?
    };
    Ok(DoctorReport {
        problems,
        fixed,
        remaining,
    })
}

/// `None` when nothing remains (exit 0); `Some(message)` when problems
/// survived the run (exit 1).
fn failure_message(report: &DoctorReport, fix: bool) -> Option<String> {
    if report.remaining.is_empty() {
        return None;
    }
    let n = report.remaining.len();
    let noun = if n == 1 { "problem" } else { "problems" };
    let fixable = report.remaining.iter().filter(|p| p.fixable).count();
    let advice = if !fix && fixable > 0 {
        "Run 'itr doctor --fix' to auto-fix fixable problems"
    } else {
        "Remaining problems need manual attention"
    };
    Some(format!(
        "Doctor found {} {} remaining. {}.",
        n, noun, advice
    ))
}

fn detect_problems(conn: &Connection) -> Result<Vec<Problem>, ItrError> {
    let mut problems: Vec<Problem> = Vec::new();

    // 1. Orphaned dependencies
    for (blocker, blocked) in find_orphaned_deps(conn)? {
        problems.push(Problem {
            kind: "orphaned_dependency".to_string(),
            message: format!(
                "Dependency {}->{} references missing issue",
                blocker, blocked
            ),
            fixable: true,
        });
    }

    // 2. Circular dependency detection
    for cycle in find_cycles(conn)? {
        problems.push(Problem {
            kind: "circular_dependency".to_string(),
            message: format!("Cycle: {}", cycle),
            fixable: false,
        });
    }

    // 3. Issues stuck in-progress > 3 days
    for (id, title, days) in find_stuck_in_progress(conn, 3)? {
        problems.push(Problem {
            kind: "stale_in_progress".to_string(),
            message: format!("Issue {} \"{}\" in-progress for {} days", id, title, days),
            fixable: false,
        });
    }

    // 4. Epics with no children
    for (id, title) in find_empty_epics(conn)? {
        problems.push(Problem {
            kind: "empty_epic".to_string(),
            message: format!("Epic {} \"{}\" has no children", id, title),
            fixable: false,
        });
    }

    // 5. Done issues still listed as blockers
    for (blocker_id, blocked_id) in find_done_blockers(conn)? {
        problems.push(Problem {
            kind: "done_blocker".to_string(),
            message: format!(
                "Done/wontfix issue {} still blocks issue {}",
                blocker_id, blocked_id
            ),
            fixable: true,
        });
    }

    // 6. Schema shape: every table, column, index and trigger this binary
    // declares (derived from db::SCHEMA and the FTS DDL). A writable open
    // already reconciles these, so this mostly fires on read-only handles or
    // when an object was dropped after the open (e.g. raw SQL in `itr ui`).
    let writable = !conn.is_readonly(rusqlite::DatabaseName::Main)?;
    let reopen_hint = if writable {
        ""
    } else {
        " (read-only database: reopen it writable to repair)"
    };
    for missing in db::missing_schema_objects(conn)? {
        problems.push(Problem {
            kind: MISSING_SCHEMA_OBJECT.to_string(),
            message: format!("Schema {missing}{reopen_hint}"),
            fixable: writable,
        });
    }

    // 7. Schema generation stamp behind this binary's generation.
    let generation = db::schema_generation(conn)?;
    if generation < db::SCHEMA_VERSION {
        problems.push(Problem {
            kind: STALE_SCHEMA_GENERATION.to_string(),
            message: format!(
                "Schema generation {} is below this itr's generation {}{}",
                generation,
                db::SCHEMA_VERSION,
                reopen_hint
            ),
            fixable: writable,
        });
    }

    // 8. FTS index health
    if db::has_fts(conn) {
        // FTS exists, check if it's in sync
        let issue_count = db::all_issues(conn)?.len();
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM issues_fts", [], |row| row.get(0))
            .unwrap_or(0);
        if (fts_count as usize) != issue_count {
            problems.push(Problem {
                kind: "fts_stale".to_string(),
                message: format!(
                    "FTS index has {} entries but {} issues exist",
                    fts_count, issue_count
                ),
                fixable: true,
            });
        }
    }

    Ok(problems)
}

fn apply_fixes(conn: &Connection, problems: &[Problem]) -> Result<Vec<String>, ItrError> {
    let mut fixed: Vec<String> = Vec::new();

    // Schema first, so the later repairs run against the full schema. This is
    // the same migrate + reconcile + FTS + stamp transaction as `open_db`.
    let schema_objects = problems
        .iter()
        .filter(|p| p.fixable && p.kind == MISSING_SCHEMA_OBJECT)
        .count();
    let stale_generation = problems
        .iter()
        .any(|p| p.fixable && p.kind == STALE_SCHEMA_GENERATION);
    if schema_objects > 0 || stale_generation {
        db::reconcile_schema(conn)?;
        fixed.push(if schema_objects > 0 {
            format!("Restored {} missing schema objects", schema_objects)
        } else {
            "Stamped the current schema generation".to_string()
        });
    }

    let orphaned = problems
        .iter()
        .filter(|p| p.kind == "orphaned_dependency")
        .count();
    if orphaned > 0 {
        fix_orphaned_deps(conn)?;
        fixed.push(format!("Removed {} orphaned dependencies", orphaned));
    }

    let done_blockers = problems.iter().filter(|p| p.kind == "done_blocker").count();
    if done_blockers > 0 {
        fix_done_blockers(conn)?;
        fixed.push(format!(
            "Removed {} stale blocker relationships",
            done_blockers
        ));
    }

    if problems.iter().any(|p| p.kind == "fts_stale") {
        db::fts_rebuild(conn)?;
        fixed.push("Rebuilt FTS index".to_string());
    }

    Ok(fixed)
}

#[derive(Clone)]
struct Problem {
    kind: String,
    message: String,
    fixable: bool,
}

fn find_orphaned_deps(conn: &Connection) -> Result<Vec<(i64, i64)>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT d.blocker_id, d.blocked_id FROM dependencies d
         WHERE NOT EXISTS (SELECT 1 FROM issues WHERE id = d.blocker_id)
         OR NOT EXISTS (SELECT 1 FROM issues WHERE id = d.blocked_id)",
    )?;
    let results: Vec<(i64, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

fn fix_orphaned_deps(conn: &Connection) -> Result<(), ItrError> {
    conn.execute(
        "DELETE FROM dependencies WHERE
         NOT EXISTS (SELECT 1 FROM issues WHERE id = dependencies.blocker_id)
         OR NOT EXISTS (SELECT 1 FROM issues WHERE id = dependencies.blocked_id)",
        [],
    )?;
    Ok(())
}

fn find_cycles(conn: &Connection) -> Result<Vec<String>, ItrError> {
    // Simple cycle detection: for each dependency, check if there's a reverse path
    let deps = db::all_dependencies(conn)?;
    let mut cycles = Vec::new();

    for (blocker, blocked) in &deps {
        // Check if blocked can reach blocker
        if db::has_path(conn, *blocked, *blocker)? {
            let cycle_str = format!("{} -> ... -> {}", blocker, blocked);
            if !cycles.contains(&cycle_str) {
                cycles.push(cycle_str);
            }
        }
    }
    Ok(cycles)
}

fn find_stuck_in_progress(
    conn: &Connection,
    max_days: i64,
) -> Result<Vec<(i64, String, i64)>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT id, title, CAST((julianday('now') - julianday(updated_at)) AS INTEGER) as days
         FROM issues
         WHERE status = 'in-progress'
         AND CAST((julianday('now') - julianday(updated_at)) AS INTEGER) > ?1",
    )?;
    let results: Vec<(i64, String, i64)> = stmt
        .query_map(params![max_days], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

fn find_empty_epics(conn: &Connection) -> Result<Vec<(i64, String)>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT i.id, i.title FROM issues i
         WHERE i.kind = 'epic'
         AND i.status NOT IN ('done', 'wontfix')
         AND NOT EXISTS (SELECT 1 FROM issues c WHERE c.parent_id = i.id)",
    )?;
    let results: Vec<(i64, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

fn find_done_blockers(conn: &Connection) -> Result<Vec<(i64, i64)>, ItrError> {
    let mut stmt = conn.prepare(
        "SELECT d.blocker_id, d.blocked_id FROM dependencies d
         JOIN issues i ON d.blocker_id = i.id
         WHERE i.status IN ('done', 'wontfix')",
    )?;
    let results: Vec<(i64, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(results)
}

fn fix_done_blockers(conn: &Connection) -> Result<(), ItrError> {
    conn.execute(
        "DELETE FROM dependencies WHERE blocker_id IN
         (SELECT id FROM issues WHERE status IN ('done', 'wontfix'))",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        db::open_test_db()
    }

    fn insert_issue(conn: &Connection, title: &str, kind: &str, status: &str) -> i64 {
        conn.execute(
            "INSERT INTO issues (title, kind, status) VALUES (?1, ?2, ?3)",
            params![title, kind, status],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn insert_dep(conn: &Connection, blocker: i64, blocked: i64) {
        conn.execute(
            "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?1, ?2)",
            params![blocker, blocked],
        )
        .unwrap();
    }

    fn seed_stale_done_blocker(conn: &Connection) {
        let blocker = insert_issue(conn, "done blocker", "task", "done");
        let blocked = insert_issue(conn, "blocked issue", "task", "open");
        insert_dep(conn, blocker, blocked);
    }

    // Issue #166: `doctor --fix` that repairs every detected problem must
    // succeed (exit 0), not report an error about the problems it just fixed.
    #[test]
    fn fix_that_repairs_everything_exits_zero() {
        let conn = test_conn();
        seed_stale_done_blocker(&conn);

        let result = run(&conn, true, Format::Compact);
        assert!(
            result.is_ok(),
            "doctor --fix that repaired everything must exit 0: {:?}",
            result.err()
        );
    }

    #[test]
    fn fix_that_repairs_everything_reports_fixed_and_no_remaining() {
        let conn = test_conn();
        seed_stale_done_blocker(&conn);

        let report = diagnose(&conn, true).unwrap();
        assert_eq!(report.problems.len(), 1);
        assert_eq!(report.problems[0].kind, "done_blocker");
        assert_eq!(
            report.fixed,
            vec!["Removed 1 stale blocker relationships".to_string()]
        );
        assert!(
            report.remaining.is_empty(),
            "fix should leave no remaining problems"
        );
        assert_eq!(failure_message(&report, true), None);
    }

    // Issue #166: exit 1 is reserved for problems that remain after the run,
    // and the failure must not masquerade as a user-input INVALID_VALUE error.
    #[test]
    fn unfixable_problems_remain_after_fix_and_fail_without_invalid_value() {
        let conn = test_conn();
        insert_issue(&conn, "lonely epic", "epic", "open");
        seed_stale_done_blocker(&conn);

        let report = diagnose(&conn, true).unwrap();
        assert_eq!(report.problems.len(), 2);
        assert_eq!(report.fixed.len(), 1);
        assert_eq!(report.remaining.len(), 1);
        assert_eq!(report.remaining[0].kind, "empty_epic");

        let msg = failure_message(&report, true).expect("problems remain, must fail");
        assert!(
            !msg.contains("Invalid value"),
            "diagnostic failure must not use InvalidValue wording: {}",
            msg
        );
        assert!(
            msg.contains("manual attention"),
            "after --fix the advice must not be to re-run --fix: {}",
            msg
        );
        assert_ne!(PROBLEMS_REMAIN_CODE, "INVALID_VALUE");
    }

    #[test]
    fn without_fix_fixable_problems_remain_and_advise_fix() {
        let conn = test_conn();
        seed_stale_done_blocker(&conn);

        let report = diagnose(&conn, false).unwrap();
        assert!(report.fixed.is_empty());
        assert_eq!(report.remaining.len(), 1);

        let msg = failure_message(&report, false).expect("problems remain, must fail");
        assert!(
            msg.contains("itr doctor --fix"),
            "without --fix, fixable problems should suggest --fix: {}",
            msg
        );
    }

    #[test]
    fn clean_database_has_no_failure() {
        let conn = test_conn();
        insert_issue(&conn, "healthy issue", "task", "open");

        let report = diagnose(&conn, false).unwrap();
        assert!(report.problems.is_empty());
        assert!(report.remaining.is_empty());
        assert_eq!(failure_message(&report, false), None);
        run(&conn, false, Format::Compact).unwrap();
    }

    fn kinds(problems: &[Problem]) -> Vec<&str> {
        problems.iter().map(|p| p.kind.as_str()).collect()
    }

    // #241: dropped SCHEMA-only objects are reported, and --fix restores them
    // through the same reconcile transaction `open_db` runs.
    #[test]
    fn missing_schema_objects_are_reported_and_fixed() {
        let conn = test_conn();
        insert_issue(&conn, "healthy issue", "task", "open");
        conn.execute_batch(
            "DROP INDEX idx_issues_status;
             DROP TRIGGER trg_issues_updated_at;
             DROP TRIGGER issues_fts_au;",
        )
        .unwrap();

        let report = diagnose(&conn, false).unwrap();
        assert_eq!(kinds(&report.problems), vec![MISSING_SCHEMA_OBJECT; 3]);
        assert!(report.problems.iter().all(|p| p.fixable));
        let messages: Vec<&str> = report.problems.iter().map(|p| p.message.as_str()).collect();
        assert!(messages.contains(&"Schema index idx_issues_status is missing"));
        assert!(messages.contains(&"Schema trigger trg_issues_updated_at is missing"));
        assert!(messages.contains(&"Schema trigger issues_fts_au is missing"));
        assert!(failure_message(&report, false)
            .unwrap()
            .contains("itr doctor --fix"));

        let report = diagnose(&conn, true).unwrap();
        assert_eq!(
            report.fixed,
            vec!["Restored 3 missing schema objects".to_string()]
        );
        assert!(report.remaining.is_empty());
        assert!(db::missing_schema_objects(&conn).unwrap().is_empty());
    }

    #[test]
    fn stale_schema_generation_is_reported_and_fixed() {
        let conn = test_conn();
        conn.execute_batch("PRAGMA user_version = 0;").unwrap();

        let report = diagnose(&conn, false).unwrap();
        assert_eq!(kinds(&report.problems), vec![STALE_SCHEMA_GENERATION]);
        assert!(report.problems[0].fixable);

        let report = diagnose(&conn, true).unwrap();
        assert!(report.remaining.is_empty());
        assert_eq!(db::schema_generation(&conn).unwrap(), db::SCHEMA_VERSION);
    }

    #[test]
    fn readonly_schema_problems_are_reported_but_not_fixed() {
        let dir = std::env::temp_dir().join(format!(
            "itr-doctor-ro-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(".itr.db");
        {
            let file = db::init_db(&path).unwrap();
            file.execute_batch(
                "DROP INDEX idx_issues_kind; PRAGMA user_version = 0; PRAGMA journal_mode=DELETE;",
            )
            .unwrap();
        }
        // URI mode enforces a read-only handle even when tests run as root.
        let uri = format!("file:{}?mode=ro", path.to_string_lossy().replace('\\', "/"));
        let ro = db::open_db(std::path::Path::new(&uri)).unwrap();
        assert!(ro.is_readonly(rusqlite::DatabaseName::Main).unwrap());

        let report = diagnose(&ro, true).unwrap();
        assert_eq!(
            kinds(&report.problems),
            vec![MISSING_SCHEMA_OBJECT, STALE_SCHEMA_GENERATION]
        );
        assert!(report.problems.iter().all(|p| !p.fixable));
        assert!(report.problems[0].message.contains("reopen it writable"));
        assert!(
            report.fixed.is_empty(),
            "--fix must not write a read-only handle"
        );
        assert_eq!(report.remaining.len(), 2);
        drop(ro);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
