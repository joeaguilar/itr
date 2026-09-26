use crate::db;
use crate::error::ItrError;
use crate::models::ExportData;
use rusqlite::Connection;

/// Resolve `--export-format`. Case-insensitive; an unknown value falls back
/// to JSONL with a `REVIEW:` note instead of silently producing a format the
/// caller did not ask for.
fn resolve_format(export_format: &str) -> &'static str {
    match export_format.trim().to_lowercase().as_str() {
        "json" => "json",
        "jsonl" | "ndjson" => "jsonl",
        other => {
            eprintln!("REVIEW: export format '{other}' not recognized, defaulted to 'jsonl'. Valid: jsonl, json");
            "jsonl"
        }
    }
}

/// Collect every issue bundle from ONE read snapshot.
///
/// All reads run inside a single transaction, so in WAL mode they observe
/// the same committed database state: a writer that commits mid-export
/// cannot produce a bundle whose issue row and child rows come from
/// different generations (#269). The transaction only reads and is rolled
/// back when dropped.
fn collect(conn: &Connection) -> Result<Vec<ExportData>, ItrError> {
    let tx = conn.unchecked_transaction()?;
    let issues = db::all_issues(&tx)?;

    let mut export_items: Vec<ExportData> = Vec::with_capacity(issues.len());
    for issue in issues {
        let notes = db::get_notes(&tx, issue.id)?;
        let blocked_by = db::get_blockers(&tx, issue.id)?;
        let events = db::get_events_for_issue(&tx, issue.id)?;
        let relations = db::get_relations(&tx, issue.id)?;
        export_items.push(ExportData {
            issue,
            notes,
            blocked_by,
            events,
            relations,
        });
    }
    Ok(export_items)
}

pub fn run(conn: &Connection, export_format: &str) -> Result<(), ItrError> {
    let format = resolve_format(export_format);
    let export_items = collect(conn)?;

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&export_items)?);
    } else {
        // JSONL: one item per line
        for item in &export_items {
            println!("{}", serde_json::to_string(item)?);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_format_is_case_insensitive_and_defaults_to_jsonl() {
        assert_eq!(resolve_format("JSON"), "json");
        assert_eq!(resolve_format(" jsonl "), "jsonl");
        assert_eq!(resolve_format("ndjson"), "jsonl");
        assert_eq!(resolve_format("csv"), "jsonl");
    }

    #[test]
    fn collect_reads_every_table_for_each_issue() {
        let conn = db::open_test_db();
        let a = db::insert_issue(
            &conn,
            "a",
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
        .unwrap();
        let b = db::insert_issue(
            &conn,
            "b",
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
        .unwrap();
        db::add_note(&conn, a.id, "note on a", "tester").unwrap();
        db::add_dependency(&conn, a.id, b.id).unwrap();
        let items = collect(&conn).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].notes.len(), 1);
        assert_eq!(items[1].blocked_by, vec![a.id]);
        // The snapshot transaction is released: a write still works.
        db::add_note(&conn, b.id, "after export", "tester").unwrap();
    }
}
