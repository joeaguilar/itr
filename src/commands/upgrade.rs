use crate::error::ItrError;
use crate::format::Format;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn run(no_pull: bool, source_dir: Option<String>, fmt: Format) -> Result<(), ItrError> {
    let src = find_source_dir(source_dir)?;
    let old_version = env!("ITR_VERSION").to_string();
    let verbose = !fmt.is_json();

    let mut pulled_changes = false;

    if !no_pull {
        if verbose {
            eprintln!("UPGRADE: pulling latest from remote...");
        }
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

    // Build release — inherit stderr so cargo's progress output streams through
    if verbose {
        eprintln!("UPGRADE: building release (this may take 15-20s)...");
    }
    let status = Command::new("cargo")
        .args(["build", "--release"])
        .current_dir(&src)
        .stdout(Stdio::null())
        .stderr(if verbose {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .status()
        .map_err(|e| ItrError::UpgradeFailed(format!("cargo build failed: {}", e)))?;

    if !status.success() {
        return Err(ItrError::UpgradeFailed("cargo build failed".to_string()));
    }

    // Get new version from the freshly built binary
    let built = src.join("target/release/itr");
    let new_version = Command::new(&built)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map_or_else(
            || "unknown".to_string(),
            |s| s.trim().trim_start_matches("itr ").to_string(),
        );

    // Copy built binary to current exe location
    if verbose {
        eprintln!("UPGRADE: installing...");
    }
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
                "old_version": old_version,
                "new_version": new_version,
                "source": src.to_string_lossy(),
                "binary": current_exe.to_string_lossy(),
                "pulled": !no_pull,
                "new_changes": pulled_changes,
            });
            println!("{}", out);
        }
        _ => {
            println!("UPGRADE: {} -> {}", old_version, new_version);
            println!("  rebuilt from {}", src.display());
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
