use super::{build_issue_detail, print_detail_with_unblocked};
use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::IssueDetail;
use crate::urgency::UrgencyConfig;
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    id: i64,
    reason: Option<String>,
    wontfix: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let (detail, unblocked) = close_issue(conn, id, reason, wontfix)?;
    print_detail_with_unblocked(&detail, &unblocked, fmt);
    Ok(())
}

/// Apply all close writes (status event, status flip, optional `close_reason`
/// event + field, dependency-edge cleanup) inside a single transaction so a
/// mid-close failure leaves the issue fully unchanged, and build the output
/// detail from the updated state before committing.
fn close_issue(
    conn: &Connection,
    id: i64,
    reason: Option<String>,
    wontfix: bool,
) -> Result<(IssueDetail, Vec<(i64, String)>), ItrError> {
    let reason = reason.unwrap_or_default();

    let status = if wontfix { "wontfix" } else { "done" };

    let tx = conn.unchecked_transaction()?;

    // Capture old values for event recording
    let old_issue = db::get_issue(&tx, id)?;

    db::record_event(&tx, id, "status", &old_issue.status, status)?;
    db::update_issue_field(&tx, id, "status", status)?;
    if !reason.is_empty() {
        db::record_event(&tx, id, "close_reason", &old_issue.close_reason, &reason)?;
        db::update_issue_field(&tx, id, "close_reason", &reason)?;
    }

    // Auto-clean dependency edges where this issue was the blocker
    let unblocked = db::get_newly_unblocked(&tx, id)?;
    db::remove_blocker_edges(&tx, id)?;

    // Build the output detail from the updated state
    let issue = db::get_issue(&tx, id)?;
    let config = UrgencyConfig::load(&tx);
    let detail = build_issue_detail(&tx, issue, &config)?;

    tx.commit()?;
    Ok((detail, unblocked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_conn() -> Connection {
        db::init_db(Path::new(":memory:")).expect("init in-memory db")
    }

    fn insert_issue(conn: &Connection, title: &str) -> i64 {
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
    fn close_applies_all_writes_and_reports_unblocked() {
        let conn = test_conn();
        let blocker = insert_issue(&conn, "blocker");
        let blocked = insert_issue(&conn, "blocked");
        db::add_dependency(&conn, blocker, blocked).expect("add dependency");

        let (detail, unblocked) =
            close_issue(&conn, blocker, Some("all done".to_string()), false).expect("close");

        assert_eq!(detail.issue.status, "done");
        assert_eq!(detail.issue.close_reason, "all done");
        assert_eq!(unblocked, vec![(blocked, "blocked".to_string())]);

        let issue = db::get_issue(&conn, blocker).expect("get issue");
        assert_eq!(issue.status, "done");
        assert_eq!(issue.close_reason, "all done");
        let events = db::get_events_for_issue(&conn, blocker).expect("events");
        let fields: Vec<&str> = events.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["status", "close_reason"]);
        assert!(
            db::get_blockers(&conn, blocked)
                .expect("blockers")
                .is_empty(),
            "blocker edge must be cleaned up on close"
        );
    }

    #[test]
    fn mid_close_failure_leaves_issue_fully_unchanged() {
        let conn = test_conn();
        let blocker = insert_issue(&conn, "blocker");
        let blocked = insert_issue(&conn, "blocked");
        db::add_dependency(&conn, blocker, blocked).expect("add dependency");

        // Inject a failure at the LAST write step (dependency cleanup), so
        // every earlier write (status event, status flip, close_reason event,
        // close_reason field) has already been issued when the close fails.
        conn.execute_batch(
            "CREATE TRIGGER fail_dep_cleanup BEFORE DELETE ON dependencies
             BEGIN SELECT RAISE(ABORT, 'injected mid-close failure'); END;",
        )
        .expect("create failure trigger");

        let result = close_issue(&conn, blocker, Some("all done".to_string()), false);
        assert!(result.is_err(), "injected failure must propagate");

        // All-or-nothing: the issue must be exactly as before the close.
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
}
