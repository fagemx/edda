//! `edda inbox wait|ack|send|status` — the receiving half of the node lane
//! (GH-685).
//!
//! `wait` is a bounded, read-only waiter: it blocks until an unacked request
//! addressed to `--actor` exists, then returns it. It never acknowledges
//! anything. `ack` records the existing `request_ack` covering exactly one
//! request id, and enqueues the reverse-direction `receipt{state:"acked"}` when
//! the request arrived over the node wire. `send` is an alias for the
//! machine-qualified forwarding path. `status` reads the sender-side durable
//! queue and reports `pending | delivered | acked | dead`.
//!
//! There is no scheduler here and no second task state: `wait` polls disk and
//! nothing runs in the background.

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::path::Path;
use std::time::{Duration, Instant};

use edda_bridge_claude::peers;

/// Bounded `wait` window, per the frozen contract §6.
const WAIT_MIN_SECS: u64 = 1;
const WAIT_MAX_SECS: u64 = 600;
/// No faster than one poll per 250 ms; never a busy loop.
const WAIT_POLL: Duration = Duration::from_millis(250);
/// No ack within this window makes a lane request `dead` (frozen contract §5).
/// Default 24 h; `EDDA_NODE_REQUEST_TTL_SECS` overrides it for tests and for an
/// operator who wants a different horizon.
pub(crate) const DEFAULT_REQUEST_TTL_SECS: u64 = 24 * 60 * 60;

#[derive(Subcommand, Debug)]
pub enum InboxCmd {
    /// Block (bounded) until an unacked request for --actor exists; never acks
    Wait(WaitArgs),
    /// Record an ack covering exactly one request id
    Ack(AckArgs),
    /// Send a request to a label or a `<machine>/<role>` peer
    Send(SendArgs),
    /// Show the delivery state of a request id
    Status(StatusArgs),
}

#[derive(Args, Debug)]
pub struct WaitArgs {
    /// Label the request is addressed to
    #[arg(long)]
    pub actor: String,
    /// Seconds to wait (clamped to 1..=600)
    #[arg(long, default_value_t = 60)]
    pub timeout: u64,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct AckArgs {
    /// Label acknowledging the request
    #[arg(long)]
    pub actor: String,
    /// Request id to acknowledge
    #[arg(long)]
    pub id: String,
    /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
    #[arg(long)]
    pub session: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct SendArgs {
    /// Target label or `<machine>/<role>`
    #[arg(long)]
    pub to: String,
    /// Request message
    #[arg(long)]
    pub message: String,
    /// Session ID (uses EDDA_SESSION_ID; --session required when identity is ambiguous)
    #[arg(long)]
    pub session: Option<String>,
    /// Send even when no active session answers to a local target label
    #[arg(long)]
    pub force: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    /// Request id
    #[arg(long)]
    pub id: String,
    #[arg(long)]
    pub json: bool,
}

/// Run one inbox subcommand and return the process exit code (0 normally, 2 on
/// a `wait` timeout). The caller maps a non-zero code to `std::process::exit`.
pub fn run(cmd: InboxCmd, repo_root: &Path) -> Result<i32> {
    match cmd {
        InboxCmd::Wait(args) => wait(&args, repo_root),
        InboxCmd::Ack(args) => {
            ack(&args, repo_root)?;
            Ok(0)
        }
        InboxCmd::Send(args) => {
            crate::cmd_bridge::request(
                repo_root,
                &args.to,
                &args.message,
                args.session.as_deref(),
                args.force,
                None,
                args.json,
            )?;
            Ok(0)
        }
        InboxCmd::Status(args) => {
            print_request_status(repo_root, &args.id, args.json)?;
            Ok(0)
        }
    }
}

/// The oldest unacked request addressed to `actor` that has not aged past the
/// dead-letter horizon. Read-only: nothing is written.
pub(crate) fn outstanding_request(project_id: &str, actor: &str) -> Option<peers::RequestEntry> {
    let board = peers::compute_board_state(project_id);
    board
        .requests
        .iter()
        .filter(|request| request.to_label == actor)
        .filter(|request| {
            !board.request_acks.iter().any(|ack| {
                ack.request_ids
                    .as_ref()
                    .is_some_and(|ids| ids.iter().any(|id| id == &request.id))
            })
        })
        .filter(|request| !peers::request_is_expired(request))
        .min_by(|left, right| left.ts.cmp(&right.ts).then_with(|| left.id.cmp(&right.id)))
        .cloned()
}

fn wait(args: &WaitArgs, repo_root: &Path) -> Result<i32> {
    let project_id = edda_store::project_id(repo_root);
    let timeout = args.timeout.clamp(WAIT_MIN_SECS, WAIT_MAX_SECS);
    let deadline = Duration::from_secs(timeout);
    let started = Instant::now();
    loop {
        if let Some(request) = outstanding_request(&project_id, &args.actor) {
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "delivered",
                        "request": {
                            "id": request.id,
                            "fromLabel": request.from_label,
                            "toLabel": request.to_label,
                            "message": request.message,
                            "ts": request.ts,
                        },
                    }))?
                );
            } else {
                println!(
                    "[{}] from {}: {}",
                    request.id, request.from_label, request.message
                );
            }
            return Ok(0);
        }
        let elapsed = started.elapsed();
        if elapsed >= deadline {
            break;
        }
        // The timeout is the only sleep bound: no busy loop, no scheduler.
        std::thread::sleep(WAIT_POLL.min(deadline.saturating_sub(elapsed)));
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "timeout",
                "actor": args.actor,
                "timeoutSecs": timeout,
            }))?
        );
    }
    eprintln!(
        "timeout: no unacked request for '{}' within {timeout}s",
        args.actor
    );
    Ok(2)
}

fn ack(args: &AckArgs, repo_root: &Path) -> Result<()> {
    let project_id = edda_store::project_id(repo_root);
    let board = peers::compute_board_state(&project_id);
    let request = board
        .requests
        .iter()
        .find(|request| request.id == args.id)
        .ok_or_else(|| {
            anyhow::anyhow!("unknown request id '{}'; nothing was acknowledged", args.id)
        })?;
    let (session_id, _label) =
        crate::cmd_bridge::resolve_session_id(args.session.as_deref(), &project_id, "inbox")?;
    // The ack names exactly this request id and nothing else.
    peers::write_request_ack_id(&project_id, &session_id, &request.from_label, &args.id)?;

    // A request that arrived over the node wire gets a reverse-direction
    // receipt so the sender's durable queue entry can move to `acked`. A purely
    // local request needs no receipt.
    let delivered = board
        .request_delivered
        .iter()
        .find(|delivered| delivered.request_id == args.id);
    if let Some(delivered) = delivered {
        if !delivered.via_machine.is_empty() && !delivered.via_event_id.is_empty() {
            // Resolve the target peer from the local node config, never from the
            // wire text directly. A `via_machine` that is not a configured peer
            // is a named error and nothing is enqueued (frozen contract §4/§5).
            let config =
                edda_ledger::node::load_node_config(&edda_ledger::node::node_config_path())
                    .map_err(|error| {
                        anyhow::anyhow!("cannot route the ack receipt for '{}': {error:#}", args.id)
                    })?;
            let peer = config
                .peers
                .iter()
                .find(|peer| peer.alias == delivered.via_machine)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "request '{}' arrived from machine '{}', which is not a configured node \
                         peer in node.json; refusing to enqueue a receipt",
                        args.id,
                        delivered.via_machine
                    )
                })?;
            let origin = crate::cmd_bridge::local_machine_alias(repo_root).ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot route the ack receipt: this machine has no node alias (configure node.json)"
                )
            })?;
            let receipt = edda_ledger::node::NodeEvent::new(
                edda_ledger::node::NodeEventBody::Receipt(edda_ledger::node::Receipt {
                    of_event_id: delivered.via_event_id.clone(),
                    of_kind: "lane_request".into(),
                    state: "acked".into(),
                    actor: args.actor.clone(),
                    ts: now_rfc3339(),
                    origin_machine: origin,
                }),
            );
            edda_ledger::node::OutboundQueue::open(&peer.alias)?.enqueue(&receipt)?;
        }
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "status": "acked",
                "requestId": args.id,
                "actor": args.actor,
            }))?
        );
    } else {
        println!("acknowledged request {} for [{}]", args.id, args.actor);
    }
    Ok(())
}

/// `EDDA_NODE_REQUEST_TTL_SECS`, else 24 h. A named constant, documented, never
/// a silent zero.
pub(crate) fn request_delivery_ttl_secs() -> u64 {
    std::env::var("EDDA_NODE_REQUEST_TTL_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_REQUEST_TTL_SECS)
}

/// Locate a lane request in the sender's durable outbound queue and report its
/// delivery state. `None` means no queue entry names the id.
pub(crate) fn request_status_at(
    request_id: &str,
    now: time::OffsetDateTime,
    ttl_secs: u64,
) -> Option<serde_json::Value> {
    let dir = edda_ledger::node::OutboundQueue::queue_dir();
    let entries = std::fs::read_dir(&dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(peer) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let queue = edda_ledger::node::OutboundQueue::open(peer).ok()?;
        for queue_entry in queue.entries().ok()? {
            if queue_entry.kind != "lane_request" {
                continue;
            }
            if queue_entry
                .payload
                .get("requestId")
                .and_then(serde_json::Value::as_str)
                != Some(request_id)
            {
                continue;
            }
            return Some(serde_json::json!({
                "requestId": request_id,
                "peer": peer,
                "state": queue_entry_state(&queue_entry, now, ttl_secs),
                "firstSeenAt": queue_entry.first_seen_at,
                "lastAttemptAt": queue_entry.last_attempt_at,
                "attempts": queue_entry.attempts,
            }));
        }
    }
    None
}

fn queue_entry_state(
    entry: &edda_ledger::node::QueueEntry,
    now: time::OffsetDateTime,
    ttl_secs: u64,
) -> &'static str {
    use edda_ledger::node::QueueState;
    match entry.state {
        QueueState::Acked => "acked",
        QueueState::Delivered => "delivered",
        QueueState::Sent => {
            // Only an unacknowledged request can age out; a delivered-but-unacked
            // request is still pending, never a silent zero.
            if request_is_dead(&entry.first_seen_at, now, ttl_secs) {
                "dead"
            } else {
                "pending"
            }
        }
    }
}

/// True when `stamp` is older than `ttl_secs`. An unparseable timestamp is
/// never called dead — losing a live request is worse than keeping a stale one.
fn request_is_dead(stamp: &str, now: time::OffsetDateTime, ttl_secs: u64) -> bool {
    match time::OffsetDateTime::parse(stamp, &time::format_description::well_known::Rfc3339) {
        Ok(then) => (now - then).whole_seconds() > ttl_secs as i64,
        Err(_) => false,
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
}

/// Print (or error on) the delivery state of one request id.
pub(crate) fn print_request_status(_repo_root: &Path, request_id: &str, json: bool) -> Result<()> {
    let ttl = request_delivery_ttl_secs();
    let Some(status) = request_status_at(request_id, time::OffsetDateTime::now_utc(), ttl) else {
        bail!("unknown request id '{request_id}': no durable queue entry names it");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!(
            "request {}: {} (firstSeenAt={} lastAttemptAt={})",
            request_id,
            status["state"].as_str().unwrap_or("unknown"),
            status["firstSeenAt"].as_str().unwrap_or("-"),
            status["lastAttemptAt"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use edda_ledger::node::{LaneRequest, NodeEvent, NodeEventBody, OutboundQueue};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn workspace() -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_inbox_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = edda_ledger::EddaPaths::discover(&tmp);
        edda_ledger::ledger::init_workspace(&paths).unwrap();
        edda_ledger::ledger::init_head(&paths, "main").unwrap();
        edda_ledger::ledger::init_branches_json(&paths, "main").unwrap();
        tmp
    }

    fn lane_event(request_id: &str, from: &str, to: &str, machine: &str) -> NodeEvent {
        NodeEvent::new(NodeEventBody::LaneRequest(LaneRequest {
            request_id: request_id.into(),
            from_label: from.into(),
            to_label: to.into(),
            message: "please look".into(),
            ts: "2026-09-16T00:00:00Z".into(),
            origin_machine: machine.into(),
        }))
    }

    // §6.4
    #[test]
    fn request_status_moves_pending_to_delivered_to_acked() {
        let _store = crate::test_support::isolated_store();
        let request_id = "req-status-1";
        let event = lane_event(request_id, "alpha", "beta", "machine-a");
        let queue = OutboundQueue::open("docs").unwrap();
        queue.enqueue(&event).unwrap();

        let now = time::OffsetDateTime::now_utc();
        let state = |id: &str| {
            request_status_at(id, now, DEFAULT_REQUEST_TTL_SECS)
                .and_then(|value| value["state"].as_str().map(str::to_string))
                .expect("status entry")
        };
        assert_eq!(state(request_id), "pending");
        queue
            .mark_delivered(std::slice::from_ref(&event.event_id))
            .unwrap();
        assert_eq!(state(request_id), "delivered");
        queue
            .mark_acked(std::slice::from_ref(&event.event_id))
            .unwrap();
        assert_eq!(state(request_id), "acked");
    }

    // §6.5
    #[test]
    fn request_status_is_dead_after_ttl() {
        let _store = crate::test_support::isolated_store();
        let request_id = "req-dead-1";
        let event = lane_event(request_id, "alpha", "beta", "machine-a");
        let queue = OutboundQueue::open("docs").unwrap();
        queue.enqueue(&event).unwrap();
        // An acked entry is acked regardless of age.
        queue
            .mark_acked(std::slice::from_ref(&event.event_id))
            .unwrap();
        let later = time::OffsetDateTime::now_utc() + time::Duration::hours(48);
        let status = request_status_at(request_id, later, DEFAULT_REQUEST_TTL_SECS).unwrap();
        assert_eq!(status["state"], "acked");

        // A fresh `sent` entry with a tiny TTL is dead.
        let request_id = "req-dead-2";
        let event = lane_event(request_id, "alpha", "beta", "machine-a");
        queue.enqueue(&event).unwrap();
        let status = request_status_at(request_id, later, DEFAULT_REQUEST_TTL_SECS).unwrap();
        assert_eq!(status["state"], "dead", "an unacked request ages out");
    }

    // §6.2
    #[test]
    fn inbox_wait_returns_on_delivery_and_exits_nonzero_on_timeout() {
        let _store = crate::test_support::isolated_store();
        let repo = workspace();
        let project_id = edda_store::project_id(&repo);
        let _ = edda_store::ensure_dirs(&project_id);

        // A delivered request is returned immediately, and `wait` never acks it.
        peers::write_remote_request(
            &project_id,
            "node-machine-b",
            "req-wait-1",
            "alpha",
            "beta",
            "please look",
            "machine-b",
        )
        .unwrap();
        peers::write_request_delivered(&project_id, "req-wait-1", "beta", "machine-b", "evt-1")
            .unwrap();
        let code = run(
            InboxCmd::Wait(WaitArgs {
                actor: "beta".into(),
                timeout: 1,
                json: true,
            }),
            &repo,
        )
        .unwrap();
        assert_eq!(code, 0, "wait returns 0 when the request exists");
        assert!(
            outstanding_request(&project_id, "beta").is_some(),
            "wait must not ack the request"
        );

        // No request for the actor: the bounded wait exits non-zero (2).
        let code = run(
            InboxCmd::Wait(WaitArgs {
                actor: "nobody".into(),
                timeout: 1,
                json: true,
            }),
            &repo,
        )
        .unwrap();
        assert_eq!(code, 2, "a timeout is a non-zero exit");
    }

    // §6.3 + receipt flow
    #[test]
    fn inbox_ack_covers_exactly_one_request_id() {
        let _store = crate::test_support::isolated_store();
        let repo = workspace();
        let project_id = edda_store::project_id(&repo);
        let _ = edda_store::ensure_dirs(&project_id);
        for id in ["req-ack-a", "req-ack-b"] {
            peers::write_remote_request(
                &project_id,
                "node-machine-b",
                id,
                "alpha",
                "beta",
                "please look",
                "machine-b",
            )
            .unwrap();
        }
        // An unknown id is a named error, not a silent success.
        let error = run(
            InboxCmd::Ack(AckArgs {
                actor: "beta".into(),
                id: "req-unknown".into(),
                session: None,
                json: true,
            }),
            &repo,
        )
        .expect_err("unknown id must be refused");
        assert!(error.to_string().contains("unknown request id"));

        run(
            InboxCmd::Ack(AckArgs {
                actor: "beta".into(),
                id: "req-ack-a".into(),
                session: Some("s-ack".into()),
                json: true,
            }),
            &repo,
        )
        .unwrap();
        assert!(
            outstanding_request(&project_id, "beta").is_some_and(|r| r.id == "req-ack-b"),
            "ack retires exactly the named request"
        );
    }

    #[test]
    fn inbox_ack_enqueues_a_receipt_that_moves_the_sender_to_acked() {
        let _store = crate::test_support::isolated_store();
        let repo = workspace();
        let project_id = edda_store::project_id(&repo);
        let _ = edda_store::ensure_dirs(&project_id);
        std::fs::write(
            edda_ledger::node::node_config_path(),
            serde_json::to_vec_pretty(&serde_json::json!({
                "version": 1,
                "node": { "alias": "machine-b", "bind": "127.0.0.1", "port": 6850 },
                "peers": [
                    { "alias": "docs", "host": "127.0.0.1", "port": 1, "token": "t" }
                ],
            }))
            .unwrap(),
        )
        .unwrap();

        // The sender's durable queue holds the lane_request under event id E.
        let event = lane_event("req-ack-receipt", "alpha", "beta-role", "machine-a");
        let event_id = event.event_id.clone();
        let queue = OutboundQueue::open("docs").unwrap();
        queue.enqueue(&event).unwrap();
        // The receiving side landed it, recording the origin as the peer.
        peers::write_remote_request(
            &project_id,
            "node-machine-b",
            "req-ack-receipt",
            "alpha",
            "beta-role",
            "please look",
            "docs",
        )
        .unwrap();
        peers::write_request_delivered(
            &project_id,
            "req-ack-receipt",
            "beta-role",
            "docs",
            &event_id,
        )
        .unwrap();

        run(
            InboxCmd::Ack(AckArgs {
                actor: "beta-role".into(),
                id: "req-ack-receipt".into(),
                session: Some("s-ack".into()),
                json: true,
            }),
            &repo,
        )
        .unwrap();

        // The ack enqueued a receipt naming the original event id.
        let entries = queue.entries().unwrap();
        let receipt = entries
            .iter()
            .find(|entry| entry.kind == "receipt")
            .expect("ack enqueued a receipt");
        assert_eq!(receipt.payload["ofEventId"], event_id);
        assert_eq!(receipt.payload["state"], "acked");
        let receipt_event = edda_ledger::node::parse_event(&receipt.to_wire_value())
            .expect("receipt is a valid wire event");

        // Applying it on the sender moves the matching queue entry to acked.
        edda_ledger::node::import_batch(&repo, &[receipt_event]).unwrap();
        let status = queue.status().unwrap();
        assert_eq!(status.acked, 1, "the sender's entry is acked");
    }

    // §6.6
    #[test]
    fn machine_qualified_target_enqueues_for_the_peer_and_stays_local_for_self() {
        let _store = crate::test_support::isolated_store();
        let repo = workspace();
        // Configure this machine as `alpha` with a peer `docs`.
        let config = serde_json::json!({
            "version": 1,
            "node": { "alias": "alpha", "bind": "127.0.0.1", "port": 6850 },
            "peers": [
                { "alias": "docs", "host": "127.0.0.1", "port": 1, "token": "t" }
            ],
        });
        std::fs::write(
            edda_ledger::node::node_config_path(),
            serde_json::to_vec_pretty(&config).unwrap(),
        )
        .unwrap();

        // A remote target enqueues a lane_request for the peer.
        crate::cmd_bridge::request(
            &repo,
            "docs/role",
            "cross-machine please",
            Some("s-cli"),
            false,
            None,
            true,
        )
        .unwrap();
        let queue = OutboundQueue::open("docs").unwrap();
        let entries = queue.entries().unwrap();
        assert_eq!(entries.len(), 1, "one lane_request was enqueued");
        assert_eq!(entries[0].kind, "lane_request");
        assert_eq!(entries[0].payload["toLabel"], "role");
        assert_eq!(entries[0].payload["originMachine"], "alpha");

        // A same-machine target stays local: no queue entry is created.
        crate::cmd_bridge::request(
            &repo,
            "alpha/role",
            "local please",
            Some("s-cli"),
            true,
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            queue.entries().unwrap().len(),
            1,
            "a same-machine target must not cross the wire"
        );
    }

    #[test]
    fn machine_qualified_target_to_an_unconfigured_peer_is_refused_unless_forced() {
        let _store = crate::test_support::isolated_store();
        let repo = workspace();
        let config = serde_json::json!({
            "version": 1,
            "node": { "alias": "alpha", "bind": "127.0.0.1", "port": 6850 },
            "peers": [],
        });
        std::fs::write(
            edda_ledger::node::node_config_path(),
            serde_json::to_vec_pretty(&config).unwrap(),
        )
        .unwrap();

        // A machine that is not a configured peer is a named error and queues
        // nothing, mirroring the local unknown-label rule.
        let error =
            crate::cmd_bridge::request(&repo, "ghost/role", "hi", Some("s-cli"), false, None, true)
                .expect_err("an unconfigured peer must be refused without --force");
        assert!(
            error.to_string().contains("not a configured node peer"),
            "unexpected refusal: {error}"
        );
        assert!(
            !edda_ledger::node::OutboundQueue::queue_dir()
                .join("ghost.jsonl")
                .exists(),
            "a refused target queues nothing"
        );

        // With --force it queues, with a warning that nothing will flush it.
        crate::cmd_bridge::request(&repo, "ghost/role", "hi", Some("s-cli"), true, None, true)
            .unwrap();
        let queue = OutboundQueue::open("ghost").unwrap();
        assert_eq!(queue.entries().unwrap().len(), 1);
    }
}
