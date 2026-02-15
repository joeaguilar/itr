use crate::db;
use crate::error::NitError;
use crate::format::Format;
use crate::models::ExportData;
use rusqlite::{params, Connection};
use std::fs;
use std::io::{self, BufRead};

pub fn run(
    conn: &Connection,
    file: Option<String>,
    merge: bool,
    fmt: Format,
) -> Result<(), NitError> {
    let input = match file {
        Some(path) => fs::read_to_string(&path)?,
        None => {
            let mut buf = String::new();
            let stdin = io::stdin();
            for line in stdin.lock().lines() {
                let line = line?;
                buf.push_str(&line);
                buf.push('\n');
            }
            buf
        }
    };

    let input = input.trim();

    // Try JSON array first, then JSONL
    let items: Vec<ExportData> = if input.starts_with('[') {
        serde_json::from_str(input)?
    } else {
        input
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l))
            .collect::<Result<Vec<_>, _>>()?
    };

    let tx = conn.unchecked_transaction()?;
    let mut imported = 0;
    let mut skipped = 0;

    for item in &items {
        let issue = &item.issue;

        if merge {
            if db::issue_exists(&tx, issue.id).unwrap_or(false) {
                skipped += 1;
                continue;
            }
        }

        let files_json = serde_json::to_string(&issue.files)?;
        let tags_json = serde_json::to_string(&issue.tags)?;

        tx.execute(
            "INSERT OR REPLACE INTO issues (id, title, status, priority, kind, context, files, tags, acceptance, parent_id, close_reason, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                issue.id,
                issue.title,
                issue.status,
                issue.priority,
                issue.kind,
                issue.context,
                files_json,
                tags_json,
                issue.acceptance,
                issue.parent_id,
                issue.close_reason,
                issue.created_at,
                issue.updated_at,
            ],
        )?;

        // Import notes
        for note in &item.notes {
            tx.execute(
                "INSERT OR REPLACE INTO notes (id, issue_id, content, agent, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![note.id, note.issue_id, note.content, note.agent, note.created_at],
            )?;
        }

        // Import dependencies
        for blocker_id in &item.blocked_by {
            let _ = tx.execute(
                "INSERT OR IGNORE INTO dependencies (blocker_id, blocked_id) VALUES (?1, ?2)",
                params![blocker_id, issue.id],
            );
        }

        imported += 1;
    }

    tx.commit()?;

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "import",
                "imported": imported,
                "skipped": skipped,
            });
            println!("{}", out);
        }
        _ => {
            println!("IMPORT: {} imported, {} skipped", imported, skipped);
        }
    }

    Ok(())
}
