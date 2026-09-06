use std::path::{Path, PathBuf};

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{
    tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;

use edda_core::event::{finalize_event, new_decision_event, new_note_event};
use edda_core::types::{rel, DecisionPayload, Provenance};
use edda_derive::{rebuild_branch, render_context, DeriveOptions};
use edda_ledger::lock::WorkspaceLock;
use edda_ledger::Ledger;

mod client_ops;
#[cfg(test)]
mod tests;

// --- Client-contract tool parameter structs (GH-611) ---

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskNewParams {
    /// Task title
    title: String,
    /// Agent label this task is assigned to (e.g. "tester")
    assignee: Option<String>,
    /// Agent transport kind (e.g. "claude-acp")
    agent_kind: Option<String>,
    /// Task ids that must be done first
    after: Option<Vec<u64>>,
    /// Paths this task may write
    scope_paths: Option<Vec<String>>,
    /// Plan this task belongs to
    plan: Option<String>,
    /// Work unit this task delivers
    work_unit: Option<String>,
    /// Brief reference (path or free text)
    brief: Option<String>,
    /// Idempotency key — the same key never creates a twin task
    idempotency_key: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskStartParams {
    /// Task id
    id: u64,
    /// Lease TTL in seconds (default 3600)
    lease_ttl_s: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskDoneParams {
    /// Task id
    id: u64,
    /// What was done, verifiable. Required: no receipt, no done.
    receipt: String,
    /// Evidence paths
    evidence_paths: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct TaskFailParams {
    /// Task id
    id: u64,
    /// Why the task failed
    reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ReceiptParams {
    /// Task id to read the recorded receipt for
    task_id: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ClaimParams {
    /// Scope label (e.g. "auth")
    label: String,
    /// Paths this session may write (globs)
    paths: Option<Vec<String>>,
    /// Optional subject this scope is for
    subject: Option<String>,
    /// Session id (default: deterministic "mcp-<label>")
    session: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct VerifyParams {}

// --- Tool parameter structs ---

#[derive(Debug, Deserialize, JsonSchema)]
struct NoteParams {
    /// Note text content
    text: String,
    /// Role: user, assistant, or system (default: assistant)
    role: Option<String>,
    /// Tags for the note (e.g. todo, decision)
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ContextParams {
    /// Number of recent commits/signals to show (default: 5)
    depth: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct DecideParams {
    /// Decision in key=value format (e.g. "db.engine=postgres")
    decision: String,
    /// Reason for the decision
    reason: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AskParams {
    /// Query string (keyword, domain, or exact key like "db.engine"). Leave empty for all active decisions.
    query: Option<String>,
    /// Semantic context summary used for similarity retrieval when query is omitted.
    context_summary: Option<String>,
    /// Maximum results per section (default: 10)
    limit: Option<usize>,
    /// Include superseded decisions (default: false)
    include_superseded: Option<bool>,
    /// Filter by branch (default: all branches)
    branch: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct LogParams {
    /// Filter by event type (e.g. "note", "cmd", "commit")
    event_type: Option<String>,
    /// Case-insensitive keyword search in event payload
    keyword: Option<String>,
    /// Only events after this date (ISO 8601 prefix, e.g. "2026-02")
    after: Option<String>,
    /// Only events before this date
    before: Option<String>,
    /// Maximum events to return (default: 50)
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ToolTierParams {
    /// Tool name to query (e.g. "bash", "Write", "rm")
    tool_name: String,
}

// --- Minimal draft structs for inbox display ---

#[derive(Debug, Deserialize)]
struct MinimalDraft {
    #[serde(default)]
    draft_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    stages: Vec<MinimalStage>,
}

#[derive(Debug, Deserialize)]
struct MinimalStage {
    #[serde(default)]
    stage_id: String,
    #[serde(default)]
    role: String,
    #[serde(default)]
    min_approvals: usize,
    #[serde(default)]
    approved_by: Vec<String>,
    #[serde(default)]
    status: String,
}

// --- MCP Server ---

/// MCP Server for edda working memory.
#[derive(Clone)]
pub struct EddaServer {
    repo_root: PathBuf,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl EddaServer {
    pub fn new(repo_root: PathBuf) -> Self {
        Self {
            repo_root,
            tool_router: Self::tool_router(),
        }
    }

    fn open_ledger(&self) -> Result<Ledger, McpError> {
        Ledger::open(&self.repo_root).map_err(to_mcp_err)
    }

    /// Show workspace status: current branch, last commit, uncommitted events
    #[tool(description = "Show workspace status: current branch, last commit, uncommitted events")]
    async fn edda_status(&self) -> Result<CallToolResult, McpError> {
        let ledger = self.open_ledger()?;
        let head = ledger.head_branch().map_err(to_mcp_err)?;
        let snap = rebuild_branch(&ledger, &head).map_err(to_mcp_err)?;

        let mut lines = vec![format!("On branch {head}")];

        if let Some(c) = &snap.last_commit {
            lines.push(format!(
                "Last commit: {} {} \"{}\"",
                c.ts, c.event_id, c.title
            ));
        } else {
            lines.push("Last commit: (none)".to_string());
        }

        lines.push(format!("Uncommitted events: {}", snap.uncommitted_events));

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    /// Record a note to the working memory ledger
    #[tool(description = "Record a note to the working memory ledger")]
    async fn edda_note(
        &self,
        Parameters(params): Parameters<NoteParams>,
    ) -> Result<CallToolResult, McpError> {
        let ledger = self.open_ledger()?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).map_err(to_mcp_err)?;

        let branch = ledger.head_branch().map_err(to_mcp_err)?;
        let parent_hash = ledger.last_event_hash().map_err(to_mcp_err)?;
        let role = params.role.unwrap_or_else(|| "assistant".to_string());
        let tags = params.tags.unwrap_or_default();

        let event = new_note_event(&branch, parent_hash.as_deref(), &role, &params.text, &tags)
            .map_err(to_mcp_err)?;

        ledger.append_event(&event).map_err(to_mcp_err)?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Wrote NOTE {}",
            event.event_id
        ))]))
    }

    /// Get full working memory context snapshot as Markdown
    #[tool(description = "Get full working memory context snapshot as Markdown")]
    async fn edda_context(
        &self,
        Parameters(params): Parameters<ContextParams>,
    ) -> Result<CallToolResult, McpError> {
        let ledger = self.open_ledger()?;
        let head = ledger.head_branch().map_err(to_mcp_err)?;
        let depth = params.depth.unwrap_or(5);

        let text = render_context(&ledger, &head, DeriveOptions { depth }).map_err(to_mcp_err)?;

        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    /// Record a binding decision (key=value) with optional reason and auto-supersede
    #[tool(
        description = "Record a binding decision (key=value) with optional reason and auto-supersede detection"
    )]
    async fn edda_decide(
        &self,
        Parameters(params): Parameters<DecideParams>,
    ) -> Result<CallToolResult, McpError> {
        let (key, value) = params.decision.split_once('=').ok_or_else(|| {
            McpError::invalid_params(
                "decision must be in key=value format (e.g. \"db.engine=postgres\")",
                None,
            )
        })?;
        let key = key.trim();
        let value = value.trim();

        let ledger = self.open_ledger()?;
        let _lock = WorkspaceLock::acquire(&ledger.paths).map_err(to_mcp_err)?;

        let branch = ledger.head_branch().map_err(to_mcp_err)?;
        let parent_hash = ledger.last_event_hash().map_err(to_mcp_err)?;

        let dp = DecisionPayload {
            key: key.to_string(),
            value: value.to_string(),
            reason: params.reason.clone(),
            scope: None,
            authority: None,
            affected_paths: None,
            tags: None,
            review_after: None,
            reversibility: None,
            village_id: None,
        };
        let mut event = new_decision_event(&branch, parent_hash.as_deref(), "system", &dp)
            .map_err(to_mcp_err)?;

        // Auto-supersede: find prior decision with same key via SQL index (skip if idempotent)
        let prior = ledger
            .find_active_decision(&branch, key)
            .map_err(to_mcp_err)?;
        let mut supersede_info = String::new();
        if let Some(ref row) = prior {
            if row.value != value {
                supersede_info =
                    format!(" (supersedes {} which was \"{}\")", row.event_id, row.value);
                event.refs.provenance.push(Provenance {
                    target: row.event_id.clone(),
                    rel: rel::SUPERSEDES.to_string(),
                    note: Some(format!("key '{}' re-decided", key)),
                });
            }
        }

        // Re-finalize after payload/refs mutation
        finalize_event(&mut event).map_err(to_mcp_err)?;
        ledger.append_event(&event).map_err(to_mcp_err)?;

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Decision recorded: {key} = {value} [{}]{supersede_info}",
            event.event_id
        ))]))
    }

    /// Query project decisions, history, and conversations
    #[tool(
        description = "Query project decisions, history, and conversations. Returns a structured context bundle with decisions, timeline, related commits, notes, and transcript excerpts."
    )]
    async fn edda_ask(
        &self,
        Parameters(params): Parameters<AskParams>,
    ) -> Result<CallToolResult, McpError> {
        let ledger = self.open_ledger()?;
        let q = params
            .query
            .as_deref()
            .or(params.context_summary.as_deref())
            .unwrap_or("");
        let opts = edda_ask::AskOptions {
            limit: params.limit.unwrap_or(10),
            include_superseded: params.include_superseded.unwrap_or(false),
            branch: params.branch,
            impact: false,
            after: None,
            before: None,
            tags: vec![],
            village_id: None,
        };

        let result = edda_ask::ask(&ledger, q, &opts, None).map_err(to_mcp_err)?;
        let json = serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?;

        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    /// Query the event log with optional filters (type, keyword, date range)
    #[tool(description = "Query the event log with optional filters (type, keyword, date range)")]
    async fn edda_log(
        &self,
        Parameters(params): Parameters<LogParams>,
    ) -> Result<CallToolResult, McpError> {
        let ledger = self.open_ledger()?;
        let head = ledger.head_branch().map_err(to_mcp_err)?;
        let limit = params.limit.unwrap_or(50);

        let results = ledger
            .iter_events_filtered(
                &head,
                params.event_type.as_deref(),
                params.keyword.as_deref(),
                params.after.as_deref(),
                params.before.as_deref(),
                limit,
            )
            .map_err(to_mcp_err)?;

        if results.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No events match the given filters.",
            )]));
        }

        let lines: Vec<String> = results
            .iter()
            .map(|e| {
                let ts_short = e.ts.get(..19).unwrap_or(&e.ts);
                let id_short = e.event_id.get(..12).unwrap_or(&e.event_id);
                let detail = e
                    .payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .or_else(|| e.payload.get("title").and_then(|v| v.as_str()))
                    .unwrap_or("");
                format!(
                    "[{ts_short}] {} {} {id_short} {detail}",
                    e.event_type, e.branch
                )
            })
            .collect();

        Ok(CallToolResult::success(vec![Content::text(
            lines.join("\n"),
        )]))
    }

    /// List pending draft approval items (read-only governance inbox)
    #[tool(description = "List pending draft approval items (read-only governance inbox)")]
    async fn edda_draft_inbox(&self) -> Result<CallToolResult, McpError> {
        let ledger = self.open_ledger()?;
        let drafts_dir = &ledger.paths.drafts_dir;

        if !drafts_dir.exists() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No pending items.",
            )]));
        }

        let entries = std::fs::read_dir(drafts_dir).map_err(|e| to_mcp_err(e.into()))?;
        let mut items = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| to_mcp_err(e.into()))?;
            let path = entry.path();

            // Skip non-JSON and latest.json
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if path.file_stem().and_then(|s| s.to_str()) == Some("latest") {
                continue;
            }

            let content = std::fs::read_to_string(&path).map_err(|e| to_mcp_err(e.into()))?;
            let draft: MinimalDraft = match serde_json::from_str(&content) {
                Ok(d) => d,
                Err(_) => continue, // skip malformed files
            };

            if draft.status == "applied" {
                continue;
            }

            for stage in &draft.stages {
                if stage.status != "pending" {
                    continue;
                }
                let current = stage.approved_by.len();
                items.push(format!(
                    "{} | {} | stage: {} ({}) | approvals: {}/{}",
                    draft.draft_id,
                    draft.title,
                    stage.stage_id,
                    stage.role,
                    current,
                    stage.min_approvals,
                ));
            }
        }

        if items.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No pending items.",
            )]));
        }

        Ok(CallToolResult::success(vec![Content::text(
            items.join("\n"),
        )]))
    }

    /// Query a tool's risk tier (T0-T4) and approval requirement
    #[tool(description = "Query a tool's risk tier (T0-T4) and approval requirement")]
    async fn edda_tool_tier(
        &self,
        Parameters(params): Parameters<ToolTierParams>,
    ) -> Result<CallToolResult, McpError> {
        let edda_dir = self.repo_root.join(".edda");
        let config =
            edda_core::tool_tier::load_tool_tiers_from_dir(&edda_dir).map_err(to_mcp_err)?;
        let result = edda_core::tool_tier::resolve_tool_tier(&config, &params.tool_name);
        let json = serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?;
        Ok(CallToolResult::success(vec![Content::text(json)]))
    }

    // --- Client-contract operations (GH-611); shared state machine lives in
    // `edda_ledger::task_actions` / `client_ops`, not here. ---

    /// Create a task on the rail (idempotent by key)
    #[tool(
        description = "Create a task on the task rail. State transitions are hash-chained task events; the same idempotency key never creates a twin task."
    )]
    async fn edda_task_new(
        &self,
        Parameters(params): Parameters<TaskNewParams>,
    ) -> Result<CallToolResult, McpError> {
        let args = edda_ledger::task_actions::NewTaskArgs {
            title: &params.title,
            assignee: params.assignee.as_deref(),
            agent_kind: params.agent_kind.as_deref(),
            after: params.after.as_deref().unwrap_or(&[]),
            plan: params.plan.as_deref(),
            work_unit: params.work_unit.as_deref(),
            brief: params.brief.as_deref(),
            idempotency_key: params.idempotency_key.as_deref(),
            scope_paths: params.scope_paths.as_deref().unwrap_or(&[]),
        };
        let result = client_ops::task_new(&self.repo_root, &args).map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?,
        )]))
    }

    /// Take the lease on a task and mark it running
    #[tool(
        description = "Start a task: take the lease and mark it running (attempt incremented; blocked/done/running tasks refuse)."
    )]
    async fn edda_task_start(
        &self,
        Parameters(params): Parameters<TaskStartParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = client_ops::task_start(
            &self.repo_root,
            params.id,
            params.lease_ttl_s.unwrap_or(3600),
        )
        .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?,
        )]))
    }

    /// Complete a task with a receipt (no receipt, no done)
    #[tool(
        description = "Complete a task — one action: done + receipt. Requires a non-empty receipt; successors become ready."
    )]
    async fn edda_task_done(
        &self,
        Parameters(params): Parameters<TaskDoneParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = client_ops::task_done(
            &self.repo_root,
            params.id,
            &params.receipt,
            params.evidence_paths.as_deref().unwrap_or(&[]),
        )
        .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?,
        )]))
    }

    /// Mark a running task failed
    #[tool(
        description = "Mark a running task failed with a reason; the task can be started again to retry."
    )]
    async fn edda_task_fail(
        &self,
        Parameters(params): Parameters<TaskFailParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = client_ops::task_fail(&self.repo_root, params.id, &params.reason)
            .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?,
        )]))
    }

    /// Read the recorded receipt of a task
    #[tool(description = "Read the receipt recorded by `task done` for a task id.")]
    async fn edda_receipt(
        &self,
        Parameters(params): Parameters<ReceiptParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = client_ops::receipt(&self.repo_root, params.task_id).map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?,
        )]))
    }

    /// Claim a coordination scope on the session board
    #[tool(
        description = "Claim a coordination scope (label + path globs) for this session. One claim per session: a second claim replaces the first."
    )]
    async fn edda_claim(
        &self,
        Parameters(params): Parameters<ClaimParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = client_ops::claim(
            &self.repo_root,
            &params.label,
            params.paths.as_deref().unwrap_or(&[]),
            params.subject.as_deref(),
            params.session.as_deref(),
        )
        .map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?,
        )]))
    }

    /// Verify the ledger hash chain (read-only)
    #[tool(
        description = "Verify the ledger hash chain read-only; reports ok/events/first_bad_event (same payload as `edda verify --json`)."
    )]
    async fn edda_verify(
        &self,
        Parameters(_params): Parameters<VerifyParams>,
    ) -> Result<CallToolResult, McpError> {
        let result = client_ops::verify(&self.repo_root).map_err(to_mcp_err)?;
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).map_err(|e| to_mcp_err(e.into()))?,
        )]))
    }
}

#[tool_handler]
impl ServerHandler for EddaServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "edda working memory server — record decisions, track context, manage AI agent memory"
                    .into(),
            ),
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
            ..Default::default()
        }
    }

    async fn list_resources(
        &self,
        _req: Option<PaginatedRequestParams>,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, McpError> {
        let mut ctx_resource = RawResource::new("edda://context", "Working Memory Context");
        ctx_resource.description = Some("Current branch context snapshot as Markdown".into());
        ctx_resource.mime_type = Some("text/markdown".into());

        let mut log_resource = RawResource::new("edda://log", "Event Log");
        log_resource.description = Some("Recent events in the current branch".into());
        log_resource.mime_type = Some("text/plain".into());

        Ok(ListResourcesResult {
            resources: vec![ctx_resource.no_annotation(), log_resource.no_annotation()],
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        req: ReadResourceRequestParams,
        _ctx: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, McpError> {
        let ledger = self.open_ledger()?;
        let head = ledger.head_branch().map_err(to_mcp_err)?;

        match req.uri.as_str() {
            "edda://context" => {
                let text = render_context(&ledger, &head, DeriveOptions { depth: 5 })
                    .map_err(to_mcp_err)?;
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(text, &req.uri)],
                })
            }
            "edda://log" => {
                // SQL-filtered: get last 50 events on this branch (newest first), then reverse for display
                let mut recent = ledger
                    .iter_events_filtered(&head, None, None, None, None, 50)
                    .map_err(to_mcp_err)?;
                recent.reverse(); // display in chronological order
                let lines: Vec<String> = recent
                    .iter()
                    .map(|e| {
                        format!(
                            "{} [{}] {} {}",
                            e.ts,
                            e.event_type,
                            e.event_id,
                            e.payload
                                .get("text")
                                .and_then(|v| v.as_str())
                                .or_else(|| e.payload.get("title").and_then(|v| v.as_str()))
                                .unwrap_or("")
                        )
                    })
                    .collect();
                Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(lines.join("\n"), &req.uri)],
                })
            }
            _ => Err(McpError::resource_not_found(
                format!("Unknown resource: {}", req.uri),
                None,
            )),
        }
    }
}

fn to_mcp_err(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

/// Start the MCP server on stdio transport.
pub async fn serve(repo_root: &Path) -> anyhow::Result<()> {
    let paths = edda_ledger::paths::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("not an edda workspace (run `edda init` first)");
    }

    let server = EddaServer::new(repo_root.to_path_buf());
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
