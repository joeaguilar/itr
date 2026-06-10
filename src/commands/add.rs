use crate::commands::batch::{parse_add_item, parse_blocked_by_entry, BlockedByRef};
use crate::commands::build_issue_detail;
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::IssueDetail;
use crate::normalize::{self, validate_kind, validate_priority};
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;
use std::io::{self, Read};

/// Fully parsed `add` input, independent of whether it came from CLI flags or
/// a `--stdin-json` payload. `review_notes` carries REVIEW notes accumulated
/// during parsing (invalid `blocked_by` tokens, unrecognized JSON fields, ...).
pub(crate) struct AddRequest {
    pub title: String,
    pub priority: String,
    pub kind: String,
    pub context: String,
    pub files: Vec<String>,
    pub tags: Vec<String>,
    pub skills: Vec<String>,
    pub acceptance: String,
    pub parent_id: Option<i64>,
    pub assigned_to: String,
    pub blocked_by_ids: Vec<i64>,
    pub review_notes: Vec<String>,
}

fn parse_blocked_by_tokens(blocked_by: Option<String>) -> (Vec<i64>, Vec<String>) {
    let Some(blocked_by) = blocked_by else {
        return (Vec::new(), Vec::new());
    };

    let mut ids = Vec::new();
    let mut invalid = Vec::new();
    for token in blocked_by
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match token.parse::<i64>() {
            Ok(id) => ids.push(id),
            Err(_) => invalid.push(token.to_string()),
        }
    }
    (ids, invalid)
}

/// Parse a `--stdin-json` payload. Accepts the same fields as a `batch add`
/// item, including string/integer `blocked_by` entries (#165) and the
/// `parent` alias for `parent_id` (#150). Unresolvable tokens become REVIEW
/// notes instead of being silently dropped.
fn parse_stdin_json(input: &str) -> Result<AddRequest, ItrError> {
    let value: serde_json::Value = serde_json::from_str(input)?;
    let (data, mut review_notes) = parse_add_item(&value)?;

    let mut blocked_by_ids = Vec::new();
    for dep in &data.blocked_by {
        match parse_blocked_by_entry(dep) {
            Ok(BlockedByRef::Id(n)) => blocked_by_ids.push(n),
            Ok(BlockedByRef::BatchIndex(idx)) => review_notes.push(format!(
                "REVIEW: blocked_by '@{idx}' was ignored; '@N' batch references are only valid in batch add"
            )),
            Err(token) => review_notes.push(format!(
                "REVIEW: blocked_by '{token}' is not a valid issue ID and was ignored. Valid: integer issue IDs"
            )),
        }
    }

    Ok(AddRequest {
        title: data.title,
        priority: data.priority,
        kind: data.kind,
        context: data.context,
        files: data.files,
        tags: data.tags,
        skills: data
            .skills
            .iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        acceptance: data.acceptance,
        parent_id: data.parent_id,
        assigned_to: data.assigned_to,
        blocked_by_ids,
        review_notes,
    })
}

/// Validate, insert, and link a parsed add request. Returns the detail of the
/// created issue. Soft fallbacks: unrecognized priority/kind default with a
/// REVIEW note; a nonexistent parent creates the issue parentless with a
/// REVIEW note (#167). A nonexistent `blocked_by` ID remains a hard `NotFound`
/// error (whole insert rolls back), matching the documented CLI contract.
pub(crate) fn execute(conn: &Connection, req: AddRequest) -> Result<IssueDetail, ItrError> {
    let mut review_notes = req.review_notes;

    let priority = normalize::normalize_priority(&req.priority);
    let kind = normalize::normalize_kind(&req.kind);

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

    let tx = conn.unchecked_transaction()?;

    // Soft fallback (#167): a parent that doesn't exist would otherwise
    // surface as a raw FOREIGN KEY constraint error.
    let parent_id = match req.parent_id {
        Some(p) if !db::issue_exists(&tx, p)? => {
            review_notes.push(format!(
                "REVIEW: parent {p} not found; issue created without a parent"
            ));
            None
        }
        other => other,
    };

    let mut tags_vec = req.tags;
    if !review_notes.is_empty() && !tags_vec.contains(&"_needs_review".to_string()) {
        tags_vec.push("_needs_review".to_string());
    }

    let issue = db::insert_issue(
        &tx,
        &req.title,
        &priority,
        &kind,
        &req.context,
        &req.files,
        &tags_vec,
        &req.skills,
        &req.acceptance,
        parent_id,
        &req.assigned_to,
    )?;

    // Add review notes
    for note_text in &review_notes {
        db::add_note(&tx, issue.id, note_text, "itr")?;
    }

    // Add dependencies
    for blocker_id in &req.blocked_by_ids {
        db::add_dependency(&tx, *blocker_id, issue.id)?;
    }

    tx.commit()?;

    // Build detail for output
    let config = UrgencyConfig::load(conn);
    build_issue_detail(conn, issue, &config)
}

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
    let request = if stdin_json {
        let mut input = String::new();
        io::stdin().read_to_string(&mut input)?;
        parse_stdin_json(&input)?
    } else {
        let title = title.ok_or_else(|| ItrError::InvalidValue {
            field: "title".to_string(),
            value: String::new(),
            valid: "non-empty string".to_string(),
        })?;
        let mut files_vec: Vec<String> = files
            .as_deref()
            .map(util::parse_comma_list)
            .unwrap_or_default();
        files_vec.extend(
            file.into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
        let mut tags_vec: Vec<String> = tags
            .as_deref()
            .map(util::parse_comma_list)
            .unwrap_or_default();
        tags_vec.extend(
            tag.into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
        let mut skills_vec: Vec<String> = skills
            .as_deref()
            .map(util::parse_comma_list_lower)
            .unwrap_or_default();
        skills_vec.extend(
            skill
                .into_iter()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty()),
        );
        let (blocked_by_ids, invalid_blocked_by) = parse_blocked_by_tokens(blocked_by);
        let review_notes: Vec<String> = invalid_blocked_by
            .iter()
            .map(|token| {
                format!(
                    "REVIEW: blocked_by '{}' is not a valid issue ID and was ignored. Valid: comma-separated integer IDs",
                    token
                )
            })
            .collect();
        AddRequest {
            title,
            priority: priority.to_string(),
            kind: kind.to_string(),
            context: context.unwrap_or_default(),
            files: files_vec,
            tags: tags_vec,
            skills: skills_vec,
            acceptance: acceptance.unwrap_or_default(),
            parent_id: parent,
            assigned_to: assigned_to.unwrap_or_default(),
            blocked_by_ids,
            review_notes,
        }
    };

    let detail = execute(conn, request)?;
    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
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

    fn request(title: &str) -> AddRequest {
        AddRequest {
            title: title.to_string(),
            priority: "medium".to_string(),
            kind: "task".to_string(),
            context: String::new(),
            files: vec![],
            tags: vec![],
            skills: vec![],
            acceptance: String::new(),
            parent_id: None,
            assigned_to: String::new(),
            blocked_by_ids: vec![],
            review_notes: vec![],
        }
    }

    // --- #165: stdin-json blocked_by parity with batch add ---

    #[test]
    fn stdin_json_string_blocked_by_creates_dependency() {
        let conn = open_test_db();
        seed(&conn, "one");
        seed(&conn, "two");
        let three = seed(&conn, "three");
        let req = parse_stdin_json(r#"{"title":"t","blocked_by":["3"]}"#).unwrap();
        let detail = execute(&conn, req).unwrap();
        assert_eq!(detail.blocked_by, vec![three]);
        assert!(detail.is_blocked);
    }

    #[test]
    fn stdin_json_mixed_int_and_string_blocked_by() {
        let conn = open_test_db();
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        let req =
            parse_stdin_json(&format!(r#"{{"title":"t","blocked_by":["{a}",{b}]}}"#)).unwrap();
        let detail = execute(&conn, req).unwrap();
        let mut blockers = detail.blocked_by.clone();
        blockers.sort_unstable();
        assert_eq!(blockers, vec![a, b]);
    }

    #[test]
    fn stdin_json_invalid_blocked_by_reviewed_not_dropped() {
        let conn = open_test_db();
        let req = parse_stdin_json(r#"{"title":"t","blocked_by":["junk","@0"]}"#).unwrap();
        let detail = execute(&conn, req).unwrap();
        assert!(detail.blocked_by.is_empty());
        assert!(detail.issue.tags.contains(&"_needs_review".to_string()));
        let notes: Vec<&str> = detail.notes.iter().map(|n| n.content.as_str()).collect();
        assert!(notes.iter().any(|n| n.contains("junk")));
        assert!(notes.iter().any(|n| n.contains("@0")));
    }

    // --- #150: `parent` alias accepted on the stdin-json path too ---

    #[test]
    fn stdin_json_parent_alias_links_parent() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let req = parse_stdin_json(&format!(r#"{{"title":"child","parent":{epic}}}"#)).unwrap();
        let detail = execute(&conn, req).unwrap();
        assert_eq!(detail.issue.parent_id, Some(epic));
    }

    #[test]
    fn stdin_json_unknown_field_emits_review_note() {
        let conn = open_test_db();
        let req = parse_stdin_json(r#"{"title":"t","priorty":"high"}"#).unwrap();
        let detail = execute(&conn, req).unwrap();
        assert!(detail.issue.tags.contains(&"_needs_review".to_string()));
        assert!(detail.notes.iter().any(|n| n.content.contains("priorty")));
    }

    // --- #167: nonexistent --parent gets soft fallback, not a FK error ---

    #[test]
    fn missing_parent_creates_parentless_with_review() {
        let conn = open_test_db();
        let mut req = request("orphan");
        req.parent_id = Some(9999);
        let detail = execute(&conn, req).unwrap();
        assert_eq!(detail.issue.parent_id, None);
        assert!(detail.issue.tags.contains(&"_needs_review".to_string()));
        assert!(detail
            .notes
            .iter()
            .any(|n| n.content.contains("parent 9999")));
    }

    #[test]
    fn existing_parent_still_links() {
        let conn = open_test_db();
        let epic = seed(&conn, "Epic");
        let mut req = request("child");
        req.parent_id = Some(epic);
        let detail = execute(&conn, req).unwrap();
        assert_eq!(detail.issue.parent_id, Some(epic));
        assert!(detail.notes.is_empty());
    }

    // --- documented CLI contract: missing blocked_by ID stays a hard error ---

    #[test]
    fn missing_blocked_by_id_rolls_back() {
        let conn = open_test_db();
        let mut req = request("blocked");
        req.blocked_by_ids = vec![999];
        let err = execute(&conn, req).unwrap_err();
        assert!(matches!(err, ItrError::NotFound(999)));
        assert!(
            !db::issue_exists(&conn, 1).unwrap(),
            "insert must roll back"
        );
    }
}
