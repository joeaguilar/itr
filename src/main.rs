mod agent_docs;
mod cli;
mod commands;
mod db;
mod error;
mod format;
mod models;
mod normalize;
mod urgency;
mod util;

use clap::Parser;
use cli::{BatchAction, BulkAction, Cli, Commands, ConfigAction};
use error::handle_error;
use format::Format;
use models::ListFilter;

/// Merge multi-word subcommands that clap can't handle natively.
/// "getting started" (two args) → "getting-started" (one arg).
fn preprocess_args() -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    // Look for consecutive "getting" + "started" and merge them.
    if let Some(pos) = args.iter().position(|a| {
        a.to_str()
            .is_some_and(|s| s.eq_ignore_ascii_case("getting"))
    }) {
        if args
            .get(pos + 1)
            .and_then(|a| a.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("started"))
        {
            args[pos] = "getting-started".into();
            args.remove(pos + 1);
        }
    }
    args
}

fn main() {
    let cli = Cli::parse_from(preprocess_args());

    let fmt = Format::from_str(&cli.format).unwrap_or_else(|| {
        eprintln!(
            "ERROR: Invalid format '{}'. Valid: compact, json, pretty, oneline",
            cli.format
        );
        std::process::exit(1);
    });

    // Parse and validate --fields (unknown fields are warned but kept)
    let fields: Option<Vec<String>> = cli.fields.map(|f| {
        let parsed = format::parse_fields(&f);
        format::validate_fields(&parsed);
        parsed
    });

    // Store fields in a thread-local for all output formats
    if let Some(f) = fields {
        format::set_fields_filter(f);
    }

    let result = match cli.command {
        Commands::Init { agents_md } => commands::init::run(agents_md, fmt, cli.db.as_deref()),
        Commands::AgentInfo => commands::agent_info::run(fmt),
        Commands::Schema => commands::schema::run(fmt),
        Commands::Skill { action } => commands::skill::run(action, fmt),
        Commands::Upgrade {
            no_pull,
            source_dir,
        } => commands::upgrade::run(no_pull, source_dir, fmt),
        _ => {
            // All other commands need the database
            let db_path = match db::find_db(cli.db.as_deref()) {
                Ok(p) => p,
                Err(e) => handle_error(e, fmt.is_json()),
            };
            let conn = match db::open_db(&db_path) {
                Ok(c) => c,
                Err(e) => handle_error(e, fmt.is_json()),
            };

            run_command(cli.command, &conn, &db_path, fmt)
        }
    };

    if let Err(e) = result {
        handle_error(e, fmt.is_json());
    }
}

/// Build the `ListFilter` for `itr list`.
///
/// Filters narrow results without changing blocked-visibility semantics
/// (#169): blocked issues are always listed unless `--blocked` narrows to
/// blocked-only. `--include-blocked` is accepted for compatibility but is
/// now the default behavior.
#[allow(clippy::too_many_arguments)]
fn build_list_filter(
    all: bool,
    statuses: Vec<String>,
    priorities: Vec<String>,
    kinds: Vec<String>,
    tags: Vec<String>,
    tag_any: Vec<String>,
    skills: Vec<String>,
    blocked: bool,
    _include_blocked: bool,
    parent_id: Option<i64>,
    assigned_to: Option<String>,
) -> ListFilter {
    ListFilter {
        statuses,
        priorities,
        kinds,
        tags,
        tag_any,
        skills,
        blocked_only: blocked,
        include_blocked: true,
        parent_id,
        assigned_to,
        all,
    }
}

/// Resolve the `(reason, wontfix)` pair passed to `close::run`.
///
/// `--duplicate-of` supplies a default reason naming the duplicate target,
/// and `--wontfix` is honored rather than silently discarded when combined
/// with `--duplicate-of` (#190).
fn close_args(
    reason: Option<String>,
    wontfix: bool,
    duplicate_of: Option<i64>,
) -> (Option<String>, bool) {
    match duplicate_of {
        Some(dup_id) => (
            Some(reason.unwrap_or_else(|| format!("Duplicate of #{}", dup_id))),
            wontfix,
        ),
        None => (reason, wontfix),
    }
}

fn run_command(
    command: Commands,
    conn: &rusqlite::Connection,
    db_path: &std::path::Path,
    fmt: Format,
) -> Result<(), error::ItrError> {
    match command {
        Commands::Init { .. }
        | Commands::AgentInfo
        | Commands::Schema
        | Commands::Skill { .. }
        | Commands::Upgrade { .. } => {
            unreachable!()
        }

        Commands::Add {
            title,
            title_flag,
            priority,
            kind,
            context,
            files,
            file,
            tags,
            tag,
            skills,
            skill,
            acceptance,
            blocked_by,
            parent,
            assigned_to,
            stdin_json,
        } => {
            // Merge: --title flag takes precedence over positional
            let effective_title = match (title, title_flag) {
                (Some(pos), Some(flag)) => {
                    eprintln!(
                        "REVIEW: both positional title and --title provided; using --title. \
                         Positional '{}' was ignored — fix your invocation to use one or the other.",
                        pos
                    );
                    Some(flag)
                }
                (None, Some(flag)) => Some(flag),
                (pos, None) => pos,
            };
            commands::add::run(
                conn,
                effective_title,
                &priority,
                &kind,
                context,
                files,
                file,
                tags,
                tag,
                skills,
                skill,
                acceptance,
                blocked_by,
                parent,
                assigned_to,
                stdin_json,
                fmt,
            )
        }

        Commands::List {
            all,
            status,
            priority,
            kind,
            tag,
            tag_any,
            skill,
            blocked,
            include_blocked,
            parent,
            assigned_to,
            sort,
            limit,
        } => {
            let filter = build_list_filter(
                all,
                status,
                priority,
                kind,
                tag,
                tag_any,
                skill,
                blocked,
                include_blocked,
                parent,
                assigned_to,
            );
            commands::list::run(conn, &filter, &sort, limit, fmt)
        }

        Commands::Get { ids } => commands::get::run(conn, &ids, fmt),

        Commands::Update {
            id,
            status,
            priority,
            kind,
            title,
            context,
            files,
            file,
            tags,
            tag,
            skills,
            skill,
            acceptance,
            parent,
            no_parent,
            assigned_to,
            add_tag,
            remove_tag,
            add_file,
            remove_file,
            add_skill,
            remove_skill,
        } => commands::update::run(
            conn,
            id,
            status,
            priority,
            kind,
            title,
            context,
            files,
            file,
            tags,
            tag,
            skills,
            skill,
            acceptance,
            parent,
            no_parent,
            assigned_to,
            add_tag,
            remove_tag,
            add_file,
            remove_file,
            add_skill,
            remove_skill,
            fmt,
        ),

        Commands::Close {
            id,
            positional_reason,
            reason_flag,
            wontfix,
            duplicate_of,
        } => {
            // Merge: --reason flag takes precedence over positional
            let effective_reason = match (positional_reason, reason_flag) {
                (Some(pos), Some(flag)) => {
                    eprintln!(
                        "REVIEW: both positional reason and --reason provided; using --reason. \
                         Positional '{}' was ignored — fix your invocation to use one or the other.",
                        pos
                    );
                    Some(flag)
                }
                (None, Some(flag)) => Some(flag),
                (pos, None) => pos,
            };
            let (reason, wontfix) = close_args(effective_reason, wontfix, duplicate_of);
            if let Some(dup_id) = duplicate_of {
                db::add_relation(conn, id, dup_id, "duplicate")?;
            }
            commands::close::run(conn, id, reason, wontfix, fmt)
        }

        Commands::Note { id, text, agent } => commands::note::run(conn, id, text, &agent, fmt),

        Commands::NoteDelete { id } => commands::note::run_delete(conn, id, fmt),

        Commands::NoteUpdate { id, text } => commands::note::run_update(conn, id, &text, fmt),

        Commands::Depend { id, on } => commands::depend::run(conn, id, on, fmt),

        Commands::Undepend { id, on } => commands::depend::run_undepend(conn, id, on, fmt),

        Commands::Next {
            claim,
            skill,
            agent,
            assigned_to,
        } => commands::next::run(conn, claim, None, skill, agent, assigned_to, fmt),

        Commands::Ready {
            limit,
            status,
            skill,
            assigned_to,
        } => commands::ready::run(conn, limit, status, skill, assigned_to, fmt),

        Commands::Batch { action } => match action {
            BatchAction::Add => commands::batch::run_add(conn, fmt),
            BatchAction::Close { dry_run } => commands::batch::run_close(conn, dry_run, fmt),
            BatchAction::Update { dry_run } => commands::batch::run_update(conn, dry_run, fmt),
            BatchAction::Note => commands::batch::run_note(conn, fmt),
        },

        Commands::Bulk { action } => match action {
            BulkAction::Close {
                reason,
                wontfix,
                status,
                priority,
                kind,
                tag,
                skill,
                assigned_to,
                dry_run,
            } => commands::bulk::run_close(
                conn,
                reason,
                wontfix,
                status,
                priority,
                kind,
                tag,
                skill,
                assigned_to,
                dry_run,
                fmt,
            ),
            BulkAction::Update {
                set_status,
                set_priority,
                add_tag,
                status,
                priority,
                kind,
                tag,
                skill,
                assigned_to,
                dry_run,
            } => commands::bulk::run_update(
                conn,
                set_status,
                set_priority,
                add_tag,
                status,
                priority,
                kind,
                tag,
                skill,
                assigned_to,
                dry_run,
                fmt,
            ),
        },

        Commands::Graph { all } => commands::graph::run(conn, all, fmt),

        Commands::Stats => commands::stats::run(conn, fmt),
        Commands::Summary => commands::summary::run(conn, fmt),

        Commands::Export { export_format } => commands::export::run(conn, &export_format),

        Commands::Import { file, merge } => commands::import::run(conn, file, merge, fmt),

        Commands::Doctor { fix } => commands::doctor::run(conn, fix, fmt),

        Commands::Ui {
            port,
            no_open,
            once,
            allow_dangerous,
        } => commands::ui::run(conn, db_path, port, no_open, once, allow_dangerous, fmt),

        Commands::Config { action } => match action {
            ConfigAction::List => commands::config::run_list(conn, fmt),
            ConfigAction::Get { key } => commands::config::run_get(conn, &key, fmt),
            ConfigAction::Set { key, value } => commands::config::run_set(conn, &key, &value, fmt),
            ConfigAction::Reset => commands::config::run_reset(conn, fmt),
        },

        Commands::Log {
            id,
            limit,
            since,
            agent,
        } => commands::log::run(conn, id, limit, since, agent, fmt),

        Commands::Reindex => commands::reindex::run(conn, fmt),

        Commands::Relate {
            id,
            to,
            relation_type,
        } => commands::relate::run_relate(conn, id, to, &relation_type, fmt),

        Commands::Unrelate {
            id,
            from,
            relation_type,
        } => commands::relate::run_unrelate(conn, id, from, relation_type.as_deref(), fmt),

        Commands::Search {
            query,
            all,
            status,
            priority,
            kind,
            skill,
            assigned_to,
            limit,
        } => commands::search::run(
            conn,
            &query,
            all,
            status,
            priority,
            kind,
            skill,
            assigned_to,
            limit,
            fmt,
        ),

        Commands::Claim {
            id,
            skill,
            agent,
            assigned_to,
        } => commands::next::run(conn, true, id, skill, agent, assigned_to, fmt),

        Commands::Assign { id, agent } => commands::assign::run_assign(conn, id, &agent, fmt),

        Commands::Unassign { id } => commands::assign::run_unassign(conn, id, fmt),

        Commands::Wip => commands::list::run(
            conn,
            &ListFilter {
                statuses: vec!["in-progress".to_string()],
                include_blocked: true,
                ..ListFilter::default()
            },
            "urgency",
            None,
            fmt,
        ),

        Commands::Show { ids, all } => {
            if ids.is_empty() {
                if all {
                    eprintln!("hint: use `itr list --all` for full filtering options");
                }
                commands::list::run(
                    conn,
                    &ListFilter {
                        include_blocked: true,
                        all,
                        ..ListFilter::default()
                    },
                    "urgency",
                    None,
                    fmt,
                )
            } else {
                commands::get::run(conn, &ids, fmt)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter_with(statuses: Vec<String>, priorities: Vec<String>) -> ListFilter {
        build_list_filter(
            false,
            statuses,
            priorities,
            vec![],
            vec![],
            vec![],
            vec![],
            false,
            false,
            None,
            None,
        )
    }

    // --- #169: filters narrow results, they do not flip blocked-visibility ---

    #[test]
    fn status_filter_does_not_hide_blocked_issues() {
        let filter = filter_with(vec!["done".to_string()], vec![]);
        assert!(
            filter.include_blocked,
            "an explicit -s filter must keep the same blocked-visibility default as plain list"
        );

        let filter = filter_with(vec!["open".to_string()], vec![]);
        assert!(filter.include_blocked);
    }

    #[test]
    fn priority_filter_does_not_hide_blocked_issues() {
        let filter = filter_with(vec![], vec!["high".to_string()]);
        assert!(filter.include_blocked);
    }

    #[test]
    fn blocked_only_filter_is_preserved() {
        let filter = build_list_filter(
            false,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            true,
            false,
            None,
            None,
        );
        assert!(
            filter.blocked_only,
            "--blocked must still narrow to blocked"
        );
    }

    // --- #190: close --duplicate-of must not swallow --wontfix ---

    #[test]
    fn close_args_honors_wontfix_with_duplicate_of() {
        let (reason, wontfix) = close_args(None, true, Some(3));
        assert_eq!(reason.as_deref(), Some("Duplicate of #3"));
        assert!(
            wontfix,
            "--wontfix must be honored alongside --duplicate-of"
        );
    }

    #[test]
    fn close_args_plain_duplicate_of_unchanged() {
        let (reason, wontfix) = close_args(None, false, Some(7));
        assert_eq!(reason.as_deref(), Some("Duplicate of #7"));
        assert!(!wontfix);

        let (reason, wontfix) = close_args(Some("custom".to_string()), false, Some(7));
        assert_eq!(reason.as_deref(), Some("custom"));
        assert!(!wontfix);

        let (reason, wontfix) = close_args(Some("r".to_string()), true, None);
        assert_eq!(reason.as_deref(), Some("r"));
        assert!(wontfix);
    }

    #[test]
    fn close_duplicate_of_with_wontfix_sets_wontfix_status() {
        let conn = db::open_test_db();
        let original = db::insert_issue(
            &conn,
            "original",
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
        .expect("insert original")
        .id;
        let dup = db::insert_issue(
            &conn,
            "dup",
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
        .expect("insert dup")
        .id;

        run_command(
            Commands::Close {
                id: dup,
                positional_reason: None,
                reason_flag: None,
                wontfix: true,
                duplicate_of: Some(original),
            },
            &conn,
            std::path::Path::new("unused"),
            Format::Compact,
        )
        .expect("close as duplicate + wontfix");

        let issue = db::get_issue(&conn, dup).expect("get issue");
        assert_eq!(
            issue.status, "wontfix",
            "--wontfix combined with --duplicate-of must close as wontfix"
        );
        assert_eq!(issue.close_reason, format!("Duplicate of #{}", original));
        let relations = db::get_relations(&conn, dup).expect("relations");
        assert!(
            relations.iter().any(|r| r.relation_type == "duplicate"),
            "duplicate relation must still be recorded"
        );
    }
}
