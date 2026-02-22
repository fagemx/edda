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
        cmd: BranchCmd,
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
        cmd: DraftCmd,
    },
    /// Bridge operations (install/uninstall hooks for Claude Code)
    Bridge {
        #[command(subcommand)]
        cmd: BridgeCmd,
    },
    /// Hook entrypoint (called by Claude Code hooks)
    Hook {
        #[command(subcommand)]
        cmd: HookCmd,
    },
    /// Health check for bridge integration
    Doctor {
        #[command(subcommand)]
        cmd: DoctorCmd,
    },
    /// Index operations
    Index {
        #[command(subcommand)]
        cmd: IndexCmd,
    },
    /// Read or write workspace config (.edda/config.json)
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Manage pattern store (.edda/patterns/)
    Pattern {
        #[command(subcommand)]
        cmd: PatternCmd,
    },
    /// MCP server operations
    Mcp {
        #[command(subcommand)]
        cmd: McpCommand,
    },
    /// Full-text search (Tantivy)
    Search {
        #[command(subcommand)]
        cmd: SearchCmd,
    },
    /// Manage blob metadata (classify, pin, unpin, info, stats)
    Blob {
        #[command(subcommand)]
        cmd: BlobCmd,
    },
    /// Plan scaffolding and templates
    Plan {
        #[command(subcommand)]
        cmd: PlanCmd,
    },
    /// Multi-phase AI plan conductor
    Conduct {
        #[command(subcommand)]
        cmd: ConductCmd,
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
enum BranchCmd {
    /// Create a new branch
    Create {
        /// Branch name
        name: String,
        /// Purpose of this branch
        #[arg(short = 'm', long = "purpose")]
        purpose: String,
    },
}

#[derive(Subcommand)]
enum PlanCmd {
    /// Scan codebase and suggest a plan
    Scan {
        /// High-level intent for the plan (injected into purpose field)
        #[arg(long)]
        purpose: Option<String>,
    },
    /// Generate plan.yaml from built-in template
    Init {
        /// Template name (rust-cli, rust-lib, python-api, node-app, fullstack, minimal)
        template: Option<String>,
        /// Output file path
        #[arg(short, long, default_value = "plan.yaml")]
        output: String,
    },
}

#[derive(Subcommand)]
enum BridgeCmd {
    /// Claude Code bridge operations
    Claude {
        #[command(subcommand)]
        cmd: BridgeClaudeCmd,
    },
    /// OpenClaw bridge operations
    Openclaw {
        #[command(subcommand)]
        cmd: BridgeOpenclawCmd,
    },
}

#[derive(Subcommand)]
enum BridgeClaudeCmd {
    /// Install edda hooks into .claude/settings.local.json
    Install {
        /// Skip writing edda section to .claude/CLAUDE.md
        #[arg(long)]
        no_claude_md: bool,
    },
    /// Uninstall edda hooks from .claude/settings.local.json
    Uninstall,
    /// Manually digest a session into workspace ledger
    Digest {
        /// Session ID to digest
        #[arg(long)]
        session: Option<String>,
        /// Digest all pending sessions
        #[arg(long)]
        all: bool,
    },
    /// Show active peer sessions for current project
    Peers,
    /// Claim a scope for coordination (e.g. "auth", "billing")
    Claim {
        /// Short label for this session's scope
        label: String,
        /// File path patterns this scope covers (e.g. "src/auth/*")
        #[arg(long)]
        paths: Vec<String>,
        /// Session ID (auto-inferred from active heartbeats if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Record a binding decision for all sessions
    Decide {
        /// Decision in key=value format (e.g. "auth.method=JWT RS256")
        decision: String,
        /// Reason for the decision
        #[arg(long)]
        reason: Option<String>,
        /// Session ID (auto-inferred from active heartbeats if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Send a request to another session
    Request {
        /// Target session label
        to: String,
        /// Request message
        message: String,
        /// Session ID (auto-inferred from active heartbeats if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Render write-back protocol (static teaching text)
    RenderWriteback,
    /// Render workspace context from .edda/ ledger
    RenderWorkspace {
        /// Max chars budget
        #[arg(long, default_value = "2500")]
        budget: usize,
    },
    /// Render L2 coordination protocol
    RenderCoordination {
        /// Session ID (auto-inferred if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Render hot pack (recent turns summary, reads last-built pack)
    RenderPack,
    /// Render active plan excerpt
    RenderPlan,
    /// Write session heartbeat for peer discovery
    HeartbeatWrite {
        /// Session label (e.g. "auth", "billing")
        #[arg(long)]
        label: String,
        /// Session ID (auto-inferred if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Touch heartbeat timestamp (liveness ping)
    HeartbeatTouch {
        /// Session ID (auto-inferred if omitted)
        #[arg(long)]
        session: Option<String>,
    },
    /// Remove session heartbeat
    HeartbeatRemove {
        /// Session ID (auto-inferred if omitted)
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Subcommand)]
enum BridgeOpenclawCmd {
    /// Install edda OpenClaw plugin
    Install {
        /// Custom target directory (default: ~/.openclaw/extensions/edda-bridge/)
        #[arg(long)]
        target: Option<String>,
    },
    /// Uninstall edda OpenClaw plugin
    Uninstall {
        /// Custom target directory
        #[arg(long)]
        target: Option<String>,
    },
    /// Manually digest a session into workspace ledger
    Digest {
        /// Session ID to digest
        #[arg(long)]
        session: Option<String>,
        /// Digest all pending sessions
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    /// Claude Code hook entrypoint (reads stdin JSON)
    Claude,
    /// OpenClaw hook entrypoint (reads stdin JSON)
    Openclaw,
}

#[derive(Subcommand)]
enum DoctorCmd {
    /// Check Claude Code bridge health
    Claude,
    /// Check OpenClaw bridge health
    Openclaw,
}

#[derive(Subcommand)]
enum IndexCmd {
    /// Verify index entries match store records
    Verify {
        /// Project ID
        #[arg(long)]
        project: String,
        /// Session ID
        #[arg(long)]
        session: String,
        /// Number of records to sample
        #[arg(long, default_value_t = 50)]
        sample: usize,
        /// Check all records
        #[arg(long)]
        all: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Set a config value
    Set {
        /// Config key (e.g. skill_guide)
        key: String,
        /// Config value (true/false/number/string)
        value: String,
    },
    /// Get a config value
    Get {
        /// Config key
        key: String,
    },
    /// List all config values
    List,
}

#[derive(Subcommand)]
enum PatternCmd {
    /// Add a new pattern
    Add {
        /// Pattern ID (e.g. test-no-db)
        #[arg(long)]
        id: String,
        /// File glob patterns (repeatable)
        #[arg(long = "glob")]
        globs: Vec<String>,
        /// Rule description
        #[arg(long)]
        rule: String,
        /// Source reference (e.g. "PR #2587")
        #[arg(long, default_value = "")]
        source: String,
    },
    /// Remove a pattern
    Remove {
        /// Pattern ID
        id: String,
    },
    /// List all patterns
    List,
    /// Test which patterns match a file path
    Test {
        /// File path to test
        file_path: String,
    },
}

#[derive(Subcommand)]
enum DraftCmd {
    /// Create a draft commit (does not write to ledger)
    Propose {
        /// Draft title
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
        /// Maximum number of auto-evidence items
        #[arg(long, default_value_t = 20)]
        max_evidence: usize,
    },
    /// Show a draft by ID
    Show {
        /// Draft ID (drf_*)
        id: String,
    },
    /// List all drafts
    List {
        /// Output as JSON lines (one object per draft)
        #[arg(long)]
        json: bool,
    },
    /// Apply a draft commit to the ledger (with rebase)
    Apply {
        /// Draft ID (drf_*)
        id: String,
        /// Preview without writing to ledger
        #[arg(long)]
        dry_run: bool,
        /// Delete draft after successful apply
        #[arg(long)]
        delete: bool,
    },
    /// Delete a draft
    Delete {
        /// Draft ID (drf_*)
        id: String,
    },
    /// Approve a draft
    Approve {
        /// Draft ID (drf_*)
        id: String,
        /// Actor name
        #[arg(long, default_value = "human")]
        by: String,
        /// Approval note
        #[arg(long, default_value = "")]
        note: String,
        /// Stage ID (required for multi-stage drafts)
        #[arg(long)]
        stage: Option<String>,
    },
    /// Reject a draft
    Reject {
        /// Draft ID (drf_*)
        id: String,
        /// Actor name
        #[arg(long, default_value = "human")]
        by: String,
        /// Rejection note
        #[arg(long, default_value = "")]
        note: String,
        /// Stage ID (required for multi-stage drafts)
        #[arg(long)]
        stage: Option<String>,
    },
    /// Show pending approval items
    Inbox {
        /// Filter by actor name
        #[arg(long)]
        by: Option<String>,
        /// Filter by role
        #[arg(long)]
        role: Option<String>,
        /// Output as JSON lines (one object per item)
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum McpCommand {
    /// Start MCP server (stdio transport, JSON-RPC 2.0)
    Serve,
}

#[derive(Subcommand)]
enum BlobCmd {
    /// Classify a blob (artifact, decision_evidence, trace_noise)
    Classify {
        /// Blob hash or prefix
        hash: String,
        /// Classification: artifact, decision_evidence, trace_noise
        #[arg(long)]
        class: String,
    },
    /// Pin a blob (prevent GC from removing it)
    Pin {
        /// Blob hash or prefix
        hash: String,
    },
    /// Unpin a blob (allow GC to remove it)
    Unpin {
        /// Blob hash or prefix
        hash: String,
    },
    /// Show blob info (hash, size, class, pinned, location)
    Info {
        /// Blob hash or prefix
        hash: String,
    },
    /// Show blob store statistics
    Stats,
    /// List tombstones (deleted blob records)
    Tombstones,
}

#[derive(Subcommand)]
enum SearchCmd {
    /// Build or update search index (Tantivy)
    Index {
        /// Project ID (defaults to current repo)
        #[arg(long)]
        project: Option<String>,
        /// Session ID (index single session instead of all)
        #[arg(long)]
        session: Option<String>,
    },
    /// Search for events and transcript turns
    Query {
        /// Search query (supports fuzzy, "exact", /regex/)
        query: String,
        /// Project ID (defaults to current repo)
        #[arg(long)]
        project: Option<String>,
        /// Session ID filter
        #[arg(long)]
        session: Option<String>,
        /// Filter by document type: event or turn
        #[arg(long, name = "type")]
        doc_type: Option<String>,
        /// Filter by event type: note, commit, merge, etc.
        #[arg(long)]
        event_type: Option<String>,
        /// Exact match (disable fuzzy)
        #[arg(long)]
        exact: bool,
        /// Maximum results (default: 20)
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Show full content of a specific turn
    Show {
        /// Turn ID (from search results)
        #[arg(long)]
        turn: String,
        /// Project ID (defaults to current repo)
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum ConductCmd {
    /// Run a plan from a YAML file
    Run {
        /// Path to plan.yaml
        plan_file: String,
        /// Override working directory
        #[arg(long)]
        cwd: Option<String>,
        /// Preview plan without executing
        #[arg(long)]
        dry_run: bool,
        /// Suppress live agent activity output
        #[arg(short, long)]
        quiet: bool,
    },
    /// Show status of running/completed plans
    Status {
        /// Plan name (auto-detects if only one)
        plan_name: Option<String>,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Reset a failed/stale phase to Pending
    Retry {
        /// Phase ID to retry
        phase_id: String,
        /// Plan name (auto-detects if only one)
        #[arg(long)]
        plan: Option<String>,
    },
    /// Skip a failed/stale/pending phase
    Skip {
        /// Phase ID to skip
        phase_id: String,
        /// Reason for skipping
        #[arg(long)]
        reason: Option<String>,
        /// Plan name (auto-detects if only one)
        #[arg(long)]
        plan: Option<String>,
    },
    /// Abort a running plan
    Abort {
        /// Plan name (auto-detects if only one)
        plan_name: Option<String>,
    },
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
        Command::Branch { cmd } => match cmd {
            BranchCmd::Create { name, purpose } => cmd_branch::create(&repo_root, &name, &purpose),
        },
        Command::Switch { name } => cmd_switch::execute(&repo_root, &name),
        Command::Merge { src, dst, reason } => cmd_merge::execute(&repo_root, &src, &dst, &reason),
        Command::Draft { cmd } => match cmd {
            DraftCmd::Propose {
                title,
                purpose,
                contrib,
                evidence,
                labels,
                auto,
                max_evidence,
            } => cmd_draft::propose(cmd_draft::ProposeParams {
                repo_root: &repo_root,
                title: &title,
                purpose: purpose.as_deref(),
                contrib: contrib.as_deref(),
                evidence_args: &evidence,
                labels,
                auto,
                max_evidence,
            }),
            DraftCmd::Show { id } => cmd_draft::show(&repo_root, &id),
            DraftCmd::List { json } => cmd_draft::list(&repo_root, json),
            DraftCmd::Apply {
                id,
                dry_run,
                delete,
            } => cmd_draft::apply(&repo_root, &id, dry_run, delete),
            DraftCmd::Delete { id } => cmd_draft::delete(&repo_root, &id),
            DraftCmd::Approve {
                id,
                by,
                note,
                stage,
            } => cmd_draft::approve(&repo_root, &id, &by, &note, stage.as_deref()),
            DraftCmd::Reject {
                id,
                by,
                note,
                stage,
            } => cmd_draft::reject(&repo_root, &id, &by, &note, stage.as_deref()),
            DraftCmd::Inbox { by, role, json } => {
                cmd_draft::inbox(&repo_root, by.as_deref(), role.as_deref(), json)
            }
        },
        Command::Bridge { cmd } => match cmd {
            BridgeCmd::Claude { cmd } => match cmd {
                BridgeClaudeCmd::Install { no_claude_md } => {
                    cmd_bridge::install(&repo_root, no_claude_md)
                }
                BridgeClaudeCmd::Uninstall => cmd_bridge::uninstall(&repo_root),
                BridgeClaudeCmd::Digest { session, all } => {
                    cmd_bridge::digest(&repo_root, session.as_deref(), all)
                }
                BridgeClaudeCmd::Peers => cmd_bridge::peers(&repo_root),
                BridgeClaudeCmd::Claim {
                    label,
                    paths,
                    session,
                } => cmd_bridge::claim(&repo_root, &label, &paths, session.as_deref()),
                BridgeClaudeCmd::Decide {
                    decision,
                    reason,
                    session,
                } => {
                    cmd_bridge::decide(&repo_root, &decision, reason.as_deref(), session.as_deref())
                }
                BridgeClaudeCmd::Request {
                    to,
                    message,
                    session,
                } => cmd_bridge::request(&repo_root, &to, &message, session.as_deref()),
                BridgeClaudeCmd::RenderWriteback => cmd_bridge::render_writeback(),
                BridgeClaudeCmd::RenderWorkspace { budget } => {
                    cmd_bridge::render_workspace(&repo_root, budget)
                }
                BridgeClaudeCmd::RenderCoordination { session } => {
                    cmd_bridge::render_coordination(&repo_root, session.as_deref())
                }
                BridgeClaudeCmd::RenderPack => cmd_bridge::render_pack(&repo_root),
                BridgeClaudeCmd::RenderPlan => cmd_bridge::render_plan(&repo_root),
                BridgeClaudeCmd::HeartbeatWrite { label, session } => {
                    cmd_bridge::heartbeat_write(&repo_root, &label, session.as_deref())
                }
                BridgeClaudeCmd::HeartbeatTouch { session } => {
                    cmd_bridge::heartbeat_touch(&repo_root, session.as_deref())
                }
                BridgeClaudeCmd::HeartbeatRemove { session } => {
                    cmd_bridge::heartbeat_remove(&repo_root, session.as_deref())
                }
            },
            BridgeCmd::Openclaw { cmd } => match cmd {
                BridgeOpenclawCmd::Install { target } => {
                    cmd_bridge::install_openclaw(target.as_deref().map(std::path::Path::new))
                }
                BridgeOpenclawCmd::Uninstall { target } => {
                    cmd_bridge::uninstall_openclaw(target.as_deref().map(std::path::Path::new))
                }
                BridgeOpenclawCmd::Digest { session, all } => {
                    cmd_bridge::digest(&repo_root, session.as_deref(), all)
                }
            },
        },
        Command::Hook { cmd } => match cmd {
            HookCmd::Claude => cmd_bridge::hook_claude(),
            HookCmd::Openclaw => cmd_bridge::hook_openclaw(),
        },
        Command::Doctor { cmd } => match cmd {
            DoctorCmd::Claude => cmd_bridge::doctor(&repo_root),
            DoctorCmd::Openclaw => cmd_bridge::doctor_openclaw(),
        },
        Command::Config { cmd } => match cmd {
            ConfigCmd::Set { key, value } => cmd_config::set(&repo_root, &key, &value),
            ConfigCmd::Get { key } => cmd_config::get(&repo_root, &key),
            ConfigCmd::List => cmd_config::list(&repo_root),
        },
        Command::Pattern { cmd } => match cmd {
            PatternCmd::Add {
                id,
                globs,
                rule,
                source,
            } => cmd_pattern::add(&repo_root, &id, &globs, &rule, &source),
            PatternCmd::Remove { id } => cmd_pattern::remove(&repo_root, &id),
            PatternCmd::List => cmd_pattern::list(&repo_root),
            PatternCmd::Test { file_path } => cmd_pattern::test(&repo_root, &file_path),
        },
        Command::Index { cmd } => match cmd {
            IndexCmd::Verify {
                project,
                session,
                sample,
                all,
            } => cmd_bridge::index_verify(&project, &session, sample, all),
        },
        Command::Mcp { cmd } => match cmd {
            McpCommand::Serve => {
                tokio::runtime::Runtime::new()?.block_on(edda_mcp::serve(&repo_root))?;
                Ok(())
            }
        },
        Command::Plan { cmd } => match cmd {
            PlanCmd::Scan { purpose } => cmd_plan::scan(&repo_root, purpose.as_deref()),
            PlanCmd::Init { template, output } => {
                cmd_plan::init(&repo_root, template.as_deref(), &output)
            }
        },
        Command::Conduct { cmd } => match cmd {
            ConductCmd::Run {
                plan_file,
                cwd,
                dry_run,
                quiet,
            } => cmd_conduct::run(
                std::path::Path::new(&plan_file),
                cwd.as_deref().map(std::path::Path::new),
                dry_run,
                !quiet,
            ),
            ConductCmd::Status { plan_name, json } => {
                cmd_conduct::status(&repo_root, plan_name.as_deref(), json)
            }
            ConductCmd::Retry { phase_id, plan } => {
                cmd_conduct::retry(&repo_root, &phase_id, plan.as_deref())
            }
            ConductCmd::Skip {
                phase_id,
                reason,
                plan,
            } => cmd_conduct::skip(&repo_root, &phase_id, reason.as_deref(), plan.as_deref()),
            ConductCmd::Abort { plan_name } => cmd_conduct::abort(&repo_root, plan_name.as_deref()),
        },
        Command::Watch => cmd_watch::execute(&repo_root),
        Command::Blob { cmd } => match cmd {
            BlobCmd::Classify { hash, class } => cmd_blob::classify(&repo_root, &hash, &class),
            BlobCmd::Pin { hash } => cmd_blob::pin(&repo_root, &hash),
            BlobCmd::Unpin { hash } => cmd_blob::unpin(&repo_root, &hash),
            BlobCmd::Info { hash } => cmd_blob::info(&repo_root, &hash),
            BlobCmd::Stats => cmd_blob::stats(&repo_root),
            BlobCmd::Tombstones => cmd_blob::tombstones(&repo_root),
        },
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
        Command::Search { cmd } => {
            let default_pid = cmd_search::resolve_project_id(&repo_root);
            match cmd {
                SearchCmd::Index { project, session } => {
                    let pid = project.as_deref().unwrap_or(&default_pid);
                    cmd_search::index(&repo_root, pid, session.as_deref())
                }
                SearchCmd::Query {
                    query,
                    project,
                    session,
                    doc_type,
                    event_type,
                    exact,
                    limit,
                } => {
                    let pid = project.as_deref().unwrap_or(&default_pid);
                    cmd_search::query(
                        pid,
                        &query,
                        session.as_deref(),
                        doc_type.as_deref(),
                        event_type.as_deref(),
                        exact,
                        limit,
                    )
                }
                SearchCmd::Show { turn, project } => {
                    let pid = project.as_deref().unwrap_or(&default_pid);
                    cmd_search::show(pid, &turn)
                }
            }
        }
    }
}
