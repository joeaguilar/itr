use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::util;
use rusqlite::Connection;

/// Validate a `--type` value shared by the single-ID, multi-ID, and bulk paths.
pub(crate) fn validate_relation_type(relation_type: &str) -> Result<(), ItrError> {
    match relation_type {
        "duplicate" | "related" | "supersedes" => Ok(()),
        _ => Err(ItrError::InvalidValue {
            field: "relation_type".to_string(),
            value: relation_type.to_string(),
            valid: "duplicate, related, supersedes".to_string(),
        }),
    }
}

/// `itr relate <ID>... --to N` — one or more source issue IDs, repeated,
/// comma-separated, or inclusive `A-B` ranges.
///
/// - Exactly one unique ID: unchanged single-issue contract (hard `NOT_FOUND`
///   on either side, hard `INVALID_VALUE` on a self-relation).
/// - Multiple unique IDs: all relations are created in one transaction with
///   per-ID soft fallback — a missing ID emits `REVIEW: id N not found;
///   skipped`, and an ID equal to `--to` skips the self-relation. Exit 0 if
///   at least one relation was processed, exit 1 if none were.
pub fn run_relate_multi(
    conn: &Connection,
    id_tokens: &[String],
    target_id: i64,
    relation_type: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    validate_relation_type(relation_type)?;

    let parsed = util::parse_id_tokens(id_tokens);
    for note in &parsed.notes {
        eprintln!("{}", note);
    }
    for token in &parsed.invalid {
        eprintln!(
            "REVIEW: ignoring non-integer issue ID '{}' — IDs may be repeated, comma-separated, or ranges (e.g. `itr relate 124-132 --to 53`)",
            token
        );
    }
    for id in &parsed.duplicates {
        eprintln!(
            "REVIEW: duplicate issue ID {} requested; relating it once",
            id
        );
    }
    if parsed.ids.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid:
                "integer issue IDs, repeated, comma-separated, or ranges (e.g. `itr relate 124-132 --to 53`)"
                    .to_string(),
        });
    }

    if parsed.ids.len() == 1 {
        return run_relate(conn, parsed.ids[0], target_id, relation_type, fmt);
    }

    // A missing --to target can never soft-recover: fail before touching
    // anything, matching the single-ID behavior.
    if !db::issue_exists(conn, target_id)? {
        return Err(ItrError::NotFound(target_id));
    }

    let tx = conn.unchecked_transaction()?;
    let mut links: Vec<(i64, bool)> = Vec::new();
    for &id in &parsed.ids {
        if id == target_id {
            eprintln!(
                "REVIEW: id {} equals the --to target; self-relation skipped",
                id
            );
            continue;
        }
        match db::add_relation(&tx, id, target_id, relation_type) {
            Ok(created) => links.push((id, created)),
            Err(ItrError::NotFound(_)) => {
                eprintln!("REVIEW: id {} not found; skipped", id);
            }
            Err(e) => return Err(e),
        }
    }
    if links.is_empty() {
        return Err(ItrError::InvalidValue {
            field: "id".to_string(),
            value: id_tokens.join(","),
            valid: "at least one existing issue ID distinct from --to".to_string(),
        });
    }
    tx.commit()?;

    match fmt {
        Format::Json => {
            let arr: Vec<serde_json::Value> = links
                .iter()
                .map(|(id, created)| {
                    serde_json::json!({
                        "source_id": id,
                        "target_id": target_id,
                        "relation_type": relation_type,
                        "created": created,
                    })
                })
                .collect();
            println!("{}", serde_json::Value::Array(arr));
        }
        _ => {
            for (id, created) in &links {
                let verb = if *created { "created" } else { "exists" };
                println!(
                    "RELATION:{} {} -> {} ({})",
                    verb, id, target_id, relation_type
                );
            }
        }
    }
    Ok(())
}

pub fn run_relate(
    conn: &Connection,
    source_id: i64,
    target_id: i64,
    relation_type: &str,
    fmt: Format,
) -> Result<(), ItrError> {
    validate_relation_type(relation_type)?;

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
        validate_relation_type(rt)?;
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

    // --- spec P1: multi-ID relate ---

    #[test]
    fn relate_multi_links_every_source_to_target() {
        let conn = db::open_test_db();
        let target = seed(&conn, "target");
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        let c = seed(&conn, "c");

        run_relate_multi(
            &conn,
            &[format!("{}-{}", a, c)],
            target,
            "related",
            Format::Compact,
        )
        .expect("multi relate");

        for id in [a, b, c] {
            let rels = db::get_relations(&conn, id).expect("relations");
            assert!(
                rels.iter()
                    .any(|r| r.source_id == id && r.target_id == target),
                "issue {id} must be related to the target"
            );
        }
    }

    #[test]
    fn relate_multi_skips_self_and_missing() {
        let conn = db::open_test_db();
        let target = seed(&conn, "target");
        let a = seed(&conn, "a");

        run_relate_multi(
            &conn,
            &[a.to_string(), target.to_string(), "999".to_string()],
            target,
            "related",
            Format::Compact,
        )
        .expect("soft fallback");

        let rels = db::get_relations(&conn, target).expect("relations");
        assert_eq!(rels.len(), 1, "only a->target; no self-relation");
        assert_eq!(rels[0].source_id, a);
    }

    #[test]
    fn relate_multi_missing_target_is_hard_error() {
        let conn = db::open_test_db();
        let a = seed(&conn, "a");
        let b = seed(&conn, "b");
        let err = run_relate_multi(
            &conn,
            &[format!("{},{}", a, b)],
            999,
            "related",
            Format::Compact,
        )
        .unwrap_err();
        assert!(matches!(err, ItrError::NotFound(999)));
    }

    #[test]
    fn relate_multi_single_id_keeps_single_contract() {
        let conn = db::open_test_db();
        let a = seed(&conn, "a");
        // Self-relation through the single path stays a hard INVALID_VALUE.
        let err =
            run_relate_multi(&conn, &[a.to_string()], a, "related", Format::Compact).unwrap_err();
        assert!(matches!(err, ItrError::InvalidValue { .. }));
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
