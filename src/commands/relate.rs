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
    relation_type: Option<&str>,
    fmt: Format,
) -> Result<(), ItrError> {
    // Optional --type filter: only remove links of one relation type,
    // leaving other typed links between the pair intact. `None` keeps the
    // historical behavior of removing every type.
    if let Some(rt) = relation_type {
        match rt {
            "duplicate" | "related" | "supersedes" => {}
            _ => {
                return Err(ItrError::InvalidValue {
                    field: "relation_type".to_string(),
                    value: rt.to_string(),
                    valid: "duplicate, related, supersedes".to_string(),
                });
            }
        }
    }

    // Direction-aware: the pair is matched however it was stored, and every
    // removed link is reported with its type and stored direction (#186).
    let removed = db::remove_relation(conn, source_id, target_id, relation_type)?;

    match fmt {
        Format::Json => {
            let removed_relations: Vec<serde_json::Value> = removed
                .iter()
                .map(|rel| {
                    serde_json::json!({
                        "source_id": rel.source_id,
                        "target_id": rel.target_id,
                        "relation_type": rel.relation_type,
                    })
                })
                .collect();
            let json = serde_json::json!({
                "source_id": source_id,
                "target_id": target_id,
                "removed": !removed.is_empty(),
                "removed_relations": removed_relations,
            });
            println!("{}", json);
        }
        _ => {
            if removed.is_empty() {
                println!("RELATION:not_found {} -> {}", source_id, target_id);
            } else {
                for rel in &removed {
                    println!(
                        "RELATION:removed {} -> {} ({})",
                        rel.source_id, rel.target_id, rel.relation_type
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    fn seed(conn: &Connection, title: &str) -> i64 {
        db::insert_issue(
            conn,
            title,
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
        .expect("insert issue")
        .id
    }

    #[test]
    fn unrelate_with_type_removes_only_that_type() {
        // Handoff from #(relations wave): `--type` threads through the CLI
        // handler into db::remove_relation, leaving other typed links intact.
        let conn = db::open_test_db();
        let a = seed(&conn, "source");
        let b = seed(&conn, "target");
        db::add_relation(&conn, a, b, "related").expect("add related");
        db::add_relation(&conn, a, b, "duplicate").expect("add duplicate");

        run_unrelate(&conn, a, b, Some("related"), Format::Compact).expect("typed unrelate");

        let remaining = db::get_relations(&conn, a).expect("relations");
        assert_eq!(
            remaining.len(),
            1,
            "only the related link is removed, got: {:?}",
            remaining
        );
        assert_eq!(remaining[0].relation_type, "duplicate");
    }

    #[test]
    fn unrelate_without_type_still_removes_all_types() {
        let conn = db::open_test_db();
        let a = seed(&conn, "source");
        let b = seed(&conn, "target");
        db::add_relation(&conn, a, b, "related").expect("add related");
        db::add_relation(&conn, a, b, "duplicate").expect("add duplicate");

        run_unrelate(&conn, a, b, None, Format::Compact).expect("untyped unrelate");

        assert!(
            db::get_relations(&conn, a).expect("relations").is_empty(),
            "untyped unrelate keeps the historical remove-everything behavior"
        );
    }

    #[test]
    fn unrelate_rejects_unknown_type_and_leaves_relations_untouched() {
        let conn = db::open_test_db();
        let a = seed(&conn, "source");
        let b = seed(&conn, "target");
        db::add_relation(&conn, a, b, "related").expect("add related");

        let err = run_unrelate(&conn, a, b, Some("bogus"), Format::Compact).unwrap_err();
        assert!(
            matches!(err, ItrError::InvalidValue { .. }),
            "unknown --type must be INVALID_VALUE, got: {:?}",
            err
        );
        assert_eq!(
            db::get_relations(&conn, a).expect("relations").len(),
            1,
            "rejected unrelate must not remove anything"
        );
    }
}
