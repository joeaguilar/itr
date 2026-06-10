use crate::db;
use crate::models::{Issue, UrgencyBreakdown};
use crate::util;
use rusqlite::Connection;

/// Coefficient table for the urgency formula.
///
/// Each field contributes to the score additively (see
/// [`compute_urgency_with_breakdown`]). Default values are the project's
/// out-of-the-box weights; per-project overrides live in the `config` table
/// under keys like `urgency.priority.critical` and are loaded by
/// [`UrgencyConfig::load`].
///
/// # Examples
///
/// ```text
/// use itr::urgency::UrgencyConfig;
/// let cfg = UrgencyConfig::default();
/// assert!(cfg.priority_critical > cfg.priority_low);
/// // Blocking other work is a strong positive signal; being blocked is negative.
/// assert!(cfg.blocking > 0.0);
/// assert!(cfg.blocked < 0.0);
/// ```
pub struct UrgencyConfig {
    pub priority_critical: f64,
    pub priority_high: f64,
    pub priority_medium: f64,
    pub priority_low: f64,
    pub blocking: f64,
    pub blocked: f64,
    pub age: f64,
    pub has_acceptance: f64,
    pub kind_bug: f64,
    pub kind_feature: f64,
    pub kind_task: f64,
    pub kind_epic: f64,
    pub in_progress: f64,
    pub notes_count: f64,
}

impl Default for UrgencyConfig {
    fn default() -> Self {
        Self {
            priority_critical: 10.0,
            priority_high: 6.0,
            priority_medium: 3.0,
            priority_low: 1.0,
            blocking: 8.0,
            blocked: -10.0,
            age: 2.0,
            has_acceptance: 1.0,
            kind_bug: 2.0,
            kind_feature: 0.0,
            kind_task: 0.0,
            kind_epic: -2.0,
            in_progress: 4.0,
            notes_count: 0.5,
        }
    }
}

impl UrgencyConfig {
    /// Build a config seeded with defaults, then overlay any per-key overrides
    /// found in the database's `config` table.
    ///
    /// Unknown keys are ignored and unparseable values emit a `REVIEW:` note
    /// on stderr — defaults stay in place either way. This is the standard
    /// soft-fallback behavior for the urgency system: misconfiguration
    /// degrades to defaults rather than failing the command.
    ///
    /// # Examples
    ///
    /// ```text
    /// use itr::urgency::UrgencyConfig;
    /// // given `conn`: an open rusqlite::Connection
    /// let cfg = UrgencyConfig::load(&conn);
    /// assert!(cfg.priority_critical >= 0.0);
    /// ```
    pub fn load(conn: &Connection) -> Self {
        let mut config = Self::default();

        // We'll load each key individually since we can't easily iterate mut refs
        Self::load_key(
            conn,
            "urgency.priority.critical",
            &mut config.priority_critical,
        );
        Self::load_key(conn, "urgency.priority.high", &mut config.priority_high);
        Self::load_key(conn, "urgency.priority.medium", &mut config.priority_medium);
        Self::load_key(conn, "urgency.priority.low", &mut config.priority_low);
        Self::load_key(conn, "urgency.blocking", &mut config.blocking);
        Self::load_key(conn, "urgency.blocked", &mut config.blocked);
        Self::load_key(conn, "urgency.age", &mut config.age);
        Self::load_key(conn, "urgency.has_acceptance", &mut config.has_acceptance);
        Self::load_key(conn, "urgency.kind.bug", &mut config.kind_bug);
        Self::load_key(conn, "urgency.kind.feature", &mut config.kind_feature);
        Self::load_key(conn, "urgency.kind.task", &mut config.kind_task);
        Self::load_key(conn, "urgency.kind.epic", &mut config.kind_epic);
        Self::load_key(conn, "urgency.in_progress", &mut config.in_progress);
        Self::load_key(conn, "urgency.notes_count", &mut config.notes_count);

        config
    }

    fn load_key(conn: &Connection, key: &str, target: &mut f64) {
        if let Ok(Some(val)) = db::config_get(conn, key) {
            match val.parse::<f64>() {
                Ok(v) => *target = v,
                Err(_) => eprintln!(
                    "REVIEW: config value '{}' for '{}' is not numeric; urgency engine is using the default {}",
                    val, key, target
                ),
            }
        }
    }

    /// Return the default coefficient table as a list of
    /// `(config_key, value)` pairs.
    ///
    /// Used by `itr config list` / `itr config reset` to surface the keys
    /// the user can tune without consulting the source.
    ///
    /// # Examples
    ///
    /// ```text
    /// use itr::urgency::UrgencyConfig;
    /// let pairs = UrgencyConfig::defaults_map();
    /// assert!(pairs.iter().any(|(k, _)| *k == "urgency.priority.critical"));
    /// assert!(pairs.iter().any(|(k, _)| *k == "urgency.blocking"));
    /// ```
    pub fn defaults_map() -> Vec<(&'static str, f64)> {
        let d = Self::default();
        vec![
            ("urgency.priority.critical", d.priority_critical),
            ("urgency.priority.high", d.priority_high),
            ("urgency.priority.medium", d.priority_medium),
            ("urgency.priority.low", d.priority_low),
            ("urgency.blocking", d.blocking),
            ("urgency.blocked", d.blocked),
            ("urgency.age", d.age),
            ("urgency.has_acceptance", d.has_acceptance),
            ("urgency.kind.bug", d.kind_bug),
            ("urgency.kind.feature", d.kind_feature),
            ("urgency.kind.task", d.kind_task),
            ("urgency.kind.epic", d.kind_epic),
            ("urgency.in_progress", d.in_progress),
            ("urgency.notes_count", d.notes_count),
        ]
    }

    /// Return the known urgency config key closest to `key` by edit distance.
    ///
    /// Used to power "did you mean ...?" suggestions when a user sets an
    /// unrecognized `urgency.*` key. The candidate list is derived from
    /// [`UrgencyConfig::defaults_map`] so it cannot drift from the keys the
    /// engine actually reads.
    ///
    /// # Examples
    ///
    /// ```text
    /// use itr::urgency::UrgencyConfig;
    /// let hit = UrgencyConfig::closest_key("urgency.priority.critcal");
    /// assert_eq!(hit, Some("urgency.priority.critical"));
    /// ```
    pub fn closest_key(key: &str) -> Option<&'static str> {
        Self::defaults_map()
            .iter()
            .map(|(k, _)| (*k, levenshtein(key, k)))
            .min_by_key(|(_, dist)| *dist)
            .map(|(k, _)| k)
    }
}

/// Classic two-row Levenshtein edit distance over bytes.
///
/// Hand-rolled to keep the dependency footprint at zero; config keys are
/// short ASCII, so byte-wise comparison is exact enough for suggestions.
fn levenshtein(a: &str, b: &str) -> usize {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.is_empty() {
        return b.len();
    }
    if b.is_empty() {
        return a.len();
    }

    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];

    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b.len()]
}

/// Thin wrapper around [`compute_urgency_with_breakdown`] that returns just
/// the scalar score.
///
/// Use this when you only need the number (e.g. when sorting a list);
/// reach for the breakdown variant when you also want to surface the
/// per-component contributions to the user.
///
/// # Examples
///
/// ```text
/// use itr::urgency::{UrgencyConfig, compute_urgency};
/// // given `issue`: an Issue, and `conn`: an open rusqlite::Connection
/// let cfg = UrgencyConfig::default();
/// let score = compute_urgency(&issue, &cfg, &conn);
/// assert!(score.is_finite());
/// ```
pub fn compute_urgency(issue: &Issue, config: &UrgencyConfig, conn: &Connection) -> f64 {
    let (score, _) = compute_urgency_with_breakdown(issue, config, conn);
    score
}

/// Score an issue and return both the total and the per-component breakdown.
///
/// Urgency is always computed fresh from the current state of the issue and
/// its relations — it is never persisted. The components combined are:
///
/// - `priority.<bucket>` — coefficient lookup keyed by priority
/// - `kind.<bucket>` — coefficient lookup keyed by kind (epics may be negative)
/// - `blocking` — added when this issue blocks any other active issue
/// - `blocked` — subtracted when this issue is blocked
/// - `age` — `config.age * clamp(days_since_created / 10, 0, 1)`
/// - `in_progress` — added when status is `in-progress`
/// - `has_acceptance` — added when the acceptance field is non-empty
/// - `notes` — `config.notes_count * min(notes / 6, 1)`
///
/// DB lookup failures degrade to neutral defaults with a `REVIEW:` note on
/// stderr — the scorer never panics or errors out a list command.
///
/// # Examples
///
/// ```text
/// use itr::urgency::{UrgencyConfig, compute_urgency_with_breakdown};
/// // given `issue`: an Issue, and `conn`: an open rusqlite::Connection
/// let cfg = UrgencyConfig::default();
/// let (score, breakdown) = compute_urgency_with_breakdown(&issue, &cfg, &conn);
/// // Sum of (non-zero) components reconstructs the score, modulo float rounding.
/// let total: f64 = breakdown.components.iter().map(|(_, v)| v).sum();
/// assert!((total - score).abs() < 1e-9);
/// ```
pub fn compute_urgency_with_breakdown(
    issue: &Issue,
    config: &UrgencyConfig,
    conn: &Connection,
) -> (f64, UrgencyBreakdown) {
    let mut score = 0.0;
    let mut components = Vec::with_capacity(7);

    // Priority
    let priority_val = match issue.priority.as_str() {
        "critical" => config.priority_critical,
        "high" => config.priority_high,
        "medium" => config.priority_medium,
        "low" => config.priority_low,
        _ => 0.0,
    };
    score += priority_val;
    components.push((format!("priority.{}", issue.priority), priority_val));

    // Kind
    let kind_val = match issue.kind.as_str() {
        "bug" => config.kind_bug,
        "feature" => config.kind_feature,
        "task" => config.kind_task,
        "epic" => config.kind_epic,
        _ => 0.0,
    };
    score += kind_val;
    components.push((format!("kind.{}", issue.kind), kind_val));

    // Blocking others
    let is_blocking = db::blocks_active_issues(conn, issue.id).unwrap_or_else(|e| {
        eprintln!(
            "REVIEW: DB query failed checking if #{} blocks others (treating as not blocking): {}",
            issue.id, e
        );
        false
    });
    if is_blocking {
        score += config.blocking;
        components.push(("blocking".to_string(), config.blocking));
    }

    // Blocked by others
    let is_blocked = db::is_blocked(conn, issue.id).unwrap_or_else(|e| {
        eprintln!(
            "REVIEW: DB query failed checking if #{} is blocked (treating as not blocked): {}",
            issue.id, e
        );
        false
    });
    if is_blocked {
        score += config.blocked;
        components.push(("blocked".to_string(), config.blocked));
    }

    // Age factor
    let age_days = util::days_since(&issue.created_at);
    let age_factor = (age_days / 10.0).clamp(0.0, 1.0);
    let age_val = config.age * age_factor;
    score += age_val;
    components.push(("age".to_string(), age_val));

    // In-progress boost
    if issue.status == "in-progress" {
        score += config.in_progress;
        components.push(("in_progress".to_string(), config.in_progress));
    }

    // Has acceptance criteria
    if !issue.acceptance.is_empty() {
        score += config.has_acceptance;
        components.push(("has_acceptance".to_string(), config.has_acceptance));
    }

    // Notes count
    let notes = db::count_notes(conn, issue.id).unwrap_or_else(|e| {
        eprintln!(
            "REVIEW: DB query failed counting notes for #{} (treating as 0): {}",
            issue.id, e
        );
        0
    });
    let notes_factor = (notes as f64 / 6.0).min(1.0);
    let notes_val = config.notes_count * notes_factor;
    score += notes_val;
    if notes_val != 0.0 {
        components.push(("notes".to_string(), notes_val));
    }

    (score, UrgencyBreakdown { components })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(db::get_schema_sql()).unwrap();
        conn
    }

    fn add_issue(conn: &Connection, priority: &str, kind: &str) -> Issue {
        db::insert_issue(
            conn,
            "test issue",
            priority,
            kind,
            "",
            &[],
            &[],
            &[],
            "",
            None,
            "",
        )
        .unwrap()
    }

    fn add_notes(conn: &Connection, issue_id: i64, count: usize) {
        for i in 0..count {
            conn.execute(
                "INSERT INTO notes (issue_id, content, agent) VALUES (?1, ?2, '')",
                rusqlite::params![issue_id, format!("note {}", i)],
            )
            .unwrap();
        }
    }

    fn component(breakdown: &UrgencyBreakdown, name: &str) -> Option<f64> {
        breakdown
            .components
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| *v)
    }

    // --- #184: negative notes component must appear in the breakdown ---

    #[test]
    fn negative_notes_coefficient_appears_in_breakdown() {
        let conn = test_conn();
        db::config_set(&conn, "urgency.notes_count", "-3").unwrap();
        let issue = add_issue(&conn, "medium", "task");
        add_notes(&conn, issue.id, 2);

        let config = UrgencyConfig::load(&conn);
        let (score, breakdown) = compute_urgency_with_breakdown(&issue, &config, &conn);

        let notes = component(&breakdown, "notes")
            .expect("negative notes component must be present in the breakdown");
        assert!(
            (notes - (-1.0)).abs() < 1e-9,
            "expected -3 * (2/6) = -1.0, got {notes}"
        );

        let total: f64 = breakdown.components.iter().map(|(_, v)| v).sum();
        assert!(
            (total - score).abs() < 1e-9,
            "components ({total}) must sum to the score ({score})"
        );
    }

    #[test]
    fn breakdown_components_sum_to_score_across_configs() {
        let cases: &[&[(&str, &str)]] = &[
            &[],
            &[("urgency.notes_count", "-3")],
            &[("urgency.notes_count", "0")],
            &[("urgency.priority.high", "7.5"), ("urgency.kind.bug", "-1")],
            &[
                ("urgency.in_progress", "-4"),
                ("urgency.has_acceptance", "2"),
            ],
        ];
        for overrides in cases {
            let conn = test_conn();
            for (key, value) in *overrides {
                db::config_set(&conn, key, value).unwrap();
            }
            let mut issue = add_issue(&conn, "high", "bug");
            issue.status = "in-progress".to_string();
            issue.acceptance = "it works".to_string();
            add_notes(&conn, issue.id, 3);

            let config = UrgencyConfig::load(&conn);
            let (score, breakdown) = compute_urgency_with_breakdown(&issue, &config, &conn);
            let total: f64 = breakdown.components.iter().map(|(_, v)| v).sum();
            assert!(
                (total - score).abs() < 1e-9,
                "overrides {overrides:?}: components ({total}) must sum to score ({score})"
            );
        }
    }

    // --- #183: load keeps defaults when a stored value is not numeric ---

    #[test]
    fn load_falls_back_to_default_on_non_numeric_value() {
        let conn = test_conn();
        db::config_set(&conn, "urgency.priority.medium", "abc").unwrap();
        let config = UrgencyConfig::load(&conn);
        assert!(
            (config.priority_medium - UrgencyConfig::default().priority_medium).abs() < 1e-9,
            "non-numeric stored value must fall back to the default"
        );
    }

    #[test]
    fn closest_key_suggests_known_key_for_typo() {
        assert_eq!(
            UrgencyConfig::closest_key("urgency.priority.critcal"),
            Some("urgency.priority.critical")
        );
        assert_eq!(
            UrgencyConfig::closest_key("urgency.notes-count"),
            Some("urgency.notes_count")
        );
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein("", ""), 0);
        assert_eq!(levenshtein("abc", ""), 3);
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }
}
