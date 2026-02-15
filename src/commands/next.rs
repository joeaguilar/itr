use crate::db;
use crate::error::{self, NitError};
use crate::format::{self, Format};
use crate::models::IssueDetail;
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

pub fn run(conn: &Connection, claim: bool, fmt: Format) -> Result<(), NitError> {
    // Get all open, unblocked issues
    let issues = db::list_issues(
        conn,
        &["open".to_string()],
        &[],
        &[],
        &[],
        false,
        false,
        None,
        false,
    )?;

    if issues.is_empty() {
        error::exit_empty(fmt.is_json(), "No eligible issues found.");
    }

    let config = UrgencyConfig::load(conn);

    // Find highest urgency
    let mut best = None;
    let mut best_urg = f64::NEG_INFINITY;

    for issue in &issues {
        let urg = urgency::compute_urgency(issue, &config, conn);
        if urg > best_urg {
            best_urg = urg;
            best = Some(issue.clone());
        }
    }

    let issue = best.unwrap();

    // Claim if requested
    if claim {
        db::update_issue_field(conn, issue.id, "status", "in-progress")?;
    }

    // Re-read if claimed (status changed)
    let issue = if claim {
        db::get_issue(conn, issue.id)?
    } else {
        issue
    };

    let (urg, breakdown) = urgency::compute_urgency_with_breakdown(&issue, &config, conn);
    let blocked_by = db::get_blockers(conn, issue.id)?;
    let blocks = db::get_blocking(conn, issue.id)?;
    let is_blocked = db::is_blocked(conn, issue.id)?;
    let notes = db::get_notes(conn, issue.id)?;

    let detail = IssueDetail {
        issue,
        urgency: urg,
        blocked_by,
        blocks,
        is_blocked,
        notes,
        urgency_breakdown: Some(breakdown),
        children: None,
    };

    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
}
