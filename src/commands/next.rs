use crate::commands::build_issue_detail;
use crate::db::{self, ClaimOutcome};
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::{Issue, ListFilter};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::env;

pub fn run(
    conn: &Connection,
    claim: bool,
    id: Option<i64>,
    skills: Vec<String>,
    agent: Option<String>,
    assigned_to: Option<String>,
    fmt: Format,
) -> Result<(), ItrError> {
    let config = UrgencyConfig::load(conn);
    // Resolve agent name: explicit flag > ITR_AGENT env var
    let agent_name = agent.or_else(|| env::var("ITR_AGENT").ok().filter(|s| !s.is_empty()));

    // If a specific ID is provided, claim it directly (with guardrails)
    let issue = if let Some(target_id) = id {
        if claim {
            let notes = claim_by_id(
                conn,
                target_id,
                &skills,
                agent_name.as_deref(),
                assigned_to.as_deref(),
            )?;
            for note in &notes {
                eprintln!("{note}");
            }
        }
        db::get_issue(conn, target_id)?
    } else {
        // Get all open, unblocked issues
        let issues = db::list_issues(
            conn,
            &ListFilter {
                statuses: vec!["open".to_string()],
                skills,
                assigned_to,
                ..ListFilter::default()
            },
        )?;

        if issues.is_empty() {
            error::print_empty(fmt.is_json(), "No eligible issues found.");
            return Ok(());
        }

        // Order candidates by urgency, highest first
        let candidates = rank_by_urgency(conn, issues, &config);

        if claim {
            // Compare-and-swap claim: a race loser whose candidate was stolen
            // by a concurrent claimer moves on to the next one.
            let ids: Vec<i64> = candidates.iter().map(|i| i.id).collect();
            match try_claim_in_order(conn, &ids, agent_name.as_deref())? {
                Some(claimed_id) => db::get_issue(conn, claimed_id)?,
                None => {
                    error::print_empty(fmt.is_json(), "No eligible issues found.");
                    return Ok(());
                }
            }
        } else {
            candidates.into_iter().next().ok_or(ItrError::NotFound(0))?
        }
    };

    let detail = build_issue_detail(conn, issue, &config)?;
    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
}

/// Sort issues by computed urgency, highest first.
fn rank_by_urgency(conn: &Connection, issues: Vec<Issue>, config: &UrgencyConfig) -> Vec<Issue> {
    let mut scored: Vec<(f64, Issue)> = issues
        .into_iter()
        .map(|issue| (urgency::compute_urgency(&issue, config, conn), issue))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, issue)| issue).collect()
}

/// Attempt to claim each candidate in order via the guarded compare-and-swap
/// in `db::claim_issue`. A candidate stolen by a concurrent claimer (0 rows
/// updated) is skipped and the next one is tried. Returns the claimed ID, or
/// `None` when every candidate was taken.
fn try_claim_in_order(
    conn: &Connection,
    ids: &[i64],
    agent: Option<&str>,
) -> Result<Option<i64>, ItrError> {
    for &candidate_id in ids {
        match db::claim_issue(conn, candidate_id, agent) {
            Ok(ClaimOutcome::Claimed { .. }) => return Ok(Some(candidate_id)),
            // Stolen between listing and claiming — try the next candidate.
            Ok(ClaimOutcome::NotOpen { .. }) => {}
            // Deleted between listing and claiming — also move on.
            Err(ItrError::NotFound(_)) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// Claim an explicit issue ID with guardrails. Returns `REVIEW:` notes for
/// stderr. Soft-fallback semantics:
/// - `open` issues are claimed via the same atomic compare-and-swap as
///   claim-next (with notes when the issue is blocked or was assigned to
///   someone else).
/// - `done`/`wontfix` issues are NOT resurrected; a note explains how to
///   reopen deliberately.
/// - `in-progress` issues assigned to a different agent are NOT stolen; a
///   note names the current assignee.
fn claim_by_id(
    conn: &Connection,
    id: i64,
    skills: &[String],
    agent: Option<&str>,
    assigned_filter: Option<&str>,
) -> Result<Vec<String>, ItrError> {
    let mut notes = Vec::new();

    // Selection filters only apply when picking a candidate; an explicit ID
    // bypasses them, so say so instead of silently dropping the flags.
    if !skills.is_empty() {
        notes.push(format!(
            "REVIEW: --skill is a selection filter and is ignored when claiming an explicit ID; issue {} was not validated against [{}]",
            id,
            skills.join(", ")
        ));
    }
    if let Some(filter) = assigned_filter {
        notes.push(format!(
            "REVIEW: --assigned-to '{filter}' is a selection filter and is ignored when claiming an explicit ID"
        ));
    }

    match db::claim_issue(conn, id, agent)? {
        ClaimOutcome::Claimed { prior_assigned_to } => {
            if !prior_assigned_to.is_empty() && agent != Some(prior_assigned_to.as_str()) {
                notes.push(format!(
                    "REVIEW: issue {id} was assigned to '{prior_assigned_to}' before this claim"
                ));
            }
            let blockers = open_blockers(conn, id)?;
            if !blockers.is_empty() {
                let list: Vec<String> = blockers.iter().map(|b| format!("#{b}")).collect();
                notes.push(format!(
                    "REVIEW: issue {} is blocked by open issue(s) {}; claimed anyway because the ID was explicit",
                    id,
                    list.join(", ")
                ));
            }
        }
        ClaimOutcome::NotOpen {
            status,
            assigned_to,
        } => match status.as_str() {
            "done" | "wontfix" => {
                notes.push(format!(
                    "REVIEW: issue {id} is '{status}' and was not reopened; run `itr update {id} --status open` first if you really want to claim it"
                ));
            }
            _ => {
                // in-progress (the only other non-open status)
                let taken_by_other = !assigned_to.is_empty() && agent != Some(assigned_to.as_str());
                if taken_by_other {
                    notes.push(format!(
                        "REVIEW: issue {id} is already in-progress and assigned to '{assigned_to}'; assignment left unchanged (use `itr assign {id} <agent>` to take over)"
                    ));
                } else if assigned_to.is_empty() {
                    if let Some(name) = agent {
                        db::record_event(conn, id, "assigned_to", &assigned_to, name)?;
                        db::update_issue_field(conn, id, "assigned_to", name)?;
                        notes.push(format!(
                            "REVIEW: issue {id} was already in-progress; recorded assignment to '{name}'"
                        ));
                    } else {
                        notes.push(format!("REVIEW: issue {id} is already in-progress"));
                    }
                } else {
                    notes.push(format!(
                        "REVIEW: issue {id} is already in-progress and assigned to you; claim was a no-op"
                    ));
                }
            }
        },
    }
    Ok(notes)
}

/// Blocker IDs of `id` whose issues are still active (not done/wontfix).
fn open_blockers(conn: &Connection, id: i64) -> Result<Vec<i64>, ItrError> {
    let mut open = Vec::new();
    for blocker_id in db::get_blockers(conn, id)? {
        let blocker = db::get_issue(conn, blocker_id)?;
        if blocker.status != "done" && blocker.status != "wontfix" {
            open.push(blocker_id);
        }
    }
    Ok(open)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::get_schema_sql()).unwrap();
        conn
    }

    fn add(conn: &Connection, title: &str) -> i64 {
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
        .unwrap()
        .id
    }

    // --- #154: race loser retries the next candidate ---

    #[test]
    fn race_loser_retries_next_candidate() {
        let conn = test_conn();
        let a = add(&conn, "top candidate");
        let b = add(&conn, "runner up");

        // A concurrent claimer steals the top candidate first.
        assert!(matches!(
            db::claim_issue(&conn, a, Some("rival")).unwrap(),
            ClaimOutcome::Claimed { .. }
        ));

        let won = try_claim_in_order(&conn, &[a, b], Some("me")).unwrap();
        assert_eq!(
            won,
            Some(b),
            "loser must fall through to the next candidate"
        );
        assert_eq!(db::get_issue(&conn, a).unwrap().assigned_to, "rival");
        assert_eq!(db::get_issue(&conn, b).unwrap().assigned_to, "me");
    }

    #[test]
    fn try_claim_in_order_returns_none_when_all_taken() {
        let conn = test_conn();
        let a = add(&conn, "only candidate");
        db::claim_issue(&conn, a, Some("rival")).unwrap();

        assert_eq!(try_claim_in_order(&conn, &[a], Some("me")).unwrap(), None);
        assert_eq!(
            db::get_issue(&conn, a).unwrap().assigned_to,
            "rival",
            "exhausted claimer must not steal"
        );
    }

    #[test]
    fn try_claim_in_order_skips_deleted_candidates() {
        let conn = test_conn();
        let a = add(&conn, "vanishing");
        let b = add(&conn, "still here");
        conn.execute("DELETE FROM issues WHERE id = ?1", rusqlite::params![a])
            .unwrap();

        assert_eq!(
            try_claim_in_order(&conn, &[a, b], Some("me")).unwrap(),
            Some(b)
        );
    }

    // --- #172: claim-by-ID guardrails ---

    #[test]
    fn claim_by_id_refuses_to_reopen_done_issue() {
        let conn = test_conn();
        let id = add(&conn, "already shipped");
        db::update_issue_field(&conn, id, "status", "done").unwrap();

        let notes = claim_by_id(&conn, id, &[], Some("me"), None).unwrap();

        let after = db::get_issue(&conn, id).unwrap();
        assert_eq!(after.status, "done", "claim must not resurrect done issues");
        assert_eq!(after.assigned_to, "");
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("REVIEW:") && n.contains("'done'")),
            "must emit a REVIEW note naming the prior state, got {notes:?}"
        );
    }

    #[test]
    fn claim_by_id_refuses_to_reopen_wontfix_issue() {
        let conn = test_conn();
        let id = add(&conn, "not doing this");
        db::update_issue_field(&conn, id, "status", "wontfix").unwrap();

        let notes = claim_by_id(&conn, id, &[], Some("me"), None).unwrap();

        assert_eq!(db::get_issue(&conn, id).unwrap().status, "wontfix");
        assert!(notes
            .iter()
            .any(|n| n.starts_with("REVIEW:") && n.contains("'wontfix'")));
    }

    #[test]
    fn claim_by_id_on_blocked_issue_notes_blockers() {
        let conn = test_conn();
        let blocker = add(&conn, "the blocker");
        let blocked = add(&conn, "the blocked one");
        db::add_dependency(&conn, blocker, blocked).unwrap();

        let notes = claim_by_id(&conn, blocked, &[], Some("me"), None).unwrap();

        // Explicit ID: still claimed (soft fallback), but with a signal.
        assert_eq!(db::get_issue(&conn, blocked).unwrap().status, "in-progress");
        assert!(
            notes.iter().any(|n| n.starts_with("REVIEW:")
                && n.contains("blocked")
                && n.contains(&format!("#{blocker}"))),
            "must name the open blocker, got {notes:?}"
        );
    }

    #[test]
    fn claim_by_id_does_not_steal_another_agents_in_progress_issue() {
        let conn = test_conn();
        let id = add(&conn, "rival's work");
        db::claim_issue(&conn, id, Some("rival")).unwrap();

        let notes = claim_by_id(&conn, id, &[], Some("me"), None).unwrap();

        let after = db::get_issue(&conn, id).unwrap();
        assert_eq!(after.assigned_to, "rival", "assignment must be unchanged");
        assert_eq!(after.status, "in-progress");
        assert!(
            notes.iter().any(|n| n.starts_with("REVIEW:")
                && n.contains("in-progress")
                && n.contains("'rival'")),
            "must name the current assignee, got {notes:?}"
        );
    }

    #[test]
    fn claim_by_id_adopts_unassigned_in_progress_issue() {
        let conn = test_conn();
        let id = add(&conn, "orphaned wip");
        db::claim_issue(&conn, id, None).unwrap();

        let notes = claim_by_id(&conn, id, &[], Some("me"), None).unwrap();

        assert_eq!(db::get_issue(&conn, id).unwrap().assigned_to, "me");
        assert!(notes
            .iter()
            .any(|n| n.starts_with("REVIEW:") && n.contains("already in-progress")));
    }

    #[test]
    fn claim_by_id_notes_prior_assignee_of_open_issue() {
        let conn = test_conn();
        let id = add(&conn, "pre-assigned but open");
        db::update_issue_field(&conn, id, "assigned_to", "rival").unwrap();

        let notes = claim_by_id(&conn, id, &[], Some("me"), None).unwrap();

        let after = db::get_issue(&conn, id).unwrap();
        assert_eq!(after.status, "in-progress");
        assert_eq!(after.assigned_to, "me", "open issues are claimable");
        assert!(notes
            .iter()
            .any(|n| n.starts_with("REVIEW:") && n.contains("'rival'")));
    }

    #[test]
    fn claim_by_id_warns_that_selection_filters_are_ignored() {
        let conn = test_conn();
        let id = add(&conn, "direct claim");

        let notes = claim_by_id(
            &conn,
            id,
            &["rust".to_string()],
            Some("me"),
            Some("someone-else"),
        )
        .unwrap();

        assert!(notes
            .iter()
            .any(|n| n.starts_with("REVIEW:") && n.contains("--skill")));
        assert!(notes
            .iter()
            .any(|n| n.starts_with("REVIEW:") && n.contains("--assigned-to")));
        // The claim itself still goes through.
        assert_eq!(db::get_issue(&conn, id).unwrap().status, "in-progress");
    }
}
