use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::{ExportData, Issue};
use rusqlite::{params, Connection};
use std::collections::HashSet;
use std::fs;
use std::io::{self, BufRead};

/// Counters produced by a single import run.
#[derive(Debug, Default)]
struct ImportCounts {
    imported: usize,
    skipped: usize,
    /// Existing issues overwritten by ID collision in non-merge (replace) mode.
    replaced: usize,
    notes: usize,
    dependencies: usize,
    events: usize,
    relations: usize,
    /// Parent links dropped because the parent exists in neither the
    /// payload nor the database.
    dangling_parents: usize,
    /// Blocker edges dropped for the same reason (or self-references).
    dangling_blockers: usize,
    /// Relation rows dropped for the same reason (or self-references).
    dangling_relations: usize,
}

impl ImportCounts {
    fn dangling_total(&self) -> usize {
        self.dangling_parents + self.dangling_blockers + self.dangling_relations
    }
}

/// Tracks which issue IDs are known to exist in the database during pass two.
///
/// Every accepted payload ID is seeded up front; anything else is looked up
/// once and cached so dangling-reference checks stay cheap on large imports.
struct KnownIssues<'a> {
    conn: &'a Connection,
    present: HashSet<i64>,
    absent: HashSet<i64>,
}

impl<'a> KnownIssues<'a> {
    fn new(conn: &'a Connection, seed: impl IntoIterator<Item = i64>) -> Self {
        Self {
            conn,
            present: seed.into_iter().collect(),
            absent: HashSet::new(),
        }
    }

    fn contains(&mut self, id: i64) -> bool {
        if self.present.contains(&id) {
            return true;
        }
        if self.absent.contains(&id) {
            return false;
        }
        let exists = db::issue_exists(self.conn, id).unwrap_or(false);
        if exists {
            self.present.insert(id);
        } else {
            self.absent.insert(id);
        }
        exists
    }
}

/// Core import logic, separated from I/O so it is unit-testable.
///
/// Runs in two passes inside one transaction so exports can be restored
/// regardless of ID order:
///
/// 1. Every issue row lands under its original ID (so `blocked_by`,
///    `parent_id`, and relation references stay meaningful) with its parent
///    link in place; foreign keys are deferred to commit so a parent may
///    appear later in the payload. In non-merge mode an ID collision updates the existing row in place
///    and clears only the child rows the payload re-supplies (notes, events,
///    incoming blockers, relations). Child issues keep their `parent_id` and
///    outgoing blocker edges survive. In merge mode a collision is skipped.
/// 2. Blocker edges, notes, audit events, and relations are attached now
///    that every referenced issue exists. Notes, events, and
///    relations get fresh row IDs; nothing in the export format references
///    them by ID, and reusing source rowids would clobber unrelated rows.
///
/// References to issues that exist in neither the payload nor the database
/// are dropped and counted (soft fallback) instead of failing the import.
fn import_items(
    conn: &Connection,
    items: &[ExportData],
    merge: bool,
) -> Result<ImportCounts, ItrError> {
    let tx = conn.unchecked_transaction()?;
    // Parents may appear later in the payload than their children, so
    // foreign keys are checked at commit rather than per statement.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    // Restoring an export must keep its `updated_at` values verbatim; the
    // touch trigger would re-stamp every replaced row. It is recreated
    // before commit (and comes back on its own if the transaction rolls
    // back, since SQLite DDL is transactional).
    db::suspend_updated_at_trigger(&tx)?;
    let mut counts = ImportCounts::default();

    // Every payload ID exists once pass 1 finishes: it is inserted,
    // replaced, or skipped because it already existed.
    let mut known = KnownIssues::new(&tx, items.iter().map(|item| item.issue.id));

    // Pass 1: issue rows (with parent links, checked at commit).
    let mut accepted: Vec<&ExportData> = Vec::with_capacity(items.len());
    for item in items {
        let issue = &item.issue;
        let exists = db::issue_exists(&tx, issue.id).unwrap_or(false);

        if merge && exists {
            counts.skipped += 1;
            continue;
        }

        let parent_id = match issue.parent_id {
            Some(parent) if parent != issue.id && known.contains(parent) => Some(parent),
            Some(_) => {
                counts.dangling_parents += 1;
                None
            }
            None => None,
        };

        if exists {
            counts.replaced += 1;
            replace_issue_row(&tx, issue, parent_id)?;
        } else {
            insert_issue_row(&tx, issue, parent_id)?;
        }

        // Keep imported issues searchable: index into FTS the same way
        // db::insert_issue does, so search works without a manual reindex.
        db::fts_index_issue(&tx, issue);

        accepted.push(item);
        counts.imported += 1;
    }

    // Pass 2: child rows and edges, now that every referenced issue exists.
    for item in &accepted {
        let issue = &item.issue;

        for note in &item.notes {
            tx.execute(
                "INSERT INTO notes (issue_id, content, agent, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![issue.id, note.content, note.agent, note.created_at],
            )?;
            counts.notes += 1;
        }

        for blocker_id in &item.blocked_by {
            if *blocker_id == issue.id || !known.contains(*blocker_id) {
                counts.dangling_blockers += 1;
                continue;
            }
            counts.dependencies += tx.execute(
                "INSERT OR IGNORE INTO dependencies (blocker_id, blocked_id) VALUES (?1, ?2)",
                params![blocker_id, issue.id],
            )?;
        }

        for event in &item.events {
            tx.execute(
                "INSERT INTO events (issue_id, field, old_value, new_value, agent, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    issue.id,
                    event.field,
                    event.old_value,
                    event.new_value,
                    event.agent,
                    event.created_at,
                ],
            )?;
            counts.events += 1;
        }

        // Exports list each relation under both endpoints; the UNIQUE
        // constraint plus OR IGNORE collapses the duplicate to one row.
        for relation in &item.relations {
            if relation.source_id == relation.target_id
                || !known.contains(relation.source_id)
                || !known.contains(relation.target_id)
            {
                counts.dangling_relations += 1;
                continue;
            }
            counts.relations += tx.execute(
                "INSERT OR IGNORE INTO relations (source_id, target_id, relation_type, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![
                    relation.source_id,
                    relation.target_id,
                    relation.relation_type,
                    relation.created_at,
                ],
            )?;
        }
    }

    db::restore_updated_at_trigger(&tx)?;
    tx.commit()?;
    Ok(counts)
}

fn issue_json_columns(issue: &Issue) -> Result<(String, String, String), ItrError> {
    Ok((
        serde_json::to_string(&issue.files)?,
        serde_json::to_string(&issue.tags)?,
        serde_json::to_string(&issue.skills)?,
    ))
}

/// Insert a brand-new issue row under its exported ID. `parent_id` is the
/// already-validated link (None when the parent was dangling).
fn insert_issue_row(
    tx: &Connection,
    issue: &Issue,
    parent_id: Option<i64>,
) -> Result<(), ItrError> {
    let (files_json, tags_json, skills_json) = issue_json_columns(issue)?;
    tx.execute(
        "INSERT INTO issues (id, title, status, priority, kind, context, files, tags, skills, acceptance, parent_id, close_reason, created_at, updated_at, assigned_to)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?15, ?11, ?12, ?13, ?14)",
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
            issue.close_reason,
            issue.created_at,
            issue.updated_at,
            issue.assigned_to,
            parent_id,
        ],
    )?;
    Ok(())
}

/// Replace an existing issue in place. An UPDATE (rather than
/// `INSERT OR REPLACE`, which is delete-then-insert) keeps the row's
/// identity so `ON DELETE CASCADE` / `ON DELETE SET NULL` never fire: child
/// issues keep their `parent_id` and edges where this issue is the blocker
/// survive. Only the child rows the payload re-supplies are cleared.
fn replace_issue_row(
    tx: &Connection,
    issue: &Issue,
    parent_id: Option<i64>,
) -> Result<(), ItrError> {
    let (files_json, tags_json, skills_json) = issue_json_columns(issue)?;
    tx.execute(
        "UPDATE issues SET title = ?2, status = ?3, priority = ?4, kind = ?5, context = ?6,
             files = ?7, tags = ?8, skills = ?9, acceptance = ?10, parent_id = ?15,
             close_reason = ?11, created_at = ?12, updated_at = ?13, assigned_to = ?14
         WHERE id = ?1",
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
            issue.close_reason,
            issue.created_at,
            issue.updated_at,
            issue.assigned_to,
            parent_id,
        ],
    )?;
    tx.execute("DELETE FROM notes WHERE issue_id = ?1", params![issue.id])?;
    tx.execute("DELETE FROM events WHERE issue_id = ?1", params![issue.id])?;
    tx.execute(
        "DELETE FROM dependencies WHERE blocked_id = ?1",
        params![issue.id],
    )?;
    tx.execute(
        "DELETE FROM relations WHERE source_id = ?1 OR target_id = ?1",
        params![issue.id],
    )?;
    Ok(())
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

    if counts.dangling_total() > 0 {
        let mut parts: Vec<String> = Vec::new();
        if counts.dangling_parents > 0 {
            parts.push(format!("{} parent link(s)", counts.dangling_parents));
        }
        if counts.dangling_blockers > 0 {
            parts.push(format!("{} blocker edge(s)", counts.dangling_blockers));
        }
        if counts.dangling_relations > 0 {
            parts.push(format!("{} relation(s)", counts.dangling_relations));
        }
        eprintln!(
            "REVIEW: import dropped {} because the referenced issue exists in \
             neither the import payload nor the database (or references itself). \
             The issues themselves were imported.",
            parts.join(", ")
        );
    }

    if counts.replaced > 0 {
        eprintln!(
            "REVIEW: import replaced {} existing issue(s) whose IDs collided \
             with the imported data; their notes, audit events, incoming \
             blockers, and relations now match the payload (child issues and \
             outgoing blocker edges were kept). Pass --merge to keep existing \
             issues and skip colliding IDs instead.",
            counts.replaced
        );
    }

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "import",
                "imported": counts.imported,
                "skipped": counts.skipped,
                "replaced": counts.replaced,
                "notes": counts.notes,
                "dependencies": counts.dependencies,
                "events": counts.events,
                "relations": counts.relations,
                "dropped_references": counts.dangling_total(),
            });
            println!("{}", out);
        }
        _ => {
            println!(
                "IMPORT: {} imported, {} skipped, {} replaced (notes {}, dependencies {}, events {}, relations {})",
                counts.imported,
                counts.skipped,
                counts.replaced,
                counts.notes,
                counts.dependencies,
                counts.events,
                counts.relations
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Event, Issue, Note, Relation};
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

    fn export_event(issue_id: i64, field: &str, new_value: &str) -> Event {
        Event {
            id: 0,
            issue_id,
            field: field.to_string(),
            old_value: String::new(),
            new_value: new_value.to_string(),
            agent: "bot".to_string(),
            created_at: "2026-01-02T00:00:00Z".to_string(),
        }
    }

    fn export_relation(source_id: i64, target_id: i64, relation_type: &str) -> Relation {
        Relation {
            id: 0,
            source_id,
            target_id,
            relation_type: relation_type.to_string(),
            created_at: "2026-01-03T00:00:00Z".to_string(),
        }
    }

    /// #263: parents and blockers that appear later in the payload (higher
    /// IDs, as every real export has) must import into a fresh DB.
    #[test]
    fn import_resolves_forward_parent_and_blocker_references() {
        let (conn, path) = test_db("forward-refs");

        let mut child = export_item(10, "Child", vec![]);
        child.issue.parent_id = Some(20);
        child.blocked_by = vec![30];
        let parent = export_item(20, "Parent (epic later in file)", vec![]);
        let blocker = export_item(30, "Blocker later in file", vec![]);

        let counts = import_items(&conn, &[child, parent, blocker], false).unwrap();
        assert_eq!(counts.imported, 3);
        assert_eq!(counts.dependencies, 1);
        assert_eq!(counts.dangling_parents, 0);
        assert_eq!(counts.dangling_blockers, 0);

        assert_eq!(db::get_issue(&conn, 10).unwrap().parent_id, Some(20));
        assert_eq!(db::get_blockers(&conn, 10).unwrap(), vec![30]);
        // Exported timestamps survive verbatim (no touch-trigger re-stamp).
        assert_eq!(
            db::get_issue(&conn, 10).unwrap().updated_at,
            "2026-01-01T00:00:00Z"
        );
        // The touch trigger is back in place after commit.
        let trigger_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = 'trg_issues_updated_at'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(trigger_count, 1, "updated_at trigger must be restored");

        cleanup(&path);
    }

    /// #263: events and relations round-trip. Relations are listed under
    /// both endpoints in an export and must collapse to one row.
    #[test]
    fn import_restores_events_and_relations() {
        let (conn, path) = test_db("events-relations");

        let mut a = export_item(1, "A", vec![]);
        a.events = vec![
            export_event(1, "status", "in-progress"),
            export_event(1, "priority", "high"),
        ];
        a.relations = vec![export_relation(1, 2, "duplicate")];
        let mut b = export_item(2, "B", vec![]);
        b.relations = vec![export_relation(1, 2, "duplicate")];

        let counts = import_items(&conn, &[a, b], false).unwrap();
        assert_eq!(counts.events, 2);
        assert_eq!(counts.relations, 1, "mirrored relation must dedupe");

        let events = db::get_events_for_issue(&conn, 1).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].field, "status");
        assert_eq!(events[0].new_value, "in-progress");
        assert_eq!(events[0].agent, "bot");
        assert_eq!(events[0].created_at, "2026-01-02T00:00:00Z");

        let rels = db::get_relations(&conn, 2).unwrap();
        assert_eq!(rels.len(), 1);
        assert_eq!((rels[0].source_id, rels[0].target_id), (1, 2));
        assert_eq!(rels[0].relation_type, "duplicate");
        assert_eq!(rels[0].created_at, "2026-01-03T00:00:00Z");

        cleanup(&path);
    }

    /// #263: references to issues that exist nowhere are dropped and
    /// counted; the issue itself still imports (soft fallback).
    #[test]
    fn import_drops_and_counts_dangling_references() {
        let (conn, path) = test_db("dangling");

        let mut item = export_item(5, "Orphan refs", vec![]);
        item.issue.parent_id = Some(999);
        item.blocked_by = vec![998, 5];
        item.relations = vec![export_relation(5, 997, "related")];

        let counts = import_items(&conn, &[item], false).unwrap();
        assert_eq!(counts.imported, 1);
        assert_eq!(counts.dangling_parents, 1);
        assert_eq!(counts.dangling_blockers, 2, "missing + self-reference");
        assert_eq!(counts.dangling_relations, 1);
        assert_eq!(counts.dependencies, 0);
        assert_eq!(counts.relations, 0);
        assert_eq!(db::get_issue(&conn, 5).unwrap().parent_id, None);

        cleanup(&path);
    }

    /// #223/#263: replacing an existing issue must not cascade away rows
    /// the payload does not own: children keep their parent, outgoing
    /// blocker edges survive.
    #[test]
    fn replace_import_keeps_children_and_outgoing_edges() {
        let (conn, path) = test_db("replace-no-cascade");

        let epic = seed_issue(&conn, "Epic");
        let child = seed_issue(&conn, "Child");
        conn.execute(
            "UPDATE issues SET parent_id = ?1 WHERE id = ?2",
            params![epic.id, child.id],
        )
        .unwrap();
        let downstream = seed_issue(&conn, "Downstream");
        db::add_dependency(&conn, epic.id, downstream.id).unwrap();
        db::add_note(&conn, epic.id, "old note", "alice").unwrap();

        let mut item = export_item(epic.id, "Epic (restored)", vec![]);
        item.issue.updated_at = "2025-06-01T00:00:00Z".to_string();
        let counts = import_items(&conn, &[item], false).unwrap();
        assert_eq!(counts.replaced, 1);

        let restored = db::get_issue(&conn, epic.id).unwrap();
        assert_eq!(restored.title, "Epic (restored)");
        assert_eq!(restored.updated_at, "2025-06-01T00:00:00Z");
        assert_eq!(
            db::get_issue(&conn, child.id).unwrap().parent_id,
            Some(epic.id),
            "child lost its parent on replace"
        );
        assert_eq!(
            db::get_blockers(&conn, downstream.id).unwrap(),
            vec![epic.id],
            "outgoing blocker edge lost on replace"
        );
        // Rows the payload owns are replaced by the payload's (empty) set.
        assert!(db::get_notes(&conn, epic.id).unwrap().is_empty());

        cleanup(&path);
    }
}
