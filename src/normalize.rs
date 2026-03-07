pub fn normalize_priority(p: &str) -> String {
    match p.to_lowercase().as_str() {
        "critical" | "high" | "medium" | "low" => p.to_lowercase(),
        "urgent" | "p0" | "highest" => "critical".to_string(),
        "p1" => "high".to_string(),
        "p2" | "normal" => "medium".to_string(),
        "p3" | "lowest" => "low".to_string(),
        _ => p.to_lowercase(),
    }
}

pub fn normalize_kind(k: &str) -> String {
    match k.to_lowercase().as_str() {
        "bug" | "feature" | "task" | "epic" => k.to_lowercase(),
        "enhancement" | "feat" | "story" => "feature".to_string(),
        "bugfix" | "defect" => "bug".to_string(),
        "chore" | "subtask" => "task".to_string(),
        _ => k.to_lowercase(),
    }
}

pub fn normalize_status(s: &str) -> String {
    match s.to_lowercase().as_str() {
        "open" | "in-progress" | "done" | "wontfix" => s.to_lowercase(),
        "todo" | "new" | "backlog" => "open".to_string(),
        "closed" | "resolved" | "fixed" => "done".to_string(),
        "cancelled" | "canceled" => "wontfix".to_string(),
        "wip" | "started" | "progress" | "in_progress" | "inprogress" => "in-progress".to_string(),
        _ => s.to_lowercase(),
    }
}

use crate::error::ItrError;

pub fn validate_priority(p: &str) -> Result<(), ItrError> {
    match p {
        "critical" | "high" | "medium" | "low" => Ok(()),
        _ => Err(ItrError::InvalidValue {
            field: "priority".to_string(),
            value: p.to_string(),
            valid: "critical, high, medium, low".to_string(),
        }),
    }
}

pub fn validate_kind(k: &str) -> Result<(), ItrError> {
    match k {
        "bug" | "feature" | "task" | "epic" => Ok(()),
        _ => Err(ItrError::InvalidValue {
            field: "kind".to_string(),
            value: k.to_string(),
            valid: "bug, feature, task, epic".to_string(),
        }),
    }
}

pub fn validate_status(s: &str) -> Result<(), ItrError> {
    match s {
        "open" | "in-progress" | "done" | "wontfix" => Ok(()),
        _ => Err(ItrError::InvalidValue {
            field: "status".to_string(),
            value: s.to_string(),
            valid: "open, in-progress, done, wontfix".to_string(),
        }),
    }
}
