use crate::db;
use crate::error::ItrError;
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
) -> Result<(), ItrError> {
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
            .map(serde_json::from_str)
            .collect::<Result<Vec<_>, _>>()?
    };

    let tx = conn.unchecked_transaction()?;
    let mut imported = 0;
    let mut skipped = 0;
    let mut dropped_events: usize = 0;
    let mut dropped_relations: usize = 0;

    for item in &items {
        let issue = &item.issue;

        if merge && db::issue_exists(&tx, issue.id).unwrap_or(false) {
            skipped += 1;
            continue;
        }

        // Soft fallback: import does not restore audit events or relation
        // rows yet. Count them so we can surface a single REVIEW: warning
        // on stderr after the transaction commits.
        dropped_events += item.events.len();
        dropped_relations += item.relations.len();

        let files_json = serde_json::to_string(&issue.files)?;
        let tags_json = serde_json::to_string(&issue.tags)?;
        let skills_json = serde_json::to_string(&issue.skills)?;

        tx.execute(
            "INSERT OR REPLACE INTO issues (id, title, status, priority, kind, context, files, tags, skills, acceptance, parent_id, close_reason, created_at, updated_at, assigned_to)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                issue.id,
                issue.title,
                issue.status,
                issue.priority,
                issue.kind,
                issue.context,
                files_json,
                tags_json,
                skills_json,
                issue.acceptance,
                issue.parent_id,
                issue.close_reason,
                issue.created_at,
                issue.updated_at,
                issue.assigned_to,
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

    if dropped_events > 0 || dropped_relations > 0 {
        let mut parts: Vec<String> = Vec::new();
        if dropped_events > 0 {
            parts.push(format!("events ({} row(s))", dropped_events));
        }
        if dropped_relations > 0 {
            parts.push(format!("relations ({} row(s))", dropped_relations));
        }
        eprintln!(
            "REVIEW: import dropped data from unsupported tables: {}. \
             Round-trip restore of audit history and relation rows is not \
             implemented; use a direct .itr.db file copy for full-fidelity \
             backups. See docs/backup-import-export.md.",
            parts.join(", ")
        );
    }

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
