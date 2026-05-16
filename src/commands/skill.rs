use crate::cli::{SkillAction, SkillScope};
use crate::error::ItrError;
use crate::format::Format;
use std::env;
use std::fs;
use std::path::PathBuf;

const SKILL_MD: &str = include_str!("../../skills/itr/SKILL.md");
const SKILL_DIR_NAME: &str = "itr";

pub fn run(action: Option<SkillAction>, fmt: Format) -> Result<(), ItrError> {
    match action {
        None => emit(fmt),
        Some(SkillAction::Install { scope, force }) => install(scope, force, fmt),
        Some(SkillAction::Path { scope }) => print_path(scope, fmt),
    }
}

#[allow(clippy::unnecessary_wraps)]
fn emit(fmt: Format) -> Result<(), ItrError> {
    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "skill": SKILL_MD });
            println!("{}", out);
        }
        _ => {
            print!("{}", SKILL_MD);
        }
    }
    Ok(())
}

fn install(scope: SkillScope, force: bool, fmt: Format) -> Result<(), ItrError> {
    let dir = skill_dir(scope)?;
    let path = dir.join("SKILL.md");

    if path.exists() && !force {
        eprintln!(
            "REVIEW: {} already exists. Re-run with --force to overwrite.",
            path.display()
        );
        return Ok(());
    }

    fs::create_dir_all(&dir)?;
    fs::write(&path, SKILL_MD)?;

    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "installed": path.display().to_string() });
            println!("{}", out);
        }
        _ => {
            println!("Installed itr skill → {}", path.display());
        }
    }
    Ok(())
}

fn print_path(scope: SkillScope, fmt: Format) -> Result<(), ItrError> {
    let path = skill_dir(scope)?.join("SKILL.md");
    match fmt {
        Format::Json => {
            let out = serde_json::json!({ "path": path.display().to_string() });
            println!("{}", out);
        }
        _ => {
            println!("{}", path.display());
        }
    }
    Ok(())
}

fn skill_dir(scope: SkillScope) -> Result<PathBuf, ItrError> {
    let base = match scope {
        SkillScope::User => home_dir()?,
        SkillScope::Project => env::current_dir()?,
    };
    Ok(base.join(".claude").join("skills").join(SKILL_DIR_NAME))
}

fn home_dir() -> Result<PathBuf, ItrError> {
    env::var_os("HOME")
        .or_else(|| env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| {
            ItrError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "Could not resolve home directory (HOME/USERPROFILE unset)",
            ))
        })
}
