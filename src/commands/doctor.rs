use crate::db;
use crate::error::NitError;
use crate::format::Format;
use rusqlite::{params, Connection};

pub fn run(conn: &Connection, fix: bool, fmt: Format) -> Result<(), NitError> {
    let mut problems: Vec<Problem> = Vec::new();
    let mut fixed: Vec<String> = Vec::new();

    // 1. Orphaned dependencies
    let orphaned_deps = find_orphaned_deps(conn)?;
    for (blocker, blocked) in &orphaned_deps {
        problems.push(Problem {
            kind: "orphaned_dependency".to_string(),
            message: format!("Dependency {}->{} references missing issue", blocker, blocked),
            fixable: true,
        });
    }
    if fix && !orphaned_deps.is_empty() {
        fix_orphaned_deps(conn)?;
        fixed.push(format!("Removed {} orphaned dependencies", orphaned_deps.len()));
    }

    // 2. Circular dependency detection
    let cycles = find_cycles(conn)?;
    for cycle in &cycles {
        problems.push(Problem {
            kind: "circular_dependency".to_string(),
            message: format!("Cycle: {}", cycle),
            fixable: false,
        });
    }

    // 3. Issues stuck in-progress > 3 days
    let stuck = find_stuck_in_progress(conn, 3)?;
    for (id, title, days) in &stuck {
        problems.push(Problem {
            kind: "stale_in_progress".to_string(),
            message: format!("Issue {} \"{}\" in-progress for {} days", id, title, days),
            fixable: false,
        });
    }

    // 4. Epics with no children
    let empty_epics = find_empty_epics(conn)?;
    for (id, title) in &empty_epics {
        problems.push(Problem {
            kind: "empty_epic".to_string(),
            message: format!("Epic {} \"{}\" has no children", id, title),
            fixable: false,
        });
    }

    // 5. Done issues still listed as blockers
    let done_blockers = find_done_blockers(conn)?;
    for (blocker_id, blocked_id) in &done_blockers {
        problems.push(Problem {
            kind: "done_blocker".to_string(),
            message: format!(
                "Done/wontfix issue {} still blocks issue {}",
                blocker_id, blocked_id
            ),
            fixable: true,
        });
    }
    if fix && !done_blockers.is_empty() {
        fix_done_blockers(conn)?;
        fixed.push(format!(
            "Removed {} stale blocker relationships",
            done_blockers.len()
        ));
    }

    // Output
    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "problems": problems.iter().map(|p| serde_json::json!({
                    "kind": p.kind,
                    "message": p.message,
                    "fixable": p.fixable,
                })).collect::<Vec<_>>(),
                "fixed": fixed,
                "clean": problems.is_empty(),
            });
            println!("{}", out);
        }
        _ => {
            if problems.is_empty() {
                println!("DOCTOR: All clean");
            } else {
                for p in &problems {
                    let fix_marker = if p.fixable { " [fixable]" } else { "" };
                    println!("PROBLEM: [{}]{} {}", p.kind, fix_marker, p.message);
                }
                for f in &fixed {
                    println!("FIXED: {}", f);
                }
            }
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        // Exit 1 for problems found
        std::process::exit(1);
    }
}

struct Problem {
    kind: String,
    message: String,
    fixable: bool,
}

fn find_orphaned_deps(conn: &Connection) -> Result<Vec<(i64, i64)>, NitError> {
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

fn fix_orphaned_deps(conn: &Connection) -> Result<(), NitError> {
    conn.execute(
        "DELETE FROM dependencies WHERE
         NOT EXISTS (SELECT 1 FROM issues WHERE id = dependencies.blocker_id)
         OR NOT EXISTS (SELECT 1 FROM issues WHERE id = dependencies.blocked_id)",
        [],
    )?;
    Ok(())
}

fn find_cycles(conn: &Connection) -> Result<Vec<String>, NitError> {
    // Simple cycle detection: for each dependency, check if there's a reverse path
    let deps = db::all_dependencies(conn)?;
    let mut cycles = Vec::new();

    for (blocker, blocked) in &deps {
        // Check if blocked can reach blocker
        if can_reach(conn, *blocked, *blocker)? {
            let cycle_str = format!("{} -> ... -> {}", blocker, blocked);
            if !cycles.contains(&cycle_str) {
                cycles.push(cycle_str);
            }
        }
    }
    Ok(cycles)
}

fn can_reach(conn: &Connection, from: i64, to: i64) -> Result<bool, NitError> {
    let mut visited = std::collections::HashSet::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(from);

    while let Some(current) = queue.pop_front() {
        if current == to {
            return Ok(true);
        }
        if !visited.insert(current) {
            continue;
        }
        let blocking = db::get_blocking(conn, current)?;
        for b in blocking {
            if !visited.contains(&b) {
                queue.push_back(b);
            }
        }
    }
    Ok(false)
}

fn find_stuck_in_progress(conn: &Connection, max_days: i64) -> Result<Vec<(i64, String, i64)>, NitError> {
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

fn find_empty_epics(conn: &Connection) -> Result<Vec<(i64, String)>, NitError> {
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

fn find_done_blockers(conn: &Connection) -> Result<Vec<(i64, i64)>, NitError> {
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

fn fix_done_blockers(conn: &Connection) -> Result<(), NitError> {
    conn.execute(
        "DELETE FROM dependencies WHERE blocker_id IN
         (SELECT id FROM issues WHERE status IN ('done', 'wontfix'))",
        [],
    )?;
    Ok(())
}
