use std::path::Path;
use std::process::Stdio;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub struct CodexAppServer {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CodexTurnOutcome {
    pub thread_id: String,
    pub final_text: Option<String>,
}

impl CodexAppServer {
    pub async fn spawn(bin: &Path) -> Result<Self> {
        let mut command = Command::new(bin);
        command.arg("app-server");
        Self::spawn_with_command(command).await
    }

    async fn spawn_with_command(mut command: Command) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true);

        let mut child = command
            .spawn()
            .context("failed to spawn Codex App Server")?;
        let stdin = child
            .stdin
            .take()
            .context("Codex App Server stdin was not piped")?;
        let stdout = child
            .stdout
            .take()
            .context("Codex App Server stdout was not piped")?;
        let mut server = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
        };

        server
            .request(
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "edda",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                    "capabilities": {},
                }),
            )
            .await?;
        server.notify("initialized").await?;
        Ok(server)
    }

    /// Test-only wrapper so sibling-module tests (codex_rpc) can drive fake
    /// app-servers through the same spawn+initialize path as [`Self::spawn`].
    #[cfg(test)]
    pub(crate) async fn spawn_command(command: Command) -> Result<Self> {
        Self::spawn_with_command(command).await
    }

    pub async fn open_thread(&mut self, cwd: &Path, resume: Option<&str>) -> Result<String> {
        let (method, params) = match resume {
            Some(thread_id) => (
                "thread/resume",
                json!({ "cwd": cwd.to_string_lossy(), "threadId": thread_id }),
            ),
            None => ("thread/start", json!({ "cwd": cwd.to_string_lossy() })),
        };
        let result = self.request(method, params).await?;
        match thread_id_from_result(&result) {
            Ok(thread_id) => Ok(thread_id.to_owned()),
            Err(error) => {
                self.terminate().await;
                Err(error)
            }
        }
    }

    pub async fn run_turn(&mut self, thread_id: &str, prompt: &str) -> Result<CodexTurnOutcome> {
        let result = self.run_turn_inner(thread_id, prompt).await;
        if result.is_err() {
            self.terminate().await;
        }
        result
    }

    async fn run_turn_inner(&mut self, thread_id: &str, prompt: &str) -> Result<CodexTurnOutcome> {
        let id = self.take_id();
        let request = request_value(
            id,
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{ "type": "text", "text": prompt }],
            }),
        );
        let child = &mut self.child;
        let stdin = &mut self.stdin;
        let stdout = &mut self.stdout;
        let mut guard = KillOnCancel::new(child);
        write_json_line(stdin, &request).await?;

        let mut pending_notifications = Vec::new();
        let mut turn = None;
        loop {
            let Some(line) = stdout
                .next_line()
                .await
                .context("failed reading Codex App Server stdout")?
            else {
                return Err(guard.eof_error());
            };
            let message = parse_message(&line)?;

            if let Some(result) = matching_response(&message, id) {
                let result = result?;
                let turn_id = turn_id_from_result(&result)?;
                let mut accumulator = TurnAccumulator::new(thread_id, turn_id);
                for notification in pending_notifications.drain(..) {
                    if let Some(outcome) = accumulator.observe(&notification)? {
                        guard.disarm();
                        return Ok(outcome);
                    }
                }
                turn = Some(accumulator);
                continue;
            }

            reject_server_request(&message)?;
            if let Some(accumulator) = &mut turn {
                if let Some(outcome) = accumulator.observe(&message)? {
                    guard.disarm();
                    return Ok(outcome);
                }
            } else {
                pending_notifications.push(message);
            }
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let result = self.request_inner(method, params).await;
        if result.is_err() {
            self.terminate().await;
        }
        result
    }

    async fn request_inner(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.take_id();
        let request = request_value(id, method, params);
        let child = &mut self.child;
        let stdin = &mut self.stdin;
        let stdout = &mut self.stdout;
        let mut guard = KillOnCancel::new(child);
        write_json_line(stdin, &request).await?;

        loop {
            let Some(line) = stdout
                .next_line()
                .await
                .context("failed reading Codex App Server stdout")?
            else {
                return Err(guard.eof_error());
            };
            let message = parse_message(&line)?;
            if let Some(result) = matching_response(&message, id) {
                let result = result?;
                guard.disarm();
                return Ok(result);
            }
            reject_server_request(&message)?;
        }
    }

    async fn notify(&mut self, method: &str) -> Result<()> {
        let result = self.notify_inner(method).await;
        if result.is_err() {
            self.terminate().await;
        }
        result
    }

    async fn notify_inner(&mut self, method: &str) -> Result<()> {
        let notification = json!({ "jsonrpc": "2.0", "method": method });
        let mut guard = KillOnCancel::new(&mut self.child);
        write_json_line(&mut self.stdin, &notification).await?;
        guard.disarm();
        Ok(())
    }

    async fn terminate(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl Drop for CodexAppServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

struct KillOnCancel<'a> {
    child: &'a mut Child,
    armed: bool,
}

impl<'a> KillOnCancel<'a> {
    fn new(child: &'a mut Child) -> Self {
        Self { child, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }

    fn eof_error(&mut self) -> anyhow::Error {
        match self.child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                anyhow!("Codex App Server exited with non-zero status {status}")
            }
            Ok(Some(status)) => anyhow!("unexpected EOF from Codex App Server (status {status})"),
            Ok(None) => anyhow!("unexpected EOF from Codex App Server"),
            Err(error) => anyhow!("unexpected EOF from Codex App Server: {error}"),
        }
    }
}

impl Drop for KillOnCancel<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.child.start_kill();
        }
    }
}

async fn write_json_line(stdin: &mut ChildStdin, value: &Value) -> Result<()> {
    let mut line = serde_json::to_vec(value).context("failed to encode app-server request")?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .context("failed writing Codex App Server stdin")?;
    stdin
        .flush()
        .await
        .context("failed flushing Codex App Server stdin")
}

fn request_value(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

fn parse_message(line: &str) -> Result<Value> {
    serde_json::from_str(line).with_context(|| format!("invalid app-server JSON: {line}"))
}

fn matching_response(message: &Value, id: u64) -> Option<Result<Value>> {
    if message.get("method").is_some() || message.get("id").and_then(Value::as_u64) != Some(id) {
        return None;
    }
    if let Some(error) = message.get("error") {
        let code = error.get("code").map(Value::to_string).unwrap_or_default();
        let detail = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Some(Err(anyhow!("Codex App Server error {code}: {detail}")));
    }
    Some(
        message
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("app-server response {id} has no result or error")),
    )
}

fn reject_server_request(message: &Value) -> Result<()> {
    if message.get("id").is_some() {
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            bail!("unsupported Codex App Server request: {method}");
        }
    }
    Ok(())
}

fn thread_id_from_result(result: &Value) -> Result<&str> {
    result
        .pointer("/thread/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .context("app-server thread response has no thread id")
}

fn turn_id_from_result(result: &Value) -> Result<&str> {
    result
        .pointer("/turn/id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .context("app-server turn response has no turn id")
}

struct TurnAccumulator<'a> {
    thread_id: &'a str,
    turn_id: String,
    final_text: Option<String>,
}

impl<'a> TurnAccumulator<'a> {
    fn new(thread_id: &'a str, turn_id: &str) -> Self {
        Self {
            thread_id,
            turn_id: turn_id.to_owned(),
            final_text: None,
        }
    }

    fn observe(&mut self, message: &Value) -> Result<Option<CodexTurnOutcome>> {
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            return Ok(None);
        };
        let params = &message["params"];
        if params.get("threadId").and_then(Value::as_str) != Some(self.thread_id) {
            return Ok(None);
        }

        match method {
            "item/agentMessage/delta" => {
                if params.get("turnId").and_then(Value::as_str) != Some(self.turn_id.as_str()) {
                    return Ok(None);
                }
                let delta = params
                    .get("delta")
                    .and_then(Value::as_str)
                    .context("agent message delta has no text")?;
                self.final_text
                    .get_or_insert_with(String::new)
                    .push_str(delta);
            }
            "item/completed"
                if params.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage") =>
            {
                if params.get("turnId").and_then(Value::as_str) != Some(self.turn_id.as_str()) {
                    return Ok(None);
                }
                self.final_text = Some(
                    params
                        .pointer("/item/text")
                        .and_then(Value::as_str)
                        .context("completed agent message has no text")?
                        .to_owned(),
                );
            }
            "turn/completed" => {
                if params.pointer("/turn/id").and_then(Value::as_str) != Some(self.turn_id.as_str())
                {
                    return Ok(None);
                }
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .context("terminal turn notification has no status")?;
                if status != "completed" {
                    let detail = params
                        .pointer("/turn/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or(status);
                    bail!("Codex turn {status}: {detail}");
                }
                return Ok(Some(CodexTurnOutcome {
                    thread_id: self.thread_id.to_owned(),
                    final_text: self.final_text.clone(),
                }));
            }
            _ => {}
        }
        Ok(None)
    }
}

/// Scripted Codex App Server processes for tests, shared with the launcher
/// tests in `codex_rpc.rs` so every protocol-level fake lives in one place.
#[cfg(test)]
pub(crate) mod fake_support {
    use anyhow::Result;
    use tokio::process::Command;

    #[derive(Clone, Copy)]
    pub(crate) enum FakeScenario {
        MissingThreadId,
        EmptyThreadId,
        /// One thread/start (t-1) + one completed turn ("turn complete").
        RunTurnCompletes,
        RunTurnInterleaving,
        /// turn/start answers with a JSON-RPC error on request id 2, i.e. a
        /// client that calls run_turn directly without opening a thread first.
        RunTurnError,
        /// thread/start succeeds (t-1), then turn/start answers with a
        /// JSON-RPC error. Scripted for a client that opened the thread first,
        /// so the error lands on request id 3.
        RunTurnStartError,
        /// Two full turns: thread/start (t-1) + turn ("first answer"), then
        /// thread/resume (t-2) + turn ("second answer"). Drives the session
        /// continuity path end to end.
        TwoTurnsWithResume,
        Idle,
    }

    pub(crate) fn fake_app_server(scenario: FakeScenario) -> Result<(tempfile::TempDir, Command)> {
        let dir = tempfile::tempdir()?;

        #[cfg(windows)]
        {
            let script = dir.path().join("fake-app-server.ps1");
            std::fs::write(&script, powershell_fake_script(scenario))?;
            // Absolute where the platform offers a reliable one, so a doctored
            // PATH cannot choose the shell this harness runs. `%SystemRoot%` is
            // set on every Windows install; falling back to PATH keeps the
            // helper working if it is somehow absent (GH-482).
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
            // Feed the script to `sh` instead of exec'ing it, mirroring the
            // PowerShell branch above. Linux `execve` fails with ETXTBSY while
            // any process holds the target file open for writing, and in a
            // threaded test binary a concurrent spawn elsewhere can be sitting
            // between fork and exec holding an inherited write fd for this very
            // file -- O_CLOEXEC only clears it at exec, not at fork. `sh` opens
            // the script read-only, so that window cannot bite.
            let script = dir.path().join("fake-app-server.sh");
            std::fs::write(&script, shell_fake_script(scenario))?;
            // Absolute for the same reason as the Windows branch above. Note
            // POSIX does *not* fix this path -- the `sh` page's Application
            // Usage says so outright and points applications at `getconf PATH`,
            // and Solaris put the conformant shell under /usr/xpg4/bin. What
            // makes it safe here is narrower: every platform this harness runs
            // on has /bin/sh, so pinning it costs portability we do not need
            // and removes a lookup we do not control (GH-482).
            let mut command = Command::new("/bin/sh");
            command.arg(script);
            Ok((dir, command))
        }
    }

    #[cfg(windows)]
    fn powershell_fake_script(scenario: FakeScenario) -> String {
        let body = match scenario {
            FakeScenario::MissingThreadId => {
                "Read-Line\nWrite-Line '{\"id\":2,\"result\":{\"thread\":{}}}'\nStart-Sleep -Seconds 60"
            }
            FakeScenario::EmptyThreadId => {
                "Read-Line\nWrite-Line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"\"}}}'\nStart-Sleep -Seconds 60"
            }
            FakeScenario::RunTurnCompletes => {
                "Read-Line\nWrite-Line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nRead-Line\nWrite-Line '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}'\nWrite-Line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-1\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"turn complete\"}}}'\nWrite-Line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-1\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nStart-Sleep -Seconds 60"
            }
            FakeScenario::RunTurnInterleaving => {
                "Read-Line\nWrite-Line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nRead-Line\nWrite-Line '{\"id\":99,\"result\":{\"ignored\":true}}'\nWrite-Line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"other-thread\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"wrong thread\"}}}'\nWrite-Line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"other-thread\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nWrite-Line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-1\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"target final\"}}}'\nWrite-Line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-1\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nWrite-Line '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}'\nStart-Sleep -Seconds 60"
            }
            FakeScenario::RunTurnError => {
                "Read-Line\nWrite-Line '{\"id\":2,\"error\":{\"code\":-32602,\"message\":\"bad turn\"}}'\nStart-Sleep -Seconds 60"
            }
            FakeScenario::RunTurnStartError => {
                "Read-Line\nWrite-Line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nRead-Line\nWrite-Line '{\"id\":3,\"error\":{\"code\":-32602,\"message\":\"bad turn\"}}'\nStart-Sleep -Seconds 60"
            }
            FakeScenario::TwoTurnsWithResume => {
                "Read-Line\nWrite-Line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nRead-Line\nWrite-Line '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}'\nWrite-Line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-1\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"first answer\"}}}'\nWrite-Line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-1\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nRead-Line\nWrite-Line '{\"id\":4,\"result\":{\"thread\":{\"id\":\"t-2\"}}}'\nRead-Line\nWrite-Line '{\"id\":5,\"result\":{\"turn\":{\"id\":\"turn-2\"}}}'\nWrite-Line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-2\",\"turnId\":\"turn-2\",\"item\":{\"type\":\"agentMessage\",\"text\":\"second answer\"}}}'\nWrite-Line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-2\",\"turn\":{\"id\":\"turn-2\",\"status\":\"completed\"}}}'\nStart-Sleep -Seconds 60"
            }
            FakeScenario::Idle => "Start-Sleep -Seconds 60",
        };
        format!(
            "$ErrorActionPreference = 'Stop'\nfunction Read-Line {{ if ($null -eq [Console]::In.ReadLine()) {{ exit 0 }} }}\nfunction Write-Line([string]$line) {{ [Console]::Out.WriteLine($line); [Console]::Out.Flush() }}\nRead-Line\nWrite-Line '{{\"id\":1,\"result\":{{}}}}'\nRead-Line\n{body}\n"
        )
    }

    #[cfg(unix)]
    fn shell_fake_script(scenario: FakeScenario) -> String {
        let body = match scenario {
            FakeScenario::MissingThreadId => {
                "read_line\nwrite_line '{\"id\":2,\"result\":{\"thread\":{}}}'\nsleep 60"
            }
            FakeScenario::EmptyThreadId => {
                "read_line\nwrite_line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"\"}}}'\nsleep 60"
            }
            FakeScenario::RunTurnCompletes => {
                "read_line\nwrite_line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nread_line\nwrite_line '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}'\nwrite_line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-1\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"turn complete\"}}}'\nwrite_line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-1\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nsleep 60"
            }
            FakeScenario::RunTurnInterleaving => {
                "read_line\nwrite_line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nread_line\nwrite_line '{\"id\":99,\"result\":{\"ignored\":true}}'\nwrite_line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"other-thread\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"wrong thread\"}}}'\nwrite_line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"other-thread\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nwrite_line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-1\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"target final\"}}}'\nwrite_line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-1\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nwrite_line '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}'\nsleep 60"
            }
            FakeScenario::RunTurnError => {
                "read_line\nwrite_line '{\"id\":2,\"error\":{\"code\":-32602,\"message\":\"bad turn\"}}'\nsleep 60"
            }
            FakeScenario::RunTurnStartError => {
                "read_line\nwrite_line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nread_line\nwrite_line '{\"id\":3,\"error\":{\"code\":-32602,\"message\":\"bad turn\"}}'\nsleep 60"
            }
            FakeScenario::TwoTurnsWithResume => {
                "read_line\nwrite_line '{\"id\":2,\"result\":{\"thread\":{\"id\":\"t-1\"}}}'\nread_line\nwrite_line '{\"id\":3,\"result\":{\"turn\":{\"id\":\"turn-1\"}}}'\nwrite_line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-1\",\"turnId\":\"turn-1\",\"item\":{\"type\":\"agentMessage\",\"text\":\"first answer\"}}}'\nwrite_line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-1\",\"turn\":{\"id\":\"turn-1\",\"status\":\"completed\"}}}'\nread_line\nwrite_line '{\"id\":4,\"result\":{\"thread\":{\"id\":\"t-2\"}}}'\nread_line\nwrite_line '{\"id\":5,\"result\":{\"turn\":{\"id\":\"turn-2\"}}}'\nwrite_line '{\"method\":\"item/completed\",\"params\":{\"threadId\":\"t-2\",\"turnId\":\"turn-2\",\"item\":{\"type\":\"agentMessage\",\"text\":\"second answer\"}}}'\nwrite_line '{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"t-2\",\"turn\":{\"id\":\"turn-2\",\"status\":\"completed\"}}}'\nsleep 60"
            }
            FakeScenario::Idle => "sleep 60",
        };
        format!(
            "#!/bin/sh\nread_line() {{ IFS= read -r _ || exit 0; }}\nwrite_line() {{ printf '%s\\n' \"$1\"; }}\nread_line\nwrite_line '{{\"id\":1,\"result\":{{}}}}'\nread_line\n{body}\n"
        )
    }
}

#[cfg(test)]
fn response_for_id<'a>(lines: impl IntoIterator<Item = &'a str>, id: u64) -> Result<Value> {
    for line in lines {
        let message = parse_message(line)?;
        if let Some(result) = matching_response(&message, id) {
            return result;
        }
    }
    bail!("unexpected EOF waiting for app-server response {id}")
}

#[cfg(test)]
fn turn_outcome_from_lines<'a>(
    lines: impl IntoIterator<Item = &'a str>,
    thread_id: &str,
    turn_id: &str,
) -> Result<CodexTurnOutcome> {
    let mut turn = TurnAccumulator::new(thread_id, turn_id);
    for line in lines {
        if let Some(outcome) = turn.observe(&parse_message(line)?)? {
            return Ok(outcome);
        }
    }
    bail!("unexpected EOF waiting for terminal Codex turn notification")
}

#[cfg(test)]
mod tests {
    use super::fake_support::{fake_app_server, FakeScenario};
    use super::*;

    async fn spawn_fake_app_server(
        scenario: FakeScenario,
    ) -> anyhow::Result<(tempfile::TempDir, CodexAppServer)> {
        let (dir, command) = fake_app_server(scenario)?;
        let server = CodexAppServer::spawn_command(command).await?;
        Ok((dir, server))
    }

    async fn wait_for_process_stop(pid: u32) -> anyhow::Result<()> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async move {
            loop {
                if !process_is_alive(pid)? {
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .context("fake app-server PID kept running")?
    }

    async fn wait_for_child_exit(child: &mut Child) -> anyhow::Result<()> {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if child.try_wait()?.is_some() {
                    return Ok(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
        })
        .await
        .context("fake app-server child did not exit")?
    }

    #[cfg(windows)]
    fn tasklist_reports_alive(output: &std::process::Output, pid: u32) -> anyhow::Result<bool> {
        anyhow::ensure!(
            output.status.success(),
            "tasklist.exe exited with {}",
            output.status
        );
        Ok(String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\"")))
    }

    /// Reports whether `pid` is still a *running* process.
    ///
    /// A killed child stays in the process table as a zombie until its parent
    /// reaps it, and dropping the client cannot reap: it hands the `Child` to
    /// tokio's global orphan queue, drained later off SIGCHLD. So "the PID is
    /// absent" is not something drop can guarantee -- "the process no longer
    /// runs" is. A zombie has already terminated, so it does not count as
    /// alive; a child that drop failed to kill still does.
    ///
    /// The unix probe reads a non-zero `ps` exit as "gone". That is right when
    /// `ps` ran and found nothing, and wrong when `ps` itself rejected the
    /// arguments -- BusyBox does not accept `-o state= -p`, and the two cases
    /// are indistinguishable from the exit status alone. On such a system a
    /// caller waiting for exit would pass without the process having gone
    /// anywhere.
    ///
    /// The `kill -0` probe this replaced shared the *class* of flaw and was in
    /// one way worse -- it swallowed spawn failure too. But on BusyBox, the
    /// trigger named above, the two do not match: BusyBox `kill` parses numeric
    /// signals, so `kill -0` worked there, while its `ps` rejects these
    /// arguments twice over (the state column is `stat`, and an unknown `-o`
    /// column is fatal; `-p` is not in its base option set). So the switch to
    /// `ps` made this case reachable on exactly the platform class the
    /// deferral cites, rather than inheriting it.
    ///
    /// It is recorded rather than fixed because `ci.yml` runs only the three
    /// GitHub-hosted images, none of them musl or Alpine and none in a
    /// container -- so no job can reach it today. Distinguishing the two
    /// (treating a non-zero exit that also wrote to stderr as a probe failure)
    /// is worth doing the day such a job is added, and this note is what should
    /// make that obvious then (GH-482).
    fn process_is_alive(pid: u32) -> anyhow::Result<bool> {
        #[cfg(windows)]
        {
            // Windows drops terminated processes from the table outright, so
            // being listed at all already means still running.
            let output = std::process::Command::new("tasklist.exe")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output()
                .context("failed to run tasklist.exe")?;
            tasklist_reports_alive(&output, pid)
        }
        #[cfg(unix)]
        {
            // `kill -0` succeeds for zombies, so ask for the state instead: `ps`
            // exits non-zero once the PID is gone, and reports `Z` (Linux) or
            // `Z+` (macOS) while it is a terminated-but-unreaped zombie.
            let output = std::process::Command::new("ps")
                .args(["-o", "state=", "-p", &pid.to_string()])
                .output()
                .context("failed to run ps")?;
            if !output.status.success() {
                return Ok(false);
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let state = stdout.trim();
            Ok(!state.is_empty() && !state.starts_with('Z'))
        }
    }

    #[cfg(windows)]
    #[test]
    fn rejects_failed_tasklist_probe() {
        use std::os::windows::process::ExitStatusExt;

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: br#""fake.exe","123","Console","1","1 K""#.to_vec(),
            stderr: b"probe failed".to_vec(),
        };

        let error = tasklist_reports_alive(&output, 123)
            .expect_err("failed tasklist probe should not report a process state");

        assert!(error.to_string().contains("tasklist.exe exited"));
    }

    #[test]
    fn correlates_response_ids_while_skipping_notifications() -> anyhow::Result<()> {
        let lines = [
            r#"{"method":"thread/started","params":{"thread":{"id":"t-1"}}}"#,
            r#"{"id":1,"result":{"ignored":true}}"#,
            r#"{"id":2,"result":{"thread":{"id":"t-1"}}}"#,
        ];

        let result = response_for_id(lines.iter().copied(), 2)?;

        assert_eq!(result["thread"]["id"], "t-1");
        Ok(())
    }

    #[test]
    fn rejects_malformed_app_server_json() {
        let error = response_for_id(["not-json"], 2).expect_err("malformed JSON should fail");

        assert!(error.to_string().contains("invalid app-server JSON"));
    }

    #[test]
    fn preserves_json_rpc_error_message() {
        let error = response_for_id(
            [r#"{"id":2,"error":{"code":-32602,"message":"bad params"}}"#],
            2,
        )
        .expect_err("JSON-RPC error should fail");

        assert!(error.to_string().contains("bad params"));
    }

    #[test]
    fn reports_unexpected_eof() {
        let error = response_for_id([], 2).expect_err("EOF should fail");
        let turn_error = turn_outcome_from_lines([], "t-1", "turn-1")
            .expect_err("terminal turn EOF should fail");

        assert!(error.to_string().contains("unexpected EOF"));
        assert!(turn_error.to_string().contains("unexpected EOF"));
    }

    #[test]
    fn rejects_permission_requests_without_approving() -> anyhow::Result<()> {
        let message = parse_message(
            r#"{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{}}"#,
        )?;

        let error = reject_server_request(&message).expect_err("permission request should fail");

        assert!(error.to_string().contains("requestApproval"));
        Ok(())
    }

    #[test]
    fn extracts_started_thread_id() -> anyhow::Result<()> {
        let request = request_value(2, "thread/start", serde_json::json!({ "cwd": "C:/repo" }));
        let result = response_for_id([r#"{"id":2,"result":{"thread":{"id":"t-new"}}}"#], 2)?;

        assert_eq!(request["method"], "thread/start");
        assert_eq!(request["params"]["cwd"], "C:/repo");
        assert_eq!(thread_id_from_result(&result)?, "t-new");
        Ok(())
    }

    #[test]
    fn extracts_resumed_thread_id() -> anyhow::Result<()> {
        let request = request_value(
            3,
            "thread/resume",
            serde_json::json!({ "cwd": "C:/repo", "threadId": "t-existing" }),
        );
        let result = response_for_id([r#"{"id":3,"result":{"thread":{"id":"t-existing"}}}"#], 3)?;

        assert_eq!(request["method"], "thread/resume");
        assert_eq!(request["params"]["threadId"], "t-existing");
        assert_eq!(thread_id_from_result(&result)?, "t-existing");
        Ok(())
    }

    #[test]
    fn turn_start_uses_text_input() {
        let request = request_value(
            4,
            "turn/start",
            serde_json::json!({
                "threadId": "t-1",
                "input": [{ "type": "text", "text": "do the task" }],
            }),
        );

        assert_eq!(request["method"], "turn/start");
        assert_eq!(request["params"]["threadId"], "t-1");
        assert_eq!(request["params"]["input"][0]["text"], "do the task");
    }

    #[test]
    fn terminal_turn_returns_last_assistant_text() -> anyhow::Result<()> {
        let lines = [
            r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"other-turn","item":{"id":"other","type":"agentMessage","text":"wrong answer"}}}"#,
            r#"{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"other-turn","status":"completed","items":[]}}}"#,
            r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"a-1","type":"agentMessage","text":"draft"}}}"#,
            r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"a-2","type":"agentMessage","text":"final answer"}}}"#,
            r#"{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"turn-1","status":"completed","items":[]}}}"#,
        ];

        let outcome = turn_outcome_from_lines(lines.iter().copied(), "t-1", "turn-1")?;

        assert_eq!(outcome.thread_id, "t-1");
        assert_eq!(outcome.final_text.as_deref(), Some("final answer"));
        Ok(())
    }

    #[tokio::test]
    async fn start_missing_thread_id_terminates_and_reaps_child() -> anyhow::Result<()> {
        let (_fake, mut server) = spawn_fake_app_server(FakeScenario::MissingThreadId).await?;
        let pid = server.child.id().context("fake app-server has no PID")?;

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            server.open_thread(Path::new("."), None),
        )
        .await
        .context("thread/start did not finish")?
        .expect_err("missing thread id should fail");

        assert!(error.to_string().contains("no thread id"));
        assert!(server.child.try_wait()?.is_some(), "child was not reaped");
        wait_for_process_stop(pid).await?;
        Ok(())
    }

    #[tokio::test]
    async fn resume_empty_thread_id_terminates_and_reaps_child() -> anyhow::Result<()> {
        let (_fake, mut server) = spawn_fake_app_server(FakeScenario::EmptyThreadId).await?;
        let pid = server.child.id().context("fake app-server has no PID")?;

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            server.open_thread(Path::new("."), Some("persisted-thread")),
        )
        .await
        .context("thread/resume did not finish")?
        .expect_err("empty thread id should fail");

        assert!(error.to_string().contains("no thread id"));
        assert!(server.child.try_wait()?.is_some(), "child was not reaped");
        wait_for_process_stop(pid).await?;
        Ok(())
    }

    #[tokio::test]
    async fn run_turn_buffers_target_notifications_until_matching_response() -> anyhow::Result<()> {
        let (_fake, mut server) = spawn_fake_app_server(FakeScenario::RunTurnInterleaving).await?;
        let thread_id = server.open_thread(Path::new("."), None).await?;

        let outcome = server.run_turn(&thread_id, "do the task").await?;

        assert_eq!(outcome.thread_id, "t-1");
        assert_eq!(outcome.final_text.as_deref(), Some("target final"));
        Ok(())
    }

    #[tokio::test]
    async fn run_turn_error_terminates_and_reaps_child() -> anyhow::Result<()> {
        let (_fake, mut server) = spawn_fake_app_server(FakeScenario::RunTurnError).await?;
        let pid = server.child.id().context("fake app-server has no PID")?;

        let error = server
            .run_turn("t-1", "bad turn")
            .await
            .expect_err("JSON-RPC error should fail");

        assert!(error.to_string().contains("bad turn"));
        assert!(server.child.try_wait()?.is_some(), "child was not reaped");
        wait_for_process_stop(pid).await?;
        Ok(())
    }

    #[tokio::test]
    async fn dropping_concrete_client_stops_the_child_process() -> anyhow::Result<()> {
        let (_fake, server) = spawn_fake_app_server(FakeScenario::Idle).await?;
        let pid = server.child.id().context("fake app-server has no PID")?;

        drop(server);

        wait_for_process_stop(pid).await
    }

    #[tokio::test]
    async fn cancelling_concrete_method_stops_the_child_process() -> anyhow::Result<()> {
        let (_fake, mut server) = spawn_fake_app_server(FakeScenario::Idle).await?;
        let pid = server.child.id().context("fake app-server has no PID")?;

        let timed_out = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            server.run_turn("t-1", "wait forever"),
        )
        .await;

        assert!(timed_out.is_err(), "fake method unexpectedly completed");
        wait_for_child_exit(&mut server.child).await?;
        wait_for_process_stop(pid).await?;
        Ok(())
    }

    #[test]
    fn cancellation_child() {
        if std::env::var_os("EDDA_CODEX_CANCELLATION_CHILD").is_some() {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }

    #[tokio::test]
    async fn cancellation_kills_child() -> anyhow::Result<()> {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg("agent::codex_app_server::tests::cancellation_child")
            .arg("--exact")
            .env("EDDA_CODEX_CANCELLATION_CHILD", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn()?;

        drop(KillOnCancel::new(&mut child));

        let status = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .context("cancelled child did not exit")??;
        assert!(!status.success());
        Ok(())
    }

    #[tokio::test]
    async fn reports_non_zero_child_exit() -> anyhow::Result<()> {
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("cmd.exe");
            command.args(["/C", "exit", "7"]);
            command
        };
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "exit 7"]);
            command
        };
        let mut child = command.spawn()?;
        child.wait().await?;
        let mut guard = KillOnCancel::new(&mut child);

        let error = guard.eof_error();
        guard.disarm();

        assert!(error.to_string().contains("non-zero status"));
        Ok(())
    }

    #[tokio::test]
    #[ignore = "requires an authenticated Codex installation"]
    async fn live_start_resume_drill() -> anyhow::Result<()> {
        let bin = std::env::var_os("CODEX_BIN").context("CODEX_BIN is not set")?;
        let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut first = CodexAppServer::spawn(Path::new(&bin)).await?;
        let thread_id = first.open_thread(cwd, None).await?;
        let first_outcome = first
            .run_turn(&thread_id, "Reply with exactly EDDA_APP_SERVER_ONE")
            .await?;
        assert_eq!(
            first_outcome.final_text.as_deref(),
            Some("EDDA_APP_SERVER_ONE")
        );
        drop(first);

        let mut resumed = CodexAppServer::spawn(Path::new(&bin)).await?;
        assert_eq!(resumed.open_thread(cwd, Some(&thread_id)).await?, thread_id);
        let second_outcome = resumed
            .run_turn(&thread_id, "Reply with exactly EDDA_APP_SERVER_TWO")
            .await?;
        assert_eq!(
            second_outcome.final_text.as_deref(),
            Some("EDDA_APP_SERVER_TWO")
        );
        Ok(())
    }
}
