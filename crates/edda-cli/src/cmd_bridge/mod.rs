use clap::Subcommand;
use std::path::Path;

mod bg_review;
mod claim;
mod claude;
mod decide;
mod peers;
mod render;
mod request;
mod vendors;

#[cfg(test)]
mod tests;

pub use bg_review::bg_review;
pub use claim::{claim, unclaim};
pub use claude::{digest, doctor, hook_claude, index_verify, install, uninstall};
pub use decide::{decide, ratify};
pub use peers::peers;
pub use render::{
    heartbeat_remove, heartbeat_touch, heartbeat_write, render_coordination, render_fleet,
    render_pack, render_plan, render_workspace, render_writeback,
};
pub use request::{request, request_ack};
pub use vendors::{
    doctor_codex, doctor_cursor, doctor_hermes, doctor_openclaw, hook_codex, hook_cursor,
    hook_hermes, hook_openclaw, install_codex, install_cursor, install_hermes, install_openclaw,
    uninstall_codex, uninstall_cursor, uninstall_hermes, uninstall_openclaw,
};

pub(crate) use request::resolve_session_id;

// ── CLI Schema ──

#[derive(Subcommand)]
pub enum BridgeCmd {
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
    /// Codex CLI bridge operations
    Codex {
        #[command(subcommand)]
        cmd: BridgeCodexCmd,
    },
    /// Hermes agent bridge operations
    Hermes {
        #[command(subcommand)]
        cmd: BridgeHermesCmd,
    },
    /// Cursor IDE bridge operations
    Cursor {
        #[command(subcommand)]
        cmd: BridgeCursorCmd,
    },
}

#[derive(Subcommand)]
pub enum BridgeCursorCmd {
    /// Install edda hooks into ~/.cursor/hooks.json
    Install {
        /// Custom hooks.json path (default: ~/.cursor/hooks.json)
        #[arg(long)]
        target: Option<String>,
    },
    /// Uninstall edda hooks from Cursor hooks.json
    Uninstall {
        /// Custom hooks.json path
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum BridgeCodexCmd {
    /// Install edda hooks into ~/.codex/hooks.json
    Install {
        /// Custom hooks.json path (default: ~/.codex/hooks.json)
        #[arg(long)]
        target: Option<String>,
    },
    /// Uninstall edda hooks from Codex hooks.json
    Uninstall {
        /// Custom hooks.json path
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum BridgeHermesCmd {
    /// Install edda hooks into ~/.hermes/cli-config.yaml (merges into existing hooks: block)
    Install {
        /// Custom cli-config.yaml path (default: ~/.hermes/cli-config.yaml)
        #[arg(long)]
        target: Option<String>,
    },
    /// Uninstall edda hooks from Hermes cli-config.yaml
    Uninstall {
        /// Custom cli-config.yaml path
        #[arg(long)]
        target: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum BridgeClaudeCmd {
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
    Peers {
        /// Output sessions, claims, requests, and acknowledgements as JSON
        #[arg(long)]
        json: bool,
    },
    /// Claim a scope for coordination (e.g. "auth", "billing")
    Claim {
        /// Short label for this session's scope
        label: String,
        /// File path patterns this scope covers (e.g. "src/auth/*")
        #[arg(long)]
        paths: Vec<String>,
        /// Process object or subject this scope covers (e.g. "pr:570", "release:v0.4.1")
        #[arg(long)]
        subject: Option<String>,
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
    },
    /// Release this session's coordination scope
    Unclaim {
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
        /// Exit 0 when there is nothing to release, for unconditional teardown
        #[arg(long)]
        if_claimed: bool,
    },
    /// Record a decision — agent-authored, unratified until `edda ratify`
    Decide {
        /// Decision in key=value format (e.g. "auth.method=JWT RS256")
        decision: String,
        /// Reason for the decision
        #[arg(long)]
        reason: Option<String>,
        /// Decision keys this decision depends on (repeatable)
        #[arg(long = "refs")]
        refs: Vec<String>,
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
        /// File glob patterns this decision governs (repeatable)
        #[arg(long = "paths")]
        paths: Vec<String>,
        /// Comma-separated tags for this decision
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
    },
    /// Send a request to another session
    Request {
        /// Target session label
        to: String,
        /// Request message
        message: String,
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
        /// Send even when no active session answers to the target label
        #[arg(long)]
        force: bool,
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
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
    },
    /// Render hot pack (recent turns summary, reads last-built pack)
    RenderPack,
    /// Render the Fleet section — sibling projects' rulings and waiting work
    RenderFleet {
        /// Max chars budget
        #[arg(long, default_value = "800")]
        budget: usize,
    },
    /// Render active plan excerpt
    RenderPlan,
    /// Write session heartbeat for peer discovery
    HeartbeatWrite {
        /// Session label (e.g. "auth", "billing")
        #[arg(long)]
        label: String,
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
    },
    /// Touch heartbeat timestamp (liveness ping)
    HeartbeatTouch {
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
    },
    /// Remove session heartbeat
    HeartbeatRemove {
        /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
        #[arg(long)]
        session: Option<String>,
    },
    /// Review background-extracted draft decisions
    BgReview {
        /// List all pending draft decisions
        #[arg(long)]
        list: bool,
        /// Accept decisions for a session (comma-separated indices)
        #[arg(long)]
        accept: Option<String>,
        /// Reject decisions for a session (comma-separated indices)
        #[arg(long)]
        reject: Option<String>,
        /// Accept all pending decisions for a session
        #[arg(long)]
        accept_all: bool,
        /// Session ID to review
        #[arg(long)]
        session: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum BridgeOpenclawCmd {
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
pub enum HookCmd {
    /// Claude Code hook entrypoint (reads stdin JSON)
    Claude,
    /// Codex CLI hook entrypoint (reads stdin JSON)
    Codex,
    /// Hermes agent shell-hook entrypoint (reads stdin JSON)
    Hermes,
    /// OpenClaw hook entrypoint (reads stdin JSON)
    Openclaw,
    /// Cursor IDE hook entrypoint (reads stdin JSON)
    Cursor,
}

#[derive(Subcommand)]
pub enum DoctorCmd {
    /// Check Claude Code bridge health
    Claude,
    /// Check Codex bridge health
    Codex,
    /// Check Hermes bridge health
    Hermes,
    /// Check OpenClaw bridge health
    Openclaw,
    /// Check Cursor bridge health
    Cursor,
}

#[derive(Subcommand)]
pub enum IndexCmd {
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

// ── Dispatch ──

pub fn run_bridge(cmd: BridgeCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        BridgeCmd::Claude { cmd } => match cmd {
            BridgeClaudeCmd::Install { no_claude_md } => install(repo_root, no_claude_md),
            BridgeClaudeCmd::Uninstall => uninstall(repo_root),
            BridgeClaudeCmd::Digest { session, all } => digest(repo_root, session.as_deref(), all),
            BridgeClaudeCmd::Peers { json } => peers(repo_root, json),
            BridgeClaudeCmd::Claim {
                label,
                paths,
                subject,
                session,
            } => claim(
                repo_root,
                &label,
                &paths,
                subject.as_deref(),
                session.as_deref(),
            ),
            BridgeClaudeCmd::Unclaim {
                session,
                if_claimed,
            } => unclaim(repo_root, session.as_deref(), if_claimed),
            BridgeClaudeCmd::Decide {
                decision,
                reason,
                refs,
                session,
                paths,
                tags,
            } => decide(
                repo_root,
                &decision,
                reason.as_deref(),
                &refs,
                session.as_deref(),
                None,
                &paths,
                &tags,
            ),
            BridgeClaudeCmd::Request {
                to,
                message,
                session,
                force,
            } => request(repo_root, &to, &message, session.as_deref(), force),
            BridgeClaudeCmd::RenderWriteback => render_writeback(),
            BridgeClaudeCmd::RenderWorkspace { budget } => render_workspace(repo_root, budget),
            BridgeClaudeCmd::RenderCoordination { session } => {
                render_coordination(repo_root, session.as_deref())
            }
            BridgeClaudeCmd::RenderPack => render_pack(repo_root),
            BridgeClaudeCmd::RenderFleet { budget } => render_fleet(repo_root, budget),
            BridgeClaudeCmd::RenderPlan => render_plan(repo_root),
            BridgeClaudeCmd::HeartbeatWrite { label, session } => {
                heartbeat_write(repo_root, &label, session.as_deref())
            }
            BridgeClaudeCmd::HeartbeatTouch { session } => {
                heartbeat_touch(repo_root, session.as_deref())
            }
            BridgeClaudeCmd::HeartbeatRemove { session } => {
                heartbeat_remove(repo_root, session.as_deref())
            }
            BridgeClaudeCmd::BgReview {
                list,
                accept,
                reject,
                accept_all,
                session,
            } => bg_review(repo_root, list, accept, reject, accept_all, session),
        },
        BridgeCmd::Openclaw { cmd } => match cmd {
            BridgeOpenclawCmd::Install { target } => {
                install_openclaw(target.as_deref().map(std::path::Path::new))
            }
            BridgeOpenclawCmd::Uninstall { target } => {
                uninstall_openclaw(target.as_deref().map(std::path::Path::new))
            }
            BridgeOpenclawCmd::Digest { session, all } => {
                digest(repo_root, session.as_deref(), all)
            }
        },
        BridgeCmd::Codex { cmd } => match cmd {
            BridgeCodexCmd::Install { target } => {
                install_codex(target.as_deref().map(std::path::Path::new))
            }
            BridgeCodexCmd::Uninstall { target } => {
                uninstall_codex(target.as_deref().map(std::path::Path::new))
            }
        },
        BridgeCmd::Hermes { cmd } => match cmd {
            BridgeHermesCmd::Install { target } => {
                install_hermes(target.as_deref().map(std::path::Path::new))
            }
            BridgeHermesCmd::Uninstall { target } => {
                uninstall_hermes(target.as_deref().map(std::path::Path::new))
            }
        },
        BridgeCmd::Cursor { cmd } => match cmd {
            BridgeCursorCmd::Install { target } => {
                install_cursor(target.as_deref().map(std::path::Path::new))
            }
            BridgeCursorCmd::Uninstall { target } => {
                uninstall_cursor(target.as_deref().map(std::path::Path::new))
            }
        },
    }
}

pub fn run_hook(cmd: HookCmd) -> anyhow::Result<()> {
    match cmd {
        HookCmd::Claude => hook_claude(),
        HookCmd::Codex => hook_codex(),
        HookCmd::Hermes => hook_hermes(),
        HookCmd::Openclaw => hook_openclaw(),
        HookCmd::Cursor => hook_cursor(),
    }
}

pub fn run_doctor(cmd: DoctorCmd, repo_root: &Path) -> anyhow::Result<()> {
    match cmd {
        DoctorCmd::Claude => doctor(repo_root),
        DoctorCmd::Codex => doctor_codex(),
        DoctorCmd::Hermes => doctor_hermes(),
        DoctorCmd::Openclaw => doctor_openclaw(),
        DoctorCmd::Cursor => doctor_cursor(),
    }
}

pub fn run_index(cmd: IndexCmd) -> anyhow::Result<()> {
    match cmd {
        IndexCmd::Verify {
            project,
            session,
            sample,
            all,
        } => index_verify(&project, &session, sample, all),
    }
}
