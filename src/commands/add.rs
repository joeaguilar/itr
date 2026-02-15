use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{BatchAddInput, IssueDetail};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::io::{self, Read};

pub fn run(
    conn: &Connection,
    title: Option<String>,
    priority: &str,
    kind: &str,
    context: Option<String>,
    files: Option<String>,
    tags: Option<String>,
    acceptance: Option<String>,
    blocked_by: Option<String>,
    parent: Option<i64>,
    stdin_json: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let (title, priority, kind, context, files_vec, tags_vec, acceptance, parent_id, blocked_by_ids) =
        if stdin_json {
            let mut input = String::new();
            io::stdin().read_to_string(&mut input)?;
            let data: BatchAddInput = serde_json::from_str(&input)?;
            let blocked: Vec<i64> = data
                .blocked_by
                .iter()
                .filter_map(|v| v.as_i64())
                .collect();
            (
                data.title,
                data.priority,
                data.kind,
                data.context,
                data.files,
                data.tags,
                data.acceptance,
                data.parent_id,
                blocked,
            )
        } else {
            let title = title.ok_or_else(|| ItrError::InvalidValue {
                field: "title".to_string(),
                value: String::new(),
                valid: "non-empty string".to_string(),
            })?;
            let files_vec: Vec<String> = files
                .map(|f| f.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default();
            let tags_vec: Vec<String> = tags
                .map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                .unwrap_or_default();
            let blocked_by_ids: Vec<i64> = blocked_by
                .map(|b| {
                    b.split(',')
                        .filter_map(|s| s.trim().parse::<i64>().ok())
                        .collect()
                })
                .unwrap_or_default();
            (
                title,
                priority.to_string(),
                kind.to_string(),
                context.unwrap_or_default(),
                files_vec,
                tags_vec,
                acceptance.unwrap_or_default(),
                parent,
                blocked_by_ids,
            )
        };

    validate_priority(&priority)?;
    validate_kind(&kind)?;

    let issue = db::insert_issue(
        conn,
        &title,
        &priority,
        &kind,
        &context,
        &files_vec,
        &tags_vec,
        &acceptance,
        parent_id,
    )?;

    // Add dependencies
    for blocker_id in &blocked_by_ids {
        db::add_dependency(conn, *blocker_id, issue.id)?;
    }

    // Build detail for output
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
    };

    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
}

pub fn validate_priority(p: &str) -> Result<(), ItrError> {
    match p {
        "critical" | "high" | "medium" | "low" => Ok(()),
        _ => Err(ItrError::InvalidValue {
            field: "priority".to_string(),
            value: p.to_string(),
            valid: "critical, high, medium, low".to_string(),
        }),
    }
}

pub fn validate_kind(k: &str) -> Result<(), ItrError> {
    match k {
        "bug" | "feature" | "task" | "epic" => Ok(()),
        _ => Err(ItrError::InvalidValue {
            field: "kind".to_string(),
            value: k.to_string(),
            valid: "bug, feature, task, epic".to_string(),
        }),
    }
}

pub fn validate_status(s: &str) -> Result<(), ItrError> {
    match s {
        "open" | "in-progress" | "done" | "wontfix" => Ok(()),
        _ => Err(ItrError::InvalidValue {
            field: "status".to_string(),
            value: s.to_string(),
            valid: "open, in-progress, done, wontfix".to_string(),
        }),
    }
}
