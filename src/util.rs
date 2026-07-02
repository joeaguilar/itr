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

/// Largest span an `A-B` range token may expand to. A typo like `1-999999`
/// should soft-fail with a REVIEW note instead of allocating a million IDs.
const MAX_RANGE_SPAN: i64 = 1000;

/// Outcome of parsing positional issue-ID arguments shared by `get`/`show`
/// and the multi-ID mutating verbs (`close`, `note`, `relate`, `depend`).
///
/// IDs may be repeated arguments, comma-separated lists, inclusive `A-B`
/// ranges, or a mix (`itr get 1 2,3 5-8`). Parsing is a pure function so the
/// soft-fallback reporting (REVIEW notes for duplicates, non-integer tokens,
/// and malformed ranges) stays in the command handlers and the splitting
/// logic is unit-testable.
#[derive(Debug, Default)]
pub struct ParsedIds {
    /// Unique IDs in first-seen request order.
    pub ids: Vec<i64>,
    /// Explicitly repeated IDs (unique, first-seen order). IDs covered more
    /// than once via overlapping ranges are deduplicated silently.
    pub duplicates: Vec<i64>,
    /// Tokens that did not parse as an integer or an `A-B` range.
    pub invalid: Vec<String>,
    /// Soft-fallback REVIEW messages about recovered range tokens
    /// (reversed bounds), ready to print to stderr.
    pub notes: Vec<String>,
}

/// Parse one `A-B` token into inclusive integer bounds. Returns `None` when
/// either side is not a plain non-negative integer (so `-5` or `x-3` fall
/// through to the invalid-token path).
fn parse_range_token(token: &str) -> Option<(i64, i64)> {
    let (a, b) = token.split_once('-')?;
    let a = a.trim().parse::<i64>().ok()?;
    let b = b.trim().parse::<i64>().ok()?;
    Some((a, b))
}

/// Parse positional ID arguments: repeated args, comma-separated lists, and
/// inclusive `A-B` ranges, in any mix. Duplicated single IDs are recorded in
/// `duplicates`; range-expanded IDs deduplicate silently. A reversed range
/// (`9-5`) is recovered by swapping the bounds with a REVIEW note; a range
/// wider than [`MAX_RANGE_SPAN`] is rejected as invalid.
///
/// # Examples
///
/// ```text
/// use itr::util::parse_id_tokens;
/// let parsed = parse_id_tokens(&["1,2".into(), "5-7".into()]);
/// assert_eq!(parsed.ids, vec![1, 2, 5, 6, 7]);
/// ```
pub fn parse_id_tokens(args: &[String]) -> ParsedIds {
    let mut parsed = ParsedIds::default();
    let push_id = |parsed: &mut ParsedIds, id: i64, from_range: bool| {
        if parsed.ids.contains(&id) {
            if !from_range && !parsed.duplicates.contains(&id) {
                parsed.duplicates.push(id);
            }
        } else {
            parsed.ids.push(id);
        }
    };
    for arg in args {
        for token in arg.split(',') {
            let token = token.trim();
            if token.is_empty() {
                continue;
            }
            if let Ok(id) = token.parse::<i64>() {
                push_id(&mut parsed, id, false);
                continue;
            }
            if let Some((a, b)) = parse_range_token(token) {
                let (lo, hi) = if a <= b {
                    (a, b)
                } else {
                    parsed.notes.push(format!(
                        "REVIEW: range '{}' is reversed; interpreting as {}-{}",
                        token, b, a
                    ));
                    (b, a)
                };
                if hi - lo >= MAX_RANGE_SPAN {
                    parsed.notes.push(format!(
                        "REVIEW: range '{}' spans more than {} IDs; skipped — narrow the range",
                        token, MAX_RANGE_SPAN
                    ));
                    parsed.invalid.push(token.to_string());
                    continue;
                }
                for id in lo..=hi {
                    push_id(&mut parsed, id, true);
                }
                continue;
            }
            parsed.invalid.push(token.to_string());
        }
    }
    parsed
}

/// Returns true when `token` is ID-shaped: a plain integer, an `A-B` range,
/// or a comma-separated list of those. Used to split the leading ID list from
/// trailing free text in `close`/`note` positional arguments.
pub fn is_id_token(token: &str) -> bool {
    let mut saw_piece = false;
    for piece in token.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if piece.parse::<i64>().is_err() && parse_range_token(piece).is_none() {
            return false;
        }
        saw_piece = true;
    }
    saw_piece
}

/// Split positional arguments into a leading run of ID-shaped tokens and the
/// trailing free text (close reason, note body). The first non-ID token
/// starts the text; any following tokens are joined with single spaces.
///
/// A numeric-only text argument after the IDs is indistinguishable from an
/// ID and will be consumed as one — callers document `--reason`/quoting as
/// the unambiguous alternative.
///
/// # Examples
///
/// ```text
/// use itr::util::split_ids_and_text;
/// let (ids, text) = split_ids_and_text(&["12,14".into(), "fixed".into()]);
/// assert_eq!(ids, vec!["12,14".to_string()]);
/// assert_eq!(text.as_deref(), Some("fixed"));
/// ```
pub fn split_ids_and_text(args: &[String]) -> (Vec<String>, Option<String>) {
    let split_at = args
        .iter()
        .position(|a| !is_id_token(a))
        .unwrap_or(args.len());
    let ids = args[..split_at].to_vec();
    let text = if split_at < args.len() {
        Some(args[split_at..].join(" "))
    } else {
        None
    };
    (ids, text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_id_tokens / split_ids_and_text (multi-ID verbs) ---

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn parse_id_tokens_accepts_comma_and_repeated_forms() {
        let parsed = parse_id_tokens(&args(&["1,2", "3", "4,5"]));
        assert_eq!(parsed.ids, vec![1, 2, 3, 4, 5]);
        assert!(parsed.duplicates.is_empty());
        assert!(parsed.invalid.is_empty());
        assert!(parsed.notes.is_empty());
    }

    #[test]
    fn parse_id_tokens_expands_inclusive_ranges() {
        let parsed = parse_id_tokens(&args(&["124-128", "130"]));
        assert_eq!(parsed.ids, vec![124, 125, 126, 127, 128, 130]);
        assert!(parsed.invalid.is_empty());
    }

    #[test]
    fn parse_id_tokens_range_inside_comma_list() {
        let parsed = parse_id_tokens(&args(&["1,5-7,9"]));
        assert_eq!(parsed.ids, vec![1, 5, 6, 7, 9]);
    }

    #[test]
    fn parse_id_tokens_single_id_range_is_one_id() {
        let parsed = parse_id_tokens(&args(&["4-4"]));
        assert_eq!(parsed.ids, vec![4]);
    }

    #[test]
    fn parse_id_tokens_reversed_range_recovers_with_note() {
        let parsed = parse_id_tokens(&args(&["9-5"]));
        assert_eq!(parsed.ids, vec![5, 6, 7, 8, 9]);
        assert_eq!(parsed.notes.len(), 1);
        assert!(parsed.notes[0].contains("reversed"));
    }

    #[test]
    fn parse_id_tokens_oversized_range_is_invalid_with_note() {
        let parsed = parse_id_tokens(&args(&["1-999999"]));
        assert!(parsed.ids.is_empty());
        assert_eq!(parsed.invalid, vec!["1-999999".to_string()]);
        assert!(parsed.notes[0].contains("spans more than"));
    }

    #[test]
    fn parse_id_tokens_overlapping_ranges_dedupe_silently() {
        let parsed = parse_id_tokens(&args(&["1-3", "2-4"]));
        assert_eq!(parsed.ids, vec![1, 2, 3, 4]);
        assert!(
            parsed.duplicates.is_empty(),
            "range overlap must not spam duplicate notes"
        );
    }

    #[test]
    fn parse_id_tokens_explicit_duplicates_still_reported() {
        let parsed = parse_id_tokens(&args(&["1,1,2", "1", "2"]));
        assert_eq!(parsed.ids, vec![1, 2]);
        assert_eq!(parsed.duplicates, vec![1, 2]);
    }

    #[test]
    fn parse_id_tokens_collects_invalid_tokens_and_keeps_valid_ones() {
        let parsed = parse_id_tokens(&args(&["1,abc", "x-3", "2"]));
        assert_eq!(parsed.ids, vec![1, 2]);
        assert_eq!(parsed.invalid, vec!["abc".to_string(), "x-3".to_string()]);
    }

    #[test]
    fn parse_id_tokens_skips_empty_tokens() {
        let parsed = parse_id_tokens(&args(&["1,,2,", " 3 "]));
        assert_eq!(parsed.ids, vec![1, 2, 3]);
        assert!(parsed.invalid.is_empty());
    }

    #[test]
    fn is_id_token_variants() {
        assert!(is_id_token("12"));
        assert!(is_id_token("12,14"));
        assert!(is_id_token("5-8"));
        assert!(is_id_token("1,5-8,9"));
        assert!(!is_id_token("fixed"));
        assert!(!is_id_token("42 things"));
        assert!(!is_id_token(""));
        assert!(!is_id_token(","));
        // A bare negative integer parses as an i64, so it counts as ID-shaped;
        // it can never match an issue and soft-falls with a REVIEW note.
        assert!(is_id_token("-5"));
    }

    #[test]
    fn split_ids_and_text_basic() {
        let (ids, text) = split_ids_and_text(&args(&["12,14", "17", "fixed in a1b2c3d"]));
        assert_eq!(ids, args(&["12,14", "17"]));
        assert_eq!(text.as_deref(), Some("fixed in a1b2c3d"));
    }

    #[test]
    fn split_ids_and_text_no_text() {
        let (ids, text) = split_ids_and_text(&args(&["12", "14"]));
        assert_eq!(ids, args(&["12", "14"]));
        assert!(text.is_none());
    }

    #[test]
    fn split_ids_and_text_joins_trailing_tokens() {
        let (ids, text) = split_ids_and_text(&args(&["5", "verified", "end-to-end"]));
        assert_eq!(ids, args(&["5"]));
        assert_eq!(text.as_deref(), Some("verified end-to-end"));
    }

    #[test]
    fn split_ids_and_text_numeric_token_after_text_stays_text() {
        let (ids, text) = split_ids_and_text(&args(&["5", "wave", "2", "verified"]));
        assert_eq!(ids, args(&["5"]));
        assert_eq!(text.as_deref(), Some("wave 2 verified"));
    }

    #[test]
    fn split_ids_and_text_all_text() {
        let (ids, text) = split_ids_and_text(&args(&["not-an-id"]));
        assert!(ids.is_empty());
        assert_eq!(text.as_deref(), Some("not-an-id"));
    }

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

// Tests for the version-shaping logic that build.rs bakes into ITR_VERSION.
// The function lives in src/version_shape.rs and is include!d both here and
// in build.rs, so these tests cover the exact code the build script runs.
// Test-only: in normal builds the binary never calls shape_version itself.
#[cfg(test)]
mod version_shape_tests {
    include!("version_shape.rs");

    #[test]
    fn tag_describe_passes_through() {
        assert_eq!(shape_version(Some("v2.10.0"), "0.1.0"), "v2.10.0");
    }

    #[test]
    fn tag_describe_with_ahead_count_passes_through() {
        assert_eq!(
            shape_version(Some("v2.10.0-4-gf40ddd4"), "0.1.0"),
            "v2.10.0-4-gf40ddd4"
        );
        assert_eq!(
            shape_version(Some("v2.10.0-4-gf40ddd4-dirty"), "0.1.0"),
            "v2.10.0-4-gf40ddd4-dirty"
        );
    }

    // The CI regression (tagless shallow checkout): `git describe
    // --tags --always --dirty` emits a bare hash, which must not become the
    // whole version string — it has to stay semver-shaped.
    #[test]
    fn bare_hash_falls_back_to_pkg_version_plus_hash() {
        assert_eq!(shape_version(Some("f40ddd4"), "0.1.0"), "0.1.0+f40ddd4");
    }

    #[test]
    fn bare_hash_dirty_keeps_dirty_marker_in_metadata() {
        assert_eq!(
            shape_version(Some("f40ddd4-dirty"), "0.1.0"),
            "0.1.0+f40ddd4-dirty"
        );
    }

    #[test]
    fn full_length_hash_is_still_a_hash() {
        let full = "f40ddd4aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        assert_eq!(full.len(), 40);
        assert_eq!(shape_version(Some(full), "0.1.0"), format!("0.1.0+{full}"));
    }

    #[test]
    fn no_describe_output_falls_back_to_pkg_version() {
        assert_eq!(shape_version(None, "0.1.0"), "0.1.0");
    }

    #[test]
    fn short_hex_lookalike_tag_is_not_treated_as_hash() {
        // 7+ hex chars is a hash; anything shorter (e.g. a hypothetical
        // `abc123` tag) passes through.
        assert_eq!(shape_version(Some("abc123"), "0.1.0"), "abc123");
    }
}
