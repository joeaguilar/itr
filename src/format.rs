use crate::models::{IssueDetail, IssueSummary, Stats, GraphOutput, UnblockedIssue};

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
        Format::Json => serde_json::to_string(detail).unwrap_or_default(),
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
            d.blocked_by.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        ));
    }
    if !d.blocks.is_empty() {
        first.push_str(&format!(
            " BLOCKS:{}",
            d.blocks.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        ));
    }
    lines.push(first);

    if !d.issue.tags.is_empty() {
        lines.push(format!("TAGS:{}", d.issue.tags.join(",")));
    }
    if !d.issue.files.is_empty() {
        lines.push(format!("FILES:{}", d.issue.files.join(",")));
    }
    lines.push(format!("TITLE: {}", d.issue.title));
    if !d.issue.context.is_empty() {
        lines.push(format!("CONTEXT: {}", d.issue.context));
    }
    if !d.issue.acceptance.is_empty() {
        lines.push(format!("ACCEPTANCE: {}", d.issue.acceptance));
    }
    if d.issue.parent_id.is_some() {
        lines.push(format!("PARENT: {}", d.issue.parent_id.unwrap()));
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

    if !d.notes.is_empty() {
        lines.push("--- NOTES ---".to_string());
        for note in &d.notes {
            let agent_str = if note.agent.is_empty() {
                String::new()
            } else {
                format!(" ({})", note.agent)
            };
            lines.push(format!("[{}]{} {}", note.created_at, agent_str, note.content));
        }
    }

    lines.join("\n")
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
    if !d.issue.context.is_empty() {
        lines.push(format!("  Context: {}", d.issue.context));
    }
    if !d.issue.acceptance.is_empty() {
        lines.push(format!("  Acceptance: {}", d.issue.acceptance));
    }
    if !d.blocked_by.is_empty() {
        lines.push(format!(
            "  Blocked by: {}",
            d.blocked_by.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
        ));
    }
    if !d.blocks.is_empty() {
        lines.push(format!(
            "  Blocks: {}",
            d.blocks.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(", ")
        ));
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
        Format::Json => serde_json::to_string(issues).unwrap_or_default(),
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
                    i.blocked_by.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(",")
                ));
            }
            let mut lines = vec![first];
            if !i.tags.is_empty() {
                lines.push(format!("TAGS:{}", i.tags.join(",")));
            }
            if !i.files.is_empty() {
                lines.push(format!("FILES:{}", i.files.join(",")));
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
    lines.push(format!(
        "-----|-------|-------------|----------|---------|------------------------------------------|--------"
    ));
    for i in issues {
        let title = if i.title.len() > 40 {
            format!("{}...", &i.title[..37])
        } else {
            i.title.clone()
        };
        let blocked = if i.blocked_by.is_empty() {
            String::new()
        } else {
            i.blocked_by.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")
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
        lines.push(format!("EDGE: {} -> {} ({})", edge.from, edge.to, edge.edge_type));
    }
    lines.join("\n")
}

fn format_graph_dot(graph: &GraphOutput) -> String {
    let mut lines = Vec::new();
    lines.push("digraph itr {".to_string());
    lines.push("  rankdir=LR;".to_string());
    for node in &graph.nodes {
        let title_short = if node.title.len() > 30 {
            format!("{}...", &node.title[..27])
        } else {
            node.title.clone()
        };
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
