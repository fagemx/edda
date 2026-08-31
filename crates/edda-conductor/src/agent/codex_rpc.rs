use crate::agent::codex_app_server::{CodexAppServer, CodexTurnOutcome};
use crate::agent::launcher::{AgentLauncher, PhaseResult};
use crate::plan::schema::Phase;
use anyhow::Result;
use std::collections::HashMap;
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
/// thread via `thread/resume` whenever a caller reuses a session id. That
/// is a forward-looking path for a future dispatch primitive: the
/// sequential runner derives a unique session id per plan+phase+attempt,
/// so resume does not currently fire in production (see
/// [`crate::agent::launcher::phase_session_id`]). When the child dies
/// (crash, timeout, cancellation) the client is dropped and the next phase
/// re-spawns it; the thread map survives because codex persists threads.
pub struct CodexLauncher {
    pub codex_bin: PathBuf,
    pub verbose: bool,
    state: Mutex<LauncherState>,
}

#[derive(Default)]
struct LauncherState {
    server: Option<CodexAppServer>,
    /// conductor session_id → codex thread_id
    threads: HashMap<String, String>,
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
            state: Mutex::new(LauncherState::default()),
        }
    }

    pub fn with_bin(codex_bin: PathBuf) -> Self {
        Self {
            codex_bin,
            verbose: false,
            state: Mutex::new(LauncherState::default()),
        }
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
    async fn run_phase(
        &self,
        phase: &Phase,
        prompt: &str,
        plan_context: &str,
        session_id: &str,
        cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<PhaseResult> {
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

        let LauncherState { server, threads } = &mut *state;
        let server = server.as_mut().expect("server spawned above");
        let (result, keep_server) =
            drive_turn(server, threads, phase, &message, session_id, cwd, &cancel).await;

        if !keep_server {
            // The turn ended in a crash, timeout, or shutdown, and the child
            // was killed along the way (KillOnCancel or terminate). Drop the
            // client so the next phase re-spawns a fresh app-server. The
            // thread map survives: codex persists threads and `thread/resume`
            // restores the conversation.
            state.server = None;
        }
        Ok(result)
    }
}

/// Open (or resume) the codex thread for `session_id`, run one turn, and map
/// the outcome onto `PhaseResult`.
///
/// Returns whether the server is still usable: `false` after any crash,
/// timeout, or cancellation, since every one of those paths kills the child.
async fn drive_turn(
    server: &mut CodexAppServer,
    threads: &mut HashMap<String, String>,
    phase: &Phase,
    message: &str,
    session_id: &str,
    cwd: &Path,
    cancel: &CancellationToken,
) -> (PhaseResult, bool) {
    let timeout = Duration::from_secs(phase.timeout_sec.unwrap_or(1800));
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    let shutdown = || PhaseResult::AgentCrash {
        error: "conductor shutdown".into(),
    };

    let resume = threads.get(session_id).cloned();
    let thread_id = tokio::select! {
        opened = server.open_thread(cwd, resume.as_deref()) => match opened {
            Ok(thread_id) => thread_id,
            Err(error) => {
                return (
                    PhaseResult::AgentCrash {
                        error: error.to_string(),
                    },
                    false,
                );
            }
        },
        _ = &mut deadline => return (PhaseResult::Timeout, false),
        _ = cancel.cancelled() => return (shutdown(), false),
    };
    threads.insert(session_id.to_owned(), thread_id.clone());

    let outcome: CodexTurnOutcome = tokio::select! {
        turned = server.run_turn(&thread_id, message) => match turned {
            Ok(outcome) => outcome,
            Err(error) => {
                return (
                    PhaseResult::AgentCrash {
                        error: error.to_string(),
                    },
                    false,
                );
            }
        },
        _ = &mut deadline => return (PhaseResult::Timeout, false),
        _ = cancel.cancelled() => return (shutdown(), false),
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
        return (PhaseResult::BudgetExceeded { cost_usd }, true);
    }
    (
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
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tokio::sync::Mutex;

    /// A launcher pre-seeded with an already-spawned server, for tests that
    /// drive `run_phase` against a fake app-server without spawning binaries.
    pub(crate) fn launcher_with_server(server: CodexAppServer) -> CodexLauncher {
        let mut launcher = CodexLauncher::with_bin(PathBuf::from("unused-fake-bin"));
        launcher.state = Mutex::new(super::LauncherState {
            server: Some(server),
            threads: HashMap::new(),
        });
        launcher
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::launcher_with_server;
    use super::*;
    use crate::agent::codex_app_server::fake_support::{fake_app_server, FakeScenario};
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
        let (result, keep_server) = drive_turn(
            &mut server,
            &mut threads,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(keep_server);
        match result {
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
        let (result, keep_server) = drive_turn(
            &mut server,
            &mut threads,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(keep_server);
        assert!(
            matches!(result, PhaseResult::AgentDone { cost_usd: None, .. }),
            "expected AgentDone without cost, got {result:?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn turn_error_maps_to_agent_crash() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (_dir, mut server) = spawn_fake_server(FakeScenario::RunTurnStartError).await;
        let mut threads = HashMap::new();
        let (result, keep_server) = drive_turn(
            &mut server,
            &mut threads,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(!keep_server, "a failed turn kills the app-server child");
        match result {
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
        let (result, keep_server) = drive_turn(
            &mut server,
            &mut threads,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(!keep_server);
        assert!(matches!(result, PhaseResult::Timeout));
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
        let (result, keep_server) = drive_turn(
            &mut server,
            &mut threads,
            &phase,
            "do the task",
            "sid",
            Path::new("."),
            &cancel,
        )
        .await;
        assert!(!keep_server);
        match result {
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

        let (first, keep) = drive_turn(
            &mut server,
            &mut threads,
            &phase,
            "turn one",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(keep);
        match first {
            PhaseResult::AgentDone { result_text, .. } => {
                assert_eq!(result_text.as_deref(), Some("first answer"));
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }

        let (second, keep) = drive_turn(
            &mut server,
            &mut threads,
            &phase,
            "turn two",
            "sid",
            Path::new("."),
            &CancellationToken::new(),
        )
        .await;
        assert!(keep);
        match second {
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
}
