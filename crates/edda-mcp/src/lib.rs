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
        };
        let mut event = new_decision_event(&branch, parent_hash.as_deref(), "system", &dp)
            .map_err(to_mcp_err)?;

        // Auto-supersede: find prior decision with same key (skip if idempotent)
        let prior = find_prior_decision(&ledger, &branch, key);
        let mut supersede_info = String::new();
        if let Some((prior_id, prior_value)) = &prior {
            if prior_value.as_deref() != Some(value) {
                supersede_info = format!(
                    " (supersedes {} which was \"{}\")",
                    prior_id,
                    prior_value.as_deref().unwrap_or("?")
                );
                event.refs.provenance.push(Provenance {
                    target: prior_id.clone(),
                    rel: rel::SUPERSEDES.to_string(),
                    note: Some(format!("key '{}' re-decided", key)),
                });
            }
        }

        // Re-finalize after payload/refs mutation
        finalize_event(&mut event);
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
        let q = params.query.as_deref().unwrap_or("");
        let opts = edda_ask::AskOptions {
            limit: params.limit.unwrap_or(10),
            include_superseded: params.include_superseded.unwrap_or(false),
            branch: params.branch,
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
        let events = ledger.iter_events().map_err(to_mcp_err)?;
        let limit = params.limit.unwrap_or(50);

        let results: Vec<_> = events
            .iter()
            .rev()
            .filter(|e| e.branch == head)
            .filter(|e| {
                if let Some(ref et) = params.event_type {
                    if e.event_type != *et {
                        return false;
                    }
                }
                if let Some(ref kw) = params.keyword {
                    let payload_str = e.payload.to_string().to_lowercase();
                    if !payload_str.contains(&kw.to_lowercase()) {
                        return false;
                    }
                }
                if let Some(ref after) = params.after {
                    if e.ts.as_str() < after.as_str() {
                        return false;
                    }
                }
                if let Some(ref before) = params.before {
                    if e.ts.as_str() > before.as_str() {
                        return false;
                    }
                }
                true
            })
            .take(limit)
            .collect();

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
                let events = ledger.iter_events().map_err(to_mcp_err)?;
                let branch_events: Vec<_> = events.iter().filter(|e| e.branch == head).collect();
                let recent: Vec<_> = branch_events.iter().rev().take(50).rev().collect();
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

/// Find the most recent decision event with the same key on the given branch.
fn find_prior_decision(
    ledger: &Ledger,
    branch: &str,
    key: &str,
) -> Option<(String, Option<String>)> {
    let events = ledger.iter_events().ok()?;
    events
        .iter()
        .rev()
        .filter(|e| e.branch == branch && e.event_type == "note")
        .filter(|e| edda_core::decision::is_decision(&e.payload))
        .find_map(|e| {
            let dp = edda_core::decision::extract_decision(&e.payload)?;
            if dp.key == key {
                Some((e.event_id.clone(), Some(dp.value)))
            } else {
                None
            }
        })
}

fn to_mcp_err(e: anyhow::Error) -> McpError {
    McpError::internal_error(e.to_string(), None)
}

/// Start the MCP server on stdio transport.
pub async fn serve(repo_root: &Path) -> anyhow::Result<()> {
    let paths = edda_ledger::paths::EddaPaths::discover(repo_root);
    if !paths.is_initialized() {
        anyhow::bail!("not a edda workspace (run `edda init` first)");
    }

    let server = EddaServer::new(repo_root.to_path_buf());
    let service = server.serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_workspace() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        let paths = edda_ledger::paths::EddaPaths::discover(&root);
        paths.ensure_layout().unwrap();
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        (tmp, root)
    }

    #[test]
    fn server_info_has_tools_and_resources() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);
        let info = server.get_info();
        assert!(info.capabilities.tools.is_some());
        assert!(info.capabilities.resources.is_some());
    }

    #[test]
    fn open_ledger_works_for_valid_workspace() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);
        assert!(server.open_ledger().is_ok());
    }

    #[test]
    fn open_ledger_fails_for_invalid_path() {
        let server = EddaServer::new(PathBuf::from("/nonexistent/path"));
        assert!(server.open_ledger().is_err());
    }

    // --- edda_decide tests ---

    #[tokio::test]
    async fn test_decide_basic() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root.clone());

        let result = server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=postgres".to_string(),
                reason: Some("JSONB support".to_string()),
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("Decision recorded: db.engine = postgres"));
        assert!(text.contains("evt_"));

        // Verify event in ledger
        let ledger = Ledger::open(&root).unwrap();
        let events = ledger.iter_events().unwrap();
        let dec = events.iter().find(|e| {
            e.payload
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().any(|t| t.as_str() == Some("decision")))
                .unwrap_or(false)
        });
        assert!(dec.is_some());
        let dec = dec.unwrap();
        assert_eq!(
            dec.payload["decision"]["key"].as_str().unwrap(),
            "db.engine"
        );
        assert_eq!(
            dec.payload["decision"]["value"].as_str().unwrap(),
            "postgres"
        );
        assert_eq!(
            dec.payload["decision"]["reason"].as_str().unwrap(),
            "JSONB support"
        );
    }

    #[tokio::test]
    async fn test_decide_auto_supersede() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root.clone());

        // First decision
        server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=sqlite".to_string(),
                reason: None,
            }))
            .await
            .unwrap();

        // Second decision with same key, different value
        let result = server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=postgres".to_string(),
                reason: Some("need JSONB".to_string()),
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("supersedes"));

        // Verify provenance link in ledger
        let ledger = Ledger::open(&root).unwrap();
        let events = ledger.iter_events().unwrap();
        let last_dec = events
            .iter()
            .rev()
            .find(|e| {
                e.payload
                    .get("decision")
                    .and_then(|d| d.get("value"))
                    .and_then(|v| v.as_str())
                    == Some("postgres")
            })
            .unwrap();
        assert_eq!(last_dec.refs.provenance.len(), 1);
        assert_eq!(last_dec.refs.provenance[0].rel, "supersedes");
    }

    #[tokio::test]
    async fn test_decide_idempotent_no_supersede() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root.clone());

        // Same key, same value twice — should NOT create supersede link
        server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=postgres".to_string(),
                reason: None,
            }))
            .await
            .unwrap();

        let result = server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=postgres".to_string(),
                reason: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(!text.contains("supersedes"));

        // Verify no provenance link on second event
        let ledger = Ledger::open(&root).unwrap();
        let events = ledger.iter_events().unwrap();
        let last = events.last().unwrap();
        assert!(last.refs.provenance.is_empty());
    }

    #[tokio::test]
    async fn test_decide_invalid_format() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        let result = server
            .edda_decide(Parameters(DecideParams {
                decision: "no-equals-sign".to_string(),
                reason: None,
            }))
            .await;

        assert!(result.is_err());
    }

    // --- edda_ask tests ---

    #[tokio::test]
    async fn test_ask_finds_decisions() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=postgres".to_string(),
                reason: Some("JSONB support".to_string()),
            }))
            .await
            .unwrap();
        server
            .edda_decide(Parameters(DecideParams {
                decision: "auth.method=JWT".to_string(),
                reason: None,
            }))
            .await
            .unwrap();

        let result = server
            .edda_ask(Parameters(AskParams {
                query: Some("postgres".to_string()),
                limit: None,
                include_superseded: None,
                branch: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["input_type"], "keyword");
        assert_eq!(parsed["decisions"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["decisions"][0]["key"], "db.engine");
    }

    #[tokio::test]
    async fn test_ask_empty_returns_all_active() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=postgres".to_string(),
                reason: None,
            }))
            .await
            .unwrap();
        server
            .edda_decide(Parameters(DecideParams {
                decision: "auth.method=JWT".to_string(),
                reason: None,
            }))
            .await
            .unwrap();

        let result = server
            .edda_ask(Parameters(AskParams {
                query: None,
                limit: None,
                include_superseded: None,
                branch: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["input_type"], "overview");
        assert_eq!(parsed["decisions"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_ask_domain_browse() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        server
            .edda_decide(Parameters(DecideParams {
                decision: "db.engine=postgres".to_string(),
                reason: None,
            }))
            .await
            .unwrap();
        server
            .edda_decide(Parameters(DecideParams {
                decision: "db.pool=10".to_string(),
                reason: None,
            }))
            .await
            .unwrap();
        server
            .edda_decide(Parameters(DecideParams {
                decision: "auth.method=JWT".to_string(),
                reason: None,
            }))
            .await
            .unwrap();

        let result = server
            .edda_ask(Parameters(AskParams {
                query: Some("db".to_string()),
                limit: None,
                include_superseded: None,
                branch: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["input_type"], "domain");
        assert_eq!(parsed["decisions"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_ask_no_results() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        let result = server
            .edda_ask(Parameters(AskParams {
                query: Some("nonexistent".to_string()),
                limit: None,
                include_superseded: None,
                branch: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        let parsed: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(parsed["decisions"].as_array().unwrap().is_empty());
    }

    // --- edda_log tests ---

    #[tokio::test]
    async fn test_log_filter_by_type() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        // Add a note
        server
            .edda_note(Parameters(NoteParams {
                text: "test note".to_string(),
                role: None,
                tags: None,
            }))
            .await
            .unwrap();

        // Filter by note type — should find the event
        let result = server
            .edda_log(Parameters(LogParams {
                event_type: Some("note".to_string()),
                keyword: None,
                after: None,
                before: None,
                limit: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("note"));
        assert!(text.contains("test note"));

        // Filter by non-existent type — should return nothing
        let result = server
            .edda_log(Parameters(LogParams {
                event_type: Some("commit".to_string()),
                keyword: None,
                after: None,
                before: None,
                limit: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("No events match"));
    }

    #[tokio::test]
    async fn test_log_filter_by_keyword() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        server
            .edda_note(Parameters(NoteParams {
                text: "authentication flow".to_string(),
                role: None,
                tags: None,
            }))
            .await
            .unwrap();

        server
            .edda_note(Parameters(NoteParams {
                text: "database schema".to_string(),
                role: None,
                tags: None,
            }))
            .await
            .unwrap();

        let result = server
            .edda_log(Parameters(LogParams {
                event_type: None,
                keyword: Some("auth".to_string()),
                after: None,
                before: None,
                limit: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("authentication"));
        assert!(!text.contains("database"));
    }

    #[tokio::test]
    async fn test_log_date_filter() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        server
            .edda_note(Parameters(NoteParams {
                text: "some note".to_string(),
                role: None,
                tags: None,
            }))
            .await
            .unwrap();

        // Filter with future date should show nothing
        let result = server
            .edda_log(Parameters(LogParams {
                event_type: None,
                keyword: None,
                after: Some("2099-01-01".to_string()),
                before: None,
                limit: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("No events match"));

        // Filter with past date should show the event
        let result = server
            .edda_log(Parameters(LogParams {
                event_type: None,
                keyword: None,
                after: Some("2020-01-01".to_string()),
                before: None,
                limit: None,
            }))
            .await
            .unwrap();

        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("some note"));
    }

    // --- edda_draft_inbox tests ---

    #[tokio::test]
    async fn test_draft_inbox_empty() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root);

        let result = server.edda_draft_inbox().await.unwrap();
        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert_eq!(text, "No pending items.");
    }

    #[tokio::test]
    async fn test_draft_inbox_with_pending() {
        let (_tmp, root) = setup_workspace();
        let server = EddaServer::new(root.clone());

        // Create a mock draft file
        let drafts_dir = root.join(".edda").join("drafts");
        let draft_json = serde_json::json!({
            "version": 1,
            "draft_id": "drf_test123",
            "title": "Add auth module",
            "status": "proposed",
            "stages": [
                {
                    "stage_id": "lead",
                    "role": "lead",
                    "min_approvals": 1,
                    "approved_by": [],
                    "status": "pending"
                }
            ]
        });
        std::fs::write(
            drafts_dir.join("drf_test123.json"),
            serde_json::to_string_pretty(&draft_json).unwrap(),
        )
        .unwrap();

        let result = server.edda_draft_inbox().await.unwrap();
        let text = result.content[0].raw.as_text().unwrap().text.as_str();
        assert!(text.contains("drf_test123"));
        assert!(text.contains("Add auth module"));
        assert!(text.contains("stage: lead"));
        assert!(text.contains("approvals: 0/1"));
    }
}
