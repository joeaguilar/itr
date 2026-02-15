use crate::db;
use crate::error::{self, NitError};
use crate::format::{self, Format};
use crate::models::IssueSummary;
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    limit: Option<usize>,
    status: Option<String>,
    fmt: Format,
) -> Result<(), NitError> {
    let statuses = match status {
        Some(s) => vec![s],
        None => vec!["open".to_string(), "in-progress".to_string()],
    };

    // Get unblocked, non-terminal issues
    let issues = db::list_issues(
        conn,
        &statuses,
        &[],
        &[],
        &[],
        false,
        false, // exclude blocked
        None,
        false,
    )?;

    if issues.is_empty() {
        error::exit_empty(fmt.is_json(), "No ready issues found.");
    }

    let config = UrgencyConfig::load(conn);

    let mut summaries: Vec<IssueSummary> = issues
        .iter()
        .map(|i| {
            let urg = urgency::compute_urgency(i, &config, conn);
            let blocked_by = db::get_blockers(conn, i.id).unwrap_or_default();
            IssueSummary {
                id: i.id,
                title: i.title.clone(),
                status: i.status.clone(),
                priority: i.priority.clone(),
                kind: i.kind.clone(),
                urgency: urg,
                is_blocked: false, // by definition, ready issues are not blocked
                blocked_by,
                tags: i.tags.clone(),
                files: i.files.clone(),
                acceptance: i.acceptance.clone(),
            }
        })
        .collect();

    // Sort by urgency descending
    summaries.sort_by(|a, b| {
        b.urgency
            .partial_cmp(&a.urgency)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if let Some(n) = limit {
        summaries.truncate(n);
    }

    println!("{}", format::format_issue_list(&summaries, fmt));
    Ok(())
}
