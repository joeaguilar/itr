use crate::error::ItrError;
use crate::models::{
    Event, GraphOutput, IssueDetail, IssueSummary, Relation, SearchResult, Stats, UnblockedIssue,
};
use std::cell::RefCell;

thread_local! {
    static FIELDS_FILTER: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

pub fn set_fields_filter(fields: Vec<String>) {
    FIELDS_FILTER.with(|f| {
        *f.borrow_mut() = Some(fields);
    });
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

/// Print JSON output, applying field filtering if --fields was set
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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    Compact,
    Json,
    Pretty,
}

impl Format {
    pub fn from_str(s: &str) -> Option<Format> {
        match s {
            "compact" => Some(Format::Compact),
            "json" => Some(Format::Json),
            "pretty" => Some(Format::Pretty),
            _ => None,
        }
    }

    pub fn is_json(&self) -> bool {
        matches!(self, Format::Json)
    }
}

// --- Issue Detail ---

pub fn format_issue_detail(detail: &IssueDetail, fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(detail).unwrap_or_default()),
        Format::Compact => format_issue_detail_compact(detail),
        Format::Pretty => format_issue_detail_pretty(detail),
    }
}

fn format_issue_detail_compact(d: &IssueDetail) -> String {
    let mut lines = Vec::new();

    let mut first = format!(
        "ID:{} STATUS:{} PRIORITY:{} KIND:{} URGENCY:{:.1}",
        d.issue.id, d.issue.status, d.issue.priority, d.issue.kind, d.urgency
    );
    if !d.blocked_by.is_empty() {
        first.push_str(&format!(
            " BLOCKED_BY:{}",
            d.blocked_by
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if !d.blocks.is_empty() {
        first.push_str(&format!(
            " BLOCKS:{}",
            d.blocks
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    lines.push(first);

    if !d.issue.tags.is_empty() {
        lines.push(format!("TAGS:{}", d.issue.tags.join(",")));
    }
    if !d.issue.files.is_empty() {
        lines.push(format!("FILES:{}", d.issue.files.join(",")));
    }
    if !d.issue.skills.is_empty() {
        lines.push(format!("SKILLS:{}", d.issue.skills.join(",")));
    }
    if !d.issue.assigned_to.is_empty() {
        lines.push(format!("ASSIGNED:{}", d.issue.assigned_to));
    }
    lines.push(format!("TITLE: {}", d.issue.title));
    if !d.issue.context.is_empty() {
        lines.push(format!("CONTEXT: {}", d.issue.context));
    }
    if !d.issue.acceptance.is_empty() {
        lines.push(format!("ACCEPTANCE: {}", d.issue.acceptance));
    }
    if let Some(pid) = d.issue.parent_id {
        lines.push(format!("PARENT: {}", pid));
    }
    if !d.issue.close_reason.is_empty() {
        lines.push(format!("CLOSE_REASON: {}", d.issue.close_reason));
    }
    lines.push(format!("CREATED: {}", d.issue.created_at));
    lines.push(format!("UPDATED: {}", d.issue.updated_at));

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

    if !d.relations.is_empty() {
        lines.push("--- RELATIONS ---".to_string());
        for rel in &d.relations {
            lines.push(format_relation_compact(rel, d.issue.id));
        }
    }

    if !d.notes.is_empty() {
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
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !d.blocks.is_empty() {
        lines.push(format!(
            "  Blocks: {}",
            d.blocks
                .iter()
                .map(|i| i.to_string())
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

pub fn format_issue_list(issues: &[IssueSummary], fmt: Format) -> String {
    match fmt {
        Format::Json => apply_fields_filter(&serde_json::to_string(issues).unwrap_or_default()),
        Format::Compact => format_issue_list_compact(issues),
        Format::Pretty => format_issue_list_pretty(issues),
    }
}

fn format_issue_list_compact(issues: &[IssueSummary]) -> String {
    issues
        .iter()
        .map(|i| {
            let mut first = format!(
                "ID:{} STATUS:{} PRIORITY:{} KIND:{} URGENCY:{:.1}",
                i.id, i.status, i.priority, i.kind, i.urgency
            );
            if !i.blocked_by.is_empty() {
                first.push_str(&format!(
                    " BLOCKED_BY:{}",
                    i.blocked_by
                        .iter()
                        .map(|x| x.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ));
            }
            let mut lines = vec![first];
            if !i.tags.is_empty() {
                lines.push(format!("TAGS:{}", i.tags.join(",")));
            }
            if !i.files.is_empty() {
                lines.push(format!("FILES:{}", i.files.join(",")));
            }
            if !i.skills.is_empty() {
                lines.push(format!("SKILLS:{}", i.skills.join(",")));
            }
            if !i.assigned_to.is_empty() {
                lines.push(format!("ASSIGNED:{}", i.assigned_to));
            }
            lines.push(format!("TITLE: {}", i.title));
            if !i.acceptance.is_empty() {
                lines.push(format!("ACCEPTANCE: {}", i.acceptance));
            }
            lines.join("\n")
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_issue_list_pretty(issues: &[IssueSummary]) -> String {
    if issues.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push(format!(
        " {:>3} | {:>5} | {:11} | {:8} | {:7} | {:40} | Blocked",
        "#", "Urg", "Status", "Pri", "Kind", "Title"
    ));
    lines.push(
        "-----|-------|-------------|----------|---------|------------------------------------------|--------"
            .to_string(),
    );
    for i in issues {
        let title = truncate_with_ellipsis(&i.title, 40);
        let blocked = if i.blocked_by.is_empty() {
            String::new()
        } else {
            i.blocked_by
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        lines.push(format!(
            " {:>3} | {:>5.1} | {:11} | {:8} | {:7} | {:40} | {}",
            i.id, i.urgency, i.status, i.priority, i.kind, title, blocked
        ));
    }
    lines.join("\n")
}

// --- Stats ---

pub fn format_stats(stats: &Stats, fmt: Format) -> String {
    match fmt {
        Format::Json => serde_json::to_string(stats).unwrap_or_default(),
        Format::Compact => format_stats_compact(stats),
        Format::Pretty => format_stats_compact(stats), // same for now
    }
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

pub fn format_graph(graph: &GraphOutput, fmt: Format) -> String {
    match fmt {
        Format::Json => serde_json::to_string(graph).unwrap_or_default(),
        Format::Compact => format_graph_compact(graph),
        Format::Pretty => format_graph_dot(graph),
    }
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
        Format::Pretty => format_search_pretty(results),
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
                        .map(|x| x.to_string())
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
        Format::Pretty => format_events_pretty(events),
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
];

pub fn parse_fields(fields_str: &str) -> Vec<String> {
    fields_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

pub fn validate_fields(fields: &[String]) -> Result<(), ItrError> {
    for f in fields {
        if !VALID_FIELDS.contains(&f.as_str()) {
            return Err(ItrError::InvalidValue {
                field: "fields".to_string(),
                value: f.clone(),
                valid: VALID_FIELDS.join(", "),
            });
        }
    }
    Ok(())
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
