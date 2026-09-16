//! Unit tests for the `edda node` ledger core (SPEC §8.1–§8.9, §8.11).
//!
//! Each test that touches the store root installs its own thread-local isolated
//! store root, so parallel tests never see each other's queue, imported-key log,
//! transitions, handover or observations.

use super::config::*;
use super::envelope::*;
use super::import::*;
use super::queue::*;
use crate::{ledger as ledger_mod, EddaPaths, Ledger};

// ── harness ───────────────────────────────────────────────────────────

fn isolated_store() -> edda_store::test_support::IsolatedStoreRoot {
    edda_store::test_support::isolated_store_root().expect("isolated store root")
}

fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp workspace");
    let paths = EddaPaths::discover(dir.path());
    paths.ensure_layout().expect("layout");
    ledger_mod::init_workspace(&paths).expect("init workspace");
    ledger_mod::init_head(&paths, "main").expect("init head");
    ledger_mod::init_branches_json(&paths, "main").expect("init branches");
    dir
}

/// Serialize tests that redirect `EDDA_RETURN_ROOT`: it is process-global, so
/// parallel tests would otherwise share one mailbox.
static MAILBOX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct MailboxOverride {
    previous: Option<std::ffi::OsString>,
}

impl Drop for MailboxOverride {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(value) => std::env::set_var("EDDA_RETURN_ROOT", value),
            None => std::env::remove_var("EDDA_RETURN_ROOT"),
        }
    }
}

/// A per-test store root, workspace and mailbox root, with the process-global
/// `EDDA_RETURN_ROOT` pinned to the workspace and restored on drop.
struct TestEnv {
    _store: edda_store::test_support::IsolatedStoreRoot,
    repo: tempfile::TempDir,
    _mailbox: MailboxOverride,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl TestEnv {
    fn new() -> Self {
        let lock = MAILBOX_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let store = isolated_store();
        let repo = workspace();
        let previous = std::env::var_os("EDDA_RETURN_ROOT");
        std::env::set_var("EDDA_RETURN_ROOT", repo.path());
        Self {
            _store: store,
            repo,
            _mailbox: MailboxOverride { previous },
            _lock: lock,
        }
    }

    fn path(&self) -> &std::path::Path {
        self.repo.path()
    }
}

fn sha(value: &str) -> String {
    edda_core::hash::sha256_hex(value.as_bytes())
}

fn owner_return_event(machine: &str, work: &str, message_id_seed: &str) -> NodeEvent {
    let message = format!("body for {work}");
    let logical = owner_return_logical_id(
        "assistant/p",
        work,
        "done",
        Some("ok"),
        Some("out.md"),
        Some(&message),
    );
    NodeEvent::new(NodeEventBody::OwnerReturn(OwnerReturn {
        logical_id: logical,
        message_id: sha(message_id_seed),
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

fn decision_fact_event(machine: &str, key: &str, value: &str, ratified: bool) -> NodeEvent {
    NodeEvent::new(NodeEventBody::DecisionFact(DecisionFact {
        key: key.into(),
        value: value.into(),
        scope: "shared".into(),
        ratified,
        actor: "agent-a".into(),
        origin_machine: machine.into(),
        ts: "2026-09-14T00:00:00.000Z".into(),
    }))
}

fn mailbox_message_count(repo_root: &std::path::Path) -> usize {
    let dir = repo_root.join(".edda").join("returns").join("messages");
    match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
            .count(),
        Err(_) => 0,
    }
}

// ── §8.1 config ───────────────────────────────────────────────────────

#[test]
fn config_rejects_unknown_key() {
    let json = r#"{
        "version": 1,
        "node": { "alias": "alpha", "bind": "100.64.0.1", "port": 6850, "extra": true },
        "peers": []
    }"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.json");
    std::fs::write(&path, json).unwrap();
    let error = load_node_config(&path).expect_err("unknown key must fail closed");
    assert!(
        format!("{error:#}").contains("extra"),
        "the refusal must name the offending key: {error:#}"
    );
}

#[test]
fn config_rejects_non_tailnet_bind() {
    let config = NodeConfig {
        version: 1,
        node: NodeSection {
            alias: "alpha".into(),
            bind: "127.0.0.1".into(),
            port: 6850,
        },
        peers: vec![PeerConfig {
            alias: "beta".into(),
            host: "100.64.0.2".into(),
            port: 6850,
            token_env: Some("EDDA_NODE_TOKEN".into()),
            token: None,
        }],
    };
    let error = validate_config(&config).expect_err("non-tailnet bind must be refused");
    assert!(format!("{error:#}").contains("100.x") || format!("{error:#}").contains("Tailscale"));
    // The explicit test-only insecure flag unlocks the local rig.
    validate_config_with_bind_policy(&config, true).expect("insecure flag unlocks local test rig");
}

#[test]
fn node_bind_non_tailnet_refused_without_insecure_flag() {
    // The bind guard itself: the listener bind address and every peer host are
    // both checked, and a non-tailnet peer host is refused by name.
    let mut config = valid_tailnet_config();
    config.node.bind = "0.0.0.0".into();
    assert!(validate_config(&config).is_err(), "0.0.0.0 must be refused");
    let mut config = valid_tailnet_config();
    config.peers[0].host = "192.168.1.10".into();
    let error = validate_config(&config).expect_err("LAN peer host must be refused");
    assert!(format!("{error:#}").contains("peers[0].host"));
    validate_config_with_bind_policy(&config, true).expect("insecure flag unlocks a LAN test host");
}

#[test]
fn config_rejects_bad_alias() {
    for alias in ["", "Alpha", "a b", "a/b", &"x".repeat(65)] {
        let mut config = valid_tailnet_config();
        config.node.alias = alias.to_string();
        assert!(
            validate_config(&config).is_err(),
            "alias {alias:?} must be refused"
        );
    }
}

#[test]
fn config_never_needs_a_token_value_in_the_file() {
    let json = r#"{
        "version": 1,
        "node": { "alias": "alpha", "bind": "100.64.0.1", "port": 6850 },
        "peers": [
            { "alias": "beta", "host": "100.64.0.2", "port": 6850, "tokenEnv": "EDDA_NODE_TEST_UNSET" }
        ]
    }"#;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.json");
    std::fs::write(&path, json).unwrap();
    let config = load_node_config(&path).expect("tokenEnv-only config loads");
    validate_config(&config).expect("tokenEnv-only config is valid");
    // An unset token resolves to None — a 401-class failure, never an open door.
    assert_eq!(resolve_peer_token(&config.peers[0]), None);
    let serialized = serde_json::to_string(&config).unwrap();
    assert!(
        !serialized.contains("\"token\":"),
        "a config with no literal token must not serialize a token key: {serialized}"
    );
    // A literal test-rig token is accepted and resolved as the fallback.
    let mut peer = config.peers[0].clone();
    peer.token = Some("rig-token".into());
    assert_eq!(resolve_peer_token(&peer).as_deref(), Some("rig-token"));
}

fn valid_tailnet_config() -> NodeConfig {
    NodeConfig {
        version: 1,
        node: NodeSection {
            alias: "alpha".into(),
            bind: "100.64.0.1".into(),
            port: 6850,
        },
        peers: vec![PeerConfig {
            alias: "beta".into(),
            host: "100.64.0.2".into(),
            port: 6850,
            token_env: Some("EDDA_NODE_TOKEN".into()),
            token: None,
        }],
    }
}

// ── §8.2 canonical identity ───────────────────────────────────────────

#[test]
fn event_id_is_stable_and_recomputed_on_import() {
    let first = owner_return_event("alpha", "job-a", "seed-a");
    let second = owner_return_event("alpha", "job-a", "seed-a");
    assert_eq!(first.event_id, second.event_id, "eventId is deterministic");
    assert_eq!(first.event_id.len(), 64);

    let wire = first.to_wire_value();
    let reparsed = parse_event(&wire).expect("canonical event parses");
    assert_eq!(reparsed, first, "wire round trip is lossless");

    // A tampered eventId no longer matches the recomputed canonical value.
    let mut tampered = wire.clone();
    tampered["eventId"] = serde_json::json!(sha("not-the-canonical-bytes"));
    let refusal = parse_event(&tampered).expect_err("tampered eventId must be refused");
    assert!(
        refusal.reason.contains("eventId does not match"),
        "unexpected refusal: {}",
        refusal.reason
    );
}

// ── §8.3 unknown / forbidden fields ───────────────────────────────────

#[test]
fn parse_event_refuses_unknown_field() {
    let mut wire = owner_return_event("alpha", "job-a", "seed-a").to_wire_value();
    wire["fooBar"] = serde_json::json!("nope");
    let refusal = parse_event(&wire).expect_err("unknown field must be refused");
    assert_eq!(refusal.reason, "unknown field `fooBar`");
}

#[test]
fn parse_event_refuses_secret_shaped_field() {
    for forbidden in ["sessionId", "path", "ownerRoot", "token"] {
        let mut wire = owner_return_event("alpha", "job-a", "seed-a").to_wire_value();
        wire[forbidden] = serde_json::json!("leak");
        let refusal = parse_event(&wire).expect_err("forbidden field must be refused");
        assert_eq!(
            refusal.reason,
            format!("forbidden field `{forbidden}`"),
            "the refusal must spell the forbidden field name"
        );
    }
    // Every other forbidden name is refused by name too.
    for forbidden in FORBIDDEN_WIRE_FIELDS {
        let mut wire = owner_return_event("alpha", "job-a", "seed-a").to_wire_value();
        wire[*forbidden] = serde_json::json!("leak");
        let refusal = parse_event(&wire).expect_err("forbidden field must be refused");
        assert_eq!(refusal.reason, format!("forbidden field `{forbidden}`"));
    }
}

// ── §8.4 idempotent import ────────────────────────────────────────────

#[test]
fn import_is_idempotent_on_origin_and_event_id() {
    let env = TestEnv::new();
    let first = owner_return_event("alpha", "job-a", "seed-a");
    let second = owner_return_event("alpha", "job-b", "seed-b");

    let batch = vec![first.clone(), second.clone()];
    let outcome = import_batch(env.path(), &batch).expect("first import");
    assert_eq!(outcome.accepted.len(), 2);
    assert!(outcome.duplicates.is_empty());
    assert!(outcome.refused.is_empty());
    assert_eq!(mailbox_message_count(env.path()), 2);

    // The same batch again is entirely duplicates and applies nothing twice.
    let again = import_batch(env.path(), &batch).expect("second import");
    assert!(again.accepted.is_empty());
    assert_eq!(again.duplicates.len(), 2);
    assert_eq!(mailbox_message_count(env.path()), 2);

    // A duplicate event inside a single batch is caught too.
    let intra = import_batch(env.path(), &[first.clone(), first.clone()]).expect("intra-batch");
    assert!(intra.accepted.is_empty());
    assert_eq!(intra.duplicates.len(), 2);
    assert_eq!(mailbox_message_count(env.path()), 2);

    // Same content, different messageId → still one logical return.
    let logical_duplicate = owner_return_event("alpha", "job-a", "seed-other");
    let outcome = import_batch(env.path(), &[logical_duplicate]).expect("logical dedupe");
    assert!(outcome.accepted.is_empty());
    assert_eq!(outcome.duplicates.len(), 1);
    assert_eq!(mailbox_message_count(env.path()), 2);
}

// ── §8.5 owner-return landing is messages/-only ───────────────────────

#[test]
fn owner_return_import_never_creates_holder_or_claim_state() {
    let env = TestEnv::new();
    let event = owner_return_event("alpha", "job-a", "seed-a");
    let outcome = import_batch(env.path(), &[event]).expect("import");
    assert_eq!(outcome.accepted.len(), 1);
    assert!(
        !outcome.consumed.contains(&outcome.accepted[0]),
        "an imported return is not consumed: it stays unclaimable"
    );

    let returns = env.path().join(".edda").join("returns");
    assert!(returns.join("messages").is_dir(), "messages/ is created");
    assert!(
        !returns.join("owners").exists(),
        "import must never create an owner binding"
    );
    assert!(
        !returns.join("claims").exists(),
        "import must never create a claim marker"
    );
}

#[test]
fn imported_owner_return_is_unclaimable_until_binding() {
    let env = TestEnv::new();
    let event = owner_return_event("alpha", "job-a", "seed-a");
    import_batch(env.path(), &[event]).expect("import");

    let returns = env.path().join(".edda").join("returns");
    // The pending return is visible — but with no holder record, the local
    // claim arbiter (which refuses a claim unless the session is the holder)
    // cannot succeed, and no claim marker exists.
    assert_eq!(mailbox_message_count(env.path()), 1);
    let owner_file = returns
        .join("owners")
        .join(format!("{}.json", sha("assistant/p")));
    assert!(
        !owner_file.exists(),
        "no holder record may exist before this machine binds the owner"
    );
    assert!(!returns.join("claims").exists());
    // Nothing in the written record names an origin session.
    let record_path = std::fs::read_dir(returns.join("messages"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&record_path).unwrap()).unwrap();
    assert_eq!(record["posted_by_session"], "");
    assert_eq!(record["origin"]["machine"], "alpha");
}

// ── §8.6 decision facts ───────────────────────────────────────────────

#[test]
fn decision_fact_conflict_imports_inactive() {
    let env = TestEnv::new();
    let first = decision_fact_event("alpha", "db.engine", "postgres", false);
    let conflicting = decision_fact_event("beta", "db.engine", "sqlite", false);

    let outcome = import_batch(env.path(), std::slice::from_ref(&first)).expect("first decision");
    assert_eq!(outcome.accepted.len(), 1);

    let outcome =
        import_batch(env.path(), std::slice::from_ref(&conflicting)).expect("conflict decision");
    assert_eq!(outcome.accepted.len(), 1);

    let ledger = Ledger::open(env.path()).expect("ledger");
    let active = ledger
        .find_active_decision("main", "db.engine")
        .unwrap()
        .expect("an active decision");
    assert_eq!(active.value, "postgres", "#394: merge, never overwrite");

    let imported = ledger
        .sqlite
        .decision_timeline("db.engine", None, None)
        .unwrap()
        .into_iter()
        .find(|row| row.source_event_id.as_deref() == Some(conflicting.event_id.as_str()))
        .expect("the conflicting import is recorded");
    assert!(!imported.is_active, "the conflicting import is inactive");
    assert_eq!(imported.source_project_id.as_deref(), Some("beta"));
    assert_eq!(imported.authority, "agent-a");
    assert_eq!(ledger.sqlite.count_decisions().unwrap(), 2);
}

#[test]
fn decision_fact_round_trip_preserves_provenance() {
    let env = TestEnv::new();
    let fact = decision_fact_event("docs", "coord.transport", "edda-node", true);
    let outcome = import_batch(env.path(), std::slice::from_ref(&fact)).expect("import");
    assert_eq!(outcome.accepted, vec![fact.event_id.clone()]);
    assert_eq!(outcome.consumed, vec![fact.event_id.clone()]);

    let ledger = Ledger::open(env.path()).expect("ledger");
    let row = ledger
        .sqlite
        .find_active_decision("main", "coord.transport")
        .unwrap()
        .expect("a decision visible to `edda ask`");
    assert_eq!(row.value, "edda-node");
    assert_eq!(row.authority, "agent-a", "original actor preserved");
    assert_eq!(
        row.source_project_id.as_deref(),
        Some("docs"),
        "source machine preserved"
    );
    let ratified = ledger.ratified_decisions_map().unwrap();
    let info = ratified
        .get(&row.event_id)
        .expect("the transported ratification is visible");
    assert_eq!(info.ratified_by, "agent-a");

    // An identical re-import is skipped, not duplicated.
    let outcome = import_batch(env.path(), &[fact]).expect("re-import");
    assert!(outcome.accepted.is_empty());
    assert_eq!(outcome.duplicates.len(), 1);
    assert_eq!(ledger.sqlite.count_decisions().unwrap(), 1);
}

// ── §8.7 work transitions ─────────────────────────────────────────────

#[test]
fn work_transition_record_does_not_touch_the_task_rail() {
    let env = TestEnv::new();
    let before: Vec<u64> =
        crate::tasks::project_tasks(&Ledger::open(env.path()).unwrap().iter_events().unwrap())
            .iter()
            .map(|task| task.task_id)
            .collect();

    let event = NodeEvent::new(NodeEventBody::WorkTransition(
        super::envelope::WorkTransition {
            work_id: "gh685".into(),
            from_state: "open".into(),
            to_state: "in_progress".into(),
            receipt_id: "receipt-1".into(),
            actor: "agent-a".into(),
            origin_machine: "alpha".into(),
            ts: "2026-09-14T00:00:00.000Z".into(),
        },
    ));
    let outcome = import_batch(env.path(), std::slice::from_ref(&event)).expect("import");
    assert_eq!(outcome.accepted.len(), 1);

    let transitions = node_dir().join("transitions.jsonl");
    let text = std::fs::read_to_string(&transitions).expect("transitions record");
    assert!(text.contains("gh685"));
    assert!(text.contains("\"originMachine\":\"alpha\""));

    let after: Vec<u64> =
        crate::tasks::project_tasks(&Ledger::open(env.path()).unwrap().iter_events().unwrap())
            .iter()
            .map(|task| task.task_id)
            .collect();
    assert_eq!(before, after, "the task rail must be unchanged");
    assert!(
        Ledger::open(env.path())
            .unwrap()
            .iter_events()
            .unwrap()
            .iter()
            .all(|e| !e.event_type.starts_with("task.")),
        "no task event may be created by a transported transition"
    );
}

fn node_dir() -> std::path::PathBuf {
    edda_store::store_root().join("node")
}

// ── §8.8 handover ─────────────────────────────────────────────────────

#[test]
fn handover_surfaces_pending_and_unknown_holder() {
    let env = TestEnv::new();
    let event = NodeEvent::new(NodeEventBody::Handover(super::envelope::Handover {
        owner: "assistant/p".into(),
        from_holder: "alpha/manager".into(),
        to_holder: "unknown".into(),
        from_machine: "alpha".into(),
        to_machine: "gamma".into(),
        note: "handing over".into(),
        ts: "2026-09-14T00:00:00.000Z".into(),
    }));
    let outcome = import_batch(env.path(), &[event]).expect("import");
    assert_eq!(outcome.accepted.len(), 1);

    let handover = read_handover().expect("handover record");
    let record = handover.get("assistant/p").expect("keyed by owner");
    assert_eq!(record["pending"], true);
    assert_eq!(record["holder"], "unknown");
    assert_eq!(record["machine"], "gamma");
    assert_eq!(record["fromMachine"], "alpha");
}

// ── §8.9 activation observations ──────────────────────────────────────

#[test]
fn activation_observation_keeps_origin_and_freshness() {
    let env = TestEnv::new();
    let observed_at = super::now_rfc3339_millis();
    let event = NodeEvent::new(NodeEventBody::ActivationObservation(
        ActivationObservation {
            machine: "alpha".into(),
            revision: "b2cb417".into(),
            observed_at: observed_at.clone(),
            origin_machine: "docs".into(),
        },
    ));
    let outcome = import_batch(env.path(), &[event]).expect("import");
    assert_eq!(outcome.accepted.len(), 1);

    let observations = read_observations().expect("observations");
    let observation = observations.get("alpha").expect("keyed by machine");
    assert_eq!(observation.revision, "b2cb417");
    assert_eq!(observation.origin_machine, "docs");
    assert_eq!(observation.observed_at, observed_at);
    assert!(
        !observation.is_stale(1.0),
        "a just-seen observation is fresh"
    );
    assert!(
        observation.is_stale(-1.0),
        "a negative threshold is always stale"
    );

    // A second observation updates the revision but keeps firstSeenAt.
    let first_seen = observation.first_seen_at.clone();
    let later = super::now_rfc3339_millis();
    let update = NodeEvent::new(NodeEventBody::ActivationObservation(
        ActivationObservation {
            machine: "alpha".into(),
            revision: "c3d4e5f".into(),
            observed_at: later,
            origin_machine: "docs".into(),
        },
    ));
    import_batch(env.path(), &[update]).expect("update");
    let observations = read_observations().expect("observations");
    let observation = observations.get("alpha").unwrap();
    assert_eq!(observation.revision, "c3d4e5f");
    assert_eq!(observation.first_seen_at, first_seen);
}

// ── outbound queue durability ─────────────────────────────────────────

#[test]
fn outbound_queue_is_durable_and_transitions_are_monotonic() {
    let _store = isolated_store();
    let event = owner_return_event("alpha", "job-a", "seed-a");

    {
        let queue = OutboundQueue::open("beta").expect("open");
        queue.enqueue(&event).expect("enqueue");
        queue.enqueue(&event).expect("idempotent enqueue");
        let status = queue.status().unwrap();
        assert_eq!(status.sent, 1);
        assert_eq!(status.pending, 1);
    }

    // Reopen as a fresh process would: the entry is still there.
    let queue = OutboundQueue::open("beta").expect("reopen");
    assert_eq!(queue.entries().unwrap().len(), 1);
    queue
        .mark_delivered(std::slice::from_ref(&event.event_id))
        .unwrap();
    assert_eq!(queue.status().unwrap().delivered, 1);
    assert_eq!(queue.status().unwrap().pending, 0);
    queue
        .mark_acked(std::slice::from_ref(&event.event_id))
        .unwrap();
    assert_eq!(queue.status().unwrap().acked, 1);
    // An ack is never demoted back to delivered.
    queue
        .mark_delivered(std::slice::from_ref(&event.event_id))
        .unwrap();
    assert_eq!(queue.status().unwrap().acked, 1);
    assert!(queue.status().unwrap().last_success_at.is_some());
}

// ── rev 3 contract: lane_request and receipt ──────────────────────────

fn lane_request_event(machine: &str, request_id: &str) -> NodeEvent {
    NodeEvent::new(NodeEventBody::LaneRequest(LaneRequest {
        request_id: request_id.into(),
        from_label: "alpha/lane".into(),
        to_label: "beta/lane".into(),
        message: "please run the scan".into(),
        ts: "2026-09-14T00:00:00.000Z".into(),
        origin_machine: machine.into(),
    }))
}

fn receipt_event(machine: &str, of_event_id: &str, of_kind: &str, state: &str) -> NodeEvent {
    NodeEvent::new(NodeEventBody::Receipt(Receipt {
        of_event_id: of_event_id.into(),
        of_kind: of_kind.into(),
        state: state.into(),
        actor: "beta/manager".into(),
        ts: "2026-09-14T00:00:00.000Z".into(),
        origin_machine: machine.into(),
    }))
}

#[test]
fn parse_event_accepts_lane_request_without_any_session() {
    let event = lane_request_event("alpha", "req-1");
    let parsed = parse_event(&event.to_wire_value()).expect("lane_request parses");
    assert_eq!(parsed, event);
    let wire = event.to_wire_value();
    assert!(wire.get("sessionId").is_none());
    assert!(wire.get("fromSession").is_none());
}

#[test]
fn parse_event_refuses_lane_request_session_id() {
    // `fromSession` is not an allowlisted field and is refused as unknown.
    let mut wire = lane_request_event("alpha", "req-1").to_wire_value();
    wire["fromSession"] = serde_json::json!("leak");
    assert_eq!(
        parse_event(&wire).unwrap_err().reason,
        "unknown field `fromSession`"
    );
    // `sessionId` is on the forbidden list and refused by name.
    let mut wire = lane_request_event("alpha", "req-1").to_wire_value();
    wire["sessionId"] = serde_json::json!("leak");
    assert_eq!(
        parse_event(&wire).unwrap_err().reason,
        "forbidden field `sessionId`"
    );
}

#[test]
fn parse_event_refuses_receipt_bad_state() {
    for state in ["done", "unknown", ""] {
        let mut wire =
            receipt_event("beta", &sha("sent-event"), "owner_return", "acked").to_wire_value();
        wire["state"] = serde_json::json!(state);
        let refusal = parse_event(&wire).expect_err("a bad receipt state must be refused");
        assert!(
            refusal.reason.contains("state"),
            "the refusal must name state: {}",
            refusal.reason
        );
    }
    // The two valid states parse.
    for state in ["delivered", "acked"] {
        let event = receipt_event("beta", &sha("sent-event"), "owner_return", state);
        assert_eq!(parse_event(&event.to_wire_value()).unwrap(), event);
    }
}

#[test]
fn receipt_updates_the_matching_queue_entry_state() {
    let env = TestEnv::new();
    let sent = owner_return_event("alpha", "job-a", "seed-a");
    let queue = OutboundQueue::open("beta").expect("open queue");
    queue.enqueue(&sent).expect("enqueue");
    assert_eq!(queue.status().unwrap().pending, 1);

    let receipt = receipt_event("beta", &sent.event_id, "owner_return", "acked");
    let outcome = import_batch(env.path(), std::slice::from_ref(&receipt)).expect("import receipt");
    assert_eq!(outcome.accepted, vec![receipt.event_id.clone()]);
    assert_eq!(queue.status().unwrap().acked, 1);
    assert_eq!(queue.status().unwrap().pending, 0);

    // A receipt for an unknown ofEventId is recorded as an observation and
    // refused as a no-op — never a panic and never a queue change.
    let unknown = receipt_event("beta", &sha("no-such-event"), "owner_return", "acked");
    let outcome = import_batch(env.path(), std::slice::from_ref(&unknown)).expect("import");
    assert_eq!(outcome.refused.len(), 1);
    assert!(outcome.refused[0].reason.contains("unknown ofEventId"));
    let observations = std::fs::read_to_string(node_dir().join("receipts.jsonl")).unwrap();
    assert!(observations.contains("\"matched\":false"));
    assert!(observations.contains("\"matched\":true"));
    assert_eq!(
        queue.status().unwrap().acked,
        1,
        "an unknown receipt is a no-op"
    );
}

#[test]
fn lane_request_is_returned_to_the_caller_not_applied() {
    let env = TestEnv::new();
    let event = lane_request_event("alpha", "req-1");

    let outcome = import_batch(env.path(), std::slice::from_ref(&event)).expect("import");
    assert!(outcome.accepted.is_empty());
    assert!(outcome.duplicates.is_empty());
    assert!(outcome.refused.is_empty());
    assert_eq!(outcome.lane_requests.len(), 1);
    assert_eq!(outcome.lane_requests[0].request_id, "req-1");
    assert_eq!(outcome.lane_request_ids, vec![event.event_id.clone()]);

    // Nothing was imported yet: a resend returns it again (at-least-once).
    let again = import_batch(env.path(), std::slice::from_ref(&event)).expect("resend");
    assert_eq!(again.lane_requests.len(), 1);

    // After the caller lands it and marks it imported, it is a duplicate.
    mark_imported(env.path(), std::slice::from_ref(&event.event_id)).expect("mark imported");
    let third = import_batch(env.path(), std::slice::from_ref(&event)).expect("third");
    assert_eq!(third.duplicates, vec![event.event_id.clone()]);
    assert!(third.lane_requests.is_empty());
}
