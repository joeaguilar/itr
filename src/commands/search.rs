use super::sort_by_urgency_desc;
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::SearchResult;
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::collections::HashMap;

#[allow(clippy::too_many_arguments)]
pub fn run(
    conn: &Connection,
    query: &str,
    all: bool,
    statuses: Vec<String>,
    priorities: Vec<String>,
    kinds: Vec<String>,
    skills: Vec<String>,
    assigned_to: Option<String>,
    limit: Option<usize>,
    fmt: Format,
) -> Result<(), ItrError> {
    let terms: Vec<String> = query.split_whitespace().map(|s| s.to_string()).collect();

    if terms.is_empty() {
        error::print_empty(fmt.is_json(), "No search terms provided.");
        return Ok(());
    }

    // Try FTS5 first, fall back to LIKE-based search
    let ids = if db::has_fts(conn) {
        let fts_ids = db::fts_search(conn, query)?;
        if fts_ids.is_empty() {
            // FTS returned nothing — fall back to LIKE in case FTS index is stale
            db::search_issue_ids(conn, &terms, &statuses, &priorities, &kinds, all)?
        } else {
            // Post-filter FTS results by status/priority/kind
            let mut filtered = fts_ids;
            if !all {
                let valid_statuses: Vec<String> = if statuses.is_empty() {
                    vec!["open".to_string(), "in-progress".to_string()]
                } else {
                    statuses.clone()
                };
                filtered.retain(|id| {
                    db::get_issue(conn, *id)
                        .map(|i| valid_statuses.contains(&i.status))
                        .unwrap_or(false)
                });
            }
            if !priorities.is_empty() {
                filtered.retain(|id| {
                    db::get_issue(conn, *id)
                        .map(|i| priorities.contains(&i.priority))
                        .unwrap_or(false)
                });
            }
            if !kinds.is_empty() {
                filtered.retain(|id| {
                    db::get_issue(conn, *id)
                        .map(|i| kinds.contains(&i.kind))
                        .unwrap_or(false)
                });
            }
            filtered
        }
    } else {
        db::search_issue_ids(conn, &terms, &statuses, &priorities, &kinds, all)?
    };

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
        let (matched_fields, context_snippets) =
            compute_matched_fields_with_snippets(&terms, &issue, &notes);

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
            skills: issue.skills,
            acceptance: issue.acceptance,
            assigned_to: issue.assigned_to,
            matched_fields,
            context_snippets: if context_snippets.is_empty() {
                None
            } else {
                Some(context_snippets)
            },
        });
    }

    // Filter by skills (AND logic)
    let mut results = if !skills.is_empty() {
        results
            .into_iter()
            .filter(|r| skills.iter().all(|s| r.skills.contains(s)))
            .collect()
    } else {
        results
    };

    // Filter by assigned_to
    if let Some(ref agent) = assigned_to {
        results.retain(|r| r.assigned_to == *agent);
    }

    // Sort by urgency descending
    sort_by_urgency_desc(&mut results);

    if let Some(n) = limit {
        results.truncate(n);
    }

    println!("{}", format::format_search_results(&results, fmt));
    Ok(())
}

pub fn compute_matched_fields_with_snippets(
    terms: &[String],
    issue: &crate::models::Issue,
    notes: &[crate::models::Note],
) -> (Vec<String>, HashMap<String, String>) {
    let mut fields = Vec::new();
    let mut snippets = HashMap::new();

    let check = |text: &str| -> bool {
        let lower = text.to_lowercase();
        terms.iter().any(|t| lower.contains(&t.to_lowercase()))
    };

    let first_matching_term = |text: &str| -> Option<String> {
        let lower = text.to_lowercase();
        terms
            .iter()
            .find(|t| lower.contains(&t.to_lowercase()))
            .cloned()
    };

    if check(&issue.title) {
        fields.push("title".to_string());
        if let Some(ref term) = first_matching_term(&issue.title) {
            if let Some(snippet) = extract_snippet(&issue.title, term, 40) {
                snippets.insert("title".to_string(), snippet);
            }
        }
    }
    if check(&issue.context) {
        fields.push("context".to_string());
        if let Some(ref term) = first_matching_term(&issue.context) {
            if let Some(snippet) = extract_snippet(&issue.context, term, 40) {
                snippets.insert("context".to_string(), snippet);
            }
        }
    }
    if check(&issue.acceptance) {
        fields.push("acceptance".to_string());
        if let Some(ref term) = first_matching_term(&issue.acceptance) {
            if let Some(snippet) = extract_snippet(&issue.acceptance, term, 40) {
                snippets.insert("acceptance".to_string(), snippet);
            }
        }
    }
    if check(&issue.close_reason) {
        fields.push("close_reason".to_string());
        if let Some(ref term) = first_matching_term(&issue.close_reason) {
            if let Some(snippet) = extract_snippet(&issue.close_reason, term, 40) {
                snippets.insert("close_reason".to_string(), snippet);
            }
        }
    }
    // Check tags — return matched element directly
    for tag in &issue.tags {
        if check(tag) {
            if !fields.contains(&"tags".to_string()) {
                fields.push("tags".to_string());
            }
            snippets.insert("tags".to_string(), tag.clone());
            break;
        }
    }
    // Check files — return matched element directly
    for file in &issue.files {
        if check(file) {
            if !fields.contains(&"files".to_string()) {
                fields.push("files".to_string());
            }
            snippets.insert("files".to_string(), file.clone());
            break;
        }
    }
    // Check skills — return matched element directly
    for skill in &issue.skills {
        if check(skill) {
            if !fields.contains(&"skills".to_string()) {
                fields.push("skills".to_string());
            }
            snippets.insert("skills".to_string(), skill.clone());
            break;
        }
    }
    // Check notes — return first matched note content truncated
    for note in notes {
        if check(&note.content) {
            fields.push("notes".to_string());
            if let Some(ref term) = first_matching_term(&note.content) {
                if let Some(snippet) = extract_snippet(&note.content, term, 40) {
                    snippets.insert("notes".to_string(), snippet);
                }
            }
            break;
        }
    }

    (fields, snippets)
}

/// Extract a snippet around the first occurrence of `term` in `text`.
/// Returns `...prefix **match** suffix...` with `context_chars` of context on each side.
/// UTF-8 safe — always slices on char boundaries.
fn extract_snippet(text: &str, term: &str, context_chars: usize) -> Option<String> {
    let lower = text.to_lowercase();
    let term_lower = term.to_lowercase();
    let match_start = lower.find(&term_lower)?;
    let match_end = match_start + term.len();

    // Find safe UTF-8 boundary for start
    let snippet_start = if match_start <= context_chars {
        0
    } else {
        let mut pos = match_start - context_chars;
        while pos > 0 && !text.is_char_boundary(pos) {
            pos -= 1;
        }
        pos
    };

    // Find safe UTF-8 boundary for end
    let snippet_end = if match_end + context_chars >= text.len() {
        text.len()
    } else {
        let mut pos = match_end + context_chars;
        while pos < text.len() && !text.is_char_boundary(pos) {
            pos += 1;
        }
        pos
    };

    let prefix = if snippet_start > 0 { "..." } else { "" };
    let suffix = if snippet_end < text.len() { "..." } else { "" };

    let before = &text[snippet_start..match_start];
    let matched = &text[match_start..match_end];
    let after = &text[match_end..snippet_end];

    Some(format!(
        "{}{}**{}**{}{}",
        prefix, before, matched, after, suffix
    ))
}
