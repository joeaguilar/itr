use super::{build_issue_detail, print_detail_with_unblocked};
use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::urgency::UrgencyConfig;
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    id: i64,
    reason: Option<String>,
    wontfix: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let reason = reason.unwrap_or_default();

    let status = if wontfix { "wontfix" } else { "done" };

    // Capture old values for event recording
    let old_issue = db::get_issue(conn, id)?;

    db::record_event(conn, id, "status", &old_issue.status, status)?;
    db::update_issue_field(conn, id, "status", status)?;
    if !reason.is_empty() {
        db::record_event(conn, id, "close_reason", &old_issue.close_reason, &reason)?;
        db::update_issue_field(conn, id, "close_reason", &reason)?;
    }

    // Auto-clean dependency edges where this issue was the blocker
    let unblocked = db::get_newly_unblocked(conn, id)?;
    db::remove_blocker_edges(conn, id)?;

    // Output updated issue
    let issue = db::get_issue(conn, id)?;
    let config = UrgencyConfig::load(conn);
    let detail = build_issue_detail(conn, issue, &config)?;
    print_detail_with_unblocked(&detail, &unblocked, fmt);

    Ok(())
}
