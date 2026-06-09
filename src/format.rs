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
/// ```ignore
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
/// ```ignore
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
/// ```ignore
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
    /// Parse a `--format` argument value. Returns `None` for unknown inputs so
    /// the CLI layer can produce a helpful error.
    ///
    /// # Examples
    ///
    /// ```ignore
    /// use itr::format::Format;
    /// assert_eq!(Format::from_str("compact"), Some(Format::Compact));
    /// assert_eq!(Format::from_str("oneline"), Some(Format::Oneline));
    /// assert_eq!(Format::from_str(""), None);
    /// ```
    pub fn from_str(s: &str) -> Option<Format> {
        match s {
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
    /// ```ignore
    /// use itr::format::Format;
    /// assert!(Format::Json.is_json());
    /// assert!(!Format::Compact.is_json());
    /// ```
    pub fn is_json(self) -> bool {
        matches!(self, Format::Json)
    }
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
/// ```ignore
/// use itr::format::{format_issue_detail, Format};
/// # let detail: itr::models::IssueDetail = unimplemented!();
/// let json = format_issue_detail(&detail, Format::Json);
/// assert!(json.starts_with('{'));
/// ```
pub fn format_issue_detail(detail: &IssueDetail, fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(detail).unwrap_or_default()),
        Format::Compact | Format::Oneline => format_issue_detail_compact(detail),
        Format::Pretty => format_issue_detail_pretty(detail),
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
        lines.push(format!("TAGS:{}", d.issue.tags.join(",")));
    }
    if on("files") && !d.issue.files.is_empty() {
        lines.push(format!("FILES:{}", d.issue.files.join(",")));
    }
    if on("skills") && !d.issue.skills.is_empty() {
        lines.push(format!("SKILLS:{}", d.issue.skills.join(",")));
    }
    if on("assigned_to") && !d.issue.assigned_to.is_empty() {
        lines.push(format!("ASSIGNED:{}", d.issue.assigned_to));
    }
    if on("title") {
        lines.push(format!("TITLE: {}", d.issue.title));
    }
    if on("context") && !d.issue.context.is_empty() {
        lines.push(format!("CONTEXT: {}", d.issue.context));
    }
    if on("acceptance") && !d.issue.acceptance.is_empty() {
        lines.push(format!("ACCEPTANCE: {}", d.issue.acceptance));
    }
    if on("parent_id") {
        if let Some(pid) = d.issue.parent_id {
            lines.push(format!("PARENT: {}", pid));
        }
    }
    if on("close_reason") && !d.issue.close_reason.is_empty() {
        lines.push(format!("CLOSE_REASON: {}", d.issue.close_reason));
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
                format!(" ({})", note.agent)
            };
            lines.push(format!(
                "[{}]{} {}",
                note.created_at, agent_str, note.content
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
/// ```ignore
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

fn format_issue_list_oneline(issues: &[IssueSummary]) -> String {
    issues
        .iter()
        .map(|i| {
            let assignee = if i.assigned_to.is_empty() {
                String::new()
            } else {
                format!("\t{}", i.assigned_to)
            };
            format!(
                "{}\t{}\t{}\t{}\t\"{}\"{}",
                i.id, i.status, i.priority, i.kind, i.title, assignee
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_issue_list_compact(issues: &[IssueSummary]) -> String {
    let fields = get_fields_filter();
    let on = |name: &str| field_enabled(fields.as_ref(), name);
    issues
        .iter()
        .map(|i| {
            let mut first_parts = Vec::new();
            if on("id") {
                first_parts.push(format!("ID:{}", i.id));
            }
            if on("status") {
                first_parts.push(format!("STATUS:{}", i.status));
            }
            if on("priority") {
                first_parts.push(format!("PRIORITY:{}", i.priority));
            }
            if on("kind") {
                first_parts.push(format!("KIND:{}", i.kind));
            }
            if on("urgency") {
                first_parts.push(format!("URGENCY:{:.1}", i.urgency));
            }
            if on("blocked_by") && !i.blocked_by.is_empty() {
                first_parts.push(format!(
                    "BLOCKED_BY:{}",
                    i.blocked_by
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            let mut lines = vec![first_parts.join(" ")];
            if on("tags") && !i.tags.is_empty() {
                lines.push(format!("TAGS:{}", i.tags.join(",")));
            }
            if on("files") && !i.files.is_empty() {
                lines.push(format!("FILES:{}", i.files.join(",")));
            }
            if on("skills") && !i.skills.is_empty() {
                lines.push(format!("SKILLS:{}", i.skills.join(",")));
            }
            if on("assigned_to") && !i.assigned_to.is_empty() {
                lines.push(format!("ASSIGNED:{}", i.assigned_to));
            }
            if on("title") {
                lines.push(format!("TITLE: {}", i.title));
            }
            if on("acceptance") && !i.acceptance.is_empty() {
                lines.push(format!("ACCEPTANCE: {}", i.acceptance));
            }
            lines.retain(|l| !l.is_empty());
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_issue_list_pretty(issues: &[IssueSummary]) -> String {
    if issues.is_empty() {
        return String::new();
    }
    let fields = get_fields_filter();
    let on = |name: &str| field_enabled(fields.as_ref(), name);

    // (field_name, header, width, right_align)
    // width=0 → last column, no padding
    let all_cols: &[(&str, &str, usize, bool)] = &[
        ("id", "#", 3, true),
        ("urgency", "Urg", 5, true),
        ("status", "Status", 11, false),
        ("priority", "Pri", 8, false),
        ("kind", "Kind", 7, false),
        ("assigned_to", "Assignee", 10, false),
        ("title", "Title", 40, false),
        ("blocked_by", "Blocked", 0, false),
    ];
    let cols: Vec<&(&str, &str, usize, bool)> =
        all_cols.iter().filter(|(f, _, _, _)| on(f)).collect();
    if cols.is_empty() {
        return String::new();
    }

    let header_parts: Vec<String> = cols
        .iter()
        .map(|(_, h, w, right)| {
            if *w == 0 {
                h.to_string()
            } else if *right {
                format!("{:>width$}", h, width = w)
            } else {
                format!("{:<width$}", h, width = w)
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
            .map(|(f, _, w, right)| {
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
                    _ => String::new(),
                };
                if *w == 0 {
                    val
                } else if *right {
                    format!("{:>width$}", val, width = w)
                } else {
                    format!("{:<width$}", val, width = w)
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
        Format::Json => stats_to_deterministic_json(stats),
        Format::Compact => format_stats_compact(stats),
        Format::Pretty | Format::Oneline => format_stats_compact(stats), // same for now
    }
}

/// Serialize [`Stats`] to JSON with a deterministic contract.
///
/// The `Stats` struct stores its count buckets in `HashMap`s, whose iteration
/// order is randomized per process — serializing it directly produces
/// byte-different output for semantically identical data, which makes byte-level
/// snapshot tests flap (issue #139). This builds the JSON with a fixed
/// top-level field order and **sorted** nested count-map keys (via `BTreeMap`),
/// preserving the exact same JSON shape and values while removing the
/// nondeterminism. See `docs/command-contracts.md` for the documented contract.
fn stats_to_deterministic_json(stats: &Stats) -> String {
    use serde_json::{Map, Value};
    use std::collections::BTreeMap;

    // Nested count maps: sort keys for a stable, deterministic order.
    let ordered_map = |m: &std::collections::HashMap<String, i64>| -> Value {
        let sorted: BTreeMap<&String, &i64> = m.iter().collect();
        let mut obj = Map::new();
        for (k, v) in sorted {
            obj.insert(k.clone(), Value::from(*v));
        }
        Value::Object(obj)
    };

    // Top-level object. `serde_json::Map` (without the `preserve_order`
    // feature) is backed by a `BTreeMap`, so the top-level keys serialize in a
    // stable alphabetical order regardless of insertion order. Combined with
    // the sorted nested maps above, the whole `Stats` object is deterministic.
    let mut obj = Map::new();
    obj.insert("total".to_string(), Value::from(stats.total));
    obj.insert("by_status".to_string(), ordered_map(&stats.by_status));
    obj.insert("by_priority".to_string(), ordered_map(&stats.by_priority));
    obj.insert("by_kind".to_string(), ordered_map(&stats.by_kind));
    obj.insert("blocked".to_string(), Value::from(stats.blocked));
    obj.insert("ready".to_string(), Value::from(stats.ready));
    obj.insert(
        "avg_urgency".to_string(),
        round_urgency_value(stats.avg_urgency),
    );
    obj.insert("by_skills".to_string(), ordered_map(&stats.by_skills));
    obj.insert("by_assignee".to_string(), ordered_map(&stats.by_assignee));
    obj.insert(
        "oldest_open".to_string(),
        serde_json::to_value(&stats.oldest_open).unwrap_or(Value::Null),
    );

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
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();
        lines.push(format!("BY_SKILLS: {}", parts.join(" ")));
    }
    if !stats.by_assignee.is_empty() {
        let mut pairs: Vec<(&String, &i64)> = stats.by_assignee.iter().collect();
        pairs.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let parts: Vec<String> = pairs.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        lines.push(format!("BY_ASSIGNEE: {}", parts.join(" ")));
    }
    if let Some(ref oldest) = stats.oldest_open {
        lines.push(format!(
            "OLDEST_OPEN: ID:{} DAYS:{} \"{}\"",
            oldest.id, oldest.days_old, oldest.title
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
/// ```ignore
/// use itr::format::{format_graph, Format};
/// use itr::models::GraphOutput;
/// let empty = GraphOutput { nodes: vec![], edges: vec![] };
/// assert!(format_graph(&empty, Format::Pretty).contains("digraph itr"));
/// ```
pub fn format_graph(graph: &GraphOutput, fmt: Format) -> String {
    match fmt {
        Format::Json => graph_to_deterministic_json(graph),
        Format::Compact => format_graph_compact(graph),
        Format::Pretty | Format::Oneline => format_graph_dot(graph),
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
fn graph_to_deterministic_json(graph: &GraphOutput) -> String {
    use serde_json::Value;

    let mut value = serde_json::to_value(graph).unwrap_or(Value::Null);
    if let Some(nodes) = value.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes.iter_mut() {
            if let Some(urg) = node.get("urgency").and_then(Value::as_f64) {
                node["urgency"] = round_urgency_value(urg);
            }
        }
    }
    value.to_string()
}

/// Number of decimal places urgency is rounded to in JSON output. Keeps
/// parseable formats byte-stable without affecting urgency ranking.
const URGENCY_JSON_DECIMALS: i32 = 4;

/// Round an urgency `f64` to the JSON precision contract and return it as a
/// JSON number `Value`. Integral results (e.g. `9.0`) serialize as `9.0`
/// because the source value is always a float; callers that need it embedded in
/// an object should insert the returned `Value` directly.
fn round_urgency_value(urgency: f64) -> serde_json::Value {
    let factor = 10f64.powi(URGENCY_JSON_DECIMALS);
    let rounded = (urgency * factor).round() / factor;
    // serde_json::Number::from_f64 is None only for NaN/Inf; fall back to 0.0.
    serde_json::Number::from_f64(rounded)
        .map_or_else(|| serde_json::Value::from(0.0), serde_json::Value::Number)
}

fn format_graph_compact(graph: &GraphOutput) -> String {
    let mut lines = Vec::new();
    for node in &graph.nodes {
        let blocked = if node.is_blocked { " [BLOCKED]" } else { "" };
        lines.push(format!(
            "NODE:{} STATUS:{} URGENCY:{:.1}{} \"{}\"",
            node.id, node.status, node.urgency, blocked, node.title
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
        let title_short = truncate_with_ellipsis(&node.title, 30);
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

// --- Title truncation helper ---

/// Truncate a string to fit within `max_len` bytes, appending "..." if truncated.
/// Always slices on a valid UTF-8 char boundary.
fn truncate_with_ellipsis(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let suffix_len = 3; // "..."
        let mut end = max_len - suffix_len;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

// --- Search Results ---

pub fn format_search_results(results: &[SearchResult], fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(results).unwrap_or_default()),
        Format::Compact => format_search_compact(results),
        Format::Pretty | Format::Oneline => format_search_pretty(results),
    }
}

fn format_search_compact(results: &[SearchResult]) -> String {
    results
        .iter()
        .map(|r| {
            let mut first = format!(
                "ID:{} STATUS:{} PRIORITY:{} KIND:{} URGENCY:{:.1} MATCHED:{}",
                r.id,
                r.status,
                r.priority,
                r.kind,
                r.urgency,
                r.matched_fields.join(",")
            );
            if !r.blocked_by.is_empty() {
                first.push_str(&format!(
                    " BLOCKED_BY:{}",
                    r.blocked_by
                        .iter()
                        .map(std::string::ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            let mut lines = vec![first];
            if !r.tags.is_empty() {
                lines.push(format!("TAGS:{}", r.tags.join(",")));
            }
            if !r.files.is_empty() {
                lines.push(format!("FILES:{}", r.files.join(",")));
            }
            if !r.skills.is_empty() {
                lines.push(format!("SKILLS:{}", r.skills.join(",")));
            }
            if !r.assigned_to.is_empty() {
                lines.push(format!("ASSIGNED:{}", r.assigned_to));
            }
            lines.push(format!("TITLE: {}", r.title));
            if !r.acceptance.is_empty() {
                lines.push(format!("ACCEPTANCE: {}", r.acceptance));
            }
            if let Some(ref snippets) = r.context_snippets {
                for (field, snippet) in snippets {
                    lines.push(format!("SNIPPET[{}]: {}", field, snippet));
                }
            }
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
        " {:>3} | {:>5} | {:11} | {:8} | {:7} | {:40} | Matched",
        "#", "Urg", "Status", "Pri", "Kind", "Title"
    ));
    lines.push(
        "-----|-------|-------------|----------|---------|------------------------------------------|--------"
            .to_string(),
    );
    for r in results {
        let title = truncate_with_ellipsis(&r.title, 40);
        let matched = r.matched_fields.join(",");
        lines.push(format!(
            " {:>3} | {:>5.1} | {:11} | {:8} | {:7} | {:40} | {}",
            r.id, r.urgency, r.status, r.priority, r.kind, title, matched
        ));
    }
    lines.join("\n")
}

// --- Events (Audit Log) ---

pub fn format_events(events: &[Event], fmt: Format) -> String {
    match fmt {
        Format::Json => serde_json::to_string(events).unwrap_or_default(),
        Format::Compact => format_events_compact(events),
        Format::Pretty | Format::Oneline => format_events_pretty(events),
    }
}

fn format_events_compact(events: &[Event]) -> String {
    events
        .iter()
        .map(|e| {
            let agent_str = if e.agent.is_empty() {
                String::new()
            } else {
                format!(" AGENT:{}", e.agent)
            };
            format!(
                "EVENT:{} ISSUE:{} FIELD:{} OLD:{} NEW:{}{} ({})",
                e.id, e.issue_id, e.field, e.old_value, e.new_value, agent_str, e.created_at
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
        " {:>4} | {:>5} | {:15} | {:20} | {:20} | {:15} | {}",
        "ID", "Issue", "Field", "Old", "New", "Agent", "Time"
    ));
    lines.push(
        "------|-------|-----------------|----------------------|----------------------|-----------------|--------------------"
            .to_string(),
    );
    for e in events {
        let old = truncate_with_ellipsis(&e.old_value, 20);
        let new = truncate_with_ellipsis(&e.new_value, 20);
        let agent = truncate_with_ellipsis(&e.agent, 15);
        lines.push(format!(
            " {:>4} | {:>5} | {:15} | {:20} | {:20} | {:15} | {}",
            e.id, e.issue_id, e.field, old, new, agent, e.created_at
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
/// ```ignore
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
/// ```ignore
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
    if let serde_json::Value::Object(map) = value {
        let filtered: serde_json::Map<String, serde_json::Value> = map
            .into_iter()
            .filter(|(k, _)| fields.iter().any(|f| f == k))
            .collect();
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
            .map(|(id, title)| format!("UNBLOCKED:{} \"{}\"", id, title))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

// --- Batch Results ---

pub fn format_batch_result(result: &BatchResult, fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(result).unwrap_or_default()),
        Format::Compact | Format::Pretty | Format::Oneline => format_batch_result_compact(result),
    }
}

fn format_batch_result_compact(result: &BatchResult) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{}: {} items ({} ok, {} error, {} review)",
        result.action.to_uppercase(),
        result.summary.total,
        result.summary.ok,
        result.summary.error,
        result.summary.review,
    ));
    for item in &result.results {
        match item.outcome.as_str() {
            "ok" => {
                lines.push(format!("  OK:{}", item.id));
                for ub in &item.unblocked {
                    lines.push(format!("  UNBLOCKED:{} \"{}\"", ub.id, ub.title));
                }
                for note in &item.notes {
                    lines.push(format!("  NOTE:{} \"{}\"", item.id, note));
                }
            }
            "error" => {
                let msg = item.error.as_deref().unwrap_or("unknown error");
                lines.push(format!("  ERROR:{} \"{}\"", item.id, msg));
            }
            "review" => {
                lines.push(format!("  REVIEW:{}", item.id));
                for note in &item.notes {
                    lines.push(format!("  NOTE:{} \"{}\"", item.id, note));
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
    use crate::models::{GraphNode, GraphOutput, IssueSummary};

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
        // em dash '—' is 3 bytes (U+2014). Place it so byte 37 lands inside it.
        // 35 ASCII chars + '—' (bytes 35..38) + more text
        let s = format!("{}—this continues past the limit!!", "a".repeat(35));
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 40);
    }

    #[test]
    fn truncate_multibyte_emoji_at_boundary() {
        // '🚀' is 4 bytes. Place at the cut point.
        let s = format!("{}🚀 and more stuff here!!", "a".repeat(35));
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 40);
    }

    #[test]
    fn truncate_all_multibyte() {
        // String of only 3-byte chars (em dashes): 20 chars = 60 bytes
        let s = "—".repeat(20);
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 40);
        // Validates it's valid UTF-8 (would panic if not)
        let _ = result.chars().count();
    }

    #[test]
    fn truncate_two_byte_chars_at_boundary() {
        // 'é' is 2 bytes (U+00E9)
        let s = format!("{}é more text after here!!", "a".repeat(36));
        let result = truncate_with_ellipsis(&s, 40);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 40);
    }

    #[test]
    fn truncate_graph_dot_limit() {
        // Graph DOT uses limit=30. em dash at byte 27 boundary.
        let s = format!("{}—continues on and on", "a".repeat(27));
        let result = truncate_with_ellipsis(&s, 30);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 30);
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
}
