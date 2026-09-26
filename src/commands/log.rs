use crate::db;
use crate::error::{self, ItrError};
use crate::format::{self, Format};
use crate::models::Event;
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use rusqlite::Connection;

/// The canonical form of every stored `created_at` (UTC, second resolution).
/// `--since` is compared as TEXT against it, so it must be re-emitted in
/// exactly this shape before binding.
const CANONICAL_TS: &str = "%Y-%m-%dT%H:%M:%SZ";

/// Parse a `--since` value into the canonical stored timestamp form
/// (`%Y-%m-%dT%H:%M:%SZ`, UTC). Returns `None` when unparseable (SQ-7).
///
/// Accepted forms:
/// - RFC 3339 with any offset (`2026-01-05T12:00:00Z`, `...+05:00`), also
///   with a space instead of `T`;
/// - naive date-times, taken as UTC (`2026-01-05T12:00:00`,
///   `2026-01-05 12:00:00`, `2026-01-05T12:00`, optional trailing `Z`,
///   optional fractional seconds);
/// - a bare date (`2026-01-05`), meaning midnight UTC;
/// - relative ages `<N>s|m|h|d|w` (e.g. `24h`, `7d`), plus `now`, `today`
///   and `yesterday` (midnight UTC).
pub(crate) fn parse_since(value: &str, now: DateTime<Utc>) -> Option<String> {
    let v = value.trim();
    if v.is_empty() {
        return None;
    }
    let lower = v.to_ascii_lowercase();
    let midnight = |d: NaiveDate| d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
    let resolved: Option<DateTime<Utc>> = match lower.as_str() {
        "now" => Some(now),
        "today" => midnight(now.date_naive()),
        "yesterday" => midnight(now.date_naive() - Duration::days(1)),
        _ => parse_relative(&lower, now)
            .or_else(|| parse_absolute(v))
            .or_else(|| {
                NaiveDate::parse_from_str(v, "%Y-%m-%d")
                    .ok()
                    .and_then(midnight)
            }),
    };
    resolved.map(|dt| dt.format(CANONICAL_TS).to_string())
}

/// `<N><unit>` with unit in s/m/h/d/w, measured back from `now`.
fn parse_relative(v: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let unit = v.chars().last()?;
    let amount: i64 = v[..v.len() - unit.len_utf8()].parse().ok()?;
    if amount < 0 {
        return None;
    }
    let span = match unit {
        's' => Duration::try_seconds(amount)?,
        'm' => Duration::try_minutes(amount)?,
        'h' => Duration::try_hours(amount)?,
        'd' => Duration::try_days(amount)?,
        'w' => Duration::try_weeks(amount)?,
        _ => return None,
    };
    now.checked_sub_signed(span)
}

/// RFC 3339 (any offset, `T` or space separator) or a naive date-time
/// taken as UTC.
fn parse_absolute(v: &str) -> Option<DateTime<Utc>> {
    let t_form = v.replacen(' ', "T", 1);
    if let Ok(dt) = DateTime::parse_from_rfc3339(&t_form) {
        return Some(dt.with_timezone(&Utc));
    }
    let naive = t_form.strip_suffix(['Z', 'z']).unwrap_or(&t_form);
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(naive, f).ok())
        .map(|dt| dt.and_utc())
}

pub fn run(
    conn: &Connection,
    id: Option<i64>,
    limit: usize,
    since: Option<String>,
    agent: Option<String>,
    fmt: Format,
) -> Result<(), ItrError> {
    let events = run_core(conn, id, limit, since.as_deref(), agent.as_deref())?;

    if events.is_empty() {
        error::print_empty(fmt.is_json(), "No events found.");
        return Ok(());
    }

    println!("{}", format::format_events(&events, fmt));
    Ok(())
}

/// Resolve the filtered event list for `itr log`.
///
/// Every filter (issue scope, `--since`, `--agent`) is applied in SQL before
/// the limit (#170), so the newest matching events are returned even when
/// they are interleaved with newer events from other agents. Per-issue logs
/// stay chronological (oldest first); the global log is newest first.
pub(crate) fn run_core(
    conn: &Connection,
    id: Option<i64>,
    limit: usize,
    since: Option<&str>,
    agent: Option<&str>,
) -> Result<Vec<Event>, ItrError> {
    if let Some(issue_id) = id {
        // Verify issue exists
        let _issue = db::get_issue(conn, issue_id)?;
    }

    // Normalize --since to the stored canonical form before the TEXT
    // comparison (SQ-7). An unparseable value is reported and ignored
    // (soft fallback) instead of silently misfiltering or returning [].
    let since = match since {
        Some(raw) => match parse_since(raw, Utc::now()) {
            Some(ts) => Some(ts),
            None => {
                eprintln!(
                    "REVIEW: --since '{raw}' not recognized as a timestamp; filter ignored. Use RFC 3339 (2026-01-05T12:00:00Z), YYYY-MM-DD, or a relative age like 24h / 7d"
                );
                None
            }
        },
        None => None,
    };

    let mut events = db::get_events_filtered(conn, id, limit, since.as_deref(), agent)?;
    if id.is_some() {
        // get_events_filtered returns newest-first; per-issue history reads
        // top-to-bottom in chronological order.
        events.reverse();
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    fn seed_issue(conn: &Connection) -> i64 {
        db::insert_issue(
            conn,
            "log target",
            "medium",
            "task",
            "",
            &[],
            &[],
            &[],
            "",
            None,
            "",
        )
        .unwrap()
        .id
    }

    fn insert_event_at(conn: &Connection, issue_id: i64, agent: &str, created_at: &str) {
        conn.execute(
            "INSERT INTO events (issue_id, field, old_value, new_value, agent, created_at)
             VALUES (?1, 'status', 'open', 'in-progress', ?2, ?3)",
            params![issue_id, agent, created_at],
        )
        .unwrap();
    }

    // #170 defect 1: --since was silently ignored when an issue ID was given.
    #[test]
    fn since_filters_issue_scoped_log() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-03-01T00:00:00Z");

        let events = run_core(&conn, Some(id), 50, Some("2026-02-01T00:00:00Z"), None).unwrap();
        assert_eq!(events.len(), 1, "--since must apply to `itr log <id>`");
        assert_eq!(events[0].created_at, "2026-03-01T00:00:00Z");

        // A future --since is an empty (non-error) result.
        let events = run_core(&conn, Some(id), 50, Some("2099-01-01T00:00:00Z"), None).unwrap();
        assert!(events.is_empty());
    }

    // #170 defect 2: --agent filtered in memory after LIMIT, so matches
    // older than the N newest events overall were silently dropped.
    #[test]
    fn agent_filter_applies_before_limit_in_global_log() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-02T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-03T00:00:00Z");
        insert_event_at(&conn, id, "bob", "2026-01-04T00:00:00Z");

        let events = run_core(&conn, None, 3, None, Some("alice")).unwrap();
        assert_eq!(
            events.len(),
            1,
            "alice's event lies beyond the 3 newest overall but must be found"
        );
        assert_eq!(events[0].agent, "alice");
    }

    // Per-issue logs stay chronological and keep the newest N when limited.
    #[test]
    fn issue_log_is_chronological_and_keeps_newest_when_limited() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-02T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-03T00:00:00Z");

        let events = run_core(&conn, Some(id), 2, None, None).unwrap();
        let stamps: Vec<&str> = events.iter().map(|e| e.created_at.as_str()).collect();
        assert_eq!(stamps, vec!["2026-01-02T00:00:00Z", "2026-01-03T00:00:00Z"]);
    }

    // The global log stays newest-first.
    #[test]
    fn global_log_is_newest_first() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-01T00:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-02T00:00:00Z");

        let events = run_core(&conn, None, 50, None, None).unwrap();
        let stamps: Vec<&str> = events.iter().map(|e| e.created_at.as_str()).collect();
        assert_eq!(stamps, vec!["2026-01-02T00:00:00Z", "2026-01-01T00:00:00Z"]);
    }

    // SQ-7: --since is normalized to the canonical stored form.
    #[test]
    fn parse_since_accepts_absolute_forms() {
        let now = Utc::now();
        let cases = [
            ("2026-01-05T12:00:00Z", "2026-01-05T12:00:00Z"),
            ("2026-01-05T12:00:00+05:00", "2026-01-05T07:00:00Z"),
            ("2026-01-05T12:00:00.750Z", "2026-01-05T12:00:00Z"),
            ("2026-01-05 12:00:00", "2026-01-05T12:00:00Z"),
            ("2026-01-05 12:00:00-02:00", "2026-01-05T14:00:00Z"),
            ("2026-01-05T12:00:00", "2026-01-05T12:00:00Z"),
            ("2026-01-05T12:00", "2026-01-05T12:00:00Z"),
            ("2026-01-05", "2026-01-05T00:00:00Z"),
            ("  2026-01-05  ", "2026-01-05T00:00:00Z"),
        ];
        for (input, want) in cases {
            assert_eq!(
                parse_since(input, now).as_deref(),
                Some(want),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn parse_since_accepts_relative_forms() {
        let now = DateTime::parse_from_rfc3339("2026-01-05T12:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let cases = [
            ("24h", "2026-01-04T12:30:00Z"),
            ("7d", "2025-12-29T12:30:00Z"),
            ("2w", "2025-12-22T12:30:00Z"),
            ("30m", "2026-01-05T12:00:00Z"),
            ("45s", "2026-01-05T12:29:15Z"),
            ("now", "2026-01-05T12:30:00Z"),
            ("today", "2026-01-05T00:00:00Z"),
            ("Yesterday", "2026-01-04T00:00:00Z"),
        ];
        for (input, want) in cases {
            assert_eq!(
                parse_since(input, now).as_deref(),
                Some(want),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn parse_since_rejects_garbage() {
        let now = Utc::now();
        for input in ["", "   ", "soon", "7x", "-3d", "2026-13-45", "d"] {
            assert_eq!(parse_since(input, now), None, "input {input:?}");
        }
    }

    // SQ-7 end to end: a space-separated or offset --since filters on the
    // real instant instead of a raw string comparison.
    #[test]
    fn since_is_compared_as_an_instant() {
        let conn = db::open_test_db();
        let id = seed_issue(&conn);
        insert_event_at(&conn, id, "alice", "2026-01-05T08:00:00Z");
        insert_event_at(&conn, id, "alice", "2026-01-05T20:00:00Z");

        let stamps = |since: &str| -> Vec<String> {
            run_core(&conn, None, 50, Some(since), None)
                .unwrap()
                .into_iter()
                .map(|e| e.created_at)
                .collect()
        };
        assert_eq!(stamps("2026-01-05 12:00:00"), vec!["2026-01-05T20:00:00Z"]);
        assert_eq!(
            stamps("2026-01-05T12:00:00+05:00"),
            vec!["2026-01-05T20:00:00Z", "2026-01-05T08:00:00Z"],
            "12:00+05:00 is 07:00Z, so the 08:00Z event is included"
        );
        // Unparseable: REVIEW note + filter ignored, never a silent [].
        assert_eq!(stamps("soon").len(), 2);
    }

    // Unknown issue IDs still surface NOT_FOUND.
    #[test]
    fn unknown_issue_is_not_found() {
        let conn = db::open_test_db();
        assert!(matches!(
            run_core(&conn, Some(999), 50, None, None),
            Err(ItrError::NotFound(999))
        ));
    }
}
