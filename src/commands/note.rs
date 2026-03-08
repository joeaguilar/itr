use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use rusqlite::Connection;
use std::env;
pub fn run(
    conn: &Connection,
    id: i64,
    text: Option<String>,
    agent: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    // Fall back to ITR_AGENT if agent is empty
    let agent = if agent.is_empty() {
        env::var("ITR_AGENT").unwrap_or_default()
    } else {
        agent.to_string()
    };
    let content = match text {
        Some(t) => t,
        None => {
            return Err(ItrError::InvalidValue {
                field: "text".to_string(),
                value: String::new(),
                valid: "non-empty string".to_string(),
            });
        }
    };

    let note = db::add_note(conn, id, &content, &agent)?;

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
    db::record_event(conn, old_note.issue_id, "note_updated", &old_note.content, text)?;

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
