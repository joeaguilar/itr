use super::build_issue_summary;
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::{IssueDetail, IssueSummary, ListFilter};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

/// Outcome of parsing the raw positional ID arguments for `get`/`show`.
///
/// IDs may be repeated arguments, comma-separated lists, or a mix
/// (`itr get 1 2,3`). Parsing is a pure function so the soft-fallback
/// reporting (REVIEW notes for duplicates and non-integer tokens) stays in
/// [`run`] and the splitting/dedup logic is unit-testable.
struct ParsedIds {
    /// Unique IDs in first-seen request order.
    ids: Vec<i64>,
    /// IDs requested more than once (unique, first-seen order).
    duplicates: Vec<i64>,
    /// Tokens that did not parse as integers.
    invalid: Vec<String>,
}

fn parse_id_args(args: &[String]) -> ParsedIds {
    let mut ids = Vec::new();
    let mut duplicates = Vec::new();
    let mut invalid = Vec::new();
    for arg in args {
        for token in arg.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            match token.parse::<i64>() {
                Ok(id) => {
                    if ids.contains(&id) {
                        if !duplicates.contains(&id) {
                            duplicates.push(id);
                        }
                    } else {
                        ids.push(id);
                    }
                }
                Err(_) => invalid.push(token.to_string()),
            }
        }
    }
    ParsedIds {
        ids,
        duplicates,
        invalid,
    }
}

/// Fetch the full [`IssueDetail`] for one issue: urgency breakdown, blockers,
/// notes, relations, and (for epics) child summaries. This is the single
/// source of detail used by both the single-ID and batched paths so the two
/// can never drift.
fn fetch_detail(conn: &Connection, id: i64) -> Result<IssueDetail, ItrError> {
    let issue = db::get_issue(conn, id)?;
    let config = UrgencyConfig::load(conn);
    let (urg, breakdown) = urgency::compute_urgency_with_breakdown(&issue, &config, conn);
    let blocked_by = db::get_blockers(conn, issue.id)?;
    let blocks = db::get_blocking(conn, issue.id)?;
    let is_blocked = db::is_blocked(conn, issue.id)?;
    let notes = db::get_notes(conn, issue.id)?;

    // If epic, get children
    let children = if issue.kind == "epic" {
        let child_issues = db::list_issues(
            conn,
            &ListFilter {
                include_blocked: true,
                parent_id: Some(issue.id),
                all: true,
                ..ListFilter::default()
            },
        )?;
        let child_summaries: Vec<IssueSummary> = child_issues
            .iter()
            .map(|i| build_issue_summary(conn, i, &config))
            .collect();
        if child_summaries.is_empty() {
            None
        } else {
            Some(child_summaries)
        }
    } else {
        None
    };

    Ok(IssueDetail {
        issue,
        urgency: urg,
        blocked_by,
        blocks,
        is_blocked,
        notes,
        urgency_breakdown: Some(breakdown),
        children,
        relations: db::get_relations(conn, id)?,
    })
}

/// Fetch details for a batch of IDs. Missing issues do not fail the batch:
/// they are collected into the returned `missing` list (soft fallback, #136)
/// while every other error still propagates. Order of `details` follows the
/// request order of `ids`.
fn collect_details(
    conn: &Connection,
    ids: &[i64],
) -> Result<(Vec<IssueDetail>, Vec<i64>), ItrError> {
    let mut details = Vec::with_capacity(ids.len());
    let mut missing = Vec::new();
    for &id in ids {
        match fetch_detail(conn, id) {
            Ok(detail) => details.push(detail),
            Err(ItrError::NotFound(_)) => missing.push(id),
            Err(err) => return Err(err),
        }
    }
    Ok((details, missing))
}

/// `itr get <ID>...` / `itr show <ID>...` — one or more issue IDs, repeated
/// or comma-separated (#136).
///
/// - Exactly one unique ID: byte-identical to the historical single-issue
///   contract, including the hard `NOT_FOUND` error for a missing issue.
/// - Multiple unique IDs: batched output (JSON array of `IssueDetail`;
///   blank-line-separated per-issue blocks in compact/oneline/pretty).
///   Missing IDs emit a `REVIEW:` note each and the found issues are still
///   returned with exit 0; all-missing prints the standard empty result.
/// - Duplicate IDs are fetched once; non-integer tokens are skipped — both
///   with `REVIEW:` notes. A request with no parseable ID at all is a hard
///   `INVALID_VALUE`.
pub fn run(conn: &Connection, id_args: &[String], fmt: Format) -> Result<(), ItrError> {
    let parsed = parse_id_args(id_args);
    for token in &parsed.invalid {
        eprintln!(
            "REVIEW: ignoring non-integer issue ID '{}' — IDs may be repeated or comma-separated (e.g. `itr get 1,2,3`)",
            token
        );
    }
    for id in &parsed.duplicates {
        eprintln!(
            "REVIEW: duplicate issue ID {} requested; returning it once",
            id
        );
    }

    if parsed.ids.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_args.join(","),
            valid: "integer issue IDs, repeated or comma-separated (e.g. `itr get 1,2,3`)"
                .to_string(),
        });
    }

    if parsed.ids.len() == 1 {
        // Single-ID contract: unchanged bytes, hard NOT_FOUND on a missing issue.
        let detail = fetch_detail(conn, parsed.ids[0])?;
        println!("{}", format::format_issue_detail(&detail, fmt));
        return Ok(());
    }

    let (details, missing) = collect_details(conn, &parsed.ids)?;
    for id in &missing {
        eprintln!("REVIEW: issue {} not found; skipped in batched get", id);
    }
    if details.is_empty() {
        error::print_empty(fmt.is_json(), "No matching issues found.");
        return Ok(());
    }
    println!("{}", format::format_issue_details(&details, fmt));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parse_id_args_accepts_comma_and_repeated_forms() {
        let parsed = parse_id_args(&args(&["1,2", "3", "4,5"]));
        assert_eq!(parsed.ids, vec![1, 2, 3, 4, 5]);
        assert!(parsed.duplicates.is_empty());
        assert!(parsed.invalid.is_empty());
    }

    #[test]
    fn parse_id_args_dedups_and_reports_duplicates_once() {
        let parsed = parse_id_args(&args(&["1,1,2", "1", "2"]));
        assert_eq!(parsed.ids, vec![1, 2], "first-seen order, no repeats");
        assert_eq!(parsed.duplicates, vec![1, 2]);
        assert!(parsed.invalid.is_empty());
    }

    #[test]
    fn parse_id_args_collects_invalid_tokens_and_keeps_valid_ones() {
        let parsed = parse_id_args(&args(&["1,abc", "x", "2"]));
        assert_eq!(parsed.ids, vec![1, 2]);
        assert_eq!(parsed.invalid, vec!["abc".to_string(), "x".to_string()]);
    }

    #[test]
    fn parse_id_args_skips_empty_tokens() {
        // Trailing/doubled commas and whitespace are noise, not errors.
        let parsed = parse_id_args(&args(&["1,,2,", " 3 "]));
        assert_eq!(parsed.ids, vec![1, 2, 3]);
        assert!(parsed.invalid.is_empty());
    }

    fn seed(conn: &rusqlite::Connection, title: &str) -> i64 {
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

    #[test]
    fn collect_details_preserves_request_order_and_reports_missing() {
        let conn = db::open_test_db();
        let a = seed(&conn, "first");
        let b = seed(&conn, "second");

        let (details, missing) =
            collect_details(&conn, &[b, 999, a]).expect("batched fetch succeeds");
        assert_eq!(
            details.iter().map(|d| d.issue.id).collect::<Vec<_>>(),
            vec![b, a],
            "details follow request order"
        );
        assert_eq!(missing, vec![999]);
        assert!(
            details.iter().all(|d| d.urgency_breakdown.is_some()),
            "batched details carry the full single-issue payload"
        );
    }

    #[test]
    fn collect_details_all_missing_is_empty_not_error() {
        let conn = db::open_test_db();
        seed(&conn, "only");
        let (details, missing) = collect_details(&conn, &[998, 999]).expect("soft fallback");
        assert!(details.is_empty());
        assert_eq!(missing, vec![998, 999]);
    }

    #[test]
    fn run_single_missing_id_stays_a_hard_not_found() {
        // Single-ID compatibility: `itr get 999` must still hard-error.
        let conn = db::open_test_db();
        let err = run(&conn, &args(&["999"]), Format::Compact).unwrap_err();
        assert!(matches!(err, ItrError::NotFound(999)));
    }

    #[test]
    fn run_with_no_parseable_ids_is_invalid_value() {
        let conn = db::open_test_db();
        let err = run(&conn, &args(&["abc,def"]), Format::Compact).unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { .. }));
    }
}
