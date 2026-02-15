use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{IssueDetail, IssueSummary};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

pub fn run(conn: &Connection, id: i64, fmt: Format) -> Result<(), ItrError> {
    let issue = db::get_issue(conn, id)?;
    let config = UrgencyConfig::load(conn);
    let (urg, breakdown) = urgency::compute_urgency_with_breakdown(&issue, &config, conn);
    let blocked_by = db::get_blockers(conn, issue.id)?;
    let blocks = db::get_blocking(conn, issue.id)?;
    let is_blocked = db::is_blocked(conn, issue.id)?;
    let notes = db::get_notes(conn, issue.id)?;

    // If epic, get children
    let children = if issue.kind == "epic" {
        let child_issues = db::list_issues(conn, &[], &[], &[], &[], false, true, Some(issue.id), true)?;
        let child_summaries: Vec<IssueSummary> = child_issues
            .iter()
            .map(|i| {
                let u = urgency::compute_urgency(i, &config, conn);
                let bb = db::get_blockers(conn, i.id).unwrap_or_default();
                let ib = db::is_blocked(conn, i.id).unwrap_or(false);
                IssueSummary {
                    id: i.id,
                    title: i.title.clone(),
                    status: i.status.clone(),
                    priority: i.priority.clone(),
                    kind: i.kind.clone(),
                    urgency: u,
                    is_blocked: ib,
                    blocked_by: bb,
                    tags: i.tags.clone(),
                    files: i.files.clone(),
                    acceptance: i.acceptance.clone(),
                }
            })
            .collect();
        if child_summaries.is_empty() {
            None
        } else {
            Some(child_summaries)
        }
    } else {
        None
    };

    let detail = IssueDetail {
        issue,
        urgency: urg,
        blocked_by,
        blocks,
        is_blocked,
        notes,
        urgency_breakdown: Some(breakdown),
        children,
    };

    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
}
