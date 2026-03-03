use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::IssueSummary;
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

#[allow(clippy::too_many_arguments)]
pub fn run(
    conn: &Connection,
    all: bool,
    statuses: Vec<String>,
    priorities: Vec<String>,
    kinds: Vec<String>,
    tags: Vec<String>,
    skills: Vec<String>,
    blocked_only: bool,
    include_blocked: bool,
    parent: Option<i64>,
    assigned_to: Option<String>,
    sort: &str,
    limit: Option<usize>,
    fmt: Format,
) -> Result<(), ItrError> {
    let issues = db::list_issues(
        conn,
        &statuses,
        &priorities,
        &kinds,
        &tags,
        blocked_only,
        include_blocked,
        parent,
        all,
        &skills,
        assigned_to.as_deref(),
    )?;

    if issues.is_empty() {
        error::print_empty(fmt.is_json(), "No matching issues found.");
        return Ok(());
    }

    let config = UrgencyConfig::load(conn);

    let mut summaries: Vec<IssueSummary> = issues
        .iter()
        .map(|i| {
            let urg = urgency::compute_urgency(i, &config, conn);
            let blocked_by = db::get_blockers(conn, i.id).unwrap_or_default();
            let is_blocked = db::is_blocked(conn, i.id).unwrap_or(false);
            IssueSummary {
                id: i.id,
                title: i.title.clone(),
                status: i.status.clone(),
                priority: i.priority.clone(),
                kind: i.kind.clone(),
                urgency: urg,
                is_blocked,
                blocked_by,
                tags: i.tags.clone(),
                files: i.files.clone(),
                skills: i.skills.clone(),
                acceptance: i.acceptance.clone(),
                assigned_to: i.assigned_to.clone(),
            }
        })
        .collect();

    // Sort
    match sort {
        "urgency" => summaries.sort_by(|a, b| {
            b.urgency
                .partial_cmp(&a.urgency)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "priority" => {
            summaries.sort_by(|a, b| priority_ord(&a.priority).cmp(&priority_ord(&b.priority)))
        }
        "created" => {} // already ordered by insertion
        "updated" => {} // would need updated_at on summary
        "id" => summaries.sort_by_key(|s| s.id),
        _ => summaries.sort_by(|a, b| {
            b.urgency
                .partial_cmp(&a.urgency)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
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
