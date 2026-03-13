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
pub mod summary;
pub mod update;
pub mod upgrade;

use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{Issue, IssueDetail, IssueSummary};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::cmp::Ordering;

/// Build an `IssueSummary` for a single issue: compute urgency, resolve blockers.
pub fn build_issue_summary(
    conn: &Connection,
    issue: &Issue,
    config: &UrgencyConfig,
) -> IssueSummary {
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
        created_at: issue.created_at.clone(),
        updated_at: issue.updated_at.clone(),
    }
}

/// Sort by urgency descending (highest first).
pub fn sort_by_urgency_desc<T: HasUrgency>(items: &mut [T]) {
    items.sort_by(|a, b| {
        b.urgency_val()
            .partial_cmp(&a.urgency_val())
            .unwrap_or(Ordering::Equal)
    });
}

/// Trait for types that have an urgency score.
pub trait HasUrgency {
    fn urgency_val(&self) -> f64;
}

impl HasUrgency for IssueSummary {
    fn urgency_val(&self) -> f64 {
        self.urgency
    }
}

impl HasUrgency for crate::models::SearchResult {
    fn urgency_val(&self) -> f64 {
        self.urgency
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

/// Print an `IssueDetail` along with any newly-unblocked issues.
/// Used by close.rs and update.rs after modifying an issue.
pub fn print_detail_with_unblocked(detail: &IssueDetail, unblocked: &[(i64, String)], fmt: Format) {
    match fmt {
        Format::Json => {
            let mut value = serde_json::to_value(detail).unwrap_or_default();
            if !unblocked.is_empty() {
                let list: Vec<serde_json::Value> = unblocked
                    .iter()
                    .map(|(uid, utitle)| serde_json::json!({"id": uid, "title": utitle}))
                    .collect();
                value["unblocked"] = serde_json::Value::Array(list);
            }
            format::println_json(&value.to_string());
        }
        _ => {
            println!("{}", format::format_issue_detail(detail, fmt));
            if !unblocked.is_empty() {
                let unblocked_str = format::format_unblocked(unblocked, fmt);
                if !unblocked_str.is_empty() {
                    println!("{}", unblocked_str);
                }
            }
        }
    }
}
