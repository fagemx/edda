use crate::agent::codex_app_server::{CodexAppServer, CodexTurnOutcome};
use crate::agent::launcher::{AgentLauncher, PhaseResult};
use crate::plan::schema::Phase;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

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
/// Cross-process, `edda dispatch` persists the map in the per-user edda
/// store at `<store_root>/projects/<project_id(cwd)>/state/
/// codex-threads.json`, written through `edda_store::write_atomic` under an
/// exclusive file lock (GH-535). Persistence is dispatch-scoped: a launcher
/// built with [`CodexLauncher::with_persistent_threads`] — which is exactly
/// what `edda dispatch` builds — merges the persisted map into `threads` on
/// the first successful spawn, so a repeated `--session-id` resumes the
/// conversation a previous process recorded. Conduct deliberately keeps the
/// default non-persistent launcher: its session ids are deterministic per
/// plan/phase/attempt, and its behavior must stay byte-identical with the
/// pre-persistence path (no store reads, no surprise resumes). A missing
/// entry is simply a new conversation. A persisted binding that the server
/// rejects — corrupt file or a stale/invalid thread id — degrades to
/// `thread/start` within the same dispatch, and the bad binding is erased
/// from the persisted map: the removal is recorded as an explicit deletion
/// that [`ThreadStore::persist`] honors, so even a failed fallback (the
/// retry also erroring, timing out, or being cancelled) cannot write the
/// rejected binding back and make the next dispatch resume it again.
/// Resume is a convenience and must never fail a dispatch. This degrade
/// path is persistence-scoped: a launcher without a thread store (conduct)
/// keeps the pre-persistence behavior for an in-memory resume failure — the
/// turn simply crashes — because conduct's verdict-gated redispatch turns
/// reuse the same session id on purpose and must stay byte-identical.
/// Within one process the in-memory map stays the hot path and behavior is
/// unchanged. When the child dies (crash, timeout, cancellation) the
/// client is dropped and the next phase re-spawns it; the thread map
/// survives because codex persists threads and the store records their ids.
pub struct CodexLauncher {
    pub codex_bin: PathBuf,
    pub verbose: bool,
    thread_store: Option<ThreadStore>,
    state: Mutex<LauncherState>,
}

/// File-backed cold-start store for the session→thread map.
///
/// Purely a resume convenience: every fallible step (missing file, corrupt
/// JSON, lock or write failure) degrades to "no persisted entries" rather
/// than an error, so persistence problems can never fail a dispatch. Only
/// corruption warns — a missing file is just a first conversation.
struct ThreadStore {
    /// Per-user edda store root (`edda_store::store_root()`).
    root: PathBuf,
}

impl ThreadStore {
    fn from_default_root() -> Self {
        Self {
            root: edda_store::store_root(),
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

    fn load(&self, cwd: &Path) -> HashMap<String, String> {
        let path = self.map_path(cwd);
        match std::fs::read_to_string(&path) {
            Err(_) => HashMap::new(),
            Ok(raw) => match serde_json::from_str(&raw) {
                Ok(map) => map,
                Err(_) => {
                    eprintln!(
                        "Warning: codex thread map {} is corrupt; ignoring it and \"
                         starting fresh conversations (resume needs a new dispatch).",
                        path.display()
                    );
                    HashMap::new()
                }
            },
        }
    }

    /// Merge `threads` into the persisted map under an exclusive file lock,
    /// so two concurrent dispatch processes cannot lose each other's
    /// entries. In-memory entries win on key conflicts, and every session
    /// in `removals` is deleted from the merged map before the overlay: a
    /// binding the server rejected must not survive a failed fallback just
    /// because it is absent from the in-memory overlay.
    fn persist(&self, cwd: &Path, threads: &HashMap<String, String>, removals: &HashSet<String>) {
        if threads.is_empty() && removals.is_empty() {
            return;
        }
        let path = self.map_path(cwd);
        let Ok(_lock) = edda_store::lock_file(&path.with_extension("lock")) else {
            return;
        };
        let mut merged = self.load_quiet(cwd);
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
        if let Ok(bytes) = serde_json::to_vec(&merged) {
            let _ = edda_store::write_atomic(&path, &bytes);
        }
    }

    /// [`Self::load`] without the corruption warning — used while already
    /// holding the lock inside [`Self::persist`], where the warning would
    /// fire twice for the same bad file.
    fn load_quiet(&self, cwd: &Path) -> HashMap<String, String> {
        let path = self.map_path(cwd);
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
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
            state: Mutex::new(LauncherState::default()),
        }
    }

    pub fn with_bin(codex_bin: PathBuf) -> Self {
        Self {
            codex_bin,
            verbose: false,
            thread_store: None,
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
        self.thread_store = Some(ThreadStore { root });
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
        // GH-574 honesty: the codex app-server exposes no model/thinking/
        // tool-policy selection path edda can verify, so a phase declaring
        // any of them would be silently ignored — the exact failure mode
        // GH-574 removes. Refuse with an explicit error instead of guessing
        // at an unverified configuration channel.
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

        // The app-server has no system-prompt channel; carry plan context
        // inline, same as the pi launcher.
        let message = if plan_context.is_empty() {
            prompt.to_owned()
        } else {
            format!("{plan_context}\n\n{prompt}")
        };
        let mut state = self.state.lock().await;
        if state.server.is_none() {
            match CodexAppServer::spawn(&self.codex_bin).await {
                // Cold start: merge the persisted session→thread map so a
                // session id recorded by a previous process resumes instead
                // of starting over. In-memory entries win (hot path).
                Ok(server) => {
                    if let Some(store) = &self.thread_store {
                        for (session, thread) in store.load(cwd) {
                            state.threads.entry(session).or_insert(thread);
                        }
                    }
                    state.server = Some(server);
                }
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

        let LauncherState {
            server,
            threads,
            removals,
        } = &mut *state;
        let server = server.as_mut().expect("server spawned above");
        // Only a persistence-enabled launcher may degrade a rejected
        // resume to thread/start. With `thread_store: None` (conduct), an
        // in-memory resume failure must crash exactly as before the
        // persistence feature: conduct deliberately redispatches the same
        // session id, and silently starting a new thread would lose the
        // same-session conversation.
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
            // The turn ended in a crash, timeout, or shutdown, and the child
            // was killed along the way (KillOnCancel or terminate). Drop the
            // client so the next phase re-spawns a fresh app-server. The
            // thread map survives: codex persists threads and `thread/resume`
            // restores the conversation.
            state.server = None;
        }
        let result = if outcome.dropped_stale_binding {
            // The persisted binding for this session was rejected by the
            // server (stale or invalid thread id). Degrade to a plain
            // `thread/start` within the same dispatch: the failed resume
            // terminated the child, so spawn a fresh app-server first. The
            // bad binding was already removed from `threads`, so it is not
            // written back below.
            match CodexAppServer::spawn(&self.codex_bin).await {
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
                state.server = None;
            }
            retry.result
        } else {
            outcome.result
        };
        // Record the turn's session→thread binding even on a crash path:
        // the thread was created and codex persists it, so the next process
        // can resume it. Best-effort — failures are swallowed by design.
        if let Some(store) = &self.thread_store {
            store.persist(cwd, &state.threads, &state.removals);
        }
        Ok(result)
    }
}

/// One [`drive_turn`] attempt: the mapped phase result, whether the
/// app-server child survived, and whether a stale persisted thread binding
/// was dropped — asking [`AgentLauncher::run_phase`] to retry once as a
/// plain `thread/start`.
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

/// Why a thread open did not produce a thread id. `Error` leaves the
/// child's fate to the protocol layer (a failed request terminates it);
/// timeout and cancellation mean the dispatch itself is over.
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

/// Open (or resume) the codex thread for `session_id`, run one turn, and map
/// the outcome onto [`DriveOutcome`]. The parameters are one flat turn
/// description; a parameter struct would be churn, so the argument count is
/// accepted here.
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
        // A persisted binding the server rejects (stale or invalid thread
        // id) must degrade to `thread/start`, not fail the dispatch. Drop
        // the binding and record an explicit deletion so `persist` erases
        // it from disk even if the fallback never records a fresh id, and
        // tell run_phase to retry on a fresh app-server — the failed resume
        // terminated the child. Gated on persistence: with no thread store
        // (conduct) the error below falls through to the plain crash path,
        // byte-identical with the pre-persistence behavior.
        Err(OpenFailure::Error(_)) if resume.is_some() && persist_enabled => {
            threads.remove(session_id);
            removals.insert(session_id.to_owned());
            return DriveOutcome {
                result: PhaseResult::AgentCrash {
                    error: "persisted thread binding rejected; degrading to thread/start".into(),
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

    /// A launcher pre-seeded with an already-spawned server, for tests that
    /// drive `run_phase` against a fake app-server without spawning binaries.
    /// Persistence is off (the default now): these tests assert in-process
    /// behavior and must never touch the real per-user store.
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

    /// GH-574: codex has no verifiable model/thinking/tool-policy selection
    /// path, so each declared capability must be refused explicitly — never
    /// accepted and silently ignored. The refusal fires before any server
    /// spawn, so the test runs against a bare launcher.
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

    #[test]
    fn codex_bin_falls_back_to_the_platform_install() {
        // npm ships codex as an extensionless sh launcher plus codex.cmd on
        // Windows, with no codex.exe, and CreateProcess does not apply
        // PATHEXT — the bare name never resolves there.
        let expected = if cfg!(windows) { "codex.cmd" } else { "codex" };
        assert_eq!(resolve_codex_bin(None), PathBuf::from(expected));
    }

    #[test]
    fn edda_codex_bin_overrides_the_platform_default() {
        let custom = "/opt/codex/bin/codex-custom";
        assert_eq!(
            resolve_codex_bin(Some(OsString::from(custom))),
            PathBuf::from(custom)
        );
    }

    #[test]
    fn empty_edda_codex_bin_is_treated_as_unset() {
        let expected = if cfg!(windows) { "codex.cmd" } else { "codex" };
        assert_eq!(
            resolve_codex_bin(Some(OsString::new())),
            PathBuf::from(expected),
            "an empty override must not produce an unspawnable empty path"
        );
    }

    #[test]
    fn with_bin_overrides_the_default() {
        let custom = PathBuf::from("/opt/codex/bin/codex");
        assert_eq!(CodexLauncher::with_bin(custom.clone()).codex_bin, custom);
    }

    #[test]
    fn persistence_is_opt_in() {
        // GH-535 round 1: persistence must be dispatch-scoped. `new()` and
        // `with_bin()` — the constructors conduct's launcher factory goes
        // through — must not touch the per-user store at all; only an
        // explicit opt-in enables it.
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
        // The app-server protocol reports no usage, so a budgeted phase that
        // completes normally still lands on AgentDone rather than
        // BudgetExceeded — the budget gate is inert for codex by design.
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
        // Exercises the forward-looking resume path: the scripted fake
        // answers thread/start with t-1 and thread/resume with t-2, so a
        // second turn that produces output proves the launcher resumed the
        // persisted thread instead of starting a new one. The sequential
        // runner assigns a unique session id per phase+attempt, so this
        // reuse is not hit in production today.
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
        // First phase crashes (turn error); the client is dropped, so the
        // second phase reports the re-spawn failure instead of reusing a
        // dead child — proving the reset happened.
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

    // ── Cross-process thread-map persistence (GH-535) ──

    /// The map file a launcher with `root` writes for `cwd`.
    fn map_path_for(root: &Path, cwd: &Path) -> PathBuf {
        root.join("projects")
            .join(edda_store::project_id(cwd))
            .join("state")
            .join("codex-threads.json")
    }

    #[test]
    fn thread_store_round_trips_the_session_map() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore {
            root: root.path().to_path_buf(),
        };
        let mut threads = HashMap::new();
        threads.insert("sess-1".to_owned(), "t-1".to_owned());

        store.persist(cwd.path(), &threads, &HashSet::new());

        let loaded = store.load(cwd.path());
        assert_eq!(loaded.get("sess-1").map(String::as_str), Some("t-1"));
        Ok(())
    }

    #[test]
    fn thread_store_persist_merges_with_entries_from_another_process() -> Result<()> {
        // Simulate the other process having written its own binding to the
        // shared map between our load and our write: the merge must keep it.
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore {
            root: root.path().to_path_buf(),
        };
        let foreign = r#"{"sess-other":"t-other","sess-mine":"t-stale"}"#;
        std::fs::create_dir_all(map_path_for(root.path(), cwd.path()).parent().unwrap())?;
        std::fs::write(map_path_for(root.path(), cwd.path()), foreign)?;

        let mut threads = HashMap::new();
        threads.insert("sess-mine".to_owned(), "t-fresh".to_owned());
        store.persist(cwd.path(), &threads, &HashSet::new());

        let loaded = store.load(cwd.path());
        assert_eq!(
            loaded.get("sess-other").map(String::as_str),
            Some("t-other")
        );
        assert_eq!(loaded.get("sess-mine").map(String::as_str), Some("t-fresh"));
        Ok(())
    }

    #[test]
    fn thread_store_persist_honors_removal_tombstones() -> Result<()> {
        // A rejected binding is modeled as an explicit deletion, not mere
        // absence from the in-memory overlay: persist must apply the
        // removal to the on-disk map, so a failed fallback (no fresh
        // binding recorded) cannot resurrect the rejected entry.
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore {
            root: root.path().to_path_buf(),
        };
        let map = map_path_for(root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, r#"{"sess-1":"stale","sess-other":"t-other"}"#)?;

        let mut threads = HashMap::new();
        threads.insert("sess-2".to_owned(), "t-2".to_owned());
        let mut removals = HashSet::new();
        removals.insert("sess-1".to_owned());
        store.persist(cwd.path(), &threads, &removals);

        let loaded = store.load(cwd.path());
        assert_eq!(
            loaded.get("sess-other").map(String::as_str),
            Some("t-other")
        );
        assert_eq!(loaded.get("sess-2").map(String::as_str), Some("t-2"));
        assert!(
            !loaded.contains_key("sess-1"),
            "the tombstoned binding must be deleted from disk, got {loaded:?}"
        );
        Ok(())
    }

    #[test]
    fn thread_store_missing_or_corrupt_file_loads_empty() -> Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let store = ThreadStore {
            root: root.path().to_path_buf(),
        };
        assert!(store.load(cwd.path()).is_empty(), "missing file is empty");

        let map = map_path_for(root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, b"{ not json")?;
        assert!(store.load(cwd.path()).is_empty(), "corrupt file is empty");
        Ok(())
    }

    #[tokio::test]
    async fn fresh_launcher_resumes_the_thread_a_previous_process_recorded() -> Result<()> {
        // Stand-in for two `edda dispatch --agent codex --session-id sess-1`
        // processes: two independently constructed launchers sharing the
        // store root. The first records sess-1 → t-1 via thread/start; the
        // second must send thread/resume (the ResumeOnly fake answers an
        // error for anything else) and produce the resumed turn's answer.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");

        let (_fake_dir, first_bin) =
            fake_app_server_bin(FakeScenario::RunTurnCompletes).expect("first fake written");
        let first =
            CodexLauncher::with_bin(first_bin).with_thread_store(store_root.path().to_path_buf());
        let first_result = first
            .run_phase(
                &phase,
                "turn one",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("first dispatch runs");
        assert!(
            matches!(&first_result, PhaseResult::AgentDone { result_text, .. } if result_text.as_deref() == Some("turn complete")),
            "first dispatch should complete, got {first_result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(map_path_for(store_root.path(), cwd.path()))
                .expect("map persisted"),
            r#"{"sess-1":"t-1"}"#
        );

        let (_fake_dir, second_bin) =
            fake_app_server_bin(FakeScenario::ResumeOnly).expect("second fake written");
        let second =
            CodexLauncher::with_bin(second_bin).with_thread_store(store_root.path().to_path_buf());
        let second_result = second
            .run_phase(
                &phase,
                "turn two",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("second dispatch runs");
        match second_result {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("resumed answer"));
            }
            other => panic!("second dispatch should resume the recorded thread, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn corrupt_store_entry_degrades_to_thread_start() -> Result<()> {
        // A corrupt map must never fail the dispatch: the fresh launcher
        // warns and falls back to thread/start, which completes normally.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, b"{ corrupted")?;

        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::RunTurnCompletes).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "do the task",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("dispatch must not fail on a corrupt store entry");
        assert!(
            matches!(&result, PhaseResult::AgentDone { result_text, .. } if result_text.as_deref() == Some("turn complete")),
            "degraded dispatch should complete via thread/start, got {result:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn stale_persisted_binding_degrades_to_thread_start_and_is_not_rewritten() -> Result<()> {
        // A syntactically valid but stale entry (the server no longer knows
        // the thread) loads fine, so the first open is a thread/resume that
        // the fake rejects. The dispatch must recover via thread/start
        // within the same run — not crash — and must not write the stale
        // binding back: the map ends up holding only the fresh binding.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, br#"{"sess-1":"stale-thread"}"#)?;

        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::ResumeErrorThenStart).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "turn one",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("a stale binding must degrade, not fail the dispatch");
        match result {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("fresh answer"));
            }
            other => panic!(
                "stale binding should degrade to thread/start in the same dispatch, got {other:?}"
            ),
        }
        assert_eq!(
            std::fs::read_to_string(&map).expect("map rewritten"),
            r#"{"sess-1":"t-1"}"#,
            "the stale binding must not be written back"
        );
        Ok(())
    }

    /// Seed the in-memory session→thread map of a launcher built by
    /// [`launcher_with_server`], without touching the real store.
    async fn seed_threads(launcher: &CodexLauncher, session: &str, thread: &str) {
        let mut state = launcher.state.lock().await;
        state.threads.insert(session.to_owned(), thread.to_owned());
    }

    #[tokio::test]
    async fn conduct_without_persistence_still_crashes_when_resume_is_rejected() -> Result<()> {
        // GH-535 round 2, P1-B: the stale-binding fallback (drop + retry as
        // thread/start) must be reachable only on the persistence-enabled
        // dispatch path. Conduct reuses a deterministic session id for a
        // verdict-gated redispatch; if that in-memory resume is rejected,
        // conduct must fail with the server's error exactly as before the
        // persistence feature — not silently start a new thread and lose
        // the same-session conversation. The ResumeErrorThenStart fake
        // would happily answer thread/start with "fresh answer", so a
        // retry here would show up as AgentDone instead of the crash.
        let (_dir, server) = spawn_fake_server(FakeScenario::ResumeErrorThenStart).await;
        let launcher = launcher_with_server(server);
        seed_threads(&launcher, "sid", "stale-thread").await;
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "redispatch turn",
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
                    error.contains("unknown thread"),
                    "conduct resume failure must surface the server's own error, got {error}"
                );
            }
            other => {
                panic!("non-persistent conduct must not fall back to thread/start, got {other:?}")
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn failed_fallback_still_erases_the_stale_binding_from_disk() -> Result<()> {
        // GH-535 round 2, P1-A: dropping the stale binding from the
        // in-memory map is not enough — if the fallback thread/start fails
        // (or times out, or is cancelled) before a fresh id is recorded,
        // the merge-style persist reloads the on-disk map and would write
        // the rejected binding straight back. The removal must survive as
        // an explicit deletion that persist honors, so the next dispatch
        // never resumes the same rejected thread id again.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let map = map_path_for(store_root.path(), cwd.path());
        std::fs::create_dir_all(map.parent().unwrap())?;
        std::fs::write(&map, br#"{"sess-1":"stale-thread"}"#)?;

        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::ResumeErrorThenStartError).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "turn one",
                "",
                "sess-1",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("run_phase returns a result, not an IO error");
        // The fallback thread/start fails too, so the phase ends in a
        // crash — but the rejected binding must still be gone from disk.
        assert!(
            matches!(&result, PhaseResult::AgentCrash { error } if error.contains("unknown thread")),
            "expected the failed fallback to surface as AgentCrash, got {result:?}"
        );
        let loaded: HashMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&map).expect("map still readable"))
                .expect("map stays valid JSON");
        assert!(
            !loaded.contains_key("sess-1"),
            "the rejected binding must not survive on disk after a failed fallback, got {loaded:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn missing_store_entry_starts_a_fresh_thread() -> Result<()> {
        // No map file at all: the first dispatch with a session id is a
        // plain thread/start, not a failure and not a warning.
        let store_root = tempfile::tempdir()?;
        let cwd = tempfile::tempdir()?;
        let (_fake_dir, bin) =
            fake_app_server_bin(FakeScenario::RunTurnCompletes).expect("fake written");
        let launcher =
            CodexLauncher::with_bin(bin).with_thread_store(store_root.path().to_path_buf());
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let result = launcher
            .run_phase(
                &phase,
                "do the task",
                "",
                "sess-fresh",
                cwd.path(),
                CancellationToken::new(),
            )
            .await
            .expect("first dispatch runs");
        assert!(matches!(result, PhaseResult::AgentDone { .. }));
        Ok(())
    }
}
