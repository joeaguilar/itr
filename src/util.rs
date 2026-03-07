/// Split a comma-separated string into trimmed, non-empty parts.
pub fn parse_comma_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Split a comma-separated string into trimmed, lowercased, non-empty parts.
pub fn parse_comma_list_lower(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Apply add/remove edits to a tag list.
/// Tags in `add` that are not already present are appended.
/// Tags in `remove` are dropped.
pub fn apply_tags(mut current: Vec<String>, add: &[String], remove: &[String]) -> Vec<String> {
    for t in add {
        if !current.contains(t) {
            current.push(t.clone());
        }
    }
    current.retain(|t| !remove.contains(t));
    current
}

/// Apply add/remove edits to a skill list.
/// Skills are normalized to lowercase. Skills in `add` that are not already
/// present are appended. Skills in `remove` (after lowercasing) are dropped.
pub fn apply_skills(mut current: Vec<String>, add: &[String], remove: &[String]) -> Vec<String> {
    for s in add {
        let lowered = s.trim().to_lowercase();
        if !lowered.is_empty() && !current.contains(&lowered) {
            current.push(lowered);
        }
    }
    let remove_lower: Vec<String> = remove.iter().map(|s| s.trim().to_lowercase()).collect();
    current.retain(|s| !remove_lower.contains(s));
    current
}

/// Parse an ISO 8601 timestamp and return the number of days since that time.
pub fn days_since(iso_date: &str) -> f64 {
    use chrono::{NaiveDateTime, Utc};
    let parsed = NaiveDateTime::parse_from_str(iso_date, "%Y-%m-%dT%H:%M:%SZ");
    match parsed {
        Ok(dt) => {
            let now = Utc::now().naive_utc();
            let duration = now.signed_duration_since(dt);
            duration.num_seconds() as f64 / 86400.0
        }
        Err(_) => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_comma_list ---

    #[test]
    fn parse_comma_list_basic() {
        assert_eq!(parse_comma_list("foo,bar,baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn parse_comma_list_trims_whitespace() {
        assert_eq!(parse_comma_list("foo , bar , baz"), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn parse_comma_list_filters_empty() {
        assert_eq!(parse_comma_list("foo,,bar"), vec!["foo", "bar"]);
        assert_eq!(parse_comma_list(",foo,"), vec!["foo"]);
    }

    #[test]
    fn parse_comma_list_single() {
        assert_eq!(parse_comma_list("foo"), vec!["foo"]);
    }

    #[test]
    fn parse_comma_list_empty_string() {
        let result: Vec<String> = parse_comma_list("");
        assert!(result.is_empty());
    }

    // --- parse_comma_list_lower ---

    #[test]
    fn parse_comma_list_lower_normalizes_case() {
        assert_eq!(parse_comma_list_lower("Rust,SQL,Go"), vec!["rust", "sql", "go"]);
    }

    #[test]
    fn parse_comma_list_lower_trims_and_filters() {
        assert_eq!(parse_comma_list_lower(" Rust , , SQL "), vec!["rust", "sql"]);
    }

    // --- apply_tags ---

    #[test]
    fn apply_tags_adds_new() {
        let result = apply_tags(vec!["a".into()], &["b".into()], &[]);
        assert_eq!(result, vec!["a", "b"]);
    }

    #[test]
    fn apply_tags_no_duplicate_add() {
        let result = apply_tags(vec!["a".into()], &["a".into()], &[]);
        assert_eq!(result, vec!["a"]);
    }

    #[test]
    fn apply_tags_removes() {
        let result = apply_tags(vec!["a".into(), "b".into()], &[], &["a".into()]);
        assert_eq!(result, vec!["b"]);
    }

    #[test]
    fn apply_tags_add_and_remove() {
        let result = apply_tags(
            vec!["a".into(), "b".into()],
            &["c".into()],
            &["a".into()],
        );
        assert_eq!(result, vec!["b", "c"]);
    }

    #[test]
    fn apply_tags_remove_nonexistent_is_noop() {
        let result = apply_tags(vec!["a".into()], &[], &["z".into()]);
        assert_eq!(result, vec!["a"]);
    }

    #[test]
    fn apply_tags_empty_current() {
        let result = apply_tags(vec![], &["x".into()], &[]);
        assert_eq!(result, vec!["x"]);
    }

    // --- apply_skills ---

    #[test]
    fn apply_skills_lowercases_on_add() {
        let result = apply_skills(vec![], &["Rust".into(), "SQL".into()], &[]);
        assert_eq!(result, vec!["rust", "sql"]);
    }

    #[test]
    fn apply_skills_no_duplicate_add() {
        let result = apply_skills(vec!["rust".into()], &["Rust".into()], &[]);
        assert_eq!(result, vec!["rust"]);
    }

    #[test]
    fn apply_skills_removes_case_insensitive() {
        let result = apply_skills(vec!["rust".into(), "go".into()], &[], &["Rust".into()]);
        assert_eq!(result, vec!["go"]);
    }

    #[test]
    fn apply_skills_skips_empty_on_add() {
        let result = apply_skills(vec![], &["  ".into(), "rust".into()], &[]);
        assert_eq!(result, vec!["rust"]);
    }

    #[test]
    fn apply_skills_add_and_remove() {
        let result = apply_skills(
            vec!["rust".into(), "go".into()],
            &["sql".into()],
            &["go".into()],
        );
        assert_eq!(result, vec!["rust", "sql"]);
    }

    // --- days_since ---

    #[test]
    fn days_since_known_past_date() {
        let result = days_since("2020-01-01T00:00:00Z");
        assert!(result > 0.0, "expected positive days for a past date, got {result}");
    }

    #[test]
    fn days_since_unparseable_returns_zero() {
        assert_eq!(days_since("not-a-date"), 0.0);
    }
}
