use crate::commands::build_issue_summary;
use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::urgency::UrgencyConfig;
use crate::util;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Serialize)]
struct Summary {
    total: usize,
    done: usize,
    open: usize,
    in_progress: usize,
    blocked: usize,
    ready: usize,
    completion_pct: f64,
    oldest_open: Option<OldestEntry>,
    in_progress_issues: Vec<SummaryIssue>,
    ready_issues: Vec<SummaryIssue>,
    recent_events: Vec<RecentEvent>,
}

#[derive(Serialize)]
struct OldestEntry {
    id: i64,
    title: String,
    days_old: i64,
}

#[derive(Serialize)]
struct SummaryIssue {
    id: i64,
    title: String,
    priority: String,
    kind: String,
    urgency: f64,
    assigned_to: String,
}

#[derive(Serialize)]
struct RecentEvent {
    issue_id: i64,
    field: String,
    new_value: String,
    created_at: String,
}

pub fn run(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    let all_issues = db::all_issues(conn)?;
    let config = UrgencyConfig::load(conn);

    let total = all_issues.len();
    let mut done = 0usize;
    let mut open = 0usize;
    let mut in_progress = 0usize;
    let mut blocked = 0usize;
    let mut ready = 0usize;
    let mut oldest_open: Option<OldestEntry> = None;
    let mut wip_issues = Vec::new();
    let mut ready_issues = Vec::new();

    for issue in &all_issues {
        match issue.status.as_str() {
            "done" | "wontfix" => done += 1,
            "in-progress" => {
                in_progress += 1;
                let s = build_issue_summary(conn, issue, &config);
                wip_issues.push(SummaryIssue {
                    id: issue.id,
                    title: issue.title.clone(),
                    priority: issue.priority.clone(),
                    kind: issue.kind.clone(),
                    urgency: s.urgency,
                    assigned_to: issue.assigned_to.clone(),
                });
            }
            _ => {
                open += 1;
                let is_blocked = db::is_blocked(conn, issue.id).unwrap_or(false);
                if is_blocked {
                    blocked += 1;
                } else {
                    ready += 1;
                    let s = build_issue_summary(conn, issue, &config);
                    ready_issues.push(SummaryIssue {
                        id: issue.id,
                        title: issue.title.clone(),
                        priority: issue.priority.clone(),
                        kind: issue.kind.clone(),
                        urgency: s.urgency,
                        assigned_to: issue.assigned_to.clone(),
                    });
                }

                let days = util::days_since(&issue.created_at) as i64;
                match &oldest_open {
                    None => {
                        oldest_open = Some(OldestEntry {
                            id: issue.id,
                            title: issue.title.clone(),
                            days_old: days,
                        });
                    }
                    Some(current) if days > current.days_old => {
                        oldest_open = Some(OldestEntry {
                            id: issue.id,
                            title: issue.title.clone(),
                            days_old: days,
                        });
                    }
                    _ => {}
                }
            }
        }
    }

    // Sort ready by urgency descending
    ready_issues.sort_by(|a, b| b.urgency.partial_cmp(&a.urgency).unwrap_or(std::cmp::Ordering::Equal));
    ready_issues.truncate(5);

    // Get recent events (last 5)
    let events = db::get_recent_events(conn, 5, None)?;
    let recent_events: Vec<RecentEvent> = events
        .into_iter()
        .map(|e| RecentEvent {
            issue_id: e.issue_id,
            field: e.field,
            new_value: e.new_value,
            created_at: e.created_at,
        })
        .collect();

    let completion_pct = if total > 0 {
        (done as f64 / total as f64) * 100.0
    } else {
        0.0
    };

    let summary = Summary {
        total,
        done,
        open,
        in_progress,
        blocked,
        ready,
        completion_pct,
        oldest_open,
        in_progress_issues: wip_issues,
        ready_issues,
        recent_events,
    };

    match fmt {
        Format::Json => println!("{}", serde_json::to_string(&summary).unwrap_or_default()),
        _ => print_compact(&summary),
    }

    Ok(())
}

fn print_compact(s: &Summary) {
    println!(
        "PROJECT: {} issues, {} done ({:.0}%), {} in-progress, {} open ({} ready, {} blocked)",
        s.total, s.done, s.completion_pct, s.in_progress, s.open, s.ready, s.blocked
    );

    if !s.in_progress_issues.is_empty() {
        println!("IN-PROGRESS:");
        for i in &s.in_progress_issues {
            let assignee = if i.assigned_to.is_empty() {
                String::new()
            } else {
                format!(" [{}]", i.assigned_to)
            };
            println!("  #{} {} {} \"{}\"{}", i.id, i.priority, i.kind, i.title, assignee);
        }
    }

    if !s.ready_issues.is_empty() {
        println!("READY:");
        for i in &s.ready_issues {
            println!("  #{} {} {} \"{}\" (urgency: {:.1})", i.id, i.priority, i.kind, i.title, i.urgency);
        }
    }

    if let Some(ref o) = s.oldest_open {
        println!("OLDEST-OPEN: #{} \"{}\" ({} days)", o.id, o.title, o.days_old);
    }

    if !s.recent_events.is_empty() {
        println!("RECENT:");
        for e in &s.recent_events {
            println!("  #{} {} -> {} ({})", e.issue_id, e.field, e.new_value, e.created_at);
        }
    }
}
