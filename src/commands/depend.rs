use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use rusqlite::Connection;

pub fn run(conn: &Connection, id: i64, on: i64, fmt: Format) -> Result<(), ItrError> {
    let created = db::add_dependency(conn, on, id)?;

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "depend",
                "blocked_id": id,
                "blocker_id": on,
                "created": created,
            });
            println!("{}", out);
        }
        _ => {
            println!("DEPEND: {} blocked by {}", id, on);
        }
    }

    Ok(())
}

pub fn run_undepend(conn: &Connection, id: i64, on: i64, fmt: Format) -> Result<(), ItrError> {
    // Capture pre-state so UNBLOCKED only fires on a real blocked->unblocked
    // transition caused by this command, never on a no-op (#191).
    let was_blocked = db::is_blocked(conn, id)?;
    let removed = db::remove_dependency(conn, on, id)?;

    let unblocked = if removed && was_blocked && !db::is_blocked(conn, id)? {
        let issue = db::get_issue(conn, id)?;
        if issue.status != "done" && issue.status != "wontfix" {
            vec![(issue.id, issue.title)]
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "undepend",
                "blocked_id": id,
                "blocker_id": on,
                "removed": removed,
            });
            println!("{}", out);
        }
        _ => {
            if removed {
                println!("UNDEPEND: {} no longer blocked by {}", id, on);
            } else {
                println!("UNDEPEND:not_found {} was not blocked by {}", id, on);
            }
        }
    }

    if !unblocked.is_empty() {
        let unblocked_str = crate::format::format_unblocked(&unblocked, fmt);
        if !unblocked_str.is_empty() {
            println!("{}", unblocked_str);
        }
    }

    Ok(())
}
