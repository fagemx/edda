//! `edda-serve` node endpoint tests (SPEC §8.10, §8.12).
//!
//! These tests bind real in-process axum servers on `127.0.0.1` with the
//! test-only insecure bind, use real HTTP over `std::net::TcpStream`, and keep
//! the shared `EDDA_STORE_ROOT` / `EDDA_RETURN_ROOT` redirect under the same
//! process lock `tests.rs` uses so parallel tests never share a store root.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::Router;
use edda_ledger::node::{
    self, LaneRequest, NodeConfig, NodeEvent, NodeEventBody, NodeSection, OutboundQueue,
    OwnerReturn, PeerConfig, QueueEntry,
};

use crate::state::AppState;
use crate::tests::STORE_LOCK;

// ── harness ───────────────────────────────────────────────────────────

struct EnvOverride {
    store: Option<std::ffi::OsString>,
    returns: Option<std::ffi::OsString>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvOverride {
    fn install(store_root: &Path, return_root: &Path) -> Self {
        let lock = STORE_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        let store = std::env::var_os("EDDA_STORE_ROOT");
        let returns = std::env::var_os("EDDA_RETURN_ROOT");
        std::env::set_var("EDDA_STORE_ROOT", store_root);
        std::env::set_var("EDDA_RETURN_ROOT", return_root);
        Self {
            store,
            returns,
            _lock: lock,
        }
    }
}

impl Drop for EnvOverride {
    fn drop(&mut self) {
        match self.store.take() {
            Some(value) => std::env::set_var("EDDA_STORE_ROOT", value),
            None => std::env::remove_var("EDDA_STORE_ROOT"),
        }
        match self.returns.take() {
            Some(value) => std::env::set_var("EDDA_RETURN_ROOT", value),
            None => std::env::remove_var("EDDA_RETURN_ROOT"),
        }
    }
}

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp workspace");
    let paths = edda_ledger::EddaPaths::discover(dir.path());
    paths.ensure_layout().expect("layout");
    edda_ledger::ledger::init_workspace(&paths).expect("init workspace");
    edda_ledger::ledger::init_head(&paths, "main").expect("init head");
    edda_ledger::ledger::init_branches_json(&paths, "main").expect("init branches");
    dir
}

fn node_config(alias: &str, bind: &str, port: u16, peers: Vec<PeerConfig>) -> NodeConfig {
    NodeConfig {
        version: 1,
        node: NodeSection {
            alias: alias.into(),
            bind: bind.into(),
            port,
        },
        peers,
    }
}

fn peer(alias: &str, host: &str, port: u16, token: &str) -> PeerConfig {
    PeerConfig {
        alias: alias.into(),
        host: host.into(),
        port,
        token_env: None,
        token: Some(token.into()),
    }
}

fn node_router(repo_root: &Path, config: NodeConfig, token: &str) -> Router {
    let state = Arc::new(AppState {
        repo_root: repo_root.to_path_buf(),
        chronicle: None,
        pending_pairings: Mutex::new(std::collections::HashMap::new()),
        node: Some(config),
        node_token: Some(token.to_string()),
    });
    crate::api::node::routes().with_state(state)
}

/// Spawn an axum server on its own OS thread with its own current-thread
/// runtime, so the test thread can use blocking sockets without starving it.
fn spawn_server(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    listener.set_nonblocking(true).expect("nonblocking");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test server runtime");
        runtime.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
            let _ = axum::serve(listener, router.into_make_service()).await;
        });
    });
    addr
}

fn http_post(
    addr: SocketAddr,
    path: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> (u16, String) {
    try_http_post(addr, path, token, body).expect("connect test server")
}

fn try_http_post(
    addr: SocketAddr,
    path: &str,
    token: Option<&str>,
    body: &serde_json::Value,
) -> std::io::Result<(u16, String)> {
    let body = serde_json::to_string(body).expect("serialize body");
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(&body);

    let mut stream = TcpStream::connect(addr)?;
    stream.write_all(request.as_bytes())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;
    Ok(parse_http(&raw))
}

fn http_get(addr: SocketAddr, path: &str, token: Option<&str>) -> (u16, String) {
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    request.push_str("\r\n");
    let mut stream = TcpStream::connect(addr).expect("connect test server");
    stream.write_all(request.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read response");
    parse_http(&raw)
}

fn parse_http(raw: &[u8]) -> (u16, String) {
    let text = String::from_utf8_lossy(raw).into_owned();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let response_body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body.to_string())
        .unwrap_or_default();
    (status, response_body)
}

fn owner_return_event(machine: &str, work: &str) -> NodeEvent {
    let message = format!("report for {work}");
    let logical_id = node::owner_return_logical_id(
        "assistant/p",
        work,
        "done",
        Some("ok"),
        Some("out.md"),
        Some(&message),
    );
    NodeEvent::new(NodeEventBody::OwnerReturn(OwnerReturn {
        logical_id,
        message_id: edda_core::hash::sha256_hex(work.as_bytes()),
        owner: "assistant/p".into(),
        work: work.into(),
        status: "done".into(),
        result: Some("ok".into()),
        deliverable: Some("out.md".into()),
        message: Some(message),
        posted_at: "2026-09-14T00:00:00.000Z".into(),
        origin_machine: machine.into(),
    }))
}

fn sync_body(origin: &str, events: &[NodeEvent]) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "kind": "edda.node.sync",
        "originMachine": origin,
        "events": events.iter().map(NodeEvent::to_wire_value).collect::<Vec<_>>(),
    })
}

fn entry_to_wire(entry: &QueueEntry) -> serde_json::Value {
    let mut object = entry.payload.as_object().cloned().unwrap_or_default();
    object.insert("eventId".into(), serde_json::json!(entry.event_id));
    object.insert("kind".into(), serde_json::json!(entry.kind));
    serde_json::Value::Object(object)
}

/// Mirrors `edda node start`'s flush: send this peer's `sent` entries, mark
/// delivered on 2xx, keep them `sent` (with an attempt recorded) otherwise.
fn flush(addr: SocketAddr, token: Option<&str>, origin: &str) -> anyhow::Result<usize> {
    let queue = OutboundQueue::open("beta")?;
    let pending = queue.pending()?;
    if pending.is_empty() {
        return Ok(0);
    }
    let events: Vec<serde_json::Value> = pending.iter().map(entry_to_wire).collect();
    let body = serde_json::json!({
        "version": 1,
        "kind": "edda.node.sync",
        "originMachine": origin,
        "events": events,
    });
    let ids: Vec<String> = pending.iter().map(|entry| entry.event_id.clone()).collect();
    let response = try_http_post(addr, "/api/sync", token, &body);
    match response {
        Ok((status, _)) if (200..300).contains(&status) => {
            queue.mark_delivered(&ids)?;
            Ok(pending.len())
        }
        Ok((status, body)) => {
            queue.mark_attempt(&ids)?;
            anyhow::bail!("sync returned {status}: {body}")
        }
        Err(error) => {
            queue.mark_attempt(&ids)?;
            anyhow::bail!("sync send failed: {error}")
        }
    }
}

fn mailbox_ids(repo_root: &Path) -> BTreeSet<String> {
    let dir = repo_root.join(".edda").join("returns").join("messages");
    let mut ids = BTreeSet::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return ids;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json") {
            ids.insert(
                entry
                    .path()
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or_default()
                    .to_string(),
            );
        }
    }
    ids
}

// ── §8.10 token guard ─────────────────────────────────────────────────

#[test]
fn sync_without_token_is_401_and_writes_nothing() {
    let store = tempfile::tempdir().unwrap();
    let repo = workspace();
    let _env = EnvOverride::install(store.path(), repo.path());
    let config = node_config("beta", "127.0.0.1", 6850, vec![]);
    let addr = spawn_server(node_router(repo.path(), config, "shared-token"));

    let events = vec![owner_return_event("alpha", "job-a")];
    let (status, _body) = http_post(addr, "/api/sync", None, &sync_body("alpha", &events));
    assert_eq!(status, 401, "a tokenless request must be unauthorized");
    assert!(
        mailbox_ids(repo.path()).is_empty(),
        "a 401 must write no message"
    );
    assert!(
        !store.path().join("node").join("imported.jsonl").exists(),
        "a 401 must record nothing as imported"
    );
}

#[test]
fn sync_with_wrong_token_is_401() {
    let store = tempfile::tempdir().unwrap();
    let repo = workspace();
    let _env = EnvOverride::install(store.path(), repo.path());
    let config = node_config("beta", "127.0.0.1", 6850, vec![]);
    let addr = spawn_server(node_router(repo.path(), config, "shared-token"));

    let events = vec![owner_return_event("alpha", "job-a")];
    let (status, _body) = http_post(
        addr,
        "/api/sync",
        Some("wrong-token"),
        &sync_body("alpha", &events),
    );
    assert_eq!(status, 401, "a wrong token must be unauthorized");
    assert!(mailbox_ids(repo.path()).is_empty());

    // The correct token is accepted and writes the message.
    let (status, body) = http_post(
        addr,
        "/api/sync",
        Some("shared-token"),
        &sync_body("alpha", &events),
    );
    assert_eq!(status, 200, "the correct token is accepted: {body}");
    assert_eq!(mailbox_ids(repo.path()).len(), 1);
}

// ── §8.12 local two-node contract test ────────────────────────────────

#[test]
fn two_nodes_receipt_progression_and_idempotent_reconnect_flush() {
    let store = tempfile::tempdir().unwrap();
    let _repo_a = workspace();
    let repo_b = workspace();
    // Both nodes share the process store root; node B is the receiver, so the
    // mailbox root is B's workspace.
    let _env = EnvOverride::install(store.path(), repo_b.path());

    // Node B first, so the sender has a real address to reach.
    let config_b = node_config("beta", "127.0.0.1", 6851, vec![]);
    let addr_b = spawn_server(node_router(repo_b.path(), config_b, "shared-token"));

    let dead_port = {
        // Bind and immediately drop a port so nothing is listening on it.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let dead_addr: SocketAddr = format!("127.0.0.1:{dead_port}").parse().unwrap();
    let _config_a = node_config(
        "alpha",
        "127.0.0.1",
        6850,
        vec![peer("beta", "127.0.0.1", addr_b.port(), "shared-token")],
    );

    let events = vec![
        owner_return_event("alpha", "job-1"),
        owner_return_event("alpha", "job-2"),
        owner_return_event("alpha", "job-3"),
    ];
    let ids: BTreeSet<String> = events.iter().map(|event| event.event_id.clone()).collect();

    let queue = OutboundQueue::open("beta").expect("open queue");
    for event in &events {
        queue.enqueue(event).expect("enqueue");
    }
    let status = queue.status().unwrap();
    assert_eq!(status.sent, 3, "sent: queued durably");
    assert_eq!(status.pending, 3);

    // Simulated peer outage: the flush fails and nothing is lost or deleted.
    let outage = flush(dead_addr, Some("shared-token"), "alpha");
    assert!(outage.is_err(), "a dead peer must fail the flush");
    let status = queue.status().unwrap();
    assert_eq!(status.pending, 3, "an outage loses nothing");
    assert_eq!(status.delivered, 0);
    assert!(mailbox_ids(repo_b.path()).is_empty());

    // Reconnect flush: the peer returns, every event is delivered once.
    let delivered = flush(addr_b, Some("shared-token"), "alpha").expect("reconnect flush");
    assert_eq!(delivered, 3);
    let status = queue.status().unwrap();
    assert_eq!(status.delivered, 3, "delivered: the peer returned 2xx");
    assert_eq!(status.pending, 0);
    let applied_once = mailbox_ids(repo_b.path());
    let expected_messages: BTreeSet<String> = ["job-1", "job-2", "job-3"]
        .iter()
        .map(|work| edda_core::hash::sha256_hex(work.as_bytes()))
        .collect();
    assert_eq!(
        applied_once, expected_messages,
        "every event applied exactly once"
    );

    // A crash between send and mark causes a resend; the peer dedupes it.
    let (status_code, body) = http_post(
        addr_b,
        "/api/sync",
        Some("shared-token"),
        &sync_body("alpha", &events),
    );
    assert_eq!(status_code, 200);
    let response: serde_json::Value = serde_json::from_str(&body).expect("json response");
    assert_eq!(response["accepted"].as_array().unwrap().len(), 0);
    assert_eq!(response["duplicates"].as_array().unwrap().len(), 3);
    assert_eq!(
        mailbox_ids(repo_b.path()),
        applied_once,
        "a resend applies nothing twice"
    );

    // acked: the receiving side recorded consumption.
    let all: Vec<String> = ids.iter().cloned().collect();
    queue.mark_acked(&all).expect("ack");
    let status = queue.status().unwrap();
    assert_eq!(status.acked, 3, "acked: the receiving side consumed them");
    assert_eq!(status.delivered, 0);
}

// ── status endpoint ───────────────────────────────────────────────────

#[test]
fn node_status_reports_queue_and_unobserved_peer_reason() {
    let store = tempfile::tempdir().unwrap();
    let repo = workspace();
    let _env = EnvOverride::install(store.path(), repo.path());
    let config = node_config(
        "alpha",
        "127.0.0.1",
        6850,
        vec![peer("beta", "127.0.0.1", 1, "shared-token")],
    );
    let addr = spawn_server(node_router(repo.path(), config, "shared-token"));

    // Status requires the token too.
    let (status, _) = http_get(addr, "/api/node/status", None);
    assert_eq!(status, 401);

    let (status, body) = http_get(addr, "/api/node/status", Some("shared-token"));
    assert_eq!(status, 200, "status body: {body}");
    let response: serde_json::Value = serde_json::from_str(&body).expect("json status");
    assert_eq!(response["machine"], "alpha");
    assert_eq!(response["bind"], "127.0.0.1");
    assert_eq!(response["port"], 6850);
    // An unobserved peer is never a silent zero: explicit false plus a reason.
    assert_eq!(response["peers"][0]["alias"], "beta");
    assert_eq!(response["peers"][0]["reachable"], false);
    assert!(!response["peers"][0]["reason"].as_str().unwrap().is_empty());
    assert_eq!(response["queue"][0]["peer"], "beta");
    assert_eq!(response["queue"][0]["pending"], 0);
}

#[test]
fn sync_rejects_unknown_request_field() {
    let store = tempfile::tempdir().unwrap();
    let repo = workspace();
    let _env = EnvOverride::install(store.path(), repo.path());
    let config = node_config("beta", "127.0.0.1", 6850, vec![]);
    let addr = spawn_server(node_router(repo.path(), config, "shared-token"));

    let mut body = sync_body("alpha", &[owner_return_event("alpha", "job-a")]);
    body["sessionId"] = serde_json::json!("leak");
    let (status, _body) = http_post(addr, "/api/sync", Some("shared-token"), &body);
    assert_eq!(status, 400, "an unknown request field fails closed");
    assert!(mailbox_ids(repo.path()).is_empty());
}

#[test]
fn sync_refuses_unknown_event_field_without_partial_application() {
    let store = tempfile::tempdir().unwrap();
    let repo = workspace();
    let _env = EnvOverride::install(store.path(), repo.path());
    let config = node_config("beta", "127.0.0.1", 6850, vec![]);
    let addr = spawn_server(node_router(repo.path(), config, "shared-token"));

    let mut wire = owner_return_event("alpha", "job-a").to_wire_value();
    wire["path"] = serde_json::json!("/etc/passwd");
    let body = serde_json::json!({
        "version": 1,
        "kind": "edda.node.sync",
        "originMachine": "alpha",
        "events": [wire],
    });
    let (status, response) = http_post(addr, "/api/sync", Some("shared-token"), &body);
    assert_eq!(
        status, 200,
        "a per-event refusal is a 200 with a refused list"
    );
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["refused"][0]["index"], 0);
    assert_eq!(response["refused"][0]["reason"], "forbidden field `path`");
    assert!(
        mailbox_ids(repo.path()).is_empty(),
        "nothing partially applied"
    );
}

// ── GH-685 lane-request landing ────────────────────────────────────────

fn lane_request_event(machine: &str, request_id: &str, to_label: &str) -> NodeEvent {
    NodeEvent::new(NodeEventBody::LaneRequest(LaneRequest {
        request_id: request_id.into(),
        from_label: "alpha".into(),
        to_label: to_label.into(),
        message: "cross-machine please".into(),
        ts: "2026-09-16T00:00:00Z".into(),
        origin_machine: machine.into(),
    }))
}

fn coordination_lines(repo_root: &Path) -> Vec<serde_json::Value> {
    let project_id = edda_store::project_id(repo_root);
    let path = edda_store::project_dir(&project_id)
        .join("state")
        .join("coordination.jsonl");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("coordination line"))
        .collect()
}

/// §6.7 — the receiving node lands a `lane_request` by writing a local
/// `request` event with the wire id/labels and a `request_delivered` marker; no
/// session id crosses the wire.
#[test]
fn lane_request_landing_writes_request_then_request_delivered_and_no_session_id() {
    let store = tempfile::tempdir().unwrap();
    let repo = workspace();
    let _env = EnvOverride::install(store.path(), repo.path());
    let config = node_config("beta", "127.0.0.1", 6850, vec![]);
    let addr = spawn_server(node_router(repo.path(), config, "shared-token"));

    let event = lane_request_event("alpha", "req-lane-1", "beta-role");
    let event_id = event.event_id.clone();
    let (status, body) = http_post(
        addr,
        "/api/sync",
        Some("shared-token"),
        &sync_body("alpha", std::slice::from_ref(&event)),
    );
    assert_eq!(status, 200, "sync body: {body}");
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        response["accepted"].as_array().unwrap(),
        &vec![serde_json::json!(event_id)],
        "a landed lane_request is accepted (the delivered receipt)"
    );
    assert!(response["refused"].as_array().unwrap().is_empty());

    let lines = coordination_lines(repo.path());
    assert_eq!(lines.len(), 2, "request then request_delivered");
    assert_eq!(lines[0]["event_type"], "request");
    assert_eq!(lines[0]["payload"]["id"], "req-lane-1");
    assert_eq!(lines[0]["payload"]["from_label"], "alpha");
    assert_eq!(lines[0]["payload"]["to_label"], "beta-role");
    assert_eq!(lines[0]["payload"]["via_machine"], "alpha");
    // `session_id` is the local node's identity, never anything from the wire.
    assert_eq!(lines[0]["session_id"], "node-beta");
    assert_eq!(lines[1]["event_type"], "request_delivered");
    assert_eq!(lines[1]["payload"]["request_id"], "req-lane-1");
    assert_eq!(lines[1]["payload"]["via_machine"], "alpha");
    assert_eq!(lines[1]["payload"]["via_event_id"], event_id);
    let raw = serde_json::to_string(&lines).unwrap();
    assert!(
        !raw.contains("sessionId"),
        "no session id came off the wire"
    );

    // A resend is a transport duplicate and applies nothing twice.
    let (status, body) = http_post(
        addr,
        "/api/sync",
        Some("shared-token"),
        &sync_body("alpha", std::slice::from_ref(&event)),
    );
    assert_eq!(status, 200);
    let response: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(response["duplicates"].as_array().unwrap().len(), 1);
    assert_eq!(coordination_lines(repo.path()).len(), 2, "applied once");
}

/// §6.8 — a `lane_request` carrying a session id is refused by name; the same
/// applies to this machine's own alias (a loop).
#[test]
fn lane_request_carrying_a_session_id_is_refused() {
    let store = tempfile::tempdir().unwrap();
    let repo = workspace();
    let _env = EnvOverride::install(store.path(), repo.path());
    let config = node_config("beta", "127.0.0.1", 6850, vec![]);
    let addr = spawn_server(node_router(repo.path(), config, "shared-token"));

    let mut wire = lane_request_event("alpha", "req-lane-2", "beta-role").to_wire_value();
    wire["sessionId"] = serde_json::json!("s-leak");
    let body = serde_json::json!({
        "version": 1,
        "kind": "edda.node.sync",
        "originMachine": "alpha",
        "events": [wire],
    });
    let (status, response) = http_post(addr, "/api/sync", Some("shared-token"), &body);
    assert_eq!(status, 200, "a per-event refusal is a 200");
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(
        response["refused"][0]["reason"],
        "forbidden field `sessionId`"
    );
    assert!(coordination_lines(repo.path()).is_empty());

    // A lane_request whose origin is this machine's own alias is refused as a
    // loop rather than landed.
    let event = lane_request_event("beta", "req-loop", "beta-role");
    let (status, response) = http_post(
        addr,
        "/api/sync",
        Some("shared-token"),
        &sync_body("beta", std::slice::from_ref(&event)),
    );
    assert_eq!(status, 200);
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["refused"].as_array().unwrap().len(), 1);
    assert!(
        response["refused"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("loop"),
        "the refusal names the loop: {response}"
    );
    assert!(
        coordination_lines(repo.path()).is_empty(),
        "a loop is not landed"
    );
}
