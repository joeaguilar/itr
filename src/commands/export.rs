use crate::db;
use crate::error::ItrError;
use crate::models::ExportData;
use rusqlite::Connection;

pub fn run(conn: &Connection, export_format: &str) -> Result<(), ItrError> {
    let issues = db::all_issues(conn)?;

    let mut export_items: Vec<ExportData> = Vec::with_capacity(issues.len());
    for issue in issues {
        let notes = db::get_notes(conn, issue.id)?;
        let blocked_by = db::get_blockers(conn, issue.id)?;
        let events = db::get_events_for_issue(conn, issue.id)?;
        let relations = db::get_relations(conn, issue.id)?;
        export_items.push(ExportData {
            issue,
            notes,
            blocked_by,
            events,
            relations,
        });
    }

    match export_format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&export_items)?);
        }
        _ => {
            // JSONL: one item per line
            for item in &export_items {
                println!("{}", serde_json::to_string(item)?);
            }
        }
    }

    Ok(())
}
