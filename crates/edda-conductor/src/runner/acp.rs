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
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::task::LocalSet;
use tokio::time::timeout;
use tokio_util::compat::{TokioAsyncReadCompatExt as _, TokioAsyncWriteCompatExt as _};
use tokio_util::sync::CancellationToken;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
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
        let audit = Arc::clone(&self.audit);
        let local = LocalSet::new();
        let result = local
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

                connection
                    .initialize(
                        acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                            .client_capabilities(acp::ClientCapabilities::default())
                            .client_info(acp::Implementation::new(
                                "edda",
                                env!("CARGO_PKG_VERSION"),
                            )),
                    )
                    .await
                    .context("ACP initialize")?;

                let servers = vec![acp::McpServer::Stdio(
                    acp::McpServerStdio::new("edda-mcp", request.mcp_server.program)
                        .args(request.mcp_server.args),
                )];
                let session_id = if let Some(session_id) = request.resume_session_id {
                    audit.decision(request.task_id, "session_load", true)?;
                    connection
                        .load_session(
                            acp::LoadSessionRequest::new(session_id.clone(), worktree.clone())
                                .mcp_servers(servers),
                        )
                        .await
                        .context("ACP session/load")?;
                    session_id
                } else {
                    let created = connection
                        .new_session(
                            acp::NewSessionRequest::new(worktree.clone()).mcp_servers(servers),
                        )
                        .await
                        .context("ACP session/new")?;
                    let session_id = created.session_id.0.to_string();
                    audit.session_created(request.task_id, &session_id)?;
                    session_id
                };
                let prompt =
                    acp::PromptRequest::new(session_id.clone(), vec![request.prompt.into()]);
                let response = tokio::select! {
                    result = timeout(REQUEST_TIMEOUT, connection.prompt(prompt)) => {
                        result.context("ACP prompt timeout")?.context("ACP session/prompt")?
                    }
                    _ = cancel.cancelled() => {
                        audit.decision(request.task_id, "cancel", false)?;
                        connection.cancel(acp::CancelNotification::new(session_id.clone())).await?;
                        anyhow::bail!("ACP turn cancelled")
                    }
                };
                Ok(AcpTurnResult {
                    session_id,
                    stop_reason: response.stop_reason,
                    usage: effective_usage(&response),
                })
            })
            .await;
        drain_child(&mut child).await;
        result
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
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryAudit {
        events: Mutex<Vec<&'static str>>,
    }

    impl AcpAudit for MemoryAudit {
        fn session_created(&self, _task_id: u64, _session_id: &str) -> Result<()> {
            self.events.lock().unwrap().push("new");
            Ok(())
        }

        fn decision(&self, _task_id: u64, kind: &'static str, _allowed: bool) -> Result<()> {
            self.events.lock().unwrap().push(kind);
            Ok(())
        }

        fn update(&self, _task_id: u64, kind: &'static str) -> Result<()> {
            self.events.lock().unwrap().push(kind);
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeAgent {
        calls: Arc<Mutex<Vec<&'static str>>>,
    }

    #[async_trait::async_trait(?Send)]
    impl acp::Agent for FakeAgent {
        async fn initialize(
            &self,
            _request: acp::InitializeRequest,
        ) -> acp::Result<acp::InitializeResponse> {
            self.calls.lock().unwrap().push("initialize");
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
            self.calls.lock().unwrap().push("new");
            Ok(acp::NewSessionResponse::new("fake-session"))
        }

        async fn load_session(
            &self,
            _request: acp::LoadSessionRequest,
        ) -> acp::Result<acp::LoadSessionResponse> {
            self.calls.lock().unwrap().push("load");
            Ok(acp::LoadSessionResponse::new())
        }

        async fn set_session_mode(
            &self,
            _request: acp::SetSessionModeRequest,
        ) -> acp::Result<acp::SetSessionModeResponse> {
            Ok(acp::SetSessionModeResponse::default())
        }

        async fn prompt(&self, _request: acp::PromptRequest) -> acp::Result<acp::PromptResponse> {
            self.calls.lock().unwrap().push("prompt");
            Ok(acp::PromptResponse::new(acp::StopReason::EndTurn))
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

    #[tokio::test(flavor = "current_thread")]
    async fn fake_agent_exercises_initialize_new_prompt_and_load() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let agent = FakeAgent {
            calls: Arc::clone(&calls),
        };
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
            *calls.lock().unwrap(),
            ["initialize", "new", "prompt", "load"]
        );
    }
}
