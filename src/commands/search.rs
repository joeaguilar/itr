use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::SearchResult;
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    query: &str,
    all: bool,
    statuses: Vec<String>,
    priorities: Vec<String>,
    kinds: Vec<String>,
    limit: Option<usize>,
    fmt: Format,
) -> Result<(), ItrError> {
    let terms: Vec<String> = query
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    if terms.is_empty() {
        error::print_empty(fmt.is_json(), "No search terms provided.");
        return Ok(());
    }

    let ids = db::search_issue_ids(conn, &terms, &statuses, &priorities, &kinds, all)?;

    if ids.is_empty() {
        error::print_empty(fmt.is_json(), "No matching issues found.");
        return Ok(());
    }

    let config = UrgencyConfig::load(conn);

    let mut results: Vec<SearchResult> = Vec::new();
    for id in &ids {
        let issue = db::get_issue(conn, *id)?;
        let notes = db::get_notes(conn, *id)?;
        let urg = urgency::compute_urgency(&issue, &config, conn);
        let blocked_by = db::get_blockers(conn, *id).unwrap_or_default();
        let is_blocked = db::is_blocked(conn, *id).unwrap_or(false);
        let matched_fields = compute_matched_fields(&terms, &issue, &notes);

        results.push(SearchResult {
            id: issue.id,
            title: issue.title,
            status: issue.status,
            priority: issue.priority,
            kind: issue.kind,
            urgency: urg,
            is_blocked,
            blocked_by,
            tags: issue.tags,
            files: issue.files,
            acceptance: issue.acceptance,
            matched_fields,
        });
    }

    // Sort by urgency descending
    results.sort_by(|a, b| {
        b.urgency
            .partial_cmp(&a.urgency)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    if let Some(n) = limit {
        results.truncate(n);
    }

    println!("{}", format::format_search_results(&results, fmt));
    Ok(())
}

fn compute_matched_fields(
    terms: &[String],
    issue: &crate::models::Issue,
    notes: &[crate::models::Note],
) -> Vec<String> {
    let mut fields = Vec::new();

    let check = |text: &str| -> bool {
        let lower = text.to_lowercase();
        terms.iter().any(|t| lower.contains(&t.to_lowercase()))
    };

    if check(&issue.title) {
        fields.push("title".to_string());
    }
    if check(&issue.context) {
        fields.push("context".to_string());
    }
    if check(&issue.acceptance) {
        fields.push("acceptance".to_string());
    }
    if check(&issue.close_reason) {
        fields.push("close_reason".to_string());
    }
    // Check tags as joined text
    let tags_text = issue.tags.join(" ");
    if check(&tags_text) {
        fields.push("tags".to_string());
    }
    // Check files as joined text
    let files_text = issue.files.join(" ");
    if check(&files_text) {
        fields.push("files".to_string());
    }
    // Check notes
    if notes.iter().any(|n| check(&n.content)) {
        fields.push("notes".to_string());
    }

    fields
}
