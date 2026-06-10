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

    // Install built binary at the current exe location
    if verbose {
        eprintln!("UPGRADE: installing...");
    }
    let current_exe = env::current_exe()
        .map_err(|e| ItrError::UpgradeFailed(format!("cannot find current exe: {}", e)))?;

    // Skip install if source and destination are the same file
    let built_canonical = std::fs::canonicalize(&built).unwrap_or_else(|_| built.clone());
    let exe_canonical = std::fs::canonicalize(&current_exe).unwrap_or_else(|_| current_exe.clone());
    if built_canonical != exe_canonical {
        install_binary(&built, &current_exe)?;
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

/// Install `src` over `dest` atomically: stage a copy next to the
/// destination, then rename it over the target. The temp file lives in the
/// destination directory so the rename stays on one filesystem and is
/// atomic — it replaces the target even while the old binary is running
/// (on Unix the old inode lives on until the process exits), avoiding
/// ETXTBSY on Linux, and an interrupted upgrade never leaves a truncated
/// binary at the install path.
fn install_binary(src: &Path, dest: &Path) -> Result<(), ItrError> {
    let dest_name = dest
        .file_name()
        .map_or_else(|| "itr".to_string(), |n| n.to_string_lossy().into_owned());
    let tmp = dest.with_file_name(format!("{}.new.{}", dest_name, std::process::id()));
    let result = stage_and_rename(src, dest, &tmp);
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn stage_and_rename(src: &Path, dest: &Path, tmp: &Path) -> Result<(), ItrError> {
    std::fs::copy(src, tmp).map_err(|e| ItrError::UpgradeFailed(format!("copy failed: {}", e)))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Carry the source mode over and guarantee the staged binary is
        // executable before it goes live.
        let mode = std::fs::metadata(src)
            .map(|m| m.permissions().mode())
            .unwrap_or(0o755);
        std::fs::set_permissions(tmp, std::fs::Permissions::from_mode(mode | 0o111))
            .map_err(|e| ItrError::UpgradeFailed(format!("chmod failed: {}", e)))?;
    }

    std::fs::rename(tmp, dest)
        .map_err(|e| ItrError::UpgradeFailed(format!("rename failed: {}", e)))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Fresh per-test directory under the OS temp dir, process-id-unique
    /// (same convention as the import.rs/db.rs test fixtures).
    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("itr-upgrade-unit-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).expect("metadata").permissions().mode()
    }

    /// Names of leftover staging files (`<dest>.new*`) in `dir`.
    fn temp_artifacts(dir: &Path, dest_name: &str) -> Vec<String> {
        let prefix = format!("{}.new", dest_name);
        fs::read_dir(dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&prefix))
            .collect()
    }

    #[test]
    fn install_replaces_existing_dest() {
        let dir = test_dir("replace");
        let src = dir.join("src-bin");
        let dest = dir.join("itr");
        fs::write(&src, b"new-binary-bytes").expect("write src");
        fs::write(&dest, b"old-binary-bytes").expect("write dest");

        install_binary(&src, &dest).expect("install should succeed");

        assert_eq!(fs::read(&dest).expect("read dest"), b"new-binary-bytes");
        assert!(
            temp_artifacts(&dir, "itr").is_empty(),
            "no staging file left behind"
        );
        #[cfg(unix)]
        assert_eq!(
            mode_of(&dest) & 0o111,
            0o111,
            "installed binary is executable"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// ETXTBSY analog: the installed binary cannot be opened for writing
    /// (write-protected here; busy text file on Linux). A straight
    /// `fs::copy` over the destination fails, while temp+rename succeeds
    /// because rename only needs write access to the directory.
    #[cfg(unix)]
    #[test]
    fn install_replaces_dest_that_cannot_be_opened_for_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = test_dir("etxtbsy");
        let src = dir.join("src-bin");
        let dest = dir.join("itr");
        fs::write(&src, b"new-binary-bytes").expect("write src");
        fs::write(&dest, b"old-binary-bytes").expect("write dest");
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o555)).expect("chmod dest");

        install_binary(&src, &dest).expect("install over unwritable dest should succeed");

        assert_eq!(fs::read(&dest).expect("read dest"), b"new-binary-bytes");
        assert_eq!(mode_of(&dest) & 0o111, 0o111);
        let _ = fs::remove_dir_all(&dir);
    }

    /// An upgrade interrupted while staging must never touch the
    /// installed binary: dest stays byte-identical and executable.
    #[test]
    fn failed_stage_leaves_dest_intact() {
        let dir = test_dir("stage-fail");
        let src = dir.join("src-bin");
        let dest = dir.join("itr");
        fs::write(&src, b"new-binary-bytes").expect("write src");
        fs::write(&dest, b"old-binary-bytes").expect("write dest");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).expect("chmod dest");
        }
        // Staging path in a directory that does not exist: the staging
        // copy fails before the destination is ever touched.
        let tmp = dir.join("missing-subdir").join("itr.new");

        let result = stage_and_rename(&src, &dest, &tmp);

        assert!(result.is_err(), "staging into missing dir must fail");
        assert_eq!(fs::read(&dest).expect("read dest"), b"old-binary-bytes");
        #[cfg(unix)]
        assert_eq!(mode_of(&dest) & 0o111, 0o111, "old binary still executable");
        let _ = fs::remove_dir_all(&dir);
    }

    /// When the final rename fails, the staging file must be cleaned up
    /// and the destination left as it was.
    #[test]
    fn failed_install_cleans_up_temp_file() {
        let dir = test_dir("cleanup");
        let src = dir.join("src-bin");
        // dest is a directory: staging succeeds, the final rename fails.
        let dest = dir.join("itr");
        fs::write(&src, b"new-binary-bytes").expect("write src");
        fs::create_dir_all(&dest).expect("create dest dir");

        let result = install_binary(&src, &dest);

        assert!(result.is_err(), "rename over a directory must fail");
        assert!(dest.is_dir(), "dest untouched");
        assert!(
            temp_artifacts(&dir, "itr").is_empty(),
            "staging file removed on failure"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
