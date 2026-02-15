use crate::commands::add::{validate_kind, validate_priority};
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{BatchAddInput, IssueDetail};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::io::{self, Read};

pub fn run_add(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let items: Vec<BatchAddInput> = serde_json::from_str(&input)?;

    // Validate all first
    for item in &items {
        validate_priority(&item.priority)?;
        validate_kind(&item.kind)?;
    }

    // Use a transaction
    let tx = conn.unchecked_transaction()?;

    let mut created_ids: Vec<i64> = Vec::new();

    // First pass: create all issues
    for item in &items {
        let issue = db::insert_issue(
            &tx,
            &item.title,
            &item.priority,
            &item.kind,
            &item.context,
            &item.files,
            &item.tags,
            &item.acceptance,
            item.parent_id,
        )?;
        created_ids.push(issue.id);
    }

    // Second pass: create dependencies
    for (idx, item) in items.iter().enumerate() {
        let blocked_id = created_ids[idx];
        for dep in &item.blocked_by {
            let blocker_id = if let Some(s) = dep.as_str() {
                if let Some(stripped) = s.strip_prefix('@') {
                    let batch_idx: usize = stripped.parse().map_err(|_| ItrError::InvalidValue {
                        field: "blocked_by".to_string(),
                        value: s.to_string(),
                        valid: "@N where N is a batch index".to_string(),
                    })?;
                    if batch_idx >= created_ids.len() {
                        return Err(ItrError::InvalidValue {
                            field: "blocked_by".to_string(),
                            value: s.to_string(),
                            valid: format!("@0 to @{}", created_ids.len() - 1),
                        });
                    }
                    created_ids[batch_idx]
                } else {
                    s.parse::<i64>().map_err(|_| ItrError::InvalidValue {
                        field: "blocked_by".to_string(),
                        value: s.to_string(),
                        valid: "integer ID or @N batch reference".to_string(),
                    })?
                }
            } else if let Some(n) = dep.as_i64() {
                n
            } else {
                return Err(ItrError::InvalidValue {
                    field: "blocked_by".to_string(),
                    value: dep.to_string(),
                    valid: "integer ID or @N batch reference".to_string(),
                });
            };
            db::add_dependency(&tx, blocker_id, blocked_id)?;
        }
    }

    tx.commit()?;

    // Output created issues
    let config = UrgencyConfig::load(conn);
    let mut details: Vec<IssueDetail> = Vec::new();
    for id in &created_ids {
        let issue = db::get_issue(conn, *id)?;
        let (urg, breakdown) = urgency::compute_urgency_with_breakdown(&issue, &config, conn);
        let blocked_by = db::get_blockers(conn, issue.id)?;
        let blocks = db::get_blocking(conn, issue.id)?;
        let is_blocked = db::is_blocked(conn, issue.id)?;
        let notes = db::get_notes(conn, issue.id)?;

        details.push(IssueDetail {
            issue,
            urgency: urg,
            blocked_by,
            blocks,
            is_blocked,
            notes,
            urgency_breakdown: Some(breakdown),
            children: None,
        });
    }

    match fmt {
        Format::Json => {
            println!("{}", serde_json::to_string(&details)?);
        }
        _ => {
            for detail in &details {
                println!("{}", format::format_issue_detail(detail, fmt));
                println!();
            }
        }
    }

    Ok(())
}
