use super::build_issue_summary;
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::{IssueDetail, IssueSummary, ListFilter};
use crate::urgency::{self, UrgencyConfig};
use crate::util;
use rusqlite::Connection;

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

/// `itr get <ID>...` / `itr show <ID>...` — one or more issue IDs, repeated,
/// comma-separated, or inclusive `A-B` ranges (#136).
///
/// - Exactly one unique ID: byte-identical to the historical single-issue
///   contract, including the hard `NOT_FOUND` error for a missing issue.
/// - Multiple unique IDs: batched output (JSON array of `IssueDetail`;
///   blank-line-separated per-issue blocks in compact/oneline/pretty).
///   Missing IDs emit a `REVIEW:` note each and the found issues are still
///   returned with exit 0; all-missing prints the standard empty result.
/// - Duplicate IDs are fetched once; unparseable tokens are skipped — both
///   with `REVIEW:` notes. A request with no parseable ID at all is a hard
///   `INVALID_VALUE`.
pub fn run(conn: &Connection, id_args: &[String], fmt: Format) -> Result<(), ItrError> {
    let parsed = util::parse_id_tokens(id_args);
    for note in &parsed.notes {
        eprintln!("{}", note);
    }
    for token in &parsed.invalid {
        eprintln!(
            "REVIEW: ignoring non-integer issue ID '{}' — IDs may be repeated, comma-separated, or ranges (e.g. `itr get 1,2,5-8`)",
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
            valid:
                "integer issue IDs, repeated, comma-separated, or ranges (e.g. `itr get 1,2,5-8`)"
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

    // ID-token parsing (comma lists, ranges, duplicates) is unit-tested in
    // `util::tests` — the parser is shared with the multi-ID mutating verbs.

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
