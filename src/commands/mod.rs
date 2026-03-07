pub mod add;
pub mod agent_info;
pub mod assign;
pub mod batch;
pub mod bulk;
pub mod close;
pub mod config;
pub mod depend;
pub mod doctor;
pub mod export;
pub mod get;
pub mod graph;
pub mod import;
pub mod init;
pub mod list;
pub mod log;
pub mod next;
pub mod note;
pub mod ready;
pub mod reindex;
pub mod relate;
pub mod schema;
pub mod search;
pub mod stats;
pub mod update;
pub mod upgrade;

use crate::db;
use crate::error::ItrError;
use crate::models::{IssueSummary, IssueDetail, Issue};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

/// Build an `IssueSummary` for a single issue: compute urgency, resolve blockers.
pub fn build_issue_summary(conn: &Connection, issue: &Issue, config: &UrgencyConfig) -> IssueSummary {
    let urg = urgency::compute_urgency(issue, config, conn);
    let blocked_by = db::get_blockers(conn, issue.id).unwrap_or_default();
    let is_blocked = db::is_blocked(conn, issue.id).unwrap_or(false);
    IssueSummary {
        id: issue.id,
        title: issue.title.clone(),
        status: issue.status.clone(),
        priority: issue.priority.clone(),
        kind: issue.kind.clone(),
        urgency: urg,
        is_blocked,
        blocked_by,
        tags: issue.tags.clone(),
        files: issue.files.clone(),
        skills: issue.skills.clone(),
        acceptance: issue.acceptance.clone(),
        assigned_to: issue.assigned_to.clone(),
    }
}

/// Build an `IssueDetail` for a single issue using standard DB lookups.
/// `children` and `relations` default to empty — callers that need them set
/// the fields on the returned struct afterward, or use the `get` handler directly.
pub fn build_issue_detail(
    conn: &Connection,
    issue: Issue,
    config: &UrgencyConfig,
) -> Result<IssueDetail, ItrError> {
    let (urgency, urgency_breakdown) =
        urgency::compute_urgency_with_breakdown(&issue, config, conn);
    let blocked_by = db::get_blockers(conn, issue.id)?;
    let blocks = db::get_blocking(conn, issue.id)?;
    let is_blocked = db::is_blocked(conn, issue.id)?;
    let notes = db::get_notes(conn, issue.id)?;
    Ok(IssueDetail {
        issue,
        urgency,
        blocked_by,
        blocks,
        is_blocked,
        notes,
        urgency_breakdown: Some(urgency_breakdown),
        children: None,
        relations: vec![],
    })
}
