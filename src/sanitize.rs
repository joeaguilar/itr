//! Shared cleaning for every value that is written into the database.
//!
//! `itr` has several write paths (`add`, `add --stdin-json`, `batch`, `bulk`,
//! `update`, `import`, and the UI JSON API). Each used to clean input its own
//! way, so the same logical value could be stored as `" A "`, `"A"`, or
//! `["A","A",""]` depending on the path. The helpers here are the single
//! definition of "clean", and `db::insert_issue` / `db::update_issue_field` /
//! `db::add_note` / `db::update_note` apply them as the last line of defense,
//! so no path can store a value the others would have rejected.
//!
//! Callers that want to explain a change (a `REVIEW:` note) compare their
//! input against the helper's output; the helpers themselves never print.

use chrono::{DateTime, NaiveDateTime, SecondsFormat, Utc};

/// Canonical timestamp layout for every `*_at` column (UTC ISO 8601).
pub const TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// Largest issue ID accepted from external input (import). Matches
/// JavaScript's `Number.MAX_SAFE_INTEGER`, so the UI can address every
/// issue, and keeps `AUTOINCREMENT` far from `i64::MAX` (reaching it makes
/// every later insert fail with `SQLITE_FULL`).
pub const MAX_ISSUE_ID: i64 = 9_007_199_254_740_991;

/// True for characters that must never be stored: C0 controls (NUL, ESC,
/// BEL, ...), DEL, and C1 controls. Tab, newline, and carriage return are
/// handled separately by the callers because multi-line fields keep them.
fn is_forbidden_control(c: char) -> bool {
    c.is_control() && !matches!(c, '\t' | '\n' | '\r')
}

/// True if `s` contains a character [`clean_title`] or [`clean_text`] would
/// remove or rewrite.
pub fn has_control_chars(s: &str) -> bool {
    s.chars().any(is_forbidden_control)
}

/// Clean a single-line value (title, list element, assignee): forbidden
/// control characters are removed, tabs and line breaks become spaces, and
/// surrounding whitespace is trimmed. Single-line values are rendered on one
/// line in compact / oneline / pretty output, so an embedded newline or an
/// ANSI escape could otherwise forge a record or repaint the terminal.
pub fn clean_line(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .filter(|c| !is_forbidden_control(*c))
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect();
    mapped.trim().to_string()
}

/// Clean an issue title. Same rules as [`clean_line`]; an empty result means
/// the caller must fall back or reject (a title is required).
pub fn clean_title(raw: &str) -> String {
    clean_line(raw)
}

/// Clean a multi-line free-text value (context, acceptance, note content,
/// close reason). Forbidden control characters are removed, `\r\n` / `\r`
/// become `\n`, and trailing whitespace is trimmed. Leading whitespace and
/// inner layout are kept because they can be meaningful (indented code).
pub fn clean_text(raw: &str) -> String {
    let unified = raw.replace("\r\n", "\n").replace('\r', "\n");
    let kept: String = unified
        .chars()
        .filter(|c| !is_forbidden_control(*c))
        .collect();
    kept.trim_end().to_string()
}

/// Clean an assignee name: single line, trimmed. Empty means unassigned.
pub fn clean_assignee(raw: &str) -> String {
    clean_line(raw)
}

/// Clean a list column (`files`, `tags`): every element goes through
/// [`clean_line`], empty elements are dropped, and duplicates are removed
/// keeping the first occurrence. Comparison is case-sensitive.
pub fn clean_list(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(values.len());
    for value in values {
        let cleaned = clean_line(value);
        if !cleaned.is_empty() && !out.contains(&cleaned) {
            out.push(cleaned);
        }
    }
    out
}

/// Clean the `skills` list: like [`clean_list`] but lowercased first, so
/// `Rust`, `rust`, and ` RUST ` collapse to one `rust` entry (skills are a
/// case-insensitive vocabulary and filters compare lowercase).
pub fn clean_skills(values: &[String]) -> Vec<String> {
    let lowered: Vec<String> = values.iter().map(|v| v.to_lowercase()).collect();
    clean_list(&lowered)
}

/// Normalize a timestamp to [`TIMESTAMP_FORMAT`] in UTC.
///
/// Accepts the canonical form, any RFC 3339 timestamp (offsets are converted
/// to UTC; fractional seconds are dropped), and the space-separated
/// `YYYY-MM-DD HH:MM:SS` form `SQLite` itself produces (read as UTC). Returns
/// `None` for anything else so the caller can fall back and explain.
pub fn normalize_timestamp(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if let Ok(naive) = NaiveDateTime::parse_from_str(raw, TIMESTAMP_FORMAT) {
        return Some(naive.format(TIMESTAMP_FORMAT).to_string());
    }
    if let Ok(parsed) = DateTime::parse_from_rfc3339(raw) {
        let utc = parsed.with_timezone(&Utc);
        return Some(
            utc.to_rfc3339_opts(SecondsFormat::Secs, true)
                .replace("+00:00", "Z"),
        );
    }
    for layout in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, layout) {
            return Some(naive.format(TIMESTAMP_FORMAT).to_string());
        }
    }
    None
}

/// The current instant in [`TIMESTAMP_FORMAT`].
pub fn now_timestamp() -> String {
    Utc::now().format(TIMESTAMP_FORMAT).to_string()
}

/// Map a relation type onto `duplicate` / `related` / `supersedes`,
/// accepting case and common synonyms. Unknown input is returned lowercased
/// so the caller can decide (the same contract as `normalize_priority`).
pub fn normalize_relation_type(raw: &str) -> String {
    let lowered = raw.trim().to_lowercase();
    match lowered.as_str() {
        "dup" | "dupe" | "duplicates" | "duplicate-of" | "duplicate_of" => "duplicate".to_string(),
        "relates" | "relates-to" | "relates_to" | "relation" | "related-to" | "related_to" => {
            "related".to_string()
        }
        "supersede" | "replaces" | "obsoletes" => "supersedes".to_string(),
        _ => lowered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    #[test]
    fn clean_title_trims_and_strips_controls() {
        assert_eq!(clean_title("  padded  "), "padded");
        assert_eq!(clean_title("a\u{0}b\u{1b}[31mred\u{7}"), "ab[31mred");
        assert_eq!(
            clean_title("line one\nline two\ttab"),
            "line one line two tab"
        );
        assert_eq!(clean_title("   "), "");
        assert_eq!(clean_title("\u{85}next-line\u{9f}"), "next-line");
        assert_eq!(clean_title("naïve – ünïcode 日本"), "naïve – ünïcode 日本");
    }

    #[test]
    fn clean_text_keeps_layout_but_strips_controls() {
        assert_eq!(
            clean_text("  indented\r\ncode\u{0}\n\n"),
            "  indented\ncode"
        );
        assert_eq!(clean_text("a\rb"), "a\nb");
        assert_eq!(clean_text("tab\there"), "tab\there");
        assert_eq!(clean_text("esc\u{1b}[0m"), "esc[0m");
    }

    #[test]
    fn clean_list_trims_drops_empties_and_dedupes() {
        assert_eq!(
            clean_list(&strings(&[" A ", "A", "", "  ", "a", "x\ny"])),
            strings(&["A", "a", "x y"])
        );
    }

    #[test]
    fn clean_skills_lowercases_before_dedupe() {
        assert_eq!(
            clean_skills(&strings(&["Rust", " rust ", "RUST", "Go", ""])),
            strings(&["rust", "go"])
        );
    }

    #[test]
    fn normalize_timestamp_accepts_canonical_rfc3339_and_sqlite_forms() {
        assert_eq!(
            normalize_timestamp("2026-01-02T03:04:05Z").as_deref(),
            Some("2026-01-02T03:04:05Z")
        );
        assert_eq!(
            normalize_timestamp("2026-01-02T03:04:05+05:00").as_deref(),
            Some("2026-01-01T22:04:05Z")
        );
        assert_eq!(
            normalize_timestamp("2026-01-02T03:04:05.999Z").as_deref(),
            Some("2026-01-02T03:04:05Z")
        );
        assert_eq!(
            normalize_timestamp("2026-01-02 03:04:05").as_deref(),
            Some("2026-01-02T03:04:05Z")
        );
        assert_eq!(normalize_timestamp("garbage"), None);
        assert_eq!(normalize_timestamp(""), None);
        assert_eq!(normalize_timestamp("zzzz"), None);
    }

    #[test]
    fn normalize_relation_type_maps_synonyms() {
        assert_eq!(normalize_relation_type(" Duplicate "), "duplicate");
        assert_eq!(normalize_relation_type("dupe"), "duplicate");
        assert_eq!(normalize_relation_type("relates-to"), "related");
        assert_eq!(normalize_relation_type("Supersedes"), "supersedes");
        assert_eq!(normalize_relation_type("blocks"), "blocks");
    }
}
