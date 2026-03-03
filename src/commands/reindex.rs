use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use rusqlite::Connection;

pub fn run(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    db::fts_rebuild(conn)?;

    let count = db::all_issues(conn)?.len();

    match fmt {
        Format::Json => {
            let json = serde_json::json!({
                "action": "reindex",
                "indexed": count,
            });
            println!("{}", json);
        }
        _ => {
            println!("REINDEX: Rebuilt FTS index for {} issues", count);
        }
    }

    Ok(())
}
