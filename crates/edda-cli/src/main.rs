mod cmd_ask;
mod cmd_blob;
mod cmd_branch;
mod cmd_bridge;
mod cmd_commit;
mod cmd_conduct;
mod cmd_config;
mod cmd_context;
mod cmd_draft;
mod cmd_gc;
mod cmd_init;
mod cmd_log;
mod cmd_merge;
mod cmd_note;
mod cmd_pattern;
mod cmd_plan;
mod cmd_rebuild;
mod cmd_run;
mod cmd_search;
mod cmd_status;
mod cmd_switch;
mod cmd_watch;
#[cfg(feature = "tui")]
mod tui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "edda", version, about = "Decision memory for coding agents")]
struct Cli {
    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new .edda/ workspace
    Init {
        /// Skip auto-detection and installation of bridge hooks
        #[arg(long)]
        no_hooks: bool,
    },
    /// Record a note event
    Note {
        /// Note text
        text: String,
        /// Role: user, assistant, or system
        #[arg(long, default_value = "user")]
        role: String,
        /// Tags for the note (repeatable)
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
    /// Record a binding decision (shortcut for `bridge claude decide`)
    Decide {
        /// Decision in key=value format (e.g. "db=PostgreSQL")
        decision: String,
        /// Reason for the decision
        #[arg(long)]
        reason: Option<String>,
        /// Session ID (auto-inferred from active heartbeats if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Claim a scope for coordination (shortcut for `bridge claude claim`)
    Claim {
        /// Short label for this session's scope (e.g. "auth", "billing")
        label: String,
        /// File path patterns this scope covers (e.g. "src/auth/*")
        #[arg(long)]
        paths: Vec<String>,
        /// Session ID (auto-inferred from active heartbeats if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Send a request to another session (shortcut for `bridge claude request`)
    Request {
        /// Target session label
        to: String,
        /// Request message
        message: String,
        /// Session ID (auto-inferred from active heartbeats if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Query project decisions, history, and conversations
    Ask {
        /// Query string (keyword, domain, or exact key like "db.engine"). Omit for all active decisions.
        query: Option<String>,
        /// Maximum results per section (default: 20)
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Include superseded decisions
        #[arg(long)]
        all: bool,
        /// Filter by branch
        #[arg(long)]
        branch: Option<String>,
    },
    /// Run a command and record its output
    Run {
        /// Command and arguments (after --)
        #[arg(last = true)]
        argv: Vec<String>,
    },
    /// Show workspace status
    Status,
    /// Create a commit event
    Commit {
        /// Commit title
        #[arg(short, long)]
        title: String,
        /// Purpose of this commit
        #[arg(long)]
        purpose: Option<String>,
        /// Contribution description (defaults to title)
        #[arg(long)]
        contrib: Option<String>,
        /// Evidence refs: evt_* or blob:sha256:* (repeatable)
        #[arg(long = "evidence")]
        evidence: Vec<String>,
        /// Labels (repeatable)
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Enable auto-evidence collection (also auto-enabled when no --evidence given)
        #[arg(long)]
        auto: bool,
        /// Preview commit without writing to ledger
        #[arg(long)]
        dry_run: bool,
        /// Maximum number of auto-evidence items
        #[arg(long, default_value_t = 20)]
        max_evidence: usize,
    },
    /// Query events from the ledger with filters
    Log {
        /// Filter by event type (note, cmd, commit, merge, etc.)
        #[arg(long = "type")]
        event_type: Option<String>,
        /// Filter by event family (signal, milestone, admin, governance)
        #[arg(long)]
        family: Option<String>,
        /// Filter by tag (matches payload.tags array)
        #[arg(long)]
        tag: Option<String>,
        /// Filter by keyword (case-insensitive payload text search)
        #[arg(long)]
        keyword: Option<String>,
        /// Filter events after this date/time (ISO 8601 prefix, e.g. 2026-02-13)
        #[arg(long)]
        after: Option<String>,
        /// Filter events before this date/time
        #[arg(long)]
        before: Option<String>,
        /// Filter by branch name
        #[arg(long)]
        branch: Option<String>,
        /// Maximum number of events to show (0 = unlimited)
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Output as JSON lines (one event per line)
        #[arg(long)]
        json: bool,
    },
    /// Output context snapshot as Markdown
    Context {
        /// Branch name (defaults to HEAD)
        #[arg(long)]
        branch: Option<String>,
        /// Number of recent commits/signals to show
        #[arg(long, default_value = "5")]
        depth: usize,
    },
    /// Rebuild derived views
    Rebuild {
        /// Rebuild a specific branch (defaults to HEAD)
        #[arg(long)]
        branch: Option<String>,
        /// Rebuild all branches
        #[arg(long)]
        all: bool,
        /// Reason for rebuild
        #[arg(long, default_value = "rebuild views")]
        reason: String,
    },
    /// Branch operations
    Branch {
        #[command(subcommand)]
        cmd: cmd_branch::BranchCmd,
    },
    /// Switch to another branch
    Switch {
        /// Target branch name
        name: String,
    },
    /// Merge a source branch into a destination branch
    Merge {
        /// Source branch
        src: String,
        /// Destination branch (must be HEAD)
        dst: String,
        /// Reason for merge
        #[arg(short = 'm', long = "reason")]
        reason: String,
    },
    /// Draft commit operations (propose, show, list, apply, delete)
    Draft {
        #[command(subcommand)]
        cmd: cmd_draft::DraftCmd,
    },
    /// Bridge operations (install/uninstall hooks for Claude Code)
    Bridge {
        #[command(subcommand)]
        cmd: cmd_bridge::BridgeCmd,
    },
    /// Hook entrypoint (called by Claude Code hooks)
    Hook {
        #[command(subcommand)]
        cmd: cmd_bridge::HookCmd,
    },
    /// Health check for bridge integration
    Doctor {
        #[command(subcommand)]
        cmd: cmd_bridge::DoctorCmd,
    },
    /// Index operations
    Index {
        #[command(subcommand)]
        cmd: cmd_bridge::IndexCmd,
    },
    /// Read or write workspace config (.edda/config.json)
    Config {
        #[command(subcommand)]
        cmd: cmd_config::ConfigCmd,
    },
    /// Manage pattern store (.edda/patterns/)
    Pattern {
        #[command(subcommand)]
        cmd: cmd_pattern::PatternCmd,
    },
    /// MCP server operations
    Mcp {
        #[command(subcommand)]
        cmd: McpCommand,
    },
    /// Full-text search (Tantivy)
    Search {
        #[command(subcommand)]
        cmd: cmd_search::SearchCmd,
    },
    /// Manage blob metadata (classify, pin, unpin, info, stats)
    Blob {
        #[command(subcommand)]
        cmd: cmd_blob::BlobCmd,
    },
    /// Plan scaffolding and templates
    Plan {
        #[command(subcommand)]
        cmd: cmd_plan::PlanCmd,
    },
    /// Multi-phase AI plan conductor
    Conduct {
        #[command(subcommand)]
        cmd: cmd_conduct::ConductCmd,
    },
    /// Launch the real-time peer status and event TUI
    Watch,
    /// Garbage collect expired blobs and transcripts
    Gc {
        /// Preview without deleting
        #[arg(long)]
        dry_run: bool,
        /// Override retention days (default: from config or 90)
        #[arg(long)]
        keep_days: Option<u32>,
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
        /// Also clean global transcript store
        #[arg(long)]
        global: bool,
        /// Archive blobs instead of deleting
        #[arg(long)]
        archive: bool,
        /// Purge expired archived blobs
        #[arg(long)]
        purge_archive: bool,
        /// Override archive retention days (default: from config or 180)
        #[arg(long)]
        archive_keep_days: Option<u32>,
        /// Also clean session ledgers, index files, and stale state files
        #[arg(long)]
        include_sessions: bool,
    },
}

#[derive(Subcommand)]
enum McpCommand {
    /// Start MCP server (stdio transport, JSON-RPC 2.0)
    Serve,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let repo_root = std::env::current_dir()?;

    match cli.cmd {
        Command::Init { no_hooks } => cmd_init::execute(&repo_root, no_hooks),
        Command::Note { text, role, tags } => cmd_note::execute(&repo_root, &text, &role, &tags),
        Command::Decide {
            decision,
            reason,
            session,
        } => cmd_bridge::decide(&repo_root, &decision, reason.as_deref(), session.as_deref()),
        Command::Claim {
            label,
            paths,
            session,
        } => cmd_bridge::claim(&repo_root, &label, &paths, session.as_deref()),
        Command::Request {
            to,
            message,
            session,
        } => cmd_bridge::request(&repo_root, &to, &message, session.as_deref()),
        Command::Ask {
            query,
            limit,
            json,
            all,
            branch,
        } => cmd_ask::execute(
            &repo_root,
            query.as_deref(),
            limit,
            json,
            all,
            branch.as_deref(),
        ),
        Command::Run { argv } => cmd_run::execute(&repo_root, &argv),
        Command::Status => cmd_status::execute(&repo_root),
        Command::Commit {
            title,
            purpose,
            contrib,
            evidence,
            labels,
            auto,
            dry_run,
            max_evidence,
        } => cmd_commit::execute(cmd_commit::CommitCliParams {
            repo_root: &repo_root,
            title: &title,
            purpose: purpose.as_deref(),
            contrib: contrib.as_deref(),
            evidence_args: &evidence,
            labels,
            auto,
            dry_run,
            max_evidence,
        }),
        Command::Log {
            event_type,
            family,
            tag,
            keyword,
            after,
            before,
            branch,
            limit,
            json,
        } => cmd_log::execute(&cmd_log::LogParams {
            repo_root: &repo_root,
            event_type: event_type.as_deref(),
            family: family.as_deref(),
            tag: tag.as_deref(),
            keyword: keyword.as_deref(),
            after: after.as_deref(),
            before: before.as_deref(),
            branch: branch.as_deref(),
            limit,
            json,
        }),
        Command::Context { branch, depth } => {
            cmd_context::execute(&repo_root, branch.as_deref(), depth)
        }
        Command::Rebuild {
            branch,
            all,
            reason,
        } => cmd_rebuild::execute(&repo_root, branch.as_deref(), all, &reason),
        Command::Branch { cmd } => cmd_branch::run(cmd, &repo_root),
        Command::Switch { name } => cmd_switch::execute(&repo_root, &name),
        Command::Merge { src, dst, reason } => cmd_merge::execute(&repo_root, &src, &dst, &reason),
        Command::Draft { cmd } => cmd_draft::run(cmd, &repo_root),
        Command::Bridge { cmd } => cmd_bridge::run_bridge(cmd, &repo_root),
        Command::Hook { cmd } => cmd_bridge::run_hook(cmd),
        Command::Doctor { cmd } => cmd_bridge::run_doctor(cmd, &repo_root),
        Command::Index { cmd } => cmd_bridge::run_index(cmd),
        Command::Config { cmd } => cmd_config::run(cmd, &repo_root),
        Command::Pattern { cmd } => cmd_pattern::run(cmd, &repo_root),
        Command::Mcp { cmd } => match cmd {
            McpCommand::Serve => {
                tokio::runtime::Runtime::new()?.block_on(edda_mcp::serve(&repo_root))?;
                Ok(())
            }
        },
        Command::Search { cmd } => cmd_search::run_cmd(cmd, &repo_root),
        Command::Blob { cmd } => cmd_blob::run(cmd, &repo_root),
        Command::Plan { cmd } => cmd_plan::run(cmd, &repo_root),
        Command::Conduct { cmd } => cmd_conduct::run_cmd(cmd, &repo_root),
        Command::Watch => cmd_watch::execute(&repo_root),
        Command::Gc {
            dry_run,
            keep_days,
            force,
            global,
            archive,
            purge_archive,
            archive_keep_days,
            include_sessions,
        } => cmd_gc::execute(&cmd_gc::GcParams {
            repo_root: &repo_root,
            dry_run,
            keep_days,
            force,
            global,
            archive,
            purge_archive,
            archive_keep_days,
            include_sessions,
        }),
    }
}
