mod agent_docs;
mod cli;
mod commands;
mod db;
mod error;
mod format;
mod models;
mod normalize;
mod urgency;

use clap::Parser;
use cli::{BatchAction, BulkAction, Cli, Commands, ConfigAction};
use error::handle_error;
use format::Format;

/// Merge multi-word subcommands that clap can't handle natively.
/// "getting started" (two args) → "getting-started" (one arg).
fn preprocess_args() -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    // Look for consecutive "getting" + "started" and merge them.
    if let Some(pos) = args
        .iter()
        .position(|a| a.to_str().map(|s| s.eq_ignore_ascii_case("getting")) == Some(true))
    {
        if args
            .get(pos + 1)
            .and_then(|a| a.to_str())
            .map(|s| s.eq_ignore_ascii_case("started"))
            == Some(true)
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
            "ERROR: Invalid format '{}'. Valid: compact, json, pretty",
            cli.format
        );
        std::process::exit(1);
    });

    // Parse and validate --fields
    let fields: Option<Vec<String>> = cli.fields.map(|f| format::parse_fields(&f));
    if let Some(ref f) = fields {
        if let Err(e) = format::validate_fields(f) {
            handle_error(e, fmt.is_json());
        }
    }

    // Store fields in a thread-local for JSON filtering
    if let Some(f) = fields {
        if fmt.is_json() {
            format::set_fields_filter(f);
        }
    }

    let result = match cli.command {
        Commands::Init { agents_md } => commands::init::run(agents_md, fmt, cli.db.as_deref()),
        Commands::AgentInfo => commands::agent_info::run(fmt),
        Commands::Schema => commands::schema::run(fmt),
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

            run_command(cli.command, &conn, fmt)
        }
    };

    if let Err(e) = result {
        handle_error(e, fmt.is_json());
    }
}

fn run_command(
    command: Commands,
    conn: &rusqlite::Connection,
    fmt: Format,
) -> Result<(), error::ItrError> {
    match command {
        Commands::Init { .. }
        | Commands::AgentInfo
        | Commands::Schema
        | Commands::Upgrade { .. } => {
            unreachable!()
        }

        Commands::Add {
            title,
            priority,
            kind,
            context,
            files,
            tags,
            skills,
            acceptance,
            blocked_by,
            parent,
            assigned_to,
            stdin_json,
        } => commands::add::run(
            conn,
            title,
            &priority,
            &kind,
            context,
            files,
            tags,
            skills,
            acceptance,
            blocked_by,
            parent,
            assigned_to,
            stdin_json,
            fmt,
        ),

        Commands::List {
            all,
            status,
            priority,
            kind,
            tag,
            skill,
            blocked,
            include_blocked,
            parent,
            assigned_to,
            sort,
            limit,
        } => {
            let no_filters = status.is_empty()
                && priority.is_empty()
                && kind.is_empty()
                && tag.is_empty()
                && skill.is_empty()
                && !blocked
                && parent.is_none()
                && assigned_to.is_none();
            let effective_include_blocked = include_blocked || (no_filters && !all);
            commands::list::run(
                conn,
                all,
                status,
                priority,
                kind,
                tag,
                skill,
                blocked,
                effective_include_blocked,
                parent,
                assigned_to,
                &sort,
                limit,
                fmt,
            )
        }

        Commands::Get { id } => commands::get::run(conn, id, fmt),

        Commands::Update {
            id,
            status,
            priority,
            kind,
            title,
            context,
            files,
            tags,
            skills,
            acceptance,
            parent,
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
            tags,
            skills,
            acceptance,
            parent,
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
            reason,
            wontfix,
            duplicate_of,
        } => {
            if let Some(dup_id) = duplicate_of {
                db::add_relation(conn, id, dup_id, "duplicate")?;
                let reason = reason.unwrap_or_else(|| format!("Duplicate of #{}", dup_id));
                commands::close::run(conn, id, Some(reason), false, fmt)
            } else {
                commands::close::run(conn, id, reason, wontfix, fmt)
            }
        }

        Commands::Note { id, text, agent } => commands::note::run(conn, id, text, &agent, fmt),

        Commands::Depend { id, on } => commands::depend::run(conn, id, on, fmt),

        Commands::Undepend { id, on } => commands::depend::run_undepend(conn, id, on, fmt),

        Commands::Next {
            claim,
            skill,
            agent,
            assigned_to,
        } => commands::next::run(conn, claim, skill, agent, assigned_to, fmt),

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

        Commands::Export { export_format } => commands::export::run(conn, &export_format),

        Commands::Import { file, merge } => commands::import::run(conn, file, merge, fmt),

        Commands::Doctor { fix } => commands::doctor::run(conn, fix, fmt),

        Commands::Config { action } => match action {
            ConfigAction::List => commands::config::run_list(conn, fmt),
            ConfigAction::Get { key } => commands::config::run_get(conn, &key, fmt),
            ConfigAction::Set { key, value } => commands::config::run_set(conn, &key, &value, fmt),
            ConfigAction::Reset => commands::config::run_reset(conn, fmt),
        },

        Commands::Log { id, limit, since } => commands::log::run(conn, id, limit, since, fmt),

        Commands::Reindex => commands::reindex::run(conn, fmt),

        Commands::Relate {
            id,
            to,
            relation_type,
        } => commands::relate::run_relate(conn, id, to, &relation_type, fmt),

        Commands::Unrelate { id, from } => commands::relate::run_unrelate(conn, id, from, fmt),

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
            skill,
            agent,
            assigned_to,
        } => commands::next::run(conn, true, skill, agent, assigned_to, fmt),

        Commands::Assign { id, agent } => commands::assign::run_assign(conn, id, &agent, fmt),

        Commands::Unassign { id } => commands::assign::run_unassign(conn, id, fmt),

        Commands::Show { id: Some(id), .. } => commands::get::run(conn, id, fmt),
        Commands::Show { id: None, all } => {
            if all {
                eprintln!("hint: use `itr list --all` for full filtering options");
            }
            commands::list::run(
                conn,
                all,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                false,
                true,
                None,
                None,
                "urgency",
                None,
                fmt,
            )
        }
    }
}
