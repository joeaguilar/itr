use crate::db;
use crate::error::ItrError;
use crate::format::Format;
use crate::urgency::UrgencyConfig;
use rusqlite::Connection;

pub fn run_list(conn: &Connection, fmt: Format) -> Result<(), ItrError> {
    let stored = db::config_list(conn)?;
    let defaults = UrgencyConfig::defaults_map();

    // Merge: show defaults with overrides
    let mut entries: Vec<(String, String, bool)> = Vec::new(); // (key, value, is_custom)

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
                    return Err(ItrError::NotFound(-1));
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

pub fn run_set(conn: &Connection, key: &str, value: &str, fmt: Format) -> Result<(), ItrError> {
    db::config_set(conn, key, value)?;

    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "action": "set", "key": key, "value": value });
            println!("{}", out);
        }
        _ => {
            println!("SET: {}={}", key, value);
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
