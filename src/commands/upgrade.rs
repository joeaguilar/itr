use crate::error::ItrError;
use crate::format::Format;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn run(no_pull: bool, source_dir: Option<String>, fmt: Format) -> Result<(), ItrError> {
    let src = find_source_dir(source_dir)?;

    let mut pulled_changes = false;

    if !no_pull {
        let output = Command::new("git")
            .args(["pull"])
            .current_dir(&src)
            .output()
            .map_err(|e| ItrError::UpgradeFailed(format!("git pull failed: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(ItrError::UpgradeFailed(format!(
                "git pull failed: {}",
                stderr.trim()
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        pulled_changes = !stdout.contains("Already up to date");
    }

    // Build release
    let output = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&src)
        .output()
        .map_err(|e| ItrError::UpgradeFailed(format!("cargo build failed: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(ItrError::UpgradeFailed(format!(
            "cargo build failed: {}",
            stderr.trim()
        )));
    }

    // Copy built binary to current exe location
    let built = src.join("target/release/itr");
    let current_exe = env::current_exe()
        .map_err(|e| ItrError::UpgradeFailed(format!("cannot find current exe: {}", e)))?;

    // Skip copy if source and destination are the same file
    let built_canonical = std::fs::canonicalize(&built).unwrap_or_else(|_| built.clone());
    let exe_canonical = std::fs::canonicalize(&current_exe).unwrap_or_else(|_| current_exe.clone());
    if built_canonical != exe_canonical {
        std::fs::copy(&built, &current_exe)
            .map_err(|e| ItrError::UpgradeFailed(format!("copy failed: {}", e)))?;
    }

    match fmt {
        Format::Json => {
            let out = serde_json::json!({
                "action": "upgrade",
                "source": src.to_string_lossy(),
                "binary": current_exe.to_string_lossy(),
                "pulled": !no_pull,
                "new_changes": pulled_changes,
            });
            println!("{}", out);
        }
        _ => {
            println!("UPGRADE: rebuilt from {}", src.display());
            if pulled_changes {
                println!("  new changes pulled from remote");
            }
            println!("  installed to {}", current_exe.display());
        }
    }

    Ok(())
}

fn find_source_dir(override_dir: Option<String>) -> Result<PathBuf, ItrError> {
    // 1. Explicit override
    if let Some(d) = override_dir {
        let p = PathBuf::from(&d);
        if is_itr_source(&p) {
            return Ok(p);
        }
        return Err(ItrError::UpgradeFailed(format!(
            "'{}' does not contain itr Cargo.toml",
            d
        )));
    }

    // 2. ITR_SOURCE_DIR env var
    if let Ok(d) = env::var("ITR_SOURCE_DIR") {
        let p = PathBuf::from(&d);
        if is_itr_source(&p) {
            return Ok(p);
        }
    }

    // 3. Compile-time source directory (always valid if built from source)
    let compile_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if is_itr_source(&compile_dir) {
        return Ok(compile_dir);
    }

    // 4. Infer from binary path ancestors
    if let Ok(exe) = env::current_exe() {
        let mut dir = exe.clone();
        for _ in 0..5 {
            dir.pop();
            if is_itr_source(&dir) {
                return Ok(dir);
            }
        }
    }

    // 5. Walk up from cwd
    if let Ok(mut dir) = env::current_dir() {
        loop {
            if is_itr_source(&dir) {
                return Ok(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }

    Err(ItrError::UpgradeFailed(
        "cannot find itr source directory. Use --source-dir or set ITR_SOURCE_DIR".to_string(),
    ))
}

fn is_itr_source(dir: &Path) -> bool {
    let cargo_toml = dir.join("Cargo.toml");
    if let Ok(content) = std::fs::read_to_string(cargo_toml) {
        content.contains("name = \"itr\"")
    } else {
        false
    }
}
