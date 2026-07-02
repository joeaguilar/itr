use crate::commands::build_issue_detail;
use crate::commands::update::persist_list_field;
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{
    BatchAddInput, BatchCloseInput, BatchItemResult, BatchNoteInput, BatchResult, BatchSummary,
    BatchUpdateInput, ParentChange, UnblockedIssue,
};
use crate::normalize;
use crate::normalize::{validate_kind, validate_priority, validate_status};
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;
use std::io::{self, Read};

/// JSON keys recognized by [`BatchAddInput`] (including serde aliases).
/// Keep in sync with the struct definition in `models.rs` — anything else in
/// an item is reported via a REVIEW note instead of being silently dropped.
const BATCH_ADD_KNOWN_KEYS: &[&str] = &[
    "title",
    "priority",
    "kind",
    "context",
    "files",
    "tags",
    "skills",
    "acceptance",
    "parent_id",
    "parent",
    "assigned_to",
    "blocked_by",
];

/// JSON keys recognized by [`BatchUpdateInput`] (including serde aliases).
/// Keep in sync with the struct definition in `models.rs` (#212).
const BATCH_UPDATE_KNOWN_KEYS: &[&str] = &[
    "id",
    "status",
    "priority",
    "kind",
    "title",
    "context",
    "assigned_to",
    "add_tags",
    "remove_tags",
    "add_skills",
    "remove_skills",
    "parent_id",
    "parent",
    "no_parent",
];

/// JSON keys recognized by [`BatchCloseInput`] (#212).
const BATCH_CLOSE_KNOWN_KEYS: &[&str] = &["id", "reason", "wontfix"];

/// JSON keys recognized by [`BatchNoteInput`] (#212).
const BATCH_NOTE_KNOWN_KEYS: &[&str] = &["id", "text", "agent"];

/// REVIEW notes for any keys of `value` not in `known_keys` — the shared
/// "never silently swallow input" check behind every batch verb (#150, #212).
fn unknown_key_notes(value: &serde_json::Value, known_keys: &[&str]) -> Vec<String> {
    let Some(map) = value.as_object() else {
        return vec![];
    };
    let unknown: Vec<String> = map
        .keys()
        .filter(|k| !known_keys.contains(&k.as_str()))
        .cloned()
        .collect();
    if unknown.is_empty() {
        return vec![];
    }
    vec![format!(
        "REVIEW: unrecognized field(s) ignored: {}. Known fields: {}",
        unknown.join(", "),
        known_keys.join(", ")
    )]
}

/// A single resolved `blocked_by` entry from an add payload.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BlockedByRef {
    /// A concrete issue ID (JSON integer or numeric string).
    Id(i64),
    /// An `"@N"` reference to the N-th item of the same batch.
    BatchIndex(usize),
}

/// Parse one `blocked_by` JSON value. `Err` carries the display form of the
/// unparseable token so callers can quote it in a REVIEW note.
pub(crate) fn parse_blocked_by_entry(dep: &serde_json::Value) -> Result<BlockedByRef, String> {
    if let Some(s) = dep.as_str() {
        if let Some(stripped) = s.strip_prefix('@') {
            return stripped
                .parse::<usize>()
                .map(BlockedByRef::BatchIndex)
                .map_err(|_| s.to_string());
        }
        return s
            .trim()
            .parse::<i64>()
            .map(BlockedByRef::Id)
            .map_err(|_| s.to_string());
    }
    if let Some(n) = dep.as_i64() {
        return Ok(BlockedByRef::Id(n));
    }
    Err(dep.to_string())
}

/// Deserialize one add item, collecting REVIEW notes for unrecognized JSON
/// keys (soft fallback: unknown fields must never be silently dropped, #150).
pub(crate) fn parse_add_item(
    value: &serde_json::Value,
) -> Result<(BatchAddInput, Vec<String>), serde_json::Error> {
    let item: BatchAddInput = serde_json::from_value(value.clone())?;
    Ok((item, unknown_key_notes(value, BATCH_ADD_KNOWN_KEYS)))
}

/// Deserialize each element of a JSON array individually so one malformed
/// item becomes a per-item `error` outcome instead of rejecting the whole
/// batch (soft fallback, #164). Each parsed item carries REVIEW notes for
/// keys outside `known_keys` — unknown fields must never be silently
/// dropped (#212). A top-level non-array payload is still a hard parse
/// error.
/// One deserialized batch item plus its unknown-key REVIEW notes, or the
/// per-item `error` outcome when the item failed to parse at all.
type ParsedItem<T> = Result<(T, Vec<String>), BatchItemResult>;

fn parse_each<T: serde::de::DeserializeOwned>(
    input: &str,
    known_keys: &[&str],
) -> Result<Vec<ParsedItem<T>>, ItrError> {
    let values: Vec<serde_json::Value> = serde_json::from_str(input)?;
    Ok(values
        .into_iter()
        .enumerate()
        .map(|(idx, value)| {
            serde_json::from_value::<T>(value.clone())
                .map(|item| (item, unknown_key_notes(&value, known_keys)))
                .map_err(|e| BatchItemResult {
                    id: value
                        .get("id")
                        .and_then(serde_json::Value::as_i64)
                        .unwrap_or(0),
                    outcome: "error".to_string(),
                    error: Some(format!("item {idx}: {e}")),
                    notes: vec![],
                    unblocked: vec![],
                    issue: None,
                })
        })
        .collect())
}

fn read_stdin() -> Result<String, ItrError> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    Ok(input)
}

/// Add the `_needs_review` tag to an issue if it is not already present.
/// The auto-added tag is an edit like any other, so it records a tags audit
/// event in the same JSON-array format as explicit tag changes (#187).
fn ensure_needs_review_tag(conn: &Connection, id: i64) -> Result<(), ItrError> {
    let tags = db::get_issue(conn, id)?.tags;
    if !tags.contains(&"_needs_review".to_string()) {
        let mut new_tags = tags.clone();
        new_tags.push("_needs_review".to_string());
        persist_list_field(conn, id, "tags", &tags, &new_tags)?;
    }
    Ok(())
}

pub fn run_add(conn: &Connection, dry_run: bool, fmt: Format) -> Result<(), ItrError> {
    let input = read_stdin()?;
    let batch_result = run_add_core(conn, &input, dry_run)?;
    println!("{}", format::format_batch_result(&batch_result, fmt));
    Ok(())
}

/// Core of `batch add`. With `dry_run`, the exact same parse/validate/insert
/// path runs inside the transaction and the transaction is rolled back
/// instead of committed — per-item verdicts (including resolved priority/kind
/// defaults and `@N` dependency resolution) match the real run while nothing
/// is written.
fn run_add_core(conn: &Connection, input: &str, dry_run: bool) -> Result<BatchResult, ItrError> {
    let values: Vec<serde_json::Value> = serde_json::from_str(input)?;

    // Parse each item individually; a malformed item is reported as a
    // per-item error outcome while the valid items still get created.
    let mut parsed: Vec<Result<(BatchAddInput, Vec<String>), String>> = values
        .iter()
        .enumerate()
        .map(|(idx, value)| parse_add_item(value).map_err(|e| format!("item {idx}: {e}")))
        .collect();

    // Use a transaction
    let tx = conn.unchecked_transaction()?;

    // First pass: create all issues with soft fallback. `created[idx]` is
    // None when the item at that input index failed to parse.
    let mut created: Vec<Option<i64>> = Vec::with_capacity(parsed.len());
    for entry in &mut parsed {
        let Ok((item, review_notes)) = entry else {
            created.push(None);
            continue;
        };

        item.priority = normalize::normalize_priority(&item.priority);
        item.kind = normalize::normalize_kind(&item.kind);

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

        // Soft fallback (#167): a parent that doesn't exist would otherwise
        // surface as a raw FOREIGN KEY error and abort the whole batch.
        if let Some(p) = item.parent_id {
            if !db::issue_exists(&tx, p)? {
                review_notes.push(format!(
                    "REVIEW: parent {p} not found; issue created without a parent"
                ));
                item.parent_id = None;
            }
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
        created.push(Some(issue.id));
    }

    // Second pass: create dependencies. An unresolvable entry skips that
    // edge with a REVIEW note instead of aborting the batch (#164). Cycles
    // remain hard errors (CLAUDE.md: cycle detection cannot recover).
    for (idx, entry) in parsed.iter_mut().enumerate() {
        let Ok((item, review_notes)) = entry else {
            continue;
        };
        let Some(blocked_id) = created[idx] else {
            continue;
        };
        for dep in &item.blocked_by {
            let blocker_id = match parse_blocked_by_entry(dep) {
                Ok(BlockedByRef::Id(n)) => n,
                Ok(BlockedByRef::BatchIndex(i)) => match created.get(i).copied().flatten() {
                    Some(id) => id,
                    None => {
                        review_notes.push(format!(
                            "REVIEW: blocked_by '@{i}' does not refer to a created batch item; dependency skipped. Valid: @0 to @{}",
                            created.len().saturating_sub(1)
                        ));
                        continue;
                    }
                },
                Err(token) => {
                    review_notes.push(format!(
                        "REVIEW: blocked_by '{token}' is not a valid issue ID or '@N' batch reference; dependency skipped"
                    ));
                    continue;
                }
            };
            match db::add_dependency(&tx, blocker_id, blocked_id) {
                Ok(_) => {}
                Err(ItrError::NotFound(missing)) => {
                    review_notes.push(format!(
                        "REVIEW: blocked_by {missing} not found; dependency skipped"
                    ));
                }
                Err(e) => return Err(e),
            }
        }
    }

    // Third pass: persist review notes (and the `_needs_review` tag for
    // items whose notes were only discovered during dependency resolution).
    for (idx, entry) in parsed.iter().enumerate() {
        let Ok((_, review_notes)) = entry else {
            continue;
        };
        let Some(id) = created[idx] else {
            continue;
        };
        if !review_notes.is_empty() {
            ensure_needs_review_tag(&tx, id)?;
        }
        for note_text in review_notes {
            db::add_note(&tx, id, note_text, "itr")?;
        }
    }

    // Build results with issue details from the transaction state, so the
    // dry-run path reports exactly what a committed run would have created.
    let config = UrgencyConfig::load(&tx);
    let mut results: Vec<BatchItemResult> = Vec::with_capacity(parsed.len());
    for (idx, entry) in parsed.iter().enumerate() {
        match (entry, created[idx]) {
            (Ok((_, review_notes)), Some(id)) => {
                let issue = db::get_issue(&tx, id)?;
                let detail = build_issue_detail(&tx, issue, &config)?;

                let outcome = if review_notes.is_empty() {
                    "ok"
                } else {
                    "review"
                };

                results.push(BatchItemResult {
                    id,
                    outcome: outcome.to_string(),
                    error: None,
                    notes: review_notes.clone(),
                    unblocked: vec![],
                    issue: Some(detail),
                });
            }
            (Err(msg), _) => {
                results.push(BatchItemResult {
                    id: 0,
                    outcome: "error".to_string(),
                    error: Some(msg.clone()),
                    notes: vec![],
                    unblocked: vec![],
                    issue: None,
                });
            }
            (Ok(_), None) => unreachable!("parsed batch add item without a created issue id"),
        }
    }

    if !dry_run {
        tx.commit()?;
    }

    let summary = build_summary(&results);
    Ok(BatchResult {
        action: "batch_add".to_string(),
        results,
        summary,
        dry_run,
    })
}

pub fn run_close(conn: &Connection, dry_run: bool, fmt: Format) -> Result<(), ItrError> {
    let input = read_stdin()?;
    let batch_result = run_close_core(conn, &input, dry_run)?;
    println!("{}", format::format_batch_result(&batch_result, fmt));
    Ok(())
}

fn run_close_core(conn: &Connection, input: &str, dry_run: bool) -> Result<BatchResult, ItrError> {
    let items = parse_each::<BatchCloseInput>(input, BATCH_CLOSE_KNOWN_KEYS)?;

    let tx = conn.unchecked_transaction()?;

    let mut results: Vec<BatchItemResult> = Vec::with_capacity(items.len());

    for entry in items {
        let (item, review_notes) = match entry {
            Ok(item) => item,
            Err(error_result) => {
                results.push(error_result);
                continue;
            }
        };

        // Try to get the issue
        let issue = match db::get_issue(&tx, item.id) {
            Ok(i) => i,
            Err(ItrError::NotFound(_)) => {
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: "error".to_string(),
                    error: Some(format!("Issue {} not found", item.id)),
                    notes: review_notes,
                    unblocked: vec![],
                    issue: None,
                });
                continue;
            }
            Err(e) => return Err(e),
        };

        // Unknown payload keys still close the issue (accept partial valid
        // input) but flag the item for review (#212).
        if !review_notes.is_empty() {
            ensure_needs_review_tag(&tx, item.id)?;
            for note_text in &review_notes {
                db::add_note(&tx, item.id, note_text, "itr")?;
            }
        }
        let outcome = if review_notes.is_empty() {
            "ok"
        } else {
            "review"
        };

        // Already closed — idempotent ok
        if issue.status == "done" || issue.status == "wontfix" {
            let mut notes = vec![format!("Already {}", issue.status)];
            notes.extend(review_notes);
            results.push(BatchItemResult {
                id: item.id,
                outcome: outcome.to_string(),
                error: None,
                notes,
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

        // Check for newly unblocked issues, then auto-clean stale edges
        let unblocked_list = db::get_newly_unblocked(&tx, item.id)?;
        db::remove_blocker_edges(&tx, item.id)?;
        let unblocked: Vec<UnblockedIssue> = unblocked_list
            .into_iter()
            .map(|(id, title)| UnblockedIssue { id, title })
            .collect();

        let mut notes = if item.reason.is_empty() {
            vec![]
        } else {
            vec![item.reason.clone()]
        };
        notes.extend(review_notes);

        results.push(BatchItemResult {
            id: item.id,
            outcome: outcome.to_string(),
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
    Ok(BatchResult {
        action: "batch_close".to_string(),
        results,
        summary,
        dry_run,
    })
}

pub fn run_update(conn: &Connection, dry_run: bool, fmt: Format) -> Result<(), ItrError> {
    let input = read_stdin()?;
    let batch_result = run_update_core(conn, &input, dry_run)?;
    println!("{}", format::format_batch_result(&batch_result, fmt));
    Ok(())
}

fn run_update_core(conn: &Connection, input: &str, dry_run: bool) -> Result<BatchResult, ItrError> {
    let items = parse_each::<BatchUpdateInput>(input, BATCH_UPDATE_KNOWN_KEYS)?;

    let tx = conn.unchecked_transaction()?;

    let mut results: Vec<BatchItemResult> = Vec::with_capacity(items.len());

    for entry in items {
        let (item, mut review_notes) = match entry {
            Ok(item) => item,
            Err(error_result) => {
                results.push(error_result);
                continue;
            }
        };

        // Try to get the issue
        let issue = match db::get_issue(&tx, item.id) {
            Ok(i) => i,
            Err(ItrError::NotFound(_)) => {
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: "error".to_string(),
                    error: Some(format!("Issue {} not found", item.id)),
                    notes: review_notes,
                    unblocked: vec![],
                    issue: None,
                });
                continue;
            }
            Err(e) => return Err(e),
        };

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

        // Handle add_tags / remove_tags (audited in JSON-array format, #187)
        if !item.add_tags.is_empty() || !item.remove_tags.is_empty() {
            let current = db::get_issue(&tx, item.id)?.tags;
            let updated = util::apply_tags(current.clone(), &item.add_tags, &item.remove_tags);
            persist_list_field(&tx, item.id, "tags", &current, &updated)?;
        }

        // Handle add_skills / remove_skills (audited in JSON-array format, #187)
        if !item.add_skills.is_empty() || !item.remove_skills.is_empty() {
            let current = db::get_issue(&tx, item.id)?.skills;
            let updated =
                util::apply_skills(current.clone(), &item.add_skills, &item.remove_skills);
            persist_list_field(&tx, item.id, "skills", &current, &updated)?;
        }

        // Handle parent changes: `"parent_id": N` re-parents, `"parent_id":
        // null` or `"no_parent": true` clears — mirroring `itr update
        // --parent/--no-parent`, but with soft-fallback review notes where
        // the single-update path hard-errors (missing parent, cycle).
        let parent_change = if item.no_parent {
            if matches!(item.parent_id, ParentChange::Set(_)) {
                review_notes.push(
                    "both parent_id and no_parent set; parent unchanged. Use one of parent_id: <ID> or no_parent: true".to_string(),
                );
                ParentChange::Unchanged
            } else {
                ParentChange::Clear
            }
        } else {
            item.parent_id
        };
        let old_parent = issue.parent_id.map(|p| p.to_string()).unwrap_or_default();
        match parent_change {
            ParentChange::Unchanged => {}
            ParentChange::Set(pid) => {
                if !db::issue_exists(&tx, pid)? {
                    review_notes.push(format!("parent {pid} not found; parent unchanged"));
                } else if db::is_self_or_descendant(&tx, item.id, pid)? {
                    review_notes.push(format!(
                        "parent {} would create a cycle with {}; parent unchanged",
                        pid, item.id
                    ));
                } else if old_parent != pid.to_string() {
                    db::record_event(&tx, item.id, "parent_id", &old_parent, &pid.to_string())?;
                    db::update_issue_parent(&tx, item.id, Some(pid))?;
                }
            }
            ParentChange::Clear => {
                if !old_parent.is_empty() {
                    db::record_event(&tx, item.id, "parent_id", &old_parent, "")?;
                    db::update_issue_parent(&tx, item.id, None)?;
                }
            }
        }

        // Add _needs_review tag and notes if any field was auto-corrected
        if !review_notes.is_empty() {
            ensure_needs_review_tag(&tx, item.id)?;
            for note_text in &review_notes {
                db::add_note(&tx, item.id, note_text, "itr")?;
            }
        }

        // Check for newly unblocked issues if status changed to terminal
        let unblocked = match new_status.as_deref() {
            Some("done" | "wontfix") => {
                let list = db::get_newly_unblocked(&tx, item.id)?;
                db::remove_blocker_edges(&tx, item.id)?;
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
    Ok(BatchResult {
        action: "batch_update".to_string(),
        results,
        summary,
        dry_run,
    })
}

pub fn run_note(conn: &Connection, dry_run: bool, fmt: Format) -> Result<(), ItrError> {
    let input = read_stdin()?;
    let batch_result = run_note_core(conn, &input, dry_run)?;
    println!("{}", format::format_batch_result(&batch_result, fmt));
    Ok(())
}

fn run_note_core(conn: &Connection, input: &str, dry_run: bool) -> Result<BatchResult, ItrError> {
    let items = parse_each::<BatchNoteInput>(input, BATCH_NOTE_KNOWN_KEYS)?;

    let tx = conn.unchecked_transaction()?;

    let mut results: Vec<BatchItemResult> = Vec::with_capacity(items.len());

    for entry in items {
        let (item, review_notes) = match entry {
            Ok(item) => item,
            Err(error_result) => {
                results.push(error_result);
                continue;
            }
        };

        // Resolve agent: input agent field, else ITR_AGENT env, else empty
        let agent = if item.agent.is_empty() {
            std::env::var("ITR_AGENT").unwrap_or_default()
        } else {
            item.agent.clone()
        };

        match db::add_note(&tx, item.id, &item.text, &agent) {
            Ok(note) => {
                // Unknown payload keys still add the note (accept partial
                // valid input) but flag the item for review (#212).
                if !review_notes.is_empty() {
                    ensure_needs_review_tag(&tx, item.id)?;
                    for note_text in &review_notes {
                        db::add_note(&tx, item.id, note_text, "itr")?;
                    }
                }
                let outcome = if review_notes.is_empty() {
                    "ok"
                } else {
                    "review"
                };
                let mut notes = vec![note.content];
                notes.extend(review_notes);
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: outcome.to_string(),
                    error: None,
                    notes,
                    unblocked: vec![],
                    issue: None,
                });
            }
            Err(ItrError::NotFound(_)) => {
                results.push(BatchItemResult {
                    id: item.id,
                    outcome: "error".to_string(),
                    error: Some(format!("Issue {} not found", item.id)),
                    notes: review_notes,
                    unblocked: vec![],
                    issue: None,
                });
            }
            Err(e) => return Err(e),
        }
    }

    if !dry_run {
        tx.commit()?;
    }

    let summary = build_summary(&results);
    Ok(BatchResult {
        action: "batch_note".to_string(),
        results,
        summary,
        dry_run,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::open_test_db;

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

    fn note_contents(conn: &Connection, id: i64) -> Vec<String> {
        db::get_notes(conn, id)
            .unwrap()
            .into_iter()
            .map(|n| n.content)
            .collect()
    }

    // --- #150: `parent` (CLI-flag spelling) must not be silently dropped ---

    #[test]
    fn add_parent_alias_links_child_to_parent() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let input = format!(r#"[{{"title":"child","parent":{epic}}}]"#);
        let result = run_add_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[0].outcome, "ok");
        let child = db::get_issue(&conn, result.results[0].id).unwrap();
        assert_eq!(
            child.parent_id,
            Some(epic),
            "JSON 'parent' key must map to parent_id"
        );
    }

    #[test]
    fn add_parent_id_field_still_links() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let input = format!(r#"[{{"title":"child","parent_id":{epic}}}]"#);
        let result = run_add_core(&conn, &input, false).unwrap();
        let child = db::get_issue(&conn, result.results[0].id).unwrap();
        assert_eq!(child.parent_id, Some(epic));
    }

    #[test]
    fn add_unknown_field_emits_review_note() {
        let conn = open_test_db();
        let result = run_add_core(&conn, r#"[{"title":"x","parnt_id":7}]"#, false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert!(
            result.results[0].notes[0].contains("parnt_id"),
            "note should name the unrecognized key: {:?}",
            result.results[0].notes
        );
        let issue = db::get_issue(&conn, result.results[0].id).unwrap();
        assert!(issue.tags.contains(&"_needs_review".to_string()));
    }

    // --- #167: nonexistent parent gets soft fallback, not a FK error ---

    #[test]
    fn add_missing_parent_creates_parentless_with_review() {
        let conn = open_test_db();
        let result =
            run_add_core(&conn, r#"[{"title":"orphan","parent_id":9999}]"#, false).unwrap();
        assert_eq!(result.summary.total, 1);
        assert_eq!(result.results[0].outcome, "review");
        let issue = db::get_issue(&conn, result.results[0].id).unwrap();
        assert_eq!(issue.parent_id, None);
        assert!(issue.tags.contains(&"_needs_review".to_string()));
        assert!(
            note_contents(&conn, issue.id)
                .iter()
                .any(|n| n.contains("parent 9999")),
            "REVIEW note must name the missing parent"
        );
    }

    // --- #164: one malformed item must not reject the whole batch ---

    #[test]
    fn add_malformed_item_is_per_item_error() {
        let conn = open_test_db();
        let result = run_add_core(
            &conn,
            r#"[{"title":"good"},{"title":42},{"title":"also good"}]"#,
            false,
        )
        .unwrap();
        assert_eq!(result.summary.total, 3);
        assert_eq!(result.summary.ok, 2);
        assert_eq!(result.summary.error, 1);
        assert_eq!(result.results[1].outcome, "error");
        let msg = result.results[1].error.as_deref().unwrap();
        assert!(msg.contains("item 1"), "error should locate the bad item");
        assert!(db::issue_exists(&conn, result.results[0].id).unwrap());
        assert!(db::issue_exists(&conn, result.results[2].id).unwrap());
    }

    #[test]
    fn add_unresolvable_blocked_by_skips_edge_with_review() {
        let conn = open_test_db();
        let result =
            run_add_core(&conn, r#"[{"title":"a","blocked_by":[999,"junk"]}]"#, false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        let id = result.results[0].id;
        assert_eq!(db::get_blockers(&conn, id).unwrap(), Vec::<i64>::new());
        let notes = note_contents(&conn, id);
        assert!(notes.iter().any(|n| n.contains("999")));
        assert!(notes.iter().any(|n| n.contains("junk")));
        let issue = db::get_issue(&conn, id).unwrap();
        assert!(issue.tags.contains(&"_needs_review".to_string()));
    }

    #[test]
    fn add_at_ref_out_of_range_skips_edge_with_review() {
        let conn = open_test_db();
        let result = run_add_core(&conn, r#"[{"title":"a","blocked_by":["@5"]}]"#, false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        let id = result.results[0].id;
        assert_eq!(db::get_blockers(&conn, id).unwrap(), Vec::<i64>::new());
        assert!(note_contents(&conn, id).iter().any(|n| n.contains("@5")));
    }

    #[test]
    fn add_at_ref_to_failed_item_skips_edge_with_review() {
        let conn = open_test_db();
        let result = run_add_core(
            &conn,
            r#"[{"title":1},{"title":"b","blocked_by":["@0"]}]"#,
            false,
        )
        .unwrap();
        assert_eq!(result.results[0].outcome, "error");
        assert_eq!(result.results[1].outcome, "review");
        let id = result.results[1].id;
        assert_eq!(db::get_blockers(&conn, id).unwrap(), Vec::<i64>::new());
        assert!(note_contents(&conn, id).iter().any(|n| n.contains("@0")));
    }

    #[test]
    fn add_valid_at_refs_and_string_ids_still_link() {
        let conn = open_test_db();
        let pre = seed(&conn, "Pre-existing");
        let input = format!(r#"[{{"title":"a"}},{{"title":"b","blocked_by":["@0","{pre}"]}}]"#);
        let result = run_add_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[1].outcome, "ok");
        let mut blockers = db::get_blockers(&conn, result.results[1].id).unwrap();
        blockers.sort_unstable();
        assert_eq!(blockers, vec![pre, result.results[0].id]);
    }

    #[test]
    fn add_happy_path_shape_unchanged() {
        // Guard for the batch_bulk snapshots: valid items keep the exact
        // ok-outcome envelope (no notes, no errors, embedded issue).
        let conn = open_test_db();
        let result = run_add_core(
            &conn,
            r#"[{"title":"A","priority":"high"},{"title":"B"}]"#,
            false,
        )
        .unwrap();
        assert_eq!(result.action, "batch_add");
        assert!(!result.dry_run);
        assert_eq!(result.summary.total, 2);
        assert_eq!(result.summary.ok, 2);
        assert_eq!(result.summary.error, 0);
        assert_eq!(result.summary.review, 0);
        for item in &result.results {
            assert_eq!(item.outcome, "ok");
            assert!(item.error.is_none());
            assert!(item.notes.is_empty());
            assert!(item.issue.is_some());
        }
    }

    // --- spec P3: batch add / note --dry-run ---

    fn issue_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM issues", [], |row| row.get(0))
            .unwrap()
    }

    /// Golden payload for the dry-run contract: one malformed item, one
    /// unknown key, and one `@N` batch reference (spec acceptance #3).
    const DRY_RUN_GOLDEN: &str = r#"[
        {"title":"Story: pan gesture on chart","priority":"urgent","file":"src/chart.rs"},
        {"title":42},
        {"title":"Story: tick labels","blocked_by":["@0"]}
    ]"#;

    #[test]
    fn add_dry_run_writes_nothing() {
        let conn = open_test_db();
        let result = run_add_core(&conn, DRY_RUN_GOLDEN, true).unwrap();
        assert!(result.dry_run);
        assert_eq!(issue_count(&conn), 0, "dry-run must not create issues");
        let event_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .unwrap();
        assert_eq!(event_count, 0, "dry-run must not record events");
    }

    #[test]
    fn add_dry_run_verdicts_match_real_run() {
        // The same payload must produce identical per-item outcomes and notes
        // whether or not the writes are committed (same code path, spec P3).
        let dry_conn = open_test_db();
        let dry = run_add_core(&dry_conn, DRY_RUN_GOLDEN, true).unwrap();
        let real_conn = open_test_db();
        let real = run_add_core(&real_conn, DRY_RUN_GOLDEN, false).unwrap();

        assert_eq!(dry.summary.total, real.summary.total);
        assert_eq!(dry.summary.ok, real.summary.ok);
        assert_eq!(dry.summary.review, real.summary.review);
        assert_eq!(dry.summary.error, real.summary.error);
        for (d, r) in dry.results.iter().zip(real.results.iter()) {
            assert_eq!(d.outcome, r.outcome);
            assert_eq!(d.notes, r.notes);
        }
        assert_eq!(issue_count(&dry_conn), 0);
        assert_eq!(issue_count(&real_conn), 2);
    }

    #[test]
    fn add_dry_run_reports_resolved_defaults() {
        // Authors must see the resolved priority/kind they'll actually get.
        let conn = open_test_db();
        let result = run_add_core(
            &conn,
            r#"[{"title":"x","priority":"bogus","kind":"story"}]"#,
            true,
        )
        .unwrap();
        assert_eq!(result.results[0].outcome, "review");
        let issue = &result.results[0].issue.as_ref().unwrap().issue;
        assert_eq!(issue.priority, "medium", "resolved default is visible");
        assert!(
            result.results[0].notes.iter().any(|n| n.contains("bogus")),
            "note names the unrecognized value"
        );
        assert_eq!(issue_count(&conn), 0);
    }

    #[test]
    fn note_dry_run_writes_nothing_but_reports_ok() {
        let conn = open_test_db();
        let id = seed(&conn, "target");
        let input = format!(r#"[{{"id":{id},"text":"planned"}},{{"id":999,"text":"nope"}}]"#);
        let result = run_note_core(&conn, &input, true).unwrap();
        assert!(result.dry_run);
        assert_eq!(result.summary.ok, 1);
        assert_eq!(result.summary.error, 1);
        assert!(
            note_contents(&conn, id).is_empty(),
            "dry-run must not write notes"
        );
    }

    // --- #164: close/update/note per-item shape errors ---

    #[test]
    fn close_malformed_item_is_per_item_error() {
        let conn = open_test_db();
        let id = seed(&conn, "to close");
        let input = format!(r#"[{{"id":{id},"reason":"done"}},{{"id":"x"}}]"#);
        let result = run_close_core(&conn, &input, false).unwrap();
        assert_eq!(result.summary.ok, 1);
        assert_eq!(result.summary.error, 1);
        assert_eq!(result.results[1].outcome, "error");
        assert_eq!(db::get_issue(&conn, id).unwrap().status, "done");
    }

    #[test]
    fn update_malformed_item_is_per_item_error() {
        let conn = open_test_db();
        let id = seed(&conn, "to update");
        let input = format!(r#"[{{"id":{id},"status":"in-progress"}},{{"bogus":true}}]"#);
        let result = run_update_core(&conn, &input, false).unwrap();
        assert_eq!(result.summary.ok, 1);
        assert_eq!(result.summary.error, 1);
        assert_eq!(result.results[1].outcome, "error");
        assert_eq!(db::get_issue(&conn, id).unwrap().status, "in-progress");
    }

    #[test]
    fn note_malformed_item_is_per_item_error() {
        let conn = open_test_db();
        let id = seed(&conn, "to note");
        let input = format!(r#"[{{"id":{id},"text":"hi"}},{{"text":"no id"}}]"#);
        let result = run_note_core(&conn, &input, false).unwrap();
        assert_eq!(result.summary.ok, 1);
        assert_eq!(result.summary.error, 1);
        assert_eq!(result.results[1].outcome, "error");
        assert_eq!(note_contents(&conn, id), vec!["hi".to_string()]);
    }

    #[test]
    fn malformed_item_error_uses_id_from_payload_when_present() {
        let parsed =
            parse_each::<BatchCloseInput>(r#"[{"id":42,"wontfix":"yes"}]"#, BATCH_CLOSE_KNOWN_KEYS)
                .unwrap();
        let err = parsed[0].as_ref().unwrap_err();
        assert_eq!(err.id, 42);
        assert_eq!(err.outcome, "error");
    }

    // --- #187: batch update tag/skill changes record audit events ---

    fn events_for(conn: &Connection, id: i64, field: &str) -> Vec<crate::models::Event> {
        db::get_events_for_issue(conn, id)
            .unwrap()
            .into_iter()
            .filter(|e| e.field == field)
            .collect()
    }

    #[test]
    fn update_add_tags_records_tags_event_in_json_array_format() {
        let conn = open_test_db();
        let id = seed(&conn, "tagged");
        let input = format!(r#"[{{"id":{id},"add_tags":["urgent"]}}]"#);
        let result = run_update_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[0].outcome, "ok");
        let events = events_for(&conn, id, "tags");
        assert_eq!(events.len(), 1, "batch tag add must be audited");
        assert_eq!(events[0].old_value, "[]");
        assert_eq!(events[0].new_value, r#"["urgent"]"#);
    }

    #[test]
    fn update_add_skills_records_skills_event() {
        let conn = open_test_db();
        let id = seed(&conn, "skilled");
        let input = format!(r#"[{{"id":{id},"add_skills":["sql"]}}]"#);
        run_update_core(&conn, &input, false).unwrap();
        let events = events_for(&conn, id, "skills");
        assert_eq!(events.len(), 1, "batch skill add must be audited");
        assert_eq!(events[0].old_value, "[]");
        assert_eq!(events[0].new_value, r#"["sql"]"#);
    }

    #[test]
    fn update_auto_needs_review_tag_records_tags_event() {
        let conn = open_test_db();
        let id = seed(&conn, "reviewed");
        let input = format!(r#"[{{"id":{id},"priority":"bogus"}}]"#);
        let result = run_update_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        let events = events_for(&conn, id, "tags");
        assert_eq!(events.len(), 1, "auto-added _needs_review must be audited");
        assert_eq!(events[0].new_value, r#"["_needs_review"]"#);
    }

    // --- #163: unrecognized status keeps the existing value, matching the
    // single-update path ---

    #[test]
    fn update_unrecognized_status_keeps_existing_value() {
        let conn = open_test_db();
        let id = seed(&conn, "done work");
        run_update_core(&conn, &format!(r#"[{{"id":{id},"status":"done"}}]"#), false).unwrap();
        let result = run_update_core(
            &conn,
            &format!(r#"[{{"id":{id},"status":"blocked"}}]"#),
            false,
        )
        .unwrap();
        assert_eq!(result.results[0].outcome, "review");
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.status, "done", "unrecognized status must not reopen");
        assert!(
            result.results[0].notes[0].contains("kept 'done'"),
            "review note must say which value was kept: {:?}",
            result.results[0].notes
        );
    }

    // --- #212: unknown item keys in update/close/note get REVIEW notes
    // instead of being silently dropped (parity with batch add, #150) ---

    #[test]
    fn update_unknown_key_emits_review_note_and_applies_valid_fields() {
        let conn = open_test_db();
        let id = seed(&conn, "typo victim");
        let input = format!(r#"[{{"id":{id},"prioriy":"high","status":"in-progress"}}]"#);
        let result = run_update_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert!(
            result.results[0].notes[0].contains("prioriy"),
            "note should name the unrecognized key: {:?}",
            result.results[0].notes
        );
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.status, "in-progress", "valid fields must still apply");
        assert_eq!(
            issue.priority, "medium",
            "typoed key must not change anything"
        );
        assert!(issue.tags.contains(&"_needs_review".to_string()));
    }

    #[test]
    fn close_unknown_key_emits_review_note_and_still_closes() {
        let conn = open_test_db();
        let id = seed(&conn, "to close");
        let input = format!(r#"[{{"id":{id},"reson":"fixed"}}]"#);
        let result = run_close_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert!(
            result.results[0].notes.iter().any(|n| n.contains("reson")),
            "note should name the unrecognized key: {:?}",
            result.results[0].notes
        );
        let issue = db::get_issue(&conn, id).unwrap();
        assert_eq!(issue.status, "done", "unknown key must not block the close");
        assert_eq!(issue.close_reason, "", "typoed reason must not be applied");
        assert!(issue.tags.contains(&"_needs_review".to_string()));
    }

    #[test]
    fn note_unknown_key_emits_review_note_and_still_adds_note() {
        let conn = open_test_db();
        let id = seed(&conn, "to annotate");
        let input = format!(r#"[{{"id":{id},"text":"hello","agnt":"bot"}}]"#);
        let result = run_note_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert!(
            result.results[0].notes.iter().any(|n| n.contains("agnt")),
            "note should name the unrecognized key: {:?}",
            result.results[0].notes
        );
        let contents = note_contents(&conn, id);
        assert!(
            contents.iter().any(|n| n == "hello"),
            "the note itself must still be added: {contents:?}"
        );
        assert!(db::get_issue(&conn, id)
            .unwrap()
            .tags
            .contains(&"_needs_review".to_string()));
    }

    // --- #211: batch update must support parent changes (was silently
    // dropped: item reported ok, nothing written) ---

    #[test]
    fn update_parent_id_reparents_and_records_event() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let child = seed(&conn, "child");
        let input = format!(r#"[{{"id":{child},"parent_id":{epic}}}]"#);
        let result = run_update_core(&conn, &input, false).unwrap();
        assert_eq!(result.results[0].outcome, "ok");
        assert_eq!(db::get_issue(&conn, child).unwrap().parent_id, Some(epic));
        let events = events_for(&conn, child, "parent_id");
        assert_eq!(events.len(), 1, "re-parenting must be audited");
        assert_eq!(events[0].old_value, "");
        assert_eq!(events[0].new_value, epic.to_string());
    }

    #[test]
    fn update_parent_alias_reparents() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let child = seed(&conn, "child");
        let input = format!(r#"[{{"id":{child},"parent":{epic}}}]"#);
        run_update_core(&conn, &input, false).unwrap();
        assert_eq!(
            db::get_issue(&conn, child).unwrap().parent_id,
            Some(epic),
            "JSON 'parent' key must map to parent_id"
        );
    }

    #[test]
    fn update_parent_null_clears_parent() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let child = seed(&conn, "child");
        run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"parent_id":{epic}}}]"#),
            false,
        )
        .unwrap();
        let result = run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"parent_id":null}}]"#),
            false,
        )
        .unwrap();
        assert_eq!(result.results[0].outcome, "ok");
        assert_eq!(db::get_issue(&conn, child).unwrap().parent_id, None);
        let events = events_for(&conn, child, "parent_id");
        assert_eq!(events.len(), 2, "clearing must be audited");
        assert_eq!(events[1].new_value, "");
    }

    #[test]
    fn update_no_parent_true_clears_parent() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let child = seed(&conn, "child");
        run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"parent_id":{epic}}}]"#),
            false,
        )
        .unwrap();
        run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"no_parent":true}}]"#),
            false,
        )
        .unwrap();
        assert_eq!(db::get_issue(&conn, child).unwrap().parent_id, None);
    }

    #[test]
    fn update_absent_parent_key_leaves_parent_unchanged() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let child = seed(&conn, "child");
        run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"parent_id":{epic}}}]"#),
            false,
        )
        .unwrap();
        run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"priority":"high"}}]"#),
            false,
        )
        .unwrap();
        assert_eq!(
            db::get_issue(&conn, child).unwrap().parent_id,
            Some(epic),
            "an item without a parent key must not touch the parent"
        );
    }

    #[test]
    fn update_missing_parent_is_soft_fallback() {
        let conn = open_test_db();
        let id = seed(&conn, "orphan");
        let result = run_update_core(
            &conn,
            &format!(r#"[{{"id":{id},"parent_id":9999}}]"#),
            false,
        )
        .unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert_eq!(db::get_issue(&conn, id).unwrap().parent_id, None);
        assert!(
            result.results[0].notes[0].contains("parent 9999"),
            "review note must name the missing parent: {:?}",
            result.results[0].notes
        );
        assert!(
            events_for(&conn, id, "parent_id").is_empty(),
            "no event when nothing was written"
        );
    }

    #[test]
    fn update_parent_cycle_is_soft_fallback() {
        let conn = open_test_db();
        let a = seed(&conn, "A");
        let b = seed(&conn, "B");
        run_update_core(&conn, &format!(r#"[{{"id":{a},"parent_id":{b}}}]"#), false).unwrap();
        let result =
            run_update_core(&conn, &format!(r#"[{{"id":{b},"parent_id":{a}}}]"#), false).unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert_eq!(
            db::get_issue(&conn, b).unwrap().parent_id,
            None,
            "cycle-creating parent must be rejected"
        );
        assert!(
            result.results[0].notes[0].contains("cycle"),
            "review note must mention the cycle: {:?}",
            result.results[0].notes
        );
    }

    #[test]
    fn update_self_parent_is_soft_fallback() {
        let conn = open_test_db();
        let id = seed(&conn, "narcissist");
        let result = run_update_core(
            &conn,
            &format!(r#"[{{"id":{id},"parent_id":{id}}}]"#),
            false,
        )
        .unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert_eq!(db::get_issue(&conn, id).unwrap().parent_id, None);
    }

    #[test]
    fn update_parent_id_and_no_parent_conflict_is_soft_fallback() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let child = seed(&conn, "child");
        run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"parent_id":{epic}}}]"#),
            false,
        )
        .unwrap();
        let other = seed(&conn, "Other epic");
        let result = run_update_core(
            &conn,
            &format!(r#"[{{"id":{child},"parent_id":{other},"no_parent":true}}]"#),
            false,
        )
        .unwrap();
        assert_eq!(result.results[0].outcome, "review");
        assert_eq!(
            db::get_issue(&conn, child).unwrap().parent_id,
            Some(epic),
            "conflicting parent directives must leave the parent unchanged"
        );
    }

    // --- pure parsing helpers ---

    #[test]
    fn parse_blocked_by_entry_variants() {
        assert!(matches!(
            parse_blocked_by_entry(&serde_json::json!(3)),
            Ok(BlockedByRef::Id(3))
        ));
        assert!(matches!(
            parse_blocked_by_entry(&serde_json::json!("3")),
            Ok(BlockedByRef::Id(3))
        ));
        assert!(matches!(
            parse_blocked_by_entry(&serde_json::json!(" 3 ")),
            Ok(BlockedByRef::Id(3))
        ));
        assert!(matches!(
            parse_blocked_by_entry(&serde_json::json!("@2")),
            Ok(BlockedByRef::BatchIndex(2))
        ));
        assert_eq!(
            parse_blocked_by_entry(&serde_json::json!("junk")),
            Err("junk".to_string())
        );
        assert_eq!(
            parse_blocked_by_entry(&serde_json::json!("@x")),
            Err("@x".to_string())
        );
        assert_eq!(
            parse_blocked_by_entry(&serde_json::json!(true)),
            Err("true".to_string())
        );
    }
}
