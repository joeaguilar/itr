use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::{BulkResult, ListFilter, UnblockedIssue};
use crate::normalize;
use rusqlite::Connection;
use std::collections::HashSet;

/// Resolve the shared bulk filter grammar (`--status/--priority/--kind/
/// --tag/--skill/--assigned-to`) to the matching issue IDs. Errors with
/// `NoFilters` when no filter was given — bulk verbs never operate on the
/// whole database implicitly.
#[allow(clippy::too_many_arguments)]
fn resolve_filter_ids(
    conn: &Connection,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    skill: Option<String>,
    assigned_to: Option<String>,
) -> Result<Vec<i64>, ItrError> {
    if status.is_none()
        && priority.is_none()
        && kind.is_none()
        && tag.is_none()
        && skill.is_none()
        && assigned_to.is_none()
    {
        return Err(ItrError::NoFilters);
    }

    let statuses = status
        .map(|s| vec![normalize::normalize_status(&s)])
        .unwrap_or_default();
    let priorities = priority
        .map(|p| vec![normalize::normalize_priority(&p)])
        .unwrap_or_default();
    let kinds = kind
        .map(|k| vec![normalize::normalize_kind(&k)])
        .unwrap_or_default();
    let tags: Vec<String> = tag.into_iter().collect();
    let skills: Vec<String> = skill.into_iter().collect();

    let issues = db::list_issues(
        conn,
        &ListFilter {
            statuses,
            priorities,
            kinds,
            tags,
            skills,
            include_blocked: true,
            assigned_to,
            ..ListFilter::default()
        },
    )?;
    Ok(issues.iter().map(|i| i.id).collect())
}

#[allow(clippy::too_many_arguments)]
pub fn run_close(
    conn: &Connection,
    reason: Option<String>,
    wontfix: bool,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    skill: Option<String>,
    assigned_to: Option<String>,
    dry_run: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let ids = resolve_filter_ids(conn, status, priority, kind, tag, skill, assigned_to)?;
    let close_status = if wontfix { "wontfix" } else { "done" };
    let reason = reason.unwrap_or_default();

    let mut all_unblocked = Vec::new();

    if !dry_run {
        let tx = conn.unchecked_transaction()?;
        for id in &ids {
            let old_issue = db::get_issue(&tx, *id)?;
            db::record_event(&tx, *id, "status", &old_issue.status, close_status)?;
            db::update_issue_field(&tx, *id, "status", close_status)?;
            if !reason.is_empty() {
                db::record_event(&tx, *id, "close_reason", &old_issue.close_reason, &reason)?;
                db::update_issue_field(&tx, *id, "close_reason", &reason)?;
            }
            let unblocked = db::get_newly_unblocked(&tx, *id)?;
            for (uid, utitle) in unblocked {
                if !all_unblocked.iter().any(|u: &UnblockedIssue| u.id == uid) {
                    all_unblocked.push(UnblockedIssue {
                        id: uid,
                        title: utitle,
                    });
                }
            }
            // Auto-clean dependency edges where this issue was the blocker
            db::remove_blocker_edges(&tx, *id)?;
        }
        tx.commit()?;
    }

    let result = BulkResult {
        action: "bulk_close".to_string(),
        count: ids.len(),
        ids,
        unblocked: all_unblocked,
        dry_run,
    };

    print_result(&result, fmt);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run_update(
    conn: &Connection,
    set_status: Option<String>,
    set_priority: Option<String>,
    add_tag: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    skill: Option<String>,
    assigned_to: Option<String>,
    dry_run: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let (result, review_notes) = run_update_core(
        conn,
        set_status,
        set_priority,
        add_tag,
        status,
        priority,
        kind,
        tag,
        skill,
        assigned_to,
        dry_run,
    )?;
    for note in &review_notes {
        eprintln!("{note}");
    }
    print_result(&result, fmt);
    Ok(())
}

/// Testable core of `bulk update`: returns the result envelope plus REVIEW
/// notes destined for stderr instead of printing directly.
#[allow(clippy::too_many_arguments)]
fn run_update_core(
    conn: &Connection,
    set_status: Option<String>,
    set_priority: Option<String>,
    add_tag: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    skill: Option<String>,
    assigned_to: Option<String>,
    dry_run: bool,
) -> Result<(BulkResult, Vec<String>), ItrError> {
    let mut review_notes: Vec<String> = Vec::new();

    // Soft fallback (#162): validate --set-status / --set-priority up front
    // so an unrecognized value keeps each issue's current value with a
    // REVIEW note instead of leaking a raw CHECK-constraint DB error.
    // Mirrors batch update's keep-current semantics.
    let set_status = match set_status.map(|s| normalize::normalize_status(&s)) {
        Some(s) if normalize::validate_status(&s).is_err() => {
            review_notes.push(format!(
                "REVIEW: status '{s}' not recognized; kept each issue's current status. Valid: open, in-progress, done, wontfix"
            ));
            None
        }
        other => other,
    };
    let set_priority = match set_priority.map(|p| normalize::normalize_priority(&p)) {
        Some(p) if normalize::validate_priority(&p).is_err() => {
            review_notes.push(format!(
                "REVIEW: priority '{p}' not recognized; kept each issue's current priority. Valid: critical, high, medium, low"
            ));
            None
        }
        other => other,
    };

    let ids = resolve_filter_ids(conn, status, priority, kind, tag, skill, assigned_to)?;
    let mut all_unblocked = Vec::new();
    let mut seen_unblocked = HashSet::new();
    let cleanup_blockers = matches!(set_status.as_deref(), Some("done" | "wontfix"));

    if !dry_run {
        let tx = conn.unchecked_transaction()?;
        for id in &ids {
            let old_issue = db::get_issue(&tx, *id)?;
            if let Some(ref s) = set_status {
                db::record_event(&tx, *id, "status", &old_issue.status, s)?;
                db::update_issue_field(&tx, *id, "status", s)?;
            }
            if let Some(ref p) = set_priority {
                db::record_event(&tx, *id, "priority", &old_issue.priority, p)?;
                db::update_issue_field(&tx, *id, "priority", p)?;
            }
            if let Some(ref new_tag) = add_tag {
                let mut current_tags = old_issue.tags.clone();
                if !current_tags.contains(new_tag) {
                    let old_json = serde_json::to_string(&current_tags)?;
                    current_tags.push(new_tag.clone());
                    let new_json = serde_json::to_string(&current_tags)?;
                    db::record_event(&tx, *id, "tags", &old_json, &new_json)?;
                    db::update_issue_field(&tx, *id, "tags", &new_json)?;
                }
            }
            if cleanup_blockers {
                let unblocked = db::get_newly_unblocked(&tx, *id)?;
                for (uid, utitle) in unblocked {
                    if seen_unblocked.insert(uid) {
                        all_unblocked.push(UnblockedIssue {
                            id: uid,
                            title: utitle,
                        });
                    }
                }
                db::remove_blocker_edges(&tx, *id)?;
            }
        }
        tx.commit()?;
    }

    let result = BulkResult {
        action: "bulk_update".to_string(),
        count: ids.len(),
        ids,
        unblocked: all_unblocked,
        dry_run,
    };

    Ok((result, review_notes))
}

/// `itr bulk relate --to N [--type T] <filters>` — relate every issue
/// matching the shared bulk filter grammar to one target issue.
///
/// Self-edges (a matched issue equal to `--to`) are skipped with a `REVIEW:`
/// note. `--dry-run` runs the exact same code path inside a transaction that
/// is rolled back instead of committed, so validation cannot drift from the
/// real run while the database stays untouched.
#[allow(clippy::too_many_arguments)]
pub fn run_relate(
    conn: &Connection,
    to: i64,
    relation_type: &str,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    skill: Option<String>,
    assigned_to: Option<String>,
    dry_run: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    super::relate::validate_relation_type(relation_type)?;
    let ids = resolve_filter_ids(conn, status, priority, kind, tag, skill, assigned_to)?;
    if !db::issue_exists(conn, to)? {
        return Err(ItrError::NotFound(to));
    }

    let tx = conn.unchecked_transaction()?;
    let mut links: Vec<(i64, bool)> = Vec::new();
    for &id in &ids {
        if id == to {
            eprintln!(
                "REVIEW: issue {} matches the filter and equals --to; self-relation skipped",
                id
            );
            continue;
        }
        // Same code path as single `itr relate` — including NOT_FOUND checks.
        let created = db::add_relation(&tx, id, to, relation_type)?;
        links.push((id, created));
    }
    if !dry_run {
        tx.commit()?;
    }

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "bulk_relate",
                "to": to,
                "relation_type": relation_type,
                "count": links.len(),
                "ids": links.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                "dry_run": dry_run,
            });
            println!("{}", out);
        }
        _ => {
            for (id, created) in &links {
                let verb = if *created { "created" } else { "exists" };
                println!("RELATION:{} {} -> {} ({})", verb, id, to, relation_type);
            }
            println!(
                "BULK_RELATE: {} issues [{}]{}",
                links.len(),
                links
                    .iter()
                    .map(|(id, _)| id.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                if dry_run { " (dry-run)" } else { "" }
            );
        }
    }
    Ok(())
}

/// `itr bulk depend --on N <filters>` — make every issue matching the shared
/// bulk filter grammar blocked by one issue.
///
/// Self-edges (a matched issue equal to `--on`) are skipped with a `REVIEW:`
/// note. Cycle detection is the single-`depend` code path and stays a hard
/// error, rolling the whole invocation back. `--dry-run` rolls the
/// transaction back instead of committing — planned edges are printed and
/// nothing is written.
#[allow(clippy::too_many_arguments)]
pub fn run_depend(
    conn: &Connection,
    on: i64,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    skill: Option<String>,
    assigned_to: Option<String>,
    dry_run: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let ids = resolve_filter_ids(conn, status, priority, kind, tag, skill, assigned_to)?;
    if !db::issue_exists(conn, on)? {
        return Err(ItrError::NotFound(on));
    }

    let tx = conn.unchecked_transaction()?;
    let mut edges: Vec<(i64, bool)> = Vec::new();
    for &id in &ids {
        if id == on {
            eprintln!(
                "REVIEW: issue {} matches the filter and equals --on; self-dependency skipped",
                id
            );
            continue;
        }
        // Same code path as single `itr depend` — cycles stay hard errors.
        let created = db::add_dependency(&tx, on, id)?;
        edges.push((id, created));
    }
    if !dry_run {
        tx.commit()?;
    }

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "bulk_depend",
                "on": on,
                "count": edges.len(),
                "ids": edges.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                "dry_run": dry_run,
            });
            println!("{}", out);
        }
        _ => {
            for (id, _) in &edges {
                println!("DEPEND: {} blocked by {}", id, on);
            }
            println!(
                "BULK_DEPEND: {} issues [{}]{}",
                edges.len(),
                edges
                    .iter()
                    .map(|(id, _)| id.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                if dry_run { " (dry-run)" } else { "" }
            );
        }
    }
    Ok(())
}

/// `itr bulk note <TEXT> [--agent A] <filters>` — append the same note to
/// every issue matching the shared bulk filter grammar.
///
/// `--dry-run` rolls the transaction back instead of committing — the
/// would-be notes are printed and nothing is written.
#[allow(clippy::too_many_arguments)]
pub fn run_note(
    conn: &Connection,
    text: &str,
    agent: &str,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    skill: Option<String>,
    assigned_to: Option<String>,
    dry_run: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    if text.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "text".to_string(),
            value: String::new(),
            valid: "non-empty string".to_string(),
        });
    }
    let agent = super::note::resolve_agent(agent);
    let ids = resolve_filter_ids(conn, status, priority, kind, tag, skill, assigned_to)?;

    let tx = conn.unchecked_transaction()?;
    let mut notes = Vec::new();
    for &id in &ids {
        notes.push(db::add_note(&tx, id, text, &agent)?);
    }
    if !dry_run {
        tx.commit()?;
    }

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "bulk_note",
                "count": notes.len(),
                "ids": notes.iter().map(|n| n.issue_id).collect::<Vec<_>>(),
                "dry_run": dry_run,
            });
            println!("{}", out);
        }
        _ => {
            for note in &notes {
                println!("{}", super::note::format_note_line(note));
            }
            println!(
                "BULK_NOTE: {} issues [{}]{}",
                notes.len(),
                notes
                    .iter()
                    .map(|n| n.issue_id.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                if dry_run { " (dry-run)" } else { "" }
            );
        }
    }
    Ok(())
}

fn print_result(result: &BulkResult, fmt: Format) {
    match fmt {
        Format::Json => {
            println!("{}", serde_json::to_string(result).unwrap_or_default());
        }
        _ => {
            println!(
                "{}: {} issues [{}]{}",
                result.action.to_uppercase(),
                result.count,
                result
                    .ids
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
                if result.dry_run { " (dry-run)" } else { "" }
            );
            for u in &result.unblocked {
                println!("UNBLOCKED:{} \"{}\"", u.id, u.title);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_test_db;

    fn seed_tagged(conn: &Connection, title: &str, tag: &str) -> i64 {
        db::insert_issue(
            conn,
            title,
            "medium",
            "task",
            "",
            &[],
            &[tag.to_string()],
            &[],
            "",
            None,
            "",
        )
        .unwrap()
        .id
    }

    // --- #162: bogus --set-status/--set-priority must soft-fall, not leak a
    // raw CHECK-constraint DB error ---

    #[test]
    fn update_bogus_set_status_keeps_current_not_check_error() {
        let conn = open_test_db();
        let id = seed_tagged(&conn, "victim", "x");
        let result = run_update(
            &conn,
            Some("bogus".to_string()),
            None,
            None,
            None,
            None,
            None,
            Some("x".to_string()),
            None,
            None,
            false,
            Format::Compact,
        );
        assert!(
            result.is_ok(),
            "bogus --set-status must soft-fall, got: {result:?}"
        );
        assert_eq!(
            db::get_issue(&conn, id).unwrap().status,
            "open",
            "issue must keep its current status"
        );
    }

    #[test]
    fn update_bogus_set_priority_keeps_current_not_check_error() {
        let conn = open_test_db();
        let id = seed_tagged(&conn, "victim", "x");
        let result = run_update(
            &conn,
            None,
            Some("bogus".to_string()),
            None,
            None,
            None,
            None,
            Some("x".to_string()),
            None,
            None,
            false,
            Format::Compact,
        );
        assert!(
            result.is_ok(),
            "bogus --set-priority must soft-fall, got: {result:?}"
        );
        assert_eq!(
            db::get_issue(&conn, id).unwrap().priority,
            "medium",
            "issue must keep its current priority"
        );
    }

    fn event_count(conn: &Connection, id: i64) -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM events WHERE issue_id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn update_core_bogus_set_status_emits_review_note_and_no_event() {
        let conn = open_test_db();
        let id = seed_tagged(&conn, "victim", "x");
        let (result, notes) = run_update_core(
            &conn,
            Some("bogus".to_string()),
            None,
            None,
            None,
            None,
            None,
            Some("x".to_string()),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(result.action, "bulk_update");
        assert_eq!(result.count, 1, "stdout envelope still reports the match");
        assert_eq!(result.ids, vec![id]);
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("REVIEW")
                && notes[0].contains("'bogus'")
                && notes[0].contains("open, in-progress, done, wontfix"),
            "note must name the bad value and list valid statuses: {notes:?}"
        );
        assert_eq!(
            event_count(&conn, id),
            0,
            "kept-current field must not record an event"
        );
    }

    #[test]
    fn update_core_bogus_set_priority_emits_review_note() {
        let conn = open_test_db();
        let id = seed_tagged(&conn, "victim", "x");
        let (result, notes) = run_update_core(
            &conn,
            None,
            Some("bogus".to_string()),
            None,
            None,
            None,
            None,
            Some("x".to_string()),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(result.ids, vec![id]);
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("'bogus'") && notes[0].contains("critical, high, medium, low"),
            "note must name the bad value and list valid priorities: {notes:?}"
        );
        assert_eq!(db::get_issue(&conn, id).unwrap().priority, "medium");
    }

    #[test]
    fn update_core_partial_valid_input_applies_good_field() {
        // Soft-fallback philosophy: a bad --set-status must not block a
        // valid --set-priority in the same invocation.
        let conn = open_test_db();
        let id = seed_tagged(&conn, "victim", "x");
        let (_, notes) = run_update_core(
            &conn,
            Some("bogus".to_string()),
            Some("high".to_string()),
            None,
            None,
            None,
            None,
            Some("x".to_string()),
            None,
            None,
            false,
        )
        .unwrap();
        assert_eq!(notes.len(), 1, "only the bad field gets a note");
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.status, "open", "bad status kept current");
        assert_eq!(issue.priority, "high", "valid priority still applied");
    }

    #[test]
    fn update_core_valid_alias_still_applies_without_notes() {
        // Happy path guard: normalization of recognized aliases is unchanged.
        let conn = open_test_db();
        let id = seed_tagged(&conn, "victim", "x");
        let (result, notes) = run_update_core(
            &conn,
            Some("wip".to_string()),
            Some("urgent".to_string()),
            None,
            None,
            None,
            None,
            Some("x".to_string()),
            None,
            None,
            false,
        )
        .unwrap();
        assert!(
            notes.is_empty(),
            "no REVIEW notes on valid input: {notes:?}"
        );
        assert_eq!(result.count, 1);
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.status, "in-progress");
        assert_eq!(issue.priority, "critical");
    }

    // --- spec P2: bulk relate / depend / note ---

    fn event_fields(conn: &Connection, id: i64) -> Vec<String> {
        db::get_events_for_issue(conn, id)
            .unwrap()
            .into_iter()
            .map(|e| e.field)
            .collect()
    }

    #[test]
    fn bulk_depend_applies_edges_and_audits() {
        let conn = open_test_db();
        let blocker = seed_tagged(&conn, "blocker", "other");
        let a = seed_tagged(&conn, "a", "sprint-9");
        let b = seed_tagged(&conn, "b", "sprint-9");

        run_depend(
            &conn,
            blocker,
            None,
            None,
            None,
            Some("sprint-9".to_string()),
            None,
            None,
            false,
            Format::Compact,
        )
        .expect("bulk depend");

        for id in [a, b] {
            assert_eq!(db::get_blockers(&conn, id).unwrap(), vec![blocker]);
            assert!(
                event_fields(&conn, id).contains(&"dependency_added".to_string()),
                "bulk depend must appear in the audit log (#35)"
            );
        }
    }

    #[test]
    fn bulk_depend_dry_run_writes_nothing() {
        let conn = open_test_db();
        let blocker = seed_tagged(&conn, "blocker", "other");
        let a = seed_tagged(&conn, "a", "sprint-9");

        run_depend(
            &conn,
            blocker,
            None,
            None,
            None,
            Some("sprint-9".to_string()),
            None,
            None,
            true,
            Format::Compact,
        )
        .expect("dry run");

        assert!(
            db::get_blockers(&conn, a).unwrap().is_empty(),
            "dry-run must not write dependency edges"
        );
        assert!(
            event_fields(&conn, a).is_empty(),
            "dry-run must not write audit events"
        );
    }

    #[test]
    fn bulk_depend_skips_self_edge() {
        let conn = open_test_db();
        let blocker = seed_tagged(&conn, "blocker", "sprint-9");
        let a = seed_tagged(&conn, "a", "sprint-9");

        run_depend(
            &conn,
            blocker,
            None,
            None,
            None,
            Some("sprint-9".to_string()),
            None,
            None,
            false,
            Format::Compact,
        )
        .expect("bulk depend");

        assert_eq!(db::get_blockers(&conn, a).unwrap(), vec![blocker]);
        assert!(
            db::get_blockers(&conn, blocker).unwrap().is_empty(),
            "the --on issue matching the filter must not depend on itself"
        );
    }

    #[test]
    fn bulk_depend_cycle_is_hard_error_and_rolls_back() {
        let conn = open_test_db();
        let a = seed_tagged(&conn, "a", "x");
        let b = seed_tagged(&conn, "b", "grp");
        let c = seed_tagged(&conn, "c", "grp");
        // a depends on b; bulk-depending {b,c} on a would cycle at b.
        db::add_dependency(&conn, b, a).expect("edge");

        let err = run_depend(
            &conn,
            a,
            None,
            None,
            None,
            Some("grp".to_string()),
            None,
            None,
            false,
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::CycleDetected(_)));
        assert!(
            db::get_blockers(&conn, c).unwrap().is_empty(),
            "cycle failure must roll the whole bulk depend back"
        );
    }

    #[test]
    fn bulk_depend_requires_filters() {
        let conn = open_test_db();
        let blocker = seed_tagged(&conn, "blocker", "x");
        let err = run_depend(
            &conn,
            blocker,
            None,
            None,
            None,
            None,
            None,
            None,
            false,
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::NoFilters));
    }

    #[test]
    fn bulk_relate_applies_and_skips_self() {
        let conn = open_test_db();
        let target = seed_tagged(&conn, "target", "grp");
        let a = seed_tagged(&conn, "a", "grp");

        run_relate(
            &conn,
            target,
            "related",
            None,
            None,
            None,
            Some("grp".to_string()),
            None,
            None,
            false,
            Format::Compact,
        )
        .expect("bulk relate");

        let rels = db::get_relations(&conn, a).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!(rels[0].target_id, target);
        assert_eq!(
            db::get_relations(&conn, target).unwrap().len(),
            1,
            "only the a->target link exists; no self-relation on the target"
        );
    }

    #[test]
    fn bulk_relate_dry_run_writes_nothing() {
        let conn = open_test_db();
        let target = seed_tagged(&conn, "target", "other");
        let a = seed_tagged(&conn, "a", "grp");

        run_relate(
            &conn,
            target,
            "related",
            None,
            None,
            None,
            Some("grp".to_string()),
            None,
            None,
            true,
            Format::Compact,
        )
        .expect("dry run");

        assert!(db::get_relations(&conn, a).unwrap().is_empty());
        assert!(event_fields(&conn, a).is_empty());
    }

    #[test]
    fn bulk_relate_rejects_unknown_type() {
        let conn = open_test_db();
        seed_tagged(&conn, "a", "grp");
        let err = run_relate(
            &conn,
            1,
            "bogus",
            None,
            None,
            None,
            Some("grp".to_string()),
            None,
            None,
            false,
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { .. }));
    }

    #[test]
    fn bulk_note_applies_to_all_matches() {
        let conn = open_test_db();
        let a = seed_tagged(&conn, "a", "grp");
        let b = seed_tagged(&conn, "b", "grp");

        run_note(
            &conn,
            "wave 2 verified",
            "scrum",
            None,
            None,
            None,
            Some("grp".to_string()),
            None,
            None,
            false,
            Format::Compact,
        )
        .expect("bulk note");

        for id in [a, b] {
            let notes = db::get_notes(&conn, id).unwrap();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].content, "wave 2 verified");
            assert_eq!(notes[0].agent, "scrum");
        }
    }

    #[test]
    fn bulk_note_dry_run_writes_nothing() {
        let conn = open_test_db();
        let a = seed_tagged(&conn, "a", "grp");

        run_note(
            &conn,
            "planned note",
            "",
            None,
            None,
            None,
            Some("grp".to_string()),
            None,
            None,
            true,
            Format::Compact,
        )
        .expect("dry run");

        assert!(db::get_notes(&conn, a).unwrap().is_empty());
        assert!(event_fields(&conn, a).is_empty());
    }

    #[test]
    fn bulk_note_requires_text() {
        let conn = open_test_db();
        seed_tagged(&conn, "a", "grp");
        let err = run_note(
            &conn,
            "",
            "",
            None,
            None,
            None,
            Some("grp".to_string()),
            None,
            None,
            false,
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { ref field, .. } if field == "text"));
    }

    #[test]
    fn update_core_dry_run_still_reports_bogus_value() {
        let conn = open_test_db();
        let id = seed_tagged(&conn, "victim", "x");
        let (result, notes) = run_update_core(
            &conn,
            Some("bogus".to_string()),
            None,
            None,
            None,
            None,
            None,
            Some("x".to_string()),
            None,
            None,
            true,
        )
        .unwrap();
        assert!(result.dry_run);
        assert_eq!(
            notes.len(),
            1,
            "validation feedback also surfaces in dry-run"
        );
        assert_eq!(db::get_issue(&conn, id).unwrap().status, "open");
    }
}
