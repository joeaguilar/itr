use super::build_issue_summary;
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::IssueSummary;
use crate::urgency::UrgencyConfig;
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    limit: Option<usize>,
    status: Option<String>,
    skills: Vec<String>,
    assigned_to: Option<String>,
    fmt: Format,
) -> Result<(), ItrError> {
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
        &skills,
        assigned_to.as_deref(),
    )?;

    if issues.is_empty() {
        error::print_empty(fmt.is_json(), "No ready issues found.");
        return Ok(());
    }

    let config = UrgencyConfig::load(conn);

    let mut summaries: Vec<IssueSummary> = issues
        .iter()
        .map(|i| build_issue_summary(conn, i, &config))
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
