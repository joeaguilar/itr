use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::util;
use rusqlite::Connection;

/// `itr depend <ID>... --on N` — one or more blocked-issue IDs, repeated,
/// comma-separated, or inclusive `A-B` ranges.
///
/// - Exactly one unique ID: unchanged single-issue contract (hard `NOT_FOUND`
///   on either side, hard `CYCLE_DETECTED`).
/// - Multiple unique IDs: all edges are added in one transaction with per-ID
///   soft fallback — a missing ID emits `REVIEW: id N not found; skipped`,
///   and an ID equal to `--on` skips the self-edge. Cycles stay hard errors
///   (cycle detection cannot recover) and roll the whole invocation back.
///   Exit 0 if at least one edge was processed, exit 1 if none were.
pub fn run_multi(
    conn: &Connection,
    id_tokens: &[String],
    on: i64,
    fmt: Format,
) -> Result<(), ItrError> {
    let parsed = util::parse_id_tokens(id_tokens);
    for note in &parsed.notes {
        eprintln!("{}", note);
    }
    for token in &parsed.invalid {
        eprintln!(
            "REVIEW: ignoring non-integer issue ID '{}' — IDs may be repeated, comma-separated, or ranges (e.g. `itr depend 5-8 --on 200`)",
            token
        );
    }
    for id in &parsed.duplicates {
        eprintln!(
            "REVIEW: duplicate issue ID {} requested; adding the edge once",
            id
        );
    }
    if parsed.ids.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid:
                "integer issue IDs, repeated, comma-separated, or ranges (e.g. `itr depend 5-8 --on 200`)"
                    .to_string(),
        });
    }

    if parsed.ids.len() == 1 {
        return run(conn, parsed.ids[0], on, fmt);
    }

    // A missing --on blocker can never soft-recover: fail before touching
    // anything, matching the single-ID behavior.
    if !db::issue_exists(conn, on)? {
        return Err(ItrError::NotFound(on));
    }

    let tx = conn.unchecked_transaction()?;
    let mut edges: Vec<(i64, bool)> = Vec::new();
    for &id in &parsed.ids {
        if id == on {
            eprintln!(
                "REVIEW: id {} equals the --on blocker; self-dependency skipped",
                id
            );
            continue;
        }
        match db::add_dependency(&tx, on, id) {
            Ok(created) => edges.push((id, created)),
            Err(ItrError::NotFound(_)) => {
                eprintln!("REVIEW: id {} not found; skipped", id);
            }
            // Cycles (and everything else) stay hard errors and roll back.
            Err(e) => return Err(e),
        }
    }
    if edges.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid: "at least one existing issue ID distinct from --on".to_string(),
        });
    }
    tx.commit()?;

    match fmt {
        Format::Json => {
            let arr: Vec<serde_json::Value> = edges
                .iter()
                .map(|(id, created)| {
                    serde_json::json!({
                        "action": "depend",
                        "blocked_id": id,
                        "blocker_id": on,
                        "created": created,
                    })
                })
                .collect();
            println!("{}", serde_json::Value::Array(arr));
        }
        _ => {
            for (id, _) in &edges {
                println!("DEPEND: {} blocked by {}", id, on);
            }
        }
    }
    Ok(())
}

pub fn run(conn: &Connection, id: i64, on: i64, fmt: Format) -> Result<(), ItrError> {
    let created = db::add_dependency(conn, on, id)?;

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "depend",
                "blocked_id": id,
                "blocker_id": on,
                "created": created,
            });
            println!("{}", out);
        }
        _ => {
            println!("DEPEND: {} blocked by {}", id, on);
        }
    }

    Ok(())
}

pub fn run_undepend(conn: &Connection, id: i64, on: i64, fmt: Format) -> Result<(), ItrError> {
    // Capture pre-state so UNBLOCKED only fires on a real blocked->unblocked
    // transition caused by this command, never on a no-op (#191).
    let was_blocked = db::is_blocked(conn, id)?;
    let removed = db::remove_dependency(conn, on, id)?;

    let unblocked = if removed && was_blocked && !db::is_blocked(conn, id)? {
        let issue = db::get_issue(conn, id)?;
        if issue.status != "done" && issue.status != "wontfix" {
            vec![(issue.id, issue.title)]
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "undepend",
                "blocked_id": id,
                "blocker_id": on,
                "removed": removed,
            });
            println!("{}", out);
        }
        _ => {
            if removed {
                println!("UNDEPEND: {} no longer blocked by {}", id, on);
            } else {
                println!("UNDEPEND:not_found {} was not blocked by {}", id, on);
            }
        }
    }

    if !unblocked.is_empty() {
        let unblocked_str = crate::format::format_unblocked(&unblocked, fmt);
        if !unblocked_str.is_empty() {
            println!("{}", unblocked_str);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(conn: &Connection, title: &str) -> i64 {
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
    fn run_multi_blocks_every_id_and_records_events() {
        let conn = db::open_test_db();
        let blocker = seed(&conn, "blocker");
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        run_multi(&conn, &[format!("{},{}", a, b)], blocker, Format::Compact)
            .expect("multi depend");
        for id in [a, b] {
            assert_eq!(db::get_blockers(&conn, id).unwrap(), vec![blocker]);
            let events = db::get_events_for_issue(&conn, id).expect("events");
            assert!(
                events.iter().any(|e| e.field == "dependency_added"),
                "dependency mutations must appear in the audit log (#35)"
            );
        }
    }

    #[test]
    fn run_multi_skips_self_and_missing() {
        let conn = db::open_test_db();
        let blocker = seed(&conn, "blocker");
        let a = seed(&conn, "a");
        run_multi(
            &conn,
            &[a.to_string(), blocker.to_string(), "999".to_string()],
            blocker,
            Format::Compact,
        )
        .expect("soft fallback");
        assert_eq!(db::get_blockers(&conn, a).unwrap(), vec![blocker]);
        assert!(
            db::get_blockers(&conn, blocker).unwrap().is_empty(),
            "self-dependency must be skipped"
        );
    }

    #[test]
    fn run_multi_missing_blocker_is_hard_error() {
        let conn = db::open_test_db();
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        let err = run_multi(&conn, &[format!("{},{}", a, b)], 999, Format::Compact).unwrap_err();
        assert!(matches!(err, ItrError::NotFound(999)));
    }

    #[test]
    fn run_multi_cycle_stays_hard_error_and_rolls_back() {
        let conn = db::open_test_db();
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        let c = seed(&conn, "c");
        // b is blocked by a; making a blocked by b would be a cycle.
        db::add_dependency(&conn, a, b).expect("edge");

        let err = run_multi(&conn, &[format!("{},{}", c, a)], b, Format::Compact).unwrap_err();
        assert!(matches!(err, ItrError::CycleDetected(_)));
        assert!(
            db::get_blockers(&conn, c).unwrap().is_empty(),
            "the whole transaction must roll back on a cycle"
        );
    }

    #[test]
    fn run_multi_single_id_keeps_single_contract() {
        let conn = db::open_test_db();
        let err = run_multi(&conn, &["999".to_string()], 1, Format::Compact).unwrap_err();
        assert!(matches!(err, ItrError::NotFound(_)));
    }
}
