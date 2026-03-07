use crate::commands::build_issue_detail;
use crate::db;
use crate::error::ItrError;
use crate::format::{self, Format};
use crate::urgency::UrgencyConfig;
use rusqlite::Connection;

pub fn run_assign(conn: &Connection, id: i64, agent: &str, fmt: Format) -> Result<(), ItrError> {
    let old_issue = db::get_issue(conn, id)?;

    db::record_event(conn, id, "assigned_to", &old_issue.assigned_to, agent)?;
    db::update_issue_field(conn, id, "assigned_to", agent)?;
    db::add_note(conn, id, &format!("Assigned to {}", agent), "itr")?;

    print_detail(conn, id, fmt)
}

pub fn run_unassign(conn: &Connection, id: i64, fmt: Format) -> Result<(), ItrError> {
    let issue = db::get_issue(conn, id)?;

    db::record_event(conn, id, "assigned_to", &issue.assigned_to, "")?;
    if !issue.assigned_to.is_empty() {
        db::add_note(
            conn,
            id,
            &format!("Unassigned from {}", issue.assigned_to),
            "itr",
        )?;
    }
    db::update_issue_field(conn, id, "assigned_to", "")?;

    print_detail(conn, id, fmt)
}

fn print_detail(conn: &Connection, id: i64, fmt: Format) -> Result<(), ItrError> {
    let issue = db::get_issue(conn, id)?;
    let config = UrgencyConfig::load(conn);
    let detail = build_issue_detail(conn, issue, &config)?;
    println!("{}", format::format_issue_detail(&detail, fmt));
    Ok(())
}
