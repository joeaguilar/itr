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
