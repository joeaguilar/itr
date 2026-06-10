/// Split a comma-separated string into trimmed, non-empty parts.
///
/// Used to parse CLI inputs like `--tags rust,docs,score`. Whitespace around
/// each segment is stripped; empty segments (from leading, trailing, or
/// doubled commas) are dropped without erroring.
///
/// # Examples
///
/// ```text
/// use itr::util::parse_comma_list;
/// assert_eq!(parse_comma_list("foo,bar,baz"), vec!["foo", "bar", "baz"]);
/// assert_eq!(parse_comma_list("foo , bar"), vec!["foo", "bar"]);
/// assert_eq!(parse_comma_list(",foo,,bar,"), vec!["foo", "bar"]);
/// assert!(parse_comma_list("").is_empty());
/// ```
pub fn parse_comma_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Split a comma-separated string into trimmed, lowercased, non-empty parts.
///
/// Same shape as [`parse_comma_list`], but also normalizes case. Used for
/// case-insensitive vocabularies like `skills` where `Rust` and `rust` should
/// collapse to one entry.
///
/// # Examples
///
/// ```text
/// use itr::util::parse_comma_list_lower;
/// assert_eq!(parse_comma_list_lower("Rust,SQL,Go"), vec!["rust", "sql", "go"]);
/// assert_eq!(parse_comma_list_lower(" Rust , , SQL "), vec!["rust", "sql"]);
/// ```
pub fn parse_comma_list_lower(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_lowercase())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Apply add/remove edits to a tag list.
///
/// Tags in `add` that are not already present are appended in input order;
/// tags in `remove` are dropped. Comparison is case-sensitive (use
/// [`apply_skills`] for the case-insensitive variant). Removing a non-existent
/// tag is a no-op rather than an error.
///
/// # Examples
///
/// Add a new tag, skip the dup, drop one we no longer want:
///
/// ```text
/// use itr::util::apply_tags;
/// let result = apply_tags(
///     vec!["rust".into(), "docs".into()],
///     &["score".into(), "rust".into()],
///     &["docs".into()],
/// );
/// assert_eq!(result, vec!["rust", "score"]);
/// ```
///
/// Removing a tag that isn't in the list is a silent no-op:
///
/// ```text
/// use itr::util::apply_tags;
/// let result = apply_tags(vec!["rust".into()], &[], &["missing".into()]);
/// assert_eq!(result, vec!["rust"]);
/// ```
pub fn apply_tags(mut current: Vec<String>, add: &[String], remove: &[String]) -> Vec<String> {
    for t in add {
        if !current.contains(t) {
            current.push(t.clone());
        }
    }
    current.retain(|t| !remove.contains(t));
    current
}

/// Apply add/remove edits to a skill list, normalizing case along the way.
///
/// Skills are trimmed and lowercased before comparison so `Rust`, `rust`, and
/// ` RUST ` all collide on the same entry. Empty / whitespace-only adds are
/// silently dropped (soft fallback — see `docs/soft_fallbacks.md`).
///
/// # Examples
///
/// ```text
/// use itr::util::apply_skills;
/// let result = apply_skills(
///     vec!["rust".into()],
///     &["SQL".into(), "Rust".into(), "  ".into()],
///     &["RUST".into()],
/// );
/// assert_eq!(result, vec!["sql"]);
/// ```
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

/// Parse an ISO 8601 timestamp (`YYYY-MM-DDTHH:MM:SSZ`) and return the
/// fractional number of days between that instant and now.
///
/// Used by the urgency scorer for the age factor. Follows the project's
/// soft-fallback philosophy: an unparseable input returns `0.0` (treated as
/// "no age signal") rather than erroring.
///
/// # Examples
///
/// A historical date is always in the past, so the result is strictly
/// positive:
///
/// ```text
/// use itr::util::days_since;
/// assert!(days_since("2020-01-01T00:00:00Z") > 0.0);
/// ```
///
/// Malformed input degrades to zero days instead of panicking:
///
/// ```text
/// use itr::util::days_since;
/// assert_eq!(days_since("not-a-date"), 0.0);
/// ```
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
        assert_eq!(
            parse_comma_list("foo , bar , baz"),
            vec!["foo", "bar", "baz"]
        );
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
        assert_eq!(
            parse_comma_list_lower("Rust,SQL,Go"),
            vec!["rust", "sql", "go"]
        );
    }

    #[test]
    fn parse_comma_list_lower_trims_and_filters() {
        assert_eq!(
            parse_comma_list_lower(" Rust , , SQL "),
            vec!["rust", "sql"]
        );
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
        let result = apply_tags(vec!["a".into(), "b".into()], &["c".into()], &["a".into()]);
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
        assert!(
            result > 0.0,
            "expected positive days for a past date, got {result}"
        );
    }

    #[test]
    fn days_since_unparseable_returns_zero() {
        assert!(
            days_since("not-a-date").abs() < f64::EPSILON,
            "expected 0.0 for unparseable date"
        );
    }

    // --- Property-based tests ---

    use proptest::prelude::*;

    proptest! {
        // --- parse_comma_list ---

        /// Every produced segment is non-empty (empties are filtered).
        #[test]
        fn prop_parse_comma_list_no_empty_parts(s in ".{0,128}") {
            for part in parse_comma_list(&s) {
                prop_assert!(!part.is_empty());
            }
        }

        /// Every produced segment is trimmed (no leading/trailing whitespace).
        #[test]
        fn prop_parse_comma_list_trimmed(s in ".{0,128}") {
            for part in parse_comma_list(&s) {
                prop_assert_eq!(part.trim(), part.as_str());
            }
        }

        /// Joining the result with `,` and re-parsing is idempotent.
        #[test]
        fn prop_parse_comma_list_roundtrip(parts in prop::collection::vec("[a-zA-Z0-9]{1,8}", 0..8)) {
            let joined = parts.join(",");
            let parsed = parse_comma_list(&joined);
            prop_assert_eq!(parsed, parts);
        }

        /// Inserting extra commas (leading/trailing/duplicated) does not change output.
        #[test]
        fn prop_parse_comma_list_extra_commas_ignored(parts in prop::collection::vec("[a-zA-Z0-9]{1,8}", 1..6)) {
            let clean = parts.join(",");
            let dirty = format!(",,{},,", parts.join(",,"));
            prop_assert_eq!(parse_comma_list(&clean), parse_comma_list(&dirty));
        }

        // --- parse_comma_list_lower ---

        /// Every produced segment is lowercase.
        #[test]
        fn prop_parse_comma_list_lower_is_lowercase(s in ".{0,128}") {
            for part in parse_comma_list_lower(&s) {
                prop_assert_eq!(part.to_lowercase(), part);
            }
        }

        /// Equivalent to lowercasing the input then running parse_comma_list.
        #[test]
        fn prop_parse_comma_list_lower_equiv_lowercase_first(s in ".{0,128}") {
            prop_assert_eq!(
                parse_comma_list_lower(&s),
                parse_comma_list(&s.to_lowercase())
            );
        }

        // --- apply_tags ---

        /// After apply_tags, no tag in `remove` remains in the output.
        #[test]
        fn prop_apply_tags_remove_wins(
            current in prop::collection::vec("[a-z]{1,6}", 0..8),
            add in prop::collection::vec("[a-z]{1,6}", 0..8),
            remove in prop::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let result = apply_tags(current, &add, &remove);
            for r in &remove {
                prop_assert!(!result.contains(r), "removed tag {} still present", r);
            }
        }

        /// Output never contains duplicates (provided the starting `current` is dedup'd).
        #[test]
        fn prop_apply_tags_no_duplicates(
            current in prop::collection::vec("[a-z]{1,6}", 0..8),
            add in prop::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            // Dedupe the input first so the property is well-defined.
            let mut deduped = Vec::new();
            for c in current {
                if !deduped.contains(&c) {
                    deduped.push(c);
                }
            }
            let result = apply_tags(deduped, &add, &[]);
            let mut seen = std::collections::HashSet::new();
            for t in &result {
                prop_assert!(seen.insert(t.clone()), "duplicate tag {} in result", t);
            }
        }

        /// Empty add and empty remove leaves `current` unchanged.
        #[test]
        fn prop_apply_tags_no_ops_identity(
            current in prop::collection::vec("[a-z]{1,6}", 0..8),
        ) {
            let result = apply_tags(current.clone(), &[], &[]);
            prop_assert_eq!(result, current);
        }

        /// Adding a tag that's already present is a no-op (order preserved).
        #[test]
        fn prop_apply_tags_idempotent_add(
            current in prop::collection::vec("[a-z]{1,6}", 1..6),
        ) {
            let first = current[0].clone();
            let result = apply_tags(current.clone(), &[first], &[]);
            prop_assert_eq!(result, current);
        }

        // --- apply_skills ---

        /// Every skill in the result is lowercase and trimmed.
        #[test]
        fn prop_apply_skills_lowercase_trimmed(
            current in prop::collection::vec("[a-z]{1,6}", 0..6),
            add in prop::collection::vec("[ ]?[A-Za-z]{1,6}[ ]?", 0..6),
            remove in prop::collection::vec("[A-Za-z]{1,6}", 0..6),
        ) {
            let result = apply_skills(current, &add, &remove);
            for s in &result {
                prop_assert_eq!(s.to_lowercase(), s.clone());
                prop_assert_eq!(s.trim(), s.as_str());
                prop_assert!(!s.is_empty());
            }
        }

        /// Remove is case-insensitive: removed skill (in any case) is absent.
        #[test]
        fn prop_apply_skills_remove_case_insensitive(
            current in prop::collection::vec("[a-z]{1,6}", 0..6),
            remove in prop::collection::vec("[A-Za-z]{1,6}", 0..6),
        ) {
            let result = apply_skills(current, &[], &remove);
            for r in &remove {
                let lowered = r.trim().to_lowercase();
                prop_assert!(
                    !result.contains(&lowered),
                    "removed skill {} (lowered {}) still present",
                    r, lowered
                );
            }
        }

        /// Whitespace-only adds are dropped (soft-fallback contract).
        #[test]
        fn prop_apply_skills_whitespace_dropped(
            ws in "[ \t]{1,6}",
        ) {
            let result = apply_skills(vec![], &[ws], &[]);
            prop_assert!(result.is_empty());
        }

        /// Adding the same skill in mixed cases collapses to one entry.
        #[test]
        fn prop_apply_skills_idempotent_add(
            skill in "[a-z]{1,6}",
        ) {
            let upper = skill.to_uppercase();
            let result = apply_skills(vec![], &[skill.clone(), upper, skill.clone()], &[]);
            prop_assert_eq!(result, vec![skill]);
        }
    }
}
