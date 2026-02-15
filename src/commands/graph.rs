use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{GraphEdge, GraphNode, GraphOutput};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;

pub fn run(conn: &Connection, all: bool, fmt: Format) -> Result<(), ItrError> {
    let issues = if all {
        db::all_issues(conn)?
    } else {
        db::list_issues(conn, &[], &[], &[], &[], false, true, None, false)?
    };

    let config = UrgencyConfig::load(conn);
    let deps = db::all_dependencies(conn)?;

    let issue_ids: std::collections::HashSet<i64> = issues.iter().map(|i| i.id).collect();

    let nodes: Vec<GraphNode> = issues
        .iter()
        .map(|i| {
            let urg = urgency::compute_urgency(i, &config, conn);
            let is_blocked = db::is_blocked(conn, i.id).unwrap_or(false);
            GraphNode {
                id: i.id,
                title: i.title.clone(),
                status: i.status.clone(),
                urgency: urg,
                is_blocked,
            }
        })
        .collect();

    let edges: Vec<GraphEdge> = deps
        .iter()
        .filter(|(blocker, blocked)| issue_ids.contains(blocker) && issue_ids.contains(blocked))
        .map(|(blocker, blocked)| GraphEdge {
            from: *blocker,
            to: *blocked,
            edge_type: "blocks".to_string(),
        })
        .collect();

    let graph = GraphOutput { nodes, edges };

    // Support DOT format via pretty
    let output = if fmt == Format::Pretty {
        format::format_graph(&graph, Format::Pretty) // outputs DOT
    } else {
        format::format_graph(&graph, fmt)
    };

    println!("{}", output);
    Ok(())
}
