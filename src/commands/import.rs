use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::ExportData;
use rusqlite::{params, Connection};
use std::fs;
use std::io::{self, BufRead};

/// Counters produced by a single import run.
#[derive(Debug, Default)]
struct ImportCounts {
    imported: usize,
    skipped: usize,
    /// Existing issues overwritten by ID collision in non-merge (replace) mode.
    replaced: usize,
    dropped_events: usize,
    dropped_relations: usize,
}

/// Core import logic, separated from I/O so it is unit-testable.
///
/// Inserts each item's issue row (keeping its original ID for `blocked_by`
/// fidelity), indexes it into FTS, and attaches its notes under fresh note
/// IDs. In non-merge mode an ID collision replaces the existing issue; in
/// merge mode it is skipped.
fn import_items(
    conn: &Connection,
    items: &[ExportData],
    merge: bool,
) -> Result<ImportCounts, ItrError> {
    let tx = conn.unchecked_transaction()?;
    let mut counts = ImportCounts::default();

    for item in items {
        let issue = &item.issue;
        let exists = db::issue_exists(&tx, issue.id).unwrap_or(false);

        if merge && exists {
            counts.skipped += 1;
            continue;
        }
        if exists {
            counts.replaced += 1;
        }

        // Soft fallback: import does not restore audit events or relation
        // rows yet. Count them so we can surface a single REVIEW: warning
        // on stderr after the transaction commits.
        counts.dropped_events += item.events.len();
        counts.dropped_relations += item.relations.len();

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

        // Keep imported issues searchable: index into FTS the same way
        // db::insert_issue does, so search works without a manual reindex.
        db::fts_index_issue(&tx, issue);

        // Import notes under FRESH note IDs. Nothing in the export format
        // references note IDs, and reusing the source DB's rowids would
        // silently overwrite unrelated pre-existing notes on ID collision.
        for note in &item.notes {
            tx.execute(
                "INSERT INTO notes (issue_id, content, agent, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![issue.id, note.content, note.agent, note.created_at],
            )?;
        }

        // Import dependencies
        for blocker_id in &item.blocked_by {
            let _ = tx.execute(
                "INSERT OR IGNORE INTO dependencies (blocker_id, blocked_id) VALUES (?1, ?2)",
                params![blocker_id, issue.id],
            );
        }

        counts.imported += 1;
    }

    tx.commit()?;
    Ok(counts)
}

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

    let counts = import_items(conn, &items, merge)?;

    if counts.dropped_events > 0 || counts.dropped_relations > 0 {
        let mut parts: Vec<String> = Vec::new();
        if counts.dropped_events > 0 {
            parts.push(format!("events ({} row(s))", counts.dropped_events));
        }
        if counts.dropped_relations > 0 {
            parts.push(format!("relations ({} row(s))", counts.dropped_relations));
        }
        eprintln!(
            "REVIEW: import dropped data from unsupported tables: {}. \
             Round-trip restore of audit history and relation rows is not \
             implemented; use a direct .itr.db file copy for full-fidelity \
             backups. See docs/backup-import-export.md.",
            parts.join(", ")
        );
    }

    if counts.replaced > 0 {
        eprintln!(
            "REVIEW: import replaced {} existing issue(s) whose IDs collided \
             with the imported data. Pass --merge to keep existing issues and \
             skip colliding IDs instead.",
            counts.replaced
        );
    }

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "import",
                "imported": counts.imported,
                "skipped": counts.skipped,
            });
            println!("{}", out);
        }
        _ => {
            println!(
                "IMPORT: {} imported, {} skipped",
                counts.imported, counts.skipped
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Issue, Note};
    use std::path::{Path, PathBuf};

    /// Open a fresh on-disk test DB (`init_db` runs the same schema,
    /// migrations, and FTS setup as production). In-memory DBs are avoided
    /// so the FTS table is created exactly the way `itr init` creates it.
    fn test_db(name: &str) -> (Connection, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "itr-import-unit-{}-{}.db",
            std::process::id(),
            name
        ));
        cleanup(&path);
        let conn = db::init_db(&path).expect("init test db");
        (conn, path)
    }

    fn cleanup(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(format!("{}-wal", path.display()));
        let _ = fs::remove_file(format!("{}-shm", path.display()));
    }

    fn seed_issue(conn: &Connection, title: &str) -> Issue {
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
        .expect("seed issue")
    }

    fn export_item(id: i64, title: &str, notes: Vec<Note>) -> ExportData {
        ExportData {
            issue: Issue {
                id,
                title: title.to_string(),
                status: "open".to_string(),
                priority: "medium".to_string(),
                kind: "task".to_string(),
                context: String::new(),
                files: vec![],
                tags: vec![],
                skills: vec![],
                acceptance: String::new(),
                parent_id: None,
                assigned_to: String::new(),
                close_reason: String::new(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                updated_at: "2026-01-01T00:00:00Z".to_string(),
            },
            notes,
            blocked_by: vec![],
            events: vec![],
            relations: vec![],
        }
    }

    fn export_note(id: i64, issue_id: i64, content: &str) -> Note {
        Note {
            id,
            issue_id,
            content: content.to_string(),
            agent: "exporter".to_string(),
            created_at: "2026-01-02T00:00:00Z".to_string(),
        }
    }

    /// #153: a note-ID collision under --merge must not modify or delete
    /// any pre-existing note row; imported notes get fresh IDs.
    #[test]
    fn merge_import_note_id_collision_preserves_existing_notes() {
        let (conn, path) = test_db("note-collision");

        let existing = seed_issue(&conn, "Existing issue");
        let original = db::add_note(&conn, existing.id, "original note", "alice").unwrap();
        assert_eq!(original.id, 1, "test setup: existing note must have id 1");

        // Imported issue 100 carries a note whose source ID collides (id 1).
        let item = export_item(
            100,
            "Imported issue",
            vec![export_note(1, 100, "imported note")],
        );
        let counts = import_items(&conn, &[item], true).unwrap();
        assert_eq!(counts.imported, 1);
        assert_eq!(counts.skipped, 0);

        // Pre-existing note row is untouched.
        let kept = db::get_note(&conn, original.id).unwrap();
        assert_eq!(kept.issue_id, existing.id, "existing note was reassigned");
        assert_eq!(kept.content, "original note", "existing note was rewritten");
        assert_eq!(kept.agent, "alice");

        // Imported note attaches to the imported issue under a fresh ID.
        let imported_notes = db::get_notes(&conn, 100).unwrap();
        assert_eq!(imported_notes.len(), 1);
        assert_eq!(imported_notes[0].content, "imported note");
        assert_ne!(imported_notes[0].id, original.id);

        cleanup(&path);
    }

    /// #153: same guarantee without --merge when the colliding note belongs
    /// to an unrelated issue that is NOT being replaced.
    #[test]
    fn replace_import_note_id_collision_preserves_unrelated_notes() {
        let (conn, path) = test_db("note-collision-replace");

        let existing = seed_issue(&conn, "Existing issue");
        let original = db::add_note(&conn, existing.id, "original note", "alice").unwrap();

        let item = export_item(
            100,
            "Imported issue",
            vec![export_note(original.id, 100, "imported note")],
        );
        import_items(&conn, &[item], false).unwrap();

        let kept = db::get_note(&conn, original.id).unwrap();
        assert_eq!(kept.issue_id, existing.id);
        assert_eq!(kept.content, "original note");

        let imported_notes = db::get_notes(&conn, 100).unwrap();
        assert_eq!(imported_notes.len(), 1);
        assert_eq!(imported_notes[0].content, "imported note");

        cleanup(&path);
    }

    /// #161: imported issues must be FTS-indexed immediately, so search
    /// finds them even when pre-existing indexed issues also match.
    #[test]
    fn import_indexes_issues_into_fts() {
        let (conn, path) = test_db("fts-index");
        if !db::has_fts(&conn) {
            // SQLite without FTS5: nothing to index, nothing to assert.
            cleanup(&path);
            return;
        }

        let existing = seed_issue(&conn, "widget existing");
        let item = export_item(100, "widget imported", vec![]);
        import_items(&conn, &[item], false).unwrap();

        let ids = db::fts_search(&conn, "widget").unwrap();
        assert!(
            ids.contains(&existing.id),
            "pre-existing issue missing from FTS"
        );
        assert!(
            ids.contains(&100),
            "imported issue not FTS-indexed; search omits it when other issues match"
        );

        // Doctor parity: fts_stale fires when FTS row count != issue count.
        let fts_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM issues_fts", [], |row| row.get(0))
            .unwrap();
        let issue_count = db::all_issues(&conn).unwrap().len();
        assert_eq!(
            usize::try_from(fts_count).unwrap(),
            issue_count,
            "FTS index out of sync with issues right after import (doctor fts_stale)"
        );

        cleanup(&path);
    }

    /// #189: non-merge import replaces on ID collision (never errors) and
    /// counts the replacements; merge mode skips instead.
    #[test]
    fn non_merge_import_replaces_and_counts_collisions() {
        let (conn, path) = test_db("replace-count");

        let existing = seed_issue(&conn, "Old title");
        let item = export_item(existing.id, "New title", vec![]);

        let counts = import_items(&conn, std::slice::from_ref(&item), false).unwrap();
        assert_eq!(counts.imported, 1);
        assert_eq!(counts.skipped, 0);
        assert_eq!(counts.replaced, 1, "replace-on-collision must be counted");
        assert_eq!(
            db::get_issue(&conn, existing.id).unwrap().title,
            "New title"
        );

        // Merge mode on the same payload skips and replaces nothing.
        let counts = import_items(&conn, &[item], true).unwrap();
        assert_eq!(counts.imported, 0);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.replaced, 0);

        cleanup(&path);
    }
}
