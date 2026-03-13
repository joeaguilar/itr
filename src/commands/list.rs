use super::{build_issue_summary, sort_by_urgency_desc};
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::{IssueSummary, ListFilter};
use crate::urgency::UrgencyConfig;
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    filter: &ListFilter,
    sort: &str,
    limit: Option<usize>,
    fmt: Format,
) -> Result<(), ItrError> {
    let issues = db::list_issues(conn, filter)?;

    if issues.is_empty() {
        error::print_empty(fmt.is_json(), "No matching issues found.");
        return Ok(());
    }

    let config = UrgencyConfig::load(conn);

    let mut summaries: Vec<IssueSummary> = issues
        .iter()
        .map(|i| build_issue_summary(conn, i, &config))
        .collect();

    // Sort
    match sort {
        "urgency" => sort_by_urgency_desc(&mut summaries),
        "priority" => {
            summaries.sort_by(|a, b| priority_ord(&a.priority).cmp(&priority_ord(&b.priority)));
        }
        "created" => {} // already ordered by insertion
        "updated" => {} // would need updated_at on summary
        "id" => summaries.sort_by_key(|s| s.id),
        _ => sort_by_urgency_desc(&mut summaries),
    }

    // Limit
    if let Some(n) = limit {
        summaries.truncate(n);
    }

    println!("{}", format::format_issue_list(&summaries, fmt));
    Ok(())
}

fn priority_ord(p: &str) -> u8 {
    match p {
        "critical" => 0,
        "high" => 1,
        "medium" => 2,
        "low" => 3,
        _ => 4,
    }
}
