use super::sort_by_urgency_desc;
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::SearchResult;
use crate::normalize;
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::collections::{HashMap, HashSet};

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
    let terms: Vec<String> = query
        .split_whitespace()
        .map(std::string::ToString::to_string)
        .collect();

    if terms.is_empty() {
        error::print_empty(fmt.is_json(), "No search terms provided.");
        return Ok(());
    }

    let results = run_core(
        conn,
        query,
        &terms,
        all,
        &statuses,
        &priorities,
        &kinds,
        &skills,
        assigned_to,
        limit,
    )?;

    if results.is_empty() {
        error::print_empty(fmt.is_json(), "No matching issues found.");
        return Ok(());
    }

    println!("{}", format::format_search_results(&results, fmt));
    Ok(())
}

/// Core search: resolve matching issue IDs, build results, and apply
/// post-filters, sorting, and the limit.
///
/// Status/priority/kind filter values are normalized with the same synonym
/// tables as the write paths (`wip` → `in-progress`, `closed` → `done`, ...);
/// values still unrecognized after normalization emit a REVIEW note instead
/// of silently matching nothing (#168).
#[allow(clippy::too_many_arguments)]
fn run_core(
    conn: &Connection,
    query: &str,
    terms: &[String],
    all: bool,
    statuses: &[String],
    priorities: &[String],
    kinds: &[String],
    skills: &[String],
    assigned_to: Option<String>,
    limit: Option<usize>,
) -> Result<Vec<SearchResult>, ItrError> {
    let (statuses, status_notes) = normalize::normalize_status_filters(statuses);
    let (priorities, priority_notes) = normalize::normalize_priority_filters(priorities);
    let (kinds, kind_notes) = normalize::normalize_kind_filters(kinds);
    for note in status_notes
        .iter()
        .chain(&priority_notes)
        .chain(&kind_notes)
    {
        eprintln!("{}", note);
    }

    // Try FTS5 first for ranked field matches, then append LIKE-only matches
    // such as notes, which are intentionally not indexed in the FTS table.
    let ids = if db::has_fts(conn) {
        let fts_ids = db::fts_search(conn, query)?;
        if fts_ids.is_empty() {
            // FTS returned nothing — fall back to LIKE in case FTS index is stale
            db::search_issue_ids(conn, terms, &statuses, &priorities, &kinds, all)?
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
            let mut seen: HashSet<i64> = filtered.iter().copied().collect();
            let note_ids =
                db::search_note_issue_ids(conn, terms, &statuses, &priorities, &kinds, all)?;
            for id in note_ids {
                if seen.insert(id) {
                    filtered.push(id);
                }
            }
            filtered
        }
    } else {
        db::search_issue_ids(conn, terms, &statuses, &priorities, &kinds, all)?
    };

    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let config = UrgencyConfig::load(conn);

    let mut results: Vec<SearchResult> = Vec::with_capacity(ids.len());
    for id in &ids {
        let issue = db::get_issue(conn, *id)?;
        let notes = db::get_notes(conn, *id)?;
        let urg = urgency::compute_urgency(&issue, &config, conn);
        let blocked_by = db::get_blockers(conn, *id).unwrap_or_default();
        let is_blocked = db::is_blocked(conn, *id).unwrap_or(false);
        let (matched_fields, context_snippets) =
            compute_matched_fields_with_snippets(terms, &issue, &notes);

        // A hit with no literal field match is a false positive (e.g. an FTS
        // token match like "100" for the query "100%" that the literal term
        // does not support). Skip it so every returned result carries at
        // least one matched field.
        if matched_fields.is_empty() {
            continue;
        }

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
    let mut results = if skills.is_empty() {
        results
    } else {
        results
            .into_iter()
            .filter(|r| skills.iter().all(|s| r.skills.contains(s)))
            .collect()
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

    Ok(results)
}

pub fn compute_matched_fields_with_snippets(
    terms: &[String],
    issue: &crate::models::Issue,
    notes: &[crate::models::Note],
) -> (Vec<String>, HashMap<String, String>) {
    let mut fields = Vec::with_capacity(8);
    let mut snippets = HashMap::with_capacity(8);

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

    fn search_ids(conn: &Connection, query: &str, statuses: Vec<String>) -> Vec<i64> {
        let terms: Vec<String> = query.split_whitespace().map(ToString::to_string).collect();
        run_core(
            conn,
            query,
            &terms,
            false,
            &statuses,
            &[],
            &[],
            &[],
            None,
            None,
        )
        .expect("search")
        .iter()
        .map(|r| r.id)
        .collect()
    }

    // --- #168: search --status accepts the same synonyms as write paths ---

    #[test]
    fn search_status_filter_normalizes_synonyms() {
        let conn = db::open_test_db();
        let wip_id = insert_issue(&conn, "frobnicate widget");
        db::update_issue_field(&conn, wip_id, "status", "in-progress").expect("set status");
        let done_id = insert_issue(&conn, "frobnicate gadget");
        db::update_issue_field(&conn, done_id, "status", "done").expect("set status");

        assert_eq!(
            search_ids(&conn, "frobnicate", vec!["wip".to_string()]),
            vec![wip_id],
            "--status wip must match in-progress issues"
        );
        assert_eq!(
            search_ids(&conn, "frobnicate", vec!["closed".to_string()]),
            vec![done_id],
            "--status closed must match done issues"
        );
    }
}
