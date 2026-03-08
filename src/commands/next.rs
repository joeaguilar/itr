use crate::commands::build_issue_detail;
use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::urgency::{self, UrgencyConfig};
use rusqlite::Connection;
use std::env;

pub fn run(
    conn: &Connection,
    claim: bool,
    id: Option<i64>,
    skills: Vec<String>,
    agent: Option<String>,
    assigned_to: Option<String>,
    fmt: Format,
) -> Result<(), ItrError> {
    let config = UrgencyConfig::load(conn);

    // If a specific ID is provided, claim it directly
    let issue = if let Some(target_id) = id {
        let issue = db::get_issue(conn, target_id)?;
        if claim {
            db::record_event(conn, issue.id, "status", &issue.status, "in-progress")?;
            db::update_issue_field(conn, issue.id, "status", "in-progress")?;
            let agent_name =
                agent.or_else(|| env::var("ITR_AGENT").ok().filter(|s| !s.is_empty()));
            if let Some(ref name) = agent_name {
                db::record_event(conn, issue.id, "assigned_to", &issue.assigned_to, name)?;
                db::update_issue_field(conn, issue.id, "assigned_to", name)?;
            }
        }
        db::get_issue(conn, issue.id)?
    } else {
        // Get all open, unblocked issues
        let issues = db::list_issues(
            conn,
            &["open".to_string()],
            &[],
            &[],
            &[],
            false,
            false,
            None,
            false,
            &skills,
            assigned_to.as_deref(),
            &[],
        )?;

        if issues.is_empty() {
            error::print_empty(fmt.is_json(), "No eligible issues found.");
            return Ok(());
        }

        // Find highest urgency
        let mut best = None;
        let mut best_urg = f64::NEG_INFINITY;

        for issue in &issues {
            let urg = urgency::compute_urgency(issue, &config, conn);
            if urg > best_urg {
                best_urg = urg;
                best = Some(issue.clone());
            }
        }

        let issue = best.ok_or_else(|| ItrError::NotFound(0))?;

        // Claim if requested
        if claim {
            db::record_event(conn, issue.id, "status", &issue.status, "in-progress")?;
            db::update_issue_field(conn, issue.id, "status", "in-progress")?;

            // Resolve agent name: explicit flag > ITR_AGENT env var
            let agent_name =
                agent.or_else(|| env::var("ITR_AGENT").ok().filter(|s| !s.is_empty()));
            if let Some(ref name) = agent_name {
                db::record_event(conn, issue.id, "assigned_to", &issue.assigned_to, name)?;
                db::update_issue_field(conn, issue.id, "assigned_to", name)?;
            }
        }

        // Re-read if claimed (status/assigned_to may have changed)
        if claim {
            db::get_issue(conn, issue.id)?
        } else {
            issue
        }
    };

    let detail = build_issue_detail(conn, issue, &config)?;
    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
}
