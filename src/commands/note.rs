use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use rusqlite::Connection;
use std::io::{self, IsTerminal, Read};

pub fn run(
    conn: &Connection,
    id: i64,
    text: Option<String>,
    agent: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    let content = match text {
        Some(t) => t,
        None => {
            if io::stdin().is_terminal() {
                return Err(ItrError::InvalidValue {
                    field: "text".to_string(),
                    value: String::new(),
                    valid: "non-empty string or pipe via stdin".to_string(),
                });
            }
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            buf.trim().to_string()
        }
    };

    let note = db::add_note(conn, id, &content, agent)?;

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
            println!("NOTE:{} ISSUE:{}{} {}", note.id, note.issue_id, agent_str, note.content);
        }
    }

    Ok(())
}
