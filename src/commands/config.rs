use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::urgency::UrgencyConfig;
use rusqlite::Connection;

pub fn run_list(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    let stored = db::config_list(conn)?;
    let defaults = UrgencyConfig::defaults_map();

    // Merge: show defaults with overrides
    let mut entries: Vec<(String, String, bool)> = Vec::with_capacity(defaults.len()); // (key, value, is_custom)

    for (key, default_val) in &defaults {
        let stored_val = stored.iter().find(|(k, _)| k == key);
        match stored_val {
            Some((_, v)) => entries.push((key.to_string(), v.clone(), true)),
            None => entries.push((key.to_string(), format!("{}", default_val), false)),
        }
    }

    // Also include any non-urgency config entries
    for (key, val) in &stored {
        if !key.starts_with("urgency.") {
            entries.push((key.clone(), val.clone(), true));
        }
    }

    match fmt {
        Format::Json => {
            let map: serde_json::Map<String, serde_json::Value> = entries
                .iter()
                .map(|(k, v, _)| (k.clone(), serde_json::Value::String(v.clone())))
                .collect();
            println!("{}", serde_json::to_string(&map)?);
        }
        _ => {
            for (key, val, is_custom) in &entries {
                let marker = if *is_custom { " *" } else { "" };
                println!("{}={}{}", key, val, marker);
            }
        }
    }

    Ok(())
}

pub fn run_get(conn: &Connection, key: &str, fmt: Format) -> Result<(), ItrError> {
    let value = match db::config_get(conn, key)? {
        Some(v) => v,
        None => {
            // Check defaults
            let defaults = UrgencyConfig::defaults_map();
            match defaults.iter().find(|(k, _)| *k == key) {
                Some((_, v)) => format!("{}", v),
                None => {
                    return Err(ItrError::InvalidValue {
                        field: "config key".to_string(),
                        value: key.to_string(),
                        valid: "Use 'itr config list' to see available keys".to_string(),
                    });
                }
            }
        }
    };

    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "key": key, "value": value });
            println!("{}", out);
        }
        _ => {
            println!("{}={}", key, value);
        }
    }

    Ok(())
}

/// Outcome of soft-validating a `config set` request before it hits the DB.
struct SetValidation {
    /// Value to store, or `None` when the request should be ignored entirely
    /// (unknown `urgency.*` key — storing it would have zero effect and the
    /// key would be invisible in `config list`).
    store_value: Option<String>,
    /// `REVIEW:` warnings to emit on stderr (soft fallbacks, never errors).
    warnings: Vec<String>,
}

/// Validate a `config set` request, applying the project's soft-fallback
/// rules for `urgency.*` keys:
///
/// - known key + non-numeric value: keep the current effective coefficient
///   (existing override or default) and warn — never store a value the
///   urgency engine would silently ignore, so `config get`/`list` always
///   reflect effective behavior.
/// - unknown key: skip the write and warn with a "did you mean" suggestion
///   derived from [`UrgencyConfig::defaults_map`].
///
/// Non-urgency keys are stored verbatim with no checks.
fn validate_set(conn: &Connection, key: &str, value: &str) -> Result<SetValidation, ItrError> {
    if !key.starts_with("urgency.") {
        return Ok(SetValidation {
            store_value: Some(value.to_string()),
            warnings: Vec::new(),
        });
    }

    let defaults = UrgencyConfig::defaults_map();
    match defaults.iter().find(|(k, _)| *k == key) {
        Some((_, default_val)) => {
            if value.parse::<f64>().is_ok() {
                Ok(SetValidation {
                    store_value: Some(value.to_string()),
                    warnings: Vec::new(),
                })
            } else {
                // Soft fallback: keep whatever the engine is effectively
                // using today (a previously stored numeric override, else
                // the default) so display and behavior stay in sync.
                let effective = db::config_get(conn, key)?
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(*default_val);
                Ok(SetValidation {
                    store_value: Some(format!("{}", effective)),
                    warnings: vec![format!(
                        "REVIEW: value '{}' for '{}' is not numeric; urgency engine will use {} instead",
                        value, key, effective
                    )],
                })
            }
        }
        None => {
            let suggestion = match UrgencyConfig::closest_key(key) {
                Some(k) => format!(" (did you mean '{}'?)", k),
                None => String::new(),
            };
            Ok(SetValidation {
                store_value: None,
                warnings: vec![format!(
                    "REVIEW: unknown urgency config key '{}' ignored{}. Use 'itr config list' to see available keys",
                    key, suggestion
                )],
            })
        }
    }
}

pub fn run_set(conn: &Connection, key: &str, value: &str, fmt: Format) -> Result<(), ItrError> {
    let validation = validate_set(conn, key, value)?;
    for warning in &validation.warnings {
        eprintln!("{}", warning);
    }

    let stored = match &validation.store_value {
        Some(v) => {
            db::config_set(conn, key, v)?;
            v.as_str()
        }
        None => {
            match fmt {
                Format::Json => {
                    let out =
                        serde_json::json!({ "action": "ignored", "key": key, "value": value });
                    println!("{}", out);
                }
                _ => {
                    println!("IGNORED: {}={}", key, value);
                }
            }
            return Ok(());
        }
    };

    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "action": "set", "key": key, "value": stored });
            println!("{}", out);
        }
        _ => {
            println!("SET: {}={}", key, stored);
        }
    }

    Ok(())
}

pub fn run_reset(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    db::config_reset(conn)?;

    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "action": "reset" });
            println!("{}", out);
        }
        _ => {
            println!("CONFIG: Reset to defaults");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::get_schema_sql()).unwrap();
        conn
    }

    // --- #183: validate urgency.* keys and values at set time ---

    #[test]
    fn bogus_value_for_known_urgency_key_warns_and_falls_back_to_default() {
        let conn = test_conn();
        let v = validate_set(&conn, "urgency.priority.medium", "abc").unwrap();
        assert_eq!(v.store_value.as_deref(), Some("3"));
        assert_eq!(v.warnings.len(), 1);
        assert!(
            v.warnings[0].starts_with("REVIEW:"),
            "warning: {}",
            v.warnings[0]
        );
        assert!(
            v.warnings[0].contains("not numeric"),
            "warning: {}",
            v.warnings[0]
        );
        assert!(
            v.warnings[0].contains("will use 3"),
            "warning: {}",
            v.warnings[0]
        );
    }

    #[test]
    fn bogus_value_preserves_existing_numeric_override() {
        let conn = test_conn();
        db::config_set(&conn, "urgency.priority.medium", "5").unwrap();
        let v = validate_set(&conn, "urgency.priority.medium", "abc").unwrap();
        assert_eq!(v.store_value.as_deref(), Some("5"));
        assert!(
            v.warnings[0].contains("will use 5"),
            "warning: {}",
            v.warnings[0]
        );
    }

    #[test]
    fn unknown_urgency_key_is_ignored_with_closest_key_suggestion() {
        let conn = test_conn();
        let v = validate_set(&conn, "urgency.priority.critcal", "5").unwrap();
        assert!(v.store_value.is_none());
        assert_eq!(v.warnings.len(), 1);
        assert!(
            v.warnings[0].starts_with("REVIEW:"),
            "warning: {}",
            v.warnings[0]
        );
        assert!(
            v.warnings[0].contains("did you mean 'urgency.priority.critical'?"),
            "warning: {}",
            v.warnings[0]
        );
    }

    #[test]
    fn valid_urgency_value_and_non_urgency_keys_are_stored_verbatim() {
        let conn = test_conn();

        let v = validate_set(&conn, "urgency.priority.critical", "15.0").unwrap();
        assert_eq!(v.store_value.as_deref(), Some("15.0"));
        assert!(v.warnings.is_empty());

        let v = validate_set(&conn, "my.custom.key", "anything goes").unwrap();
        assert_eq!(v.store_value.as_deref(), Some("anything goes"));
        assert!(v.warnings.is_empty());
    }

    #[test]
    fn run_set_keeps_displayed_config_in_sync_with_effective_urgency() {
        let conn = test_conn();
        run_set(&conn, "urgency.priority.medium", "abc", Format::Compact).unwrap();

        let stored = db::config_get(&conn, "urgency.priority.medium")
            .unwrap()
            .expect("known key must still be readable after a bogus set");
        let displayed: f64 = stored
            .parse()
            .expect("stored urgency value must be numeric so display matches behavior");
        let effective = UrgencyConfig::load(&conn).priority_medium;
        assert!(
            (displayed - effective).abs() < 1e-9,
            "config get shows {displayed} but engine uses {effective}"
        );
    }

    #[test]
    fn run_set_does_not_store_unknown_urgency_keys() {
        let conn = test_conn();
        run_set(&conn, "urgency.priority.critcal", "5", Format::Compact).unwrap();
        assert_eq!(
            db::config_get(&conn, "urgency.priority.critcal").unwrap(),
            None
        );
    }
}
