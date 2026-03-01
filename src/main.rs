mod cli;
mod commands;
mod db;
mod error;
mod format;
mod models;
mod normalize;
mod urgency;

use clap::Parser;
use cli::{BatchAction, Cli, Commands, ConfigAction};
use error::handle_error;
use format::Format;

fn main() {
    let cli = Cli::parse();

    let fmt = Format::from_str(&cli.format).unwrap_or_else(|| {
        eprintln!("ERROR: Invalid format '{}'. Valid: compact, json, pretty", cli.format);
        std::process::exit(1);
    });

    let result = match cli.command {
        Commands::Init { agents_md } => commands::init::run(agents_md, fmt, cli.db.as_deref()),
        Commands::Schema => commands::schema::run(fmt),
        Commands::Upgrade { no_pull, source_dir } => commands::upgrade::run(no_pull, source_dir, fmt),
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
        Commands::Init { .. } | Commands::Schema | Commands::Upgrade { .. } => unreachable!(),

        Commands::Add {
            title,
            priority,
            kind,
            context,
            files,
            tags,
            acceptance,
            blocked_by,
            parent,
            stdin_json,
        } => commands::add::run(
            conn, title, &priority, &kind, context, files, tags, acceptance, blocked_by, parent,
            stdin_json, fmt,
        ),

        Commands::List {
            all,
            status,
            priority,
            kind,
            tag,
            blocked,
            include_blocked,
            parent,
            sort,
            limit,
        } => {
            // When no filters are given, default to showing all non-terminal issues (including blocked)
            let no_filters = status.is_empty() && priority.is_empty()
                && kind.is_empty() && tag.is_empty() && !blocked && parent.is_none();
            let effective_include_blocked = include_blocked || (no_filters && !all);
            commands::list::run(
                conn, all, status, priority, kind, tag, blocked, effective_include_blocked, parent, &sort,
                limit, fmt,
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
            acceptance,
            parent,
            add_tag,
            remove_tag,
            add_file,
            remove_file,
        } => commands::update::run(
            conn, id, status, priority, kind, title, context, files, tags, acceptance, parent,
            add_tag, remove_tag, add_file, remove_file, fmt,
        ),

        Commands::Close {
            id,
            reason,
            wontfix,
        } => commands::close::run(conn, id, reason, wontfix, fmt),

        Commands::Note { id, text, agent } => commands::note::run(conn, id, text, &agent, fmt),

        Commands::Depend { id, on } => commands::depend::run(conn, id, on, fmt),

        Commands::Undepend { id, on } => commands::depend::run_undepend(conn, id, on, fmt),

        Commands::Next { claim } => commands::next::run(conn, claim, fmt),

        Commands::Ready { limit, status } => commands::ready::run(conn, limit, status, fmt),

        Commands::Batch { action } => match action {
            BatchAction::Add => commands::batch::run_add(conn, fmt),
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

        Commands::Claim => commands::next::run(conn, true, fmt),

        Commands::Show { id: Some(id) } => commands::get::run(conn, id, fmt),
        Commands::Show { id: None } => {
            // Show all non-terminal issues (including blocked)
            commands::list::run(
                conn, false, vec![], vec![], vec![], vec![], false, true, None, "urgency",
                None, fmt,
            )
        }
    }
}
