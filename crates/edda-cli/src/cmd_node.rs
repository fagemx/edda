//! `edda node start|status|peers` — the per-machine node transport verbs.
//!
//! `start` runs the existing HTTP server with the node routes wired and a
//! bounded replicator that flushes each peer's durable outbound queue every
//! ~2 s. It is not a scheduler: no model is woken, no task state is created, no
//! remote process is executed. The queue is on disk, so a restart loses nothing.
//!
//! `status` and `peers` are local observations: the queue counts come from disk
//! and peer reachability is a bounded TCP probe. An unreachable peer is
//! reported with an explicit reason, never as a silent zero. The revision is
//! this binary's own build identity (`EDDA_LONG_VERSION`), else the workspace
//! git HEAD, with a `revisionOrigin` naming which — never a Pi package release
//! id and never a global claim (contract §7).

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use edda_ledger::node::{
    load_node_config, local_revision_origin, node_config_path, peer_observations,
    peer_queue_status, record_peer_observation, resolve_peer_token,
    validate_config_with_bind_policy, NodeConfig, OutboundQueue, PeerConfig,
};
use edda_serve::ServeConfig;

const FLUSH_INTERVAL: Duration = Duration::from_secs(2);
const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Subcommand, Debug)]
pub enum NodeCmd {
    /// Start the node: HTTP server with node routes plus the outbound replicator
    Start(StartArgs),
    /// Local observation: machine, bind, revision, per-peer queue and reachability
    Status(StatusArgs),
    /// Configured peers and their last-seen facts
    Peers(PeersArgs),
}

#[derive(Args, Debug)]
pub struct StartArgs {
    /// Node config path (default: `<store root>/node.json`)
    #[arg(long)]
    pub config: Option<String>,
    /// Test-only: allow a non-tailnet bind (never used for the real proof)
    #[arg(long)]
    pub insecure_bind: bool,
}

#[derive(Args, Debug)]
pub struct StatusArgs {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub insecure_bind: bool,
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct PeersArgs {
    #[arg(long)]
    pub config: Option<String>,
    #[arg(long)]
    pub insecure_bind: bool,
    #[arg(long)]
    pub json: bool,
}

pub fn run(cmd: NodeCmd, repo_root: &Path) -> Result<()> {
    match cmd {
        NodeCmd::Start(args) => start(repo_root, args),
        NodeCmd::Status(args) => status(repo_root, args),
        NodeCmd::Peers(args) => peers(args),
    }
}

fn config_path(given: &Option<String>) -> PathBuf {
    given
        .as_deref()
        .map(PathBuf::from)
        .unwrap_or_else(node_config_path)
}

fn load_validated(given: &Option<String>, insecure_bind: bool) -> Result<NodeConfig> {
    let path = config_path(given);
    let config = load_node_config(&path)?;
    validate_config_with_bind_policy(&config, insecure_bind)?;
    Ok(config)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ── start ─────────────────────────────────────────────────────────────

fn start(repo_root: &Path, args: StartArgs) -> Result<()> {
    let config = load_validated(&args.config, args.insecure_bind)?;

    // The shared token is read from the environment at start time and never
    // stored, printed or logged. An unset token is a 401-class server, not an
    // open door.
    let server_token = std::env::var("EDDA_NODE_TOKEN")
        .ok()
        .filter(|value| !value.is_empty());
    if server_token.is_none() {
        eprintln!(
            "warning: EDDA_NODE_TOKEN is not set; every inbound sync request will be 401 until it is"
        );
    }

    eprintln!(
        "edda node '{}' listening on {}:{} ({} peer(s))",
        config.node.alias,
        config.node.bind,
        config.node.port,
        config.peers.len()
    );

    let serve_config = ServeConfig {
        bind: config.node.bind.clone(),
        port: config.node.port,
        node: Some(config.clone()),
        node_token: server_token,
        insecure_bind: args.insecure_bind,
    };

    let repo = repo_root.to_path_buf();
    let serve_thread = std::thread::spawn(move || {
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(edda_serve::serve(&repo, serve_config))
    });

    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);
    // A previously-installed handler is not an error: without one, Ctrl-C still
    // terminates the process by default.
    let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));

    loop {
        if shutdown.load(Ordering::SeqCst) {
            eprintln!("edda node: shutting down");
            return Ok(());
        }
        if serve_thread.is_finished() {
            return serve_thread
                .join()
                .map_err(|_| anyhow::anyhow!("node server thread panicked"))
                .and_then(|result| result);
        }
        // Bounded cadence: no busy loop. The flush is blocking but short
        // (5 s global timeout per peer) and runs every 2 s.
        std::thread::sleep(FLUSH_INTERVAL);
        if let Err(error) = flush_all_peers(&config) {
            eprintln!("edda node: flush pass failed: {error:#}");
        }
    }
}

fn flush_all_peers(config: &NodeConfig) -> Result<()> {
    for peer in &config.peers {
        let Some(token) = resolve_peer_token(peer) else {
            eprintln!(
                "edda node: peer '{}' has no token (env '{}' unset); events stay queued",
                peer.alias,
                peer.token_env.as_deref().unwrap_or("-")
            );
            continue;
        };
        if let Err(error) = flush_peer(&config.node.alias, peer, Some(&token)) {
            eprintln!("edda node: peer '{}' flush failed: {error:#}", peer.alias);
        }
    }
    Ok(())
}

/// Send this peer's `sent` entries; mark `delivered` on 2xx, keep `sent` (with
/// an attempt recorded) otherwise. Nothing is ever deleted on a failed send.
fn flush_peer(origin: &str, peer: &PeerConfig, token: Option<&str>) -> Result<()> {
    let queue = OutboundQueue::open(&peer.alias)?;
    let pending = queue.pending()?;
    if pending.is_empty() {
        return Ok(());
    }
    let ids: Vec<String> = pending.iter().map(|entry| entry.event_id.clone()).collect();
    let events: Vec<serde_json::Value> = pending
        .iter()
        .map(edda_ledger::node::QueueEntry::to_wire_value)
        .collect();
    let body = serde_json::json!({
        "version": 1,
        "kind": "edda.node.sync",
        "originMachine": origin,
        "events": events,
    });

    let url = format!("http://{}:{}/api/sync", peer.host, peer.port);
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .new_agent();
    let mut request = agent.post(&url).header("Content-Type", "application/json");
    if let Some(token) = token {
        request = request.header("Authorization", &format!("Bearer {token}"));
    }
    match request.send(serde_json::to_string(&body)?) {
        Ok(response) if response.status().is_success() => {
            // A 2xx is the delivered receipt — but only for the events the peer
            // actually accepted or reported as duplicates. A refused event
            // (e.g. a lane_request this build does not land) stays queued rather
            // than being silently dropped as "delivered".
            let acknowledged: Option<Vec<String>> = response
                .into_body()
                .read_to_string()
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .map(|payload| {
                    ["accepted", "duplicates"]
                        .iter()
                        .filter_map(|key| payload.get(*key).and_then(serde_json::Value::as_array))
                        .flatten()
                        .filter_map(|value| value.as_str().map(str::to_string))
                        .collect()
                });
            let delivered = match acknowledged {
                Some(acknowledged) => {
                    let (delivered, refused): (Vec<String>, Vec<String>) =
                        ids.into_iter().partition(|id| acknowledged.contains(id));
                    if !refused.is_empty() {
                        // Keep the refused events `sent`; they are not delivered.
                        queue.mark_attempt(&refused)?;
                    }
                    delivered
                }
                // An unreadable 2xx body falls back to the plain receipt rule.
                None => ids,
            };
            if !delivered.is_empty() {
                queue.mark_delivered(&delivered)?;
            }
            Ok(())
        }
        Ok(response) => {
            queue.mark_attempt(&ids)?;
            bail!(
                "peer returned {}; {} event(s) stay queued",
                response.status(),
                ids.len()
            )
        }
        Err(error) => {
            queue.mark_attempt(&ids)?;
            bail!(
                "peer unreachable ({error}); {} event(s) stay queued",
                ids.len()
            )
        }
    }
}

// ── status ────────────────────────────────────────────────────────────

fn status(repo_root: &Path, args: StatusArgs) -> Result<()> {
    let config = load_validated(&args.config, args.insecure_bind)?;
    let (revision, revision_origin) = status_revision(repo_root);
    let stamp = now();

    let mut queues = Vec::new();
    let mut peers = Vec::new();
    for peer in &config.peers {
        let status = peer_queue_status(&peer.alias).ok();
        queues.push(serde_json::json!({
            "peer": peer.alias,
            "pending": status.as_ref().map(|s| s.pending),
            "sent": status.as_ref().map(|s| s.sent),
            "delivered": status.as_ref().map(|s| s.delivered),
            "acked": status.as_ref().map(|s| s.acked),
            "lastSuccessAt": status.as_ref().and_then(|s| s.last_success_at.clone()),
        }));
        let (reachable, reason) = probe_peer(peer);
        record_peer_observation(&peer.alias, reachable, &reason, &stamp)?;
        peers.push(serde_json::json!({
            "alias": peer.alias,
            "host": peer.host,
            "port": peer.port,
            "reachable": reachable,
            "reason": reason,
            "lastSeenAt": stamp,
        }));
    }

    let document = serde_json::json!({
        "machine": config.node.alias,
        "bind": config.node.bind,
        "port": config.node.port,
        "revision": revision,
        "revisionOrigin": revision_origin,
        "queue": queues,
        "peers": peers,
    });
    if args.json {
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }
    println!(
        "machine:  {}",
        document["machine"].as_str().unwrap_or("unknown")
    );
    println!(
        "bind:     {}:{}",
        document["bind"].as_str().unwrap_or("-"),
        config.node.port
    );
    println!(
        "revision: {} ({})",
        document["revision"].as_str().unwrap_or("null"),
        document["revisionOrigin"].as_str().unwrap_or("none")
    );
    for queue in document["queue"].as_array().into_iter().flatten() {
        println!(
            "queue  {}: pending={} sent={} delivered={} acked={} lastSuccessAt={}",
            queue["peer"].as_str().unwrap_or("-"),
            queue["pending"],
            queue["sent"],
            queue["delivered"],
            queue["acked"],
            queue["lastSuccessAt"],
        );
    }
    for peer in document["peers"].as_array().into_iter().flatten() {
        println!(
            "peer   {} {}:{} reachable={} reason={}",
            peer["alias"].as_str().unwrap_or("-"),
            peer["host"].as_str().unwrap_or("-"),
            peer["port"],
            peer["reachable"],
            peer["reason"].as_str().unwrap_or("-"),
        );
    }
    Ok(())
}

/// The revision this process reports and where it came from. Prefer this
/// binary's own build identity (`EDDA_LONG_VERSION`), falling back to the
/// workspace git HEAD. Never `EDDA_PI_RELEASE_ID`, which is a Pi package
/// release id and not the edda revision (contract §7).
fn status_revision(repo_root: &Path) -> (Option<String>, &'static str) {
    status_revision_from(env!("EDDA_LONG_VERSION"), repo_root)
}

fn status_revision_from(long_version: &str, repo_root: &Path) -> (Option<String>, &'static str) {
    if let Some(revision) = build_revision_from(long_version) {
        return (Some(revision), "build");
    }
    let (revision, origin) = local_revision_origin(repo_root);
    (revision, origin.as_str())
}

/// Extract the 12-hex revision from `EDDA_LONG_VERSION`, whose format is
/// `0.6.2 (<12-hex>[-dirty] <date>)`. `0.6.2 (unknown)` yields `None`, so the
/// caller falls back to the workspace HEAD instead of inventing a revision.
fn build_revision_from(long_version: &str) -> Option<String> {
    let open = long_version.find('(')?;
    let close = long_version.rfind(')')?;
    if close <= open + 1 {
        return None;
    }
    let identity = &long_version[open + 1..close];
    let sha = identity
        .split_whitespace()
        .next()?
        .trim_end_matches("-dirty");
    if sha.len() == 12 && sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(sha.to_string())
    } else {
        None
    }
}

/// Bounded TCP probe. `reachable` means the peer's port accepted a connection —
/// a local observation, not an authenticated peer response.
fn probe_peer(peer: &PeerConfig) -> (bool, String) {
    let target = format!("{}:{}", peer.host, peer.port);
    let Ok(address) = target.parse::<SocketAddr>() else {
        return (false, format!("'{target}' is not an IP:port address"));
    };
    match TcpStream::connect_timeout(&address, PROBE_TIMEOUT) {
        Ok(_) => (true, "tcp connect ok".to_string()),
        Err(error) => (false, format!("tcp connect failed: {error}")),
    }
}

// ── peers ─────────────────────────────────────────────────────────────

fn peers(args: PeersArgs) -> Result<()> {
    let config = load_validated(&args.config, args.insecure_bind)?;
    let observations = peer_observations().unwrap_or_default();
    let peers: Vec<serde_json::Value> = config
        .peers
        .iter()
        .map(|peer| {
            let observed = observations.get(&peer.alias);
            serde_json::json!({
                "alias": peer.alias,
                "host": peer.host,
                "port": peer.port,
                "reachable": observed
                    .and_then(|value| value.get("reachable"))
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
                "reason": observed
                    .and_then(|value| value.get("reason"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("no observation recorded"),
                "lastSeenAt": observed
                    .and_then(|value| value.get("lastSeenAt"))
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            })
        })
        .collect();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "machine": config.node.alias,
                "peers": peers,
            }))?
        );
        return Ok(());
    }
    for peer in &peers {
        println!(
            "{} {}:{} reachable={} reason={} lastSeenAt={}",
            peer["alias"].as_str().unwrap_or("-"),
            peer["host"].as_str().unwrap_or("-"),
            peer["port"],
            peer["reachable"],
            peer["reason"].as_str().unwrap_or("-"),
            peer["lastSeenAt"],
        );
    }
    Ok(())
}

/// Read a config file for tests without touching the real store root.
#[cfg(test)]
fn write_config(dir: &Path, body: &serde_json::Value) -> PathBuf {
    let path = dir.join("node.json");
    std::fs::write(&path, serde_json::to_vec_pretty(body).unwrap()).unwrap();
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_start_refuses_non_tailnet_bind_without_insecure_flag() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            &serde_json::json!({
                "version": 1,
                "node": { "alias": "alpha", "bind": "127.0.0.1", "port": 6850 },
                "peers": [],
            }),
        );
        let given = Some(path.to_string_lossy().into_owned());
        let error = load_validated(&given, false).expect_err("non-tailnet bind must be refused");
        assert!(
            format!("{error:#}").contains("Tailscale") || format!("{error:#}").contains("100.x"),
            "unexpected refusal: {error:#}"
        );
        // The explicit test-only flag unlocks it.
        load_validated(&given, true).expect("insecure flag unlocks a local test rig");
    }

    #[test]
    fn node_status_reports_unreachable_peer_without_silent_zero() {
        let _store = edda_store::test_support::isolated_store_root().expect("store");
        let dir = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            &serde_json::json!({
                "version": 1,
                "node": { "alias": "alpha", "bind": "127.0.0.1", "port": 6850 },
                "peers": [{ "alias": "beta", "host": "127.0.0.1", "port": 1 }],
            }),
        );
        status(
            repo.path(),
            StatusArgs {
                config: Some(path.to_string_lossy().into_owned()),
                insecure_bind: true,
                json: true,
            },
        )
        .expect("status");

        let observations = peer_observations().expect("observations");
        let peer = observations.get("beta").expect("recorded observation");
        assert_eq!(
            peer["reachable"], false,
            "an unreachable peer is not healthy-zero"
        );
        assert!(
            !peer["reason"].as_str().unwrap().is_empty(),
            "an unreachable peer carries a reason"
        );
        assert!(!peer["lastSeenAt"].as_str().unwrap().is_empty());
    }

    #[test]
    fn build_revision_is_extracted_from_the_long_version_not_a_pi_release_id() {
        assert_eq!(
            build_revision_from("0.6.2 (7419a701e521 2026-09-13)").as_deref(),
            Some("7419a701e521")
        );
        assert_eq!(
            build_revision_from("0.6.2 (7419a701e521-dirty 2026-09-13)").as_deref(),
            Some("7419a701e521")
        );
        // No build identity: fall back, never fabricate.
        assert_eq!(build_revision_from("0.6.2 (unknown)"), None);
        assert_eq!(build_revision_from("0.6.2"), None);
        // A Pi release id or session id is never substituted for the edda rev.
        assert_eq!(build_revision_from("0.6.2 (pi-2026.09)"), None);
    }

    #[test]
    fn status_revision_names_its_origin_and_never_fabricates() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            status_revision_from("0.6.2 (7419a701e521 2026-09-13)", dir.path()),
            (Some("7419a701e521".to_string()), "build")
        );
        // No build identity and no git HEAD: `none` pairs with `null`.
        assert_eq!(
            status_revision_from("0.6.2 (unknown)", dir.path()),
            (None, "none")
        );
    }

    #[test]
    fn node_peers_lists_configured_peers() {
        let _store = edda_store::test_support::isolated_store_root().expect("store");
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            dir.path(),
            &serde_json::json!({
                "version": 1,
                "node": { "alias": "alpha", "bind": "100.64.0.1", "port": 6850 },
                "peers": [{ "alias": "beta", "host": "100.64.0.2", "port": 6850 }],
            }),
        );
        let config = load_validated(&Some(path.to_string_lossy().into_owned()), false).unwrap();
        assert_eq!(config.peers.len(), 1);
        assert_eq!(config.peers[0].alias, "beta");
        assert!(resolve_peer_token(&config.peers[0]).is_none());

        let observations = peer_observations().unwrap();
        assert!(
            observations.get("beta").is_none(),
            "no observation is recorded before a status probe"
        );
        peers(PeersArgs {
            config: Some(path.to_string_lossy().into_owned()),
            insecure_bind: false,
            json: true,
        })
        .expect("peers");
    }
}
