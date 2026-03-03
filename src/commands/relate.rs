use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use rusqlite::Connection;

pub fn run_relate(
    conn: &Connection,
    source_id: i64,
    target_id: i64,
    relation_type: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    // Validate relation type
    match relation_type {
        "duplicate" | "related" | "supersedes" => {}
        _ => {
            return Err(ItrError::InvalidValue {
                field: "relation_type".to_string(),
                value: relation_type.to_string(),
                valid: "duplicate, related, supersedes".to_string(),
            });
        }
    }

    let created = db::add_relation(conn, source_id, target_id, relation_type)?;

    let msg = if created {
        format!(
            "RELATION:{} {} -> {} ({})",
            "created", source_id, target_id, relation_type
        )
    } else {
        format!(
            "RELATION:{} {} -> {} ({})",
            "exists", source_id, target_id, relation_type
        )
    };

    match fmt {
        Format::Json => {
            let json = serde_json::json!({
                "source_id": source_id,
                "target_id": target_id,
                "relation_type": relation_type,
                "created": created,
            });
            println!("{}", json);
        }
        _ => {
            println!("{}", msg);
        }
    }

    Ok(())
}

pub fn run_unrelate(
    conn: &Connection,
    source_id: i64,
    target_id: i64,
    fmt: Format,
) -> Result<(), ItrError> {
    let removed = db::remove_relation(conn, source_id, target_id)?;

    let msg = if removed {
        format!("RELATION:removed {} -> {}", source_id, target_id)
    } else {
        format!("RELATION:not_found {} -> {}", source_id, target_id)
    };

    match fmt {
        Format::Json => {
            let json = serde_json::json!({
                "source_id": source_id,
                "target_id": target_id,
                "removed": removed,
            });
            println!("{}", json);
        }
        _ => {
            println!("{}", msg);
        }
    }

    Ok(())
}
