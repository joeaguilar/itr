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
    filter: &ListFilter,
    sort: &str,
    limit: Option<usize>,
    fmt: Format,
) -> Result<(), ItrError> {
    let mut summaries = collect_summaries(conn, filter)?;

    if summaries.is_empty() {
        error::print_empty(fmt.is_json(), "No matching issues found.");
        return Ok(());
    }

    sort_summaries(&mut summaries, sort);

    // Limit
    if let Some(n) = limit {
        summaries.truncate(n);
    }

    println!("{}", format::format_issue_list(&summaries, fmt));
    Ok(())
}

/// Fetch and summarize the issues matching `filter`.
///
/// Status/priority/kind filter values are normalized with the same synonym
/// tables as the write paths (`wip` → `in-progress`, `closed` → `done`, ...),
/// and values still unrecognized after normalization emit a REVIEW note
/// instead of silently matching nothing (#168).
fn collect_summaries(
    conn: &Connection,
    filter: &ListFilter,
) -> Result<Vec<IssueSummary>, ItrError> {
    let (statuses, status_notes) = normalize::normalize_status_filters(&filter.statuses);
    let (priorities, priority_notes) = normalize::normalize_priority_filters(&filter.priorities);
    let (kinds, kind_notes) = normalize::normalize_kind_filters(&filter.kinds);
    for note in status_notes
        .iter()
        .chain(&priority_notes)
        .chain(&kind_notes)
    {
        eprintln!("{}", note);
    }

    let filter = ListFilter {
        statuses,
        priorities,
        kinds,
        ..filter.clone()
    };

    let issues = db::list_issues(conn, &filter)?;
    let config = UrgencyConfig::load(conn);

    // Consume issues by value so build_issue_summary_owned can move each
    // Issue's string/vec fields directly into the resulting IssueSummary
    // rather than cloning them.
    Ok(issues
        .into_iter()
        .map(|i| build_issue_summary_owned(conn, i, &config))
        .collect())
}

/// Sort summaries in place by the requested key.
///
/// `created` orders oldest-first (insertion order) and `updated` orders
/// most-recently-updated first; both use the issue ID as a stable tiebreaker
/// since timestamps are ISO 8601 strings with second resolution (#171).
/// Unrecognized keys fall back to urgency with a REVIEW note.
fn sort_summaries(summaries: &mut [IssueSummary], sort: &str) {
    match sort {
        "urgency" => sort_by_urgency_desc(summaries),
        "priority" => {
            summaries.sort_by(|a, b| priority_ord(&a.priority).cmp(&priority_ord(&b.priority)));
        }
        "created" => {
            summaries.sort_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
        }
        "updated" => {
            summaries.sort_by(|a, b| {
                b.updated_at
                    .cmp(&a.updated_at)
                    .then_with(|| b.id.cmp(&a.id))
            });
        }
        "id" => summaries.sort_by_key(|s| s.id),
        other => {
            eprintln!(
                "REVIEW: sort '{}' not recognized, defaulted to 'urgency'. Valid: urgency, priority, created, updated, id",
                other
            );
            sort_by_urgency_desc(summaries);
        }
    }
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

    fn summary(id: i64, created_at: &str, updated_at: &str) -> IssueSummary {
        IssueSummary {
            id,
            title: format!("issue {}", id),
            status: "open".to_string(),
            priority: "medium".to_string(),
            kind: "task".to_string(),
            urgency: 0.0,
            is_blocked: false,
            blocked_by: vec![],
            tags: vec![],
            files: vec![],
            skills: vec![],
            acceptance: String::new(),
            assigned_to: String::new(),
            created_at: created_at.to_string(),
            updated_at: updated_at.to_string(),
        }
    }

    fn ids(summaries: &[IssueSummary]) -> Vec<i64> {
        summaries.iter().map(|s| s.id).collect()
    }

    // --- #168: list filters accept the same synonyms as write paths ---

    #[test]
    fn status_filter_synonyms_match_canonical_values() {
        let conn = db::open_test_db();
        let id = insert_issue(&conn, "in flight");
        db::update_issue_field(&conn, id, "status", "in-progress").expect("set status");
        let done_id = insert_issue(&conn, "finished");
        db::update_issue_field(&conn, done_id, "status", "done").expect("set status");

        let wip = collect_summaries(
            &conn,
            &ListFilter {
                statuses: vec!["wip".to_string()],
                include_blocked: true,
                ..ListFilter::default()
            },
        )
        .expect("list wip");
        assert_eq!(ids(&wip), vec![id], "-s wip must match in-progress issues");

        let closed = collect_summaries(
            &conn,
            &ListFilter {
                statuses: vec!["closed".to_string()],
                include_blocked: true,
                ..ListFilter::default()
            },
        )
        .expect("list closed");
        assert_eq!(
            ids(&closed),
            vec![done_id],
            "-s closed must match done issues"
        );
    }

    #[test]
    fn priority_and_kind_filter_synonyms_match() {
        let conn = db::open_test_db();
        let id = db::insert_issue(
            &conn,
            "urgent feature",
            "critical",
            "feature",
            "",
            &[],
            &[],
            &[],
            "",
            None,
            "",
        )
        .expect("insert")
        .id;
        insert_issue(&conn, "plain task");

        let found = collect_summaries(
            &conn,
            &ListFilter {
                priorities: vec!["urgent".to_string()],
                kinds: vec!["enhancement".to_string()],
                include_blocked: true,
                ..ListFilter::default()
            },
        )
        .expect("list");
        assert_eq!(ids(&found), vec![id]);
    }

    // --- #171: created/updated sorts are implemented, not no-ops ---

    #[test]
    fn sort_created_orders_oldest_first_with_id_tiebreak() {
        let mut summaries = vec![
            summary(3, "2026-01-02T00:00:00Z", "2026-01-02T00:00:00Z"),
            summary(2, "2026-01-01T00:00:00Z", "2026-01-05T00:00:00Z"),
            summary(1, "2026-01-02T00:00:00Z", "2026-01-03T00:00:00Z"),
        ];
        sort_summaries(&mut summaries, "created");
        assert_eq!(ids(&summaries), vec![2, 1, 3]);
    }

    #[test]
    fn sort_updated_orders_most_recent_first_with_id_tiebreak() {
        let mut summaries = vec![
            summary(1, "2026-01-01T00:00:00Z", "2026-01-03T00:00:00Z"),
            summary(2, "2026-01-01T00:00:00Z", "2026-01-05T00:00:00Z"),
            summary(3, "2026-01-01T00:00:00Z", "2026-01-03T00:00:00Z"),
        ];
        sort_summaries(&mut summaries, "updated");
        assert_eq!(ids(&summaries), vec![2, 3, 1]);
    }

    #[test]
    fn unknown_sort_falls_back_to_urgency() {
        let mut a = summary(1, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z");
        a.urgency = 1.0;
        let mut b = summary(2, "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z");
        b.urgency = 5.0;
        let mut summaries = vec![a, b];
        sort_summaries(&mut summaries, "bogus");
        assert_eq!(
            ids(&summaries),
            vec![2, 1],
            "must fall back to urgency desc"
        );
    }
}
