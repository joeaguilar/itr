use crate::commands::build_issue_detail;
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::BatchAddInput;
use crate::normalize::{self, validate_kind, validate_priority};
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;
use std::io::{self, Read};

#[allow(clippy::too_many_arguments)]
pub fn run(
    conn: &Connection,
    title: Option<String>,
    priority: &str,
    kind: &str,
    context: Option<String>,
    files: Option<String>,
    file: Vec<String>,
    tags: Option<String>,
    tag: Vec<String>,
    skills: Option<String>,
    skill: Vec<String>,
    acceptance: Option<String>,
    blocked_by: Option<String>,
    parent: Option<i64>,
    assigned_to: Option<String>,
    stdin_json: bool,
    fmt: Format,
) -> Result<(), ItrError> {
    let (
        title,
        priority,
        kind,
        context,
        files_vec,
        tags_vec,
        skills_vec,
        acceptance,
        parent_id,
        assigned_to,
        blocked_by_ids,
    ) = if stdin_json {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        let data: BatchAddInput = serde_json::from_str(&input)?;
        let blocked: Vec<i64> = data.blocked_by.iter().filter_map(|v| v.as_i64()).collect();
        (
            data.title,
            data.priority,
            data.kind,
            data.context,
            data.files,
            data.tags,
            data.skills
                .iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            data.acceptance,
            data.parent_id,
            data.assigned_to,
            blocked,
        )
    } else {
        let title = title.ok_or_else(|| ItrError::InvalidValue {
            field: "title".to_string(),
            value: String::new(),
            valid: "non-empty string".to_string(),
        })?;
        let mut files_vec: Vec<String> =
            files.as_deref().map(util::parse_comma_list).unwrap_or_default();
        files_vec.extend(file.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        let mut tags_vec: Vec<String> =
            tags.as_deref().map(util::parse_comma_list).unwrap_or_default();
        tags_vec.extend(tag.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()));
        let mut skills_vec: Vec<String> =
            skills.as_deref().map(util::parse_comma_list_lower).unwrap_or_default();
        skills_vec
            .extend(skill.into_iter().map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()));
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
            skills_vec,
            acceptance.unwrap_or_default(),
            parent,
            assigned_to.unwrap_or_default(),
            blocked_by_ids,
        )
    };

    let priority = normalize::normalize_priority(&priority);
    let kind = normalize::normalize_kind(&kind);

    let mut review_notes: Vec<String> = Vec::new();
    let mut tags_vec = tags_vec;

    let priority = match validate_priority(&priority) {
        Ok(()) => priority,
        Err(_) => {
            review_notes.push(format!(
                "REVIEW: priority '{}' not recognized, defaulted to 'medium'. Valid: critical, high, medium, low",
                priority
            ));
            "medium".to_string()
        }
    };
    let kind = match validate_kind(&kind) {
        Ok(()) => kind,
        Err(_) => {
            review_notes.push(format!(
                "REVIEW: kind '{}' not recognized, defaulted to 'task'. Valid: bug, feature, task, epic",
                kind
            ));
            "task".to_string()
        }
    };

    if !review_notes.is_empty() && !tags_vec.contains(&"_needs_review".to_string()) {
        tags_vec.push("_needs_review".to_string());
    }

    let issue = db::insert_issue(
        conn,
        &title,
        &priority,
        &kind,
        &context,
        &files_vec,
        &tags_vec,
        &skills_vec,
        &acceptance,
        parent_id,
        &assigned_to,
    )?;

    // Add review notes
    for note_text in &review_notes {
        db::add_note(conn, issue.id, note_text, "itr")?;
    }

    // Add dependencies
    for blocker_id in &blocked_by_ids {
        db::add_dependency(conn, *blocker_id, issue.id)?;
    }

    // Build detail for output
    let config = UrgencyConfig::load(conn);
    let detail = build_issue_detail(conn, issue, &config)?;
    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
}
