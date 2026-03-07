use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "itr", about = "Agent-first issue tracker CLI", version = env!("ITR_VERSION"))]
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

    /// Comma-separated list of fields to include in JSON output
    #[arg(long, global = true)]
    pub fields: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new .itr.db database
    Init {
        /// Also append itr instructions to AGENTS.md
        #[arg(long)]
        agents_md: bool,
    },

    /// Create a new issue
    #[command(visible_alias = "create")]
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

        /// File path (repeatable)
        #[arg(long)]
        file: Vec<String>,

        /// Comma-separated tags
        #[arg(short, long)]
        tags: Option<String>,

        /// Tag (repeatable)
        #[arg(long)]
        tag: Vec<String>,

        /// Comma-separated skills (agent capabilities required)
        #[arg(long)]
        skills: Option<String>,

        /// Skill (repeatable)
        #[arg(long)]
        skill: Vec<String>,

        /// Acceptance criteria
        #[arg(short, long)]
        acceptance: Option<String>,

        /// Comma-separated issue IDs this depends on
        #[arg(short, long)]
        blocked_by: Option<String>,

        /// Parent epic ID
        #[arg(long)]
        parent: Option<i64>,

        /// Assign to agent
        #[arg(long)]
        assigned_to: Option<String>,

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
        #[arg(long, visible_alias = "tags")]
        tag: Vec<String>,

        /// Filter by skill (repeatable, AND logic)
        #[arg(long)]
        skill: Vec<String>,

        /// Only show blocked issues
        #[arg(long)]
        blocked: bool,

        /// Include blocked issues in results
        #[arg(long)]
        include_blocked: bool,

        /// Show children of an epic
        #[arg(long)]
        parent: Option<i64>,

        /// Filter by assignee
        #[arg(long)]
        assigned_to: Option<String>,

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

        /// Replace file (repeatable)
        #[arg(long)]
        file: Vec<String>,

        /// Replace tags list (comma-separated)
        #[arg(short, long)]
        tags: Option<String>,

        /// Replace tag (repeatable)
        #[arg(long)]
        tag: Vec<String>,

        /// Replace skills list (comma-separated)
        #[arg(long)]
        skills: Option<String>,

        /// Replace skill (repeatable)
        #[arg(long)]
        skill: Vec<String>,

        /// Replace acceptance criteria
        #[arg(short, long)]
        acceptance: Option<String>,

        /// Set parent epic
        #[arg(long)]
        parent: Option<i64>,

        /// Assign to agent
        #[arg(long)]
        assigned_to: Option<String>,

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

        /// Append a skill (repeatable)
        #[arg(long)]
        add_skill: Vec<String>,

        /// Remove a skill (repeatable)
        #[arg(long)]
        remove_skill: Vec<String>,
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

        /// Close as duplicate of another issue (creates relation + closes)
        #[arg(long)]
        duplicate_of: Option<i64>,
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

        /// Filter by skill (repeatable, AND logic)
        #[arg(long)]
        skill: Vec<String>,

        /// Agent name for assignment (falls back to ITR_AGENT env var)
        #[arg(long)]
        agent: Option<String>,

        /// Filter by assignee
        #[arg(long)]
        assigned_to: Option<String>,
    },

    /// List all unblocked, non-terminal issues by urgency
    Ready {
        /// Max results
        #[arg(short = 'n', long)]
        limit: Option<usize>,

        /// Filter by status within ready set
        #[arg(long)]
        status: Option<String>,

        /// Filter by skill (repeatable, AND logic)
        #[arg(long)]
        skill: Vec<String>,

        /// Filter by assignee
        #[arg(long)]
        assigned_to: Option<String>,
    },

    /// Bulk operations (batch add)
    Batch {
        #[command(subcommand)]
        action: BatchAction,
    },

    /// Bulk close or update issues matching filters
    Bulk {
        #[command(subcommand)]
        action: BulkAction,
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

    /// Print the full agent usage guide (no database required)
    #[command(visible_alias = "getting-started")]
    AgentInfo,

    /// Dump the current database schema
    Schema,

    /// Rebuild and reinstall itr from source
    Upgrade {
        /// Skip git pull (rebuild current source only)
        #[arg(long)]
        no_pull: bool,
        /// Override source directory
        #[arg(long)]
        source_dir: Option<String>,
    },

    /// Claim the highest-urgency unblocked issue (shorthand for next --claim)
    #[command(visible_alias = "start")]
    Claim {
        /// Optional issue ID to claim directly
        id: Option<i64>,

        /// Filter by skill (repeatable, AND logic)
        #[arg(long)]
        skill: Vec<String>,

        /// Agent name for assignment (falls back to ITR_AGENT env var)
        #[arg(long)]
        agent: Option<String>,

        /// Filter by assignee
        #[arg(long)]
        assigned_to: Option<String>,
    },

    /// Assign an issue to an agent
    Assign {
        /// Issue ID
        id: i64,

        /// Agent name
        agent: String,
    },

    /// Unassign an issue
    Unassign {
        /// Issue ID
        id: i64,
    },

    /// View event history (audit log)
    Log {
        /// Issue ID (omit for recent events across all issues)
        id: Option<i64>,

        /// Max events to show
        #[arg(short = 'n', long, default_value = "50")]
        limit: usize,

        /// Only show events since this timestamp (ISO 8601)
        #[arg(long)]
        since: Option<String>,
    },

    /// Create a relation between two issues
    Relate {
        /// Source issue ID
        id: i64,

        /// Target issue ID
        #[arg(long)]
        to: i64,

        /// Relation type: duplicate|related|supersedes
        #[arg(long, visible_alias = "type", default_value = "related")]
        relation_type: String,
    },

    /// Remove a relation between two issues
    Unrelate {
        /// Source issue ID
        id: i64,

        /// Target issue ID
        #[arg(long)]
        from: i64,
    },

    /// Rebuild the full-text search index
    Reindex,

    /// Search issues by text across all fields
    Search {
        /// Search query (all terms must match somewhere)
        query: String,

        /// Include all statuses (done, wontfix)
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

        /// Filter by skill (repeatable, AND logic)
        #[arg(long)]
        skill: Vec<String>,

        /// Filter by assignee
        #[arg(long)]
        assigned_to: Option<String>,

        /// Max results
        #[arg(short = 'n', long)]
        limit: Option<usize>,
    },

    /// Show issues or get detail for a single issue
    Show {
        /// Issue ID (omit to list all non-terminal issues)
        id: Option<i64>,
        /// Include all statuses (done, wontfix)
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
pub enum BatchAction {
    /// Bulk-create issues from JSON array on stdin
    Add,
    /// Bulk-close issues from JSON array on stdin (per-issue reasons)
    Close {
        /// Preview without applying changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Bulk-update issues from JSON array on stdin (per-issue changes)
    Update {
        /// Preview without applying changes
        #[arg(long)]
        dry_run: bool,
    },
    /// Bulk-add notes from JSON array on stdin [{id, text, agent?}]
    Note,
}

#[derive(Subcommand)]
pub enum BulkAction {
    /// Close all issues matching filters
    Close {
        /// Close reason
        #[arg(long)]
        reason: Option<String>,

        /// Close as wontfix instead of done
        #[arg(long)]
        wontfix: bool,

        /// Filter by status
        #[arg(long)]
        status: Option<String>,

        /// Filter by priority
        #[arg(long)]
        priority: Option<String>,

        /// Filter by kind
        #[arg(long)]
        kind: Option<String>,

        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,

        /// Filter by skill
        #[arg(long)]
        skill: Option<String>,

        /// Filter by assignee
        #[arg(long)]
        assigned_to: Option<String>,

        /// Preview without applying changes
        #[arg(long)]
        dry_run: bool,
    },

    /// Update fields on all issues matching filters
    Update {
        /// New status
        #[arg(long)]
        set_status: Option<String>,

        /// New priority
        #[arg(long)]
        set_priority: Option<String>,

        /// Add a tag to matched issues
        #[arg(long)]
        add_tag: Option<String>,

        /// Filter by status
        #[arg(long, visible_alias = "filter-status")]
        status: Option<String>,

        /// Filter by priority
        #[arg(long, visible_alias = "filter-priority")]
        priority: Option<String>,

        /// Filter by kind
        #[arg(long)]
        kind: Option<String>,

        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,

        /// Filter by skill
        #[arg(long)]
        skill: Option<String>,

        /// Filter by assignee
        #[arg(long)]
        assigned_to: Option<String>,

        /// Preview without applying changes
        #[arg(long)]
        dry_run: bool,
    },
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
