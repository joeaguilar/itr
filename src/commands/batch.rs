use crate::normalize::{validate_kind, validate_priority, validate_status};
use crate::commands::build_issue_detail;
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{
    BatchAddInput, BatchCloseInput, BatchItemResult, BatchNoteInput, BatchResult, BatchSummary,
    BatchUpdateInput, UnblockedIssue,
};
use crate::normalize;
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;
use std::io::{self, Read};

pub fn run_add(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let items: Vec<BatchAddInput> = serde_json::from_str(&input)?;

    // Normalize all first
    let mut items = items;
    for item in &mut items {
        item.priority = normalize::normalize_priority(&item.priority);
        item.kind = normalize::normalize_kind(&item.kind);
    }

    // Use a transaction
    let tx = conn.unchecked_transaction()?;

    let mut created_ids: Vec<i64> = Vec::new();
    // Track which items need review notes (index -> notes)
    let mut item_review_notes: Vec<Vec<String>> = Vec::new();

    // First pass: create all issues with soft fallback
    for item in &mut items {
        let mut review_notes: Vec<String> = Vec::new();

        if validate_priority(&item.priority).is_err() {
            review_notes.push(format!(
                "REVIEW: priority '{}' not recognized, defaulted to 'medium'. Valid: critical, high, medium, low",
                item.priority
            ));
            item.priority = "medium".to_string();
        }
        if validate_kind(&item.kind).is_err() {
            review_notes.push(format!(
                "REVIEW: kind '{}' not recognized, defaulted to 'task'. Valid: bug, feature, task, epic",
                item.kind
            ));
            item.kind = "task".to_string();
        }

        let mut tags = item.tags.clone();
        if !review_notes.is_empty() && !tags.contains(&"_needs_review".to_string()) {
            tags.push("_needs_review".to_string());
        }

        let skills: Vec<String> = item
            .skills
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();
        let issue = db::insert_issue(
            &tx,
            &item.title,
            &item.priority,
            &item.kind,
            &item.context,
            &item.files,
            &tags,
            &skills,
            &item.acceptance,
            item.parent_id,
            &item.assigned_to,
        )?;
        created_ids.push(issue.id);
        item_review_notes.push(review_notes);
    }

    // Add review notes for items that needed them
    for (idx, notes) in item_review_notes.iter().enumerate() {
        for note_text in notes {
            db::add_note(&tx, created_ids[idx], note_text, "itr")?;
        }
    }

    // Second pass: create dependencies
    for (idx, item) in items.iter().enumerate() {
        let blocked_id = created_ids[idx];
        for dep in &item.blocked_by {
            let blocker_id = if let Some(s) = dep.as_str() {
                if let Some(stripped) = s.strip_prefix('@') {
                    let batch_idx: usize =
                        stripped.parse().map_err(|_| ItrError::InvalidValue {
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

    // Build results with issue details
    let config = UrgencyConfig::load(conn);
    let mut results: Vec<BatchItemResult> = Vec::new();
    for (idx, id) in created_ids.iter().enumerate() {
        let issue = db::get_issue(conn, *id)?;
        let detail = build_issue_detail(conn, issue, &config)?;

        let review_notes = &item_review_notes[idx];
        let outcome = if review_notes.is_empty() {
            "ok"
        } else {
            "review"
        };

        results.push(BatchItemResult {
            id: *id,
            outcome: outcome.to_string(),
            error: None,
            notes: review_notes.clone(),
            unblocked: vec![],
            issue: Some(detail),
        });
    }

    let summary = build_summary(&results);
    let batch_result = BatchResult {
        action: "batch_add".to_string(),
        results,
        summary,
        dry_run: false,
    };

    println!("{}", format::format_batch_result(&batch_result, fmt));

    Ok(())
}

pub fn run_close(conn: &Connection, dry_run: bool, fmt: Format) -> Result<(), ItrError> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let items: Vec<BatchCloseInput> = serde_json::from_str(&input)?;

    let tx = conn.unchecked_transaction()?;

    let mut results: Vec<BatchItemResult> = Vec::new();

    for item in &items {
        // Try to get the issue
        let issue = match db::get_issue(&tx, item.id) {
            Ok(i) => i,
            Err(ItrError::NotFound(_)) => {
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: "error".to_string(),
                    error: Some(format!("Issue {} not found", item.id)),
                    notes: vec![],
                    unblocked: vec![],
                    issue: None,
                });
                continue;
            }
            Err(e) => return Err(e),
        };

        // Already closed — idempotent ok
        if issue.status == "done" || issue.status == "wontfix" {
            results.push(BatchItemResult {
                id: item.id,
                outcome: "ok".to_string(),
                error: None,
                notes: vec![format!("Already {}", issue.status)],
                unblocked: vec![],
                issue: None,
            });
            continue;
        }

        let status = if item.wontfix { "wontfix" } else { "done" };

        db::record_event(&tx, item.id, "status", &issue.status, status)?;
        db::update_issue_field(&tx, item.id, "status", status)?;

        if !item.reason.is_empty() {
            db::record_event(
                &tx,
                item.id,
                "close_reason",
                &issue.close_reason,
                &item.reason,
            )?;
            db::update_issue_field(&tx, item.id, "close_reason", &item.reason)?;
        }

        // Check for newly unblocked issues
        let unblocked_list = db::get_newly_unblocked(&tx, item.id)?;
        let unblocked: Vec<UnblockedIssue> = unblocked_list
            .into_iter()
            .map(|(id, title)| UnblockedIssue { id, title })
            .collect();

        let notes = if item.reason.is_empty() {
            vec![]
        } else {
            vec![item.reason.clone()]
        };

        results.push(BatchItemResult {
            id: item.id,
            outcome: "ok".to_string(),
            error: None,
            notes,
            unblocked,
            issue: None,
        });
    }

    if !dry_run {
        tx.commit()?;
    }

    let summary = build_summary(&results);
    let batch_result = BatchResult {
        action: "batch_close".to_string(),
        results,
        summary,
        dry_run,
    };

    println!("{}", format::format_batch_result(&batch_result, fmt));

    Ok(())
}

pub fn run_update(conn: &Connection, dry_run: bool, fmt: Format) -> Result<(), ItrError> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let items: Vec<BatchUpdateInput> = serde_json::from_str(&input)?;

    let tx = conn.unchecked_transaction()?;

    let mut results: Vec<BatchItemResult> = Vec::new();

    for item in &items {
        // Try to get the issue
        let issue = match db::get_issue(&tx, item.id) {
            Ok(i) => i,
            Err(ItrError::NotFound(_)) => {
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: "error".to_string(),
                    error: Some(format!("Issue {} not found", item.id)),
                    notes: vec![],
                    unblocked: vec![],
                    issue: None,
                });
                continue;
            }
            Err(e) => return Err(e),
        };

        let mut review_notes: Vec<String> = Vec::new();
        let mut new_status: Option<String> = None;

        // Handle status
        if let Some(ref s) = item.status {
            let normalized = normalize::normalize_status(s);
            match validate_status(&normalized) {
                Ok(()) => {
                    db::record_event(&tx, item.id, "status", &issue.status, &normalized)?;
                    db::update_issue_field(&tx, item.id, "status", &normalized)?;
                    new_status = Some(normalized);
                }
                Err(_) => {
                    review_notes.push(format!(
                        "status '{}' not recognized, kept '{}'. Valid: open, in-progress, done, wontfix",
                        s, issue.status
                    ));
                }
            }
        }

        // Handle priority
        if let Some(ref p) = item.priority {
            let normalized = normalize::normalize_priority(p);
            match validate_priority(&normalized) {
                Ok(()) => {
                    db::record_event(&tx, item.id, "priority", &issue.priority, &normalized)?;
                    db::update_issue_field(&tx, item.id, "priority", &normalized)?;
                }
                Err(_) => {
                    review_notes.push(format!(
                        "priority '{}' not recognized, kept '{}'. Valid: critical, high, medium, low",
                        p, issue.priority
                    ));
                }
            }
        }

        // Handle kind
        if let Some(ref k) = item.kind {
            let normalized = normalize::normalize_kind(k);
            match validate_kind(&normalized) {
                Ok(()) => {
                    db::record_event(&tx, item.id, "kind", &issue.kind, &normalized)?;
                    db::update_issue_field(&tx, item.id, "kind", &normalized)?;
                }
                Err(_) => {
                    review_notes.push(format!(
                        "kind '{}' not recognized, kept '{}'. Valid: bug, feature, task, epic",
                        k, issue.kind
                    ));
                }
            }
        }

        // Handle title
        if let Some(ref t) = item.title {
            db::record_event(&tx, item.id, "title", &issue.title, t)?;
            db::update_issue_field(&tx, item.id, "title", t)?;
        }

        // Handle context
        if let Some(ref c) = item.context {
            db::record_event(&tx, item.id, "context", &issue.context, c)?;
            db::update_issue_field(&tx, item.id, "context", c)?;
        }

        // Handle assigned_to
        if let Some(ref a) = item.assigned_to {
            db::record_event(&tx, item.id, "assigned_to", &issue.assigned_to, a)?;
            db::update_issue_field(&tx, item.id, "assigned_to", a)?;
        }

        // Handle add_tags / remove_tags
        if !item.add_tags.is_empty() || !item.remove_tags.is_empty() {
            let current = db::get_issue(&tx, item.id)?;
            let updated = util::apply_tags(current.tags, &item.add_tags, &item.remove_tags);
            let json = serde_json::to_string(&updated)?;
            db::update_issue_field(&tx, item.id, "tags", &json)?;
        }

        // Handle add_skills / remove_skills
        if !item.add_skills.is_empty() || !item.remove_skills.is_empty() {
            let current = db::get_issue(&tx, item.id)?;
            let updated = util::apply_skills(current.skills, &item.add_skills, &item.remove_skills);
            let json = serde_json::to_string(&updated)?;
            db::update_issue_field(&tx, item.id, "skills", &json)?;
        }

        // Add _needs_review tag and notes if any field was auto-corrected
        if !review_notes.is_empty() {
            let current = db::get_issue(&tx, item.id)?;
            let mut tags = current.tags.clone();
            if !tags.contains(&"_needs_review".to_string()) {
                tags.push("_needs_review".to_string());
                let json = serde_json::to_string(&tags)?;
                db::update_issue_field(&tx, item.id, "tags", &json)?;
            }
            for note_text in &review_notes {
                db::add_note(&tx, item.id, note_text, "itr")?;
            }
        }

        // Check for newly unblocked issues if status changed to terminal
        let unblocked = match new_status.as_deref() {
            Some("done") | Some("wontfix") => {
                let list = db::get_newly_unblocked(&tx, item.id)?;
                list.into_iter()
                    .map(|(id, title)| UnblockedIssue { id, title })
                    .collect()
            }
            _ => vec![],
        };

        let outcome = if review_notes.is_empty() {
            "ok"
        } else {
            "review"
        };

        results.push(BatchItemResult {
            id: item.id,
            outcome: outcome.to_string(),
            error: None,
            notes: review_notes,
            unblocked,
            issue: None,
        });
    }

    if !dry_run {
        tx.commit()?;
    }

    let summary = build_summary(&results);
    let batch_result = BatchResult {
        action: "batch_update".to_string(),
        results,
        summary,
        dry_run,
    };

    println!("{}", format::format_batch_result(&batch_result, fmt));

    Ok(())
}

pub fn run_note(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let items: Vec<BatchNoteInput> = serde_json::from_str(&input)?;

    let mut results: Vec<BatchItemResult> = Vec::new();

    for item in &items {
        // Resolve agent: input agent field, else ITR_AGENT env, else empty
        let agent = if item.agent.is_empty() {
            std::env::var("ITR_AGENT").unwrap_or_default()
        } else {
            item.agent.clone()
        };

        match db::add_note(conn, item.id, &item.text, &agent) {
            Ok(note) => {
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: "ok".to_string(),
                    error: None,
                    notes: vec![note.content],
                    unblocked: vec![],
                    issue: None,
                });
            }
            Err(ItrError::NotFound(_)) => {
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: "error".to_string(),
                    error: Some(format!("Issue {} not found", item.id)),
                    notes: vec![],
                    unblocked: vec![],
                    issue: None,
                });
            }
            Err(e) => return Err(e),
        }
    }

    let summary = build_summary(&results);
    let batch_result = BatchResult {
        action: "batch_note".to_string(),
        results,
        summary,
        dry_run: false,
    };

    println!("{}", format::format_batch_result(&batch_result, fmt));

    Ok(())
}

fn build_summary(results: &[BatchItemResult]) -> BatchSummary {
    let mut ok = 0;
    let mut error = 0;
    let mut review = 0;
    for r in results {
        match r.outcome.as_str() {
            "ok" => ok += 1,
            "error" => error += 1,
            "review" => review += 1,
            _ => {}
        }
    }
    BatchSummary {
        total: results.len(),
        ok,
        error,
        review,
    }
}
