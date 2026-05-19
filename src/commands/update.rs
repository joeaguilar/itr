use crate::commands::{build_issue_detail, print_detail_with_unblocked};
use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::normalize;
use crate::normalize::{validate_kind, validate_priority, validate_status};
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;

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
                review_notes.push(format!(
                    "REVIEW: status '{}' not recognized, defaulted to 'open'. Valid: open, in-progress, done, wontfix",
                    s
                ));
                db::record_event(&tx, id, "status", &old_issue.status, "open")?;
                db::update_issue_field(&tx, id, "status", "open")?;
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

    // Handle files
    if files.is_some() || !file.is_empty() {
        let mut file_list: Vec<String> = files
            .as_deref()
            .map(util::parse_comma_list)
            .unwrap_or_default();
        file_list.extend(
            file.into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
        let json = serde_json::to_string(&file_list)?;
        db::update_issue_field(&tx, id, "files", &json)?;
    } else if !add_files.is_empty() || !remove_files.is_empty() {
        let current = db::get_issue(&tx, id)?;
        let updated = util::apply_tags(current.files, &add_files, &remove_files);
        let json = serde_json::to_string(&updated)?;
        db::update_issue_field(&tx, id, "files", &json)?;
    }

    // Handle tags
    if tags.is_some() || !tag.is_empty() {
        let mut tag_list: Vec<String> = tags
            .as_deref()
            .map(util::parse_comma_list)
            .unwrap_or_default();
        tag_list.extend(
            tag.into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
        let json = serde_json::to_string(&tag_list)?;
        db::update_issue_field(&tx, id, "tags", &json)?;
    } else if !add_tags.is_empty() || !remove_tags.is_empty() {
        let current = db::get_issue(&tx, id)?;
        let updated = util::apply_tags(current.tags, &add_tags, &remove_tags);
        let json = serde_json::to_string(&updated)?;
        db::update_issue_field(&tx, id, "tags", &json)?;
    }

    // Handle skills
    if skills.is_some() || !skill.is_empty() {
        let mut skill_list: Vec<String> = skills
            .as_deref()
            .map(util::parse_comma_list_lower)
            .unwrap_or_default();
        skill_list.extend(
            skill
                .into_iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty()),
        );
        let json = serde_json::to_string(&skill_list)?;
        db::update_issue_field(&tx, id, "skills", &json)?;
    } else if !add_skills.is_empty() || !remove_skills.is_empty() {
        let current = db::get_issue(&tx, id)?;
        let updated = util::apply_skills(current.skills, &add_skills, &remove_skills);
        let json = serde_json::to_string(&updated)?;
        db::update_issue_field(&tx, id, "skills", &json)?;
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

    // Add _needs_review tag and notes if any field was auto-corrected
    if !review_notes.is_empty() {
        let current_issue = db::get_issue(&tx, id)?;
        let mut current_tags = current_issue.tags.clone();
        if !current_tags.contains(&"_needs_review".to_string()) {
            current_tags.push("_needs_review".to_string());
            let json = serde_json::to_string(&current_tags)?;
            db::update_issue_field(&tx, id, "tags", &json)?;
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
    print_detail_with_unblocked(&detail, &unblocked, fmt);

    Ok(())
}
