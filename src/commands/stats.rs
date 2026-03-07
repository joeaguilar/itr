use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::models::{OldestOpen, Stats};
use crate::urgency::{self, UrgencyConfig};
use crate::util;
use rusqlite::Connection;
use std::collections::HashMap;

pub fn run(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    let all_issues = db::all_issues(conn)?;
    let config = UrgencyConfig::load(conn);

    let total = all_issues.len() as i64;

    let mut by_status: HashMap<String, i64> = HashMap::new();
    let mut by_priority: HashMap<String, i64> = HashMap::new();
    let mut by_kind: HashMap<String, i64> = HashMap::new();

    // Initialize all known values to 0
    for s in &["open", "in-progress", "done", "wontfix"] {
        by_status.insert(s.to_string(), 0);
    }
    for p in &["critical", "high", "medium", "low"] {
        by_priority.insert(p.to_string(), 0);
    }
    for k in &["bug", "feature", "task", "epic"] {
        by_kind.insert(k.to_string(), 0);
    }

    let mut blocked_count = 0i64;
    let mut ready_count = 0i64;
    let mut urgency_sum = 0.0f64;
    let mut active_count = 0i64;
    let mut oldest_open: Option<OldestOpen> = None;
    let mut by_skills: HashMap<String, i64> = HashMap::new();
    let mut by_assignee: HashMap<String, i64> = HashMap::new();

    for issue in &all_issues {
        *by_status.entry(issue.status.clone()).or_insert(0) += 1;
        *by_priority.entry(issue.priority.clone()).or_insert(0) += 1;
        *by_kind.entry(issue.kind.clone()).or_insert(0) += 1;

        if issue.status != "done" && issue.status != "wontfix" {
            let is_blocked = db::is_blocked(conn, issue.id).unwrap_or(false);
            if is_blocked {
                blocked_count += 1;
            } else {
                ready_count += 1;
            }

            let urg = urgency::compute_urgency(issue, &config, conn);
            urgency_sum += urg;
            active_count += 1;

            for skill in &issue.skills {
                *by_skills.entry(skill.clone()).or_insert(0) += 1;
            }
            if !issue.assigned_to.is_empty() {
                *by_assignee.entry(issue.assigned_to.clone()).or_insert(0) += 1;
            }

            // Track oldest open
            if issue.status == "open" {
                let days = util::days_since(&issue.created_at) as i64;
                match &oldest_open {
                    None => {
                        oldest_open = Some(OldestOpen {
                            id: issue.id,
                            title: issue.title.clone(),
                            days_old: days,
                        });
                    }
                    Some(current) => {
                        if days > current.days_old {
                            oldest_open = Some(OldestOpen {
                                id: issue.id,
                                title: issue.title.clone(),
                                days_old: days,
                            });
                        }
                    }
                }
            }
        }
    }

    let avg_urgency = if active_count > 0 {
        urgency_sum / active_count as f64
    } else {
        0.0
    };

    let stats = Stats {
        total,
        by_status,
        by_priority,
        by_kind,
        blocked: blocked_count,
        ready: ready_count,
        avg_urgency,
        by_skills,
        by_assignee,
        oldest_open,
    };

    println!("{}", format::format_stats(&stats, fmt));
    Ok(())
}

