use crate::commands::{build_issue_detail, print_detail_with_unblocked};
use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::IssueDetail;
use crate::normalize;
use crate::normalize::{validate_kind, validate_priority, validate_status};
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;

/// Field changes for one `itr update` invocation. Mirrors the CLI flags so
/// the testable core (`run_core`) can be driven from unit tests without
/// threading two dozen positional arguments.
#[derive(Debug, Default)]
pub(crate) struct UpdateRequest {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub kind: Option<String>,
    pub title: Option<String>,
    pub context: Option<String>,
    pub files: Option<String>,
    pub file: Vec<String>,
    pub tags: Option<String>,
    pub tag: Vec<String>,
    pub skills: Option<String>,
    pub skill: Vec<String>,
    pub acceptance: Option<String>,
    pub parent: Option<i64>,
    pub no_parent: bool,
    pub assigned_to: Option<String>,
    pub add_tags: Vec<String>,
    pub remove_tags: Vec<String>,
    pub add_files: Vec<String>,
    pub remove_files: Vec<String>,
    pub add_skills: Vec<String>,
    pub remove_skills: Vec<String>,
}

/// Persist a new value for a JSON-array list column (`files`/`tags`/`skills`)
/// and record an audit event, skipping both when the list is unchanged. The
/// event old/new values are the JSON-array encodings, matching the format
/// `bulk update` records (#187). Shared with `batch update`.
pub(crate) fn persist_list_field(
    tx: &Connection,
    id: i64,
    field: &str,
    old: &[String],
    new: &[String],
) -> Result<(), ItrError> {
    let old_json = serde_json::to_string(old)?;
    let new_json = serde_json::to_string(new)?;
    if old_json != new_json {
        db::record_event(tx, id, field, &old_json, &new_json)?;
        db::update_issue_field(tx, id, field, &new_json)?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn run(
    conn: &Connection,
    id: i64,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    title: Option<String>,
    context: Option<String>,
    files: Option<String>,
    file: Vec<String>,
    tags: Option<String>,
    tag: Vec<String>,
    skills: Option<String>,
    skill: Vec<String>,
    acceptance: Option<String>,
    parent: Option<i64>,
    no_parent: bool,
    assigned_to: Option<String>,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
    add_files: Vec<String>,
    remove_files: Vec<String>,
    add_skills: Vec<String>,
    remove_skills: Vec<String>,
    fmt: Format,
) -> Result<(), ItrError> {
    let (detail, unblocked) = run_core(
        conn,
        id,
        UpdateRequest {
            status,
            priority,
            kind,
            title,
            context,
            files,
            file,
            tags,
            tag,
            skills,
            skill,
            acceptance,
            parent,
            no_parent,
            assigned_to,
            add_tags,
            remove_tags,
            add_files,
            remove_files,
            add_skills,
            remove_skills,
        },
    )?;
    print_detail_with_unblocked(&detail, &unblocked, fmt);
    Ok(())
}

pub(crate) fn run_core(
    conn: &Connection,
    id: i64,
    req: UpdateRequest,
) -> Result<(IssueDetail, Vec<(i64, String)>), ItrError> {
    let UpdateRequest {
        status,
        priority,
        kind,
        title,
        context,
        files,
        file,
        tags,
        tag,
        skills,
        skill,
        acceptance,
        parent,
        no_parent,
        assigned_to,
        add_tags,
        remove_tags,
        add_files,
        remove_files,
        add_skills,
        remove_skills,
    } = req;

    // Capture old values for event recording
    let old_issue = db::get_issue(conn, id)?;

    let status = status.map(|s| normalize::normalize_status(&s));
    let priority = priority.map(|p| normalize::normalize_priority(&p));
    let kind = kind.map(|k| normalize::normalize_kind(&k));

    let tx = conn.unchecked_transaction()?;
    let mut review_notes: Vec<String> = Vec::new();
    let mut terminal_status_applied = false;

    if let Some(ref s) = status {
        match validate_status(s) {
            Ok(()) => {
                db::record_event(&tx, id, "status", &old_issue.status, s)?;
                db::update_issue_field(&tx, id, "status", s)?;
                terminal_status_applied = s == "done" || s == "wontfix";
            }
            Err(_) => {
                // Soft fallback (#163): keep the current status instead of
                // force-reopening — a typo must not mutate workflow state the
                // caller never asked to change. Matches `batch update`.
                review_notes.push(format!(
                    "REVIEW: status '{}' not recognized, kept '{}'. Valid: open, in-progress, done, wontfix",
                    s, old_issue.status
                ));
            }
        }
    }
    if let Some(ref p) = priority {
        match validate_priority(p) {
            Ok(()) => {
                db::record_event(&tx, id, "priority", &old_issue.priority, p)?;
                db::update_issue_field(&tx, id, "priority", p)?;
            }
            Err(_) => {
                review_notes.push(format!(
                    "REVIEW: priority '{}' not recognized, defaulted to 'medium'. Valid: critical, high, medium, low",
                    p
                ));
                db::record_event(&tx, id, "priority", &old_issue.priority, "medium")?;
                db::update_issue_field(&tx, id, "priority", "medium")?;
            }
        }
    }
    if let Some(ref k) = kind {
        match validate_kind(k) {
            Ok(()) => {
                db::record_event(&tx, id, "kind", &old_issue.kind, k)?;
                db::update_issue_field(&tx, id, "kind", k)?;
            }
            Err(_) => {
                review_notes.push(format!(
                    "REVIEW: kind '{}' not recognized, defaulted to 'task'. Valid: bug, feature, task, epic",
                    k
                ));
                db::record_event(&tx, id, "kind", &old_issue.kind, "task")?;
                db::update_issue_field(&tx, id, "kind", "task")?;
            }
        }
    }
    if let Some(ref t) = title {
        db::record_event(&tx, id, "title", &old_issue.title, t)?;
        db::update_issue_field(&tx, id, "title", t)?;
    }
    if let Some(ref c) = context {
        db::record_event(&tx, id, "context", &old_issue.context, c)?;
        db::update_issue_field(&tx, id, "context", c)?;
    }
    if let Some(ref a) = acceptance {
        db::record_event(&tx, id, "acceptance", &old_issue.acceptance, a)?;
        db::update_issue_field(&tx, id, "acceptance", a)?;
    }
    if let Some(ref a) = assigned_to {
        db::record_event(&tx, id, "assigned_to", &old_issue.assigned_to, a)?;
        db::update_issue_field(&tx, id, "assigned_to", a)?;
    }

    // List fields (files/tags/skills). The replace form is applied first;
    // add/remove edits then apply on top of the replacement instead of being
    // silently discarded (#188), with a REVIEW warning on stderr. Changes are
    // persisted with an audit event in JSON-array format (#187).

    // Handle files
    let replace_files = files.is_some() || !file.is_empty();
    let edit_files = !add_files.is_empty() || !remove_files.is_empty();
    if replace_files || edit_files {
        let current = db::get_issue(&tx, id)?.files;
        let mut updated = if replace_files {
            let mut list: Vec<String> = files
                .as_deref()
                .map(util::parse_comma_list)
                .unwrap_or_default();
            list.extend(
                file.into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );
            list
        } else {
            current.clone()
        };
        if edit_files {
            if replace_files {
                eprintln!(
                    "REVIEW: --files/--file replaces the file list; --add-file/--remove-file applied on top of the replacement"
                );
            }
            updated = util::apply_tags(updated, &add_files, &remove_files);
        }
        persist_list_field(&tx, id, "files", &current, &updated)?;
    }

    // Handle tags
    let replace_tags = tags.is_some() || !tag.is_empty();
    let edit_tags = !add_tags.is_empty() || !remove_tags.is_empty();
    if replace_tags || edit_tags {
        let current = db::get_issue(&tx, id)?.tags;
        let mut updated = if replace_tags {
            let mut list: Vec<String> = tags
                .as_deref()
                .map(util::parse_comma_list)
                .unwrap_or_default();
            list.extend(
                tag.into_iter()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );
            list
        } else {
            current.clone()
        };
        if edit_tags {
            if replace_tags {
                eprintln!(
                    "REVIEW: --tags/--tag replaces the tag list; --add-tag/--remove-tag applied on top of the replacement"
                );
            }
            updated = util::apply_tags(updated, &add_tags, &remove_tags);
        }
        persist_list_field(&tx, id, "tags", &current, &updated)?;
    }

    // Handle skills
    let replace_skills = skills.is_some() || !skill.is_empty();
    let edit_skills = !add_skills.is_empty() || !remove_skills.is_empty();
    if replace_skills || edit_skills {
        let current = db::get_issue(&tx, id)?.skills;
        let mut updated = if replace_skills {
            let mut list: Vec<String> = skills
                .as_deref()
                .map(util::parse_comma_list_lower)
                .unwrap_or_default();
            list.extend(
                skill
                    .into_iter()
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty()),
            );
            list
        } else {
            current.clone()
        };
        if edit_skills {
            if replace_skills {
                eprintln!(
                    "REVIEW: --skills/--skill replaces the skill list; --add-skill/--remove-skill applied on top of the replacement"
                );
            }
            updated = util::apply_skills(updated, &add_skills, &remove_skills);
        }
        persist_list_field(&tx, id, "skills", &current, &updated)?;
    }

    // Mutually exclusive flags. clap enforces this via `conflicts_with`, but
    // we keep a defensive soft-fallback in case clap is bypassed by future
    // callers (e.g. programmatic construction of the `Update` variant).
    if parent.is_some() && no_parent {
        return Err(ItrError::InvalidValue {
            field: "parent".to_string(),
            value: "<both --parent and --no-parent set>".to_string(),
            valid: "use one of --parent <ID> or --no-parent".to_string(),
        });
    }
    if let Some(pid) = parent {
        // Reject missing parent before any partial write.
        if !db::issue_exists(&tx, pid)? {
            return Err(ItrError::NotFound(pid));
        }
        // Cycle check: parent must not be self or any descendant of `id`.
        if db::is_self_or_descendant(&tx, id, pid)? {
            return Err(ItrError::CycleDetected(format!(
                "parent_id: {} cannot be parent of {} (creates cycle)",
                pid, id
            )));
        }
        let old_value = old_issue
            .parent_id
            .map(|p| p.to_string())
            .unwrap_or_default();
        let new_value = pid.to_string();
        if old_value != new_value {
            db::record_event(&tx, id, "parent_id", &old_value, &new_value)?;
            db::update_issue_parent(&tx, id, Some(pid))?;
        }
    } else if no_parent {
        let old_value = old_issue
            .parent_id
            .map(|p| p.to_string())
            .unwrap_or_default();
        if !old_value.is_empty() {
            db::record_event(&tx, id, "parent_id", &old_value, "")?;
            db::update_issue_parent(&tx, id, None)?;
        }
    }

    // Add _needs_review tag and notes if any field was auto-corrected. The
    // auto-added tag is an edit like any other, so it records a tags event
    // too (#187).
    if !review_notes.is_empty() {
        let current_tags = db::get_issue(&tx, id)?.tags;
        if !current_tags.contains(&"_needs_review".to_string()) {
            let mut new_tags = current_tags.clone();
            new_tags.push("_needs_review".to_string());
            persist_list_field(&tx, id, "tags", &current_tags, &new_tags)?;
        }
        for note_text in &review_notes {
            db::add_note(&tx, id, note_text, "itr")?;
        }
    }

    // Re-read the updated issue
    let issue = db::get_issue(&tx, id)?;
    let config = UrgencyConfig::load(&tx);
    let detail = build_issue_detail(&tx, issue, &config)?;

    // Check for newly unblocked issues
    let unblocked = if terminal_status_applied {
        let unblocked = db::get_newly_unblocked(&tx, id)?;
        db::remove_blocker_edges(&tx, id)?;
        unblocked
    } else {
        vec![]
    };

    tx.commit()?;

    Ok((detail, unblocked))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_test_db;
    use crate::models::Event;

    fn seed(conn: &Connection, title: &str) -> i64 {
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

    fn update(conn: &Connection, id: i64, req: UpdateRequest) {
        run_core(conn, id, req).unwrap();
    }

    fn events_for(conn: &Connection, id: i64, field: &str) -> Vec<Event> {
        db::get_events_for_issue(conn, id)
            .unwrap()
            .into_iter()
            .filter(|e| e.field == field)
            .collect()
    }

    fn note_contents(conn: &Connection, id: i64) -> Vec<String> {
        db::get_notes(conn, id)
            .unwrap()
            .into_iter()
            .map(|n| n.content)
            .collect()
    }

    // --- #163: unrecognized --status keeps the current status ---

    #[test]
    fn unrecognized_status_keeps_current_status_with_review() {
        let conn = open_test_db();
        let id = seed(&conn, "done work");
        update(
            &conn,
            id,
            UpdateRequest {
                status: Some("done".to_string()),
                ..Default::default()
            },
        );

        // The bug: this used to force-write 'open', silently reopening the
        // issue. It must keep 'done' and flag the input for review instead.
        update(
            &conn,
            id,
            UpdateRequest {
                status: Some("blocked".to_string()),
                ..Default::default()
            },
        );

        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.status, "done", "unrecognized status must not reopen");
        assert!(issue.tags.contains(&"_needs_review".to_string()));
        assert!(
            note_contents(&conn, id)
                .iter()
                .any(|n| n.contains("status 'blocked' not recognized, kept 'done'")),
            "REVIEW note must say which value was kept"
        );
        // No phantom status event: the only status transition ever recorded
        // is the legitimate open -> done.
        let status_events = events_for(&conn, id, "status");
        assert_eq!(status_events.len(), 1);
        assert_eq!(status_events[0].new_value, "done");
    }

    // --- #187: list-field changes record audit events ---

    #[test]
    fn add_tag_records_tags_event_in_json_array_format() {
        let conn = open_test_db();
        let id = seed(&conn, "tagged");
        update(
            &conn,
            id,
            UpdateRequest {
                add_tags: vec!["urgent".to_string()],
                ..Default::default()
            },
        );
        let events = events_for(&conn, id, "tags");
        assert_eq!(events.len(), 1, "tag add must be audited");
        assert_eq!(events[0].old_value, "[]");
        assert_eq!(events[0].new_value, r#"["urgent"]"#);
    }

    #[test]
    fn replace_files_records_files_event() {
        let conn = open_test_db();
        let id = seed(&conn, "filed");
        update(
            &conn,
            id,
            UpdateRequest {
                files: Some("a.rs,b.rs".to_string()),
                ..Default::default()
            },
        );
        let events = events_for(&conn, id, "files");
        assert_eq!(events.len(), 1, "files replace must be audited");
        assert_eq!(events[0].old_value, "[]");
        assert_eq!(events[0].new_value, r#"["a.rs","b.rs"]"#);
    }

    #[test]
    fn add_skill_records_skills_event() {
        let conn = open_test_db();
        let id = seed(&conn, "skilled");
        update(
            &conn,
            id,
            UpdateRequest {
                add_skills: vec!["SQL".to_string()],
                ..Default::default()
            },
        );
        let events = events_for(&conn, id, "skills");
        assert_eq!(events.len(), 1, "skill add must be audited");
        assert_eq!(events[0].old_value, "[]");
        assert_eq!(events[0].new_value, r#"["sql"]"#);
    }

    #[test]
    fn noop_list_replace_records_no_event() {
        let conn = open_test_db();
        let id = seed(&conn, "stable");
        update(
            &conn,
            id,
            UpdateRequest {
                tags: Some("x".to_string()),
                ..Default::default()
            },
        );
        update(
            &conn,
            id,
            UpdateRequest {
                tags: Some("x".to_string()),
                ..Default::default()
            },
        );
        assert_eq!(
            events_for(&conn, id, "tags").len(),
            1,
            "an unchanged list must not produce a second event"
        );
    }

    #[test]
    fn auto_needs_review_tag_records_tags_event() {
        let conn = open_test_db();
        let id = seed(&conn, "reviewed");
        update(
            &conn,
            id,
            UpdateRequest {
                status: Some("bogus".to_string()),
                ..Default::default()
            },
        );
        let events = events_for(&conn, id, "tags");
        assert_eq!(events.len(), 1, "auto-added _needs_review must be audited");
        assert_eq!(events[0].new_value, r#"["_needs_review"]"#);
    }

    // --- #188: replace-form + add/remove-form flags combine instead of
    // silently dropping the add/remove edits ---

    #[test]
    fn replace_tags_and_add_tag_both_apply() {
        let conn = open_test_db();
        let id = seed(&conn, "combo tags");
        update(
            &conn,
            id,
            UpdateRequest {
                tags: Some("x,y".to_string()),
                add_tags: vec!["z".to_string()],
                ..Default::default()
            },
        );
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(
            issue.tags,
            vec!["x".to_string(), "y".to_string(), "z".to_string()],
            "--add-tag must apply on top of the --tags replacement"
        );
        let events = events_for(&conn, id, "tags");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].new_value, r#"["x","y","z"]"#);
    }

    #[test]
    fn replace_files_and_remove_file_both_apply() {
        let conn = open_test_db();
        let id = seed(&conn, "combo files");
        update(
            &conn,
            id,
            UpdateRequest {
                files: Some("a.rs,b.rs".to_string()),
                remove_files: vec!["a.rs".to_string()],
                ..Default::default()
            },
        );
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.files, vec!["b.rs".to_string()]);
    }

    #[test]
    fn replace_skills_and_add_skill_both_apply() {
        let conn = open_test_db();
        let id = seed(&conn, "combo skills");
        update(
            &conn,
            id,
            UpdateRequest {
                skills: Some("rust".to_string()),
                add_skills: vec!["sql".to_string()],
                ..Default::default()
            },
        );
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.skills, vec!["rust".to_string(), "sql".to_string()]);
    }
}
