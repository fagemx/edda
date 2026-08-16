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
        command
            .arg("app-server")
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

    pub async fn open_thread(&mut self, cwd: &Path, resume: Option<&str>) -> Result<String> {
        let (method, params) = match resume {
            Some(thread_id) => (
                "thread/resume",
                json!({ "cwd": cwd.to_string_lossy(), "threadId": thread_id }),
            ),
            None => ("thread/start", json!({ "cwd": cwd.to_string_lossy() })),
        };
        let result = self.request(method, params).await?;
        thread_id_from_result(&result).map(str::to_owned)
    }

    pub async fn run_turn(&mut self, thread_id: &str, prompt: &str) -> Result<CodexTurnOutcome> {
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

        let mut response_seen = false;
        let mut pending_outcome = None;
        let mut turn = TurnAccumulator::new(thread_id);
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
                turn_id_from_result(&result?)?;
                response_seen = true;
                if let Some(outcome) = pending_outcome.take() {
                    guard.disarm();
                    return Ok(outcome);
                }
                continue;
            }

            if let Some(outcome) = turn.observe(&message)? {
                if response_seen {
                    guard.disarm();
                    return Ok(outcome);
                }
                pending_outcome = Some(outcome);
            }
        }
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
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
        }
    }

    async fn notify(&mut self, method: &str) -> Result<()> {
        let notification = json!({ "jsonrpc": "2.0", "method": method });
        let mut guard = KillOnCancel::new(&mut self.child);
        write_json_line(&mut self.stdin, &notification).await?;
        guard.disarm();
        Ok(())
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
    final_text: Option<String>,
}

impl<'a> TurnAccumulator<'a> {
    fn new(thread_id: &'a str) -> Self {
        Self {
            thread_id,
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
                self.final_text = Some(
                    params
                        .pointer("/item/text")
                        .and_then(Value::as_str)
                        .context("completed agent message has no text")?
                        .to_owned(),
                );
            }
            "turn/completed" => {
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
) -> Result<CodexTurnOutcome> {
    let mut turn = TurnAccumulator::new(thread_id);
    for line in lines {
        if let Some(outcome) = turn.observe(&parse_message(line)?)? {
            return Ok(outcome);
        }
    }
    bail!("unexpected EOF waiting for terminal Codex turn notification")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlates_response_ids_while_skipping_notifications() -> anyhow::Result<()> {
        let lines = [
            r#"{"method":"thread/started","params":{"thread":{"id":"t-1"}}}"#,
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
            r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"a-1","type":"agentMessage","text":"draft"}}}"#,
            r#"{"method":"item/completed","params":{"threadId":"t-1","turnId":"turn-1","item":{"id":"a-2","type":"agentMessage","text":"final answer"}}}"#,
            r#"{"method":"turn/completed","params":{"threadId":"t-1","turn":{"id":"turn-1","status":"completed","items":[]}}}"#,
        ];

        let outcome = turn_outcome_from_lines(lines.iter().copied(), "t-1")?;

        assert_eq!(outcome.thread_id, "t-1");
        assert_eq!(outcome.final_text.as_deref(), Some("final answer"));
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
