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
