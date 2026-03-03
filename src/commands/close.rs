use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::IssueDetail;
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::io::{self, IsTerminal, Read};

pub fn run(
    conn: &Connection,
    id: i64,
    reason: Option<String>,
    wontfix: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    // Read reason from stdin if not provided and stdin is not a TTY
    let reason = match reason {
        Some(r) => r,
        None => {
            if io::stdin().is_terminal() {
                String::new()
            } else {
                let mut buf = String::new();
                io::stdin().read_to_string(&mut buf)?;
                buf.trim().to_string()
            }
        }
    };

    let status = if wontfix { "wontfix" } else { "done" };

    // Capture old values for event recording
    let old_issue = db::get_issue(conn, id)?;

    db::record_event(conn, id, "status", &old_issue.status, status)?;
    db::update_issue_field(conn, id, "status", status)?;
    if !reason.is_empty() {
        db::record_event(conn, id, "close_reason", &old_issue.close_reason, &reason)?;
        db::update_issue_field(conn, id, "close_reason", &reason)?;
    }

    // Output updated issue
    let issue = db::get_issue(conn, id)?;
    let config = UrgencyConfig::load(conn);
    let (urg, breakdown) = urgency::compute_urgency_with_breakdown(&issue, &config, conn);
    let blocked_by = db::get_blockers(conn, issue.id)?;
    let blocks = db::get_blocking(conn, issue.id)?;
    let is_blocked = db::is_blocked(conn, issue.id)?;
    let notes = db::get_notes(conn, issue.id)?;

    let detail = IssueDetail {
        issue,
        urgency: urg,
        blocked_by,
        blocks,
        is_blocked,
        notes,
        urgency_breakdown: Some(breakdown),
        children: None,
        relations: vec![],
    };

    // Get unblocked issues
    let unblocked = db::get_newly_unblocked(conn, id)?;

    match fmt {
        Format::Json => {
            // Combine detail + unblocked into single JSON object
            let mut value = serde_json::to_value(&detail)?;
            let unblocked_list: Vec<serde_json::Value> = unblocked
                .iter()
                .map(|(uid, utitle)| serde_json::json!({"id": uid, "title": utitle}))
                .collect();
            value["unblocked"] = serde_json::Value::Array(unblocked_list);
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
