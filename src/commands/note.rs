use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::util;
use rusqlite::Connection;
use std::env;

/// Resolve the acting agent name: explicit flag, else `ITR_AGENT`, else empty.
pub(crate) fn resolve_agent(agent: &str) -> String {
    if agent.is_empty() {
        env::var("ITR_AGENT").unwrap_or_default()
    } else {
        agent.to_string()
    }
}

/// Render one note in the line-oriented output shape shared by the single-,
/// multi-ID, and bulk paths.
pub(crate) fn format_note_line(note: &crate::models::Note) -> String {
    let agent_str = if note.agent.is_empty() {
        String::new()
    } else {
        format!(" ({})", note.agent)
    };
    format!(
        "NOTE:{} ISSUE:{}{} {}",
        note.id, note.issue_id, agent_str, note.content
    )
}

/// `itr note <ID>... <TEXT>` — one or more issue IDs, repeated,
/// comma-separated, or inclusive `A-B` ranges, followed by the note text.
///
/// - Exactly one unique ID: unchanged single-issue contract (hard `NOT_FOUND`
///   for a missing issue, `INVALID_VALUE` for missing text).
/// - Multiple unique IDs: one note per issue in a single transaction with
///   per-ID soft fallback — a missing ID emits `REVIEW: id N not found;
///   skipped`. Exit 0 if at least one note was added, exit 1 if none were.
pub fn run_multi(
    conn: &Connection,
    id_tokens: &[String],
    text: Option<String>,
    agent: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    let parsed = util::parse_id_tokens(id_tokens);
    for note in &parsed.notes {
        eprintln!("{}", note);
    }
    for token in &parsed.invalid {
        eprintln!(
            "REVIEW: ignoring non-integer issue ID '{}' — IDs may be repeated, comma-separated, or ranges (e.g. `itr note 55 56 57 \"text\"`)",
            token
        );
    }
    for id in &parsed.duplicates {
        eprintln!(
            "REVIEW: duplicate issue ID {} requested; noting it once",
            id
        );
    }
    if parsed.ids.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid:
                "integer issue IDs, repeated, comma-separated, or ranges (e.g. `itr note 55 56 57 \"text\"`)"
                    .to_string(),
        });
    }

    if parsed.ids.len() == 1 {
        return run(conn, parsed.ids[0], text, agent, fmt);
    }

    let Some(content) = text else {
        return Err(ItrError::InvalidValue {
            field: "text".to_string(),
            value: String::new(),
            valid: "non-empty string".to_string(),
        });
    };
    let agent = resolve_agent(agent);

    let tx = conn.unchecked_transaction()?;
    let mut notes = Vec::new();
    for &id in &parsed.ids {
        match db::add_note(&tx, id, &content, &agent) {
            Ok(note) => notes.push(note),
            Err(ItrError::NotFound(_)) => {
                eprintln!("REVIEW: id {} not found; skipped", id);
            }
            Err(e) => return Err(e),
        }
    }
    if notes.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid: "at least one existing issue ID".to_string(),
        });
    }
    tx.commit()?;

    match fmt {
        Format::Json => {
            println!("{}", serde_json::to_string(&notes)?);
        }
        _ => {
            for note in &notes {
                println!("{}", format_note_line(note));
            }
        }
    }
    Ok(())
}

pub fn run(
    conn: &Connection,
    id: i64,
    text: Option<String>,
    agent: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    // Fall back to ITR_AGENT if agent is empty
    let agent = resolve_agent(agent);
    let Some(content) = text else {
        return Err(ItrError::InvalidValue {
            field: "text".to_string(),
            value: String::new(),
            valid: "non-empty string".to_string(),
        });
    };

    let note = db::add_note(conn, id, &content, &agent)?;

    match fmt {
        Format::Json => {
            println!("{}", serde_json::to_string(&note)?);
        }
        _ => {
            println!("{}", format_note_line(&note));
        }
    }

    Ok(())
}

pub fn run_delete(conn: &Connection, note_id: i64, fmt: Format) -> Result<(), ItrError> {
    let note = db::delete_note(conn, note_id)?;

    // Record event for audit trail
    db::record_event(conn, note.issue_id, "note_deleted", &note.content, "")?;

    match fmt {
        Format::Json => {
            println!("{}", serde_json::to_string(&note)?);
        }
        _ => {
            println!("DELETED NOTE:{} ISSUE:{}", note.id, note.issue_id);
        }
    }

    Ok(())
}

pub fn run_update(
    conn: &Connection,
    note_id: i64,
    text: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    let old_note = db::get_note(conn, note_id)?;

    // Record event for audit trail
    db::record_event(
        conn,
        old_note.issue_id,
        "note_updated",
        &old_note.content,
        text,
    )?;

    let note = db::update_note(conn, note_id, text)?;

    match fmt {
        Format::Json => {
            println!("{}", serde_json::to_string(&note)?);
        }
        _ => {
            let agent_str = if note.agent.is_empty() {
                String::new()
            } else {
                format!(" ({})", note.agent)
            };
            println!(
                "NOTE:{} ISSUE:{}{} {}",
                note.id, note.issue_id, agent_str, note.content
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        .expect("insert issue")
        .id
    }

    fn note_texts(conn: &Connection, id: i64) -> Vec<String> {
        db::get_notes(conn, id)
            .unwrap()
            .into_iter()
            .map(|n| n.content)
            .collect()
    }

    #[test]
    fn run_multi_notes_every_id_and_records_events() {
        let conn = db::open_test_db();
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        run_multi(
            &conn,
            &[a.to_string(), b.to_string()],
            Some("verified end-to-end".to_string()),
            "fable-review",
            Format::Compact,
        )
        .expect("multi note");
        for id in [a, b] {
            assert_eq!(note_texts(&conn, id), vec!["verified end-to-end"]);
            let events = db::get_events_for_issue(&conn, id).expect("events");
            assert!(
                events.iter().any(|e| e.field == "note_added"),
                "note mutations must appear in the audit log (#35)"
            );
        }
    }

    #[test]
    fn run_multi_skips_missing_ids() {
        let conn = db::open_test_db();
        let a = seed(&conn, "a");
        run_multi(
            &conn,
            &[a.to_string(), "999".to_string()],
            Some("hi".to_string()),
            "",
            Format::Compact,
        )
        .expect("soft fallback");
        assert_eq!(note_texts(&conn, a), vec!["hi"]);
    }

    #[test]
    fn run_multi_all_missing_is_exit_1() {
        let conn = db::open_test_db();
        seed(&conn, "other");
        let err = run_multi(
            &conn,
            &["998".to_string(), "999".to_string()],
            Some("hi".to_string()),
            "",
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { .. }));
    }

    #[test]
    fn run_multi_requires_text() {
        let conn = db::open_test_db();
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        let err = run_multi(
            &conn,
            &[a.to_string(), b.to_string()],
            None,
            "",
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { ref field, .. } if field == "text"));
        assert!(note_texts(&conn, a).is_empty(), "nothing may be written");
    }

    #[test]
    fn run_multi_single_missing_id_stays_hard_not_found() {
        let conn = db::open_test_db();
        let err = run_multi(
            &conn,
            &["999".to_string()],
            Some("hi".to_string()),
            "",
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::NotFound(999)));
    }
}
