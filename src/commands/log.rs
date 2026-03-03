use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    id: Option<i64>,
    limit: usize,
    since: Option<String>,
    fmt: Format,
) -> Result<(), ItrError> {
    let events = if let Some(issue_id) = id {
        // Verify issue exists
        let _issue = db::get_issue(conn, issue_id)?;
        db::get_events_for_issue(conn, issue_id)?
    } else {
        db::get_recent_events(conn, limit, since.as_deref())?
    };

    if events.is_empty() {
        error::print_empty(fmt.is_json(), "No events found.");
        return Ok(());
    }

    // Apply limit for issue-specific queries too
    let events = if id.is_some() {
        if events.len() > limit {
            events[events.len() - limit..].to_vec()
        } else {
            events
        }
    } else {
        events
    };

    println!("{}", format::format_events(&events, fmt));
    Ok(())
}
