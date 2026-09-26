use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::models::Issue;
use crate::normalize::{self, validate_kind, validate_priority, validate_status};
use crate::sanitize;
use rusqlite::{params, Connection};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::{self, Read};

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
    /// Relation rows dropped for the same reason, self-references, or
    /// because the relation does not involve the bundle's own issue.
    dangling_relations: usize,
    /// Parent links / blocker edges dropped because they would close a cycle.
    cycle_parents: usize,
    cycle_blockers: usize,
    /// Records that could not be parsed or had an unusable ID; nothing from
    /// them was written.
    invalid: usize,
    /// Earlier copies of an issue ID that appears more than once.
    duplicates: usize,
    /// Existing child rows removed because a replaced bundle re-supplied
    /// that collection (notes, events, incoming blockers, relations).
    replaced_rows: usize,
    /// Blocker edges from a done/wontfix issue removed after import (the
    /// close paths never leave such edges behind).
    closed_blocker_edges: usize,
}

impl ImportCounts {
    fn dangling_total(&self) -> usize {
        self.dangling_parents + self.dangling_blockers + self.dangling_relations
    }
}

/// Collects `REVIEW:` notes. Per-issue findings are grouped by message so a
/// systematic problem in a large file prints one line, not one per issue.
#[derive(Debug, Default)]
struct Reviews {
    grouped: BTreeMap<String, Vec<i64>>,
    lines: Vec<String>,
}

impl Reviews {
    fn issue(&mut self, message: impl Into<String>, id: i64) {
        let ids = self.grouped.entry(message.into()).or_default();
        if !ids.contains(&id) {
            ids.push(id);
        }
    }

    fn line(&mut self, message: impl Into<String>) {
        self.lines.push(message.into());
    }

    fn render(&self) -> Vec<String> {
        const SHOWN: usize = 10;
        let mut out: Vec<String> = self
            .lines
            .iter()
            .map(|l| format!("REVIEW: import {l}"))
            .collect();
        for (message, ids) in &self.grouped {
            let shown: Vec<String> = ids.iter().take(SHOWN).map(i64::to_string).collect();
            let more = if ids.len() > SHOWN {
                format!(" (+{} more)", ids.len() - SHOWN)
            } else {
                String::new()
            };
            out.push(format!(
                "REVIEW: import {message}: issue(s) {}{more}",
                shown.join(", ")
            ));
        }
        out
    }
}

// --- Wire format -----------------------------------------------------------
//
// These mirror `models::ExportData` but are lenient on purpose: every field
// an older or hand-written export may omit has a default, and the child
// collections are `Option` so that an ABSENT key means "leave the existing
// rows alone" on replace, while a PRESENT key (even `[]`) replaces them.

const BUNDLE_KEYS: &[&str] = &["issue", "notes", "blocked_by", "events", "relations"];
const ISSUE_KEYS: &[&str] = &[
    "id",
    "title",
    "status",
    "priority",
    "kind",
    "context",
    "files",
    "tags",
    "skills",
    "acceptance",
    "parent_id",
    "assigned_to",
    "close_reason",
    "created_at",
    "updated_at",
];

#[derive(Debug, Deserialize)]
struct BundleIn {
    issue: IssueIn,
    #[serde(default)]
    notes: Option<Vec<NoteIn>>,
    #[serde(default)]
    blocked_by: Option<Vec<i64>>,
    #[serde(default)]
    events: Option<Vec<EventIn>>,
    #[serde(default)]
    relations: Option<Vec<RelationIn>>,
}

#[derive(Debug, Deserialize)]
struct IssueIn {
    id: i64,
    #[serde(default)]
    title: String,
    status: Option<String>,
    priority: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    context: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    acceptance: String,
    parent_id: Option<i64>,
    #[serde(default)]
    assigned_to: String,
    #[serde(default)]
    close_reason: String,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// Note/event/relation row IDs (and a note's `issue_id`) are ignored: rows
/// attach to the bundle's issue and get fresh IDs on insert.
#[derive(Debug, Deserialize)]
struct NoteIn {
    content: String,
    #[serde(default)]
    agent: String,
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct EventIn {
    field: String,
    #[serde(default)]
    old_value: String,
    #[serde(default)]
    new_value: String,
    #[serde(default)]
    agent: String,
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelationIn {
    source_id: i64,
    target_id: i64,
    relation_type: String,
    created_at: Option<String>,
}

// --- Validated records -----------------------------------------------------

#[derive(Debug)]
struct NoteRow {
    content: String,
    agent: String,
    created_at: String,
}

#[derive(Debug)]
struct EventRow {
    field: String,
    old_value: String,
    new_value: String,
    agent: String,
    created_at: String,
}

#[derive(Debug)]
struct RelationRow {
    source_id: i64,
    target_id: i64,
    relation_type: String,
    created_at: String,
}

/// One bundle after validation and normalization: every value here is in
/// the shape the rest of `itr` writes (see `sanitize`).
#[derive(Debug)]
struct ImportRecord {
    issue: Issue,
    notes: Option<Vec<NoteRow>>,
    blocked_by: Option<Vec<i64>>,
    events: Option<Vec<EventRow>>,
    relations: Option<Vec<RelationRow>>,
}

/// Split the raw input into labelled JSON values.
///
/// Accepts a JSON array, JSONL (one bundle per line), or a single
/// (possibly pretty-printed) bundle object, with or without a UTF-8 BOM. A
/// malformed JSONL line is reported with its real line number and skipped;
/// only an unparseable top-level array/object is a hard error, since then
/// no record boundary can be trusted.
fn split_records(
    input: &str,
    counts: &mut ImportCounts,
    reviews: &mut Reviews,
) -> Result<Vec<(String, Value)>, ItrError> {
    let input = input.trim_start_matches('\u{feff}').trim();
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if input.starts_with('[') {
        let values: Vec<Value> = serde_json::from_str(input)?;
        return Ok(values
            .into_iter()
            .enumerate()
            .map(|(idx, v)| (format!("item {idx}"), v))
            .collect());
    }
    if let Ok(value @ Value::Object(_)) = serde_json::from_str::<Value>(input) {
        return Ok(vec![("item 0".to_string(), value)]);
    }
    let mut out = Vec::new();
    for (idx, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let label = format!("line {}", idx + 1);
        match serde_json::from_str::<Value>(line) {
            Ok(value) => out.push((label, value)),
            Err(e) => {
                counts.invalid += 1;
                reviews.line(format!("skipped {label}: not valid JSON ({e})"));
            }
        }
    }
    Ok(out)
}

/// Fields `itr get -f json` / `list -f json` add to an issue that are
/// computed from other rows; a flat issue object carries them, but they are
/// never imported (they are re-derived from the imported rows).
const DERIVED_KEYS: &[&str] = &[
    "urgency",
    "urgency_breakdown",
    "is_blocked",
    "blocks",
    "children",
];

/// Turn a flat issue object (an `IssueDetail` from `itr get -f json`) into a
/// bundle: issue columns move under `issue`, child collections stay at the
/// bundle level, derived fields are dropped, and anything else stays where
/// it is so it is reported as unknown.
fn unflatten_issue(flat: serde_json::Map<String, Value>) -> serde_json::Map<String, Value> {
    let mut issue = serde_json::Map::new();
    let mut bundle = serde_json::Map::new();
    for (key, value) in flat {
        if BUNDLE_KEYS.contains(&key.as_str()) {
            bundle.insert(key, value);
        } else if !DERIVED_KEYS.contains(&key.as_str()) {
            issue.insert(key, value);
        }
    }
    bundle.insert("issue".to_string(), Value::Object(issue));
    bundle
}

/// Normalize a timestamp field, falling back to `fallback` with a REVIEW
/// note when it is missing or unparseable.
fn timestamp_or(
    raw: Option<&str>,
    fallback: &str,
    what: &str,
    id: i64,
    reviews: &mut Reviews,
) -> String {
    match raw {
        Some(value) => {
            if let Some(ts) = sanitize::normalize_timestamp(value) {
                ts
            } else {
                reviews.issue(
                    format!("replaced unparseable {what} timestamps with the import time"),
                    id,
                );
                fallback.to_string()
            }
        }
        None => {
            reviews.issue(
                format!("set missing {what} timestamps to the import time"),
                id,
            );
            fallback.to_string()
        }
    }
}

#[allow(clippy::too_many_arguments)]
/// Normalize an enum value with the same aliases and fallback wording as
/// `add`; `None` (field absent) takes the default silently, like `add`.
fn enum_or_default(
    raw: Option<&str>,
    normalize: fn(&str) -> String,
    validate: fn(&str) -> Result<(), ItrError>,
    field: &str,
    default: &str,
    valid: &str,
    id: i64,
    reviews: &mut Reviews,
) -> String {
    let Some(raw) = raw else {
        return default.to_string();
    };
    let normalized = normalize(raw);
    if validate(&normalized).is_ok() {
        normalized
    } else {
        reviews.issue(
            format!("{field} '{raw}' not recognized, defaulted to '{default}'. Valid: {valid}"),
            id,
        );
        default.to_string()
    }
}

/// Validate one bundle. Returns `Err(reason)` only when nothing sensible can
/// be imported (not a bundle object, no usable integer ID); every other
/// problem is repaired with a REVIEW note, matching the soft-fallback rules
/// the other write paths follow.
#[allow(clippy::too_many_lines)]
fn validate_record(value: Value, now: &str, reviews: &mut Reviews) -> Result<ImportRecord, String> {
    let Value::Object(mut bundle) = value else {
        return Err("not a JSON object".to_string());
    };
    let flat = !bundle.contains_key("issue") && bundle.contains_key("id");
    if flat {
        bundle = unflatten_issue(bundle);
    }
    let unknown: Vec<String> = bundle
        .keys()
        .filter(|k| !BUNDLE_KEYS.contains(&k.as_str()))
        .cloned()
        .collect();
    let Some(Value::Object(issue_obj)) = bundle.get_mut("issue") else {
        return Err("missing the \"issue\" object".to_string());
    };
    // `parent` is accepted as an alias of `parent_id`, as in `batch add`.
    if !issue_obj.contains_key("parent_id") {
        if let Some(parent) = issue_obj.remove("parent") {
            issue_obj.insert("parent_id".to_string(), parent);
        }
    }
    // `null` means "absent" for every issue field except `parent_id`
    // (`get -f json` emits `"assigned_to": null` for unassigned issues).
    issue_obj.retain(|key, v| !v.is_null() || key == "parent_id");
    let unknown_issue: Vec<String> = issue_obj
        .keys()
        .filter(|k| !ISSUE_KEYS.contains(&k.as_str()))
        .cloned()
        .collect();

    let parsed: BundleIn =
        serde_json::from_value(Value::Object(bundle)).map_err(|e| e.to_string())?;
    let raw = parsed.issue;
    let id = raw.id;
    if !(1..=sanitize::MAX_ISSUE_ID).contains(&id) {
        return Err(format!(
            "issue id {id} is out of range. Valid: 1 to {}",
            sanitize::MAX_ISSUE_ID
        ));
    }
    if flat {
        reviews.issue(
            "read flat issue objects (the `itr get -f json` shape) as export bundles",
            id,
        );
    }
    for key in unknown {
        reviews.issue(format!("ignored unknown bundle field '{key}'"), id);
    }
    for key in unknown_issue {
        reviews.issue(format!("ignored unknown issue field '{key}'"), id);
    }

    let mut title = sanitize::clean_title(&raw.title);
    if title.is_empty() {
        reviews.issue("gave empty titles the placeholder '(untitled)'", id);
        title = "(untitled)".to_string();
    }
    let texts = [
        &raw.title,
        &raw.context,
        &raw.acceptance,
        &raw.close_reason,
        &raw.assigned_to,
    ];
    if texts.iter().any(|t| sanitize::has_control_chars(t)) {
        reviews.issue("removed control characters from issue fields", id);
    }

    let status = enum_or_default(
        raw.status.as_deref(),
        normalize::normalize_status,
        validate_status,
        "status",
        "open",
        "open, in-progress, done, wontfix",
        id,
        reviews,
    );
    let priority = enum_or_default(
        raw.priority.as_deref(),
        normalize::normalize_priority,
        validate_priority,
        "priority",
        "medium",
        "critical, high, medium, low",
        id,
        reviews,
    );
    let kind = enum_or_default(
        raw.kind.as_deref(),
        normalize::normalize_kind,
        validate_kind,
        "kind",
        "task",
        "bug, feature, task, epic",
        id,
        reviews,
    );

    let files = sanitize::clean_list(&raw.files);
    let tags = sanitize::clean_list(&raw.tags);
    let skills = sanitize::clean_skills(&raw.skills);
    if files != raw.files || tags != raw.tags || skills != raw.skills {
        reviews.issue(
            "cleaned files/tags/skills (trimmed, dropped empty and duplicate entries, lowercased skills)",
            id,
        );
    }

    let created_at = timestamp_or(raw.created_at.as_deref(), now, "created_at", id, reviews);
    let updated_at = timestamp_or(
        raw.updated_at.as_deref(),
        &created_at,
        "updated_at",
        id,
        reviews,
    );

    let issue = Issue {
        id,
        title,
        status,
        priority,
        kind,
        context: sanitize::clean_text(&raw.context),
        files,
        tags,
        skills,
        acceptance: sanitize::clean_text(&raw.acceptance),
        parent_id: raw.parent_id,
        assigned_to: sanitize::clean_assignee(&raw.assigned_to),
        close_reason: sanitize::clean_text(&raw.close_reason),
        created_at,
        updated_at,
    };

    let notes = parsed.notes.map(|notes| {
        notes
            .into_iter()
            .filter_map(|note| {
                let content = sanitize::clean_text(&note.content);
                if content.trim().is_empty() {
                    reviews.issue("dropped empty notes", id);
                    return None;
                }
                Some(NoteRow {
                    content,
                    agent: sanitize::clean_line(&note.agent),
                    created_at: timestamp_or(
                        note.created_at.as_deref(),
                        &issue.created_at,
                        "note created_at",
                        id,
                        reviews,
                    ),
                })
            })
            .collect()
    });

    let events = parsed.events.map(|events| {
        events
            .into_iter()
            .map(|event| EventRow {
                field: sanitize::clean_line(&event.field),
                old_value: event.old_value,
                new_value: event.new_value,
                agent: sanitize::clean_line(&event.agent),
                created_at: timestamp_or(
                    event.created_at.as_deref(),
                    &issue.updated_at,
                    "event created_at",
                    id,
                    reviews,
                ),
            })
            .collect()
    });

    let relations = parsed.relations.map(|relations| {
        relations
            .into_iter()
            .map(|relation| {
                let mut relation_type = sanitize::normalize_relation_type(&relation.relation_type);
                if !matches!(relation_type.as_str(), "duplicate" | "related" | "supersedes") {
                    reviews.issue(
                        format!(
                            "relation type '{}' not recognized, defaulted to 'related'. Valid: duplicate, related, supersedes",
                            relation.relation_type
                        ),
                        id,
                    );
                    relation_type = "related".to_string();
                }
                RelationRow {
                    source_id: relation.source_id,
                    target_id: relation.target_id,
                    relation_type,
                    created_at: timestamp_or(
                        relation.created_at.as_deref(),
                        &issue.created_at,
                        "relation created_at",
                        id,
                        reviews,
                    ),
                }
            })
            .collect()
    });

    Ok(ImportRecord {
        issue,
        notes,
        blocked_by: parsed.blocked_by,
        events,
        relations,
    })
}

/// Parse, validate, and de-duplicate raw input into import records.
fn prepare_records(
    input: &str,
    counts: &mut ImportCounts,
    reviews: &mut Reviews,
) -> Result<Vec<ImportRecord>, ItrError> {
    let now = sanitize::now_timestamp();
    let mut records: Vec<ImportRecord> = Vec::new();
    for (label, value) in split_records(input, counts, reviews)? {
        match validate_record(value, &now, reviews) {
            Ok(record) => records.push(record),
            Err(reason) => {
                counts.invalid += 1;
                reviews.line(format!("skipped {label}: {reason}"));
            }
        }
    }
    Ok(dedupe_records(records, counts, reviews))
}

/// An issue ID that appears more than once would otherwise be merged
/// field-by-field from different copies. Keep only the LAST copy (later
/// lines win, as in a log) and drop the earlier ones whole.
fn dedupe_records(
    records: Vec<ImportRecord>,
    counts: &mut ImportCounts,
    reviews: &mut Reviews,
) -> Vec<ImportRecord> {
    let mut last_index: HashMap<i64, usize> = HashMap::new();
    for (idx, record) in records.iter().enumerate() {
        last_index.insert(record.issue.id, idx);
    }
    records
        .into_iter()
        .enumerate()
        .filter_map(|(idx, record)| {
            if last_index[&record.issue.id] == idx {
                Some(record)
            } else {
                counts.duplicates += 1;
                reviews.issue(
                    "found the same issue ID more than once and kept only the last copy",
                    record.issue.id,
                );
                None
            }
        })
        .collect()
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

    fn contains(&mut self, id: i64) -> Result<bool, ItrError> {
        if self.present.contains(&id) {
            return Ok(true);
        }
        if self.absent.contains(&id) {
            return Ok(false);
        }
        let exists = db::issue_exists(self.conn, id)?;
        if exists {
            self.present.insert(id);
        } else {
            self.absent.insert(id);
        }
        Ok(exists)
    }
}

/// Core import logic, separated from I/O so it is unit-testable.
///
/// Runs in two passes inside one write transaction so exports can be
/// restored regardless of ID order:
///
/// 1. Every issue row lands under its original ID (so `blocked_by`,
///    `parent_id`, and relation references stay meaningful) with its parent
///    link in place; foreign keys are deferred to commit so a parent may
///    appear later in the payload. In non-merge mode an ID collision updates
///    the existing row in place and clears only the child collections the
///    payload re-supplies. Child issues keep their `parent_id` and outgoing
///    blocker edges survive. In merge mode a collision is skipped. Parent
///    links that would form a cycle are then dropped.
/// 2. Blocker edges, notes, audit events, and relations are attached now
///    that every referenced issue exists. Notes, events, and relations get
///    fresh row IDs, inserted in `(created_at, payload order)` so the global
///    order `itr log` shows survives a round trip. Blocker edges that would
///    close a dependency cycle are dropped.
///
/// References that cannot be honored (dangling, self, or cyclic) are dropped
/// and counted (soft fallback) instead of failing the import.
#[allow(clippy::too_many_lines)]
fn import_records(
    conn: &Connection,
    records: &[ImportRecord],
    merge: bool,
    counts: &mut ImportCounts,
    reviews: &mut Reviews,
) -> Result<(), ItrError> {
    let tx = db::write_tx(conn)?;
    // Parents may appear later in the payload than their children, so
    // foreign keys are checked at commit rather than per statement.
    tx.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    // Restoring an export must keep its `updated_at` values verbatim; the
    // touch trigger would re-stamp every replaced row. It is recreated
    // before commit (and comes back on its own if the transaction rolls
    // back, since SQLite DDL is transactional).
    db::suspend_updated_at_trigger(&tx)?;

    // Every payload ID exists once pass 1 finishes: it is inserted,
    // replaced, or skipped because it already existed.
    let mut known = KnownIssues::new(&tx, records.iter().map(|r| r.issue.id));

    // Pass 1: issue rows (with parent links, checked at commit).
    let mut accepted: Vec<&ImportRecord> = Vec::with_capacity(records.len());
    for record in records {
        let issue = &record.issue;
        let exists = db::issue_exists(&tx, issue.id)?;

        if merge && exists {
            counts.skipped += 1;
            continue;
        }

        let parent_id = match issue.parent_id {
            Some(parent) if parent != issue.id && known.contains(parent)? => Some(parent),
            Some(_) => {
                counts.dangling_parents += 1;
                None
            }
            None => None,
        };

        if exists {
            counts.replaced += 1;
            counts.replaced_rows += replace_issue_row(&tx, record, parent_id)?;
        } else {
            insert_issue_row(&tx, issue, parent_id)?;
        }

        // Keep imported issues searchable: index into FTS the same way
        // db::insert_issue does, so search works without a manual reindex.
        db::fts_index_issue(&tx, issue);

        accepted.push(record);
        counts.imported += 1;
    }

    // Parent cycles: with every row in place, drop the link of any accepted
    // issue whose new parent is itself or one of its own descendants.
    for record in &accepted {
        let id = record.issue.id;
        let parent: Option<i64> = tx.query_row(
            "SELECT parent_id FROM issues WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        if let Some(parent) = parent {
            if db::is_self_or_descendant(&tx, id, parent)? {
                tx.execute(
                    "UPDATE issues SET parent_id = NULL WHERE id = ?1",
                    params![id],
                )?;
                counts.cycle_parents += 1;
                reviews.issue("dropped parent links that would form a cycle", id);
            }
        }
    }

    // Pass 2: child rows and edges, now that every referenced issue exists.
    let mut notes: Vec<(&str, usize, i64, &NoteRow)> = Vec::new();
    let mut events: Vec<(&str, usize, i64, &EventRow)> = Vec::new();
    for record in &accepted {
        let id = record.issue.id;
        for note in record.notes.iter().flatten() {
            notes.push((note.created_at.as_str(), notes.len(), id, note));
        }
        for event in record.events.iter().flatten() {
            events.push((event.created_at.as_str(), events.len(), id, event));
        }
    }
    notes.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    events.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));

    for (_, _, issue_id, note) in &notes {
        tx.execute(
            "INSERT INTO notes (issue_id, content, agent, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![issue_id, note.content, note.agent, note.created_at],
        )?;
        counts.notes += 1;
    }

    for (_, _, issue_id, event) in &events {
        tx.execute(
            "INSERT INTO events (issue_id, field, old_value, new_value, agent, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                issue_id,
                event.field,
                event.old_value,
                event.new_value,
                event.agent,
                event.created_at,
            ],
        )?;
        counts.events += 1;
    }

    for record in &accepted {
        let issue = &record.issue;

        for blocker_id in record.blocked_by.iter().flatten() {
            if *blocker_id == issue.id || !known.contains(*blocker_id)? {
                counts.dangling_blockers += 1;
                continue;
            }
            // The new edge makes `blocker_id` block this issue; if this
            // issue already (transitively) blocks `blocker_id`, it would
            // close a cycle that no command could ever unblock.
            if db::has_path(&tx, issue.id, *blocker_id)? {
                counts.cycle_blockers += 1;
                reviews.issue(
                    "dropped blocker edges that would form a dependency cycle",
                    issue.id,
                );
                continue;
            }
            // ON CONFLICT (not OR IGNORE) so only the duplicate-edge case
            // is absorbed; any other constraint failure still surfaces.
            counts.dependencies += tx.execute(
                "INSERT INTO dependencies (blocker_id, blocked_id) VALUES (?1, ?2)
                 ON CONFLICT(blocker_id, blocked_id) DO NOTHING",
                params![blocker_id, issue.id],
            )?;
        }

        // Exports list each relation under both endpoints; the UNIQUE
        // constraint collapses the mirrored copy to one row. A bundle may
        // only describe relations of its own issue, so a skipped (--merge)
        // or absent issue is never modified through someone else's bundle.
        for relation in record.relations.iter().flatten() {
            let involves_self = relation.source_id == issue.id || relation.target_id == issue.id;
            if !involves_self
                || relation.source_id == relation.target_id
                || !known.contains(relation.source_id)?
                || !known.contains(relation.target_id)?
            {
                counts.dangling_relations += 1;
                continue;
            }
            counts.relations += tx.execute(
                "INSERT INTO relations (source_id, target_id, relation_type, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(source_id, target_id, relation_type) DO NOTHING",
                params![
                    relation.source_id,
                    relation.target_id,
                    relation.relation_type,
                    relation.created_at,
                ],
            )?;
        }
    }

    // A resolved issue never blocks anything: every close path removes its
    // outgoing edges. Apply the same invariant to imported closed issues.
    for record in &accepted {
        if matches!(record.issue.status.as_str(), "done" | "wontfix") {
            counts.closed_blocker_edges += db::remove_blocker_edges(&tx, record.issue.id)?;
        }
    }

    db::restore_updated_at_trigger(&tx)?;
    tx.commit()?;
    Ok(())
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

/// Replace an existing issue in place and return how many existing child
/// rows were removed. An UPDATE (rather than `INSERT OR REPLACE`, which is
/// delete-then-insert) keeps the row's identity so `ON DELETE CASCADE` /
/// `ON DELETE SET NULL` never fire: child issues keep their `parent_id` and
/// edges where this issue is the blocker survive. Only the child
/// collections the payload re-supplies are cleared; an absent key leaves
/// the existing rows untouched.
fn replace_issue_row(
    tx: &Connection,
    record: &ImportRecord,
    parent_id: Option<i64>,
) -> Result<usize, ItrError> {
    let issue = &record.issue;
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
    let mut removed = 0;
    if record.notes.is_some() {
        removed += tx.execute("DELETE FROM notes WHERE issue_id = ?1", params![issue.id])?;
    }
    if record.events.is_some() {
        removed += tx.execute("DELETE FROM events WHERE issue_id = ?1", params![issue.id])?;
    }
    if record.blocked_by.is_some() {
        removed += tx.execute(
            "DELETE FROM dependencies WHERE blocked_id = ?1",
            params![issue.id],
        )?;
    }
    if record.relations.is_some() {
        removed += tx.execute(
            "DELETE FROM relations WHERE source_id = ?1 OR target_id = ?1",
            params![issue.id],
        )?;
    }
    Ok(removed)
}

/// Parse, validate, and import `input`. Shared by `run` and the tests.
fn import_input(
    conn: &Connection,
    input: &str,
    merge: bool,
) -> Result<(ImportCounts, Reviews), ItrError> {
    let mut counts = ImportCounts::default();
    let mut reviews = Reviews::default();
    let records = prepare_records(input, &mut counts, &mut reviews)?;
    import_records(conn, &records, merge, &mut counts, &mut reviews)?;
    Ok((counts, reviews))
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
            io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };

    let (counts, reviews) = import_input(conn, &input, merge)?;

    for note in reviews.render() {
        eprintln!("{note}");
    }

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
             neither the import payload nor the database (or references itself, \
             or a relation did not involve its bundle's issue). \
             The issues themselves were imported.",
            parts.join(", ")
        );
    }

    if counts.replaced > 0 {
        eprintln!(
            "REVIEW: import replaced {} existing issue(s) whose IDs collided \
             with the imported data and removed {} of their existing note, audit \
             event, incoming blocker, and relation rows in favor of the payload's \
             (child issues and outgoing blocker edges were kept). Pass --merge to \
             keep existing issues and skip colliding IDs instead.",
            counts.replaced, counts.replaced_rows
        );
    }

    if counts.closed_blocker_edges > 0 {
        eprintln!(
            "REVIEW: import removed {} blocker edge(s) from done/wontfix issues \
             (a resolved issue no longer blocks anything, as on close)",
            counts.closed_blocker_edges
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
                "dropped_cycles": counts.cycle_parents + counts.cycle_blockers,
                "invalid_records": counts.invalid,
                "duplicate_ids": counts.duplicates,
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
    use crate::models::{Event, ExportData, Issue, Note, Relation};
    use std::path::{Path, PathBuf};

    /// Run the full parse -> validate -> import pipeline over serialized
    /// export bundles (JSONL), exactly as `itr import` would.
    fn import_items(
        conn: &Connection,
        items: &[ExportData],
        merge: bool,
    ) -> Result<ImportCounts, ItrError> {
        let input = items
            .iter()
            .map(|item| serde_json::to_string(item).expect("serialize bundle"))
            .collect::<Vec<_>>()
            .join(
                "
",
            );
        import_input(conn, &input, merge).map(|(counts, _)| counts)
    }

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

    // --- Validation pipeline (review 2026-09-26) ---

    /// Import raw text through the full pipeline; returns counts and the
    /// rendered REVIEW notes.
    fn import_str(conn: &Connection, input: &str, merge: bool) -> (ImportCounts, Vec<String>) {
        let (counts, reviews) = import_input(conn, input, merge).expect("import");
        (counts, reviews.render())
    }

    fn bundle(issue: serde_json::Value) -> String {
        serde_json::json!({ "issue": issue }).to_string()
    }

    fn has_review(reviews: &[String], needle: &str) -> bool {
        reviews.iter().any(|r| r.contains(needle))
    }

    /// #230: synonyms normalize like every other write path; unknown values
    /// fall back with a REVIEW note; nothing hits the SQL CHECK constraint.
    #[test]
    fn import_normalizes_enum_values() {
        let (conn, path) = test_db("enums");
        let input = [
            bundle(serde_json::json!({"id": 1, "title": "a", "priority": "urgent", "status": "Open", "kind": " Bug "})),
            bundle(serde_json::json!({"id": 2, "title": "b", "priority": "bogus", "status": "wip", "kind": "nope"})),
        ]
        .join("\n");
        let (counts, reviews) = import_str(&conn, &input, false);
        assert_eq!(counts.imported, 2);
        let a = db::get_issue(&conn, 1).unwrap();
        assert_eq!(
            (a.priority.as_str(), a.status.as_str(), a.kind.as_str()),
            ("critical", "open", "bug")
        );
        let b = db::get_issue(&conn, 2).unwrap();
        assert_eq!(
            (b.priority.as_str(), b.status.as_str(), b.kind.as_str()),
            ("medium", "in-progress", "task")
        );
        assert!(has_review(
            &reviews,
            "priority 'bogus' not recognized, defaulted to 'medium'"
        ));
        assert!(has_review(
            &reviews,
            "kind 'nope' not recognized, defaulted to 'task'"
        ));
        cleanup(&path);
    }

    /// IE-5: one malformed line is skipped with its real line number; the
    /// other records still import. Optional fields may be omitted.
    #[test]
    fn import_skips_malformed_lines_and_names_them() {
        let (conn, path) = test_db("malformed-line");
        let input = [
            bundle(serde_json::json!({"id": 1, "title": "first"})),
            "{not json".to_string(),
            bundle(serde_json::json!({"id": "7", "title": "string id"})),
            serde_json::json!({"notes": []}).to_string(),
            bundle(serde_json::json!({"id": 5, "title": "last"})),
        ]
        .join("\n");
        let (counts, reviews) = import_str(&conn, &input, false);
        assert_eq!(counts.imported, 2);
        assert_eq!(counts.invalid, 3);
        assert!(has_review(&reviews, "skipped line 2: not valid JSON"));
        assert!(has_review(&reviews, "skipped line 3:"));
        assert!(has_review(
            &reviews,
            "skipped line 4: missing the \"issue\" object"
        ));
        assert!(db::issue_exists(&conn, 5).unwrap());
        cleanup(&path);
    }

    /// IE-1: IDs outside `1..=MAX_ISSUE_ID` are refused, so AUTOINCREMENT can
    /// never be pushed to `i64::MAX` (which breaks every later insert).
    #[test]
    fn import_refuses_out_of_range_ids() {
        let (conn, path) = test_db("id-range");
        let input = [
            bundle(serde_json::json!({"id": i64::MAX, "title": "huge"})),
            bundle(serde_json::json!({"id": 0, "title": "zero"})),
            bundle(serde_json::json!({"id": -5, "title": "negative"})),
            bundle(serde_json::json!({"id": 3, "title": "fine"})),
        ]
        .join("\n");
        let (counts, reviews) = import_str(&conn, &input, false);
        assert_eq!(counts.imported, 1);
        assert_eq!(counts.invalid, 3);
        assert!(has_review(&reviews, "is out of range"));
        let next = seed_issue(&conn, "after import");
        assert_eq!(next.id, 4, "AUTOINCREMENT continues after the valid max");
        cleanup(&path);
    }

    /// IE-3: a repeated ID keeps only the last copy, whole.
    #[test]
    fn import_keeps_last_copy_of_duplicate_ids() {
        let (conn, path) = test_db("dup-ids");
        let first = serde_json::json!({
            "issue": {"id": 1, "title": "first copy"},
            "notes": [{"content": "from first", "created_at": "2026-01-01T00:00:00Z"}]
        });
        let second = serde_json::json!({
            "issue": {"id": 1, "title": "second copy", "status": "done"},
            "notes": [{"content": "from second", "created_at": "2026-01-01T00:00:00Z"}]
        });
        let input = format!("{first}\n{second}");
        let (counts, reviews) = import_str(&conn, &input, false);
        assert_eq!(
            (counts.imported, counts.replaced, counts.duplicates),
            (1, 0, 1)
        );
        let issue = db::get_issue(&conn, 1).unwrap();
        assert_eq!(issue.title, "second copy");
        let notes: Vec<String> = db::get_notes(&conn, 1)
            .unwrap()
            .into_iter()
            .map(|n| n.content)
            .collect();
        assert_eq!(notes, vec!["from second"]);
        assert!(has_review(&reviews, "kept only the last copy"));
        cleanup(&path);
    }

    /// IE-4: unknown keys are reported, and `parent` aliases `parent_id`.
    #[test]
    fn import_reports_unknown_keys_and_accepts_parent_alias() {
        let (conn, path) = test_db("unknown-keys");
        let input = [
            bundle(serde_json::json!({"id": 1, "title": "parent"})),
            serde_json::json!({
                "issue": {"id": 2, "title": "child", "parent": 1, "assignee": "bob"},
                "blockedby": [1]
            })
            .to_string(),
        ]
        .join("\n");
        let (_, reviews) = import_str(&conn, &input, false);
        assert_eq!(db::get_issue(&conn, 2).unwrap().parent_id, Some(1));
        assert!(has_review(
            &reviews,
            "ignored unknown issue field 'assignee'"
        ));
        assert!(has_review(
            &reviews,
            "ignored unknown bundle field 'blockedby'"
        ));
        cleanup(&path);
    }

    /// IE-6 / IE-7 / IE-15: timestamps, list fields, titles, and control
    /// characters are cleaned to the same shape `add` stores.
    #[test]
    fn import_cleans_fields_like_add() {
        let (conn, path) = test_db("clean-fields");
        let input = bundle(serde_json::json!({
            "id": 1,
            "title": "  a\u{1b}[31mred\u{0}  ",
            "files": ["", "a.rs", "a.rs", " b.rs "],
            "tags": ["A", "A", " x "],
            "skills": ["Rust", "RUST", " Go "],
            "assigned_to": "  bob\n",
            "created_at": "2026-01-01T05:00:00+05:00",
            "updated_at": "garbage"
        }));
        let (_, reviews) = import_str(&conn, &input, false);
        let issue = db::get_issue(&conn, 1).unwrap();
        assert_eq!(issue.title, "a[31mred");
        assert_eq!(issue.files, vec!["a.rs", "b.rs"]);
        assert_eq!(issue.tags, vec!["A", "x"]);
        assert_eq!(issue.skills, vec!["rust", "go"]);
        assert_eq!(issue.assigned_to, "bob");
        assert_eq!(issue.created_at, "2026-01-01T00:00:00Z");
        assert!(crate::sanitize::normalize_timestamp(&issue.updated_at).is_some());
        assert!(has_review(&reviews, "removed control characters"));
        assert!(has_review(&reviews, "unparseable updated_at"));
        assert!(has_review(&reviews, "cleaned files/tags/skills"));

        let (_, reviews) = import_str(
            &conn,
            &bundle(serde_json::json!({"id": 2, "title": "   "})),
            false,
        );
        assert_eq!(db::get_issue(&conn, 2).unwrap().title, "(untitled)");
        assert!(has_review(&reviews, "(untitled)"));
        cleanup(&path);
    }

    /// #272: parent and dependency cycles are dropped (with a count), both
    /// within the payload and against existing rows.
    #[test]
    fn import_drops_parent_and_dependency_cycles() {
        let (conn, path) = test_db("cycles");
        // 3-node cycle on both parent_id and blocked_by.
        let input = [
            serde_json::json!({"issue": {"id": 1, "title": "a", "parent_id": 3}, "blocked_by": [3]}),
            serde_json::json!({"issue": {"id": 2, "title": "b", "parent_id": 1}, "blocked_by": [1]}),
            serde_json::json!({"issue": {"id": 3, "title": "c", "parent_id": 2}, "blocked_by": [2]}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        let (counts, _) = import_str(&conn, &input, false);
        assert_eq!(counts.cycle_parents, 1);
        assert_eq!(counts.cycle_blockers, 1);
        assert_eq!(counts.dependencies, 2);
        for id in 1..=3 {
            assert!(!db::is_self_or_descendant(
                &conn,
                id,
                db::get_issue(&conn, id).unwrap().parent_id.unwrap_or(0)
            )
            .unwrap());
        }

        // A cycle formed only together with existing rows (--merge): 4 blocks
        // 5 in the DB; the payload adds 5 -> blocks -> 4 through a new issue.
        let four = seed_issue(&conn, "four");
        let five = seed_issue(&conn, "five");
        db::add_dependency(&conn, four.id, five.id).unwrap();
        let input =
            serde_json::json!({"issue": {"id": four.id, "title": "four"}, "blocked_by": [five.id]})
                .to_string();
        let (counts, reviews) = import_str(&conn, &input, false);
        assert_eq!(counts.cycle_blockers, 1);
        assert!(has_review(&reviews, "dependency cycle"));
        assert!(
            db::get_blockers(&conn, four.id).unwrap().is_empty(),
            "the cycle-closing edge five -> four must not exist"
        );
        assert_eq!(db::get_blockers(&conn, five.id).unwrap(), vec![four.id]);
        cleanup(&path);
    }

    /// IE-8 / IE-2: a bundle cannot attach relations between other issues,
    /// and relation types are normalized instead of silently dropped.
    #[test]
    fn import_relations_must_involve_own_issue_and_normalize_type() {
        let (conn, path) = test_db("relations-scope");
        let one = seed_issue(&conn, "one");
        let two = seed_issue(&conn, "two");
        let input = [
            serde_json::json!({"issue": {"id": one.id, "title": "one"}}),
            serde_json::json!({
                "issue": {"id": 3, "title": "three"},
                "relations": [
                    {"source_id": one.id, "target_id": two.id, "relation_type": "duplicate"},
                    {"source_id": 3, "target_id": two.id, "relation_type": "Dupe"},
                    {"source_id": 3, "target_id": one.id, "relation_type": "bogus"}
                ]
            }),
        ]
        .map(|v| v.to_string())
        .join("\n");
        let (counts, reviews) = import_str(&conn, &input, true);
        assert_eq!(counts.skipped, 1);
        assert_eq!(counts.relations, 2);
        assert_eq!(counts.dangling_relations, 1);
        assert!(db::get_relations(&conn, one.id)
            .unwrap()
            .iter()
            .all(|r| r.source_id == 3));
        let types: Vec<String> = db::get_relations(&conn, 3)
            .unwrap()
            .into_iter()
            .map(|r| r.relation_type)
            .collect();
        assert!(types.contains(&"duplicate".to_string()));
        assert!(types.contains(&"related".to_string()));
        assert!(has_review(&reviews, "relation type 'bogus' not recognized"));
        cleanup(&path);
    }

    /// #223 remainder: an ABSENT child collection leaves existing rows
    /// alone on replace; a PRESENT one replaces them and is counted.
    #[test]
    fn replace_import_only_clears_collections_present_in_payload() {
        let (conn, path) = test_db("absent-keys");
        let issue = seed_issue(&conn, "keep my notes");
        db::add_note(&conn, issue.id, "existing", "alice").unwrap();

        let without_notes = bundle(serde_json::json!({"id": issue.id, "title": "renamed"}));
        let (counts, _) = import_str(&conn, &without_notes, false);
        assert_eq!(counts.replaced_rows, 0);
        assert_eq!(db::get_notes(&conn, issue.id).unwrap().len(), 1);

        let with_empty_notes =
            serde_json::json!({"issue": {"id": issue.id, "title": "renamed"}, "notes": []})
                .to_string();
        let (counts, _) = import_str(&conn, &with_empty_notes, false);
        assert_eq!(counts.replaced_rows, 1);
        assert!(db::get_notes(&conn, issue.id).unwrap().is_empty());
        cleanup(&path);
    }

    /// IE-13: a closed issue never keeps outgoing blocker edges.
    #[test]
    fn import_removes_blocker_edges_from_closed_issues() {
        let (conn, path) = test_db("closed-blocker");
        let input = [
            serde_json::json!({"issue": {"id": 1, "title": "done", "status": "done"}}),
            serde_json::json!({"issue": {"id": 2, "title": "blocked"}, "blocked_by": [1, 1]}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        let (counts, _) = import_str(&conn, &input, false);
        assert_eq!(counts.closed_blocker_edges, 1);
        assert!(db::get_blockers(&conn, 2).unwrap().is_empty());
        cleanup(&path);
    }

    /// IE-12: BOM-prefixed JSONL and a single pretty-printed bundle import.
    #[test]
    fn import_accepts_bom_and_single_pretty_object() {
        let (conn, path) = test_db("bom");
        let bom = format!(
            "\u{feff}{}",
            bundle(serde_json::json!({"id": 1, "title": "bom"}))
        );
        assert_eq!(import_str(&conn, &bom, false).0.imported, 1);
        let pretty = serde_json::to_string_pretty(
            &serde_json::json!({"issue": {"id": 2, "title": "pretty"}}),
        )
        .unwrap();
        assert_eq!(import_str(&conn, &pretty, false).0.imported, 1);
        cleanup(&path);
    }

    /// IE-9: events keep their global `(created_at, payload)` order across a
    /// round trip, even when they are spread over several bundles.
    #[test]
    fn import_inserts_events_in_global_time_order() {
        let (conn, path) = test_db("event-order");
        let input = [
            serde_json::json!({"issue": {"id": 1, "title": "a"}, "events": [
                {"field": "status", "created_at": "2026-01-03T00:00:00Z"}
            ]}),
            serde_json::json!({"issue": {"id": 2, "title": "b"}, "events": [
                {"field": "title", "created_at": "2026-01-01T00:00:00Z"}
            ]}),
        ]
        .map(|v| v.to_string())
        .join("\n");
        import_str(&conn, &input, false);
        let first_event_issue: i64 = conn
            .query_row(
                "SELECT issue_id FROM events ORDER BY id LIMIT 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_event_issue, 2, "older event gets the lower row id");
        cleanup(&path);
    }
}
