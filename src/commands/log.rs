use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::Event;
use rusqlite::Connection;

pub fn run(
    conn: &Connection,
    id: Option<i64>,
    limit: usize,
    since: Option<String>,
    agent: Option<String>,
    fmt: Format,
) -> Result<(), ItrError> {
    let events = run_core(conn, id, limit, since.as_deref(), agent.as_deref())?;

    if events.is_empty() {
        error::print_empty(fmt.is_json(), "No events found.");
        return Ok(());
    }

    println!("{}", format::format_events(&events, fmt));
    Ok(())
}

/// Resolve the filtered event list for `itr log`.
///
/// Every filter (issue scope, `--since`, `--agent`) is applied in SQL before
/// the limit (#170), so the newest matching events are returned even when
/// they are interleaved with newer events from other agents. Per-issue logs
/// stay chronological (oldest first); the global log is newest first.
pub(crate) fn run_core(
    conn: &Connection,
    id: Option<i64>,
    limit: usize,
    since: Option<&str>,
    agent: Option<&str>,
) -> Result<Vec<Event>, ItrError> {
    if let Some(issue_id) = id {
        // Verify issue exists
        let _issue = db::get_issue(conn, issue_id)?;
    }

    let mut events = db::get_events_filtered(conn, id, limit, since, agent)?;
    if id.is_some() {
        // get_events_filtered returns newest-first; per-issue history reads
        // top-to-bottom in chronological order.
        events.reverse();
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seed_issue(conn: &Connection) -> i64 {
        db::insert_issue(
            conn,
            "log target",
            "medium",
            "task",
            "",
            &[],
            &[],
            &[],
            "",
            None,
            "",
        )
        .unwrap()
        .id
    }

    fn insert_event_at(conn: &Connection, issue_id: i64, agent: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO events (issue_id, field, old_value, new_value, agent, created_at)
             VALUES (?1, 'status', 'open', 'in-progress', ?2, ?3)",
            params![issue_id, agent, created_at],
        )
        .unwrap();
    }

    // #170 defect 1: --since was silently ignored when an issue ID was given.
    #[test]
    fn since_filters_issue_scoped_log() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-03-01T00:00:00Z");

        let events = run_core(&conn, Some(id), 50, Some("2026-02-01T00:00:00Z"), None).unwrap();
        assert_eq!(events.len(), 1, "--since must apply to `itr log <id>`");
        assert_eq!(events[0].created_at, "2026-03-01T00:00:00Z");

        // A future --since is an empty (non-error) result.
        let events = run_core(&conn, Some(id), 50, Some("2099-01-01T00:00:00Z"), None).unwrap();
        assert!(events.is_empty());
    }

    // #170 defect 2: --agent filtered in memory after LIMIT, so matches
    // older than the N newest events overall were silently dropped.
    #[test]
    fn agent_filter_applies_before_limit_in_global_log() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-02T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-03T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-04T00:00:00Z");

        let events = run_core(&conn, None, 3, None, Some("alice")).unwrap();
        assert_eq!(
            events.len(),
            1,
            "alice's event lies beyond the 3 newest overall but must be found"
        );
        assert_eq!(events[0].agent, "alice");
    }

    // Per-issue logs stay chronological and keep the newest N when limited.
    #[test]
    fn issue_log_is_chronological_and_keeps_newest_when_limited() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-02T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-03T00:00:00Z");

        let events = run_core(&conn, Some(id), 2, None, None).unwrap();
        let stamps: Vec<&str> = events.iter().map(|e| e.created_at.as_str()).collect();
        assert_eq!(stamps, vec!["2026-01-02T00:00:00Z", "2026-01-03T00:00:00Z"]);
    }

    // The global log stays newest-first.
    #[test]
    fn global_log_is_newest_first() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-02T00:00:00Z");

        let events = run_core(&conn, None, 50, None, None).unwrap();
        let stamps: Vec<&str> = events.iter().map(|e| e.created_at.as_str()).collect();
        assert_eq!(stamps, vec!["2026-01-02T00:00:00Z", "2026-01-01T00:00:00Z"]);
    }

    // Unknown issue IDs still surface NOT_FOUND.
    #[test]
    fn unknown_issue_is_not_found() {
        let conn = db::open_test_db();
        assert!(matches!(
            run_core(&conn, Some(999), 50, None, None),
            Err(ItrError::NotFound(999))
        ));
    }
}
