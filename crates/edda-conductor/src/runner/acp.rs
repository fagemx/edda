//! ACP transport for task-rail agents.
//!
//! The runner deliberately advertises neither client filesystem nor terminal
//! capabilities.  Agent-side tools remain available through the agent itself,
//! while client callbacks are constrained by [`AcpPermissionPolicy`].

use acp::Agent as _;
use agent_client_protocol as acp;
use anyhow::{Context, Result};
use edda_core::event::{finalize_event, new_note_event, new_task_session_event};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;
use serde::Serialize;
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::task::LocalSet;
use tokio::time::timeout;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};
use tokio_util::sync::CancellationToken;

/// Setup is bounded independently from an ACP turn: some targets take minutes
/// to create or reload a session, but must not hang a cancelled task forever.
const SETUP_TIMEOUT: Duration = Duration::from_secs(300);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// A command-line ACP endpoint. `program` and `args` are never written to the
/// ledger because package-manager arguments may contain credentials.
#[derive(Debug, Clone)]
pub struct AcpEndpoint {
    pub program: PathBuf,
    pub args: Vec<String>,
}

/// Input for one new or resumed ACP turn.
#[derive(Debug, Clone)]
pub struct AcpTaskRequest {
    pub task_id: u64,
    pub worktree: PathBuf,
    pub prompt: String,
    pub endpoint: AcpEndpoint,
    pub mcp_server: AcpEndpoint,
    pub policy: AcpPermissionPolicy,
    /// Optional caller-selected prompt budget. `None` preserves a sustained
    /// task rail rather than imposing the former arbitrary 30-second limit.
    pub prompt_timeout: Option<Duration>,
    /// A previous `task.session` ACP id. When present the runner uses
    /// `session/load`, never silently creates a replacement session.
    pub resume_session_id: Option<String>,
}

/// Honest result of a prompt turn. Usage is optional in ACP; `None` means the
/// agent did not report a measurement, never a zero-cost or zero-token guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcpTurnResult {
    pub session_id: String,
    pub stop_reason: acp::StopReason,
    pub usage: Option<AcpUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AcpUsage {
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl From<acp::Usage> for AcpUsage {
    fn from(value: acp::Usage) -> Self {
        Self {
            total_tokens: value.total_tokens,
            input_tokens: value.input_tokens,
            output_tokens: value.output_tokens,
        }
    }
}

/// A bounded task-scope policy. It chooses only an `allow_once` option and
/// only for a fully located operation inside the task roots. `allow_always`,
/// unlocated requests, unknown option kinds, peer-owned paths, and verifier
/// requests fail closed.
#[derive(Debug, Clone)]
pub struct AcpPermissionPolicy {
    task_roots: Vec<PathBuf>,
    peer_roots: Vec<PathBuf>,
    verifier: bool,
}

impl AcpPermissionPolicy {
    pub fn new(task_roots: Vec<PathBuf>, peer_roots: Vec<PathBuf>, verifier: bool) -> Result<Self> {
        if task_roots.is_empty() {
            anyhow::bail!("ACP policy requires at least one task root");
        }
        let task_roots = task_roots
            .into_iter()
            .map(|path| canonical_existing(&path))
            .collect::<Result<Vec<_>>>()?;
        let peer_roots = peer_roots
            .into_iter()
            .map(|path| canonical_existing(&path))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            task_roots,
            peer_roots,
            verifier,
        })
    }

    /// Select the narrowly scoped `allow_once` option when the complete set
    /// of reported locations is inside the task scope. A request with no
    /// locations cannot prove its target and is denied.
    pub fn select_option(&self, request: &acp::RequestPermissionRequest) -> Option<String> {
        if self.verifier
            || !locations_are_scoped(&request.tool_call, &self.task_roots, &self.peer_roots)
        {
            return reject_option(&request.options);
        }
        request
            .options
            .iter()
            .find(|option| matches!(option.kind, acp::PermissionOptionKind::AllowOnce))
            .map(|option| option.option_id.0.to_string())
            .or_else(|| reject_option(&request.options))
    }
}

fn reject_option(options: &[acp::PermissionOption]) -> Option<String> {
    options
        .iter()
        .find(|option| matches!(option.kind, acp::PermissionOptionKind::RejectOnce))
        .or_else(|| {
            options
                .iter()
                .find(|option| matches!(option.kind, acp::PermissionOptionKind::RejectAlways))
        })
        .map(|option| option.option_id.0.to_string())
}

fn locations_are_scoped(
    tool_call: &acp::ToolCallUpdate,
    task_roots: &[PathBuf],
    peer_roots: &[PathBuf],
) -> bool {
    let Some(locations) = tool_call.fields.locations.as_ref() else {
        return false;
    };
    !locations.is_empty()
        && locations.iter().all(|location| {
            let Ok(path) = canonical_target(&location.path, task_roots) else {
                return false;
            };
            task_roots.iter().any(|root| path_is_within(&path, root))
                && !peer_roots.iter().any(|root| path_is_within(&path, root))
        })
}

fn canonical_target(path: &Path, task_roots: &[PathBuf]) -> Result<PathBuf> {
    let base = task_roots.first().context("ACP policy has no task root")?;
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    if candidate
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("ACP path contains traversal");
    }
    let mut existing = candidate.as_path();
    let mut tail = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .context("ACP target has no existing parent")?
            .to_os_string();
        tail.push(name);
        existing = existing.parent().context("ACP target escaped root")?;
    }
    let mut resolved = canonical_existing(existing)?;
    for part in tail.iter().rev() {
        resolved.push(part);
    }
    Ok(resolved)
}

fn canonical_existing(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("canonicalizing ACP scope path {}", path.display()))
}

fn path_is_within(candidate: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let candidate = candidate
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        let root = root
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        candidate == root
            || candidate
                .strip_prefix(&root)
                .is_some_and(|rest| rest.starts_with('\\'))
    }
    #[cfg(not(windows))]
    {
        candidate == root || candidate.starts_with(root)
    }
}

/// Durable audit sink for ACP lifecycle and permission decisions.
pub trait AcpAudit: Send + Sync {
    fn session_created(&self, task_id: u64, session_id: &str) -> Result<()>;
    fn decision(&self, task_id: u64, kind: &'static str, allowed: bool) -> Result<()>;
    fn update(&self, task_id: u64, kind: &'static str) -> Result<()>;
    /// Persist only the measured-ness and numeric usage facts, never a raw
    /// provider response or prompt payload.
    fn usage(&self, task_id: u64, usage: Option<&AcpUsage>) -> Result<()>;
}

/// Ledger-backed ACP audit. The session event is written immediately after a
/// successful `session/new`, before any prompt can make side effects.
pub struct LedgerAcpAudit {
    workspace: PathBuf,
}

impl LedgerAcpAudit {
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    fn append_note(&self, task_id: u64, kind: &'static str, allowed: bool) -> Result<()> {
        let ledger = Ledger::open(&self.workspace).context("opening ACP task ledger")?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).context("locking ACP task ledger")?;
        let branch = ledger.head_branch().context("reading ACP ledger branch")?;
        let parent = ledger
            .last_event_hash()
            .context("reading ACP ledger head")?;
        let tags = vec!["acp".to_string(), kind.to_string()];
        let mut event = new_note_event(
            &branch,
            parent.as_deref(),
            "agent",
            "ACP policy decision",
            &tags,
        )?;
        event.payload["acp"] = serde_json::json!({
            "task_id": task_id,
            "kind": kind,
            "allowed": allowed,
        });
        finalize_event(&mut event)?;
        ledger
            .append_event(&event)
            .context("appending ACP audit event")?;
        Ok(())
    }
}

impl AcpAudit for LedgerAcpAudit {
    fn session_created(&self, task_id: u64, session_id: &str) -> Result<()> {
        let ledger = Ledger::open(&self.workspace).context("opening ACP task ledger")?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).context("locking ACP task ledger")?;
        let branch = ledger.head_branch().context("reading ACP ledger branch")?;
        let parent = ledger
            .last_event_hash()
            .context("reading ACP ledger head")?;
        let event = new_task_session_event(&branch, parent.as_deref(), task_id, session_id)?;
        ledger
            .append_event(&event)
            .context("appending task.session")?;
        Ok(())
    }

    fn decision(&self, task_id: u64, kind: &'static str, allowed: bool) -> Result<()> {
        self.append_note(task_id, kind, allowed)
    }

    fn update(&self, task_id: u64, kind: &'static str) -> Result<()> {
        self.append_note(task_id, kind, true)
    }

    fn usage(&self, task_id: u64, usage: Option<&AcpUsage>) -> Result<()> {
        let ledger = Ledger::open(&self.workspace).context("opening ACP task ledger")?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).context("locking ACP task ledger")?;
        let branch = ledger.head_branch().context("reading ACP ledger branch")?;
        let parent = ledger
            .last_event_hash()
            .context("reading ACP ledger head")?;
        let mut event = new_note_event(
            &branch,
            parent.as_deref(),
            "agent",
            "ACP usage receipt",
            &["acp".into(), "usage".into()],
        )?;
        event.payload["acp"] = serde_json::json!({
            "task_id": task_id,
            "measured": usage.is_some(),
            "usage": usage,
        });
        finalize_event(&mut event)?;
        ledger
            .append_event(&event)
            .context("appending ACP usage receipt")
    }
}

/// Real ACP runner. It is intentionally separate from the old `claude -p`
/// launcher; integration selects this type explicitly rather than changing
/// legacy backend behavior by accident.
pub struct AcpRunner {
    audit: Arc<dyn AcpAudit>,
}

impl AcpRunner {
    pub fn new(audit: Arc<dyn AcpAudit>) -> Self {
        Self { audit }
    }

    pub async fn run(
        &self,
        request: AcpTaskRequest,
        cancel: CancellationToken,
    ) -> Result<AcpTurnResult> {
        let worktree = canonical_existing(&request.worktree)?;
        let mut child = spawn_agent(&request.endpoint, &worktree)?;
        let stdin = child.stdin.take().context("ACP child has no stdin")?;
        let stdout = child.stdout.take().context("ACP child has no stdout")?;
        // Failure cleanup: whatever `drive` returns, the stdio child is
        // drained (and killed) before the outcome reaches the caller, so no
        // ACP agent outlives its turn.
        let result = self.drive(stdin, stdout, request, cancel).await;
        drain_child(&mut child).await;
        result
    }

    /// Drive one ACP turn over an established stdio transport. Split from
    /// [`AcpRunner::run`] so offline fake-agent tests exercise the identical
    /// initialize/new/load/prompt flow without spawning a provider binary.
    async fn drive<W, R>(
        &self,
        stdin: W,
        stdout: R,
        request: AcpTaskRequest,
        cancel: CancellationToken,
    ) -> Result<AcpTurnResult>
    where
        W: tokio::io::AsyncWrite + Unpin + 'static,
        R: tokio::io::AsyncRead + Unpin + 'static,
    {
        let worktree = canonical_existing(&request.worktree)?;
        let audit = Arc::clone(&self.audit);
        let local = LocalSet::new();
        local
            .run_until(async move {
                let client = AcpClient {
                    task_id: request.task_id,
                    policy: request.policy,
                    audit: Arc::clone(&audit),
                };
                let (connection, io) = acp::ClientSideConnection::new(
                    client,
                    stdin.compat_write(),
                    stdout.compat(),
                    |future| {
                        tokio::task::spawn_local(future);
                    },
                );
                tokio::task::spawn_local(async move {
                    let _ = io.await;
                });

                await_acp_stage(
                    "initialize",
                    SETUP_TIMEOUT,
                    &cancel,
                    request.task_id,
                    &audit,
                    connection.initialize(
                        acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                            .client_capabilities(acp::ClientCapabilities::default())
                            .client_info(acp::Implementation::new(
                                "edda",
                                env!("CARGO_PKG_VERSION"),
                            )),
                    ),
                )
                .await?;

                let servers = vec![acp::McpServer::Stdio(
                    acp::McpServerStdio::new("edda-mcp", request.mcp_server.program)
                        .args(request.mcp_server.args),
                )];
                let session_id = if let Some(session_id) = request.resume_session_id {
                    audit.decision(request.task_id, "session_load", true)?;
                    await_acp_stage(
                        "session/load",
                        SETUP_TIMEOUT,
                        &cancel,
                        request.task_id,
                        &audit,
                        connection.load_session(
                            acp::LoadSessionRequest::new(session_id.clone(), worktree.clone())
                                .mcp_servers(servers),
                        ),
                    )
                    .await?;
                    session_id
                } else {
                    let created = await_acp_stage(
                        "session/new",
                        SETUP_TIMEOUT,
                        &cancel,
                        request.task_id,
                        &audit,
                        connection.new_session(
                            acp::NewSessionRequest::new(worktree.clone()).mcp_servers(servers),
                        ),
                    )
                    .await?;
                    let session_id = created.session_id.0.to_string();
                    audit.session_created(request.task_id, &session_id)?;
                    session_id
                };
                let prompt =
                    acp::PromptRequest::new(session_id.clone(), vec![request.prompt.into()]);
                let response = if let Some(prompt_timeout) = request.prompt_timeout {
                    await_acp_stage(
                        "session/prompt",
                        prompt_timeout,
                        &cancel,
                        request.task_id,
                        &audit,
                        connection.prompt(prompt),
                    )
                    .await?
                } else {
                    tokio::select! {
                        result = connection.prompt(prompt) => result.context("ACP session/prompt")?,
                        _ = cancel.cancelled() => {
                            audit.decision(request.task_id, "cancel", false)?;
                            let _ = timeout(DRAIN_TIMEOUT, connection.cancel(acp::CancelNotification::new(session_id.clone()))).await;
                            anyhow::bail!("ACP session/prompt cancelled")
                        }
                    }
                };
                let usage = effective_usage(&response);
                audit.usage(request.task_id, usage.as_ref())?;
                Ok(AcpTurnResult {
                    session_id,
                    stop_reason: response.stop_reason,
                    usage,
                })
            })
            .await
    }
}

struct AcpClient {
    task_id: u64,
    policy: AcpPermissionPolicy,
    audit: Arc<dyn AcpAudit>,
}

#[async_trait::async_trait(?Send)]
impl acp::Client for AcpClient {
    async fn request_permission(
        &self,
        request: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        let selected = self.policy.select_option(&request);
        let allowed = request.options.iter().any(|option| {
            selected.as_deref() == Some(option.option_id.0.as_ref())
                && matches!(option.kind, acp::PermissionOptionKind::AllowOnce)
        });
        self.audit
            .decision(self.task_id, "permission", allowed)
            .map_err(|_| acp::Error::internal_error())?;
        let outcome = selected
            .map(|id| {
                acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(id))
            })
            .unwrap_or(acp::RequestPermissionOutcome::Cancelled);
        Ok(acp::RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(&self, update: acp::SessionNotification) -> acp::Result<()> {
        let kind = match update.update {
            acp::SessionUpdate::UserMessageChunk(_) => "user_message",
            acp::SessionUpdate::AgentMessageChunk(_) => "agent_message",
            acp::SessionUpdate::AgentThoughtChunk(_) => "agent_thought",
            acp::SessionUpdate::ToolCall(_) => "tool_call",
            acp::SessionUpdate::ToolCallUpdate(_) => "tool_call_update",
            acp::SessionUpdate::Plan(_) => "plan",
            acp::SessionUpdate::AvailableCommandsUpdate(_) => "commands",
            acp::SessionUpdate::CurrentModeUpdate(_) => "mode",
            acp::SessionUpdate::ConfigOptionUpdate(_) => "config",
            acp::SessionUpdate::SessionInfoUpdate(_) => "session_info",
            #[allow(unreachable_patterns)]
            _ => "unknown",
        };
        self.audit
            .update(self.task_id, kind)
            .map_err(|_| acp::Error::internal_error())?;
        Ok(())
    }
}

/// Measured usage from a prompt result. The typed `usage` field wins; when
/// the agent reports nothing there, `_meta.usage` (Grok Build's carrier,
/// camelCase keys) is parsed. Both paths are measurements reported by the
/// agent itself: if neither is present the turn is honestly `measured:
/// false` — never a zero-cost guess.
fn effective_usage(response: &acp::PromptResponse) -> Option<AcpUsage> {
    if let Some(usage) = response.usage.as_ref() {
        return Some(AcpUsage {
            total_tokens: usage.total_tokens,
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
        });
    }
    let usage = response.meta.as_ref()?.get("usage")?;
    Some(AcpUsage {
        total_tokens: usage.get("totalTokens")?.as_u64()?,
        input_tokens: usage.get("inputTokens")?.as_u64()?,
        output_tokens: usage.get("outputTokens")?.as_u64()?,
    })
}

/// Await one setup or bounded prompt RPC while also observing cancellation.
/// The child is always drained after `run` returns, so cancellation during
/// initialize/new/load cannot strand a stdio agent before a session exists.
async fn await_acp_stage<T, F>(
    stage: &'static str,
    budget: Duration,
    cancel: &CancellationToken,
    task_id: u64,
    audit: &Arc<dyn AcpAudit>,
    future: F,
) -> Result<T>
where
    F: Future<Output = acp::Result<T>>,
{
    tokio::select! {
        result = timeout(budget, future) => {
            result
                .with_context(|| format!("ACP {stage} timeout"))?
                .with_context(|| format!("ACP {stage}"))
        }
        _ = cancel.cancelled() => {
            audit.decision(task_id, "cancel", false)?;
            anyhow::bail!("ACP {stage} cancelled")
        }
    }
}

fn spawn_agent(endpoint: &AcpEndpoint, cwd: &Path) -> Result<Child> {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("cmd");
        command
            .arg("/C")
            .arg(&endpoint.program)
            .args(&endpoint.args);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new(&endpoint.program);
        command.args(&endpoint.args);
        command
    };
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_remove("CLAUDE_CODE")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .env_remove("CLAUDE_CODE_SSE_PORT")
        .kill_on_drop(true)
        .spawn()
        .context("spawning ACP agent")
}

async fn drain_child(child: &mut Child) {
    if timeout(DRAIN_TIMEOUT, child.wait()).await.is_err() {
        let _ = child.kill().await;
        let _ = timeout(DRAIN_TIMEOUT, child.wait()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acp::Client as _;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryAudit {
        events: Mutex<Vec<String>>,
    }

    impl MemoryAudit {
        fn push(&self, event: String) {
            self.events.lock().unwrap().push(event);
        }

        fn flat(&self) -> Vec<String> {
            self.events.lock().unwrap().clone()
        }
    }

    impl AcpAudit for MemoryAudit {
        fn session_created(&self, _task_id: u64, session_id: &str) -> Result<()> {
            self.push(format!("new:{session_id}"));
            Ok(())
        }

        fn decision(&self, _task_id: u64, kind: &'static str, allowed: bool) -> Result<()> {
            self.push(format!("{kind}:allow={allowed}"));
            Ok(())
        }

        fn update(&self, _task_id: u64, kind: &'static str) -> Result<()> {
            self.push(format!("update:{kind}"));
            Ok(())
        }

        fn usage(&self, _task_id: u64, usage: Option<&AcpUsage>) -> Result<()> {
            self.push(format!("usage:measured={}", usage.is_some()));
            Ok(())
        }
    }

    /// What the fake agent's one prompt handler does.
    #[derive(Clone)]
    enum FakeTurn {
        /// Reply `EndTurn` immediately, optionally carrying `_meta.usage`
        /// (usage present/absent evidence).
        Plain(Option<(u64, u64, u64)>),
        /// Issue one server→client permission request for `path`, record the
        /// observed outcome, then end the turn (wire-level policy evidence).
        Permission { path: String },
        /// Report that the prompt was entered, then never reply — the client
        /// turn must be cancelled mid-prompt (kill/restart evidence).
        Hang,
    }

    #[derive(Default)]
    struct FakeState {
        calls: Vec<&'static str>,
        prompts: Vec<String>,
        loads: Vec<String>,
        permission_outcomes: Vec<bool>,
    }

    /// A permission request the fake agent's prompt handler wants issued
    /// through the real server→client path.
    struct PermissionJob {
        session_id: String,
        path: String,
        reply: tokio::sync::oneshot::Sender<bool>,
    }

    /// A deterministic in-process ACP agent. It speaks the same protocol the
    /// real targets do — typed initialize/new/load/prompt and server→client
    /// permission requests — over in-memory duplex streams, so the offline
    /// evidence never touches an unavailable provider.
    struct FakeAcpAgent {
        state: Arc<Mutex<FakeState>>,
        turn: FakeTurn,
        fail_new_session: bool,
        permission_tx: tokio::sync::mpsc::UnboundedSender<PermissionJob>,
        prompt_started_tx: Option<tokio::sync::mpsc::UnboundedSender<()>>,
    }

    impl FakeAcpAgent {
        fn new(
            state: Arc<Mutex<FakeState>>,
            turn: FakeTurn,
            permission_tx: tokio::sync::mpsc::UnboundedSender<PermissionJob>,
        ) -> Self {
            Self {
                state,
                turn,
                fail_new_session: false,
                permission_tx,
                prompt_started_tx: None,
            }
        }

        fn hanging(mut self, tx: tokio::sync::mpsc::UnboundedSender<()>) -> Self {
            self.prompt_started_tx = Some(tx);
            self
        }

        fn failing_new_session(mut self) -> Self {
            self.fail_new_session = true;
            self
        }
    }

    fn prompt_text(blocks: &[acp::ContentBlock]) -> String {
        blocks
            .iter()
            .filter_map(|block| match block {
                acp::ContentBlock::Text(text) => Some(text.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("")
    }

    #[async_trait::async_trait(?Send)]
    impl acp::Agent for FakeAcpAgent {
        async fn initialize(
            &self,
            _request: acp::InitializeRequest,
        ) -> acp::Result<acp::InitializeResponse> {
            self.state.lock().unwrap().calls.push("initialize");
            Ok(acp::InitializeResponse::new(acp::ProtocolVersion::V1))
        }

        async fn authenticate(
            &self,
            _request: acp::AuthenticateRequest,
        ) -> acp::Result<acp::AuthenticateResponse> {
            Ok(acp::AuthenticateResponse::default())
        }

        async fn new_session(
            &self,
            _request: acp::NewSessionRequest,
        ) -> acp::Result<acp::NewSessionResponse> {
            if self.fail_new_session {
                // A failed setup must persist nothing: no session event, no
                // usage receipt, no call record.
                return Err(acp::Error::internal_error());
            }
            self.state.lock().unwrap().calls.push("new");
            Ok(acp::NewSessionResponse::new("fake-session"))
        }

        async fn load_session(
            &self,
            request: acp::LoadSessionRequest,
        ) -> acp::Result<acp::LoadSessionResponse> {
            {
                let mut state = self.state.lock().unwrap();
                state.calls.push("load");
                state.loads.push(request.session_id.0.to_string());
            }
            Ok(acp::LoadSessionResponse::new())
        }

        async fn set_session_mode(
            &self,
            _request: acp::SetSessionModeRequest,
        ) -> acp::Result<acp::SetSessionModeResponse> {
            Ok(acp::SetSessionModeResponse::default())
        }

        async fn prompt(&self, request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
            {
                let mut state = self.state.lock().unwrap();
                state.calls.push("prompt");
                state.prompts.push(prompt_text(&request.prompt));
            }
            match self.turn.clone() {
                FakeTurn::Hang => {
                    if let Some(tx) = self.prompt_started_tx.as_ref() {
                        let _ = tx.send(());
                    }
                    return std::future::pending::<acp::Result<acp::PromptResponse>>().await;
                }
                FakeTurn::Permission { path } => {
                    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
                    let _ = self.permission_tx.send(PermissionJob {
                        session_id: request.session_id.0.to_string(),
                        path,
                        reply: reply_tx,
                    });
                    let allowed = reply_rx.await.expect("fake permission executor alive");
                    self.state.lock().unwrap().permission_outcomes.push(allowed);
                }
                FakeTurn::Plain(_) => {}
            }
            let mut response = acp::PromptResponse::new(acp::StopReason::EndTurn);
            if let FakeTurn::Plain(Some((total, input, output))) = self.turn {
                let mut meta = serde_json::Map::new();
                meta.insert(
                    "usage".to_string(),
                    serde_json::json!({
                        "totalTokens": total,
                        "inputTokens": input,
                        "outputTokens": output,
                    }),
                );
                response.meta = Some(meta);
            }
            Ok(response)
        }

        async fn cancel(&self, _request: acp::CancelNotification) -> acp::Result<()> {
            Ok(())
        }

        async fn set_session_config_option(
            &self,
            _request: acp::SetSessionConfigOptionRequest,
        ) -> acp::Result<acp::SetSessionConfigOptionResponse> {
            Ok(acp::SetSessionConfigOptionResponse::new(vec![]))
        }
    }

    /// Wire one fake agent turn to a duplex pair. Returns the client-side
    /// halves for [`AcpRunner::drive`] and runs the executor that answers
    /// the fake's permission jobs through the real server→client request
    /// path (so the runner's permission handling is exercised, not mocked).
    fn start_fake_turn(
        agent: FakeAcpAgent,
        mut permission_rx: tokio::sync::mpsc::UnboundedReceiver<PermissionJob>,
    ) -> (
        impl tokio::io::AsyncWrite + Unpin,
        impl tokio::io::AsyncRead + Unpin,
    ) {
        let (client_stream, agent_stream) = tokio::io::duplex(8192);
        let (agent_read, agent_write) = tokio::io::split(agent_stream);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (connection, io) = acp::AgentSideConnection::new(
            agent,
            agent_write.compat_write(),
            agent_read.compat(),
            |future| {
                tokio::task::spawn_local(future);
            },
        );
        tokio::task::spawn_local(async move {
            let _ = io.await;
        });
        tokio::task::spawn_local(async move {
            while let Some(job) = permission_rx.recv().await {
                let tool_call = acp::ToolCallUpdate::new(
                    acp::ToolCallId::new("fake-tool"),
                    acp::ToolCallUpdateFields::new()
                        .locations(vec![acp::ToolCallLocation::new(PathBuf::from(&job.path))]),
                );
                let request = acp::RequestPermissionRequest::new(
                    acp::SessionId::new(job.session_id),
                    tool_call,
                    vec![
                        acp::PermissionOption::new(
                            "allow-once",
                            "Allow once",
                            acp::PermissionOptionKind::AllowOnce,
                        ),
                        acp::PermissionOption::new(
                            "reject-once",
                            "Reject once",
                            acp::PermissionOptionKind::RejectOnce,
                        ),
                    ],
                );
                let allowed = match connection.request_permission(request).await {
                    Ok(response) => match response.outcome {
                        acp::RequestPermissionOutcome::Selected(selected) => {
                            selected.option_id.0.as_ref() == "allow-once"
                        }
                        _ => false,
                    },
                    // A hung-up client answered nothing; record the job as denied.
                    Err(_) => false,
                };
                let _ = job.reply.send(allowed);
            }
        });
        (client_write, client_read)
    }

    fn fake_policy(root: &Path) -> AcpPermissionPolicy {
        AcpPermissionPolicy::new(vec![root.to_path_buf()], vec![], false).unwrap()
    }

    fn fake_request(
        worktree: &Path,
        policy: AcpPermissionPolicy,
        resume: Option<&str>,
        prompt: &str,
    ) -> AcpTaskRequest {
        AcpTaskRequest {
            task_id: 42,
            worktree: worktree.to_path_buf(),
            prompt: prompt.to_string(),
            // `drive` never spawns a child, so the endpoint is inert here.
            endpoint: AcpEndpoint {
                program: PathBuf::from("fake-acp-agent"),
                args: vec![],
            },
            mcp_server: AcpEndpoint {
                program: PathBuf::from("edda"),
                args: vec!["mcp".into(), "serve".into()],
            },
            policy,
            prompt_timeout: None,
            resume_session_id: resume.map(str::to_string),
        }
    }

    fn option(id: &str, kind: acp::PermissionOptionKind) -> acp::PermissionOption {
        acp::PermissionOption::new(id.to_string(), id, kind)
    }

    fn request(path: &Path, options: Vec<acp::PermissionOption>) -> acp::RequestPermissionRequest {
        acp::RequestPermissionRequest::new(
            "session",
            acp::ToolCallUpdate::new(
                "tool",
                acp::ToolCallUpdateFields::new().locations(vec![acp::ToolCallLocation::new(path)]),
            ),
            options,
        )
    }

    #[test]
    fn task_scope_allows_once_but_never_always() {
        let root = tempfile::tempdir().unwrap();
        let policy = AcpPermissionPolicy::new(vec![root.path().into()], vec![], false).unwrap();
        let inside = root.path().join("src/new.rs");
        let choice = policy.select_option(&request(
            &inside,
            vec![
                option("always", acp::PermissionOptionKind::AllowAlways),
                option("once", acp::PermissionOptionKind::AllowOnce),
                option("deny", acp::PermissionOptionKind::RejectOnce),
            ],
        ));
        assert_eq!(choice.as_deref(), Some("once"));
    }

    #[test]
    fn policy_denies_traversal_peer_scope_and_verifier() {
        let root = tempfile::tempdir().unwrap();
        let peer = root.path().join("peer");
        std::fs::create_dir(&peer).unwrap();
        let options = vec![
            option("allow", acp::PermissionOptionKind::AllowOnce),
            option("deny", acp::PermissionOptionKind::RejectOnce),
        ];
        let policy =
            AcpPermissionPolicy::new(vec![root.path().into()], vec![peer.clone()], false).unwrap();
        assert_eq!(
            policy
                .select_option(&request(&peer.join("x.rs"), options.clone()))
                .as_deref(),
            Some("deny")
        );
        assert_eq!(
            policy
                .select_option(&request(Path::new("../escape.rs"), options.clone()))
                .as_deref(),
            Some("deny")
        );
        let verifier = AcpPermissionPolicy::new(vec![root.path().into()], vec![], true).unwrap();
        assert_eq!(
            verifier
                .select_option(&request(&root.path().join("ok.rs"), options))
                .as_deref(),
            Some("deny")
        );
    }

    #[test]
    fn unlocated_request_fails_closed() {
        let root = tempfile::tempdir().unwrap();
        let policy = AcpPermissionPolicy::new(vec![root.path().into()], vec![], false).unwrap();
        let request = acp::RequestPermissionRequest::new(
            "session",
            acp::ToolCallUpdate::new("tool", acp::ToolCallUpdateFields::new()),
            vec![
                option("allow", acp::PermissionOptionKind::AllowOnce),
                option("deny", acp::PermissionOptionKind::RejectOnce),
            ],
        );
        assert_eq!(policy.select_option(&request).as_deref(), Some("deny"));
    }

    #[test]
    fn meta_usage_is_parsed_when_typed_field_is_absent() {
        let mut meta = serde_json::Map::new();
        meta.insert(
            "usage".to_string(),
            serde_json::json!({"totalTokens": 30u64, "inputTokens": 20u64, "outputTokens": 10u64}),
        );
        let mut response = acp::PromptResponse::new(acp::StopReason::EndTurn);
        response.meta = Some(meta);
        assert_eq!(
            effective_usage(&response),
            Some(AcpUsage {
                total_tokens: 30,
                input_tokens: 20,
                output_tokens: 10,
            })
        );
    }

    #[test]
    fn absent_usage_is_none_not_zero() {
        let response = acp::PromptResponse::new(acp::StopReason::EndTurn);
        assert_eq!(effective_usage(&response), None);
        // A malformed meta block must not invent a measurement either.
        let mut meta = serde_json::Map::new();
        meta.insert(
            "usage".to_string(),
            serde_json::json!({"totalTokens": "many"}),
        );
        let mut response = acp::PromptResponse::new(acp::StopReason::EndTurn);
        response.meta = Some(meta);
        assert_eq!(effective_usage(&response), None);
    }

    #[tokio::test]
    async fn setup_timeout_and_cancellation_are_bounded_and_audited() {
        let memory = Arc::new(MemoryAudit::default());
        let audit: Arc<dyn AcpAudit> = memory.clone();
        let timeout_cancel = CancellationToken::new();
        let timeout_result = await_acp_stage(
            "initialize",
            Duration::from_millis(1),
            &timeout_cancel,
            42,
            &audit,
            std::future::pending::<acp::Result<()>>(),
        )
        .await;
        assert!(timeout_result
            .unwrap_err()
            .to_string()
            .contains("initialize timeout"));

        let cancelled = CancellationToken::new();
        cancelled.cancel();
        let cancelled_result = await_acp_stage(
            "session/load",
            Duration::from_secs(1),
            &cancelled,
            42,
            &audit,
            std::future::pending::<acp::Result<()>>(),
        )
        .await;
        assert!(cancelled_result
            .unwrap_err()
            .to_string()
            .contains("session/load cancelled"));
        assert_eq!(memory.flat(), ["cancel:allow=false"]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fake_agent_exercises_initialize_new_prompt_and_load() {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let agent = FakeAcpAgent::new(
            Arc::clone(&state),
            FakeTurn::Plain(None),
            tokio::sync::mpsc::unbounded_channel().0,
        );
        let root = tempfile::tempdir().unwrap();
        let audit = Arc::new(MemoryAudit::default());
        let policy = AcpPermissionPolicy::new(vec![root.path().into()], vec![], false).unwrap();
        let (client_stream, agent_stream) = tokio::io::duplex(8192);
        let (client_read, client_write) = tokio::io::split(client_stream);
        let (agent_read, agent_write) = tokio::io::split(agent_stream);
        let local = LocalSet::new();
        local
            .run_until(async move {
                let (client, client_io) = acp::ClientSideConnection::new(
                    AcpClient {
                        task_id: 42,
                        policy,
                        audit,
                    },
                    client_write.compat_write(),
                    client_read.compat(),
                    |future| {
                        tokio::task::spawn_local(future);
                    },
                );
                let (_agent, agent_io) = acp::AgentSideConnection::new(
                    agent,
                    agent_write.compat_write(),
                    agent_read.compat(),
                    |future| {
                        tokio::task::spawn_local(future);
                    },
                );
                tokio::task::spawn_local(async move {
                    let _ = client_io.await;
                });
                tokio::task::spawn_local(async move {
                    let _ = agent_io.await;
                });

                client
                    .initialize(acp::InitializeRequest::new(acp::ProtocolVersion::V1))
                    .await
                    .unwrap();
                let session = client
                    .new_session(acp::NewSessionRequest::new(root.path()))
                    .await
                    .unwrap();
                assert_eq!(session.session_id.0.as_ref(), "fake-session");
                let response = client
                    .prompt(acp::PromptRequest::new(
                        "fake-session",
                        vec!["brief".into()],
                    ))
                    .await
                    .unwrap();
                assert_eq!(response.stop_reason, acp::StopReason::EndTurn);
                client
                    .load_session(acp::LoadSessionRequest::new("fake-session", root.path()))
                    .await
                    .unwrap();
            })
            .await;
        assert_eq!(
            state.lock().unwrap().calls,
            ["initialize", "new", "prompt", "load"]
        );
    }

    /// Two-step offline drill: turn 1 creates the session and reports a
    /// measured usage; the follow-up controller turn resumes the same
    /// session through `session/load` and honestly reports no measurement.
    #[tokio::test(flavor = "current_thread")]
    async fn fake_agent_two_step_uses_measured_usage_and_resumes_one_session() {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let audit = Arc::new(MemoryAudit::default());
        let root = tempfile::tempdir().unwrap();
        let runner = AcpRunner::new(audit.clone() as Arc<dyn AcpAudit>);
        let local = LocalSet::new();
        local
            .run_until(async {
                // Turn 1: fresh session, agent-reported usage in `_meta`.
                let (permission_tx, permission_rx) = tokio::sync::mpsc::unbounded_channel();
                let agent = FakeAcpAgent::new(
                    Arc::clone(&state),
                    FakeTurn::Plain(Some((30, 20, 10))),
                    permission_tx,
                );
                let (stdin, stdout) = start_fake_turn(agent, permission_rx);
                let turn = runner
                    .drive(
                        stdin,
                        stdout,
                        fake_request(
                            root.path(),
                            fake_policy(root.path()),
                            None,
                            "Task #9 first brief",
                        ),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                assert_eq!(turn.session_id, "fake-session");
                assert_eq!(
                    turn.usage,
                    Some(AcpUsage {
                        total_tokens: 30,
                        input_tokens: 20,
                        output_tokens: 10,
                    })
                );

                // Turn 2: the controller follow-up resumes the persisted
                // session; the agent reports no usage, so `measured` stays false.
                let (permission_tx, permission_rx) = tokio::sync::mpsc::unbounded_channel();
                let agent =
                    FakeAcpAgent::new(Arc::clone(&state), FakeTurn::Plain(None), permission_tx);
                let (stdin, stdout) = start_fake_turn(agent, permission_rx);
                let resumed = runner
                    .drive(
                        stdin,
                        stdout,
                        fake_request(
                            root.path(),
                            fake_policy(root.path()),
                            Some("fake-session"),
                            "Task #9 follow-up",
                        ),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resumed.session_id, "fake-session");
                assert_eq!(resumed.usage, None);
            })
            .await;
        let state = state.lock().unwrap();
        assert_eq!(
            state.calls,
            [
                "initialize",
                "new",
                "prompt",
                "initialize",
                "load",
                "prompt"
            ]
        );
        assert_eq!(state.prompts, ["Task #9 first brief", "Task #9 follow-up"]);
        assert_eq!(state.loads, ["fake-session"]);
        assert_eq!(
            audit.flat(),
            [
                "new:fake-session",
                "usage:measured=true",
                "session_load:allow=true",
                "usage:measured=false",
            ]
        );
    }

    /// Wire-level policy drill: server→client permission requests are
    /// answered through the real request path — in-scope locations get the
    /// narrow `allow_once`, peer-owned locations are denied.
    #[tokio::test(flavor = "current_thread")]
    async fn fake_agent_permission_requests_follow_the_policy_on_the_wire() {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let audit = Arc::new(MemoryAudit::default());
        let root = tempfile::tempdir().unwrap();
        let peer = root.path().join("peer");
        std::fs::create_dir(&peer).unwrap();
        let policy =
            AcpPermissionPolicy::new(vec![root.path().into()], vec![peer.clone()], false).unwrap();
        let runner = AcpRunner::new(audit.clone() as Arc<dyn AcpAudit>);
        let local = LocalSet::new();
        local
            .run_until(async {
                for (path, expected) in [
                    (root.path().join("src/new.rs"), true),
                    (peer.join("x.rs"), false),
                ] {
                    let (permission_tx, permission_rx) = tokio::sync::mpsc::unbounded_channel();
                    let agent = FakeAcpAgent::new(
                        Arc::clone(&state),
                        FakeTurn::Permission {
                            path: path.display().to_string(),
                        },
                        permission_tx,
                    );
                    let (stdin, stdout) = start_fake_turn(agent, permission_rx);
                    let turn = runner
                        .drive(
                            stdin,
                            stdout,
                            fake_request(root.path(), policy.clone(), None, "Task #9 edit"),
                            CancellationToken::new(),
                        )
                        .await
                        .unwrap();
                    assert_eq!(turn.stop_reason, acp::StopReason::EndTurn);
                    assert_eq!(
                        state.lock().unwrap().permission_outcomes.last(),
                        Some(&expected)
                    );
                }
            })
            .await;
        assert_eq!(state.lock().unwrap().permission_outcomes, [true, false]);
        let events = audit.flat();
        assert!(
            events.iter().any(|event| event == "permission:allow=true"),
            "{events:?}"
        );
        assert!(
            events.iter().any(|event| event == "permission:allow=false"),
            "{events:?}"
        );
    }

    /// Kill/restart drill: a cancelled mid-prompt turn fails honestly, and
    /// the restarted controller turn loads the same persisted session
    /// (`session/load`), not a fresh one.
    #[tokio::test(flavor = "current_thread")]
    async fn fake_agent_cancelled_turn_restarts_into_the_same_session() {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let audit = Arc::new(MemoryAudit::default());
        let root = tempfile::tempdir().unwrap();
        // Owned for the spawned turn task: spawn_local futures are 'static.
        let root_path = root.path().to_path_buf();
        let runner = Arc::new(AcpRunner::new(audit.clone() as Arc<dyn AcpAudit>));
        let local = LocalSet::new();
        local
            .run_until(async {
                // Turn 1: the prompt is entered and then never answered;
                // cancellation is deterministic because the fake signals
                // `prompt started` before hanging.
                let (started_tx, mut started_rx) = tokio::sync::mpsc::unbounded_channel();
                let (permission_tx, permission_rx) = tokio::sync::mpsc::unbounded_channel();
                let agent = FakeAcpAgent::new(Arc::clone(&state), FakeTurn::Hang, permission_tx)
                    .hanging(started_tx);
                let (stdin, stdout) = start_fake_turn(agent, permission_rx);
                let cancel = CancellationToken::new();
                let turn_runner = Arc::clone(&runner);
                let turn_cancel = cancel.clone();
                let handle = tokio::task::spawn_local(async move {
                    turn_runner
                        .drive(
                            stdin,
                            stdout,
                            fake_request(
                                &root_path,
                                fake_policy(&root_path),
                                None,
                                "Task #9 kill mid-turn",
                            ),
                            turn_cancel,
                        )
                        .await
                });
                started_rx.recv().await.expect("prompt started signal");
                cancel.cancel();
                let error = handle.await.unwrap().unwrap_err();
                assert!(
                    error.to_string().contains("session/prompt cancelled"),
                    "{error}"
                );

                // Turn 2: the restarted turn loads the persisted session.
                let (permission_tx, permission_rx) = tokio::sync::mpsc::unbounded_channel();
                let agent =
                    FakeAcpAgent::new(Arc::clone(&state), FakeTurn::Plain(None), permission_tx);
                let (stdin, stdout) = start_fake_turn(agent, permission_rx);
                let restarted = runner
                    .drive(
                        stdin,
                        stdout,
                        fake_request(
                            root.path(),
                            fake_policy(root.path()),
                            Some("fake-session"),
                            "Task #9 continue",
                        ),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap();
                assert_eq!(restarted.session_id, "fake-session");
            })
            .await;
        let state = state.lock().unwrap();
        assert_eq!(
            state.calls,
            [
                "initialize",
                "new",
                "prompt",
                "initialize",
                "load",
                "prompt"
            ]
        );
        assert_eq!(state.loads, ["fake-session"]);
        assert_eq!(
            audit.flat(),
            [
                "new:fake-session",
                "cancel:allow=false",
                "session_load:allow=true",
                "usage:measured=false",
            ]
        );
    }

    /// Failure cleanup at the transport level: a failed `session/new`
    /// persists no session event, no usage receipt, and no partial state.
    #[tokio::test(flavor = "current_thread")]
    async fn fake_agent_setup_failure_persists_no_session_and_no_usage() {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let audit = Arc::new(MemoryAudit::default());
        let root = tempfile::tempdir().unwrap();
        let runner = AcpRunner::new(audit.clone() as Arc<dyn AcpAudit>);
        let local = LocalSet::new();
        local
            .run_until(async {
                let (permission_tx, permission_rx) = tokio::sync::mpsc::unbounded_channel();
                let agent =
                    FakeAcpAgent::new(Arc::clone(&state), FakeTurn::Plain(None), permission_tx)
                        .failing_new_session();
                let (stdin, stdout) = start_fake_turn(agent, permission_rx);
                let error = runner
                    .drive(
                        stdin,
                        stdout,
                        fake_request(
                            root.path(),
                            fake_policy(root.path()),
                            None,
                            "Task #9 never starts",
                        ),
                        CancellationToken::new(),
                    )
                    .await
                    .unwrap_err();
                assert!(error.to_string().contains("session/new"), "{error}");
            })
            .await;
        assert_eq!(state.lock().unwrap().calls, ["initialize"]);
        assert!(
            audit.flat().is_empty(),
            "no audit event may persist: {:?}",
            audit.flat()
        );
    }

    /// F7 nested guard (TASK_RAIL_V1 §4.3), measured on a real spawned
    /// child: the ACP agent must never inherit the Claude Code session
    /// markers, or `session/new` dies inside a nested session.
    #[tokio::test]
    async fn spawned_child_env_strips_nested_session_markers() {
        static ENV_LOCK: Mutex<()> = Mutex::new(());

        /// Sets one env var for the test's scope and removes it on drop, so
        /// the process-wide mutation never leaks into sibling tests.
        struct ScopedEnvVar(&'static str);
        impl ScopedEnvVar {
            fn set(name: &'static str, value: &str) -> Self {
                std::env::set_var(name, value);
                Self(name)
            }
        }
        impl Drop for ScopedEnvVar {
            fn drop(&mut self) {
                std::env::remove_var(self.0);
            }
        }
        // The lock serializes only the `set_var` calls; it is dropped before
        // the awaited child I/O below (never held across an await). That is
        // safe because no other test in this binary touches these markers.
        let _guards = {
            let _env = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
            [
                ScopedEnvVar::set("CLAUDECODE", "1"),
                ScopedEnvVar::set("CLAUDE_CODE", "1"),
                ScopedEnvVar::set("CLAUDE_CODE_ENTRYPOINT", "1"),
                ScopedEnvVar::set("CLAUDE_CODE_SSE_PORT", "1"),
            ]
        };

        let root = tempfile::tempdir().unwrap();
        // spawn_agent wraps Windows programs in `cmd /C`; `cmd /C set` and
        // `/usr/bin/env` both dump the child environment to stdout.
        #[cfg(windows)]
        let endpoint = AcpEndpoint {
            program: PathBuf::from("cmd"),
            args: vec!["/C".into(), "set".into()],
        };
        #[cfg(not(windows))]
        let endpoint = AcpEndpoint {
            program: PathBuf::from("/usr/bin/env"),
            args: vec![],
        };
        let mut child = spawn_agent(&endpoint, root.path()).expect("spawn env probe");
        let mut output = String::new();
        if let Some(stdout) = child.stdout.as_mut() {
            use tokio::io::AsyncReadExt as _;
            stdout
                .read_to_string(&mut output)
                .await
                .expect("read env probe");
        }
        let _ = child.wait().await;
        // Exact-name match: unrelated environment variables that merely share
        // a prefix (e.g. CLAUDE_CODE_MAX_OUTPUT_TOKENS) are not F7 markers.
        let markers = [
            "CLAUDECODE",
            "CLAUDE_CODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "CLAUDE_CODE_SSE_PORT",
        ];
        let leaked: Vec<&str> = output
            .lines()
            .filter_map(|line| line.split('=').next())
            .filter(|name| markers.contains(name))
            .collect();
        assert!(
            leaked.is_empty(),
            "ACP child inherited nested-session markers: {leaked:?}"
        );
    }
}
