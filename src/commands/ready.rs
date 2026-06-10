use super::{build_issue_summary_owned, sort_by_urgency_desc};
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::{IssueSummary, ListFilter};
use crate::normalize;
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
    let mut summaries = ready_summaries(conn, status, skills, assigned_to)?;

    if summaries.is_empty() {
        error::print_empty(fmt.is_json(), "No ready issues found.");
        return Ok(());
    }

    if let Some(n) = limit {
        summaries.truncate(n);
    }

    println!("{}", format::format_issue_list(&summaries, fmt));
    Ok(())
}

/// Collect ready (unblocked, non-terminal) issues sorted by urgency.
///
/// An explicit status filter is normalized with the same synonym tables as
/// the write paths (`wip` → `in-progress`, ...); values still unrecognized
/// after normalization emit a REVIEW note instead of silently matching
/// nothing (#168).
fn ready_summaries(
    conn: &Connection,
    status: Option<String>,
    skills: Vec<String>,
    assigned_to: Option<String>,
) -> Result<Vec<IssueSummary>, ItrError> {
    let statuses = match status {
        Some(s) => {
            let (normalized, notes) = normalize::normalize_status_filters(&[s]);
            for note in &notes {
                eprintln!("{}", note);
            }
            normalized
        }
        None => vec!["open".to_string(), "in-progress".to_string()],
    };

    // Ready issues are always unblocked and non-terminal, even when an
    // explicit status filter asks for a terminal status.
    let issues: Vec<_> = db::list_issues(
        conn,
        &ListFilter {
            statuses,
            skills,
            assigned_to,
            ..ListFilter::default()
        },
    )?
    .into_iter()
    .filter(|i| i.status == "open" || i.status == "in-progress")
    .collect();

    let config = UrgencyConfig::load(conn);

    // Consume issues by value so build_issue_summary_owned can move each
    // Issue's string/vec fields directly into the resulting IssueSummary
    // rather than cloning them.
    let mut summaries: Vec<IssueSummary> = issues
        .into_iter()
        .map(|i| build_issue_summary_owned(conn, i, &config))
        .collect();

    // Sort by urgency descending
    sort_by_urgency_desc(&mut summaries);

    Ok(summaries)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // --- #168: ready -s accepts the same status synonyms as write paths ---

    #[test]
    fn ready_status_filter_normalizes_synonyms() {
        let conn = db::open_test_db();
        insert_issue(&conn, "still open");
        let wip_id = insert_issue(&conn, "in flight");
        db::update_issue_field(&conn, wip_id, "status", "in-progress").expect("set status");

        let summaries = ready_summaries(&conn, Some("wip".to_string()), vec![], None)
            .expect("ready with wip filter");
        let ids: Vec<i64> = summaries.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![wip_id], "-s wip must match in-progress issues");
    }
}
