use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "nit", about = "Agent-first issue tracker CLI", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Output format: compact|json|pretty
    #[arg(short, long, default_value = "compact", global = true)]
    pub format: String,

    /// Override database path (skips walk-up search)
    #[arg(long, global = true)]
    pub db: Option<String>,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new .nit.db database
    Init {
        /// Also append nit instructions to AGENTS.md
        #[arg(long)]
        agents_md: bool,
    },

    /// Create a new issue
    Add {
        /// Issue title
        title: Option<String>,

        /// Priority: critical|high|medium|low
        #[arg(short, long, default_value = "medium")]
        priority: String,

        /// Kind: bug|feature|task|epic
        #[arg(short, long, default_value = "task")]
        kind: String,

        /// Freeform context/description
        #[arg(short, long)]
        context: Option<String>,

        /// Comma-separated file paths
        #[arg(long)]
        files: Option<String>,

        /// Comma-separated tags
        #[arg(short, long)]
        tags: Option<String>,

        /// Acceptance criteria
        #[arg(short, long)]
        acceptance: Option<String>,

        /// Comma-separated issue IDs this depends on
        #[arg(short, long)]
        blocked_by: Option<String>,

        /// Parent epic ID
        #[arg(long)]
        parent: Option<i64>,

        /// Read a JSON issue object from stdin
        #[arg(long)]
        stdin_json: bool,
    },

    /// List issues with filtering
    List {
        /// Include all statuses
        #[arg(long)]
        all: bool,

        /// Filter by status (repeatable)
        #[arg(short, long)]
        status: Vec<String>,

        /// Filter by priority (repeatable)
        #[arg(short, long)]
        priority: Vec<String>,

        /// Filter by kind (repeatable)
        #[arg(short, long)]
        kind: Vec<String>,

        /// Filter by tag (repeatable, AND logic)
        #[arg(long)]
        tag: Vec<String>,

        /// Only show blocked issues
        #[arg(long)]
        blocked: bool,

        /// Include blocked issues in results
        #[arg(long)]
        include_blocked: bool,

        /// Show children of an epic
        #[arg(long)]
        parent: Option<i64>,

        /// Sort by: urgency|priority|created|updated|id
        #[arg(long, default_value = "urgency")]
        sort: String,

        /// Max results
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },

    /// Get full detail for a single issue
    Get {
        /// Issue ID
        id: i64,
    },

    /// Update an issue
    Update {
        /// Issue ID
        id: i64,

        /// New status
        #[arg(short, long)]
        status: Option<String>,

        /// New priority
        #[arg(short, long)]
        priority: Option<String>,

        /// New kind
        #[arg(short, long)]
        kind: Option<String>,

        /// New title
        #[arg(long)]
        title: Option<String>,

        /// Replace context
        #[arg(short, long)]
        context: Option<String>,

        /// Replace files list (comma-separated)
        #[arg(long)]
        files: Option<String>,

        /// Replace tags list (comma-separated)
        #[arg(short, long)]
        tags: Option<String>,

        /// Replace acceptance criteria
        #[arg(short, long)]
        acceptance: Option<String>,

        /// Set parent epic
        #[arg(long)]
        parent: Option<i64>,

        /// Append a tag (repeatable)
        #[arg(long)]
        add_tag: Vec<String>,

        /// Remove a tag (repeatable)
        #[arg(long)]
        remove_tag: Vec<String>,

        /// Append a file (repeatable)
        #[arg(long)]
        add_file: Vec<String>,

        /// Remove a file (repeatable)
        #[arg(long)]
        remove_file: Vec<String>,
    },

    /// Close an issue (shorthand for update --status done)
    Close {
        /// Issue ID
        id: i64,

        /// Close reason
        reason: Option<String>,

        /// Close as wontfix instead of done
        #[arg(long)]
        wontfix: bool,
    },

    /// Append a note to an issue
    Note {
        /// Issue ID
        id: i64,

        /// Note content
        text: Option<String>,

        /// Agent/session identifier
        #[arg(long, default_value = "")]
        agent: String,
    },

    /// Add a dependency (issue becomes blocked by --on)
    Depend {
        /// Issue ID that will be blocked
        id: i64,

        /// Issue ID that blocks it
        #[arg(long)]
        on: i64,
    },

    /// Remove a dependency
    Undepend {
        /// Issue ID that was blocked
        id: i64,

        /// Issue ID that was blocking it
        #[arg(long)]
        on: i64,
    },

    /// Get the highest-urgency unblocked issue
    Next {
        /// Also set the issue to in-progress
        #[arg(long)]
        claim: bool,
    },

    /// List all unblocked, non-terminal issues by urgency
    Ready {
        /// Max results
        #[arg(short = 'n', long)]
        limit: Option<usize>,

        /// Filter by status within ready set
        #[arg(long)]
        status: Option<String>,
    },

    /// Bulk operations
    Batch {
        #[command(subcommand)]
        action: BatchAction,
    },

    /// Output the dependency graph
    Graph {
        /// Include resolved issues
        #[arg(long)]
        all: bool,
    },

    /// Project health summary
    Stats,

    /// Export the full database
    Export {
        /// Export format: jsonl|json
        #[arg(long, default_value = "jsonl")]
        export_format: String,
    },

    /// Import issues from JSONL or JSON
    Import {
        /// Input file path (or stdin)
        #[arg(long)]
        file: Option<String>,

        /// Skip existing IDs instead of erroring
        #[arg(long)]
        merge: bool,
    },

    /// Run database integrity checks
    Doctor {
        /// Auto-fix safe issues
        #[arg(long)]
        fix: bool,
    },

    /// Manage per-project configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Dump the current database schema
    Schema,
}

#[derive(Subcommand)]
pub enum BatchAction {
    /// Bulk-create issues from JSON array on stdin
    Add,
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// List all settings
    List,
    /// Get a config value
    Get { key: String },
    /// Set a config value
    Set { key: String, value: String },
    /// Restore all defaults
    Reset,
}
