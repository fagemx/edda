use anyhow::Context;
use clap::Subcommand;
use std::io::Read;
use std::path::Path;

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
                session,
            } => claim(repo_root, &label, &paths, session.as_deref()),
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

// ── Command Implementations ──

/// `edda bridge claude install`
pub fn install(repo_root: &Path, no_claude_md: bool) -> anyhow::Result<()> {
    edda_bridge_claude::install(repo_root, no_claude_md)
}

/// `edda bridge claude uninstall`
pub fn uninstall(repo_root: &Path) -> anyhow::Result<()> {
    edda_bridge_claude::uninstall(repo_root)
}

/// `edda hook claude` — read stdin, dispatch hook
///
/// Resilience: catch_unwind + configurable timeout (EDDA_HOOK_TIMEOUT_MS).
/// On panic or timeout, exits 0 — never blocks the host agent.
pub fn hook_claude() -> anyhow::Result<()> {
    run_hook_resilient("", |stdin| {
        let r = edda_bridge_claude::hook_entrypoint_from_stdin(&stdin)?;
        Ok((r.stdout, r.stderr))
    })
}

/// Shared resilience wrapper: read stdin, spawn worker with catch_unwind + timeout.
///
/// `prefix` is prepended to debug log messages (e.g., `""` for Claude, `"OPENCLAW "` for OpenClaw).
/// `entrypoint` receives the stdin string and returns (stdout, stderr).
fn run_hook_resilient<F>(prefix: &str, entrypoint: F) -> anyhow::Result<()>
where
    F: FnOnce(String) -> anyhow::Result<(Option<String>, Option<String>)> + Send + 'static,
{
    let mut stdin_buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut stdin_buf) {
        debug_log(&format!("{prefix}STDIN READ ERROR: {e}"));
        return Ok(());
    }

    debug_log(&format!(
        "{prefix}STDIN({} bytes): {}",
        stdin_buf.len(),
        &stdin_buf[..stdin_buf.len().min(200)]
    ));

    let timeout_ms = hook_timeout_ms();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| entrypoint(stdin_buf)));
        let _ = tx.send(result);
    });

    match rx.recv_timeout(std::time::Duration::from_millis(timeout_ms)) {
        Ok(Ok(Ok((stdout, stderr)))) => {
            if let Some(output) = &stdout {
                debug_log(&format!("{prefix}OK output({} bytes)", output.len()));
                print!("{output}");
            }
            if let Some(warning) = &stderr {
                debug_log(&format!("{prefix}WARNING: {warning}"));
                eprintln!("{warning}");
                // Exit 1 = non-blocking warning; Claude Code shows stderr to user
                // but does not feed it to the model or block the conversation.
                std::process::exit(1);
            }
            if stdout.is_none() && stderr.is_none() {
                debug_log(&format!("{prefix}OK (no output)"));
            }
            Ok(())
        }
        Ok(Ok(Err(e))) => {
            debug_log(&format!("{prefix}ERROR: {e}"));
            Ok(())
        }
        Ok(Err(panic_info)) => {
            let msg = panic_info
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_info.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            debug_log(&format!("{prefix}PANIC: {msg}"));
            Ok(())
        }
        Err(_) => {
            debug_log(&format!(
                "{prefix}TIMEOUT after {timeout_ms}ms — graceful exit"
            ));
            Ok(())
        }
    }
}

/// Hook timeout in milliseconds. Configurable via `EDDA_HOOK_TIMEOUT_MS` (default: 60s).
///
/// Raised from 10s to 60s to accommodate SessionEnd background threads that
/// make LLM API calls (bg_extract, bg_digest, bg_scan, bg_detect).  See #287.
fn hook_timeout_ms() -> u64 {
    std::env::var("EDDA_HOOK_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60_000)
}

fn debug_log(msg: &str) {
    if std::env::var_os("EDDA_DEBUG").is_none() {
        return;
    }
    use std::io::Write;
    let log_path = std::env::temp_dir().join("edda-hook-debug.log");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    {
        let ts = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let _ = writeln!(f, "[{ts}] {msg}");
    }
}

/// `edda doctor claude`
pub fn doctor(repo_root: &Path) -> anyhow::Result<()> {
    edda_bridge_claude::doctor(repo_root)
}

/// JSON board snapshot for `edda peers --json`.
fn peers_json(project_id: &str) -> serde_json::Value {
    let stale_threshold = edda_bridge_claude::peers::stale_secs();
    let sessions: Vec<serde_json::Value> =
        edda_bridge_claude::peers::discover_all_sessions(project_id)
            .into_iter()
            .map(|peer| {
                let stale = peer.age_secs > stale_threshold;
                let mut value = serde_json::to_value(&peer).unwrap_or_default();
                value["stale"] = serde_json::json!(stale);
                value
            })
            .collect();
    let board = edda_bridge_claude::peers::compute_board_state(project_id);
    // GH-569: claims are part of the JSON surface programs consume, so each
    // carries its age and a stale flag — otherwise a 55-day-old zombie claim
    // and a 37-second-old live claim are indistinguishable to a program.
    let now_epoch = time::OffsetDateTime::now_utc().unix_timestamp();
    let claims: Vec<serde_json::Value> = board
        .claims
        .iter()
        .map(|claim| {
            let mut value = serde_json::to_value(claim).unwrap_or_default();
            let ts_epoch = time::OffsetDateTime::parse(
                &claim.ts,
                &time::format_description::well_known::Rfc3339,
            )
            .map(|t| t.unix_timestamp())
            .unwrap_or(0);
            let age_secs = (now_epoch - ts_epoch).max(0) as u64;
            value["age_secs"] = serde_json::json!(age_secs);
            value["stale"] = serde_json::json!(age_secs > stale_threshold);
            value
        })
        .collect();
    serde_json::json!({
        "sessions": sessions,
        "claims": claims,
        "requests": board.requests,
        "acks": board.request_acks,
    })
}

/// `edda bridge claude peers` — show active peer sessions
pub fn peers(repo_root: &Path, json: bool) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&peers_json(&project_id))?
        );
        return Ok(());
    }
    let sessions = edda_bridge_claude::peers::discover_all_sessions(&project_id);

    if sessions.is_empty() {
        println!("No active sessions.");
        return Ok(());
    }

    // Collapse stale sessions (heartbeat older than threshold) to a count so
    // dead heartbeat files do not read as live contention.
    let stale_threshold = edda_bridge_claude::peers::stale_secs();
    let (active, stale): (Vec<_>, Vec<_>) =
        sessions.iter().partition(|p| p.age_secs <= stale_threshold);

    if active.is_empty() {
        println!(
            "No active sessions ({} stale heartbeat{}).",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
        return Ok(());
    }

    println!("Active sessions ({}):\n", active.len());
    for p in &active {
        let age = edda_bridge_claude::peers::format_age(p.age_secs);
        let scope = if p.claimed_paths.is_empty() {
            String::new()
        } else {
            format!(" [{}]", p.claimed_paths.join(", "))
        };
        let label = if p.label.is_empty() {
            "(no label)".to_string()
        } else {
            p.label.clone()
        };
        println!(
            "  {} — {} ({age}){scope}",
            &p.session_id[..8.min(p.session_id.len())],
            label
        );

        if !p.task_subjects.is_empty() {
            for t in &p.task_subjects {
                println!("    task: {t}");
            }
        } else if !p.focus_files.is_empty() {
            let files: Vec<&str> = p
                .focus_files
                .iter()
                .take(3)
                .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f.as_str()))
                .collect();
            println!("    focus: {}", files.join(", "));
        }
        if p.files_modified_count > 0 {
            println!("    {} files modified", p.files_modified_count);
        }
        if !p.recent_commits.is_empty() {
            for c in &p.recent_commits {
                println!("    commit: {c}");
            }
        }
    }
    if !stale.is_empty() {
        println!(
            "\n  (+{} stale session{} not shown)",
            stale.len(),
            if stale.len() == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

/// The lines `claim` prints about what a new claim did to the session's old one.
///
/// Split out so the strings — and, more importantly, the *absence* of a
/// `released:` line — can be asserted. Reporting a release that did not happen
/// is the same false success this disclosure exists to remove.
fn claim_disclosure(
    previous: Option<&edda_bridge_claude::peers::ClaimEntry>,
    label: &str,
    paths: &[String],
) -> Vec<String> {
    let Some(previous) = previous else {
        return vec![format!("Claimed scope: {label}")];
    };

    // Only paths the new claim no longer covers were actually let go. Naming
    // `previous.paths` wholesale said "released" about paths this very command
    // had just re-claimed — and an idempotent re-claim hits exactly that, since
    // tier 4 mints a deterministic `cli-<label>` and board claims never expire,
    // so a bare-shell restart re-runs the same command against its own
    // surviving claim.
    let released: Vec<&str> = previous
        .paths
        .iter()
        .filter(|p| !paths.contains(p))
        .map(String::as_str)
        .collect();
    let gained = paths.iter().any(|p| !previous.paths.contains(p));

    let mut lines = Vec::new();
    if previous.label != label {
        lines.push(format!(
            "Claimed scope: {label} (replaces this session's earlier claim on {})",
            previous.label
        ));
    } else if released.is_empty() && !gained {
        lines.push(format!("Re-claimed scope: {label} (unchanged)"));
    } else if released.is_empty() {
        lines.push(format!("Re-claimed scope: {label} (paths added)"));
    } else {
        lines.push(format!(
            "Re-claimed scope: {label} (previous paths replaced)"
        ));
    }
    if !released.is_empty() {
        lines.push(format!("  released: {}", released.join(", ")));
    }
    lines
}

/// `edda bridge claude claim <label>` — claim a coordination scope
///
/// The board folds claims into one per session, so a second claim replaces the
/// first rather than adding to it. That is the right shape — it is how a
/// session narrows or moves its scope, and how a restart re-claims
/// idempotently — but it used to happen in silence, so a worker could believe
/// it held two scopes while peers saw one (GH-488). The replacement is now
/// named.
pub fn claim(
    repo_root: &Path,
    label: &str,
    paths: &[String],
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, label)?;

    let replaced = edda_bridge_claude::peers::compute_board_state(&project_id)
        .claims
        .into_iter()
        .find(|c| c.session_id == session_id);

    edda_bridge_claude::peers::write_claim(&project_id, &session_id, label, paths);
    for line in claim_disclosure(replaced.as_ref(), label, paths) {
        println!("{line}");
    }
    if !paths.is_empty() {
        println!("  paths: {}", paths.join(", "));
    }
    println!("  session: {session_id}");
    Ok(())
}

/// `edda unclaim [--session <id>]` — release a coordination scope.
///
/// Unlike `claim`, this verb never mints a session id. `claim` may invent
/// `cli-<label>` because it is creating the claim; `unclaim` has to name one
/// that already exists, so it resolves against the board and refuses rather
/// than reporting success for a session that holds nothing (GH-486).
///
/// It also never guesses from the board. A caller with no session identity
/// cannot know that a claim is its own, and releasing someone else's would
/// drop the off-limits protection their live session depends on.
///
/// The automatic session-end path does not come through here — bridges call
/// `peers::write_unclaim` directly — so refusing costs a hooked session
/// nothing. A CI teardown that runs the verb unconditionally passes
/// `--if-claimed` and gets exit 0 when there is nothing left to release.
pub fn unclaim(
    repo_root: &Path,
    cli_session: Option<&str>,
    if_claimed: bool,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let board = edda_bridge_claude::peers::compute_board_state(&project_id);
    let session_id = match resolve_unclaim_target(cli_session, &project_id, &board.claims) {
        Ok(sid) => sid,
        // Teardown runs unconditionally and must not fail a job for the normal
        // case of having nothing left to release (GH-488). It reports what it
        // actually found: saying "nothing to unclaim" over a populated board
        // would be false, and the point of this verb is to stop reporting
        // releases that did not happen.
        Err(e) if if_claimed => {
            println!("Released nothing: {e}");
            return Ok(());
        }
        Err(e) => return Err(e),
    };

    let held: Vec<&edda_bridge_claude::peers::ClaimEntry> = board
        .claims
        .iter()
        .filter(|c| c.session_id == session_id)
        .collect();
    if held.is_empty() {
        if if_claimed {
            println!("Nothing to unclaim for session {session_id}");
            return Ok(());
        }
        anyhow::bail!(
            "session {session_id} holds no claim; nothing was released.\n\
             Pass --session with one of the ids below.\n{}",
            describe_claims(&board.claims)
        );
    }

    edda_bridge_claude::peers::write_unclaim(&project_id, &session_id);
    let labels: Vec<&str> = held.iter().map(|c| c.label.as_str()).collect();
    println!(
        "Unclaimed scope for session: {session_id} ({})",
        labels.join(", ")
    );
    Ok(())
}

/// Name the session whose claim `unclaim` should release.
///
/// Explicit `--session` wins, then process-carried `EDDA_SESSION_ID`.
/// Heartbeats are evidence that identity is ambiguous, never evidence that a
/// session belongs to this process. Refuse and show the board instead, so the
/// id for `--session` is in the error.
fn resolve_unclaim_target(
    cli_session: Option<&str>,
    project_id: &str,
    claims: &[edda_bridge_claude::peers::ClaimEntry],
) -> anyhow::Result<String> {
    if let Some(sid) = cli_session.filter(|s| !s.is_empty()) {
        return Ok(sid.to_string());
    }
    if let Ok(sid) = std::env::var("EDDA_SESSION_ID") {
        if !sid.is_empty() {
            return Ok(sid);
        }
    }
    if has_live_sessions(project_id) {
        anyhow::bail!(
            "cannot prove which live session belongs to this process, so --session is required.\n{}",
            describe_claims(claims)
        );
    }

    // No identity of our own, so there is nothing to infer from. Do NOT fall
    // back to "the sole claim on the board": a caller without a session cannot
    // know that claim is theirs, and releasing it drops the off-limits
    // protection its real owner is relying on.
    if claims.is_empty() {
        anyhow::bail!("no claims on the board; nothing to unclaim");
    }
    anyhow::bail!(
        "cannot tell which claim is yours, so --session is required.\n{}",
        describe_claims(claims)
    );
}

/// Render the board's claims for an error message, so the reader can copy the
/// session id straight into `--session` instead of hunting for it.
fn describe_claims(claims: &[edda_bridge_claude::peers::ClaimEntry]) -> String {
    if claims.is_empty() {
        return "The board holds no claims.".to_string();
    }
    let mut out = String::from("Claims on the board:");
    for c in claims {
        out.push_str(&format!("\n  {} — {}", c.session_id, c.label));
        if !c.paths.is_empty() {
            out.push_str(&format!(" ({})", c.paths.join(", ")));
        }
    }
    out
}

/// `edda bridge claude decide <key=value>` — record a decision.
///
/// GH-401: the decision is agent-authored and unratified (not binding) until
/// an operator ratifies it via `edda ratify`.
///
/// Writes to both:
/// 1. Peers `coordination.jsonl` — real-time broadcast to active peers
/// 2. Workspace ledger — permanent record visible to all sessions
#[allow(clippy::too_many_arguments)]
pub fn decide(
    repo_root: &Path,
    decision: &str,
    reason: Option<&str>,
    refs: &[String],
    cli_session: Option<&str>,
    scope_str: Option<&str>,
    paths: &[String],
    tags: &[String],
) -> anyhow::Result<()> {
    let (key, value) = decision.split_once('=').ok_or_else(|| {
        anyhow::anyhow!("decision must be in key=value format (e.g. \"auth.method=JWT RS256\")")
    })?;

    let key = key.trim();
    let value = value.trim();

    // EDDA-SECRET-GUARD1 q331: scrub value + reason before ANY persistence
    // (peer broadcast, ledger, coordination log). Deterministic zero-LLM.
    let (safe_value, value_hits) = edda_core::secret_guard::redact(value);
    let value = safe_value.as_str();
    let (safe_reason, reason_hits) = match reason {
        Some(r) => {
            let (out, hits) = edda_core::secret_guard::redact(r);
            (Some(out), hits)
        }
        None => (None, Vec::new()),
    };
    let reason: Option<&str> = safe_reason.as_deref();
    let all_hits = value_hits.len() + reason_hits.len();
    if all_hits > 0 {
        let kinds: Vec<_> = value_hits
            .iter()
            .chain(reason_hits.iter())
            .map(|h| h.kind)
            .collect();
        eprintln!(
            "⚠ secret-guard: redacted {all_hits} secret pattern(s) before writing decision ({})",
            kinds.join(", ")
        );
    }

    let project_id = edda_store::project_id(repo_root);
    let (session_id, label) = resolve_session_id(cli_session, &project_id, "cli")?;

    // L2 conflict check (coordination.jsonl) — before writing
    if let Some(conflict) =
        edda_bridge_claude::peers::find_binding_conflict(&project_id, key, value)
    {
        eprintln!(
            "\u{26a0} Conflict: key \"{key}\" already decided as \"{}\" by {} ({})",
            conflict.existing_value, conflict.by_label, conflict.ts
        );
        eprintln!("  Recording your decision \"{key}={value}\" — consider resolving with the other agent.");
        // Postmortem supply line: SELECTOR3 病一——same label = own progression,
        // not a cross-agent conflict; only record when actors differ. Best-effort, never blocks.
        let _ = edda_postmortem::signals::record_conflict_signal_if_cross_actor(
            &project_id,
            key,
            &conflict.by_label,
            &label,
        );
    }

    // 1. Broadcast to peers (real-time)
    edda_bridge_claude::peers::write_binding(&project_id, &session_id, &label, key, value);

    // 2. Write to workspace ledger (permanent)
    let ledger = edda_ledger::Ledger::open(repo_root).context("cmd_bridge: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;
    let parent_hash = ledger.last_event_hash()?;

    // Use resolved label as actor (not hardcoded "system")
    let actor = if session_id.starts_with("cli-") {
        "system"
    } else {
        &label
    };
    // GH-401: a written decision never self-declares operator authority.
    // It is tagged system (internal) or agent; operator authority is
    // conferred only by a separate `edda ratify` (decision_ratify event).
    let authority = if actor == "system" {
        edda_core::types::authority::SYSTEM
    } else {
        edda_core::types::authority::AGENT
    };
    let scope = scope_str
        .filter(|s| *s != "local")
        .map(|s| s.parse::<edda_core::types::DecisionScope>())
        .transpose()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let dp = edda_core::types::DecisionPayload {
        key: key.to_string(),
        value: value.to_string(),
        reason: reason.map(|r| r.to_string()),
        scope,
        authority: Some(authority.to_string()),
        affected_paths: if paths.is_empty() {
            None
        } else {
            Some(paths.to_vec())
        },
        tags: if tags.is_empty() {
            None
        } else {
            Some(tags.to_vec())
        },
        review_after: None,
        reversibility: None,
        village_id: None,
    };
    let mut event =
        edda_core::event::new_decision_event(&branch, parent_hash.as_deref(), actor, &dp)?;

    // Check for prior decision with same key → supersede via provenance (only if value differs)
    let prior = ledger.find_active_decision(&branch, key)?;
    if let Some(prior_row) = &prior {
        if prior_row.value != value {
            eprintln!(
                "\u{26a0} Conflict: key \"{key}\" previously decided as \"{}\" in this workspace",
                prior_row.value
            );
            eprintln!("  Recording new value \"{value}\" (supersedes prior decision)");
            event.refs.provenance.push(edda_core::types::Provenance {
                target: prior_row.event_id.clone(),
                rel: edda_core::types::rel::SUPERSEDES.to_string(),
                note: Some(format!("key '{}' re-decided", key)),
            });
            // Postmortem supply line: best-effort, never blocks the decide.
            let _ = edda_postmortem::signals::record_decision_signal(
                &project_id,
                edda_postmortem::signals::SignalKind::Superseded,
                key,
            );
        }
    }

    // Add depends_on provenance for each --refs key
    for ref_key in refs {
        if let Some(ref_row) = ledger.find_active_decision(&branch, ref_key)? {
            event.refs.provenance.push(edda_core::types::Provenance {
                target: ref_row.event_id.clone(),
                rel: edda_core::types::rel::DEPENDS_ON.to_string(),
                note: Some(ref_key.to_string()),
            });
        } else {
            eprintln!("\u{26a0} ref '{ref_key}' not found, skipping");
        }
    }

    // Re-finalize after payload/refs mutation
    edda_core::event::finalize_event(&mut event)?;
    ledger.append_event(&event)?;

    // Insert dependency edges
    let domain = edda_core::decision::extract_domain(key);

    // Explicit refs → dep edges
    for ref_key in refs {
        // Only insert if the ref key actually exists
        if ledger.find_active_decision(&branch, ref_key)?.is_some() {
            ledger.insert_dep(key, ref_key, "explicit", Some(&event.event_id))?;
        }
    }

    // Auto-link: star-shaped within same domain
    let same_domain = ledger.active_decisions(Some(&domain), None, None, None)?;
    for d in &same_domain {
        if d.key != key {
            ledger.insert_dep(key, &d.key, "auto_domain", Some(&event.event_id))?;
        }
    }

    println!("Decision recorded: {key} = {value}");
    if let Some(r) = reason {
        println!("  reason: {r}");
    }
    if let Some(s) = scope {
        println!("  scope: {s}");
    }
    if !paths.is_empty() {
        println!("  paths: {}", paths.join(", "));
    }
    if !tags.is_empty() {
        println!("  tags: {}", tags.join(", "));
    }

    // Refresh derived markdown views (log.md / main.md / commit.md) so operators
    // reading the ledger by eye see the decision immediately, not only after the
    // next `edda commit` / `edda rebuild`. Same best-effort pattern as
    // edda-serve::api::drafts.rs:508 — failure never blocks a successful decide.
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    Ok(())
}

/// `edda ratify <key>` — confer operator authority on an active decision (GH-401).
///
/// Ratification is a separate append-only fact (`decision_ratify` event),
/// never a mutation of the decision, so operator authority is conferred by a
/// deliberate act and is fully auditable via `ratified_by`. This is the
/// operator counterpart to `edda decide`; agents are taught `decide` only
/// (see the write-back protocol), so a compliant agent never self-ratifies.
///
/// Identity is not cryptographically enforced here — a session can record any
/// `ratified_by`. That enforcement is a policy-layer concern (GH-401 scope);
/// this layer delivers the separation of act, the rendering split, and the
/// audit trail.
pub fn ratify(
    repo_root: &Path,
    key: &str,
    note: Option<&str>,
    by: Option<&str>,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let key = key.trim();
    let project_id = edda_store::project_id(repo_root);
    let (_session_id, label) = resolve_session_id(cli_session, &project_id, "cli")?;
    let ratified_by = by.unwrap_or(&label);

    let ledger = edda_ledger::Ledger::open(repo_root).context("cmd_bridge: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;

    // Only an existing active decision can be ratified.
    if ledger.find_active_decision(&branch, key)?.is_none() {
        anyhow::bail!("no active decision for key '{key}' — nothing to ratify (see `edda ask`)");
    }

    let parent_hash = ledger.last_event_hash()?;
    let event = edda_core::event::new_decision_ratify_event(
        &branch,
        parent_hash.as_deref(),
        key,
        ratified_by,
        note,
    )?;
    ledger.append_event(&event)?;
    let _ = edda_derive::rebuild_branch(&ledger, &branch);

    println!("Ratified '{key}' (by {ratified_by}) — now binding.");
    if let Some(n) = note {
        println!("  note: {n}");
    }
    Ok(())
}

/// `edda bridge claude request <to> <message>` — send cross-agent request
///
/// The target is a free-string label, so a typo used to be indistinguishable
/// from a delivered message (GH-443). Resolve it against live sessions first:
/// nobody listening is an error unless `--force`, and an ambiguous label is a
/// warning, because the message really will land in several inboxes.
pub fn request(
    repo_root: &Path,
    to: &str,
    message: &str,
    cli_session: Option<&str>,
    force: bool,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, from_label) = resolve_session_id(cli_session, &project_id, "cli")?;

    let targets = edda_bridge_claude::peers::resolve_request_targets(&project_id, to);
    if targets.is_empty() && !force {
        let active = active_labels(&project_id);
        let known = if active.is_empty() {
            "no sessions are currently active".to_string()
        } else {
            format!("active labels: {}", active.join(", "))
        };
        anyhow::bail!(
            "no active session answers to '{to}' — {known}\n\
             check the label, or pass --force to queue the request for a peer that has not started yet"
        );
    }
    if targets.len() > 1 {
        eprintln!(
            "warning: '{to}' matches {} active sessions — every one of them will see this request",
            targets.len()
        );
    }

    edda_bridge_claude::peers::write_request(&project_id, &session_id, &from_label, to, message);
    let notify_config =
        edda_notify::NotifyConfig::load(&edda_ledger::EddaPaths::discover(repo_root));
    if !notify_config.channels.is_empty() {
        edda_notify::dispatch(
            &notify_config,
            &edda_notify::NotifyEvent::RequestPending {
                from_label: from_label.clone(),
                to_label: to.to_string(),
                message: message.to_string(),
            },
        );
    }
    if targets.is_empty() {
        println!("Request queued for [{to}] (no active session): \"{message}\"");
    } else {
        println!("Request sent to [{to}]: \"{message}\"");
    }
    if targets.is_empty() {
        println!("The peer will see it at their next prompt.");
    } else {
        println!(
            "To wake them now, use your host's cross-session messaging (target session: {}).",
            targets.join(", ")
        );
    }
    Ok(())
}

/// Labels of every currently active session, for "did you mean" diagnostics.
fn active_labels(project_id: &str) -> Vec<String> {
    let stale = edda_bridge_claude::peers::stale_secs();
    let mut labels: Vec<String> = edda_bridge_claude::peers::discover_all_sessions(project_id)
        .into_iter()
        .filter(|p| p.age_secs <= stale && !p.label.is_empty())
        .map(|p| p.label)
        .collect();
    labels.sort();
    labels.dedup();
    labels
}

/// `edda request-ack <from>` — acknowledge a pending request
pub fn request_ack(
    repo_root: &Path,
    from_label: &str,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _label) = resolve_session_id(cli_session, &project_id, "cli")?;

    edda_bridge_claude::peers::write_request_ack(&project_id, &session_id, from_label);
    println!("Acknowledged request from [{from_label}]");
    Ok(())
}

/// Resolve attribution identity for a session-taking CLI verb.
///
/// 1. `--session` CLI flag (explicit override)
/// 2. Process-carried `EDDA_SESSION_ID` (bridge/conductor path, user override)
/// 3. `"cli-{fallback_label}"` only when no live session makes that ambiguous
///
/// `EDDA_SESSION_ID` proves only that the invoking process received an id; it
/// is attribution and an explicit user override, not authentication or
/// authorization. Heartbeats, branches, and working directories cannot prove
/// which process owns a session, so any live heartbeat makes an uncarried
/// identity an error. With no live sessions, the deterministic `cli-*`
/// fallback preserves genuine standalone CLI use. A carrier can preserve only
/// the identity its host exposes; Codex tool hooks, for example, attribute
/// subagent commands to the parent session (GH-503).
pub(crate) fn resolve_session_id(
    cli_session: Option<&str>,
    project_id: &str,
    fallback_label: &str,
) -> anyhow::Result<(String, String)> {
    let env_label = std::env::var("EDDA_SESSION_LABEL")
        .ok()
        .filter(|v| !v.is_empty());

    // Tier 1: explicit --session flag
    if let Some(sid) = cli_session.filter(|s| !s.is_empty()) {
        let label = env_label.unwrap_or_else(|| fallback_label.to_string());
        return Ok((sid.to_string(), label));
    }

    // Tier 2: EDDA_SESSION_ID env var
    if let Ok(sid) = std::env::var("EDDA_SESSION_ID") {
        if !sid.is_empty() {
            let label = env_label.unwrap_or_else(|| fallback_label.to_string());
            return Ok((sid, label));
        }
    }

    if has_live_sessions(project_id) {
        anyhow::bail!(
            "cannot prove which live session belongs to this process, so --session is required \
             (or set EDDA_SESSION_ID in the invoking process)"
        );
    }

    let label = env_label.unwrap_or_else(|| fallback_label.to_string());
    Ok((format!("cli-{fallback_label}"), label))
}

fn has_live_sessions(project_id: &str) -> bool {
    let stale = edda_bridge_claude::peers::stale_secs();
    edda_bridge_claude::peers::discover_all_sessions(project_id)
        .into_iter()
        .any(|session| session.age_secs <= stale)
}

/// `edda bridge claude digest --session <id>` or `--all`
pub fn digest(repo_root: &Path, session: Option<&str>, all: bool) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let cwd = repo_root.to_str().unwrap_or(".");

    if let Some(session_id) = session {
        println!("Digesting session {session_id}...");
        let event_id =
            edda_bridge_claude::digest::digest_session_manual(&project_id, session_id, cwd, true)?;
        println!("  Written: {event_id}");
        return Ok(());
    }

    if all {
        let pending = edda_bridge_claude::digest::find_all_pending_sessions(&project_id);
        if pending.is_empty() {
            println!("No pending sessions to digest.");
            return Ok(());
        }
        println!("Found {} pending sessions", pending.len());
        for session_id in &pending {
            print!("  Digesting {session_id}...");
            match edda_bridge_claude::digest::digest_session_manual(
                &project_id,
                session_id,
                cwd,
                true,
            ) {
                Ok(event_id) => println!(" OK ({event_id})"),
                Err(e) => println!(" FAILED: {e}"),
            }
        }
        return Ok(());
    }

    anyhow::bail!("must specify --session <id> or --all")
}

/// `edda index verify --project <id> --session <id> [--sample N] [--all]`
pub fn index_verify(
    project_id: &str,
    session_id: &str,
    sample: usize,
    all: bool,
) -> anyhow::Result<()> {
    let project_dir = edda_store::project_dir(project_id);
    let index_path = project_dir
        .join("index")
        .join(format!("{session_id}.jsonl"));
    let store_path = project_dir
        .join("transcripts")
        .join(format!("{session_id}.jsonl"));

    if !index_path.exists() {
        anyhow::bail!("index file not found: {}", index_path.display());
    }
    if !store_path.exists() {
        anyhow::bail!("store file not found: {}", store_path.display());
    }

    let max_lines = if all { usize::MAX } else { sample * 2 };
    let records = edda_index::read_index_tail(&index_path, max_lines, 64 * 1024 * 1024)?;

    let check_count = if all {
        records.len()
    } else {
        sample.min(records.len())
    };

    // Sample evenly from the records
    let step = if check_count == 0 {
        1
    } else {
        (records.len() as f64 / check_count as f64).ceil() as usize
    };

    let mut checked = 0;
    let mut mismatches = 0;

    for (i, rec) in records.iter().enumerate() {
        if !all && i % step != 0 && checked >= check_count {
            continue;
        }
        if checked >= check_count {
            break;
        }

        let fetched = edda_index::fetch_store_line(&store_path, rec.store_offset, rec.store_len)?;
        let parsed: serde_json::Value = serde_json::from_slice(&fetched)?;
        let fetched_uuid = parsed.get("uuid").and_then(|v| v.as_str()).unwrap_or("");

        if fetched_uuid != rec.uuid {
            println!(
                "MISMATCH at index record {}: expected uuid={}, got uuid={}",
                i, rec.uuid, fetched_uuid
            );
            mismatches += 1;
        }
        checked += 1;
    }

    if mismatches > 0 {
        anyhow::bail!("{mismatches} mismatches found in {checked} checks");
    }

    println!("OK: {checked} index records verified, 0 mismatches");
    Ok(())
}

// ── Render Commands ──

/// `edda bridge claude render-writeback`
pub fn render_writeback() -> anyhow::Result<()> {
    println!("{}", edda_bridge_claude::render::writeback());
    Ok(())
}

/// `edda bridge claude render-workspace`
pub fn render_workspace(repo_root: &Path, budget: usize) -> anyhow::Result<()> {
    let cwd = repo_root.to_str().unwrap_or(".");
    match edda_bridge_claude::render::workspace(cwd, budget) {
        Some(s) => println!("{s}"),
        None => println!("(no workspace context)"),
    }
    Ok(())
}

/// `edda bridge claude render-coordination`
pub fn render_coordination(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli")?;
    match edda_bridge_claude::render::coordination(&project_id, &session_id) {
        Some(s) => println!("{s}"),
        None => println!("(no coordination context)"),
    }
    Ok(())
}

/// `edda bridge claude render-fleet`
///
/// The same section the SessionStart pack embeds. It exists as a verb because a
/// section only reachable through a hook is a section nobody can look at —
/// including whoever has to work out why it said what it said.
pub fn render_fleet(repo_root: &Path, budget: usize) -> anyhow::Result<()> {
    match edda_bridge_claude::render::fleet(&repo_root.to_string_lossy(), budget) {
        Some(s) => println!("{s}"),
        None => println!("(no siblings with rulings or waiting work)"),
    }
    Ok(())
}

/// `edda bridge claude render-pack`
pub fn render_pack(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    match edda_bridge_claude::render::pack(&project_id) {
        Some(s) => println!("{s}"),
        None => println!("(no hot pack available)"),
    }
    Ok(())
}

/// `edda bridge claude render-plan`
pub fn render_plan(repo_root: &Path) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    match edda_bridge_claude::render::plan(Some(&project_id)) {
        Some(s) => println!("{s}"),
        None => println!("(no active plan)"),
    }
    Ok(())
}

// ── Heartbeat Commands ──

/// `edda bridge claude heartbeat-write`
pub fn heartbeat_write(
    repo_root: &Path,
    label: &str,
    cli_session: Option<&str>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, label)?;
    let _ = edda_store::ensure_dirs(&project_id);
    edda_bridge_claude::peers::write_heartbeat_minimal(
        &project_id,
        &session_id,
        label,
        repo_root.to_str().unwrap_or("."),
    );
    println!("Heartbeat written: {label} ({session_id})");
    Ok(())
}

/// `edda bridge claude heartbeat-touch`
pub fn heartbeat_touch(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli")?;
    edda_bridge_claude::peers::touch_heartbeat(&project_id, &session_id);
    println!("Heartbeat touched: {session_id}");
    Ok(())
}

/// `edda bridge claude heartbeat-remove`
pub fn heartbeat_remove(repo_root: &Path, cli_session: Option<&str>) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let (session_id, _) = resolve_session_id(cli_session, &project_id, "cli")?;
    edda_bridge_claude::peers::remove_heartbeat(&project_id, &session_id);
    println!("Heartbeat removed: {session_id}");
    Ok(())
}

// ── Background Decision Extraction ──

/// `edda bridge claude bg-review`
pub fn bg_review(
    repo_root: &Path,
    list: bool,
    accept: Option<String>,
    reject: Option<String>,
    accept_all: bool,
    session: Option<String>,
) -> anyhow::Result<()> {
    let project_id = edda_store::project_id(repo_root);

    if list {
        let drafts = edda_bridge_claude::bg_extract::list_pending_drafts(&project_id)?;
        if drafts.is_empty() {
            println!("No pending draft decisions.");
            return Ok(());
        }
        for draft in &drafts {
            println!(
                "\n── Session: {} (extracted: {}, model: {}) ──",
                draft.session_id, draft.extracted_at, draft.model
            );
            for (i, d) in draft.decisions.iter().enumerate() {
                let status_marker = match d.status {
                    edda_bridge_claude::bg_extract::DraftStatus::Pending => "⏳",
                    edda_bridge_claude::bg_extract::DraftStatus::Accepted => "✅",
                    edda_bridge_claude::bg_extract::DraftStatus::Rejected => "❌",
                };
                let kind_label = match d.kind {
                    edda_bridge_claude::bg_extract::DecisionKind::Extraction => "extraction",
                    edda_bridge_claude::bg_extract::DecisionKind::Enhancement => "enhancement",
                };
                let reason_str = d.reason.as_deref().unwrap_or("-");
                println!(
                    "  [{i}] {status_marker} [{kind_label}] {}={} (confidence: {:.0}%)",
                    d.key,
                    d.value,
                    d.confidence * 100.0
                );
                if d.kind == edda_bridge_claude::bg_extract::DecisionKind::Enhancement {
                    let orig = d.original_reason.as_deref().unwrap_or("(none)");
                    println!("      original reason: {orig}");
                    println!("      enhanced reason: {reason_str}");
                } else {
                    println!("      reason: {reason_str}");
                }
                println!("      evidence: {}", d.evidence);
            }
        }
        return Ok(());
    }

    let session_id = session
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("--session is required for accept/reject operations"))?;

    if accept_all {
        let accepted =
            edda_bridge_claude::bg_extract::accept_all_decisions(&project_id, session_id)?;
        if accepted.is_empty() {
            println!("No pending decisions to accept.");
            return Ok(());
        }
        // Write accepted decisions to workspace ledger
        write_accepted_to_ledger(repo_root, &accepted)?;
        println!("Accepted {} decisions.", accepted.len());
        return Ok(());
    }

    if let Some(indices_str) = accept {
        let indices = parse_indices(&indices_str)?;
        let accepted =
            edda_bridge_claude::bg_extract::accept_decisions(&project_id, session_id, &indices)?;
        if accepted.is_empty() {
            println!("No decisions accepted (indices may be invalid or already processed).");
            return Ok(());
        }
        write_accepted_to_ledger(repo_root, &accepted)?;
        println!("Accepted {} decisions.", accepted.len());
        return Ok(());
    }

    if let Some(indices_str) = reject {
        let indices = parse_indices(&indices_str)?;
        edda_bridge_claude::bg_extract::reject_decisions(&project_id, session_id, &indices)?;
        println!("Rejected {} decisions.", indices.len());
        return Ok(());
    }

    // Default: list if no action specified
    println!("Usage: edda bridge claude bg-review --list");
    println!("       edda bridge claude bg-review --session <sid> --accept-all");
    println!("       edda bridge claude bg-review --session <sid> --accept 0,1,2");
    println!("       edda bridge claude bg-review --session <sid> --reject 3");
    Ok(())
}

fn write_accepted_to_ledger(
    repo_root: &Path,
    decisions: &[edda_bridge_claude::bg_extract::ExtractedDecision],
) -> anyhow::Result<()> {
    let ledger = edda_ledger::Ledger::open(repo_root)
        .context("cmd_bridge::write_accepted_to_ledger: opening ledger")?;
    let _lock = edda_ledger::lock::WorkspaceLock::acquire(&ledger.paths)?;
    let branch = ledger.head_branch()?;

    for d in decisions {
        let parent_hash = ledger.last_event_hash()?;
        let payload = edda_core::types::DecisionPayload {
            key: d.key.clone(),
            value: d.value.clone(),
            reason: d.reason.clone(),
            scope: None,
            // GH-401: background-extracted decisions are machine inference from
            // transcripts — the purest agent-authored case. Tag them honestly
            // so they land in the unratified tier, not binding.
            authority: Some(edda_core::types::authority::AGENT.to_string()),
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };

        let actor = match d.kind {
            edda_bridge_claude::bg_extract::DecisionKind::Enhancement => "edda-bg/reason-enhancer",
            edda_bridge_claude::bg_extract::DecisionKind::Extraction => {
                "edda-bg/decision-extractor"
            }
        };

        let mut event =
            edda_core::event::new_decision_event(&branch, parent_hash.as_deref(), actor, &payload)?;

        // For enhancements, supersede the original decision
        if d.kind == edda_bridge_claude::bg_extract::DecisionKind::Enhancement {
            if let Ok(Some(prior)) = ledger.find_active_decision(&branch, &d.key) {
                event.refs.provenance.push(edda_core::types::Provenance {
                    target: prior.event_id.clone(),
                    rel: edda_core::types::rel::SUPERSEDES.to_string(),
                    note: Some(format!(
                        "reason enhanced from: {}",
                        d.original_reason.as_deref().unwrap_or("(none)")
                    )),
                });
            }
        }

        ledger.append_event(&event)?;
    }

    Ok(())
}

fn parse_indices(s: &str) -> anyhow::Result<Vec<usize>> {
    s.split(',')
        .map(|part| {
            part.trim()
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("Invalid index: {}", part.trim()))
        })
        .collect()
}

// ── OpenClaw Bridge ──

/// `edda bridge openclaw install`
pub fn install_openclaw(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_openclaw::install(target)
}

/// `edda bridge openclaw uninstall`
pub fn uninstall_openclaw(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_openclaw::uninstall(target)
}

/// `edda hook openclaw` — read stdin, dispatch hook
///
/// Resilience: catch_unwind + configurable timeout (EDDA_HOOK_TIMEOUT_MS).
/// On panic or timeout, exits 0 — never blocks the host agent.
pub fn hook_openclaw() -> anyhow::Result<()> {
    run_hook_resilient("OPENCLAW ", |stdin| {
        let r = edda_bridge_openclaw::hook_entrypoint_from_stdin(&stdin)?;
        Ok((r.stdout, r.stderr))
    })
}

/// `edda doctor openclaw`
pub fn doctor_openclaw() -> anyhow::Result<()> {
    edda_bridge_openclaw::doctor()
}

/// `edda bridge codex install`
pub fn install_codex(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_codex::install(target).map(|_| ())
}

/// `edda bridge codex uninstall`
pub fn uninstall_codex(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_codex::uninstall(target)
}

/// `edda hook codex` — read stdin, dispatch hook
pub fn hook_codex() -> anyhow::Result<()> {
    let mut stdin = String::new();
    std::io::stdin().read_to_string(&mut stdin)?;
    let r = edda_bridge_codex::hook_entrypoint_from_stdin(&stdin)?;
    if let Some(out) = r.stdout {
        println!("{out}");
    }
    if let Some(err) = r.stderr {
        eprintln!("{err}");
    }
    Ok(())
}

/// `edda doctor codex`
pub fn doctor_codex() -> anyhow::Result<()> {
    edda_bridge_codex::doctor()
}

/// `edda bridge hermes install`
pub fn install_hermes(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_hermes::install(target).map(|_| ())
}

/// `edda bridge hermes uninstall`
pub fn uninstall_hermes(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_hermes::uninstall(target)
}

/// `edda hook hermes` — read stdin, dispatch hook
pub fn hook_hermes() -> anyhow::Result<()> {
    let mut stdin = String::new();
    std::io::stdin().read_to_string(&mut stdin)?;
    let r = edda_bridge_hermes::hook_entrypoint_from_stdin(&stdin)?;
    if let Some(out) = r.stdout {
        println!("{out}");
    }
    if let Some(err) = r.stderr {
        eprintln!("{err}");
    }
    Ok(())
}

/// `edda doctor hermes`
pub fn doctor_hermes() -> anyhow::Result<()> {
    edda_bridge_hermes::doctor()
}

/// `edda bridge cursor install`
pub fn install_cursor(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_cursor::install(target).map(|_| ())
}

/// `edda bridge cursor uninstall`
pub fn uninstall_cursor(target: Option<&Path>) -> anyhow::Result<()> {
    edda_bridge_cursor::uninstall(target)
}

/// `edda hook cursor` — read stdin, dispatch hook
pub fn hook_cursor() -> anyhow::Result<()> {
    run_hook_resilient("CURSOR ", |stdin| {
        let r = edda_bridge_cursor::hook_entrypoint_from_stdin(&stdin)?;
        Ok((r.stdout, r.stderr))
    })
}

/// `edda doctor cursor`
pub fn doctor_cursor() -> anyhow::Result<()> {
    edda_bridge_cursor::doctor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn webhook_capture() -> (
        String,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(100)))
                .unwrap();
            let mut request = Vec::new();
            let _ = stream.read_to_end(&mut request);
            tx.send(String::from_utf8_lossy(&request).into_owned())
                .unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
        });
        (url, rx, handle)
    }

    fn enable_webhook(repo: &std::path::Path, url: &str) {
        std::fs::create_dir_all(repo.join(".edda")).unwrap();
        std::fs::write(
            repo.join(".edda/config.json"),
            serde_json::json!({
                "notify_channels": [{
                    "type": "webhook",
                    "url": url,
                    "events": ["request_pending"]
                }]
            })
            .to_string(),
        )
        .unwrap();
    }

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Serialize tests that mutate process-global env vars
    /// (EDDA_SESSION_ID/LABEL, EDDA_HOOK_TIMEOUT_MS) — without this they
    /// race each other under the parallel test runner. Same pattern as
    /// edda-bridge-claude's ENV_LOCK. Poisoned locks are recovered so one
    /// failing test doesn't cascade.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn setup_workspace() -> (std::path::PathBuf, edda_ledger::Ledger) {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_bridge_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = edda_ledger::EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        let ledger = edda_ledger::Ledger::open(&tmp).unwrap();
        (tmp, ledger)
    }

    #[test]
    fn find_active_decision_returns_value() {
        let (tmp, ledger) = setup_workspace();
        let branch = ledger.head_branch().unwrap();
        let parent_hash = ledger.last_event_hash().unwrap();

        // Write a decision event with structured fields
        let tags = vec!["decision".to_string()];
        let mut event = edda_core::event::new_note_event(
            &branch,
            parent_hash.as_deref(),
            "system",
            "db.engine: postgres",
            &tags,
        )
        .unwrap();
        event.payload["decision"] = serde_json::json!({"key": "db.engine", "value": "postgres"});
        edda_core::event::finalize_event(&mut event).unwrap();
        ledger.append_event(&event).unwrap();

        let result = ledger.find_active_decision(&branch, "db.engine").unwrap();
        assert!(result.is_some(), "should find active decision");
        let row = result.unwrap();
        assert!(!row.event_id.is_empty());
        assert_eq!(row.value, "postgres");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn find_active_decision_no_match() {
        let (tmp, ledger) = setup_workspace();
        let branch = ledger.head_branch().unwrap();

        let result = ledger
            .find_active_decision(&branch, "nonexistent.key")
            .unwrap();
        assert!(result.is_none(), "should not find anything");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── Integration: decide() end-to-end (Issue #148 Gaps 1, 2) ──

    #[test]
    fn decide_writes_binding_to_coordination_log() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        let (tmp, _ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);
        // Clean coordination log
        let state_dir = edda_store::project_dir(&pid).join("state");
        let _ = std::fs::remove_file(state_dir.join("coordination.jsonl"));

        std::env::set_var("EDDA_SESSION_ID", "test-decide-bind-s1");
        std::env::set_var("EDDA_SESSION_LABEL", "auth");

        decide(
            &tmp,
            "db.engine=postgres",
            Some("need JSONB"),
            &[],
            None,
            None,
            &[],
            &[],
        )
        .unwrap();

        // Verify binding was written via L2 conflict check API
        let conflict = edda_bridge_claude::peers::find_binding_conflict(&pid, "db.engine", "OTHER");
        assert!(
            conflict.is_some(),
            "should find existing binding via conflict check"
        );
        let c = conflict.unwrap();
        assert_eq!(c.existing_value, "postgres");
        // Verify no conflict with same value (idempotent)
        let no_conflict =
            edda_bridge_claude::peers::find_binding_conflict(&pid, "db.engine", "postgres");
        assert!(no_conflict.is_none(), "same value should not conflict");

        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    #[test]
    fn decide_writes_structured_ledger_event() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        let (tmp, ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);

        std::env::set_var("EDDA_SESSION_ID", "test-decide-ledger-s2");
        std::env::set_var("EDDA_SESSION_LABEL", "billing");

        decide(
            &tmp,
            "auth.method=JWT RS256",
            Some("stateless auth"),
            &[],
            None,
            None,
            &[],
            &[],
        )
        .unwrap();

        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 1, "should have 1 event");
        let e = &events[0];
        assert_eq!(e.event_type, "note");

        // Tags
        let tags = e.payload.get("tags").and_then(|v| v.as_array()).unwrap();
        assert!(tags.iter().any(|t| t.as_str() == Some("decision")));

        // Structured decision object
        let dec = e.payload.get("decision").unwrap();
        assert_eq!(dec["key"].as_str().unwrap(), "auth.method");
        assert_eq!(dec["value"].as_str().unwrap(), "JWT RS256");
        assert_eq!(dec["reason"].as_str().unwrap(), "stateless auth");

        // GH-401: an agent-session decide is tagged authority=agent, never
        // operator — a write can never self-declare operator authority.
        assert_eq!(dec["authority"].as_str().unwrap(), "agent");

        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    #[test]
    fn ratify_records_separate_event_and_makes_decision_binding() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        let (tmp, ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);
        std::env::set_var("EDDA_SESSION_ID", "test-ratify-s1");
        std::env::set_var("EDDA_SESSION_LABEL", "worker");

        decide(
            &tmp,
            "db.engine=sqlite",
            Some("embedded"),
            &[],
            None,
            None,
            &[],
            &[],
        )
        .unwrap();

        // Before ratify: the active decision is not binding.
        assert!(ledger.ratified_decision_events().unwrap().is_empty());

        ratify(
            &tmp,
            "db.engine",
            Some("looks right"),
            Some("operator"),
            None,
        )
        .unwrap();

        // A distinct decision_ratify event was written (not a mutation).
        let ratify_events = ledger.iter_events_by_type("decision_ratify").unwrap();
        assert_eq!(ratify_events.len(), 1);
        assert_eq!(ratify_events[0].payload["key"], "db.engine");
        assert_eq!(ratify_events[0].payload["ratified_by"], "operator");

        // The projection now reports the key as binding.
        let views = ledger.active_decisions(None, None, None, None).unwrap();
        let view = views.iter().find(|v| v.key == "db.engine").unwrap();
        let set = ledger.ratified_decision_events().unwrap();
        assert!(edda_ledger::view::is_decision_ratified(view, &set));

        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    #[test]
    fn ratify_unknown_key_errors() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        let (tmp, _ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);
        let err = ratify(&tmp, "nope.key", None, None, None)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no active decision"),
            "unexpected error: {err}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    #[test]
    fn decide_supersedes_prior_decision_same_key() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        let (tmp, ledger) = setup_workspace();
        let pid = edda_store::project_id(&tmp);
        let _ = edda_store::ensure_dirs(&pid);

        std::env::set_var("EDDA_SESSION_ID", "test-decide-super-s3");
        std::env::set_var("EDDA_SESSION_LABEL", "infra");

        decide(&tmp, "db.engine=SQLite", None, &[], None, None, &[], &[]).unwrap();
        decide(
            &tmp,
            "db.engine=PostgreSQL",
            Some("need JSONB"),
            &[],
            None,
            None,
            &[],
            &[],
        )
        .unwrap();

        let events = ledger.iter_events().unwrap();
        assert_eq!(events.len(), 2, "should have 2 events");

        let first_id = &events[0].event_id;
        let second = &events[1];

        // Second event should supersede the first
        assert!(
            !second.refs.provenance.is_empty(),
            "second event should have provenance"
        );
        let prov = &second.refs.provenance[0];
        assert_eq!(prov.target, *first_id, "should point to first event");
        assert_eq!(prov.rel, edda_core::types::rel::SUPERSEDES);

        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    #[test]
    fn bare_decide_beside_two_live_sessions_refuses_without_writing() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let (repo, ledger) = setup_workspace();
        let pid = edda_store::project_id(&repo);
        edda_store::ensure_dirs(&pid).expect("store dirs");
        edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "worker-a", "worker-a", "/tmp/a");
        edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "worker-b", "worker-b", "/tmp/b");
        let before = ledger.iter_events().expect("events before").len();

        let err = decide(
            &repo,
            "unsafe.adoption=blocked",
            Some("identity must come from the process"),
            &[],
            None,
            None,
            &[],
            &[],
        )
        .expect_err("a bare shell beside live sessions must refuse");
        assert!(err.to_string().contains("--session"), "{err}");
        assert_eq!(
            ledger.iter_events().expect("events after").len(),
            before,
            "a refused decide must not append to the ledger"
        );
        assert!(
            edda_bridge_claude::peers::compute_board_state(&pid)
                .bindings
                .is_empty(),
            "a refused decide must not broadcast a binding"
        );
        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(edda_store::project_dir(&pid));
    }

    // ── Integration: process-bound session identity (GH-503) ──

    #[test]
    fn resolve_session_id_tiers() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        let pid = "test_resolve_sid_tiers";
        let _ = edda_store::ensure_dirs(pid);

        // Clear env to avoid interference
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");

        // Tier 1: explicit cli_session
        let (sid, label) = resolve_session_id(Some("explicit-sid"), pid, "cli").unwrap();
        assert_eq!(sid, "explicit-sid");
        assert_eq!(label, "cli");

        // Tier 2: EDDA_SESSION_ID env
        std::env::set_var("EDDA_SESSION_ID", "env-sid");
        let (sid, _) = resolve_session_id(None, pid, "cli").unwrap();
        assert_eq!(sid, "env-sid");
        std::env::remove_var("EDDA_SESSION_ID");

        // A process-carried id remains authoritative beside a live session.
        // Clean state dir first to avoid interference from concurrent sessions
        let state_dir = edda_store::project_dir(pid).join("state");
        if state_dir.exists() {
            for entry in std::fs::read_dir(&state_dir).unwrap() {
                let entry = entry.unwrap();
                if entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("session."))
                {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        let _ = std::fs::create_dir_all(&state_dir);
        let now = time::OffsetDateTime::now_utc();
        let now_str = now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        let hb = serde_json::json!({
            "session_id": "inferred-sess",
            "started_at": now_str,
            "last_heartbeat": now_str,
            "label": "worker",
            "focus_files": [],
            "active_tasks": [],
            "files_modified_count": 0,
            "total_edits": 0,
            "recent_commits": []
        });
        std::fs::write(
            state_dir.join("session.inferred-sess.json"),
            serde_json::to_string_pretty(&hb).unwrap(),
        )
        .unwrap();
        std::env::set_var("EDDA_SESSION_ID", "env-live-sid");
        let (sid, _) = resolve_session_id(None, pid, "cli").unwrap();
        assert_eq!(sid, "env-live-sid");
        std::env::set_var("EDDA_SESSION_ID", "");
        let err = resolve_session_id(None, pid, "cli")
            .expect_err("an empty env value must not adopt the sole heartbeat");
        assert!(err.to_string().contains("--session"), "{err}");
        std::env::remove_var("EDDA_SESSION_ID");
        let _ = std::fs::remove_file(state_dir.join("session.inferred-sess.json"));

        // Standalone fallback (no heartbeats, no env)
        let (sid, label) = resolve_session_id(None, pid, "cli").unwrap();
        assert_eq!(sid, "cli-cli");
        assert_eq!(label, "cli");

        // Tier 1 wins over Tier 2
        std::env::set_var("EDDA_SESSION_ID", "env-sid");
        let (sid, _) = resolve_session_id(Some("explicit-wins"), pid, "cli").unwrap();
        assert_eq!(sid, "explicit-wins", "tier 1 should beat tier 2");
        std::env::remove_var("EDDA_SESSION_ID");

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Render & Heartbeat CLI tests (Issue #15) ──

    #[test]
    fn render_writeback_contains_protocol() {
        let output = edda_bridge_claude::render::writeback();
        assert!(
            output.contains("Write-Back Protocol"),
            "should contain header"
        );
        assert!(output.contains("edda decide"), "should teach edda decide");
        assert!(output.contains("edda note"), "should teach edda note");
        assert!(
            output.contains("edda task done") && output.contains("--receipt"),
            "should teach the task rail verbs at the same level as decide/note (§5)"
        );
        assert!(
            output.contains("edda ask") && output.contains("edda search query"),
            "should teach the read verbs — read before you write, or the ledger is write-only"
        );
    }

    #[test]
    fn render_workspace_with_ledger() {
        let (tmp, _ledger) = setup_workspace();
        let cwd = tmp.to_str().unwrap();
        let result = edda_bridge_claude::render::workspace(cwd, 2500);
        assert!(
            result.is_some(),
            "workspace with ledger should produce output"
        );
        let text = result.unwrap();
        assert!(
            text.contains("Project") || text.contains("Branch"),
            "should contain workspace sections"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn render_workspace_no_ledger() {
        let result = edda_bridge_claude::render::workspace("/nonexistent/path", 2500);
        assert!(result.is_none(), "no workspace should return None");
    }

    #[test]
    fn render_coordination_solo_no_bindings() {
        let _store = crate::test_support::isolated_store();
        let pid = "test_render_coord_solo";
        let _ = edda_store::ensure_dirs(pid);
        let result = edda_bridge_claude::render::coordination(pid, "solo-session");
        // Solo with no bindings → None
        assert!(
            result.is_none(),
            "solo session with no bindings should return None"
        );
        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn render_pack_no_pack_file() {
        let _store = crate::test_support::isolated_store();
        let pid = "test_render_pack_empty";
        let _ = edda_store::ensure_dirs(pid);
        let result = edda_bridge_claude::render::pack(pid);
        assert!(result.is_none(), "no hot.md should return None");
        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    #[test]
    fn heartbeat_write_touch_remove_lifecycle() {
        let _store = crate::test_support::isolated_store();
        let pid = "test_hb_lifecycle";
        let sid = "sess-lifecycle-1";
        let _ = edda_store::ensure_dirs(pid);

        // Write
        edda_bridge_claude::peers::write_heartbeat_minimal(pid, sid, "worker", ".");
        let state_dir = edda_store::project_dir(pid).join("state");
        let hb_path = state_dir.join(format!("session.{sid}.json"));
        assert!(hb_path.exists(), "heartbeat file should exist after write");

        // Verify label
        let content: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hb_path).unwrap()).unwrap();
        assert_eq!(content["label"].as_str().unwrap(), "worker");
        assert_eq!(content["session_id"].as_str().unwrap(), sid);

        // Touch
        let _mtime_before = std::fs::metadata(&hb_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        edda_bridge_claude::peers::touch_heartbeat(pid, sid);
        let content_after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hb_path).unwrap()).unwrap();
        // last_heartbeat string should have changed
        assert_ne!(
            content["last_heartbeat"].as_str().unwrap(),
            content_after["last_heartbeat"].as_str().unwrap(),
            "touch should update last_heartbeat"
        );

        // Remove
        edda_bridge_claude::peers::remove_heartbeat(pid, sid);
        assert!(
            !hb_path.exists(),
            "heartbeat file should be gone after remove"
        );

        let _ = std::fs::remove_dir_all(edda_store::project_dir(pid));
    }

    // ── Hook resilience tests (#83) ──

    #[test]
    fn catch_unwind_recovers_from_panic() {
        // Verify catch_unwind pattern works with panicking closures
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> anyhow::Result<String> {
                    panic!("test panic in hook");
                },
            ));
            let _ = tx.send(result);
        });

        let outcome = rx.recv_timeout(std::time::Duration::from_secs(5));
        assert!(outcome.is_ok(), "channel should receive");
        let inner = outcome.unwrap();
        assert!(inner.is_err(), "should be a caught panic");
        let panic_info = inner.unwrap_err();
        let msg = panic_info
            .downcast_ref::<&str>()
            .copied()
            .unwrap_or("unknown");
        assert_eq!(msg, "test panic in hook");
    }

    #[test]
    fn timeout_fires_on_slow_hook() {
        let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<String>>();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(60));
            let _ = tx.send(Ok("too late".to_string()));
        });

        let outcome = rx.recv_timeout(std::time::Duration::from_millis(50));
        assert!(
            outcome.is_err(),
            "should timeout before slow hook completes"
        );
    }

    #[test]
    fn hook_timeout_ms_defaults_to_60s() {
        let _env = env_guard();
        std::env::remove_var("EDDA_HOOK_TIMEOUT_MS");
        assert_eq!(hook_timeout_ms(), 60_000);
    }

    #[test]
    fn hook_timeout_ms_reads_env() {
        let _env = env_guard();
        std::env::set_var("EDDA_HOOK_TIMEOUT_MS", "5000");
        assert_eq!(hook_timeout_ms(), 5000);
        std::env::remove_var("EDDA_HOOK_TIMEOUT_MS");
    }

    // ── Request target validation (GH-443) ──

    #[test]
    fn request_to_unknown_label_is_rejected_unless_forced() {
        let _store = crate::test_support::isolated_store();
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "s-auth", "auth", ".");

        let err = request(repo.path(), "aut", "hi", Some("s-cli"), false)
            .expect_err("a typo'd label must not silently succeed");
        let msg = err.to_string();
        assert!(
            msg.contains("no active session answers to 'aut'"),
            "error should name the unreachable label: {msg}"
        );
        assert!(
            msg.contains("auth"),
            "error should list the labels that do exist: {msg}"
        );

        // --force is the escape hatch for a peer that has not started yet.
        request(repo.path(), "aut", "hi", Some("s-cli"), true).expect("--force should send anyway");
        // A live label needs no escape hatch.
        request(repo.path(), "auth", "hi", Some("s-cli"), false).expect("live label should send");

        let board = edda_bridge_claude::peers::compute_board_state(&pid);
        assert_eq!(board.requests.len(), 2, "both sent requests are recorded");
        assert!(
            !board.requests[0].id.is_empty(),
            "every request carries an id"
        );
        assert_ne!(
            board.requests[0].id, board.requests[1].id,
            "ids must be distinct per message"
        );
    }

    #[test]
    fn request_emits_request_pending_notification() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::set_var("EDDA_SESSION_ID", "s-auth");
        std::env::set_var("EDDA_SESSION_LABEL", "auth");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "s-auth", "auth", ".");
        let (url, rx, server) = webhook_capture();
        enable_webhook(repo.path(), &url);

        request(repo.path(), "billing", "need invoice type", None, true)
            .expect("forced request should succeed");
        let body = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("request creation should dispatch a notification");
        server.join().unwrap();

        assert!(
            body.contains("\"event_type\":\"request_pending\""),
            "{body}"
        );
        assert!(body.contains("\"from_label\":\"auth\""), "{body}");
        assert!(body.contains("\"to_label\":\"billing\""), "{body}");
        assert!(body.contains("\"message\":\"need invoice type\""), "{body}");
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
    }

    fn prior_claim(label: &str, paths: &[&str]) -> edda_bridge_claude::peers::ClaimEntry {
        edda_bridge_claude::peers::ClaimEntry {
            session_id: "s1".into(),
            label: label.into(),
            paths: paths.iter().map(|p| (*p).to_string()).collect(),
            ts: "2026-08-20T00:00:00Z".into(),
        }
    }

    fn owned(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|p| (*p).to_string()).collect()
    }

    // The disclosure lines are asserted here rather than only through the board,
    // because a test that reads `compute_board_state` passes whether or not
    // anything was printed -- the fold it checks pre-dates this change.

    #[test]
    fn a_first_claim_reports_no_replacement() {
        assert_eq!(
            claim_disclosure(None, "auth", &owned(&["src/auth/*"])),
            vec!["Claimed scope: auth"]
        );
    }

    #[test]
    fn a_new_label_names_the_claim_it_replaced() {
        let previous = prior_claim("auth", &["src/auth/*"]);
        assert_eq!(
            claim_disclosure(Some(&previous), "api", &owned(&["src/api/*"])),
            vec![
                "Claimed scope: api (replaces this session's earlier claim on auth)",
                "  released: src/auth/*",
            ]
        );
    }

    #[test]
    fn an_identical_re_claim_releases_nothing() {
        // The regression this verb exists to prevent, in its own image: the
        // first version printed "released: src/api/*" for a command that had
        // just re-claimed `src/api/*`. A bare-shell restart re-running its own
        // command hits this, so it is the common case, not a corner.
        let previous = prior_claim("api", &["src/api/*"]);
        assert_eq!(
            claim_disclosure(Some(&previous), "api", &owned(&["src/api/*"])),
            vec!["Re-claimed scope: api (unchanged)"],
            "an unchanged re-claim reports no release at all"
        );
    }

    #[test]
    fn narrowing_reports_only_the_path_it_gave_up() {
        let previous = prior_claim("api", &["src/api/*", "src/api/v2/*"]);
        assert_eq!(
            claim_disclosure(Some(&previous), "api", &owned(&["src/api/v2/*"])),
            vec![
                "Re-claimed scope: api (previous paths replaced)",
                "  released: src/api/*",
            ],
            "src/api/v2/* is still claimed, so it is not released"
        );
    }

    #[test]
    fn widening_reports_paths_added_but_no_release() {
        let previous = prior_claim("api", &["src/api/*"]);
        assert_eq!(
            claim_disclosure(
                Some(&previous),
                "api",
                &owned(&["src/api/*", "src/api/v3/*"])
            ),
            vec!["Re-claimed scope: api (paths added)"]
        );
    }

    #[test]
    fn a_relabel_that_keeps_every_path_releases_nothing() {
        let previous = prior_claim("auth", &["src/auth/*"]);
        assert_eq!(
            claim_disclosure(Some(&previous), "identity", &owned(&["src/auth/*"])),
            vec!["Claimed scope: identity (replaces this session's earlier claim on auth)"],
            "the label moved but the scope did not, so nothing was released"
        );
    }

    #[test]
    fn a_second_claim_leaves_one_claim_on_the_board() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);

        claim(repo.path(), "auth", &["src/auth/*".into()], Some("s1")).expect("first claim");
        claim(repo.path(), "api", &["src/api/*".into()], Some("s1")).expect("second claim");

        // The board folds to one claim per session, so the first scope is gone.
        // The disclosure tests above separately pin what the command prints.
        let claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
        assert_eq!(claims.len(), 1, "one session holds one claim");
        assert_eq!(claims[0].label, "api");
        assert_eq!(claims[0].paths, vec!["src/api/*".to_string()]);
    }

    #[test]
    fn bare_claim_beside_one_live_session_refuses_and_preserves_scope() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        edda_store::ensure_dirs(&pid).expect("store dirs");
        edda_bridge_claude::peers::write_heartbeat_minimal(
            &pid,
            "live-worker",
            "worker",
            "/tmp/worker",
        );
        edda_bridge_claude::peers::write_claim(
            &pid,
            "live-worker",
            "worker",
            &["src/worker.rs".into()],
        );

        let err = claim(repo.path(), "intruder", &["docs/*".into()], None)
            .expect_err("an adjacent shell must not adopt the live worker");
        assert!(err.to_string().contains("--session"), "{err}");

        let claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].session_id, "live-worker");
        assert_eq!(claims[0].label, "worker");
        assert_eq!(claims[0].paths, vec!["src/worker.rs".to_string()]);
    }

    #[test]
    fn re_claiming_the_same_label_keeps_one_claim() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);

        // Narrowing a scope, and re-claiming after a restart, both go through
        // this path -- which is why replacement is right and rejecting a second
        // claim would not be.
        claim(
            repo.path(),
            "auth",
            &["src/auth/*".into(), "src/token/*".into()],
            Some("s1"),
        )
        .expect("first claim");
        claim(repo.path(), "auth", &["src/auth/*".into()], Some("s1")).expect("narrowed claim");

        let claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
        assert_eq!(claims.len(), 1);
        assert_eq!(claims[0].paths, vec!["src/auth/*".to_string()]);
    }

    #[test]
    fn one_session_claiming_does_not_disturb_another() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);

        claim(repo.path(), "auth", &["src/auth/*".into()], Some("s1")).expect("s1 claim");
        claim(repo.path(), "api", &["src/api/*".into()], Some("s2")).expect("s2 claim");

        let mut claims = edda_bridge_claude::peers::compute_board_state(&pid).claims;
        claims.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        assert_eq!(claims.len(), 2, "the fold is per session, not global");
        assert_eq!(claims[0].label, "auth");
        assert_eq!(claims[1].label, "api");
    }

    #[test]
    fn unclaim_releases_the_explicit_session_scope() {
        let _store = crate::test_support::isolated_store();
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_claim(&pid, "s1", "auth", &["src/auth.rs".into()]);

        unclaim(repo.path(), Some("s1"), false).expect("unclaim should write a release event");

        assert!(edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .is_empty());
    }

    #[test]
    fn unclaim_without_identity_refuses_rather_than_guessing_the_sole_claim() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

        let err = unclaim(repo.path(), None, false)
            .expect_err("a caller with no identity must not guess");
        assert!(err.to_string().contains("cli-auth"), "{err}");

        assert_eq!(
            edda_bridge_claude::peers::compute_board_state(&pid)
                .claims
                .len(),
            1,
            "a refused unclaim must release nothing"
        );
    }

    #[test]
    fn unclaim_without_identity_never_releases_a_live_peers_claim() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);

        // Only one of two live peers holds a claim. A shell with no identity
        // of its own must not decide that claim is its to release:
        // check_offlimits enforces exactly this claim for its live owner.
        edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "sess-a", "worker-a", "/tmp/a");
        edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "sess-b", "worker-b", "/tmp/b");
        edda_bridge_claude::peers::write_claim(&pid, "sess-a", "worker-a", &["src/a.rs".into()]);

        let err = unclaim(repo.path(), None, false)
            .expect_err("a bare shell must not release another live session's scope");
        assert!(err.to_string().contains("sess-a"), "{err}");

        assert_eq!(
            edda_bridge_claude::peers::compute_board_state(&pid)
                .claims
                .len(),
            1,
            "the live peer's claim must survive"
        );
    }

    #[test]
    fn unclaim_without_identity_never_releases_the_sole_live_peers_claim() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        edda_store::ensure_dirs(&pid).expect("store dirs");
        edda_bridge_claude::peers::write_heartbeat_minimal(
            &pid,
            "sole-live-worker",
            "worker",
            "/tmp/worker",
        );
        edda_bridge_claude::peers::write_claim(
            &pid,
            "sole-live-worker",
            "worker",
            &["src/worker.rs".into()],
        );

        let err = unclaim(repo.path(), None, false)
            .expect_err("an adjacent shell must not release the sole live worker");
        assert!(err.to_string().contains("--session"), "{err}");
        assert_eq!(
            edda_bridge_claude::peers::compute_board_state(&pid)
                .claims
                .len(),
            1,
            "the live worker's claim must survive"
        );
    }

    #[test]
    fn unclaim_without_session_refuses_when_several_claims_exist() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);
        edda_bridge_claude::peers::write_claim(&pid, "cli-api", "api", &["src/api.rs".into()]);

        let err = unclaim(repo.path(), None, false).expect_err("ambiguous target must not guess");
        let msg = err.to_string();
        assert!(msg.contains("cli-auth") && msg.contains("cli-api"), "{msg}");

        assert_eq!(
            edda_bridge_claude::peers::compute_board_state(&pid)
                .claims
                .len(),
            2,
            "a refused unclaim must release nothing"
        );
    }

    #[test]
    fn unclaim_refuses_when_the_board_is_empty() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);

        unclaim(repo.path(), None, false).expect_err("nothing to release must not report success");
    }

    #[test]
    fn unclaim_refuses_a_session_that_holds_no_claim() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

        // This is the exact silent-failure this fix exists to remove: the old
        // fallback resolved to `cli-cli`, wrote an unclaim for a session that
        // held nothing, printed success, and left the real claim standing.
        let err = unclaim(repo.path(), Some("cli-cli"), false)
            .expect_err("releasing nothing must not report success");
        assert!(err.to_string().contains("cli-auth"), "{err}");

        assert_eq!(
            edda_bridge_claude::peers::compute_board_state(&pid)
                .claims
                .len(),
            1,
            "the real claim must survive a refused unclaim"
        );
    }

    #[test]
    fn if_claimed_exits_zero_when_there_is_nothing_to_release() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);

        // A CI teardown runs the verb unconditionally; the normal case of
        // nothing left to release must not fail the job (GH-488).
        unclaim(repo.path(), None, true).expect("empty board is not an error under --if-claimed");
        unclaim(repo.path(), Some("cli-nobody"), true)
            .expect("a session holding nothing is not an error either");
    }

    #[test]
    fn if_claimed_still_releases_a_real_claim() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

        // The flag softens the failure, not the work.
        unclaim(repo.path(), Some("cli-auth"), true).expect("release still happens");

        assert!(edda_bridge_claude::peers::compute_board_state(&pid)
            .claims
            .is_empty());
    }

    #[test]
    fn if_claimed_does_not_excuse_an_ambiguous_target() {
        let _store = crate::test_support::isolated_store();
        let _env = env_guard();
        std::env::remove_var("EDDA_SESSION_ID");
        std::env::remove_var("EDDA_SESSION_LABEL");
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_claim(&pid, "cli-auth", "auth", &["src/auth.rs".into()]);

        // Two claims and no identity is not "nothing to release" -- it is a
        // caller who cannot say which claim is theirs, and silence there would
        // be the hazard GH-488 exists to remove. Teardown only excuses absence.
        edda_bridge_claude::peers::write_claim(&pid, "cli-api", "api", &["src/api.rs".into()]);

        unclaim(repo.path(), None, true).expect("teardown treats an unresolvable target as absent");

        assert_eq!(
            edda_bridge_claude::peers::compute_board_state(&pid)
                .claims
                .len(),
            2,
            "and it must still release nothing"
        );
    }

    #[test]
    fn peers_json_claims_carry_staleness() {
        // GH-569: programs reading `edda peers --json` must be able to make
        // the same live-vs-stale judgement the human view makes. Claims are
        // entries of that surface, so each carries its age and a stale flag.
        let _store = crate::test_support::isolated_store();
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_claim(&pid, "s1", "auth", &["src/auth.rs".into()]);

        let json = peers_json(&pid);
        let claim = &json["claims"][0];
        assert!(
            claim["age_secs"].is_u64(),
            "claim carries age_secs: {claim}"
        );
        assert_eq!(claim["stale"], false, "fresh claim is not stale");
    }

    #[test]
    fn peers_json_includes_sessions_and_full_board() {
        let _store = crate::test_support::isolated_store();
        let repo = tempfile::tempdir().expect("tempdir");
        let pid = edda_store::project_id(repo.path());
        let _ = edda_store::ensure_dirs(&pid);
        edda_bridge_claude::peers::write_heartbeat_minimal(&pid, "s1", "auth", ".");
        edda_bridge_claude::peers::write_claim(&pid, "s1", "auth", &["src/auth.rs".into()]);
        edda_bridge_claude::peers::write_request(&pid, "s2", "billing", "auth", "need auth");
        edda_bridge_claude::peers::write_request_ack(&pid, "s1", "billing");

        let json = peers_json(&pid);
        assert_eq!(json["sessions"][0]["session_id"], "s1");
        assert_eq!(json["claims"][0]["label"], "auth");
        assert_eq!(json["requests"][0]["message"], "need auth");
        assert_eq!(json["acks"][0]["from_label"], "billing");
    }
}
