use crate::commands::add::{validate_kind, validate_priority, validate_status};
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::IssueDetail;
use crate::normalize;
use crate::urgency::{self, UrgencyConfig};
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

    let mut review_notes: Vec<String> = Vec::new();

    if let Some(ref s) = status {
        match validate_status(s) {
            Ok(()) => {
                db::record_event(conn, id, "status", &old_issue.status, s)?;
                db::update_issue_field(conn, id, "status", s)?;
            }
            Err(_) => {
                review_notes.push(format!(
                    "REVIEW: status '{}' not recognized, defaulted to 'open'. Valid: open, in-progress, done, wontfix",
                    s
                ));
                db::record_event(conn, id, "status", &old_issue.status, "open")?;
                db::update_issue_field(conn, id, "status", "open")?;
            }
        }
    }
    if let Some(ref p) = priority {
        match validate_priority(p) {
            Ok(()) => {
                db::record_event(conn, id, "priority", &old_issue.priority, p)?;
                db::update_issue_field(conn, id, "priority", p)?;
            }
            Err(_) => {
                review_notes.push(format!(
                    "REVIEW: priority '{}' not recognized, defaulted to 'medium'. Valid: critical, high, medium, low",
                    p
                ));
                db::record_event(conn, id, "priority", &old_issue.priority, "medium")?;
                db::update_issue_field(conn, id, "priority", "medium")?;
            }
        }
    }
    if let Some(ref k) = kind {
        match validate_kind(k) {
            Ok(()) => {
                db::record_event(conn, id, "kind", &old_issue.kind, k)?;
                db::update_issue_field(conn, id, "kind", k)?;
            }
            Err(_) => {
                review_notes.push(format!(
                    "REVIEW: kind '{}' not recognized, defaulted to 'task'. Valid: bug, feature, task, epic",
                    k
                ));
                db::record_event(conn, id, "kind", &old_issue.kind, "task")?;
                db::update_issue_field(conn, id, "kind", "task")?;
            }
        }
    }
    if let Some(ref t) = title {
        db::record_event(conn, id, "title", &old_issue.title, t)?;
        db::update_issue_field(conn, id, "title", t)?;
    }
    if let Some(ref c) = context {
        db::record_event(conn, id, "context", &old_issue.context, c)?;
        db::update_issue_field(conn, id, "context", c)?;
    }
    if let Some(ref a) = acceptance {
        db::record_event(conn, id, "acceptance", &old_issue.acceptance, a)?;
        db::update_issue_field(conn, id, "acceptance", a)?;
    }
    if let Some(ref a) = assigned_to {
        db::record_event(conn, id, "assigned_to", &old_issue.assigned_to, a)?;
        db::update_issue_field(conn, id, "assigned_to", a)?;
    }

    // Handle files
    if files.is_some() || !file.is_empty() {
        let mut file_list: Vec<String> = files
            .map(|f| {
                f.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        file_list.extend(file.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        let json = serde_json::to_string(&file_list)?;
        db::update_issue_field(conn, id, "files", &json)?;
    } else if !add_files.is_empty() || !remove_files.is_empty() {
        let current_issue = db::get_issue(conn, id)?;
        let mut current_files = current_issue.files.clone();
        for f in &add_files {
            if !current_files.contains(f) {
                current_files.push(f.clone());
            }
        }
        current_files.retain(|f| !remove_files.contains(f));
        let json = serde_json::to_string(&current_files)?;
        db::update_issue_field(conn, id, "files", &json)?;
    }

    // Handle tags
    if tags.is_some() || !tag.is_empty() {
        let mut tag_list: Vec<String> = tags
            .map(|t| {
                t.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        tag_list.extend(tag.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        let json = serde_json::to_string(&tag_list)?;
        db::update_issue_field(conn, id, "tags", &json)?;
    } else if !add_tags.is_empty() || !remove_tags.is_empty() {
        let current_issue = db::get_issue(conn, id)?;
        let mut current_tags = current_issue.tags.clone();
        for t in &add_tags {
            if !current_tags.contains(t) {
                current_tags.push(t.clone());
            }
        }
        current_tags.retain(|t| !remove_tags.contains(t));
        let json = serde_json::to_string(&current_tags)?;
        db::update_issue_field(conn, id, "tags", &json)?;
    }

    // Handle skills
    if skills.is_some() || !skill.is_empty() {
        let mut skill_list: Vec<String> = skills
            .map(|s| {
                s.split(',')
                    .map(|s| s.trim().to_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        skill_list
            .extend(skill.into_iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()));
        let json = serde_json::to_string(&skill_list)?;
        db::update_issue_field(conn, id, "skills", &json)?;
    } else if !add_skills.is_empty() || !remove_skills.is_empty() {
        let current_issue = db::get_issue(conn, id)?;
        let mut current_skills = current_issue.skills.clone();
        for s in &add_skills {
            let lowered = s.trim().to_lowercase();
            if !lowered.is_empty() && !current_skills.contains(&lowered) {
                current_skills.push(lowered);
            }
        }
        let remove_lowered: Vec<String> = remove_skills
            .iter()
            .map(|s| s.trim().to_lowercase())
            .collect();
        current_skills.retain(|s| !remove_lowered.contains(s));
        let json = serde_json::to_string(&current_skills)?;
        db::update_issue_field(conn, id, "skills", &json)?;
    }

    if let Some(pid) = parent {
        db::update_issue_parent(conn, id, Some(pid))?;
    }

    // Add _needs_review tag and notes if any field was auto-corrected
    if !review_notes.is_empty() {
        let current_issue = db::get_issue(conn, id)?;
        let mut current_tags = current_issue.tags.clone();
        if !current_tags.contains(&"_needs_review".to_string()) {
            current_tags.push("_needs_review".to_string());
            let json = serde_json::to_string(&current_tags)?;
            db::update_issue_field(conn, id, "tags", &json)?;
        }
        for note_text in &review_notes {
            db::add_note(conn, id, note_text, "itr")?;
        }
    }

    // Re-read the updated issue
    let issue = db::get_issue(conn, id)?;
    let config = UrgencyConfig::load(conn);
    let (urg, breakdown) = urgency::compute_urgency_with_breakdown(&issue, &config, conn);
    let blocked_by = db::get_blockers(conn, issue.id)?;
    let blocks = db::get_blocking(conn, issue.id)?;
    let is_blocked = db::is_blocked(conn, issue.id)?;
    let notes = db::get_notes(conn, issue.id)?;

    let detail = IssueDetail {
        issue: issue.clone(),
        urgency: urg,
        blocked_by,
        blocks,
        is_blocked,
        notes,
        urgency_breakdown: Some(breakdown),
        children: None,
        relations: vec![],
    };

    // Check for newly unblocked issues
    let unblocked = if let Some(ref s) = status {
        if s == "done" || s == "wontfix" {
            db::get_newly_unblocked(conn, id)?
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    match fmt {
        Format::Json => {
            let mut value = serde_json::to_value(&detail)?;
            if !unblocked.is_empty() {
                let list: Vec<serde_json::Value> = unblocked
                    .iter()
                    .map(|(uid, utitle)| serde_json::json!({"id": uid, "title": utitle}))
                    .collect();
                value["unblocked"] = serde_json::Value::Array(list);
            }
            format::println_json(&value.to_string());
        }
        _ => {
            println!("{}", format::format_issue_detail(&detail, fmt));
            if !unblocked.is_empty() {
                let unblocked_str = format::format_unblocked(&unblocked, fmt);
                if !unblocked_str.is_empty() {
                    println!("{}", unblocked_str);
                }
            }
        }
    }

    Ok(())
}
