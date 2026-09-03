use crate::agent::launcher::{AgentLauncher, PhaseResult};
use crate::plan::schema::Phase;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Upper bound on waiting for the stderr drain once stdout has hit EOF.
const STDERR_DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Upper bound on waiting for a child reap whose exit is expected but not
/// guaranteed (stdout EOF or dead-stdin while the process may still live).
/// On expiry the error carries the stderr tail collected so far plus an
/// explicit note instead of hanging the turn; `kill_on_drop` cleans up later.
const REAP_GRACE: Duration = Duration::from_secs(5);

/// Resolve the pi executable from an explicit `EDDA_PI_BIN` value, falling
/// back to the name npm installs on this platform.
///
/// On Windows the npm package ships `pi` as a `.cmd` shim with no `.exe`;
/// `CreateProcess` — unlike a shell — does not apply `PATHEXT`, so the bare
/// name never resolves and every phase fails at `verify_available`.
///
/// Takes the override as an argument rather than reading the environment so
/// the resolution is testable without mutating process-wide state.
fn resolve_pi_bin(explicit: Option<OsString>) -> PathBuf {
    match explicit {
        // An empty `EDDA_PI_BIN=` is a set-but-unusable value; treat it as unset
        // rather than spawning an empty path.
        Some(value) if !value.is_empty() => PathBuf::from(value),
        _ if cfg!(windows) => PathBuf::from("pi.cmd"),
        _ => PathBuf::from("pi"),
    }
}

fn default_pi_bin() -> PathBuf {
    resolve_pi_bin(std::env::var_os("EDDA_PI_BIN"))
}

/// Launches the pi coding agent via `pi --mode rpc`.
///
/// pi RPC mode speaks JSONL over stdin/stdout: commands go in one JSON object
/// per line, events and command responses come out one per line. Framing is
/// strict LF (`\n`) — input may carry `\r\n`, but U+2028/U+2029 are valid
/// inside JSON strings and must never be treated as record delimiters.
///
/// A phase maps to one RPC turn: send a `prompt` command, stream events until
/// `agent_settled` (the only reliable completion signal — `agent_end` may be
/// followed by retries or queued continuations), then read the session cost.
pub struct PiRpcLauncher {
    pub pi_bin: PathBuf,
    pub verbose: bool,
    /// Optional `--model` pattern (e.g. `provider/id`). Fallback for the
    /// per-phase `phase.model` declaration, which takes precedence (GH-574).
    pub model: Option<String>,
    /// Optional `--session-dir` for pi session storage.
    pub session_dir: Option<PathBuf>,
    /// In-band model report from the most recent settled turn (RPC
    /// `get_state`), for [`AgentLauncher::last_observed_model`].
    observed_model: std::sync::Mutex<Option<String>>,
}

impl Default for PiRpcLauncher {
    fn default() -> Self {
        Self::new()
    }
}

impl PiRpcLauncher {
    pub fn new() -> Self {
        Self {
            pi_bin: default_pi_bin(),
            verbose: false,
            model: None,
            session_dir: None,
            observed_model: std::sync::Mutex::new(None),
        }
    }

    pub fn with_bin(pi_bin: PathBuf) -> Self {
        Self {
            pi_bin,
            verbose: false,
            model: None,
            session_dir: None,
            observed_model: std::sync::Mutex::new(None),
        }
    }

    fn record_observed_model(&self, model: Option<String>) {
        if let Ok(mut slot) = self.observed_model.lock() {
            *slot = model;
        }
    }

    pub fn with_verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn with_session_dir(mut self, dir: PathBuf) -> Self {
        self.session_dir = Some(dir);
        self
    }

    /// Check that the pi CLI binary is reachable.
    pub fn verify_available(&self) -> Result<()> {
        let status = std::process::Command::new(&self.pi_bin)
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(s) if s.success() => Ok(()),
            _ => anyhow::bail!(
                "pi CLI not found (looked for {:?}).\n\
                 Install: npm install -g @earendil-works/pi-coding-agent\n\
                 Or set EDDA_PI_BIN if the executable lives elsewhere.",
                self.pi_bin
            ),
        }
    }

    /// Resolve the effective model for one phase: the per-phase declaration
    /// wins (it is the more specific, per-turn contract); the builder value
    /// is the fallback. Callers must have rejected the thinking-level /
    /// model-suffix conflict beforehand.
    fn effective_model(&self, phase: &Phase) -> Option<String> {
        phase.model.clone().or_else(|| self.model.clone())
    }

    /// GH-574: pi's model pattern can embed a thinking level
    /// (`provider/id:high`) and pi also has a separate `--thinking` flag.
    /// edda refuses the ambiguous combination instead of guessing which
    /// one wins.
    fn validate_phase(&self, phase: &Phase) -> Result<()> {
        if let (Some(level), Some(model)) = (&phase.thinking, &self.effective_model(phase)) {
            if model.contains(':') {
                anyhow::bail!(
                    "phase {:?} declares both thinking level {level:?} and model pattern \
                     {model:?} with an embedded `:<thinking>` suffix; drop one — they are two \
                     spellings of the same setting and edda refuses to guess",
                    phase.id
                );
            }
        }
        Ok(())
    }

    fn build_command(&self, phase: &Phase, session_id: &str, cwd: &Path) -> Command {
        let mut cmd = Command::new(&self.pi_bin);
        cmd.arg("--mode")
            .arg("rpc")
            // Same session id across calls preserves conversation context
            // (create-or-continue), so report → dispatch-next stays coherent.
            .arg("--session-id")
            .arg(session_id)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Piped, not inherited: stderr would corrupt the console during a
            // JSONL run, but it is the only channel carrying startup failures,
            // so it is drained in the background and folded into error text.
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            // Propagate session_id so agent-spawned `edda decide` etc. can resolve identity
            .env("EDDA_SESSION_ID", session_id);

        if let Some(model) = &self.effective_model(phase) {
            cmd.arg("--model").arg(model);
        }
        if let Some(dir) = &self.session_dir {
            cmd.arg("--session-dir").arg(dir);
        }
        // GH-574: per-phase model / thinking / tool policy. Tools are
        // comma-joined per pi's own `--tools a,b` syntax. Note `--tools` is
        // an allowlist that replaces pi's default tool set, while
        // `--exclude-tools` only removes from it.
        if let Some(level) = &phase.thinking {
            cmd.arg("--thinking").arg(level);
        }
        if let Some(tools) = &phase.tools {
            cmd.arg("--tools").arg(tools.join(","));
        }
        if let Some(tools) = &phase.exclude_tools {
            cmd.arg("--exclude-tools").arg(tools.join(","));
        }

        for (k, v) in &phase.env {
            cmd.env(k, v);
        }
        cmd
    }
}

#[async_trait::async_trait]
impl AgentLauncher for PiRpcLauncher {
    async fn run_phase(
        &self,
        phase: &Phase,
        prompt: &str,
        plan_context: &str,
        session_id: &str,
        cwd: &Path,
        cancel: CancellationToken,
    ) -> Result<PhaseResult> {
        self.validate_phase(phase)?;
        let command = self.build_command(phase, session_id, cwd);
        let timeout_sec = phase.timeout_sec.unwrap_or(1800);
        let (result, observed) = run_command(
            command,
            phase,
            prompt,
            plan_context,
            self.verbose,
            Duration::from_secs(timeout_sec),
            cancel,
        )
        .await?;
        // In-band model observation: whatever pi itself reported via RPC
        // `get_state`, or nothing. Never inferred from config or sessions.
        self.record_observed_model(observed);
        Ok(result)
    }

    fn last_observed_model(&self) -> Option<String> {
        self.observed_model
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }
}

/// Run one phase against a pre-built command (injectable for tests).
/// Returns the phase result plus the model pi reported in-band via RPC
/// `get_state`, if the turn settled and pi exposed one (GH-574).
async fn run_command(
    command: Command,
    phase: &Phase,
    prompt: &str,
    plan_context: &str,
    verbose: bool,
    timeout: Duration,
    cancel: CancellationToken,
) -> Result<(PhaseResult, Option<String>)> {
    let mut session = PiRpcSession::spawn_command(command).await?;
    // pi has no system-prompt flag in RPC mode; carry plan context inline.
    let message = if plan_context.is_empty() {
        prompt.to_owned()
    } else {
        format!("{plan_context}\n\n{prompt}")
    };
    let result = session
        .run_turn(&message, phase.budget_usd, timeout, verbose, &cancel)
        .await;
    // Only a settled turn is asked for state; a crashed turn has nothing
    // trustworthy to report.
    let observed = if matches!(result, Ok(PhaseResult::AgentDone { .. })) {
        session.observed_model().await
    } else {
        None
    };
    session.terminate().await;
    let result = result?;
    Ok((result, observed))
}

/// Run `pi --list-models [search]` and return its stdout verbatim — the
/// query path that lets callers look up available provider/model pairs
/// instead of guessing a provider prefix (GH-574).
pub fn list_models(pi_bin: Option<PathBuf>, search: Option<&str>) -> Result<String> {
    let mut cmd = std::process::Command::new(pi_bin.unwrap_or_else(default_pi_bin));
    cmd.arg("--list-models");
    if let Some(term) = search {
        if !term.is_empty() {
            cmd.arg(term);
        }
    }
    let output = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("failed to run pi --list-models")?;
    if !output.status.success() {
        anyhow::bail!(
            "pi --list-models failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// One live `pi --mode rpc` process speaking JSONL.
struct PiRpcSession {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    stderr: Arc<Mutex<String>>,
    /// Handle to the stderr drain task, awaited before reading the buffer so
    /// the tail is known-complete rather than whatever happened to land first.
    stderr_drain: Option<JoinHandle<()>>,
    next_id: u64,
}

impl PiRpcSession {
    async fn spawn_command(mut command: Command) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command.spawn().context("failed to spawn pi RPC process")?;
        let stdin = child.stdin.take().context("pi RPC stdin was not piped")?;
        let stdout = child.stdout.take().context("pi RPC stdout was not piped")?;

        // Drain stderr continuously: an unread pipe fills and blocks the child.
        let stderr = Arc::new(Mutex::new(String::new()));
        let stderr_drain = child.stderr.take().map(|pipe| {
            let sink = Arc::clone(&stderr);
            tokio::spawn(async move {
                let mut lines = BufReader::new(pipe).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let mut buf = sink.lock().await;
                    buf.push_str(&line);
                    buf.push('\n');
                }
            })
        });

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            stderr,
            stderr_drain,
            next_id: 1,
        })
    }

    async fn terminate(&mut self) {
        let _ = self.child.kill().await;
        let _ = tokio::time::timeout(REAP_GRACE, self.child.wait()).await;
    }

    fn take_id(&mut self) -> String {
        let id = format!("req-{}", self.next_id);
        self.next_id += 1;
        id
    }

    async fn write(&mut self, value: &Value) -> Result<()> {
        write_json_line(&mut self.stdin, value).await
    }

    /// Send one prompt and stream events until `agent_settled`.
    ///
    /// Protocol-level failures (rejected prompt, malformed JSONL, EOF before
    /// settlement) surface as `PhaseResult::AgentCrash` — the runner retries
    /// crashes per its failure policy. `Err` is reserved for spawn/IO errors.
    #[allow(clippy::too_many_lines)] // 172 lines at #779; split tracked in none
    async fn run_turn(
        &mut self,
        message: &str,
        budget_usd: Option<f64>,
        timeout: Duration,
        verbose: bool,
        cancel: &CancellationToken,
    ) -> Result<PhaseResult> {
        let prompt_id = self.take_id();
        let request = json!({ "id": prompt_id, "type": "prompt", "message": message });
        if let Err(error) = self.write(&request).await {
            // A pi that dies during startup (bad auth, bad --model,
            // unwritable --session-dir) never reads stdin, so this write
            // races the process death. Losing that race used to discard the
            // collected stderr — the only place the real reason appears — and
            // surface a bare IO error instead. Reclassify the dead-child
            // error class into the same reap → drain-stderr → crash path the
            // stdout-EOF branch takes, so the outcome is unconditional.
            if !is_child_gone_error(&error) {
                return Err(error);
            }
            let reaped = tokio::time::timeout(REAP_GRACE, self.child.wait())
                .await
                .is_ok();
            let tail = collect_stderr_tail(&mut self.stderr_drain, &self.stderr).await;
            return Ok(PhaseResult::AgentCrash {
                error: eof_error(&mut self.child, &tail, reaped),
            });
        }

        let mut state = TurnState::default();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);

        let Self {
            child,
            stdin,
            stdout,
            stderr,
            stderr_drain,
            ..
        } = self;
        let mut settled = false;
        while !settled {
            tokio::select! {
                line = stdout.next_line() => {
                    let Some(line) = line.context("failed reading pi RPC stdout")? else {
                        // Reap first so the stderr pipe is closed, then let the
                        // drain finish — otherwise the reason races the report.
                        let reaped = tokio::time::timeout(REAP_GRACE, child.wait())
                            .await
                            .is_ok();
                        let tail = collect_stderr_tail(stderr_drain, stderr).await;
                        return Ok(PhaseResult::AgentCrash {
                            error: eof_error(child, &tail, reaped),
                        });
                    };
                    let record = clean_record(&line);
                    if record.is_empty() {
                        continue;
                    }
                    let msg = match parse_message(record) {
                        Ok(msg) => msg,
                        Err(error) => {
                            return Ok(PhaseResult::AgentCrash {
                                error: error.to_string(),
                            });
                        }
                    };

                    if let Some(reply) = extension_ui_cancellation(&msg) {
                        // Auto-dismiss dialog requests instead of blocking forever.
                        write_json_line(stdin, &reply).await?;
                        continue;
                    }

                    if is_prompt_response(&msg, &prompt_id) {
                        if msg.get("success").and_then(Value::as_bool) != Some(true) {
                            let detail = msg
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("unknown error");
                            return Ok(PhaseResult::AgentCrash {
                                error: format!("pi rejected prompt: {detail}"),
                            });
                        }
                        continue;
                    }

                    match msg.get("type").and_then(Value::as_str) {
                        Some("agent_settled") => settled = true,
                        // pi reports a failed turn in band and settles
                        // normally afterwards (GH-669): `turn_end` carries
                        // `stopReason: "error"` with the reason in
                        // `errorMessage`, and an authentication failure
                        // reaches `agent_settled` with no text and zero
                        // cost. Reading settlement alone reported that turn
                        // as done — `edda dispatch` exit 0. Only `turn_end`
                        // is read, never the per-message `message_end`: it
                        // is the turn's last word, so a recovered
                        // mid-turn error does not fail the phase.
                        Some("turn_end") => state.error = turn_error(&msg),
                        Some("message_update") => {
                            state.observe_message_update(&msg);
                            if over_budget(state.cost, budget_usd) {
                                let _ = write_json_line(stdin, &json!({ "type": "abort" })).await;
                                return Ok(PhaseResult::BudgetExceeded { cost_usd: state.cost });
                            }
                        }
                        Some("tool_execution_start") if verbose => {
                            let tool = msg
                                .get("toolName")
                                .and_then(Value::as_str)
                                .unwrap_or("?");
                            println!("  🔨 {tool}");
                        }
                        _ => {}
                    }
                }
                _ = &mut deadline => {
                    let _ = child.kill().await;
                    return Ok(PhaseResult::Timeout);
                }
                _ = cancel.cancelled() => {
                    let _ = write_json_line(stdin, &json!({ "type": "abort" })).await;
                    let _ = child.kill().await;
                    return Ok(PhaseResult::AgentCrash {
                        error: "conductor shutdown".into(),
                    });
                }
            }
        }

        if let Some(error) = state.error.take() {
            return Ok(PhaseResult::AgentCrash { error });
        }

        // After settlement, `get_session_stats` gives the authoritative
        // session-wide cost (includes tool usage); fall back to the cost
        // accumulated from stream usage events.
        let stats_cost = match tokio::time::timeout(
            Duration::from_secs(15),
            self.request(json!({ "type": "get_session_stats" })),
        )
        .await
        {
            Ok(Ok(data)) => data.get("cost").and_then(Value::as_f64),
            _ => None,
        };
        let final_cost = stats_cost.or(state.cost);

        if over_budget(final_cost, budget_usd) {
            return Ok(PhaseResult::BudgetExceeded {
                cost_usd: final_cost,
            });
        }

        let result_text = if state.result_text.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut state.result_text))
        };
        Ok(PhaseResult::AgentDone {
            cost_usd: final_cost,
            result_text,
        })
    }

    /// Ask pi for its current state and extract the in-band model report
    /// (`provider/id`) per the RPC `get_state` contract. Any failure —
    /// transport error, missing fields — renders as `None` ("unknown"),
    /// never a guess (GH-574 honesty rule).
    async fn observed_model(&mut self) -> Option<String> {
        let state = self.request(json!({ "type": "get_state" })).await.ok()?;
        let model = state.get("model")?;
        let provider = model.get("provider").and_then(Value::as_str)?;
        let id = model.get("id").and_then(Value::as_str)?;
        Some(format!("{provider}/{id}"))
    }

    /// Send one RPC command and wait for its matching response.
    /// Events interleaved before the response are ignored.
    async fn request(&mut self, mut command: Value) -> Result<Value> {
        let id = self.take_id();
        command["id"] = json!(id);
        self.write(&command).await?;
        loop {
            let Some(line) = self
                .stdout
                .next_line()
                .await
                .context("failed reading pi RPC stdout")?
            else {
                let reaped = tokio::time::timeout(REAP_GRACE, self.child.wait())
                    .await
                    .is_ok();
                let tail = collect_stderr_tail(&mut self.stderr_drain, &self.stderr).await;
                return Err(anyhow!(eof_error(&mut self.child, &tail, reaped)));
            };
            let record = clean_record(&line);
            if record.is_empty() {
                continue;
            }
            let msg = parse_message(record)?;
            if msg.get("type").and_then(Value::as_str) == Some("response")
                && msg.get("id").and_then(Value::as_str) == Some(id.as_str())
            {
                if msg.get("success").and_then(Value::as_bool) != Some(true) {
                    let detail = msg
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error");
                    return Err(anyhow!("pi RPC command failed: {detail}"));
                }
                return Ok(msg.get("data").cloned().unwrap_or(Value::Null));
            }
        }
    }
}

/// Strip a single trailing `\n` and optional `\r`.
///
/// pi RPC framing is LF-only; `\r\n` input is tolerated by stripping the
/// trailing `\r`. U+2028/U+2029 are never record delimiters — `read_line`
/// splits on the `\n` byte only, so they stay inside JSON strings.
fn clean_record(line: &str) -> &str {
    let without_newline = line.strip_suffix('\n').unwrap_or(line);
    without_newline
        .strip_suffix('\r')
        .unwrap_or(without_newline)
}

fn parse_message(line: &str) -> Result<Value> {
    serde_json::from_str(line).with_context(|| format!("invalid pi RPC JSON: {line}"))
}

/// Classify a stdin-write failure as "the child is gone".
///
/// Only `BrokenPipe` / `WriteZero` indicate the reader side of the stdin pipe
/// has been closed — the signature of a pi that died during startup and never
/// read its stdin. Every other IO error (disk full, permission denied, ...) is
/// a genuine failure while pi may still be alive and keeps the documented
/// `Err`-for-IO contract.
fn is_child_gone_error(error: &anyhow::Error) -> bool {
    error
        .chain()
        .filter_map(|cause| cause.downcast_ref::<std::io::Error>())
        .any(|io| {
            matches!(
                io.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::WriteZero
            )
        })
}

async fn write_json_line(stdin: &mut ChildStdin, value: &Value) -> Result<()> {
    let mut line = serde_json::to_vec(value).context("failed to encode pi RPC command")?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .context("failed writing pi RPC stdin")?;
    stdin.flush().await.context("failed flushing pi RPC stdin")
}

fn eof_error(child: &mut Child, stderr_tail: &str, reaped: bool) -> String {
    let base = match child.try_wait() {
        Ok(Some(status)) if !status.success() => {
            format!("pi RPC process exited with non-zero status {status} before agent_settled")
        }
        Ok(Some(status)) => format!("unexpected EOF from pi RPC process (status {status})"),
        Ok(None) => "unexpected EOF from pi RPC process".to_owned(),
        Err(error) => format!("unexpected EOF from pi RPC process: {error}"),
    };
    let error = if stderr_tail.is_empty() {
        base
    } else {
        // Startup failures (missing auth, bad --model, unwritable --session-dir)
        // only ever surface on stderr; without it the operator sees a bare
        // exit status and no reason.
        format!("{base}; pi stderr: {stderr_tail}")
    };
    if reaped {
        error
    } else {
        format!("{error}; child did not exit within grace")
    }
}

/// Wait for the stderr drain to finish, then read the buffer.
///
/// The caller reaches this only after stdout hit EOF, so pi's stderr pipe is
/// closing too and the drain terminates on its own. The bound is a safety net
/// for a child that leaves the pipe open (e.g. an inherited grandchild), where
/// a best-effort tail beats hanging the phase.
async fn collect_stderr_tail(
    drain: &mut Option<JoinHandle<()>>,
    buffer: &Arc<Mutex<String>>,
) -> String {
    if let Some(handle) = drain.take() {
        if tokio::time::timeout(STDERR_DRAIN_GRACE, handle)
            .await
            .is_err()
        {
            // Timed out: the task keeps running against a detached pipe and
            // exits when it closes. Read whatever landed so far.
        }
    }
    format_stderr_tail(&buffer.lock().await)
}

/// Last few non-empty stderr lines, joined, capped so one runaway trace cannot
/// dominate the error message.
fn format_stderr_tail(raw: &str) -> String {
    const MAX_LINES: usize = 5;
    const MAX_CHARS: usize = 500;
    let lines: Vec<&str> = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let start = lines.len().saturating_sub(MAX_LINES);
    let mut tail = lines[start..].join(" | ");
    if tail.chars().count() > MAX_CHARS {
        tail = tail.chars().take(MAX_CHARS).collect::<String>() + "…";
    }
    tail
}

fn is_prompt_response(msg: &Value, prompt_id: &str) -> bool {
    msg.get("type").and_then(Value::as_str) == Some("response")
        && msg.get("command").and_then(Value::as_str) == Some("prompt")
        && msg.get("id").and_then(Value::as_str) == Some(prompt_id)
}

fn over_budget(cost: Option<f64>, budget: Option<f64>) -> bool {
    match (cost, budget) {
        (Some(c), Some(b)) => c > b,
        _ => false,
    }
}

/// Build a cancellation reply for blocking extension UI dialog requests
/// (`select`/`confirm`/`input`/`editor`). Fire-and-forget requests
/// (`notify`, `setStatus`, ...) get `None` and are simply ignored.
fn extension_ui_cancellation(msg: &Value) -> Option<Value> {
    if msg.get("type").and_then(Value::as_str) != Some("extension_ui_request") {
        return None;
    }
    let method = msg.get("method").and_then(Value::as_str)?;
    if !matches!(method, "select" | "confirm" | "input" | "editor") {
        return None;
    }
    let id = msg.get("id")?;
    Some(json!({ "type": "extension_ui_response", "id": id, "cancelled": true }))
}

/// The reason a `turn_end` event reports, when pi ended the turn on an
/// error rather than an answer (GH-669). `None` for a turn that ended
/// normally, so a settled turn stays a completed one.
fn turn_error(msg: &Value) -> Option<String> {
    if msg.pointer("/message/stopReason").and_then(Value::as_str) != Some("error") {
        return None;
    }
    let detail = msg
        .pointer("/message/errorMessage")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .unwrap_or("pi ended the turn on an error without a reason");
    Some(detail.to_owned())
}

/// Accumulates result text and cost from `message_update` events, plus the
/// reason a `turn_end` gave for a failed turn.
#[derive(Default)]
struct TurnState {
    result_text: String,
    cost: Option<f64>,
    error: Option<String>,
}

impl TurnState {
    fn observe_message_update(&mut self, msg: &Value) {
        // Top-level usage is the latest cumulative provider-reported usage.
        if let Some(cost) = msg.pointer("/usage/cost/total").and_then(Value::as_f64) {
            self.cost = Some(cost);
        }
        let event = match msg.pointer("/assistantMessageEvent") {
            Some(event) => event,
            None => return,
        };
        if event.get("type").and_then(Value::as_str) == Some("text_delta") {
            if let Some(delta) = event.get("delta").and_then(Value::as_str) {
                self.result_text.push_str(delta);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::parser::parse_plan;

    const PROMPT_OK: &str = r#"{"id":"req-1","type":"response","command":"prompt","success":true}"#;
    const PROMPT_REJECTED: &str =
        r#"{"id":"req-1","type":"response","command":"prompt","success":false,"error":"nope"}"#;
    const SETTLED: &str = r#"{"type":"agent_settled"}"#;
    const STATS_042: &str = r#"{"id":"req-2","type":"response","command":"get_session_stats","success":true,"data":{"cost":0.42}}"#;
    const STATS_990: &str = r#"{"id":"req-2","type":"response","command":"get_session_stats","success":true,"data":{"cost":9.9}}"#;
    const STATE_GPT56: &str = r#"{"id":"req-3","type":"response","command":"get_state","success":true,"data":{"model":{"provider":"openai-codex","id":"gpt-5.6-sol"},"thinkingLevel":"high"}}"#;
    const STATE_NULL: &str = r#"{"id":"req-3","type":"response","command":"get_state","success":true,"data":{"model":null}}"#;
    const STARTUP_DIAGNOSTIC: &str = "pi: no API key configured for provider openrouter";
    /// The `turn_end` pi emits when the provider rejects its credentials,
    /// captured 2026-09-02 from `pi --mode rpc --model openai/gpt-4o-mini`
    /// with an invalid `OPENAI_API_KEY` (`content`, `usage` and the nested
    /// provider JSON elided). pi follows it with `agent_settled`.
    const TURN_END_AUTH_ERROR: &str = r#"{"type":"turn_end","message":{"role":"assistant","content":[],"provider":"openai","model":"gpt-4o-mini","stopReason":"error","errorMessage":"OpenAI API error (401): Incorrect API key provided"}}"#;

    fn delta_line(usage_cost: &str, text: &str) -> String {
        format!(
            r#"{{"type":"message_update","usage":{{"cost":{{"total":{usage_cost}}}}},"assistantMessageEvent":{{"type":"text_delta","contentIndex":0,"delta":"{text}"}}}}"#
        )
    }

    #[derive(Clone, Copy)]
    enum FakeScenario {
        Normal,
        /// Like Normal, but pi reports no model in `get_state`.
        NormalNoModel,
        BudgetAfterSettle,
        BudgetMidRun,
        PromptRejected,
        Idle,
        Malformed,
        /// JSONL line containing a raw U+2028 inside a string value.
        #[cfg(unix)]
        Utf8Framing,
        /// Writes a startup diagnostic to stderr and dies before settling.
        StderrThenDie,
        /// Answers the prompt, then ends the turn on a provider
        /// authentication error and settles normally (GH-669).
        TurnEndAuthError,
        /// Closes stdout while staying alive: stdout EOF with a live child.
        #[cfg(unix)]
        CloseStdoutAndLive,
    }

    fn body_for(scenario: FakeScenario) -> String {
        match scenario {
            FakeScenario::Normal => format!(
                "read_cmd\nwrite_line '{PROMPT_OK}'\nwrite_line '{}'\nwrite_line '{SETTLED}'\nread_cmd\nwrite_line '{STATS_042}'\nread_cmd\nwrite_line '{STATE_GPT56}'\nsleep 60",
                delta_line("0.10", "phase done")
            ),
            FakeScenario::NormalNoModel => format!(
                "read_cmd\nwrite_line '{PROMPT_OK}'\nwrite_line '{}'\nwrite_line '{SETTLED}'\nread_cmd\nwrite_line '{STATS_042}'\nread_cmd\nwrite_line '{STATE_NULL}'\nsleep 60",
                delta_line("0.10", "phase done")
            ),
            FakeScenario::BudgetAfterSettle => format!(
                "read_cmd\nwrite_line '{PROMPT_OK}'\nwrite_line '{}'\nwrite_line '{SETTLED}'\nread_cmd\nwrite_line '{STATS_990}'\nsleep 60",
                delta_line("0.10", "phase done")
            ),
            FakeScenario::BudgetMidRun => format!(
                "read_cmd\nwrite_line '{PROMPT_OK}'\nwrite_line '{}'\nsleep 60",
                delta_line("5.0", "x")
            ),
            FakeScenario::TurnEndAuthError => format!(
                "read_cmd
write_line '{PROMPT_OK}'
write_line '{TURN_END_AUTH_ERROR}'
write_line '{SETTLED}'
sleep 60"
            ),
            FakeScenario::PromptRejected => {
                format!("read_cmd\nwrite_line '{PROMPT_REJECTED}'\nsleep 60")
            }
            FakeScenario::Idle => "sleep 60".to_owned(),
            FakeScenario::Malformed => "read_cmd\nwrite_line 'not-json'\nsleep 60".to_owned(),
            // No stdout at all: only the stderr diagnostic, then a hard exit —
            // the shape of a real pi startup failure (bad auth, bad --model).
            FakeScenario::StderrThenDie => {
                format!("write_err '{STARTUP_DIAGNOSTIC}'\nexit_fail")
            }
            #[cfg(unix)]
            // stdout EOF with a still-live child: exercises the reap bound.
            FakeScenario::CloseStdoutAndLive => "exec 1>&-\nsleep 60".to_owned(),
            #[cfg(unix)]
            FakeScenario::Utf8Framing => format!(
                "read_cmd\nwrite_line '{PROMPT_OK}'\nprintf '%s\\n' '{}'\nwrite_line '{SETTLED}'\nread_cmd\nwrite_line '{STATS_042}'\nread_cmd\nwrite_line '{STATE_GPT56}'\nsleep 60",
                delta_line("0.10", "a\u{2028}b")
            ),
        }
    }

    fn fake_pi_command(scenario: FakeScenario) -> anyhow::Result<(tempfile::TempDir, Command)> {
        let dir = tempfile::tempdir()?;

        #[cfg(windows)]
        {
            let script = dir.path().join("fake-pi.ps1");
            std::fs::write(&script, powershell_script(scenario))?;
            let mut command = match std::env::var("SystemRoot") {
                Ok(root) => Command::new(format!(
                    "{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
                )),
                Err(_) => Command::new("powershell.exe"),
            };
            command.args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ]);
            command.arg(script);
            Ok((dir, command))
        }

        #[cfg(unix)]
        {
            let script = dir.path().join("fake-pi.sh");
            std::fs::write(&script, shell_script(scenario))?;
            let mut command = Command::new("/bin/sh");
            command.arg(script);
            Ok((dir, command))
        }
    }

    #[cfg(windows)]
    fn powershell_script(scenario: FakeScenario) -> String {
        let body = body_for(scenario)
            .replace("read_cmd", "Read-Line")
            .replace("write_line", "Write-Line")
            .replace("write_err", "Write-Err")
            .replace("exit_fail", "exit 3")
            .replace("sleep 60", "Start-Sleep -Seconds 60");
        format!(
            "$ErrorActionPreference = 'Stop'\nfunction Read-Line {{ if ($null -eq [Console]::In.ReadLine()) {{ exit 0 }} }}\nfunction Write-Line([string]$line) {{ [Console]::Out.WriteLine($line); [Console]::Out.Flush() }}\nfunction Write-Err([string]$line) {{ [Console]::Error.WriteLine($line); [Console]::Error.Flush() }}\n{body}\n"
        )
    }

    #[cfg(unix)]
    fn shell_script(scenario: FakeScenario) -> String {
        let body = body_for(scenario).replace("exit_fail", "exit 3");
        format!(
            "#!/bin/sh\nread_cmd() {{ IFS= read -r _ || exit 0; }}\nwrite_line() {{ printf '%s\\n' \"$1\"; }}\nwrite_err() {{ printf '%s\\n' \"$1\" >&2; }}\n{body}\n"
        )
    }

    fn phase_from_yaml(yaml: &str) -> Phase {
        parse_plan(&format!("name: t\nphases:\n{yaml}"))
            .expect("test plan parses")
            .phases
            .remove(0)
    }

    async fn run_fake(
        scenario: FakeScenario,
        phase: &Phase,
    ) -> Result<(PhaseResult, Option<String>)> {
        let (_dir, command) = fake_pi_command(scenario)?;
        let outcome = tokio::time::timeout(
            Duration::from_secs(30),
            run_command(
                command,
                phase,
                "do the task",
                "",
                false,
                Duration::from_secs(phase.timeout_sec.unwrap_or(1800)),
                CancellationToken::new(),
            ),
        )
        .await
        .context("test timed out waiting for run_command")??;
        Ok(outcome)
    }

    #[tokio::test]
    async fn completes_phase_via_agent_settled() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (result, observed) = run_fake(FakeScenario::Normal, &phase).await?;
        match result {
            PhaseResult::AgentDone {
                cost_usd,
                result_text,
            } => {
                assert!((cost_usd.unwrap() - 0.42).abs() < 1e-9);
                assert_eq!(result_text.as_deref(), Some("phase done"));
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }
        // GH-574: the model pi reported in-band via get_state.
        assert_eq!(observed.as_deref(), Some("openai-codex/gpt-5.6-sol"));
        Ok(())
    }

    #[tokio::test]
    async fn no_in_band_model_report_is_none_not_a_guess() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (result, observed) = run_fake(FakeScenario::NormalNoModel, &phase).await?;
        assert!(matches!(result, PhaseResult::AgentDone { .. }));
        assert!(observed.is_none(), "null model must render as unknown");
        Ok(())
    }

    #[tokio::test]
    async fn turn_ended_on_provider_auth_error_is_a_crash_not_done() -> Result<()> {
        // GH-669: pi settles normally after a failed turn, so reading
        // `agent_settled` alone reported an authentication failure as a
        // completed turn — `edda dispatch` exit 0 with a null error.
        let phase = phase_from_yaml(
            "  - id: a
    prompt: x
",
        );
        let (result, _observed) = run_fake(FakeScenario::TurnEndAuthError, &phase).await?;
        match result {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains("401"), "{error}");
                assert!(error.contains("Incorrect API key"), "{error}");
            }
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn turn_error_reads_only_a_failed_turn_end() {
        let settled_ok: Value = serde_json::from_str(
            r#"{"type":"turn_end","message":{"role":"assistant","stopReason":"stop"}}"#,
        )
        .unwrap();
        assert_eq!(turn_error(&settled_ok), None);

        let failed: Value = serde_json::from_str(TURN_END_AUTH_ERROR).unwrap();
        assert_eq!(
            turn_error(&failed).as_deref(),
            Some("OpenAI API error (401): Incorrect API key provided")
        );

        // A failed turn with no reason still fails, with a stated one.
        let bare: Value =
            serde_json::from_str(r#"{"type":"turn_end","message":{"stopReason":"error"}}"#)
                .unwrap();
        assert!(turn_error(&bare).is_some_and(|e| !e.is_empty()));
    }

    #[tokio::test]
    async fn budget_exceeded_after_settle() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n    budget_usd: 1.0\n");
        let (result, _observed) = run_fake(FakeScenario::BudgetAfterSettle, &phase).await?;
        match result {
            PhaseResult::BudgetExceeded { cost_usd } => {
                assert!((cost_usd.unwrap() - 9.9).abs() < 1e-9);
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn budget_exceeded_mid_run_sends_abort() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n    budget_usd: 1.0\n");
        let (result, _observed) = run_fake(FakeScenario::BudgetMidRun, &phase).await?;
        match result {
            PhaseResult::BudgetExceeded { cost_usd } => {
                assert!((cost_usd.unwrap() - 5.0).abs() < 1e-9);
            }
            other => panic!("expected BudgetExceeded, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn timeout_returns_timeout_result() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n    timeout_sec: 2\n");
        let started = tokio::time::Instant::now();
        let (result, _observed) = run_fake(FakeScenario::Idle, &phase).await?;
        assert!(matches!(result, PhaseResult::Timeout));
        assert!(started.elapsed() < Duration::from_secs(15));
        Ok(())
    }

    #[tokio::test]
    async fn cancel_returns_conductor_shutdown() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (_dir, command) = fake_pi_command(FakeScenario::Idle)?;
        let cancel = CancellationToken::new();
        let canceller = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            canceller.cancel();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            run_command(
                command,
                &phase,
                "do the task",
                "",
                false,
                Duration::from_secs(1800),
                cancel,
            ),
        )
        .await
        .context("test timed out waiting for run_command")??;
        let (result, observed) = result;
        assert!(observed.is_none(), "a cancelled turn reports no model");
        match result {
            PhaseResult::AgentCrash { error } => assert_eq!(error, "conductor shutdown"),
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn prompt_rejection_is_agent_crash() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (result, _observed) = run_fake(FakeScenario::PromptRejected, &phase).await?;
        match result {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains("rejected prompt"), "{error}");
                assert!(error.contains("nope"), "{error}");
            }
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    #[tokio::test]
    async fn malformed_jsonl_line_is_agent_crash() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (result, _observed) = run_fake(FakeScenario::Malformed, &phase).await?;
        match result {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains("invalid pi RPC JSON"), "{error}");
            }
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn crlf_and_u2028_do_not_split_records() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let (result, _observed) = run_fake(FakeScenario::Utf8Framing, &phase).await?;
        match result {
            PhaseResult::AgentDone { result_text, .. } => {
                // The fake writes CRLF via sh (printf adds LF; sh keeps LF) and
                // embeds a raw U+2028 inside the delta string — the record must
                // arrive intact with the separator preserved in the text.
                assert_eq!(result_text.as_deref(), Some("a\u{2028}b"));
            }
            other => panic!("expected AgentDone, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn clean_record_strips_crlf() {
        assert_eq!(clean_record("{\"a\":1}\r\n"), "{\"a\":1}");
        assert_eq!(clean_record("{\"a\":1}\n"), "{\"a\":1}");
        assert_eq!(clean_record("{\"a\":1}"), "{\"a\":1}");
        assert_eq!(clean_record(""), "");
    }

    #[test]
    fn parse_message_accepts_u2028_in_string() {
        let line = format!(
            r#"{{"type":"message_update","assistantMessageEvent":{{"type":"text_delta","delta":"a{}b"}}}}"#,
            '\u{2028}'
        );
        let msg = parse_message(&line).expect("U+2028 inside a JSON string is valid JSONL");
        assert_eq!(
            msg.pointer("/assistantMessageEvent/delta")
                .and_then(Value::as_str),
            Some("a\u{2028}b")
        );
    }

    #[test]
    fn parse_message_rejects_garbage() {
        let error = parse_message("not-json").expect_err("malformed JSON should fail");
        assert!(error.to_string().contains("invalid pi RPC JSON"));
    }

    #[test]
    fn turn_state_accumulates_text_and_cost() {
        let mut state = TurnState::default();
        state.observe_message_update(&parse_message(&delta_line("0.1", "Hello ")).unwrap());
        state.observe_message_update(&parse_message(&delta_line("0.2", "world")).unwrap());
        assert_eq!(state.result_text, "Hello world");
        assert!((state.cost.unwrap() - 0.2).abs() < 1e-9);
    }

    #[test]
    fn turn_state_ignores_non_text_deltas() {
        let mut state = TurnState::default();
        let tool_delta = r#"{"type":"message_update","usage":{"cost":{"total":0.5}},"assistantMessageEvent":{"type":"toolcall_start","contentIndex":1,"id":"call_1","toolName":"bash"}}"#;
        state.observe_message_update(&parse_message(tool_delta).unwrap());
        assert!(state.result_text.is_empty());
        assert!((state.cost.unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn extension_ui_dialog_gets_cancelled() {
        let msg = parse_message(
            r#"{"type":"extension_ui_request","id":"uuid-1","method":"confirm","title":"Allow?"}"#,
        )
        .unwrap();
        let reply = extension_ui_cancellation(&msg).expect("dialog should be cancelled");
        assert_eq!(reply["type"], "extension_ui_response");
        assert_eq!(reply["id"], "uuid-1");
        assert_eq!(reply["cancelled"], true);
    }

    #[test]
    fn extension_ui_fire_and_forget_is_ignored() {
        let notify = parse_message(
            r#"{"type":"extension_ui_request","id":"uuid-2","method":"notify","message":"hi"}"#,
        )
        .unwrap();
        assert!(extension_ui_cancellation(&notify).is_none());

        let event = parse_message(r#"{"type":"agent_start"}"#).unwrap();
        assert!(extension_ui_cancellation(&event).is_none());
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
    async fn startup_stderr_reaches_the_crash_error() -> Result<()> {
        // Regression guard for the drain race: the reason must be present
        // every run, not whenever the drain task happens to win.
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        for attempt in 0..5 {
            let (result, _observed) = run_fake(FakeScenario::StderrThenDie, &phase).await?;
            match result {
                PhaseResult::AgentCrash { error } => {
                    assert!(
                        error.contains(STARTUP_DIAGNOSTIC),
                        "attempt {attempt}: stderr reason missing from {error}"
                    );
                    assert!(error.contains("pi stderr:"), "attempt {attempt}: {error}");
                }
                other => panic!("expected AgentCrash, got {other:?}"),
            }
        }
        Ok(())
    }

    /// The deterministic losing branch of the startup race (#536): the fake
    /// writes a stderr diagnostic and exits WITHOUT reading stdin, and this
    /// test waits until the child has demonstrably exited BEFORE invoking the
    /// turn — so the stdin write is guaranteed to hit a dead child instead of
    /// racing it. The turn must still surface AgentCrash with the stderr
    /// reason, not a bare IO error.
    #[tokio::test]
    async fn dead_child_stdin_write_still_reports_stderr_reason() -> Result<()> {
        let (_dir, command) = fake_pi_command(FakeScenario::StderrThenDie)?;
        let mut session = PiRpcSession::spawn_command(command).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if session.child.try_wait()?.is_some() {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "fake pi never exited"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let cancel = CancellationToken::new();
        // Same outer bound `run_fake` gives siblings: a regression here must
        // fail the test, not hang the CI job.
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            session.run_turn("do the task", None, Duration::from_secs(30), false, &cancel),
        )
        .await
        .context("test timed out waiting for run_turn")?;
        session.terminate().await;
        let result = result?;
        match result {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains(STARTUP_DIAGNOSTIC), "{error}");
                assert!(error.contains("pi stderr:"), "{error}");
            }
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    /// #538: stdout hits EOF while the child is still alive — the reap must
    /// be bounded by a grace period and the error must say the child did not
    /// exit, instead of hanging the turn.
    #[cfg(unix)]
    #[tokio::test]
    async fn reap_grace_expiry_reports_instead_of_hanging() -> Result<()> {
        let phase = phase_from_yaml("  - id: a\n    prompt: x\n");
        let started = std::time::Instant::now();
        let (result, _observed) = run_fake(FakeScenario::CloseStdoutAndLive, &phase).await?;
        assert!(
            started.elapsed() < Duration::from_secs(25),
            "reap grace expiry must be prompt"
        );
        match result {
            PhaseResult::AgentCrash { error } => {
                assert!(error.contains("did not exit within grace"), "{error}");
            }
            other => panic!("expected AgentCrash, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn child_gone_error_predicate_matches_broken_pipe_and_write_zero() {
        // A genuine stdin IO error with a LIVE child (e.g. a full disk, a
        // permission failure on the pipe handle) returning `Err` cannot be
        // constructed cheaply with the fake machinery: the fake scripts only
        // control stdout/stderr and exit codes, and there is no portable way
        // to inject an arbitrary IO error into a live pipe write. The
        // classification boundary is therefore pinned directly on the
        // predicate: only BrokenPipe/WriteZero — the kinds the OS reports
        // when the reader side of the pipe is gone — are reclassified, and
        // every other kind (and non-IO error) stays `Err`.
        let broken_pipe = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "pipe closed",
        ))
        .context("failed writing pi RPC stdin");
        assert!(is_child_gone_error(&broken_pipe));

        let write_zero = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            "write zero",
        ))
        .context("failed writing pi RPC stdin");
        assert!(is_child_gone_error(&write_zero));

        let permission = anyhow::Error::new(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "denied",
        ))
        .context("failed writing pi RPC stdin");
        assert!(!is_child_gone_error(&permission));

        assert!(!is_child_gone_error(&anyhow!("not an io error")));
    }

    #[test]
    fn stderr_tail_keeps_last_lines_and_drops_blanks() {
        let raw = "first\n\n  second  \nthird\nfourth\nfifth\nsixth\n";
        let tail = format_stderr_tail(raw);
        assert_eq!(tail, "second | third | fourth | fifth | sixth");
        assert!(!tail.contains("first"), "only the last 5 lines are kept");
    }

    // ── GH-574: phase-declared capabilities reach the pi spawn line ──

    fn pi_args_for(launcher: &PiRpcLauncher, yaml: &str) -> Vec<String> {
        let phase = phase_from_yaml(yaml);
        let cmd = launcher.build_command(&phase, "sess-1", Path::new("."));
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn pi_phase_capabilities_reach_the_spawn_line() {
        let launcher = PiRpcLauncher::new();
        let args = pi_args_for(
            &launcher,
            "  - id: a\n    prompt: x\n    model: anthropic/claude-opus-5\n    thinking: high\n    tools: [read, grep]\n    exclude_tools: [edit, write]\n",
        );
        for (flag, value) in [
            ("--model", "anthropic/claude-opus-5"),
            ("--thinking", "high"),
            ("--tools", "read,grep"),
            ("--exclude-tools", "edit,write"),
        ] {
            let pos = args
                .iter()
                .position(|a| a == flag)
                .unwrap_or_else(|| panic!("{flag} must appear in the pi spawn line: {args:?}"));
            assert_eq!(args[pos + 1], value, "value after {flag}");
        }
    }

    #[test]
    fn pi_phase_model_wins_over_the_builder_fallback() {
        let launcher = PiRpcLauncher::new().with_model("openai-codex/gpt-5.6-sol");
        let args = pi_args_for(&launcher, "  - id: a\n    prompt: x\n");
        let pos = args.iter().position(|a| a == "--model").expect("--model");
        assert_eq!(args[pos + 1], "openai-codex/gpt-5.6-sol");

        let args = pi_args_for(
            &launcher,
            "  - id: a\n    prompt: x\n    model: anthropic/claude-opus-5\n",
        );
        let pos = args.iter().position(|a| a == "--model").expect("--model");
        assert_eq!(args[pos + 1], "anthropic/claude-opus-5");
    }

    #[test]
    fn pi_no_declarations_spawn_no_capability_flags() {
        let args = pi_args_for(&PiRpcLauncher::new(), "  - id: a\n    prompt: x\n");
        for flag in ["--model", "--thinking", "--tools", "--exclude-tools"] {
            assert!(
                !args.contains(&flag.to_string()),
                "{flag} must be absent without a declaration: {args:?}"
            );
        }
    }

    #[test]
    fn pi_refuses_thinking_flag_plus_model_suffix() {
        let launcher = PiRpcLauncher::new();
        let phase = phase_from_yaml(
            "  - id: a\n    prompt: x\n    model: openai-codex/gpt-5.6-sol:high\n    thinking: low\n",
        );
        let error = launcher
            .validate_phase(&phase)
            .expect_err("the ambiguous combination must be refused");
        assert!(error.to_string().contains("refuses to guess"), "{error}");
    }

    #[test]
    fn pi_accepts_thinking_flag_without_model_suffix() {
        let launcher = PiRpcLauncher::new();
        let phase = phase_from_yaml(
            "  - id: a\n    prompt: x\n    model: openai-codex/gpt-5.6-sol\n    thinking: low\n",
        );
        launcher
            .validate_phase(&phase)
            .expect("a plain provider/id pattern with --thinking is unambiguous");
    }

    #[test]
    fn list_models_fails_loudly_when_pi_is_missing() {
        let error = list_models(Some(PathBuf::from("definitely-not-pi-xyz-8f3a")), None)
            .expect_err("a missing pi binary must be an explicit error");
        assert!(error.to_string().contains("list-models"), "{error}");
    }

    #[test]
    fn pi_bin_falls_back_to_the_platform_install() {
        // npm ships pi as a .cmd shim on Windows with no .exe, and
        // CreateProcess does not apply PATHEXT — the bare name never resolves.
        let expected = if cfg!(windows) { "pi.cmd" } else { "pi" };
        assert_eq!(resolve_pi_bin(None), PathBuf::from(expected));
    }

    #[test]
    fn edda_pi_bin_overrides_the_platform_default() {
        let custom = "/opt/pi/bin/pi-custom";
        assert_eq!(
            resolve_pi_bin(Some(OsString::from(custom))),
            PathBuf::from(custom)
        );
    }

    #[test]
    fn empty_edda_pi_bin_is_treated_as_unset() {
        let expected = if cfg!(windows) { "pi.cmd" } else { "pi" };
        assert_eq!(
            resolve_pi_bin(Some(OsString::new())),
            PathBuf::from(expected),
            "an empty override must not produce an unspawnable empty path"
        );
    }

    #[test]
    fn with_bin_overrides_the_default() {
        let custom = PathBuf::from("/opt/pi/bin/pi");
        assert_eq!(PiRpcLauncher::with_bin(custom.clone()).pi_bin, custom);
    }

    #[test]
    fn stderr_tail_is_empty_for_no_output() {
        assert_eq!(format_stderr_tail(""), "");
        assert_eq!(format_stderr_tail("\n   \n"), "");
    }

    #[test]
    fn stderr_tail_is_capped() {
        let raw = "x".repeat(2000);
        let tail = format_stderr_tail(&raw);
        assert_eq!(tail.chars().count(), 501, "500 chars plus the ellipsis");
        assert!(tail.ends_with('…'));
    }
}
