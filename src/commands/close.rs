use super::{build_issue_detail, print_detail_with_unblocked};
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::IssueDetail;
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    id: i64,
    reason: Option<String>,
    wontfix: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let (detail, unblocked) = close_issue(conn, id, reason, wontfix)?;
    print_detail_with_unblocked(&detail, &unblocked, fmt);
    Ok(())
}

/// `itr close <ID>... [REASON]` — one or more issue IDs, repeated,
/// comma-separated, or inclusive `A-B` ranges.
///
/// - Exactly one unique ID: byte-identical to the historical single-issue
///   contract, including the hard `NOT_FOUND` error for a missing issue.
/// - Multiple unique IDs: all closes run in one transaction with per-ID soft
///   fallback — a missing ID emits `REVIEW: id N not found; skipped` and the
///   rest proceed. Exit 0 if at least one close succeeded, exit 1 if none did.
pub fn run_multi(
    conn: &Connection,
    id_tokens: &[String],
    reason: Option<String>,
    wontfix: bool,
    duplicate_of: Option<i64>,
    fmt: Format,
) -> Result<(), ItrError> {
    let parsed = util::parse_id_tokens(id_tokens);
    for note in &parsed.notes {
        eprintln!("{}", note);
    }
    for token in &parsed.invalid {
        eprintln!(
            "REVIEW: ignoring non-integer issue ID '{}' — IDs may be repeated, comma-separated, or ranges (e.g. `itr close 12,14,17 \"reason\"`)",
            token
        );
    }
    for id in &parsed.duplicates {
        eprintln!(
            "REVIEW: duplicate issue ID {} requested; closing it once",
            id
        );
    }
    if parsed.ids.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid:
                "integer issue IDs, repeated, comma-separated, or ranges (e.g. `itr close 12,14,17`)"
                    .to_string(),
        });
    }

    if parsed.ids.len() == 1 {
        // Single-ID contract: unchanged behavior, hard NOT_FOUND on a missing
        // issue, duplicate relation recorded before the close.
        let id = parsed.ids[0];
        if let Some(dup_id) = duplicate_of {
            db::add_relation(conn, id, dup_id, "duplicate")?;
        }
        return run(conn, id, reason, wontfix, fmt);
    }

    let (results, skipped, review_notes) =
        close_many(conn, &parsed.ids, reason, wontfix, duplicate_of)?;
    for note in &review_notes {
        eprintln!("{}", note);
    }
    for id in &skipped {
        eprintln!("REVIEW: id {} not found; skipped", id);
    }
    if results.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid: "at least one existing issue ID".to_string(),
        });
    }
    print_multi(&results, fmt);
    Ok(())
}

/// Apply the close writes for every existing ID inside one transaction.
/// Missing IDs are collected into `skipped` (soft fallback) while every other
/// error still propagates and rolls the whole invocation back. Returns each
/// closed issue's detail with the issues it newly unblocked, plus REVIEW
/// notes destined for stderr.
#[allow(clippy::type_complexity)]
fn close_many(
    conn: &Connection,
    ids: &[i64],
    reason: Option<String>,
    wontfix: bool,
    duplicate_of: Option<i64>,
) -> Result<
    (
        Vec<(IssueDetail, Vec<(i64, String)>)>,
        Vec<i64>,
        Vec<String>,
    ),
    ItrError,
> {
    let reason = reason.unwrap_or_default();
    let status = if wontfix { "wontfix" } else { "done" };

    let tx = conn.unchecked_transaction()?;
    // A missing --duplicate-of target can never soft-recover: fail before
    // touching anything, matching the single-ID behavior.
    if let Some(dup_id) = duplicate_of {
        if !db::issue_exists(&tx, dup_id)? {
            return Err(ItrError::NotFound(dup_id));
        }
    }

    let config = UrgencyConfig::load(&tx);
    let mut results = Vec::new();
    let mut skipped = Vec::new();
    let mut review_notes = Vec::new();
    for &id in ids {
        let old_issue = match db::get_issue(&tx, id) {
            Ok(i) => i,
            Err(ItrError::NotFound(_)) => {
                skipped.push(id);
                continue;
            }
            Err(e) => return Err(e),
        };

        if let Some(dup_id) = duplicate_of {
            if id == dup_id {
                review_notes.push(format!(
                    "REVIEW: id {} is the --duplicate-of target itself; closed without a self-relation",
                    id
                ));
            } else {
                db::add_relation(&tx, id, dup_id, "duplicate")?;
            }
        }

        db::record_event(&tx, id, "status", &old_issue.status, status)?;
        db::update_issue_field(&tx, id, "status", status)?;
        if !reason.is_empty() {
            db::record_event(&tx, id, "close_reason", &old_issue.close_reason, &reason)?;
            db::update_issue_field(&tx, id, "close_reason", &reason)?;
        }

        let unblocked = db::get_newly_unblocked(&tx, id)?;
        db::remove_blocker_edges(&tx, id)?;

        let issue = db::get_issue(&tx, id)?;
        let detail = build_issue_detail(&tx, issue, &config)?;
        results.push((detail, unblocked));
    }

    if !results.is_empty() {
        tx.commit()?;
    }
    Ok((results, skipped, review_notes))
}

/// Print the batched close output: per-issue detail blocks with their own
/// UNBLOCKED lines (compact/pretty/oneline), or a JSON array where each
/// element mirrors the single-close object including its `unblocked` key.
fn print_multi(results: &[(IssueDetail, Vec<(i64, String)>)], fmt: Format) {
    match fmt {
        Format::Json => {
            let arr: Vec<serde_json::Value> = results
                .iter()
                .map(|(detail, unblocked)| {
                    let mut value = serde_json::to_value(detail).unwrap_or_default();
                    if !unblocked.is_empty() {
                        let list: Vec<serde_json::Value> = unblocked
                            .iter()
                            .map(|(uid, utitle)| serde_json::json!({"id": uid, "title": utitle}))
                            .collect();
                        value["unblocked"] = serde_json::Value::Array(list);
                    }
                    value
                })
                .collect();
            format::println_json(&serde_json::Value::Array(arr).to_string());
        }
        _ => {
            let blocks: Vec<String> = results
                .iter()
                .map(|(detail, unblocked)| {
                    let mut block = format::format_issue_detail(detail, fmt);
                    let unblocked_str = format::format_unblocked(unblocked, fmt);
                    if !unblocked_str.is_empty() {
                        block.push('\n');
                        block.push_str(&unblocked_str);
                    }
                    block
                })
                .collect();
            println!("{}", blocks.join("\n\n"));
        }
    }
}

/// Apply all close writes (status event, status flip, optional `close_reason`
/// event + field, dependency-edge cleanup) inside a single transaction so a
/// mid-close failure leaves the issue fully unchanged, and build the output
/// detail from the updated state before committing.
fn close_issue(
    conn: &Connection,
    id: i64,
    reason: Option<String>,
    wontfix: bool,
) -> Result<(IssueDetail, Vec<(i64, String)>), ItrError> {
    let reason = reason.unwrap_or_default();

    let status = if wontfix { "wontfix" } else { "done" };

    let tx = conn.unchecked_transaction()?;

    // Capture old values for event recording
    let old_issue = db::get_issue(&tx, id)?;

    db::record_event(&tx, id, "status", &old_issue.status, status)?;
    db::update_issue_field(&tx, id, "status", status)?;
    if !reason.is_empty() {
        db::record_event(&tx, id, "close_reason", &old_issue.close_reason, &reason)?;
        db::update_issue_field(&tx, id, "close_reason", &reason)?;
    }

    // Auto-clean dependency edges where this issue was the blocker
    let unblocked = db::get_newly_unblocked(&tx, id)?;
    db::remove_blocker_edges(&tx, id)?;

    // Build the output detail from the updated state
    let issue = db::get_issue(&tx, id)?;
    let config = UrgencyConfig::load(&tx);
    let detail = build_issue_detail(&tx, issue, &config)?;

    tx.commit()?;
    Ok((detail, unblocked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn test_conn() -> Connection {
        db::init_db(Path::new(":memory:")).expect("init in-memory db")
    }

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

    #[test]
    fn close_applies_all_writes_and_reports_unblocked() {
        let conn = test_conn();
        let blocker = insert_issue(&conn, "blocker");
        let blocked = insert_issue(&conn, "blocked");
        db::add_dependency(&conn, blocker, blocked).expect("add dependency");

        let (detail, unblocked) =
            close_issue(&conn, blocker, Some("all done".to_string()), false).expect("close");

        assert_eq!(detail.issue.status, "done");
        assert_eq!(detail.issue.close_reason, "all done");
        assert_eq!(unblocked, vec![(blocked, "blocked".to_string())]);

        let issue = db::get_issue(&conn, blocker).expect("get issue");
        assert_eq!(issue.status, "done");
        assert_eq!(issue.close_reason, "all done");
        let events = db::get_events_for_issue(&conn, blocker).expect("events");
        let fields: Vec<&str> = events.iter().map(|e| e.field.as_str()).collect();
        assert_eq!(fields, vec!["status", "close_reason"]);
        assert!(
            db::get_blockers(&conn, blocked)
                .expect("blockers")
                .is_empty(),
            "blocker edge must be cleaned up on close"
        );
    }

    #[test]
    fn close_many_closes_all_ids_in_one_transaction() {
        let conn = test_conn();
        let a = insert_issue(&conn, "a");
        let b = insert_issue(&conn, "b");
        let c = insert_issue(&conn, "c");

        let (results, skipped, notes) =
            close_many(&conn, &[a, b, c], Some("swept".to_string()), false, None).expect("close");

        assert_eq!(results.len(), 3);
        assert!(skipped.is_empty());
        assert!(notes.is_empty());
        for id in [a, b, c] {
            let issue = db::get_issue(&conn, id).expect("get");
            assert_eq!(issue.status, "done");
            assert_eq!(issue.close_reason, "swept");
            let events = db::get_events_for_issue(&conn, id).expect("events");
            assert!(
                events
                    .iter()
                    .any(|e| e.field == "status" && e.new_value == "done"),
                "each close must record its own audit event (#35)"
            );
        }
    }

    #[test]
    fn close_many_skips_missing_ids_and_closes_the_rest() {
        let conn = test_conn();
        let a = insert_issue(&conn, "a");
        let b = insert_issue(&conn, "b");

        let (results, skipped, _) =
            close_many(&conn, &[a, 999, b], None, false, None).expect("close");

        assert_eq!(results.len(), 2);
        assert_eq!(skipped, vec![999]);
        assert_eq!(db::get_issue(&conn, a).unwrap().status, "done");
        assert_eq!(db::get_issue(&conn, b).unwrap().status, "done");
    }

    #[test]
    fn close_many_all_missing_returns_empty_without_commit() {
        let conn = test_conn();
        insert_issue(&conn, "untouched");
        let (results, skipped, _) =
            close_many(&conn, &[998, 999], None, false, None).expect("soft fallback");
        assert!(results.is_empty());
        assert_eq!(skipped, vec![998, 999]);
    }

    #[test]
    fn close_many_duplicate_of_records_relations_and_skips_self_target() {
        let conn = test_conn();
        let original = insert_issue(&conn, "original");
        let d1 = insert_issue(&conn, "dup1");
        let d2 = insert_issue(&conn, "dup2");

        let (results, _, notes) =
            close_many(&conn, &[d1, d2, original], None, false, Some(original)).expect("close");

        assert_eq!(results.len(), 3, "the target itself still closes");
        assert_eq!(notes.len(), 1, "self-relation skip gets a REVIEW note");
        assert!(notes[0].contains("--duplicate-of target"));
        for id in [d1, d2] {
            let rels = db::get_relations(&conn, id).expect("relations");
            assert!(
                rels.iter()
                    .any(|r| r.relation_type == "duplicate" && r.target_id == original),
                "duplicate relation must be recorded for {id}"
            );
        }
    }

    #[test]
    fn close_many_missing_duplicate_of_target_is_hard_error() {
        let conn = test_conn();
        let a = insert_issue(&conn, "a");
        let b = insert_issue(&conn, "b");
        let err = close_many(&conn, &[a, b], None, false, Some(999)).unwrap_err();
        assert!(matches!(err, ItrError::NotFound(999)));
        assert_eq!(
            db::get_issue(&conn, a).unwrap().status,
            "open",
            "nothing may be written when --duplicate-of is missing"
        );
    }

    #[test]
    fn run_multi_single_missing_id_stays_hard_not_found() {
        let conn = test_conn();
        let err = run_multi(
            &conn,
            &["999".to_string()],
            None,
            false,
            None,
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::NotFound(999)));
    }

    #[test]
    fn run_multi_all_missing_is_exit_1() {
        let conn = test_conn();
        insert_issue(&conn, "other");
        let err = run_multi(
            &conn,
            &["998,999".to_string()],
            None,
            false,
            None,
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { .. }));
    }

    #[test]
    fn run_multi_range_closes_span() {
        let conn = test_conn();
        let a = insert_issue(&conn, "a");
        let b = insert_issue(&conn, "b");
        let c = insert_issue(&conn, "c");
        run_multi(
            &conn,
            &[format!("{}-{}", a, c)],
            Some("done".to_string()),
            false,
            None,
            Format::Compact,
        )
        .expect("range close");
        for id in [a, b, c] {
            assert_eq!(db::get_issue(&conn, id).unwrap().status, "done");
        }
    }

    #[test]
    fn mid_close_failure_leaves_issue_fully_unchanged() {
        let conn = test_conn();
        let blocker = insert_issue(&conn, "blocker");
        let blocked = insert_issue(&conn, "blocked");
        db::add_dependency(&conn, blocker, blocked).expect("add dependency");

        // Inject a failure at the LAST write step (dependency cleanup), so
        // every earlier write (status event, status flip, close_reason event,
        // close_reason field) has already been issued when the close fails.
        conn.execute_batch(
            "CREATE TRIGGER fail_dep_cleanup BEFORE DELETE ON dependencies
             BEGIN SELECT RAISE(ABORT, 'injected mid-close failure'); END;",
        )
        .expect("create failure trigger");

        let result = close_issue(&conn, blocker, Some("all done".to_string()), false);
        assert!(result.is_err(), "injected failure must propagate");

        // All-or-nothing: the issue must be exactly as before the close.
        let issue = db::get_issue(&conn, blocker).expect("get issue");
        assert_eq!(issue.status, "open", "status flip must be rolled back");
        assert_eq!(issue.close_reason, "", "close_reason must be rolled back");
        let events = db::get_events_for_issue(&conn, blocker).expect("events");
        assert!(
            events.is_empty(),
            "recorded events must be rolled back, got: {:?}",
            events
        );
        assert_eq!(
            db::get_blockers(&conn, blocked).expect("blockers"),
            vec![blocker],
            "dependency edge must be retained"
        );
    }
}
