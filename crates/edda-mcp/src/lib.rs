use std::path::{Path, PathBuf};

use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::{tool, tool_handler, tool_router, ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::Deserialize;

use edda_core::event::new_note_event;
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

        ledger.append_event(&event, false).map_err(to_mcp_err)?;

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

        let text =
            render_context(&ledger, &head, DeriveOptions { depth }).map_err(to_mcp_err)?;

        Ok(CallToolResult::success(vec![Content::text(text)]))
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
                let branch_events: Vec<_> =
                    events.iter().filter(|e| e.branch == head).collect();
                let recent: Vec<_> =
                    branch_events.iter().rev().take(50).rev().collect();
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
}
