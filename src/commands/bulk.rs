use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::{BulkResult, UnblockedIssue};
use crate::normalize;
use rusqlite::Connection;

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
    // At least one filter required
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
        &statuses,
        &priorities,
        &kinds,
        &tags,
        false,
        true,
        None,
        false,
        &skills,
        assigned_to.as_deref(),
    )?;

    let ids: Vec<i64> = issues.iter().map(|i| i.id).collect();
    let close_status = if wontfix { "wontfix" } else { "done" };
    let reason = reason.unwrap_or_default();

    let mut all_unblocked = Vec::new();

    if !dry_run {
        let tx = conn.unchecked_transaction()?;
        for id in &ids {
            db::update_issue_field(&tx, *id, "status", close_status)?;
            if !reason.is_empty() {
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
    // At least one filter required
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
        &statuses,
        &priorities,
        &kinds,
        &tags,
        false,
        true,
        None,
        false,
        &skills,
        assigned_to.as_deref(),
    )?;

    let ids: Vec<i64> = issues.iter().map(|i| i.id).collect();

    if !dry_run {
        let tx = conn.unchecked_transaction()?;
        for id in &ids {
            if let Some(ref s) = set_status {
                let s = normalize::normalize_status(s);
                db::update_issue_field(&tx, *id, "status", &s)?;
            }
            if let Some(ref p) = set_priority {
                let p = normalize::normalize_priority(p);
                db::update_issue_field(&tx, *id, "priority", &p)?;
            }
            if let Some(ref new_tag) = add_tag {
                let issue = db::get_issue(&tx, *id)?;
                let mut current_tags = issue.tags.clone();
                if !current_tags.contains(new_tag) {
                    current_tags.push(new_tag.clone());
                    let json = serde_json::to_string(&current_tags)?;
                    db::update_issue_field(&tx, *id, "tags", &json)?;
                }
            }
        }
        tx.commit()?;
    }

    let result = BulkResult {
        action: "bulk_update".to_string(),
        count: ids.len(),
        ids,
        unblocked: vec![],
        dry_run,
    };

    print_result(&result, fmt);
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
                    .map(|i| i.to_string())
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
