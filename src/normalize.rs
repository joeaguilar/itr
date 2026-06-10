/// Map a user-supplied priority string onto one of the four canonical buckets
/// (`critical`, `high`, `medium`, `low`) using case-insensitive synonyms.
///
/// Unknown inputs are returned lowercased and unchanged — `validate_priority`
/// is the gatekeeper that decides whether to reject them; this helper just
/// applies the soft-fallback aliases. See `docs/soft_fallbacks.md`.
///
/// # Examples
///
/// Canonical values pass through untouched (lowercased):
///
/// ```text
/// use itr::normalize::normalize_priority;
/// assert_eq!(normalize_priority("HIGH"), "high");
/// assert_eq!(normalize_priority("medium"), "medium");
/// ```
///
/// Common synonyms collapse to a canonical bucket:
///
/// ```text
/// use itr::normalize::normalize_priority;
/// assert_eq!(normalize_priority("urgent"), "critical");
/// assert_eq!(normalize_priority("P0"), "critical");
/// assert_eq!(normalize_priority("normal"), "medium");
/// assert_eq!(normalize_priority("lowest"), "low");
/// ```
///
/// Unknown inputs survive (lowercased) so the caller can decide how to react:
///
/// ```text
/// use itr::normalize::normalize_priority;
/// assert_eq!(normalize_priority("Bogus"), "bogus");
/// ```
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

/// Map a user-supplied kind string onto one of the four canonical buckets
/// (`bug`, `feature`, `task`, `epic`) using case-insensitive synonyms.
///
/// Unknown inputs are returned lowercased and unchanged so callers can pair
/// this with `validate_kind` for a soft-fallback flow.
///
/// # Examples
///
/// ```text
/// use itr::normalize::normalize_kind;
/// assert_eq!(normalize_kind("Feature"), "feature");
/// assert_eq!(normalize_kind("enhancement"), "feature");
/// assert_eq!(normalize_kind("feat"), "feature");
/// assert_eq!(normalize_kind("bugfix"), "bug");
/// assert_eq!(normalize_kind("chore"), "task");
/// ```
pub fn normalize_kind(k: &str) -> String {
    match k.to_lowercase().as_str() {
        "bug" | "feature" | "task" | "epic" => k.to_lowercase(),
        "enhancement" | "feat" | "story" => "feature".to_string(),
        "bugfix" | "defect" => "bug".to_string(),
        "chore" | "subtask" => "task".to_string(),
        _ => k.to_lowercase(),
    }
}

/// Map a user-supplied status string onto one of the four canonical buckets
/// (`open`, `in-progress`, `done`, `wontfix`) using case-insensitive synonyms.
///
/// The hyphenated form `in-progress` is the canonical wire value; common
/// underscore / no-separator variants (`in_progress`, `inprogress`, `wip`,
/// `started`, `progress`) all collapse onto it.
///
/// # Examples
///
/// ```text
/// use itr::normalize::normalize_status;
/// assert_eq!(normalize_status("TODO"), "open");
/// assert_eq!(normalize_status("backlog"), "open");
/// assert_eq!(normalize_status("wip"), "in-progress");
/// assert_eq!(normalize_status("in_progress"), "in-progress");
/// assert_eq!(normalize_status("resolved"), "done");
/// assert_eq!(normalize_status("cancelled"), "wontfix");
/// ```
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

/// Accept a priority only if it is one of the four canonical values.
///
/// Call after `normalize_priority` so the validator only sees post-aliased
/// input. Returns `ItrError::InvalidValue` (machine-readable code
/// `invalid_value`) on rejection.
///
/// # Examples
///
/// ```text
/// use itr::normalize::{normalize_priority, validate_priority};
/// // Canonical or aliased input passes once normalized.
/// assert!(validate_priority(&normalize_priority("urgent")).is_ok());
/// assert!(validate_priority("low").is_ok());
/// // Raw non-canonical strings are rejected.
/// assert!(validate_priority("bogus").is_err());
/// ```
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

/// Accept a kind only if it is one of the four canonical values.
///
/// Pair with `normalize_kind`. Rejected values produce
/// `ItrError::InvalidValue`.
///
/// # Examples
///
/// ```text
/// use itr::normalize::{normalize_kind, validate_kind};
/// assert!(validate_kind(&normalize_kind("feat")).is_ok());
/// assert!(validate_kind("epic").is_ok());
/// assert!(validate_kind("Feature").is_err()); // raw, mixed case fails — normalize first
/// ```
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

/// Accept a status only if it is one of the four canonical values.
///
/// Pair with `normalize_status`. Rejected values produce
/// `ItrError::InvalidValue`.
///
/// # Examples
///
/// ```text
/// use itr::normalize::{normalize_status, validate_status};
/// assert!(validate_status(&normalize_status("wip")).is_ok());
/// assert!(validate_status("done").is_ok());
/// // Hyphen vs underscore matters at validate time — normalize first.
/// assert!(validate_status("in_progress").is_err());
/// ```
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

/// Normalize a list of user-supplied read-filter values with the same synonym
/// tables as the write paths, returning the normalized values plus a REVIEW
/// note for every value that is still not canonical after normalization.
///
/// Soft fallback: unrecognized values are kept (lowercased) so they simply
/// match nothing, but the caller surfaces the notes on stderr instead of
/// returning a silent empty result (#168).
fn normalize_filter_values(
    values: &[String],
    normalize: fn(&str) -> String,
    validate: fn(&str) -> Result<(), ItrError>,
    field: &str,
    valid: &str,
) -> (Vec<String>, Vec<String>) {
    let mut normalized = Vec::with_capacity(values.len());
    let mut notes = Vec::new();
    for value in values {
        let canon = normalize(value);
        if validate(&canon).is_err() {
            notes.push(format!(
                "REVIEW: {field} filter '{value}' not recognized; it will match nothing. Valid: {valid}"
            ));
        }
        normalized.push(canon);
    }
    (normalized, notes)
}

/// Normalize status read-filter values (`wip` → `in-progress`, `closed` →
/// `done`, ...). Returns `(normalized_values, review_notes)`.
pub fn normalize_status_filters(values: &[String]) -> (Vec<String>, Vec<String>) {
    normalize_filter_values(
        values,
        normalize_status,
        validate_status,
        "status",
        "open, in-progress, done, wontfix",
    )
}

/// Normalize priority read-filter values (`urgent` → `critical`, `p2` →
/// `medium`, ...). Returns `(normalized_values, review_notes)`.
pub fn normalize_priority_filters(values: &[String]) -> (Vec<String>, Vec<String>) {
    normalize_filter_values(
        values,
        normalize_priority,
        validate_priority,
        "priority",
        "critical, high, medium, low",
    )
}

/// Normalize kind read-filter values (`enhancement` → `feature`, `chore` →
/// `task`, ...). Returns `(normalized_values, review_notes)`.
pub fn normalize_kind_filters(values: &[String]) -> (Vec<String>, Vec<String>) {
    normalize_filter_values(
        values,
        normalize_kind,
        validate_kind,
        "kind",
        "bug, feature, task, epic",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // The four canonical values for each field.
    const CANONICAL_PRIORITIES: &[&str] = &["critical", "high", "medium", "low"];
    const CANONICAL_KINDS: &[&str] = &["bug", "feature", "task", "epic"];
    const CANONICAL_STATUSES: &[&str] = &["open", "in-progress", "done", "wontfix"];

    proptest! {
        // --- normalize_priority ---

        /// `normalize_priority` always returns an entirely-lowercase string.
        #[test]
        fn prop_normalize_priority_is_lowercase(s in ".{0,32}") {
            let out = normalize_priority(&s);
            prop_assert_eq!(out.to_lowercase(), out);
        }

        /// Normalization is idempotent: re-running it on its own output is a no-op.
        #[test]
        fn prop_normalize_priority_idempotent(s in ".{0,32}") {
            let once = normalize_priority(&s);
            let twice = normalize_priority(&once);
            prop_assert_eq!(once, twice);
        }

        /// Canonical inputs (any case) round-trip to themselves lowercased.
        #[test]
        fn prop_normalize_priority_canonical_roundtrip(idx in 0usize..CANONICAL_PRIORITIES.len()) {
            let canon = CANONICAL_PRIORITIES[idx];
            prop_assert_eq!(normalize_priority(canon), canon.to_string());
            prop_assert_eq!(normalize_priority(&canon.to_uppercase()), canon.to_string());
        }

        /// Case-insensitivity: same string in different cases normalizes the same.
        #[test]
        fn prop_normalize_priority_case_insensitive(s in "[A-Za-z0-9_]{0,16}") {
            prop_assert_eq!(normalize_priority(&s), normalize_priority(&s.to_uppercase()));
            prop_assert_eq!(normalize_priority(&s), normalize_priority(&s.to_lowercase()));
        }

        /// Known synonyms always normalize into a canonical bucket.
        #[test]
        fn prop_normalize_priority_synonyms_validate(
            syn in prop::sample::select(vec![
                "urgent", "URGENT", "p0", "P0", "highest",
                "p1", "P1",
                "p2", "P2", "normal", "Normal",
                "p3", "P3", "lowest",
            ])
        ) {
            let out = normalize_priority(syn);
            prop_assert!(
                validate_priority(&out).is_ok(),
                "synonym {} normalized to {} which failed validation",
                syn, out
            );
        }

        // --- normalize_kind ---

        #[test]
        fn prop_normalize_kind_is_lowercase(s in ".{0,32}") {
            let out = normalize_kind(&s);
            prop_assert_eq!(out.to_lowercase(), out);
        }

        #[test]
        fn prop_normalize_kind_idempotent(s in ".{0,32}") {
            let once = normalize_kind(&s);
            let twice = normalize_kind(&once);
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn prop_normalize_kind_canonical_roundtrip(idx in 0usize..CANONICAL_KINDS.len()) {
            let canon = CANONICAL_KINDS[idx];
            prop_assert_eq!(normalize_kind(canon), canon.to_string());
            prop_assert_eq!(normalize_kind(&canon.to_uppercase()), canon.to_string());
        }

        #[test]
        fn prop_normalize_kind_case_insensitive(s in "[A-Za-z0-9_]{0,16}") {
            prop_assert_eq!(normalize_kind(&s), normalize_kind(&s.to_uppercase()));
            prop_assert_eq!(normalize_kind(&s), normalize_kind(&s.to_lowercase()));
        }

        #[test]
        fn prop_normalize_kind_synonyms_validate(
            syn in prop::sample::select(vec![
                "enhancement", "Enhancement", "feat", "FEAT", "story",
                "bugfix", "Bugfix", "defect",
                "chore", "CHORE", "subtask",
            ])
        ) {
            let out = normalize_kind(syn);
            prop_assert!(
                validate_kind(&out).is_ok(),
                "synonym {} normalized to {} which failed validation",
                syn, out
            );
        }

        // --- normalize_status ---

        #[test]
        fn prop_normalize_status_is_lowercase(s in ".{0,32}") {
            let out = normalize_status(&s);
            prop_assert_eq!(out.to_lowercase(), out);
        }

        #[test]
        fn prop_normalize_status_idempotent(s in ".{0,32}") {
            let once = normalize_status(&s);
            let twice = normalize_status(&once);
            prop_assert_eq!(once, twice);
        }

        #[test]
        fn prop_normalize_status_canonical_roundtrip(idx in 0usize..CANONICAL_STATUSES.len()) {
            let canon = CANONICAL_STATUSES[idx];
            prop_assert_eq!(normalize_status(canon), canon.to_string());
            prop_assert_eq!(normalize_status(&canon.to_uppercase()), canon.to_string());
        }

        #[test]
        fn prop_normalize_status_case_insensitive(s in "[A-Za-z0-9_-]{0,16}") {
            prop_assert_eq!(normalize_status(&s), normalize_status(&s.to_uppercase()));
            prop_assert_eq!(normalize_status(&s), normalize_status(&s.to_lowercase()));
        }

        #[test]
        fn prop_normalize_status_synonyms_validate(
            syn in prop::sample::select(vec![
                "todo", "TODO", "new", "backlog",
                "closed", "CLOSED", "resolved", "fixed",
                "cancelled", "canceled", "Cancelled",
                "wip", "WIP", "started", "progress", "in_progress", "inprogress",
            ])
        ) {
            let out = normalize_status(syn);
            prop_assert!(
                validate_status(&out).is_ok(),
                "synonym {} normalized to {} which failed validation",
                syn, out
            );
        }

        /// Unknown inputs survive as their own lowercased self (soft-fallback contract).
        #[test]
        fn prop_normalize_unknown_passes_through(
            // Restrict to characters that are clearly outside any known synonym.
            s in "[!@#$%^&*()0-9]{1,16}"
        ) {
            let lower = s.to_lowercase();
            prop_assert_eq!(normalize_priority(&s), lower.clone());
            prop_assert_eq!(normalize_kind(&s), lower.clone());
            prop_assert_eq!(normalize_status(&s), lower);
        }
    }

    // --- #168: read-filter normalization helpers ---

    #[test]
    fn status_filters_normalize_synonyms_without_notes() {
        let (values, notes) = normalize_status_filters(&[
            "wip".to_string(),
            "closed".to_string(),
            "OPEN".to_string(),
        ]);
        assert_eq!(values, vec!["in-progress", "done", "open"]);
        assert!(notes.is_empty(), "recognized synonyms must not warn");
    }

    #[test]
    fn priority_and_kind_filters_normalize_synonyms_without_notes() {
        let (values, notes) = normalize_priority_filters(&["urgent".to_string()]);
        assert_eq!(values, vec!["critical"]);
        assert!(notes.is_empty());

        let (values, notes) = normalize_kind_filters(&["enhancement".to_string()]);
        assert_eq!(values, vec!["feature"]);
        assert!(notes.is_empty());
    }

    #[test]
    fn unrecognized_filter_values_keep_value_and_emit_review_note() {
        let (values, notes) = normalize_status_filters(&["bogus".to_string()]);
        assert_eq!(values, vec!["bogus"], "unknown values pass through");
        assert_eq!(notes.len(), 1);
        assert!(notes[0].starts_with("REVIEW: status filter 'bogus'"));
        assert!(notes[0].contains("open, in-progress, done, wontfix"));

        let (_, notes) = normalize_priority_filters(&["bogus".to_string()]);
        assert!(notes[0].contains("critical, high, medium, low"));

        let (_, notes) = normalize_kind_filters(&["bogus".to_string()]);
        assert!(notes[0].contains("bug, feature, task, epic"));
    }
}
