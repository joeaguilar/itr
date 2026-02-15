use crate::commands::add::{validate_kind, validate_priority, validate_status};
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::IssueDetail;
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
    tags: Option<String>,
    acceptance: Option<String>,
    parent: Option<i64>,
    add_tags: Vec<String>,
    remove_tags: Vec<String>,
    add_files: Vec<String>,
    remove_files: Vec<String>,
    fmt: Format,
) -> Result<(), ItrError> {
    // Validate issue exists
    let _issue = db::get_issue(conn, id)?;

    if let Some(ref s) = status {
        validate_status(s)?;
        db::update_issue_field(conn, id, "status", s)?;
    }
    if let Some(ref p) = priority {
        validate_priority(p)?;
        db::update_issue_field(conn, id, "priority", p)?;
    }
    if let Some(ref k) = kind {
        validate_kind(k)?;
        db::update_issue_field(conn, id, "kind", k)?;
    }
    if let Some(ref t) = title {
        db::update_issue_field(conn, id, "title", t)?;
    }
    if let Some(ref c) = context {
        db::update_issue_field(conn, id, "context", c)?;
    }
    if let Some(ref a) = acceptance {
        db::update_issue_field(conn, id, "acceptance", a)?;
    }

    // Handle files
    if let Some(ref f) = files {
        let file_list: Vec<String> = f.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
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
    if let Some(ref t) = tags {
        let tag_list: Vec<String> = t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
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

    if let Some(pid) = parent {
        db::update_issue_parent(conn, id, Some(pid))?;
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
            println!("{}", value);
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
