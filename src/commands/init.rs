use crate::db;
use crate::error::NitError;
use crate::format::Format;
use std::env;
use std::fs;
use std::path::PathBuf;

pub fn run(agents_md: bool, fmt: Format, db_override: Option<&str>) -> Result<(), NitError> {
    let db_path = if let Some(p) = db_override {
        PathBuf::from(p)
    } else if let Ok(p) = env::var("NIT_DB_PATH") {
        PathBuf::from(p)
    } else {
        let cwd = env::current_dir().map_err(NitError::Io)?;
        cwd.join(".nit.db")
    };

    let created = if db_path.exists() {
        // Idempotent: already exists
        let _conn = db::open_db(&db_path)?;
        false
    } else {
        let _conn = db::init_db(&db_path)?;
        true
    };

    if agents_md {
        let agents_dir = db_path.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| {
            env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        });
        append_agents_md(&agents_dir)?;
    }

    let path_str = db_path.to_string_lossy().to_string();
    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "init",
                "path": path_str,
                "created": created,
            });
            println!("{}", out);
        }
        _ => {
            println!("INIT: {}", path_str);
        }
    }

    Ok(())
}

fn append_agents_md(cwd: &PathBuf) -> Result<(), NitError> {
    let agents_path = cwd.join("AGENTS.md");
    let block = r#"
## Issue Tracking

This project uses `nit` for issue tracking. Before starting work, run `nit ready -f json`
to find the next actionable task. After completing work, run `nit close <ID> "reason"`.
File discovered issues with `nit add`. Always run `nit note <ID> "summary"` before ending a session.
"#;

    if agents_path.exists() {
        let content = fs::read_to_string(&agents_path)?;
        if content.contains("## Issue Tracking") {
            return Ok(()); // already has it
        }
        let mut content = content;
        content.push_str(block);
        fs::write(&agents_path, content)?;
    } else {
        fs::write(&agents_path, block.trim_start())?;
    }
    Ok(())
}
