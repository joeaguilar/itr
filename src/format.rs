use crate::models::{
    BatchResult, Event, GraphOutput, IssueDetail, IssueSummary, Relation, SearchResult, Stats,
    UnblockedIssue,
};
use std::cell::RefCell;

thread_local! {
    static FIELDS_FILTER: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Install a thread-local allowlist of output field names.
///
/// All subsequent `format_*` calls on this thread will hide any field whose
/// name isn't in `fields` (for compact/pretty output) or strip them from the
/// serialized object (for JSON output). Call with an empty `Vec` to filter
/// everything out; call [`clear_fields_filter`]-style by overwriting with the
/// default (none) — there is no explicit clear helper because each top-level
/// command sets the filter exactly once during argument parsing.
///
/// # Examples
///
/// ```text
/// use itr::format::set_fields_filter;
/// // Only emit `id` and `title` from now on, on this thread.
/// set_fields_filter(vec!["id".into(), "title".into()]);
/// ```
pub fn set_fields_filter(fields: Vec<String>) {
    FIELDS_FILTER.with(|f| {
        *f.borrow_mut() = Some(fields);
    });
}

fn get_fields_filter() -> Option<Vec<String>> {
    FIELDS_FILTER.with(|f| f.borrow().clone())
}

/// Emit a soft-fallback `REVIEW:` note when `--fields` was passed but this
/// command/format combination has no field filtering, so the full output is
/// emitted unchanged.
///
/// Silently swallowing the flag is the anti-pattern this guards against
/// (issue #197): an agent passing `--fields` to save tokens must get a signal
/// that the request was not applied.
fn warn_fields_unsupported(what: &str) {
    if FIELDS_FILTER.with(|f| f.borrow().is_some()) {
        eprintln!("REVIEW: --fields is not supported for {what}; emitting unfiltered output");
    }
}

/// Returns true if `name` should be included in output.
/// When no --fields filter is set, all fields are included.
fn field_enabled(fields: Option<&Vec<String>>, name: &str) -> bool {
    fields.is_none_or(|f| f.iter().any(|x| x == name))
}

/// Apply field filtering to a JSON string if --fields was set, returning the filtered string
fn apply_fields_filter(json_str: &str) -> String {
    FIELDS_FILTER.with(|f| {
        let filter = f.borrow();
        if let Some(ref fields) = *filter {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                let filtered = filter_json_fields(value, fields);
                return filtered.to_string();
            }
        }
        json_str.to_string()
    })
}

/// Print a JSON string to stdout, applying the thread-local `--fields` filter
/// if one is set.
///
/// If the input is not valid JSON, it's printed unchanged (the formatter is
/// best-effort and never panics on bad input).
///
/// # Examples
///
/// ```text
/// use itr::format::{println_json, set_fields_filter};
/// set_fields_filter(vec!["id".into()]);
/// println_json(r#"{"id":1,"title":"hello"}"#);
/// // stdout: {"id":1}
/// ```
pub fn println_json(json_str: &str) {
    FIELDS_FILTER.with(|f| {
        let filter = f.borrow();
        if let Some(ref fields) = *filter {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(json_str) {
                let filtered = filter_json_fields(value, fields);
                println!("{}", filtered);
                return;
            }
        }
        println!("{}", json_str);
    });
}

/// Output mode selected by `--format` on every CLI subcommand.
///
/// - `Compact` (default) — token-efficient key/value lines for agent
///   consumption.
/// - `Json` — machine-readable JSON, suitable for piping into `jq` or another
///   tool. Respects the `--fields` filter.
/// - `Pretty` — human-oriented tables, DOT graphs, etc.
/// - `Oneline` — one record per line (mostly identical to compact for detail
///   views, but listings collapse to a tab-separated single line per issue).
///
/// # Examples
///
/// ```text
/// use itr::format::Format;
/// assert_eq!(Format::from_str("json"), Some(Format::Json));
/// assert_eq!(Format::from_str("garbage"), None);
/// assert!(Format::Json.is_json());
/// assert!(!Format::Pretty.is_json());
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    Compact,
    Json,
    Pretty,
    Oneline,
}

impl Format {
    /// Parse a `--format` argument value. Matching is case-insensitive (and
    /// trims surrounding whitespace) like every other enum-ish input in the
    /// project (priority/kind/status in `normalize.rs`), so `-f JSON` works
    /// (issue #192). Returns `None` for truly unknown inputs so the CLI layer
    /// can produce a helpful error.
    ///
    /// # Examples
    ///
    /// ```text
    /// use itr::format::Format;
    /// assert_eq!(Format::from_str("compact"), Some(Format::Compact));
    /// assert_eq!(Format::from_str("JSON"), Some(Format::Json));
    /// assert_eq!(Format::from_str("oneline"), Some(Format::Oneline));
    /// assert_eq!(Format::from_str(""), None);
    /// ```
    pub fn from_str(s: &str) -> Option<Format> {
        match s.trim().to_lowercase().as_str() {
            "compact" => Some(Format::Compact),
            "json" => Some(Format::Json),
            "pretty" => Some(Format::Pretty),
            "oneline" => Some(Format::Oneline),
            _ => None,
        }
    }

    /// True when this is the JSON output mode. Useful for branches that emit
    /// errors in JSON or text depending on `--format`.
    ///
    /// # Examples
    ///
    /// ```text
    /// use itr::format::Format;
    /// assert!(Format::Json.is_json());
    /// assert!(!Format::Compact.is_json());
    /// ```
    pub fn is_json(self) -> bool {
        matches!(self, Format::Json)
    }
}

// --- Line-oriented value escaping ---
//
// Compact, oneline, and compact-event output are line-oriented contracts: one
// logical field must never span more than one physical line, and double-quoted
// `"…"` tokens must never contain an unescaped quote. These helpers define THE
// project-wide encoding for free-text values embedded in parseable
// line-oriented output (see docs/command-contracts.md, "Escaping In
// Line-Oriented Output"). Reuse them for any new output instead of inventing
// another scheme. Graphviz DOT has its own escape requirements — use
// [`escape_dot_label`] there.

/// Backslash-escape characters that would break line-oriented output:
/// `\` → `\\`, newline → `\n`, carriage return → `\r`, tab → `\t`.
///
/// Guarantees a value occupies exactly one physical line and is exactly
/// recoverable by reversing the escapes. Use for unquoted labeled values
/// (`TITLE:`, `CONTEXT:`, …) and tab-separated fields.
pub fn escape_line_value(s: &str) -> String {
    escape_value(s, false)
}

/// [`escape_line_value`] plus `"` → `\"`, for values rendered inside
/// double-quoted `"…"` tokens (oneline titles, `OLDEST_OPEN`/`NODE:`/
/// `UNBLOCKED:` titles, event `OLD:"…"`/`NEW:"…"`, batch item strings).
pub fn escape_quoted_value(s: &str) -> String {
    escape_value(s, true)
}

fn escape_value(s: &str, escape_quotes: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' if escape_quotes => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string for embedding inside a Graphviz DOT double-quoted label:
/// `\` → `\\`, `"` → `\"`, and literal newlines (LF, CR, or CRLF) become the
/// DOT `\n` line-break escape, so emitted DOT always parses.
fn escape_dot_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\\n");
            }
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out
}

// --- Issue Detail ---

/// Render a single issue detail (issue + notes + relations + breakdown) in
/// the requested output mode.
///
/// `Compact` and `Oneline` collapse to the same multi-line key:value layout
/// for detail views — only listings differ between those two modes.
///
/// # Examples
///
/// ```text
/// use itr::format::{format_issue_detail, Format};
/// // given `detail`: an itr::models::IssueDetail
/// let json = format_issue_detail(&detail, Format::Json);
/// assert!(json.starts_with('{'));
/// ```
pub fn format_issue_detail(detail: &IssueDetail, fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(detail).unwrap_or_default()),
        Format::Compact | Format::Oneline => format_issue_detail_compact(detail),
        Format::Pretty => {
            warn_fields_unsupported("issue-detail pretty output");
            format_issue_detail_pretty(detail)
        }
    }
}

/// Render a batch of issue details (`itr get 1,2,3`, #136) in the requested
/// output mode.
///
/// - `Json` — an array of `IssueDetail` objects (respects `--fields`).
/// - `Compact`/`Oneline` — the per-issue compact blocks from
///   [`format_issue_detail`], separated by one blank line (the same
///   separator as compact issue lists). Each block starts with its `ID:`
///   record line and free text is escaped per the line-oriented contract, so
///   the blank-line separator is unambiguous.
/// - `Pretty` — the per-issue pretty blocks, separated by one blank line.
///
/// Callers with exactly one resolved ID must use [`format_issue_detail`]
/// instead — the single-issue byte contract (a bare JSON object, no
/// separator) is pinned by snapshots.
pub fn format_issue_details(details: &[IssueDetail], fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(details).unwrap_or_default()),
        Format::Compact | Format::Oneline => details
            .iter()
            .map(format_issue_detail_compact)
            .collect::<Vec<_>>()
            .join("\n\n"),
        Format::Pretty => {
            warn_fields_unsupported("issue-detail pretty output");
            details
                .iter()
                .map(format_issue_detail_pretty)
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }
}

fn format_issue_detail_compact(d: &IssueDetail) -> String {
    let fields = get_fields_filter();
    let on = |name: &str| field_enabled(fields.as_ref(), name);
    let mut lines = Vec::new();

    let mut first_parts = Vec::new();
    if on("id") {
        first_parts.push(format!("ID:{}", d.issue.id));
    }
    if on("status") {
        first_parts.push(format!("STATUS:{}", d.issue.status));
    }
    if on("priority") {
        first_parts.push(format!("PRIORITY:{}", d.issue.priority));
    }
    if on("kind") {
        first_parts.push(format!("KIND:{}", d.issue.kind));
    }
    if on("urgency") {
        first_parts.push(format!("URGENCY:{:.1}", d.urgency));
    }
    if on("blocked_by") && !d.blocked_by.is_empty() {
        first_parts.push(format!(
            "BLOCKED_BY:{}",
            d.blocked_by
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if on("blocks") && !d.blocks.is_empty() {
        first_parts.push(format!(
            "BLOCKS:{}",
            d.blocks
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if !first_parts.is_empty() {
        lines.push(first_parts.join(" "));
    }

    if on("tags") && !d.issue.tags.is_empty() {
        lines.push(format!(
            "TAGS:{}",
            escape_line_value(&d.issue.tags.join(","))
        ));
    }
    if on("files") && !d.issue.files.is_empty() {
        lines.push(format!(
            "FILES:{}",
            escape_line_value(&d.issue.files.join(","))
        ));
    }
    if on("skills") && !d.issue.skills.is_empty() {
        lines.push(format!(
            "SKILLS:{}",
            escape_line_value(&d.issue.skills.join(","))
        ));
    }
    if on("assigned_to") && !d.issue.assigned_to.is_empty() {
        lines.push(format!(
            "ASSIGNED:{}",
            escape_line_value(&d.issue.assigned_to)
        ));
    }
    if on("title") {
        lines.push(format!("TITLE: {}", escape_line_value(&d.issue.title)));
    }
    if on("context") && !d.issue.context.is_empty() {
        lines.push(format!("CONTEXT: {}", escape_line_value(&d.issue.context)));
    }
    if on("acceptance") && !d.issue.acceptance.is_empty() {
        lines.push(format!(
            "ACCEPTANCE: {}",
            escape_line_value(&d.issue.acceptance)
        ));
    }
    if on("parent_id") {
        if let Some(pid) = d.issue.parent_id {
            lines.push(format!("PARENT: {}", pid));
        }
    }
    if on("close_reason") && !d.issue.close_reason.is_empty() {
        lines.push(format!(
            "CLOSE_REASON: {}",
            escape_line_value(&d.issue.close_reason)
        ));
    }
    if on("created_at") {
        lines.push(format!("CREATED: {}", d.issue.created_at));
    }
    if on("updated_at") {
        lines.push(format!("UPDATED: {}", d.issue.updated_at));
    }

    if on("urgency_breakdown") {
        if let Some(ref breakdown) = d.urgency_breakdown {
            lines.push("--- URGENCY BREAKDOWN ---".to_string());
            let parts: Vec<String> = breakdown
                .components
                .iter()
                .filter(|(_, v)| *v != 0.0)
                .map(|(k, v)| format!("{}={:.1}", k, v))
                .collect();
            lines.push(parts.join(" "));
        }
    }

    if on("relations") && !d.relations.is_empty() {
        lines.push("--- RELATIONS ---".to_string());
        for rel in &d.relations {
            lines.push(format_relation_compact(rel, d.issue.id));
        }
    }

    if on("notes") && !d.notes.is_empty() {
        lines.push("--- NOTES ---".to_string());
        for note in &d.notes {
            let agent_str = if note.agent.is_empty() {
                String::new()
            } else {
                format!(" ({})", escape_line_value(&note.agent))
            };
            lines.push(format!(
                "[{}]{} {}",
                note.created_at,
                agent_str,
                escape_line_value(&note.content)
            ));
        }
    }

    lines.join("\n")
}

fn format_relation_compact(rel: &Relation, current_id: i64) -> String {
    if rel.source_id == current_id {
        format!(
            "RELATION: {} -> #{} ({})",
            rel.relation_type, rel.target_id, rel.created_at
        )
    } else {
        format!(
            "RELATION: {} <- #{} ({})",
            rel.relation_type, rel.source_id, rel.created_at
        )
    }
}

fn format_issue_detail_pretty(d: &IssueDetail) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Issue #{}: {}", d.issue.id, d.issue.title));
    lines.push(format!(
        "  Status: {}  Priority: {}  Kind: {}  Urgency: {:.1}",
        d.issue.status, d.issue.priority, d.issue.kind, d.urgency
    ));
    if !d.issue.tags.is_empty() {
        lines.push(format!("  Tags: {}", d.issue.tags.join(", ")));
    }
    if !d.issue.files.is_empty() {
        lines.push(format!("  Files: {}", d.issue.files.join(", ")));
    }
    if !d.issue.skills.is_empty() {
        lines.push(format!("  Skills: {}", d.issue.skills.join(", ")));
    }
    if !d.issue.assigned_to.is_empty() {
        lines.push(format!("  Assigned to: {}", d.issue.assigned_to));
    }
    if !d.issue.context.is_empty() {
        lines.push(format!("  Context: {}", d.issue.context));
    }
    if !d.issue.acceptance.is_empty() {
        lines.push(format!("  Acceptance: {}", d.issue.acceptance));
    }
    if !d.blocked_by.is_empty() {
        lines.push(format!(
            "  Blocked by: {}",
            d.blocked_by
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !d.blocks.is_empty() {
        lines.push(format!(
            "  Blocks: {}",
            d.blocks
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !d.relations.is_empty() {
        lines.push("  Relations:".to_string());
        for rel in &d.relations {
            if rel.source_id == d.issue.id {
                lines.push(format!("    {} -> #{}", rel.relation_type, rel.target_id));
            } else {
                lines.push(format!("    {} <- #{}", rel.relation_type, rel.source_id));
            }
        }
    }
    if !d.notes.is_empty() {
        lines.push("  Notes:".to_string());
        for note in &d.notes {
            lines.push(format!("    [{}] {}", note.created_at, note.content));
        }
    }
    lines.join("\n")
}

// --- Issue Summary List ---

/// Render a list of issue summaries in the requested output mode.
///
/// Empty input renders to an empty string in every mode — `itr` follows the
/// "empty stdout, exit 0" convention for queries that match nothing.
///
/// # Examples
///
/// ```text
/// use itr::format::{format_issue_list, Format};
/// assert_eq!(format_issue_list(&[], Format::Pretty), "");
/// assert_eq!(format_issue_list(&[], Format::Compact), "");
/// ```
pub fn format_issue_list(issues: &[IssueSummary], fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(issues).unwrap_or_default()),
        Format::Compact => format_issue_list_compact(issues),
        Format::Pretty => format_issue_list_pretty(issues),
        Format::Oneline => format_issue_list_oneline(issues),
    }
}

/// One issue-summary field rendered as a single oneline/TSV cell. List-valued
/// fields join with `,`; free text is escaped per the line-oriented contract
/// (issue #175). Unknown field names render as an empty cell so the column
/// count stays stable for scripts (the unknown name was already warned about
/// by `validate_fields`).
fn oneline_field_value(i: &IssueSummary, field: &str) -> String {
    match field {
        "id" => i.id.to_string(),
        "status" => i.status.clone(),
        "priority" => i.priority.clone(),
        "kind" => i.kind.clone(),
        "urgency" => format!("{:.1}", i.urgency),
        "is_blocked" => i.is_blocked.to_string(),
        "blocked_by" => i
            .blocked_by
            .iter()
            .map(std::string::ToString::to_string)
            .collect::<Vec<_>>()
            .join(","),
        "tags" => escape_line_value(&i.tags.join(",")),
        "files" => escape_line_value(&i.files.join(",")),
        "skills" => escape_line_value(&i.skills.join(",")),
        "title" => escape_line_value(&i.title),
        "acceptance" => escape_line_value(&i.acceptance),
        "assigned_to" => escape_line_value(&i.assigned_to),
        "created_at" => i.created_at.clone(),
        "updated_at" => i.updated_at.clone(),
        _ => String::new(),
    }
}

fn format_issue_list_oneline(issues: &[IssueSummary]) -> String {
    // With --fields: the selected fields, tab-separated, in the requested
    // order — one issue per line, script-ready (spec P4).
    if let Some(fields) = get_fields_filter() {
        return issues
            .iter()
            .map(|i| {
                fields
                    .iter()
                    .map(|f| oneline_field_value(i, f))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    issues
        .iter()
        .map(|i| {
            // Escape free-text fields so each issue is exactly one physical
            // line with a stable tab-separated field count (issue #175).
            let assignee = if i.assigned_to.is_empty() {
                String::new()
            } else {
                format!("\t{}", escape_line_value(&i.assigned_to))
            };
            format!(
                "{}\t{}\t{}\t{}\t\"{}\"{}",
                i.id,
                i.status,
                i.priority,
                i.kind,
                escape_quoted_value(&i.title),
                assignee
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Field names rendered on the first record line of a compact list block,
/// in default order. The remaining printable fields each get their own line.
const COMPACT_FIRST_LINE_FIELDS: &[&str] =
    &["id", "status", "priority", "kind", "urgency", "blocked_by"];
const COMPACT_LINE_FIELDS: &[&str] = &[
    "tags",
    "files",
    "skills",
    "assigned_to",
    "title",
    "acceptance",
    "parent_id",
];

fn format_issue_list_compact(issues: &[IssueSummary]) -> String {
    let fields = get_fields_filter();
    // With --fields, honor the requested order within each structural tier
    // (record line vs per-field lines); without it, keep the default order.
    let first_line_fields: Vec<&str> = match fields {
        Some(ref fs) => fs
            .iter()
            .map(String::as_str)
            .filter(|f| COMPACT_FIRST_LINE_FIELDS.contains(f))
            .collect(),
        None => COMPACT_FIRST_LINE_FIELDS.to_vec(),
    };
    let line_fields: Vec<&str> = match fields {
        Some(ref fs) => fs
            .iter()
            .map(String::as_str)
            .filter(|f| COMPACT_LINE_FIELDS.contains(f))
            .collect(),
        None => COMPACT_LINE_FIELDS.to_vec(),
    };
    issues
        .iter()
        .map(|i| {
            let mut first_parts = Vec::new();
            for field in &first_line_fields {
                match *field {
                    "id" => first_parts.push(format!("ID:{}", i.id)),
                    "status" => first_parts.push(format!("STATUS:{}", i.status)),
                    "priority" => first_parts.push(format!("PRIORITY:{}", i.priority)),
                    "kind" => first_parts.push(format!("KIND:{}", i.kind)),
                    "urgency" => first_parts.push(format!("URGENCY:{:.1}", i.urgency)),
                    "blocked_by" if !i.blocked_by.is_empty() => first_parts.push(format!(
                        "BLOCKED_BY:{}",
                        i.blocked_by
                            .iter()
                            .map(std::string::ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(",")
                    )),
                    _ => {}
                }
            }
            let mut lines = vec![first_parts.join(" ")];
            for field in &line_fields {
                match *field {
                    "tags" if !i.tags.is_empty() => {
                        lines.push(format!("TAGS:{}", escape_line_value(&i.tags.join(","))));
                    }
                    "files" if !i.files.is_empty() => {
                        lines.push(format!("FILES:{}", escape_line_value(&i.files.join(","))));
                    }
                    "skills" if !i.skills.is_empty() => {
                        lines.push(format!("SKILLS:{}", escape_line_value(&i.skills.join(","))));
                    }
                    "assigned_to" if !i.assigned_to.is_empty() => {
                        lines.push(format!("ASSIGNED:{}", escape_line_value(&i.assigned_to)));
                    }
                    "title" => lines.push(format!("TITLE: {}", escape_line_value(&i.title))),
                    "acceptance" if !i.acceptance.is_empty() => {
                        lines.push(format!("ACCEPTANCE: {}", escape_line_value(&i.acceptance)));
                    }
                    // Mirror `get`'s compact `PARENT: N` line so both views agree
                    // (#216). Only rendered when the issue has a parent.
                    "parent_id" => {
                        if let Some(pid) = i.parent_id {
                            lines.push(format!("PARENT: {pid}"));
                        }
                    }
                    _ => {}
                }
            }
            lines.retain(|l| !l.is_empty());
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Every column the pretty list table can render:
/// `(field_name, header, width, right_align)`. The final selected column is
/// always rendered unpadded, so `width` only applies to non-final positions.
const PRETTY_LIST_COLS: &[(&str, &str, usize, bool)] = &[
    ("id", "#", 3, true),
    ("urgency", "Urg", 5, true),
    ("status", "Status", 11, false),
    ("priority", "Pri", 8, false),
    ("kind", "Kind", 7, false),
    ("assigned_to", "Assignee", 10, false),
    ("title", "Title", 40, false),
    ("blocked_by", "Blocked", 8, false),
    ("is_blocked", "Blk", 5, false),
    ("tags", "Tags", 16, false),
    ("files", "Files", 20, false),
    ("skills", "Skills", 12, false),
    ("acceptance", "Acceptance", 30, false),
    ("created_at", "Created", 20, false),
    ("updated_at", "Updated", 20, false),
];

/// Columns shown when no `--fields` filter is set — the historical fixed set.
const PRETTY_LIST_DEFAULT_FIELDS: &[&str] = &[
    "id",
    "urgency",
    "status",
    "priority",
    "kind",
    "assigned_to",
    "title",
    "blocked_by",
];

fn format_issue_list_pretty(issues: &[IssueSummary]) -> String {
    if issues.is_empty() {
        return String::new();
    }
    let fields = get_fields_filter();

    // With --fields, build the column set in the requested order (unknown
    // names were already warned about and are skipped here); without it,
    // keep the historical default columns.
    let selected: Vec<&str> = match fields {
        Some(ref fs) => fs
            .iter()
            .map(String::as_str)
            .filter(|f| PRETTY_LIST_COLS.iter().any(|(name, ..)| name == f))
            .collect(),
        None => PRETTY_LIST_DEFAULT_FIELDS.to_vec(),
    };
    let cols: Vec<&(&str, &str, usize, bool)> = selected
        .iter()
        .filter_map(|f| PRETTY_LIST_COLS.iter().find(|(name, ..)| name == f))
        .collect();
    if cols.is_empty() {
        return String::new();
    }

    let last = cols.len() - 1;
    let header_parts: Vec<String> = cols
        .iter()
        .enumerate()
        .map(|(idx, (_, h, w, right))| {
            if idx == last {
                h.to_string()
            } else {
                pad_display(h, *w, *right)
            }
        })
        .collect();
    let header = format!(" {}", header_parts.join(" | "));
    let separator: String = header
        .chars()
        .map(|c| if c == '|' { '|' } else { '-' })
        .collect();

    let mut lines = vec![header, separator];
    for i in issues {
        let cell_parts: Vec<String> = cols
            .iter()
            .enumerate()
            .map(|(idx, (f, _, w, right))| {
                let val = match *f {
                    "id" => format!("{}", i.id),
                    "urgency" => format!("{:.1}", i.urgency),
                    "status" => i.status.clone(),
                    "priority" => i.priority.clone(),
                    "kind" => i.kind.clone(),
                    "assigned_to" => truncate_with_ellipsis(&i.assigned_to, 10),
                    "title" => truncate_with_ellipsis(&i.title, 40),
                    "blocked_by" => i
                        .blocked_by
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    "is_blocked" => i.is_blocked.to_string(),
                    "tags" => truncate_with_ellipsis(&i.tags.join(","), 16),
                    "files" => truncate_with_ellipsis(&i.files.join(","), 20),
                    "skills" => truncate_with_ellipsis(&i.skills.join(","), 12),
                    "acceptance" => truncate_with_ellipsis(&i.acceptance, 30),
                    "created_at" => i.created_at.clone(),
                    "updated_at" => i.updated_at.clone(),
                    _ => String::new(),
                };
                if idx == last {
                    val
                } else {
                    // Display-width-aware padding so double-width (CJK) cells
                    // keep the column separators aligned (issue #196).
                    pad_display(&val, *w, *right)
                }
            })
            .collect();
        lines.push(format!(" {}", cell_parts.join(" | ")));
    }
    lines.join("\n")
}

// --- Stats ---

pub fn format_stats(stats: &Stats, fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&stats_to_deterministic_json(stats)),
        Format::Compact | Format::Pretty | Format::Oneline => {
            // Compact/pretty/oneline share the labeled compact lines and have
            // no field filtering (issue #197).
            warn_fields_unsupported("stats non-JSON output");
            format_stats_compact(stats)
        }
    }
}

/// Serialize [`Stats`] to JSON with a deterministic contract.
///
/// The `Stats` struct stores its count buckets in `HashMap`s, whose iteration
/// order is randomized per process — serializing it directly produces
/// byte-different output for semantically identical data, which makes byte-level
/// snapshot tests flap (issue #139). This builds the JSON with alphabetical
/// top-level keys and **sorted** nested count-map keys, preserving the exact
/// same JSON shape and values while removing the nondeterminism. The nested
/// `oldest_open` object's keys are likewise sorted alphabetically
/// (`days_old`, `id`, `title`). See `docs/command-contracts.md` for the
/// documented contract.
fn stats_to_deterministic_json(stats: &Stats) -> String {
    use serde_json::{Map, Value};
    use std::collections::BTreeMap;

    // Exhaustive destructure — deliberately no `..` rest pattern (issue #200).
    // This function replaced derived serialization to control the byte-level
    // contract, so adding a field to `Stats` must be a compile error here
    // until the JSON builder below includes it; otherwise the new field would
    // silently vanish from `stats -f json`.
    let Stats {
        total,
        by_status,
        by_priority,
        by_kind,
        blocked,
        ready,
        avg_urgency,
        by_skills,
        by_assignee,
        oldest_open,
    } = stats;

    // Nested count maps: sort keys for a stable, deterministic order.
    let ordered_map = |m: &std::collections::HashMap<String, i64>| -> Value {
        let sorted: BTreeMap<&String, &i64> = m.iter().collect();
        let mut obj = Map::new();
        for (k, v) in sorted {
            obj.insert(k.clone(), Value::from(*v));
        }
        Value::Object(obj)
    };

    // Top-level object. `serde_json::Map` is insertion-ordered now that the
    // `preserve_order` feature is enabled (needed so --fields can honor the
    // requested field order), so the documented alphabetical contract is kept
    // by inserting the keys in alphabetical order explicitly. Combined with
    // the sorted nested maps above, the whole `Stats` object is deterministic.
    let oldest_open_value = {
        // Round-trip through Value, then sort the object keys (days_old, id,
        // title) to keep the documented alphabetical nested contract.
        let v = serde_json::to_value(oldest_open).unwrap_or(Value::Null);
        if let Value::Object(map) = v {
            let sorted: BTreeMap<String, Value> = map.into_iter().collect();
            Value::Object(sorted.into_iter().collect())
        } else {
            v
        }
    };
    let mut obj = Map::new();
    obj.insert("avg_urgency".to_string(), round_urgency_value(*avg_urgency));
    obj.insert("blocked".to_string(), Value::from(*blocked));
    obj.insert("by_assignee".to_string(), ordered_map(by_assignee));
    obj.insert("by_kind".to_string(), ordered_map(by_kind));
    obj.insert("by_priority".to_string(), ordered_map(by_priority));
    obj.insert("by_skills".to_string(), ordered_map(by_skills));
    obj.insert("by_status".to_string(), ordered_map(by_status));
    obj.insert("oldest_open".to_string(), oldest_open_value);
    obj.insert("ready".to_string(), Value::from(*ready));
    obj.insert("total".to_string(), Value::from(*total));

    Value::Object(obj).to_string()
}

fn format_stats_compact(stats: &Stats) -> String {
    let mut lines = Vec::new();
    lines.push(format!("TOTAL:{}", stats.total));
    lines.push(format!(
        "BY_STATUS: open={} in-progress={} done={} wontfix={}",
        stats.by_status.get("open").unwrap_or(&0),
        stats.by_status.get("in-progress").unwrap_or(&0),
        stats.by_status.get("done").unwrap_or(&0),
        stats.by_status.get("wontfix").unwrap_or(&0),
    ));
    lines.push(format!(
        "BY_PRIORITY: critical={} high={} medium={} low={}",
        stats.by_priority.get("critical").unwrap_or(&0),
        stats.by_priority.get("high").unwrap_or(&0),
        stats.by_priority.get("medium").unwrap_or(&0),
        stats.by_priority.get("low").unwrap_or(&0),
    ));
    lines.push(format!(
        "BY_KIND: bug={} feature={} task={} epic={}",
        stats.by_kind.get("bug").unwrap_or(&0),
        stats.by_kind.get("feature").unwrap_or(&0),
        stats.by_kind.get("task").unwrap_or(&0),
        stats.by_kind.get("epic").unwrap_or(&0),
    ));
    lines.push(format!("BLOCKED:{} READY:{}", stats.blocked, stats.ready));
    lines.push(format!("AVG_URGENCY:{:.1}", stats.avg_urgency));
    if !stats.by_skills.is_empty() {
        let mut skill_pairs: Vec<(&String, &i64)> = stats.by_skills.iter().collect();
        skill_pairs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let parts: Vec<String> = skill_pairs
            .iter()
            .map(|(k, v)| format!("{}={}", escape_line_value(k), v))
            .collect();
        lines.push(format!("BY_SKILLS: {}", parts.join(" ")));
    }
    if !stats.by_assignee.is_empty() {
        let mut pairs: Vec<(&String, &i64)> = stats.by_assignee.iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let parts: Vec<String> = pairs
            .iter()
            .map(|(k, v)| format!("{}={}", escape_line_value(k), v))
            .collect();
        lines.push(format!("BY_ASSIGNEE: {}", parts.join(" ")));
    }
    if let Some(ref oldest) = stats.oldest_open {
        lines.push(format!(
            "OLDEST_OPEN: ID:{} DAYS:{} \"{}\"",
            oldest.id,
            oldest.days_old,
            escape_quoted_value(&oldest.title)
        ));
    }
    lines.join("\n")
}

// --- Graph ---

/// Render a dependency / blocker graph.
///
/// `Pretty` and `Oneline` both emit Graphviz DOT (`digraph itr { ... }`);
/// `Compact` emits one `NODE:…` / `EDGE:…` line per element; `Json` serializes
/// the raw [`GraphOutput`] struct.
///
/// # Examples
///
/// ```text
/// use itr::format::{format_graph, Format};
/// use itr::models::GraphOutput;
/// let empty = GraphOutput { nodes: vec![], edges: vec![] };
/// assert!(format_graph(&empty, Format::Pretty).contains("digraph itr"));
/// ```
pub fn format_graph(graph: &GraphOutput, fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&graph_to_deterministic_json(graph)),
        Format::Compact => {
            warn_fields_unsupported("graph compact output");
            format_graph_compact(graph)
        }
        Format::Pretty | Format::Oneline => {
            warn_fields_unsupported("graph DOT output");
            format_graph_dot(graph)
        }
    }
}

/// Serialize [`GraphOutput`] to JSON with a deterministic urgency-precision
/// contract.
///
/// Node urgency is an `f64` computed fresh from current state, so values like
/// `9.00019212962963` leak into JSON and make byte-level snapshots flap on
/// runs that differ only in float noise (issue #139). This rounds each node's
/// urgency to [`URGENCY_JSON_DECIMALS`] decimal places at the serialization
/// boundary — the ranking math is untouched, only the rendered precision is
/// pinned. See `docs/command-contracts.md` for the documented contract.
///
/// The rounding happens on a clone of the struct so the serde-derived
/// `Serialize` impl emits fields in declaration order (`nodes` before
/// `edges`; node keys `id`, `title`, `status`, `urgency`, `is_blocked`).
/// Round-tripping through `serde_json::Value` instead would re-sort every
/// object key alphabetically (the default `Map` is a `BTreeMap`) — an
/// undocumented whole-document reorder (issue #179).
fn graph_to_deterministic_json(graph: &GraphOutput) -> String {
    let mut rounded = graph.clone();
    for node in &mut rounded.nodes {
        node.urgency = round_urgency(node.urgency);
    }
    serde_json::to_string(&rounded).unwrap_or_default()
}

/// Number of decimal places urgency is rounded to in JSON output. Keeps
/// parseable formats byte-stable without affecting urgency ranking.
const URGENCY_JSON_DECIMALS: i32 = 4;

/// Round an urgency `f64` to the JSON precision contract. NaN/Inf (which
/// `serde_json` cannot represent) fall back to `0.0`.
fn round_urgency(urgency: f64) -> f64 {
    let factor = 10f64.powi(URGENCY_JSON_DECIMALS);
    let rounded = (urgency * factor).round() / factor;
    if rounded.is_finite() {
        rounded
    } else {
        0.0
    }
}

/// [`round_urgency`] wrapped as a JSON number `Value`. Integral results (e.g.
/// `9.0`) serialize as `9.0` because the source value is always a float;
/// callers that need it embedded in an object should insert the returned
/// `Value` directly.
fn round_urgency_value(urgency: f64) -> serde_json::Value {
    serde_json::Number::from_f64(round_urgency(urgency))
        .map_or_else(|| serde_json::Value::from(0.0), serde_json::Value::Number)
}

fn format_graph_compact(graph: &GraphOutput) -> String {
    let mut lines = Vec::new();
    for node in &graph.nodes {
        let blocked = if node.is_blocked { " [BLOCKED]" } else { "" };
        lines.push(format!(
            "NODE:{} STATUS:{} URGENCY:{:.1}{} \"{}\"",
            node.id,
            node.status,
            node.urgency,
            blocked,
            escape_quoted_value(&node.title)
        ));
    }
    for edge in &graph.edges {
        lines.push(format!(
            "EDGE: {} -> {} ({})",
            edge.from, edge.to, edge.edge_type
        ));
    }
    lines.join("\n")
}

fn format_graph_dot(graph: &GraphOutput) -> String {
    let mut lines = Vec::new();
    lines.push("digraph itr {".to_string());
    lines.push("  rankdir=LR;".to_string());
    for node in &graph.nodes {
        // Truncate first, then escape, so escape sequences are never cut in
        // half by the truncation (issue #176).
        let title_short = escape_dot_label(&truncate_with_ellipsis(&node.title, 30));
        let style = if node.is_blocked {
            " style=filled fillcolor=gray"
        } else {
            ""
        };
        lines.push(format!(
            "  {} [label=\"{}: {}\" shape=box{}]",
            node.id, node.id, title_short, style
        ));
    }
    for edge in &graph.edges {
        lines.push(format!("  {} -> {}", edge.from, edge.to));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

// --- Display width, padding, and truncation helpers ---

/// Inclusive Unicode codepoint ranges rendered as two terminal columns.
///
/// This is a deliberate approximation of the East Asian Wide/Fullwidth
/// property (issue #196): it covers the common CJK blocks (Hangul, CJK
/// radicals/symbols, Hiragana/Katakana, CJK Unified Ideographs + extensions,
/// Hangul Syllables, compatibility ideographs/forms, fullwidth forms) and
/// defaults everything else — including combining marks and most emoji — to
/// width 1. It is not exhaustive Unicode-correctness; it exists so pretty
/// tables align for the overwhelmingly common double-width inputs without
/// pulling in a new dependency.
const DOUBLE_WIDTH_RANGES: &[(u32, u32)] = &[
    (0x1100, 0x115F),   // Hangul Jamo (leading consonants)
    (0x2E80, 0x303E),   // CJK Radicals .. CJK Symbols and Punctuation
    (0x3041, 0x33FF),   // Hiragana, Katakana, Kanbun, CJK Compatibility
    (0x3400, 0x4DBF),   // CJK Unified Ideographs Extension A
    (0x4E00, 0x9FFF),   // CJK Unified Ideographs
    (0xA000, 0xA4CF),   // Yi Syllables and Radicals
    (0xAC00, 0xD7A3),   // Hangul Syllables
    (0xF900, 0xFAFF),   // CJK Compatibility Ideographs
    (0xFE30, 0xFE4F),   // CJK Compatibility Forms
    (0xFF00, 0xFF60),   // Fullwidth Forms
    (0xFFE0, 0xFFE6),   // Fullwidth Signs
    (0x20000, 0x2FFFD), // CJK Unified Ideographs Extensions B-F
    (0x30000, 0x3FFFD), // CJK Unified Ideographs Extension G
];

/// Approximate terminal display width of one char: 2 for the common
/// double-width CJK/fullwidth blocks, 1 for everything else.
fn char_display_width(c: char) -> usize {
    let cp = c as u32;
    if DOUBLE_WIDTH_RANGES
        .iter()
        .any(|&(lo, hi)| (lo..=hi).contains(&cp))
    {
        2
    } else {
        1
    }
}

/// Approximate terminal display width of a string (sum of char widths).
fn display_width(s: &str) -> usize {
    s.chars().map(char_display_width).sum()
}

/// Pad `s` with spaces to `width` display columns (left- or right-aligned).
/// Strings already at or beyond `width` are returned unchanged — identical to
/// `format!("{:<width$}")` semantics, but counting display columns instead of
/// chars so double-width (CJK) cells keep table separators aligned
/// (issue #196). For pure-ASCII input the output is byte-identical to the
/// `format!` width specifiers this replaced.
fn pad_display(s: &str, width: usize, right_align: bool) -> String {
    let w = display_width(s);
    if w >= width {
        return s.to_string();
    }
    let pad = " ".repeat(width - w);
    if right_align {
        format!("{pad}{s}")
    } else {
        format!("{s}{pad}")
    }
}

/// Truncate a string to fit within `max_cols` display columns, appending
/// "..." if truncated.
///
/// Width is measured with [`display_width`] (double-width-aware, issue #196),
/// and truncation iterates chars so it can never split a UTF-8 sequence. A
/// double-width char that would straddle the cut point is dropped entirely,
/// so the result may come up one column short of `max_cols`. For pure-ASCII
/// input this matches the old byte-based behavior exactly.
fn truncate_with_ellipsis(s: &str, max_cols: usize) -> String {
    if display_width(s) <= max_cols {
        return s.to_string();
    }
    let budget = max_cols.saturating_sub(3); // room for "..."
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = char_display_width(c);
        if used + w > budget {
            break;
        }
        used += w;
        out.push(c);
    }
    out.push_str("...");
    out
}

// --- Search Results ---

pub fn format_search_results(results: &[SearchResult], fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(results).unwrap_or_default()),
        Format::Compact => format_search_compact(results),
        Format::Pretty | Format::Oneline => {
            warn_fields_unsupported("search pretty/oneline output");
            format_search_pretty(results)
        }
    }
}

fn format_search_compact(results: &[SearchResult]) -> String {
    // Honor the --fields filter like the other compact formatters do
    // (issue #197): with no filter set, every `on(...)` check is true and the
    // output is byte-identical to the unfiltered contract.
    let fields = get_fields_filter();
    let on = |name: &str| field_enabled(fields.as_ref(), name);
    results
        .iter()
        .map(|r| {
            let mut first_parts = Vec::new();
            if on("id") {
                first_parts.push(format!("ID:{}", r.id));
            }
            if on("status") {
                first_parts.push(format!("STATUS:{}", r.status));
            }
            if on("priority") {
                first_parts.push(format!("PRIORITY:{}", r.priority));
            }
            if on("kind") {
                first_parts.push(format!("KIND:{}", r.kind));
            }
            if on("urgency") {
                first_parts.push(format!("URGENCY:{:.1}", r.urgency));
            }
            if on("matched_fields") {
                first_parts.push(format!("MATCHED:{}", r.matched_fields.join(",")));
            }
            if on("blocked_by") && !r.blocked_by.is_empty() {
                first_parts.push(format!(
                    "BLOCKED_BY:{}",
                    r.blocked_by
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            let mut lines = vec![first_parts.join(" ")];
            if on("tags") && !r.tags.is_empty() {
                lines.push(format!("TAGS:{}", escape_line_value(&r.tags.join(","))));
            }
            if on("files") && !r.files.is_empty() {
                lines.push(format!("FILES:{}", escape_line_value(&r.files.join(","))));
            }
            if on("skills") && !r.skills.is_empty() {
                lines.push(format!("SKILLS:{}", escape_line_value(&r.skills.join(","))));
            }
            if on("assigned_to") && !r.assigned_to.is_empty() {
                lines.push(format!("ASSIGNED:{}", escape_line_value(&r.assigned_to)));
            }
            if on("title") {
                lines.push(format!("TITLE: {}", escape_line_value(&r.title)));
            }
            if on("acceptance") && !r.acceptance.is_empty() {
                lines.push(format!("ACCEPTANCE: {}", escape_line_value(&r.acceptance)));
            }
            if on("context_snippets") {
                if let Some(ref snippets) = r.context_snippets {
                    for (field, snippet) in snippets {
                        lines.push(format!(
                            "SNIPPET[{}]: {}",
                            field,
                            escape_line_value(snippet)
                        ));
                    }
                }
            }
            lines.retain(|l| !l.is_empty());
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_search_pretty(results: &[SearchResult]) -> String {
    if results.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push(format!(
        " {} | {} | {} | {} | {} | {} | Matched",
        pad_display("#", 3, true),
        pad_display("Urg", 5, true),
        pad_display("Status", 11, false),
        pad_display("Pri", 8, false),
        pad_display("Kind", 7, false),
        pad_display("Title", 40, false),
    ));
    lines.push(
        "-----|-------|-------------|----------|---------|------------------------------------------|--------"
            .to_string(),
    );
    for r in results {
        let title = truncate_with_ellipsis(&r.title, 40);
        let matched = r.matched_fields.join(",");
        // Display-width-aware padding keeps separators aligned for
        // double-width (CJK) cells (issue #196).
        lines.push(format!(
            " {} | {} | {} | {} | {} | {} | {}",
            pad_display(&r.id.to_string(), 3, true),
            pad_display(&format!("{:.1}", r.urgency), 5, true),
            pad_display(&r.status, 11, false),
            pad_display(&r.priority, 8, false),
            pad_display(&r.kind, 7, false),
            pad_display(&title, 40, false),
            matched
        ));
    }
    lines.join("\n")
}

// --- Events (Audit Log) ---

pub fn format_events(events: &[Event], fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(events).unwrap_or_default()),
        Format::Compact => {
            warn_fields_unsupported("log compact output");
            format_events_compact(events)
        }
        Format::Pretty | Format::Oneline => {
            warn_fields_unsupported("log pretty output");
            format_events_pretty(events)
        }
    }
}

fn format_events_compact(events: &[Event]) -> String {
    events
        .iter()
        .map(|e| {
            let agent_str = if e.agent.is_empty() {
                String::new()
            } else {
                format!(" AGENT:{}", escape_line_value(&e.agent))
            };
            // OLD/NEW are free text (often multi-word, possibly containing
            // literal ` NEW:` or quotes): double-quote them with internal
            // escaping so a parser can recover the exact values (issue #177).
            format!(
                "EVENT:{} ISSUE:{} FIELD:{} OLD:\"{}\" NEW:\"{}\"{} ({})",
                e.id,
                e.issue_id,
                e.field,
                escape_quoted_value(&e.old_value),
                escape_quoted_value(&e.new_value),
                agent_str,
                e.created_at
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_events_pretty(events: &[Event]) -> String {
    if events.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push(format!(
        " {} | {} | {} | {} | {} | {} | {}",
        pad_display("ID", 4, true),
        pad_display("Issue", 5, true),
        pad_display("Field", 15, false),
        pad_display("Old", 20, false),
        pad_display("New", 20, false),
        pad_display("Agent", 15, false),
        "Time"
    ));
    lines.push(
        "------|-------|-----------------|----------------------|----------------------|-----------------|--------------------"
            .to_string(),
    );
    for e in events {
        let old = truncate_with_ellipsis(&e.old_value, 20);
        let new = truncate_with_ellipsis(&e.new_value, 20);
        let agent = truncate_with_ellipsis(&e.agent, 15);
        // Display-width-aware padding keeps separators aligned for
        // double-width (CJK) cells (issue #196).
        lines.push(format!(
            " {} | {} | {} | {} | {} | {} | {}",
            pad_display(&e.id.to_string(), 4, true),
            pad_display(&e.issue_id.to_string(), 5, true),
            pad_display(&e.field, 15, false),
            pad_display(&old, 20, false),
            pad_display(&new, 20, false),
            pad_display(&agent, 15, false),
            e.created_at
        ));
    }
    lines.join("\n")
}

// --- JSON field filtering ---

const VALID_FIELDS: &[&str] = &[
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
    "urgency",
    "blocked_by",
    "blocks",
    "is_blocked",
    "notes",
    "urgency_breakdown",
    "children",
    "matched_fields",
    "unblocked",
    "context_snippets",
    "relations",
    // Batch result fields
    "action",
    "results",
    "summary",
    "outcome",
    "error",
    "total",
    "ok",
    "review",
    "dry_run",
    // Stats fields (stats -f json top-level filtering, issue #197)
    "by_status",
    "by_priority",
    "by_kind",
    "blocked",
    "ready",
    "avg_urgency",
    "by_skills",
    "by_assignee",
    "oldest_open",
    // Graph fields (graph -f json top-level filtering, issue #197)
    "nodes",
    "edges",
    // Event fields (log -f json filtering, issue #197)
    "issue_id",
    "field",
    "old_value",
    "new_value",
    "agent",
];

/// Parse a `--fields` argument like `id,title,urgency` into a normalized
/// list of field names.
///
/// Whitespace around entries is trimmed; empty entries are dropped silently.
/// Unknown field names are *not* rejected here — see [`validate_fields`] for
/// the soft-fallback warning step.
///
/// # Examples
///
/// ```text
/// use itr::format::parse_fields;
/// assert_eq!(parse_fields("id,title"), vec!["id", "title"]);
/// assert_eq!(parse_fields(" id , , urgency "), vec!["id", "urgency"]);
/// assert!(parse_fields("").is_empty());
/// ```
pub fn parse_fields(fields_str: &str) -> Vec<String> {
    fields_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Warn about unknown field names on stderr, but never drop or reject them —
/// the caller keeps the filter intact. Unknown fields simply won't appear in
/// output because serde has nothing matching them.
///
/// Soft-fallback by design: a typo in `--fields` produces a `REVIEW:` note
/// rather than a hard error.
///
/// # Examples
///
/// ```text
/// use itr::format::validate_fields;
/// // No stderr output expected — all valid:
/// validate_fields(&["id".into(), "title".into()]);
/// // Emits a REVIEW: note to stderr but does not panic:
/// validate_fields(&["bogus".into()]);
/// ```
pub fn validate_fields(fields: &[String]) {
    for f in fields {
        if !VALID_FIELDS.contains(&f.as_str()) {
            eprintln!(
                "REVIEW: unknown field '{}' — will be ignored if not present in output. Valid: {}",
                f,
                VALID_FIELDS.join(", ")
            );
        }
    }
}

pub fn filter_json_fields(value: serde_json::Value, fields: &[String]) -> serde_json::Value {
    match value {
        serde_json::Value::Array(arr) => serde_json::Value::Array(
            arr.into_iter()
                .map(|v| filter_json_object(v, fields))
                .collect(),
        ),
        obj @ serde_json::Value::Object(_) => filter_json_object(obj, fields),
        other => other,
    }
}

fn filter_json_object(value: serde_json::Value, fields: &[String]) -> serde_json::Value {
    if let serde_json::Value::Object(mut map) = value {
        // Rebuild in the requested --fields order (spec P4: all formats honor
        // order). Requires serde_json's preserve_order feature, otherwise the
        // Map would re-sort keys alphabetically.
        let mut filtered = serde_json::Map::new();
        for f in fields {
            if let Some(v) = map.remove(f) {
                filtered.insert(f.clone(), v);
            }
        }
        serde_json::Value::Object(filtered)
    } else {
        value
    }
}

// --- Unblocked notifications ---

pub fn format_unblocked(issues: &[(i64, String)], fmt: Format) -> String {
    if issues.is_empty() {
        return String::new();
    }
    match fmt {
        Format::Json => {
            let list: Vec<UnblockedIssue> = issues
                .iter()
                .map(|(id, title)| UnblockedIssue {
                    id: *id,
                    title: title.clone(),
                })
                .collect();
            serde_json::to_string(&list).unwrap_or_default()
        }
        _ => issues
            .iter()
            .map(|(id, title)| format!("UNBLOCKED:{} \"{}\"", id, escape_quoted_value(title)))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

// --- Batch Results ---

pub fn format_batch_result(result: &BatchResult, fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(result).unwrap_or_default()),
        Format::Compact | Format::Pretty | Format::Oneline => {
            warn_fields_unsupported("batch non-JSON output");
            format_batch_result_compact(result)
        }
    }
}

fn format_batch_result_compact(result: &BatchResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{}: {} items ({} ok, {} error, {} review){}",
        result.action.to_uppercase(),
        result.summary.total,
        result.summary.ok,
        result.summary.error,
        result.summary.review,
        if result.dry_run {
            " (dry-run — nothing written)"
        } else {
            ""
        },
    ));
    for item in &result.results {
        match item.outcome.as_str() {
            "ok" => {
                lines.push(format!("  OK:{}", item.id));
                for ub in &item.unblocked {
                    lines.push(format!(
                        "  UNBLOCKED:{} \"{}\"",
                        ub.id,
                        escape_quoted_value(&ub.title)
                    ));
                }
                for note in &item.notes {
                    lines.push(format!(
                        "  NOTE:{} \"{}\"",
                        item.id,
                        escape_quoted_value(note)
                    ));
                }
            }
            "error" => {
                let msg = item.error.as_deref().unwrap_or("unknown error");
                lines.push(format!(
                    "  ERROR:{} \"{}\"",
                    item.id,
                    escape_quoted_value(msg)
                ));
            }
            "review" => {
                lines.push(format!("  REVIEW:{}", item.id));
                for note in &item.notes {
                    lines.push(format!(
                        "  NOTE:{} \"{}\"",
                        item.id,
                        escape_quoted_value(note)
                    ));
                }
            }
            _ => {
                lines.push(format!("  {}:{}", item.outcome.to_uppercase(), item.id));
            }
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        Event, GraphEdge, GraphNode, GraphOutput, Issue, IssueDetail, IssueSummary, OldestOpen,
    };
    use std::collections::HashMap;

    /// RAII guard for tests that exercise the thread-local `--fields` filter:
    /// installs the filter on construction and clears it on drop so no other
    /// assertion on this thread observes a leftover filter.
    struct FieldsFilterGuard;

    impl FieldsFilterGuard {
        fn set(fields: &[&str]) -> Self {
            set_fields_filter(fields.iter().map(|s| (*s).to_string()).collect());
            FieldsFilterGuard
        }
    }

    impl Drop for FieldsFilterGuard {
        fn drop(&mut self) {
            FIELDS_FILTER.with(|f| *f.borrow_mut() = None);
        }
    }

    // --- truncate_with_ellipsis unit tests ---

    #[test]
    fn truncate_ascii_short_unchanged() {
        assert_eq!(truncate_with_ellipsis("hello", 40), "hello");
    }

    #[test]
    fn truncate_ascii_exact_limit_unchanged() {
        let s = "a".repeat(40);
        assert_eq!(truncate_with_ellipsis(&s, 40), s);
    }

    #[test]
    fn truncate_ascii_over_limit() {
        let s = "a".repeat(50);
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 40);
    }

    #[test]
    fn truncate_multibyte_em_dash_at_boundary() {
        // em dash '—' is 3 bytes (U+2014) but 1 display column. The cut point
        // must never split a UTF-8 sequence.
        let s = format!("{}—this continues past the limit!!", "a".repeat(35));
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(display_width(&result) <= 40);
    }

    #[test]
    fn truncate_multibyte_emoji_at_boundary() {
        // '🚀' is 4 bytes. Place near the cut point.
        let s = format!("{}🚀 and more stuff here!!", "a".repeat(35));
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(display_width(&result) <= 40);
    }

    #[test]
    fn truncate_all_multibyte() {
        // String of only 3-byte chars (em dashes, 1 column each): 50 chars =
        // 150 bytes but 50 display columns, so it truncates at the column
        // budget, never mid-sequence.
        let s = "—".repeat(50);
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(display_width(&result) <= 40);
        assert_eq!(result, format!("{}...", "—".repeat(37)));
        // Validates it's valid UTF-8 (would panic if not)
        let _ = result.chars().count();
    }

    #[test]
    fn truncate_two_byte_chars_at_boundary() {
        // 'é' is 2 bytes (U+00E9) but 1 display column
        let s = format!("{}é more text after here!!", "a".repeat(36));
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(display_width(&result) <= 40);
    }

    #[test]
    fn truncate_graph_dot_limit() {
        // Graph DOT uses limit=30. em dash at the column-27 boundary.
        let s = format!("{}—continues on and on", "a".repeat(27));
        let result = truncate_with_ellipsis(&s, 30);
        assert!(result.ends_with("..."));
        assert!(display_width(&result) <= 30);
    }

    // --- Format::from_str case-insensitivity (issue #192) ---

    #[test]
    fn format_from_str_is_case_insensitive() {
        // Issue #192: every other enum-ish input (priority/kind/status) is
        // case-folded before matching; `--format` must be too.
        assert_eq!(Format::from_str("JSON"), Some(Format::Json));
        assert_eq!(Format::from_str("Json"), Some(Format::Json));
        assert_eq!(Format::from_str("COMPACT"), Some(Format::Compact));
        assert_eq!(Format::from_str("Pretty"), Some(Format::Pretty));
        assert_eq!(Format::from_str("OneLine"), Some(Format::Oneline));
        assert_eq!(Format::from_str(" json "), Some(Format::Json));
    }

    #[test]
    fn format_from_str_unknown_stays_none() {
        // Truly unknown values keep the existing hard-error path in main.rs
        // (the integration suite pins `-f bogus` → exit 1 with the
        // enumerated valid-formats message).
        assert_eq!(Format::from_str("bogus"), None);
        assert_eq!(Format::from_str(""), None);
        assert_eq!(Format::from_str("jsonl"), None);
    }

    // --- Display width approximation (issue #196) ---

    #[test]
    fn char_display_width_common_blocks() {
        assert_eq!(char_display_width('漢'), 2); // CJK Unified Ideographs
        assert_eq!(char_display_width('あ'), 2); // Hiragana
        assert_eq!(char_display_width('カ'), 2); // Katakana
        assert_eq!(char_display_width('한'), 2); // Hangul Syllables
        assert_eq!(char_display_width('Ａ'), 2); // Fullwidth Latin
        assert_eq!(char_display_width('。'), 2); // CJK punctuation
        assert_eq!(char_display_width('a'), 1);
        assert_eq!(char_display_width('é'), 1);
        // Documented approximation: non-CJK multibyte defaults to width 1.
        assert_eq!(char_display_width('—'), 1);
    }

    #[test]
    fn display_width_sums_char_widths() {
        assert_eq!(display_width("abc"), 3);
        assert_eq!(display_width("漢字"), 4);
        assert_eq!(display_width("a漢b"), 4);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn pad_display_counts_columns_not_chars() {
        // ASCII: byte-identical to the format! width specifiers it replaced.
        assert_eq!(pad_display("ab", 5, false), "ab   ");
        assert_eq!(pad_display("ab", 5, true), "   ab");
        // Never truncates: already at/over width returns the input unchanged.
        assert_eq!(pad_display("abcdef", 5, false), "abcdef");
        // CJK: two ideographs occupy 4 columns, so only 1 pad space remains.
        assert_eq!(pad_display("漢字", 5, false), "漢字 ");
        assert_eq!(pad_display("漢字", 5, true), " 漢字");
    }

    #[test]
    fn truncate_cjk_by_display_columns() {
        // Issue #196: 30 ideographs are 60 display columns. With a 40-column
        // budget, 18 ideographs (36 cols) + "..." (3 cols) fit; a 19th would
        // overflow. Byte-based truncation used to cut CJK at ~1/3 the visible
        // length of ASCII.
        let s = "漢".repeat(30);
        let out = truncate_with_ellipsis(&s, 40);
        assert_eq!(out, format!("{}...", "漢".repeat(18)));
        assert!(display_width(&out) <= 40);
    }

    // --- format_issue_list_pretty with multi-byte titles ---

    fn make_summary(title: &str) -> IssueSummary {
        IssueSummary {
            id: 1,
            title: title.to_string(),
            status: "open".to_string(),
            priority: "medium".to_string(),
            kind: "task".to_string(),
            urgency: 5.0,
            is_blocked: false,
            blocked_by: vec![],
            tags: vec![],
            files: vec![],
            skills: vec![],
            acceptance: String::new(),
            parent_id: None,
            assigned_to: String::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn pretty_list_with_em_dash_title_does_not_panic() {
        // This is the exact title from the original bug report
        let title =
            "Set up justfile for Rust workspace — verify, build, run, test, fmt, clippy targets";
        let issues = vec![make_summary(title)];
        let result = format_issue_list(&issues, Format::Pretty);
        assert!(result.contains("..."));
    }

    #[test]
    fn pretty_list_with_emoji_title_does_not_panic() {
        let title = "Fix the authentication bug in the 🔐 login flow for all users worldwide";
        let issues = vec![make_summary(title)];
        let result = format_issue_list(&issues, Format::Pretty);
        assert!(result.contains("..."));
    }

    #[test]
    fn pretty_list_short_multibyte_title_no_truncation() {
        let title = "Fix café bug";
        let issues = vec![make_summary(title)];
        let result = format_issue_list(&issues, Format::Pretty);
        assert!(result.contains("Fix café bug"));
        assert!(!result.contains("..."));
    }

    // --- Pretty-table alignment with double-width (CJK) cells (issue #196) ---

    /// Display-column positions of the `|` separators on a line.
    // --- spec P4: --fields on oneline and pretty list output ---

    #[test]
    fn oneline_list_fields_are_tab_separated_in_requested_order() {
        let _guard = FieldsFilterGuard::set(&["id", "status", "title"]);
        let mut summary = make_summary("Fix the thing");
        summary.id = 7;
        summary.status = "open".to_string();
        let out = format_issue_list(&[summary], Format::Oneline);
        assert_eq!(out, "7\topen\tFix the thing");
    }

    #[test]
    fn oneline_list_fields_join_list_values_with_commas() {
        let _guard = FieldsFilterGuard::set(&["id", "tags", "blocked_by"]);
        let mut summary = make_summary("t");
        summary.id = 3;
        summary.tags = vec!["a".to_string(), "b".to_string()];
        summary.blocked_by = vec![1, 2];
        let out = format_issue_list(&[summary], Format::Oneline);
        assert_eq!(out, "3\ta,b\t1,2");
    }

    #[test]
    fn oneline_list_fields_unknown_name_is_empty_cell() {
        // Soft fallback: the unknown name was warned about at parse time;
        // the column count stays stable for scripts.
        let _guard = FieldsFilterGuard::set(&["id", "bogus", "status"]);
        let mut summary = make_summary("t");
        summary.id = 4;
        summary.status = "open".to_string();
        let out = format_issue_list(&[summary], Format::Oneline);
        assert_eq!(out, "4\t\topen");
    }

    #[test]
    fn oneline_list_fields_escape_free_text() {
        let _guard = FieldsFilterGuard::set(&["id", "title"]);
        let mut summary = make_summary("multi\nline\ttitle");
        summary.id = 5;
        let out = format_issue_list(&[summary], Format::Oneline);
        assert_eq!(out, "5\tmulti\\nline\\ttitle");
    }

    #[test]
    fn oneline_list_default_shape_unchanged_without_fields() {
        let mut summary = make_summary("Plain");
        summary.id = 9;
        summary.status = "open".to_string();
        summary.priority = "medium".to_string();
        summary.kind = "task".to_string();
        let out = format_issue_list(&[summary], Format::Oneline);
        assert_eq!(out, "9\topen\tmedium\ttask\t\"Plain\"");
    }

    #[test]
    fn pretty_list_fields_build_columns_in_requested_order() {
        let _guard = FieldsFilterGuard::set(&["status", "id", "title"]);
        let mut summary = make_summary("Ordered");
        summary.id = 2;
        summary.status = "open".to_string();
        let out = format_issue_list(&[summary], Format::Pretty);
        let header = out.lines().next().unwrap();
        let cols: Vec<&str> = header.split('|').map(str::trim).collect();
        assert_eq!(cols, vec!["Status", "#", "Title"]);
        assert!(out.lines().nth(2).unwrap().contains("Ordered"));
    }

    #[test]
    fn pretty_list_fields_support_extra_columns() {
        let _guard = FieldsFilterGuard::set(&["id", "tags", "created_at"]);
        let mut summary = make_summary("Extra");
        summary.id = 2;
        summary.tags = vec!["sprint-9".to_string()];
        summary.created_at = "2026-07-02T00:00:00Z".to_string();
        let out = format_issue_list(&[summary], Format::Pretty);
        let header = out.lines().next().unwrap();
        let cols: Vec<&str> = header.split('|').map(str::trim).collect();
        assert_eq!(cols, vec!["#", "Tags", "Created"]);
        let row = out.lines().nth(2).unwrap();
        assert!(row.contains("sprint-9"));
        assert!(row.contains("2026-07-02T00:00:00Z"));
    }

    #[test]
    fn pretty_list_default_columns_unchanged_without_fields() {
        let out = format_issue_list(&[make_summary("Default")], Format::Pretty);
        let header = out.lines().next().unwrap();
        let cols: Vec<&str> = header.split('|').map(str::trim).collect();
        assert_eq!(
            cols,
            vec!["#", "Urg", "Status", "Pri", "Kind", "Assignee", "Title", "Blocked"]
        );
    }

    #[test]
    fn compact_list_fields_honor_requested_order_within_record_line() {
        let _guard = FieldsFilterGuard::set(&["status", "id"]);
        let mut summary = make_summary("t");
        summary.id = 6;
        summary.status = "open".to_string();
        let out = format_issue_list(&[summary], Format::Compact);
        assert_eq!(out, "STATUS:open ID:6");
    }

    #[test]
    fn json_list_fields_honor_requested_order() {
        let _guard = FieldsFilterGuard::set(&["title", "id"]);
        let mut summary = make_summary("Ord");
        summary.id = 8;
        let out = format_issue_list(&[summary], Format::Json);
        assert_eq!(out, "[{\"title\":\"Ord\",\"id\":8}]");
    }

    fn pipe_display_cols(line: &str) -> Vec<usize> {
        let mut cols = Vec::new();
        let mut col = 0;
        for c in line.chars() {
            if c == '|' {
                cols.push(col);
            }
            col += char_display_width(c);
        }
        cols
    }

    /// Assert every line of a rendered table puts its `|` separators at the
    /// same display columns as the header line.
    fn assert_table_aligned(table: &str) {
        let lines: Vec<&str> = table.lines().collect();
        assert!(lines.len() >= 3, "expected header + separator + rows");
        let expected = pipe_display_cols(lines[0]);
        assert!(
            !expected.is_empty(),
            "no separators in header: {}",
            lines[0]
        );
        for line in &lines[1..] {
            assert_eq!(
                pipe_display_cols(line),
                expected,
                "separators misaligned on line: {line}\nfull table:\n{table}"
            );
        }
    }

    #[test]
    fn pretty_list_aligns_cjk_and_ascii_rows() {
        // Issue #196: one CJK title and one ASCII title must land their
        // column separators at identical display columns.
        let issues = vec![
            make_summary("これは日本語のタイトルです"),
            make_summary("Plain ASCII title"),
        ];
        let out = format_issue_list(&issues, Format::Pretty);
        assert_eq!(out.lines().count(), 4); // header, separator, 2 rows
        assert_table_aligned(&out);
    }

    #[test]
    fn pretty_list_aligns_truncated_cjk_title() {
        // A CJK title wider than the 40-column title cell truncates by
        // display columns and still aligns with an over-long ASCII row.
        let issues = vec![
            make_summary(&"漢".repeat(30)),
            make_summary(&"a".repeat(60)),
        ];
        let out = format_issue_list(&issues, Format::Pretty);
        assert_table_aligned(&out);
        assert!(out.contains(&format!("{}...", "漢".repeat(18))));
    }

    #[test]
    fn pretty_search_aligns_cjk_and_ascii_rows() {
        let results = vec![
            make_search_result("日本語のタイトル検索"),
            make_search_result("ASCII search title"),
        ];
        let out = format_search_results(&results, Format::Pretty);
        assert_table_aligned(&out);
    }

    #[test]
    fn pretty_events_align_cjk_and_ascii_rows() {
        let mk = |old: &str, new: &str| Event {
            id: 1,
            issue_id: 2,
            field: "title".to_string(),
            old_value: old.to_string(),
            new_value: new.to_string(),
            agent: "agent".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        };
        let events = vec![
            mk("古いタイトルです", "新しいタイトルです"),
            mk("old ascii title", "new ascii title"),
        ];
        let out = format_events(&events, Format::Pretty);
        assert_table_aligned(&out);
    }

    #[test]
    fn graph_dot_with_em_dash_title_does_not_panic() {
        let graph = GraphOutput {
            nodes: vec![GraphNode {
                id: 1,
                title: "Some task description — with extra context".to_string(),
                status: "open".to_string(),
                urgency: 5.0,
                is_blocked: false,
            }],
            edges: vec![],
        };
        let result = format_graph(&graph, Format::Pretty);
        assert!(result.contains("..."));
        assert!(result.contains("digraph"));
    }

    // --- Escaping: hostile control characters in free-text fields ---
    // Issues #156/#175/#176/#177: line-oriented output must encode embedded
    // newlines/tabs/quotes so one logical field never spans physical lines
    // and quoted tokens stay parseable.

    /// A record an attacker tries to forge via an embedded newline.
    const FORGED: &str = "ID:777 STATUS:open PRIORITY:critical KIND:bug URGENCY:9.9";

    #[test]
    fn escape_helpers_encode_all_specials() {
        // The project-wide line-value encoding: \, LF, CR, tab — and quotes
        // only in the quoted variant.
        assert_eq!(
            escape_line_value("a\\b\nc\rd\te\"f"),
            "a\\\\b\\nc\\rd\\te\"f"
        );
        assert_eq!(
            escape_quoted_value("a\\b\nc\rd\te\"f"),
            "a\\\\b\\nc\\rd\\te\\\"f"
        );
        assert_eq!(escape_line_value("plain title"), "plain title");
    }

    fn make_detail(title: &str, context: &str) -> IssueDetail {
        IssueDetail {
            issue: Issue {
                id: 1,
                title: title.to_string(),
                status: "open".to_string(),
                priority: "medium".to_string(),
                kind: "task".to_string(),
                context: context.to_string(),
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
            urgency: 5.0,
            blocked_by: vec![],
            blocks: vec![],
            is_blocked: false,
            notes: vec![],
            urgency_breakdown: None,
            children: None,
            relations: vec![],
        }
    }

    #[test]
    fn compact_list_newline_title_cannot_forge_record() {
        // Issue #156: a title embedding a blank line plus a full record must
        // not make `itr list` (compact) emit a fabricated record line.
        let issues = vec![make_summary(&format!("Legit\n\n{}\nTITLE: fake", FORGED))];
        let out = format_issue_list(&issues, Format::Compact);
        assert!(
            out.lines().all(|l| !l.starts_with("ID:777")),
            "forged record leaked as its own line:\n{out}"
        );
        // Exactly one real record line.
        assert_eq!(out.lines().filter(|l| l.starts_with("ID:")).count(), 1);
        // The newline is visibly encoded on a single TITLE line; the forged
        // text is still present but inert mid-line.
        let title_line = out.lines().find(|l| l.starts_with("TITLE:")).unwrap();
        assert!(title_line.contains("\\n"));
        assert!(title_line.contains("ID:777"));
    }

    #[test]
    fn compact_detail_newline_title_and_context_stay_single_line() {
        // Issue #156: `itr get` compact must keep TITLE/CONTEXT on one
        // physical line each, with no unlabeled continuation lines.
        let detail = make_detail(
            "Title line1\nline2",
            &format!("ctx line1\n{}\nline3", FORGED),
        );
        let out = format_issue_detail(&detail, Format::Compact);
        assert_eq!(out.lines().filter(|l| l.starts_with("TITLE:")).count(), 1);
        assert_eq!(out.lines().filter(|l| l.starts_with("CONTEXT:")).count(), 1);
        assert!(
            out.lines().all(|l| !l.starts_with("ID:777")),
            "forged record leaked as its own line:\n{out}"
        );
        let ctx_line = out.lines().find(|l| l.starts_with("CONTEXT:")).unwrap();
        assert!(ctx_line.contains("ctx line1\\nID:777"));
        // record line, TITLE, CONTEXT, CREATED, UPDATED — nothing spills over.
        assert_eq!(out.lines().count(), 5, "unexpected line layout:\n{out}");
    }

    // --- Batched issue details (itr get 1,2,3 — issue #136) ---

    #[test]
    fn batched_details_json_is_array_of_issue_details() {
        let details = vec![make_detail("first", ""), make_detail("second", "ctx")];
        let out = format_issue_details(&details, Format::Json);
        let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
        let arr = parsed.as_array().expect("top-level array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["title"], "first");
        assert_eq!(arr[1]["title"], "second");
        // Full IssueDetail payload per element, not summaries.
        assert!(arr[0].get("is_blocked").is_some());
        assert!(arr[1].get("notes").is_some());
    }

    #[test]
    fn batched_details_compact_blocks_match_single_formatter() {
        // Each batched compact block must be byte-identical to the
        // single-issue compact output, separated by exactly one blank line.
        let a = make_detail("first", "");
        let b = make_detail("second", "ctx");
        let out = format_issue_details(&[a.clone(), b.clone()], Format::Compact);
        let expected = format!(
            "{}\n\n{}",
            format_issue_detail(&a, Format::Compact),
            format_issue_detail(&b, Format::Compact)
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn batched_details_empty_follows_empty_result_contract() {
        assert_eq!(format_issue_details(&[], Format::Json), "[]");
        assert_eq!(format_issue_details(&[], Format::Compact), "");
    }

    #[test]
    fn batched_details_hostile_title_cannot_forge_block_separator() {
        // A title embedding blank lines + a forged record must not create a
        // phantom third block: escaping keeps each block's lines physical.
        let evil = make_detail(&format!("evil\n\n{}", FORGED), "");
        let out = format_issue_details(&[evil, make_detail("plain", "")], Format::Compact);
        let blocks: Vec<&str> = out.split("\n\n").collect();
        assert_eq!(blocks.len(), 2, "blank-line separator forged:\n{out}");
        assert!(
            out.lines().all(|l| !l.starts_with("ID:777")),
            "forged record leaked as its own line:\n{out}"
        );
    }

    #[test]
    fn compact_graph_node_newline_title_stays_single_line() {
        // Issue #156: graph compact NODE lines must not split on hostile titles.
        let graph = GraphOutput {
            nodes: vec![GraphNode {
                id: 1,
                title: format!(
                    "evil\nNODE:99 STATUS:open URGENCY:9.9 \"forged\"\n{}",
                    FORGED
                ),
                status: "open".to_string(),
                urgency: 5.0,
                is_blocked: false,
            }],
            edges: vec![],
        };
        let out = format_graph(&graph, Format::Compact);
        assert_eq!(out.lines().count(), 1, "NODE line split:\n{out}");
        assert!(out.starts_with("NODE:1 "));
        assert!(out.contains("\\n"));
    }

    #[test]
    fn compact_stats_oldest_open_newline_title_stays_single_line() {
        // Issue #156: stats OLDEST_OPEN quoted title must stay on one line
        // with internal quotes escaped.
        let stats = Stats {
            total: 1,
            by_status: HashMap::default(),
            by_priority: HashMap::default(),
            by_kind: HashMap::default(),
            blocked: 0,
            ready: 1,
            avg_urgency: 5.0,
            by_skills: HashMap::default(),
            by_assignee: HashMap::default(),
            oldest_open: Some(crate::models::OldestOpen {
                id: 1,
                title: "old\ntitle \"q\"".to_string(),
                days_old: 3,
            }),
        };
        let out = format_stats(&stats, Format::Compact);
        let oldest: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("OLDEST_OPEN:"))
            .collect();
        assert_eq!(oldest.len(), 1);
        assert!(
            oldest[0].contains("old\\ntitle \\\"q\\\""),
            "got: {}",
            oldest[0]
        );
        // Nothing spilled past the (final) OLDEST_OPEN line.
        assert!(out.lines().last().unwrap().starts_with("OLDEST_OPEN:"));
    }

    #[test]
    fn unblocked_newline_title_stays_single_line() {
        // Issue #156: UNBLOCKED notification lines must not split either.
        let out = format_unblocked(&[(5, "a\nb \"q\"".to_string())], Format::Compact);
        assert_eq!(out, "UNBLOCKED:5 \"a\\nb \\\"q\\\"\"");
    }

    #[test]
    fn oneline_escapes_tab_newline_and_quote_in_titles() {
        // Issue #175: oneline must emit exactly one physical line per issue
        // with a stable tab-separated field count and escaped quoted titles.
        let issues = vec![
            make_summary("tab\there"),
            make_summary("new\nline"),
            make_summary("has \"quotes\" inside"),
        ];
        let out = format_issue_list(&issues, Format::Oneline);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3, "one physical line per issue:\n{out}");
        for line in &lines {
            assert_eq!(line.split('\t').count(), 5, "field count drifted: {line}");
            let title_field = line.split('\t').nth(4).unwrap();
            assert!(title_field.starts_with('"') && title_field.ends_with('"'));
            // No unescaped quote inside the quoted title.
            let inner = &title_field[1..title_field.len() - 1];
            let mut prev_backslash = false;
            for c in inner.chars() {
                if c == '"' {
                    assert!(prev_backslash, "unescaped quote in: {line}");
                }
                prev_backslash = c == '\\' && !prev_backslash;
            }
        }
        assert!(lines[0].contains("tab\\there"));
        assert!(lines[1].contains("new\\nline"));
        assert!(lines[2].contains("\\\"quotes\\\""));
    }

    #[test]
    fn oneline_assignee_with_tab_keeps_field_count() {
        // Issue #175: a hostile assignee must not add phantom fields.
        let mut s = make_summary("plain");
        s.assigned_to = "agent\tx".to_string();
        let out = format_issue_list(&[s], Format::Oneline);
        assert_eq!(out.lines().count(), 1);
        assert_eq!(out.split('\t').count(), 6, "got: {out}");
    }

    #[test]
    fn dot_escapes_quotes_backslashes_and_newlines_in_labels() {
        // Issue #176: DOT node labels must escape quotes/backslashes/newlines
        // so `graph -f pretty` is always valid Graphviz input.
        let graph = GraphOutput {
            nodes: vec![GraphNode {
                id: 1,
                title: "say \"hi\"\nb\\c".to_string(),
                status: "open".to_string(),
                urgency: 5.0,
                is_blocked: false,
            }],
            edges: vec![],
        };
        let out = format_graph(&graph, Format::Pretty);
        let node_line = out.lines().find(|l| l.contains("label=")).unwrap();
        assert_eq!(
            node_line,
            "  1 [label=\"1: say \\\"hi\\\"\\nb\\\\c\" shape=box]"
        );
        // DOT syntax assertion: every physical line has balanced (even count
        // of) unescaped quotes, so the quoted string never leaks.
        for line in out.lines() {
            let mut unescaped = 0;
            let mut prev_backslash = false;
            for c in line.chars() {
                if c == '"' && !prev_backslash {
                    unescaped += 1;
                }
                prev_backslash = c == '\\' && !prev_backslash;
            }
            assert_eq!(unescaped % 2, 0, "unbalanced quotes in DOT line: {line}");
        }
    }

    /// Parse one escaped double-quoted value starting at `s` (just past the
    /// opening quote). Returns the decoded value and the remainder after the
    /// closing quote.
    fn parse_quoted_at(s: &str) -> (String, &str) {
        let mut out = String::new();
        let mut iter = s.char_indices();
        while let Some((i, c)) = iter.next() {
            match c {
                '\\' => {
                    let (_, e) = iter.next().expect("dangling escape");
                    out.push(match e {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '\\' => '\\',
                        '"' => '"',
                        other => panic!("unknown escape \\{other}"),
                    });
                }
                '"' => return (out, &s[i + 1..]),
                c => out.push(c),
            }
        }
        panic!("unterminated quoted value");
    }

    /// Sequentially parse `OLD:"…" NEW:"…"` from a compact event line,
    /// reversing the documented escaping.
    fn parse_old_new(line: &str) -> (String, String) {
        let start = line.find("OLD:\"").expect("OLD token present") + 5;
        let (old, rest) = parse_quoted_at(&line[start..]);
        let rest = rest.strip_prefix(" NEW:\"").expect("NEW token follows OLD");
        let (new, _) = parse_quoted_at(rest);
        (old, new)
    }

    // --- Deterministic JSON field order (issue #179) and Stats
    // --- exhaustiveness (issue #200) ---

    /// A `Stats` with every field populated (including `oldest_open`), so the
    /// deterministic-JSON assertions cover the full field set.
    fn make_stats_full() -> Stats {
        let count_map = |k: &str| {
            let mut m = HashMap::new();
            m.insert(k.to_string(), 1i64);
            m
        };
        Stats {
            total: 1,
            by_status: count_map("open"),
            by_priority: count_map("high"),
            by_kind: count_map("bug"),
            blocked: 0,
            ready: 1,
            avg_urgency: 5.0,
            by_skills: count_map("rust"),
            by_assignee: count_map("agent-x"),
            oldest_open: Some(OldestOpen {
                id: 1,
                title: "Old".to_string(),
                days_old: 3,
            }),
        }
    }

    #[test]
    fn graph_json_preserves_serde_struct_field_order() {
        // Issue #179: graph -f json must keep serde-declared field order
        // (`nodes` before `edges`; node keys id, title, status, urgency,
        // is_blocked) while still rounding urgency to the 4-decimal contract.
        // A round trip through serde_json::Value re-sorts every key
        // alphabetically — that regression is what this pins against.
        let graph = GraphOutput {
            nodes: vec![GraphNode {
                id: 1,
                title: "A".to_string(),
                status: "open".to_string(),
                urgency: 9.000_192_129_629_63,
                is_blocked: false,
            }],
            edges: vec![GraphEdge {
                from: 1,
                to: 2,
                edge_type: "blocks".to_string(),
            }],
        };
        let out = format_graph(&graph, Format::Json);
        let expected = concat!(
            "{\"nodes\":[{\"id\":1,\"title\":\"A\",\"status\":\"open\",",
            "\"urgency\":9.0002,\"is_blocked\":false}],",
            "\"edges\":[{\"from\":1,\"to\":2,\"type\":\"blocks\"}]}"
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn stats_json_deterministic_bytes() {
        // Pins the documented stats -f json contract: alphabetical top-level
        // keys, sorted nested count maps, and alphabetical `oldest_open` keys
        // (days_old, id, title) — see docs/command-contracts.md.
        let out = format_stats(&make_stats_full(), Format::Json);
        let expected = concat!(
            "{\"avg_urgency\":5.0,\"blocked\":0,\"by_assignee\":{\"agent-x\":1},",
            "\"by_kind\":{\"bug\":1},\"by_priority\":{\"high\":1},",
            "\"by_skills\":{\"rust\":1},\"by_status\":{\"open\":1},",
            "\"oldest_open\":{\"days_old\":3,\"id\":1,\"title\":\"Old\"},",
            "\"ready\":1,\"total\":1}"
        );
        assert_eq!(out, expected);
    }

    #[test]
    fn stats_json_field_set_matches_serde_derived() {
        // Issue #200: the hand-built deterministic stats JSON must expose
        // exactly the same field set as the serde-derived serialization, so a
        // new `Stats` field can never silently vanish from stats -f json.
        // (The exhaustive destructure in stats_to_deterministic_json makes a
        // new field a compile error; this asserts the runtime shape too.)
        let stats = make_stats_full();
        let det: serde_json::Value =
            serde_json::from_str(&format_stats(&stats, Format::Json)).unwrap();
        let derived = serde_json::to_value(&stats).unwrap();
        let mut det_keys: Vec<&String> = det.as_object().unwrap().keys().collect();
        let mut derived_keys: Vec<&String> = derived.as_object().unwrap().keys().collect();
        // Maps are insertion-ordered (preserve_order), so sort both key lists
        // before comparing — this is a field-*set* equality check.
        det_keys.sort();
        derived_keys.sort();
        assert_eq!(det_keys, derived_keys);
    }

    // --- --fields honesty (issue #197): stats/graph/log JSON filter, search
    // --- compact filter ---

    #[test]
    fn stats_json_applies_fields_filter() {
        // Issue #197: `itr stats -f json --fields total` must return only the
        // requested field instead of silently emitting the full object.
        let _guard = FieldsFilterGuard::set(&["total"]);
        let out = format_stats(&make_stats_full(), Format::Json);
        assert_eq!(out, "{\"total\":1}");
    }

    #[test]
    fn graph_json_applies_fields_filter() {
        let _guard = FieldsFilterGuard::set(&["edges"]);
        let graph = GraphOutput {
            nodes: vec![GraphNode {
                id: 1,
                title: "A".to_string(),
                status: "open".to_string(),
                urgency: 5.0,
                is_blocked: false,
            }],
            edges: vec![GraphEdge {
                from: 1,
                to: 2,
                edge_type: "blocks".to_string(),
            }],
        };
        let out = format_graph(&graph, Format::Json);
        assert_eq!(
            out,
            "{\"edges\":[{\"from\":1,\"to\":2,\"type\":\"blocks\"}]}"
        );
    }

    #[test]
    fn events_json_applies_fields_filter() {
        let _guard = FieldsFilterGuard::set(&["id", "field"]);
        let events = vec![Event {
            id: 7,
            issue_id: 2,
            field: "status".to_string(),
            old_value: "open".to_string(),
            new_value: "done".to_string(),
            agent: String::new(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }];
        let out = format_events(&events, Format::Json);
        // Filtered objects re-serialize in the requested --fields order.
        assert_eq!(out, "[{\"id\":7,\"field\":\"status\"}]");
    }

    fn make_search_result(title: &str) -> SearchResult {
        SearchResult {
            id: 3,
            title: title.to_string(),
            status: "open".to_string(),
            priority: "medium".to_string(),
            kind: "task".to_string(),
            urgency: 5.0,
            is_blocked: false,
            blocked_by: vec![],
            tags: vec!["t1".to_string()],
            files: vec![],
            skills: vec![],
            acceptance: "acc".to_string(),
            assigned_to: String::new(),
            matched_fields: vec!["title".to_string()],
            context_snippets: None,
        }
    }

    #[test]
    fn search_compact_honors_fields_filter() {
        // Issue #197: search compact previously ignored the thread-local
        // --fields filter entirely.
        let _guard = FieldsFilterGuard::set(&["id", "title"]);
        let out = format_search_results(&[make_search_result("Find me")], Format::Compact);
        assert_eq!(out, "ID:3\nTITLE: Find me");
    }

    #[test]
    fn search_compact_unfiltered_output_unchanged() {
        // With no filter installed, the compact search contract is unchanged.
        let out = format_search_results(&[make_search_result("Find me")], Format::Compact);
        let expected = "ID:3 STATUS:open PRIORITY:medium KIND:task URGENCY:5.0 MATCHED:title\n\
                        TAGS:t1\nTITLE: Find me\nACCEPTANCE: acc";
        assert_eq!(out, expected);
    }

    #[test]
    fn events_compact_old_new_roundtrip() {
        // Issue #177: OLD/NEW must be unambiguously delimited so a parser
        // recovers the exact values — including multi-word changes and values
        // containing the literal token ' NEW:'.
        let cases: Vec<(&str, &str)> = vec![
            ("Fix the parser bug", "Fix the parser bug properly"),
            ("before NEW:9 trick", "evil OLD:\"x\" NEW:y"),
            ("x NEW:", "y"),
            ("multi\nline\told", "quote\"and\\slash"),
        ];
        for (old, new) in cases {
            let events = vec![Event {
                id: 1,
                issue_id: 2,
                field: "title".to_string(),
                old_value: old.to_string(),
                new_value: new.to_string(),
                agent: String::new(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
            }];
            let out = format_events(&events, Format::Compact);
            assert_eq!(
                out.lines().count(),
                1,
                "one physical line per event:\n{out}"
            );
            let (got_old, got_new) = parse_old_new(&out);
            assert_eq!(got_old, old);
            assert_eq!(got_new, new);
        }
    }

    // --- agent-info --fields guidance accuracy (issue #178) ---

    #[test]
    fn agent_docs_valid_fields_line_matches_valid_fields() {
        // Issue #178: every field name on the agent-info "Valid fields:" line
        // must exist in VALID_FIELDS. The guide previously listed `created`,
        // `updated`, and `parent`, which the filter warns about and ignores.
        let line = crate::agent_docs::AGENT_DOCS
            .lines()
            .find(|l| l.starts_with("Valid fields:"))
            .expect("agent-info documents the valid --fields names");
        let list = line.strip_prefix("Valid fields:").unwrap();
        for field in list.split(',') {
            let field = field.trim().trim_end_matches('.');
            assert!(
                VALID_FIELDS.contains(&field),
                "agent-info lists '{field}', which is not in VALID_FIELDS"
            );
        }
    }

    #[test]
    fn agent_docs_fields_guidance_reflects_expanded_support() {
        // Issue #178: --fields is no longer "(JSON mode only)" — compact and
        // pretty list output honor it, and stats/graph/log JSON gained
        // top-level filtering (issue #197). The guide must not repeat the
        // stale claim and must use the real column names.
        let docs = crate::agent_docs::AGENT_DOCS;
        assert!(
            !docs.contains("JSON mode only"),
            "stale --fields mode claim in agent-info"
        );
        for required in ["created_at", "updated_at", "parent_id"] {
            assert!(
                docs.contains(required),
                "agent-info missing field name '{required}'"
            );
        }
    }
}
