use super::timing::ProcessTiming;
use crate::agent::codex_app_server::{CodexAppServer, CodexTurnOutcome};
use crate::agent::launcher::{AgentLauncher, PhaseResult};
use crate::plan::schema::Phase;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

/// Prefix identifying a required Codex mapping-store failure. Product review
/// uses it to refuse persistence of any verdict from an unresumable round.
pub const REQUIRED_PERSISTENCE_ERROR_PREFIX: &str = "required Codex thread persistence failed";

/// Resolve the codex executable from an explicit `EDDA_CODEX_BIN` value,
/// falling back to the name npm installs on this platform.
///
/// Takes the override as an argument rather than reading the environment so
/// the resolution is testable without mutating process-wide state.
///
/// Windows default is `codex.cmd` (GH-527 / GH-528): `where.exe codex` on a
/// standard npm install finds the extensionless `codex` sh launcher and
/// `codex.cmd`, and no `codex.exe`. `CreateProcess` — unlike a shell — does
/// not apply `PATHEXT`, so neither the bare name nor the extensionless script
/// ever resolves and every phase would fail at spawn.
fn resolve_codex_bin(explicit: Option<OsString>) -> PathBuf {
    match explicit {
        // An empty `EDDA_CODEX_BIN=` is a set-but-unusable value; treat it as
        // unset rather than spawning an empty path.
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ if cfg!(windows) => PathBuf::from("codex.cmd"),
        _ => PathBuf::from("codex"),
    }
}

fn default_codex_bin() -> PathBuf {
    resolve_codex_bin(std::env::var_os("EDDA_CODEX_BIN"))
}

/// Launches the codex coding agent through `codex app-server`.
///
/// The app-server protocol is JSON-RPC over stdin/stdout: one request per
/// line in, responses and notifications one per line out. One conductor
/// phase maps to one codex turn (`turn/start` streamed until
/// `turn/completed`). The protocol layer itself lives in
/// [`CodexAppServer`] and is reused unchanged.
///
/// Session continuity: the app-server process is spawned once and reused,
/// and the `threads` map keys on the conductor session id, resuming a
/// thread via `thread/resume` whenever a caller reuses a session id.
///
/// Dispatch and product review persist the session map under the per-user
/// project state (GH-535); conduct stays non-persistent. Dispatch falls back
/// from missing/rejected bindings with best-effort storage. Every product
/// review requires storage success; strict resume also requires an existing
/// binding. Strict rejection durably removes stale bindings without starting
/// under the old reviewer UUID.
/// Within one process the in-memory map stays the hot path and behavior is
/// unchanged. When the child dies (crash, timeout, cancellation) the
/// client is dropped and the next phase re-spawns it; the thread map
/// survives because codex persists threads and the store records their ids.
pub struct CodexLauncher {
    pub codex_bin: PathBuf,
    pub verbose: bool,
    thread_store: Option<ThreadStore>,
    require_persistence: bool,
    require_thread: bool,
    state: Mutex<LauncherState>,
    timing: Arc<ProcessTiming>,
}

/// File-backed cold-start store for the session→thread map.
///
/// Ordinary dispatch treats this as a resume convenience and swallows store
/// failures. Every product-review round requires a readable store and a
/// successful final update; strict resume additionally requires an existing
/// binding and durably removes a rejected one.
struct ThreadStore {
    /// Per-user edda store root (`edda_store::store_root()`).
    root: PathBuf,
    #[cfg(test)]
    failure: StoreFailure,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum StoreFailure {
    #[default]
    None,
    Load,
    Persist,
}

impl ThreadStore {
    fn from_default_root() -> Self {
        Self::from_root(edda_store::store_root())
    }

    fn from_root(root: PathBuf) -> Self {
        Self {
            root,
            #[cfg(test)]
            failure: StoreFailure::None,
        }
    }

    /// One map per project, so two dispatch calls from the same repo (any
    /// worktree — `project_id` resolves to the main root) share threads.
    fn map_path(&self, cwd: &Path) -> PathBuf {
        self.root
            .join("projects")
            .join(edda_store::project_id(cwd))
            .join("state")
            .join("codex-threads.json")
    }

    fn load(&self, cwd: &Path) -> Result<HashMap<String, String>> {
        #[cfg(test)]
        if self.failure == StoreFailure::Load {
            anyhow::bail!("injected codex thread map read failure");
        }
        let path = self.map_path(cwd);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(HashMap::new());
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("read codex thread map {}", path.display()));
            }
        };
        serde_json::from_str(&raw)
            .with_context(|| format!("parse codex thread map {}", path.display()))
    }

    fn load_best_effort(&self, cwd: &Path) -> HashMap<String, String> {
        match self.load(cwd) {
            Ok(map) => map,
            Err(error) => {
                eprintln!(
                    "Warning: {error:#}; ignoring it and starting fresh conversations \"
                     (resume needs a new dispatch)."
                );
                HashMap::new()
            }
        }
    }

    /// Merge `threads` into the persisted map under an exclusive file lock,
    /// so two concurrent dispatch processes cannot lose each other's
    /// entries. In-memory entries win on key conflicts, and every session
    /// in `removals` is deleted from the merged map before the overlay: a
    /// binding the server rejected must not survive a failed fallback just
    /// because it is absent from the in-memory overlay.
    fn persist(
        &self,
        cwd: &Path,
        threads: &HashMap<String, String>,
        removals: &HashSet<String>,
        require_readable: bool,
    ) -> Result<()> {
        if threads.is_empty() && removals.is_empty() {
            return Ok(());
        }
        #[cfg(test)]
        if self.failure == StoreFailure::Persist {
            anyhow::bail!("injected codex thread map persist failure");
        }
        let path = self.map_path(cwd);
        let _lock = edda_store::lock_file(&path.with_extension("lock"))
            .with_context(|| format!("lock codex thread map {}", path.display()))?;
        let mut merged = if require_readable {
            self.load(cwd)?
        } else {
            // Preserve dispatch's existing best-effort repair: after warning
            // on preflight, a corrupt map may be replaced by the fresh map.
            self.load(cwd).unwrap_or_default()
        };
        // Honor explicit deletions before overlaying in-memory entries, so
        // a rejected binding is erased from disk even when no fresh binding
        // replaces it. In-memory entries still win for anything present in
        // both (a fresh binding supersedes its own tombstone).
        for session in removals {
            merged.remove(session);
        }
        for (session, thread) in threads {
            merged.insert(session.clone(), thread.clone());
        }
        let bytes = serde_json::to_vec(&merged).context("serialize codex thread map")?;
        edda_store::write_atomic(&path, &bytes)
            .with_context(|| format!("persist codex thread map {}", path.display()))?;
        Ok(())
    }
}

#[derive(Default)]
struct LauncherState {
    server: Option<CodexAppServer>,
    /// conductor session_id → codex thread_id
    threads: HashMap<String, String>,
    /// Sessions whose persisted binding the server rejected: explicit
    /// deletions that [`ThreadStore::persist`] applies to the on-disk map.
    /// A tombstone stays until a fresh binding for the session overrules
    /// it in the merge, so it cannot be resurrected by a reload.
    removals: HashSet<String>,
}

impl Default for CodexLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl CodexLauncher {
    pub fn new() -> Self {
        Self {
            codex_bin: default_codex_bin(),
            verbose: false,
            thread_store: None,
            require_persistence: false,
            require_thread: false,
            timing: Arc::default(),
            state: Mutex::new(LauncherState::default()),
        }
    }

    pub fn with_bin(codex_bin: PathBuf) -> Self {
        Self {
            codex_bin,
            verbose: false,
            thread_store: None,
            require_persistence: false,
            require_thread: false,
            timing: Arc::default(),
            state: Mutex::new(LauncherState::default()),
        }
    }

    /// Enable persistence in the per-user edda store
    /// (`edda_store::store_root()`). Dispatch-scoped (GH-535): only `edda
    /// dispatch` opts in, because its `--session-id` is caller-chosen across
    /// invocations and resuming is the point. Conduct keeps the default
    /// non-persistent launcher — deterministic conduct session ids must
    /// never load an old binding, and conduct turns must not gain store
    /// reads/writes.
    pub fn with_persistent_threads(mut self) -> Self {
        self.thread_store = Some(ThreadStore::from_default_root());
        self
    }

    /// Point the session→thread persistence at an explicit store root
    /// (tests use this to stay out of the real per-user store; dispatch
    /// uses [`CodexLauncher::with_persistent_threads`] instead).
    pub fn with_thread_store(mut self, root: PathBuf) -> Self {
        self.thread_store = Some(ThreadStore::from_root(root));
        self
    }

    /// Require readable storage before launch and a successful final mapping
    /// update. Product review enables this for both first and resumed rounds;
    /// ordinary dispatch keeps persistence best-effort.
    pub fn with_required_persistence(mut self) -> Self {
        self.require_persistence = true;
        self
    }

    /// Require the caller's session id to resolve to an existing persisted
    /// thread. Product review uses this for `--resume`; launcher construction
    /// must separately opt into storage and required persistence.
    pub fn with_required_thread(mut self) -> Self {
        self.require_thread = true;
        self
    }

    #[cfg(test)]
    fn with_store_failure(mut self, failure: StoreFailure) -> Self {
        self.thread_store
            .as_mut()
            .expect("test failure injection needs a thread store")
            .failure = failure;
        self
    }

    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    /// Check that the codex CLI binary is reachable.
    pub fn verify_available(&self) -> Result<()> {
        let status = std::process::Command::new(&self.codex_bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            _ => anyhow::bail!(
                "codex CLI not found (looked for {:?}).\n\
                 Install: npm install -g @openai/codex\n\
                 Or set EDDA_CODEX_BIN if the executable lives elsewhere.",
                self.codex_bin
            ),
        }
    }
}

#[async_trait::async_trait]
impl AgentLauncher for CodexLauncher {
    #[allow(clippy::too_many_lines)] // 163 lines at #779; split tracked in none
    async fn run_phase(
        &self,
        phase: &Phase,
        prompt: &str,
        plan_context: &str,
        session_id: &str,
        cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<PhaseResult> {
        self.timing.reset();
        if self.require_persistence && self.thread_store.is_none() {
            anyhow::bail!(
                "required codex persistence needs thread storage; refusing an uncheckable mapping"
            );
        }
        if self.require_thread && (!self.require_persistence || self.thread_store.is_none()) {
            anyhow::bail!(
                "required codex thread mode needs required persistent thread storage; refusing an uncheckable resume"
            );
        }
        // Refuse capabilities the app-server cannot verifiably select (GH-574).
        let declared: Vec<&str> = [
            ("model", phase.model.is_some()),
            ("thinking", phase.thinking.is_some()),
            ("tools", phase.tools.is_some()),
            ("exclude_tools", phase.exclude_tools.is_some()),
        ]
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| name)
        .collect();
        if !declared.is_empty() {
            anyhow::bail!(
                "codex does not support phase-declared {} (the app-server exposes no \
                 verifiable selection path); refusing to silently ignore them — \
                 remove the declaration or dispatch with a backend that supports it",
                declared.join(", ")
            );
        }

        // The app-server has no system-prompt channel.
        let message = if plan_context.is_empty() {
            prompt.to_owned()
        } else {
            format!("{plan_context}\n\n{prompt}")
        };
        let mut state = self.state.lock().await;
        if state.server.is_none() {
            // Required product-review storage is checked before the paid
            // app-server launch. A missing map is a valid first round; a map
            // that exists but cannot be read or parsed is not.
            if let Some(store) = &self.thread_store {
                let persisted = if self.require_persistence {
                    match store.load(cwd) {
                        Ok(map) => map,
                        Err(error) => {
                            return Ok(PhaseResult::AgentCrash {
                                error: format!("{REQUIRED_PERSISTENCE_ERROR_PREFIX}: {error:#}"),
                            });
                        }
                    }
                } else {
                    store.load_best_effort(cwd)
                };
                // Merge persisted bindings without replacing the hot map.
                for (session, thread) in persisted {
                    state.threads.entry(session).or_insert(thread);
                }
            }
            if self.require_thread && !state.threads.contains_key(session_id) {
                return Ok(PhaseResult::AgentCrash {
                    error: format!(
                        "--resume requires an existing persisted Codex thread mapping for reviewer session {session_id}; refusing to start a fresh thread under the old UUID"
                    ),
                });
            }
            match CodexAppServer::spawn_timed(&self.codex_bin, Arc::clone(&self.timing)).await {
                Ok(server) => state.server = Some(server),
                Err(error) => {
                    return Ok(PhaseResult::AgentCrash {
                        error: format!(
                            "failed to spawn codex app-server ({:?}): {error}",
                            self.codex_bin
                        ),
                    });
                }
            }
        }

        if self.require_thread && !state.threads.contains_key(session_id) {
            if let Some(mut server) = state.server.take() {
                server.terminate().await;
            }
            return Ok(PhaseResult::AgentCrash {
                error: format!(
                    "--resume requires an existing persisted Codex thread mapping for reviewer session {session_id}; refusing to start a fresh thread under the old UUID"
                ),
            });
        }

        let LauncherState {
            server,
            threads,
            removals,
        } = &mut *state;
        let server = server.as_mut().expect("server spawned above");
        // Non-persistent conduct never degrades a rejected in-memory resume.
        let persist_enabled = self.thread_store.is_some();
        let outcome = drive_turn(
            server,
            threads,
            removals,
            persist_enabled,
            phase,
            &message,
            session_id,
            cwd,
            &cancel,
        )
        .await;

        if !outcome.keep_server {
            // Drop a failed child; the thread map survives for a later spawn.
            if let Some(server) = state.server.as_mut() {
                server.terminate().await;
            }
            state.server = None;
        }
        let mut result = if outcome.dropped_stale_binding && !self.require_thread {
            // Best-effort dispatch retries; strict review skips this branch.
            match CodexAppServer::spawn_timed(&self.codex_bin, Arc::clone(&self.timing)).await {
                Ok(server) => state.server = Some(server),
                Err(error) => {
                    return Ok(PhaseResult::AgentCrash {
                        error: format!(
                            "failed to spawn codex app-server ({:?}): {error}",
                            self.codex_bin
                        ),
                    });
                }
            }
            let LauncherState {
                server,
                threads,
                removals,
            } = &mut *state;
            let server = server.as_mut().expect("server respawned above");
            let retry = drive_turn(
                server,
                threads,
                removals,
                persist_enabled,
                phase,
                &message,
                session_id,
                cwd,
                &cancel,
            )
            .await;
            if !retry.keep_server {
                if let Some(server) = state.server.as_mut() {
                    server.terminate().await;
                }
                state.server = None;
            }
            retry.result
        } else {
            outcome.result
        };
        // Product-review writes are required independently of whether this
        // round resumed an existing thread. Ordinary dispatch remains
        // best-effort. Preserve the agent failure text too when a rejected
        // strict binding could not be durably tombstoned.
        if let Some(store) = &self.thread_store {
            let persisted = store.persist(
                cwd,
                &state.threads,
                &state.removals,
                self.require_persistence,
            );
            if self.require_persistence {
                if let Err(error) = persisted {
                    let prior = match &result {
                        PhaseResult::AgentCrash { error } => {
                            format!("; prior agent failure: {error}")
                        }
                        _ => String::new(),
                    };
                    result = PhaseResult::AgentCrash {
                        error: format!("{REQUIRED_PERSISTENCE_ERROR_PREFIX}: {error:#}{prior}"),
                    };
                }
            }
        }
        Ok(result)
    }

    fn last_elapsed_ms(&self) -> Option<u64> {
        self.timing.elapsed_ms()
    }

    async fn finish_dispatch(&self) {
        let mut state = self.state.lock().await;
        if let Some(mut server) = state.server.take() {
            server.terminate().await;
        }
    }
}

/// One turn result, child liveness, and stale-binding fallback signal.
struct DriveOutcome {
    result: PhaseResult,
    keep_server: bool,
    dropped_stale_binding: bool,
}

impl DriveOutcome {
    fn now(result: PhaseResult, keep_server: bool) -> Self {
        Self {
            result,
            keep_server,
            dropped_stale_binding: false,
        }
    }
}

/// Why opening a thread did not produce an id.
enum OpenFailure {
    Error(anyhow::Error),
    Timeout,
    Cancelled,
}

impl OpenFailure {
    fn into_result(self) -> PhaseResult {
        match self {
            OpenFailure::Error(error) => PhaseResult::AgentCrash {
                error: error.to_string(),
            },
            OpenFailure::Timeout => PhaseResult::Timeout,
            OpenFailure::Cancelled => PhaseResult::AgentCrash {
                error: "conductor shutdown".into(),
            },
        }
    }
}

/// [`CodexAppServer::open_thread`] raced against the turn deadline and the
/// conductor cancel token.
async fn open_thread_or_fail(
    server: &mut CodexAppServer,
    cwd: &Path,
    resume: Option<&str>,
    deadline: std::pin::Pin<&mut tokio::time::Sleep>,
    cancel: &CancellationToken,
) -> Result<String, OpenFailure> {
    tokio::select! {
        opened = server.open_thread(cwd, resume) => opened.map_err(OpenFailure::Error),
        _ = deadline => Err(OpenFailure::Timeout),
        _ = cancel.cancelled() => Err(OpenFailure::Cancelled),
    }
}

/// Open/resume a thread and map one turn onto [`DriveOutcome`].
#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    server: &mut CodexAppServer,
    threads: &mut HashMap<String, String>,
    removals: &mut HashSet<String>,
    persist_enabled: bool,
    phase: &Phase,
    message: &str,
    session_id: &str,
    cwd: &Path,
    cancel: &CancellationToken,
) -> DriveOutcome {
    let timeout = Duration::from_secs(phase.timeout_sec.unwrap_or(1800));
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let shutdown = || PhaseResult::AgentCrash {
        error: "conductor shutdown".into(),
    };

    let resume = threads.get(session_id).cloned();
    let thread_id = match open_thread_or_fail(
        server,
        cwd,
        resume.as_deref(),
        deadline.as_mut(),
        cancel,
    )
    .await
    {
        Ok(thread_id) => thread_id,
        // Tombstone a rejected persisted binding; the caller decides whether
        // ordinary fallback is allowed. Conduct falls through without a store.
        Err(OpenFailure::Error(error)) if resume.is_some() && persist_enabled => {
            threads.remove(session_id);
            removals.insert(session_id.to_owned());
            return DriveOutcome {
                result: PhaseResult::AgentCrash {
                    error: format!("persisted Codex thread binding rejected: {error}"),
                },
                keep_server: false,
                dropped_stale_binding: true,
            };
        }
        Err(failure) => return DriveOutcome::now(failure.into_result(), false),
    };
    threads.insert(session_id.to_owned(), thread_id.clone());

    let outcome: CodexTurnOutcome = tokio::select! {
        turned = server.run_turn(&thread_id, message) => match turned {
            Ok(outcome) => outcome,
            Err(error) => {
                return DriveOutcome::now(
                    PhaseResult::AgentCrash {
                        error: error.to_string(),
                    },
                    false,
                );
            }
        },
        _ = &mut deadline => return DriveOutcome::now(PhaseResult::Timeout, false),
        _ = cancel.cancelled() => return DriveOutcome::now(shutdown(), false),
    };

    // The app-server protocol exposes no cost/usage data, so neither budget
    // gate can fire. The per-phase check (`over_budget(None, _)`) is always
    // false, and the sequential runner only calls BudgetTracker::record and
    // accumulates state.total_cost_usd inside `if let Some(cost)`, so the
    // plan-level tracker never sees a figure either: both phase and plan
    // budget_usd are unenforced for codex. `edda conduct run` warns about
    // this at startup; the cost column stays empty for codex phases by design.
    let cost_usd = None;
    if over_budget(cost_usd, phase.budget_usd) {
        return DriveOutcome::now(PhaseResult::BudgetExceeded { cost_usd }, true);
    }
    DriveOutcome::now(
        PhaseResult::AgentDone {
            cost_usd,
            result_text: outcome.final_text,
        },
        true,
    )
}

fn over_budget(cost: Option<f64>, budget: Option<f64>) -> bool {
    match (cost, budget) {
        (Some(c), Some(b)) => c > b,
        _ => false,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::CodexLauncher;
    use crate::agent::codex_app_server::CodexAppServer;
    use std::collections::{HashMap, HashSet};
    use std::path::PathBuf;
    use tokio::sync::Mutex;

    /// In-memory fake launcher with persistence disabled.
    pub(crate) fn launcher_with_server(server: CodexAppServer) -> CodexLauncher {
        let mut launcher = CodexLauncher::with_bin(PathBuf::from("unused-fake-bin"));
        launcher.state = Mutex::new(super::LauncherState {
            server: Some(server),
            threads: HashMap::new(),
            removals: HashSet::new(),
        });
        launcher
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::launcher_with_server;
    use super::*;
    use crate::agent::codex_app_server::fake_support::{
        fake_app_server, fake_app_server_bin, FakeScenario,
    };
    use crate::agent::codex_app_server::CodexAppServer;
    use crate::plan::parser::parse_plan;

    fn phase_from_yaml(yaml: &str) -> Phase {
        parse_plan(&format!("name: t\nphases:\n{yaml}"))
            .expect("test plan parses")
            .phases
            .remove(0)
    }

    async fn spawn_fake_server(scenario: FakeScenario) -> (tempfile::TempDir, CodexAppServer) {
        let (dir, command) = fake_app_server(scenario).expect("fake app-server script written");
        let server = CodexAppServer::spawn_command(command)
            .await
            .expect("fake spawned");
        (dir, server)
    }

    /// Unsupported Codex capability declarations fail before spawn (GH-574).
    #[tokio::test]
    async fn codex_refuses_phase_declared_capabilities() {
        let launcher = CodexLauncher::new();
        for yaml in [
            "  - id: a\n    prompt: x\n    model: anthropic/claude-opus-5\n",
            "  - id: a\n    prompt: x\n    thinking: high\n",
            "  - id: a\n    prompt: x\n    tools: [read]\n",
            "  - id: a\n    prompt: x\n    exclude_tools: [write]\n",
        ] {
            let phase = phase_from_yaml(yaml);
            let error = launcher
                .run_phase(
                    &phase,
                    "p",
                    "",
                    "s",
                    Path::new("."),
                    CancellationToken::new(),
                )
                .await
                .expect_err("codex must refuse declared capabilities");
            let text = error.to_string();
            assert!(
                text.contains("codex does not support"),
                "expected explicit refusal, got: {text}"
            );
        }
    }

    include!("codex_rpc_config_tests.rs");

    #[test]
    fn persistence_is_opt_in() {
        // Constructors stay non-persistent until a caller opts in (GH-535).
        assert!(CodexLauncher::new().thread_store.is_none());
        assert!(
            CodexLauncher::with_bin(PathBuf::from("unused"))
                .thread_store
                .is_none(),
            "with_bin must stay non-persistent by default"
        );
        assert!(
            CodexLauncher::new()
                .with_persistent_threads()
                .thread_store
                .is_some(),
            "with_persistent_threads opts into the per-user store"
        );
        assert!(
            CodexLauncher::new()
                .with_thread_store(PathBuf::from("custom-root"))
                .thread_store
                .is_some(),
            "with_thread_store opts into an explicit store root"
        );
        assert!(
            CodexLauncher::new()
                .with_required_persistence()
                .require_persistence
        );
        assert!(CodexLauncher::new().with_required_thread().require_thread);
    }

    #[test]
    fn over_budget_semantics() {
        assert!(over_budget(Some(2.0), Some(1.0)));
        assert!(!over_budget(Some(1.0), Some(1.0)));
        assert!(!over_budget(None, Some(1.0)));
        assert!(!over_budget(Some(5.0), None));
        assert!(!over_budget(None, None));
    }

    #[tokio::test]
    async fn completed_turn_maps_to_agent_done() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (_dir, mut server) = spawn_fake_server(FakeScenario::RunTurnCompletes).await;
        let mut threads = HashMap::new();
        let outcome = drive_turn(
            &mut server,
            &mut threads,
            &mut HashSet::new(),
            false,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(outcome.keep_server);
        match outcome.result {
            PhaseResult::AgentDone {
                cost_usd,
                result_text,
            } => {
                assert_eq!(cost_usd, None, "codex app-server exposes no cost data");
                assert_eq!(result_text.as_deref(), Some("turn complete"));
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn budget_cannot_fire_without_cost_data() -> Result<()> {
        // No app-server usage means the budget gate is inert.
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n    budget_usd: 0.01\n");
        let (_dir, mut server) = spawn_fake_server(FakeScenario::RunTurnCompletes).await;
        let mut threads = HashMap::new();
        let outcome = drive_turn(
            &mut server,
            &mut threads,
            &mut HashSet::new(),
            false,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(outcome.keep_server);
        assert!(
            matches!(
                outcome.result,
                PhaseResult::AgentDone { cost_usd: None, .. }
            ),
            "expected AgentDone without cost, got {:?}",
            outcome.result
        );
        Ok(())
    }

    #[tokio::test]
    async fn turn_error_maps_to_agent_crash() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (_dir, mut server) = spawn_fake_server(FakeScenario::RunTurnStartError).await;
        let mut threads = HashMap::new();
        let outcome = drive_turn(
            &mut server,
            &mut threads,
            &mut HashSet::new(),
            false,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(
            !outcome.keep_server,
            "a failed turn kills the app-server child"
        );
        match outcome.result {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains("bad turn"), "{error}");
            }
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn deadline_returns_timeout_result() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n    timeout_sec: 2\n");
        let (_dir, mut server) = spawn_fake_server(FakeScenario::Idle).await;
        let mut threads = HashMap::new();
        let started = tokio::time::Instant::now();
        let outcome = drive_turn(
            &mut server,
            &mut threads,
            &mut HashSet::new(),
            false,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(!outcome.keep_server);
        assert!(matches!(outcome.result, PhaseResult::Timeout));
        assert!(started.elapsed() < Duration::from_secs(15));
        Ok(())
    }

    #[tokio::test]
    async fn cancel_returns_conductor_shutdown() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (_dir, mut server) = spawn_fake_server(FakeScenario::Idle).await;
        let mut threads = HashMap::new();
        let cancel = CancellationToken::new();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            canceller.cancel();
        });
        let outcome = drive_turn(
            &mut server,
            &mut threads,
            &mut HashSet::new(),
            false,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &cancel,
        )
        .await;
        assert!(!outcome.keep_server);
        match outcome.result {
            PhaseResult::AgentCrash { error } => assert_eq!(error, "conductor shutdown"),
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn same_session_id_resumes_the_same_conversation() -> Result<()> {
        // A repeated in-memory session id resumes its thread.
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (_dir, mut server) = spawn_fake_server(FakeScenario::TwoTurnsWithResume).await;
        let mut threads = HashMap::new();

        let first = drive_turn(
            &mut server,
            &mut threads,
            &mut HashSet::new(),
            false,
            &phase,
            "turn one",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(first.keep_server);
        match first.result {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("first answer"));
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }

        let second = drive_turn(
            &mut server,
            &mut threads,
            &mut HashSet::new(),
            false,
            &phase,
            "turn two",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(second.keep_server);
        match second.result {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("second answer"));
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn run_phase_maps_spawn_failure_to_agent_crash() {
        let launcher =
            CodexLauncher::with_bin(PathBuf::from("definitely-not-a-real-codex-binary-gh527"));
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "do the task",
                "",
                "sid",
                Path::new("."),
                CancellationToken::new(),
            )
            .await
            .expect("run_phase returns a result, not an IO error");
        match result {
            PhaseResult::AgentCrash { error } => {
                assert!(
                    error.contains("failed to spawn codex app-server"),
                    "{error}"
                );
            }
            other => panic!("expected AgentCrash, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_phase_survives_a_crashed_server_by_respawning() -> Result<()> {
        // A crashed child is not reused by the next phase.
        let (_dir, server) = spawn_fake_server(FakeScenario::RunTurnStartError).await;
        let launcher = launcher_with_server(server);
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");

        let first = launcher
            .run_phase(
                &phase,
                "do the task",
                "",
                "sid",
                Path::new("."),
                CancellationToken::new(),
            )
            .await?;
        assert!(
            matches!(&first, PhaseResult::AgentCrash { error } if error.contains("bad turn")),
            "expected turn-error crash, got {first:?}"
        );

        let second = launcher
            .run_phase(
                &phase,
                "do the task",
                "",
                "sid",
                Path::new("."),
                CancellationToken::new(),
            )
            .await?;
        match second {
            PhaseResult::AgentCrash { error } => {
                assert!(
                    error.contains("failed to spawn codex app-server"),
                    "second phase should attempt a fresh spawn, got {error}"
                );
            }
            other => panic!("expected spawn-failure crash, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn run_phase_completes_against_a_fake_server() -> Result<()> {
        let (_dir, server) = spawn_fake_server(FakeScenario::RunTurnCompletes).await;
        let launcher = launcher_with_server(server);
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "do the task",
                "plan context",
                "sid",
                Path::new("."),
                CancellationToken::new(),
            )
            .await?;
        match result {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("turn complete"));
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }
        Ok(())
    }

    include!("codex_rpc_persistence_tests.rs");
}
