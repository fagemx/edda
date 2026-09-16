//! Idempotent import engine and per-kind landing (frozen contract §3/§4/§5).
//!
//! Import dedupes on `(originMachine, eventId)` — the record lives in
//! `<store root>/node/imported.jsonl` — so a reconnect flush loses nothing and
//! applies nothing twice.
//!
//! Per kind:
//!
//! - [`OwnerReturn`] → the existing `cmd_return` mailbox `messages/` only.
//!   Import **never** creates or writes `owners/` or `claims/`, so an imported
//!   return shows up pending but stays unclaimable until this machine binds the
//!   owner locally. The local exactly-once claim arbiter is untouched.
//! - [`DecisionFact`] → the existing ledger import path via
//!   [`crate::sync::sync_from_records`], with `source_project_id =
//!   originMachine`. Same-key/different-value imports **inactive** (#394);
//!   identical re-imports are skipped.
//! - [`WorkTransition`] → an append-only record. It never creates or mutates a
//!   task rail entry.
//! - [`Handover`] → a keyed pending record.
//! - [`ActivationObservation`] → a keyed observation with origin and freshness.
//!
//! No wire field is ever used as a filesystem path, a shell argument or an
//! environment value. Nothing is written before an event has been fully
//! validated and refused events write nothing.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::envelope::{
    owner_return_logical_id, ActivationObservation, DecisionFact, EventRefusal, Handover,
    LaneRequest, NodeEvent, NodeEventBody, OwnerReturn, Receipt, WorkTransition,
};
use super::queue::{find_queue_peer_for_event, OutboundQueue};
use super::{node_store_dir, now_rfc3339_millis, write_json_atomic};

/// The result of one batch import.
#[derive(Debug, Default)]
pub struct ImportOutcome {
    /// Events newly applied (or newly recorded as applied).
    pub accepted: Vec<String>,
    /// Events already imported before this batch, or already present in the
    /// mailbox / ledger. Never re-applied.
    pub duplicates: Vec<String>,
    /// Per-event refusals. A refused event wrote nothing.
    pub refused: Vec<EventRefusal>,
    /// Events the receiving node recorded as consumed (fully applied). An
    /// `owner_return` is deliberately **not** consumed: it stays unclaimable
    /// until this machine binds the owner.
    pub consumed: Vec<String>,
    /// Validated `lane_request` bodies, in order, that are **not** applied by
    /// the ledger layer. The caller (the `edda-serve` node endpoint) lands
    /// them and then calls [`mark_imported`]. Until then they are neither
    /// `accepted` nor `duplicates`, so a crash causes a resend.
    pub lane_requests: Vec<LaneRequest>,
    /// The matching `eventId` for each entry of [`Self::lane_requests`].
    pub lane_request_ids: Vec<String>,
}

/// Import a batch of validated events into `repo_root`'s workspace.
pub fn import_batch(repo_root: &Path, events: &[NodeEvent]) -> Result<ImportOutcome> {
    let mut outcome = ImportOutcome::default();
    let mut imported = read_imported_keys()?;
    let mailbox = returns_mailbox(repo_root);
    let mut returned_lane_requests: HashSet<String> = HashSet::new();

    let mut ledger = None;
    if events
        .iter()
        .any(|event| matches!(event.body, NodeEventBody::DecisionFact(_)))
    {
        // A workspace without a ledger cannot accept a decision fact; that is a
        // per-event refusal below, not a batch failure.
        ledger = crate::Ledger::open(repo_root).ok();
    }

    for (index, event) in events.iter().enumerate() {
        let origin = event.origin_machine().to_string();
        let event_id = event.event_id.clone();
        if imported.contains(&origin, &event_id) {
            outcome.duplicates.push(event_id);
            continue;
        }
        // A lane request is validated and deduped here but landed by the
        // caller; the ledger layer must not write coordination events.
        if let NodeEventBody::LaneRequest(body) = &event.body {
            if returned_lane_requests.insert(event_id.clone()) {
                outcome.lane_requests.push(body.clone());
                outcome.lane_request_ids.push(event_id);
            }
            continue;
        }
        match apply_event(event, &mailbox, ledger.as_ref(), repo_root) {
            Ok(ApplyResult::Applied { consumed }) => {
                record_imported(&origin, &event_id)?;
                imported.insert(&origin, &event_id);
                outcome.accepted.push(event_id.clone());
                if consumed {
                    outcome.consumed.push(event_id);
                }
            }
            Ok(ApplyResult::Duplicate) => {
                // Record it so the peer's resend is a cheap duplicate too.
                record_imported(&origin, &event_id)?;
                imported.insert(&origin, &event_id);
                outcome.duplicates.push(event_id);
            }
            Err(error) => {
                outcome
                    .refused
                    .push(EventRefusal::new(error.to_string()).at(index));
            }
        }
    }

    Ok(outcome)
}

/// Record `event_ids` as imported after the caller has landed them (the
/// lane-request landing path). The origin is not known here; a lookup also
/// matches on `eventId` alone, which is safe because the canonical id already
/// includes `originMachine` (contract §3).
pub fn mark_imported(_repo_root: &Path, event_ids: &[String]) -> Result<()> {
    for event_id in event_ids {
        record_imported("", event_id)?;
    }
    Ok(())
}

enum ApplyResult {
    Applied { consumed: bool },
    Duplicate,
}

fn apply_event(
    event: &NodeEvent,
    mailbox: &Path,
    ledger: Option<&crate::Ledger>,
    repo_root: &Path,
) -> Result<ApplyResult> {
    match &event.body {
        NodeEventBody::OwnerReturn(body) => apply_owner_return(mailbox, event, body),
        NodeEventBody::DecisionFact(body) => {
            let Some(ledger) = ledger else {
                anyhow::bail!(
                    "no ledger at {} to import a decision fact",
                    repo_root.display()
                );
            };
            apply_decision_fact(ledger, event, body)
        }
        NodeEventBody::WorkTransition(body) => apply_work_transition(event, body),
        NodeEventBody::Handover(body) => apply_handover(event, body),
        NodeEventBody::ActivationObservation(body) => apply_observation(event, body),
        NodeEventBody::Receipt(body) => apply_receipt(event, body),
        NodeEventBody::LaneRequest(_) => {
            anyhow::bail!("lane_request is landed by the caller, not the ledger import engine")
        }
    }
}

// ── imported.jsonl ────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportedKeyRecord {
    origin_machine: String,
    event_id: String,
}

fn imported_path() -> PathBuf {
    node_store_dir().join("imported.jsonl")
}

/// Imported idempotency keys: the contract pair `(originMachine, eventId)`, plus
/// an id-only set so [`mark_imported`] (called by the lane-request landing path
/// with ids alone) is honoured on a later resend.
#[derive(Debug, Default)]
struct ImportedKeys {
    pairs: HashSet<(String, String)>,
    ids: HashSet<String>,
}

impl ImportedKeys {
    fn contains(&self, origin: &str, event_id: &str) -> bool {
        self.ids.contains(event_id)
            || self
                .pairs
                .contains(&(origin.to_string(), event_id.to_string()))
    }

    fn insert(&mut self, origin: &str, event_id: &str) {
        self.ids.insert(event_id.to_string());
        self.pairs
            .insert((origin.to_string(), event_id.to_string()));
    }
}

fn read_imported_keys() -> Result<ImportedKeys> {
    let path = imported_path();
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ImportedKeys::default())
        }
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let text = String::from_utf8(bytes)
        .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
    let mut keys = ImportedKeys::default();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let record: ImportedKeyRecord = serde_json::from_str(line)
            .with_context(|| format!("parse {} record", path.display()))?;
        if record.origin_machine.is_empty() {
            keys.ids.insert(record.event_id);
        } else {
            keys.insert(&record.origin_machine, &record.event_id);
        }
    }
    Ok(keys)
}

fn record_imported(origin: &str, event_id: &str) -> Result<()> {
    use std::io::Write;
    let path = imported_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let record = ImportedKeyRecord {
        origin_machine: origin.to_string(),
        event_id: event_id.to_string(),
    };
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(serde_json::to_string(&record)?.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

// ── owner_return → the existing mailbox messages/ ─────────────────────

/// Same resolution as `edda-cli`'s `cmd_return::mailbox_root`: `EDDA_RETURN_ROOT`
/// when set (workspace-pinned shared mailbox), else the workspace root.
fn returns_mailbox(repo_root: &Path) -> PathBuf {
    match std::env::var_os("EDDA_RETURN_ROOT") {
        Some(value) if !value.is_empty() => {
            let path = PathBuf::from(&value);
            if path.is_absolute() {
                path.join(".edda").join("returns")
            } else {
                repo_root.join(".edda").join("returns")
            }
        }
        _ => repo_root.join(".edda").join("returns"),
    }
}

/// A record shaped exactly like `cmd_return`'s `MessageRecord`.
#[derive(Serialize)]
struct MessageRecordOut {
    version: u32,
    id: String,
    owner: String,
    work: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deliverable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    posted_by_session: String,
    posted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<MessageOriginOut>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageOriginOut {
    logical_id: String,
    message_id: String,
    machine: String,
    exported_at: String,
}

/// Just enough of a stored record to recompute its logical identity, whatever
/// wrote it (`post` or a previous `replicate`/node import).
#[derive(Deserialize)]
struct StoredMessage {
    owner: String,
    work: String,
    status: String,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    deliverable: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

fn message_file(mailbox: &Path, id: &str) -> PathBuf {
    mailbox.join("messages").join(format!("{id}.json"))
}

fn lock_mailbox(mailbox: &Path) -> Result<std::fs::File> {
    use fs2::FileExt;
    // The same `.lock` file `cmd_return` uses, so a node import cannot race a
    // local claim. Creating messages/ and .lock is all import is allowed to do.
    std::fs::create_dir_all(mailbox.join("messages"))
        .with_context(|| format!("create {}", mailbox.join("messages").display()))?;
    let path = mailbox.join(".lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("lock {}", path.display()))?;
    Ok(file)
}

fn existing_logical_ids(mailbox: &Path) -> Result<HashSet<String>> {
    let dir = mailbox.join("messages");
    let mut ids = HashSet::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(ids),
        Err(error) => return Err(error).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let record: StoredMessage =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        ids.insert(owner_return_logical_id(
            &record.owner,
            &record.work,
            &record.status,
            record.result.as_deref(),
            record.deliverable.as_deref(),
            record.message.as_deref(),
        ));
    }
    Ok(ids)
}

fn apply_owner_return(
    mailbox: &Path,
    event: &NodeEvent,
    body: &OwnerReturn,
) -> Result<ApplyResult> {
    let _guard = lock_mailbox(mailbox)?;
    let mut seen = existing_logical_ids(mailbox)?;
    if seen.contains(&body.logical_id) {
        return Ok(ApplyResult::Duplicate);
    }

    let path = message_file(mailbox, &body.message_id);
    if path.exists() {
        let bytes = std::fs::read(&path).with_context(|| format!("read {}", path.display()))?;
        let existing: StoredMessage =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        let existing_logical = owner_return_logical_id(
            &existing.owner,
            &existing.work,
            &existing.status,
            existing.result.as_deref(),
            existing.deliverable.as_deref(),
            existing.message.as_deref(),
        );
        if existing_logical != body.logical_id {
            anyhow::bail!(
                "messages/{}.json already holds logical identity {existing_logical}, not {}",
                body.message_id,
                body.logical_id
            );
        }
        return Ok(ApplyResult::Duplicate);
    }

    let record = MessageRecordOut {
        version: 1,
        id: body.message_id.clone(),
        owner: body.owner.clone(),
        work: body.work.clone(),
        status: body.status.clone(),
        result: body.result.clone(),
        deliverable: body.deliverable.clone(),
        message: body.message.clone(),
        // No origin session is replicated: an imported return is never
        // attributable to the session that posted it elsewhere.
        posted_by_session: String::new(),
        posted_at: body.posted_at.clone(),
        origin: Some(MessageOriginOut {
            logical_id: body.logical_id.clone(),
            message_id: body.message_id.clone(),
            machine: body.origin_machine.clone(),
            exported_at: now_rfc3339_millis(),
        }),
    };
    write_json_atomic(&path, &record)?;
    seen.insert(body.logical_id.clone());
    let _ = event;
    // Not consumed: the return stays unclaimable until this machine binds the
    // owner. There is deliberately no `owners/` or `claims/` write here.
    Ok(ApplyResult::Applied { consumed: false })
}

// ── decision_fact → the ledger import path ────────────────────────────

fn apply_decision_fact(
    ledger: &crate::Ledger,
    event: &NodeEvent,
    body: &DecisionFact,
) -> Result<ApplyResult> {
    let record = crate::sync::DecisionRecord {
        event_id: event.event_id.clone(),
        fact: body.clone(),
    };
    let source_project_id = body.origin_machine.clone();
    let already = ledger
        .sqlite
        .is_already_imported(&source_project_id, &event.event_id)?;
    if already {
        return Ok(ApplyResult::Duplicate);
    }
    let target_project_id = edda_store::project_id(&ledger.paths.root);
    let result = crate::sync::sync_from_records(
        ledger,
        std::slice::from_ref(&record),
        &target_project_id,
        false,
    )?;
    if result.skipped > 0 {
        return Ok(ApplyResult::Duplicate);
    }
    Ok(ApplyResult::Applied { consumed: true })
}

// ── work_transition ───────────────────────────────────────────────────

fn transitions_path() -> PathBuf {
    node_store_dir().join("transitions.jsonl")
}

fn apply_work_transition(event: &NodeEvent, body: &WorkTransition) -> Result<ApplyResult> {
    use std::io::Write;
    let path = transitions_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let record = serde_json::json!({
        "eventId": event.event_id,
        "workId": body.work_id,
        "fromState": body.from_state,
        "toState": body.to_state,
        "receiptId": body.receipt_id,
        "actor": body.actor,
        "originMachine": body.origin_machine,
        "ts": body.ts,
        "importedAt": now_rfc3339_millis(),
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(serde_json::to_string(&record)?.as_bytes())?;
    file.write_all(b"\n")?;
    // Deliberately does not touch the task rail: a transported lifecycle fact is
    // a record, not a second task state.
    Ok(ApplyResult::Applied { consumed: true })
}

// ── handover ──────────────────────────────────────────────────────────

fn handover_path() -> PathBuf {
    node_store_dir().join("handover.json")
}

fn apply_handover(event: &NodeEvent, body: &Handover) -> Result<ApplyResult> {
    let path = handover_path();
    let mut map = read_json_map(&path)?;
    map.insert(
        body.owner.clone(),
        serde_json::json!({
            "eventId": event.event_id,
            "holder": body.to_holder,
            "machine": body.to_machine,
            "pending": true,
            "fromHolder": body.from_holder,
            "fromMachine": body.from_machine,
            "note": body.note,
            "ts": body.ts,
            "importedAt": now_rfc3339_millis(),
        }),
    );
    write_json_atomic(&path, &map)?;
    Ok(ApplyResult::Applied { consumed: true })
}

/// Read the keyed pending-handover record.
pub fn read_handover() -> Result<serde_json::Map<String, serde_json::Value>> {
    read_json_map(&handover_path())
}

// ── activation_observation ────────────────────────────────────────────

fn observations_path() -> PathBuf {
    node_store_dir().join("observations.json")
}

/// One machine's activation observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub revision: String,
    pub origin_machine: String,
    pub observed_at: String,
    pub first_seen_at: String,
}

impl Observation {
    /// Age in hours of `observedAt` (negative on clock skew), or `None` when
    /// unparseable. Unknown freshness must stay visible, never silently fresh.
    pub fn age_hours(&self) -> Option<f64> {
        let then = time::OffsetDateTime::parse(
            &self.observed_at,
            &time::format_description::well_known::Rfc3339,
        )
        .ok()?;
        Some((time::OffsetDateTime::now_utc() - then).whole_seconds() as f64 / 3600.0)
    }

    pub fn is_stale(&self, threshold_hours: f64) -> bool {
        self.age_hours().is_none_or(|age| age >= threshold_hours)
    }
}

fn apply_observation(_event: &NodeEvent, body: &ActivationObservation) -> Result<ApplyResult> {
    let path = observations_path();
    let mut map = read_json_map(&path)?;
    let first_seen_at = map
        .get(&body.machine)
        .and_then(|value| value.get("firstSeenAt"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(now_rfc3339_millis);
    map.insert(
        body.machine.clone(),
        serde_json::json!({
            "revision": body.revision,
            "originMachine": body.origin_machine,
            "observedAt": body.observed_at,
            "firstSeenAt": first_seen_at,
        }),
    );
    write_json_atomic(&path, &map)?;
    Ok(ApplyResult::Applied { consumed: true })
}

/// Read observations keyed by machine.
pub fn read_observations() -> Result<std::collections::BTreeMap<String, Observation>> {
    let path = observations_path();
    let map = read_json_map(&path)?;
    let mut out = std::collections::BTreeMap::new();
    for (machine, value) in map {
        let observation: Observation = serde_json::from_value(value)
            .with_context(|| format!("parse observation for '{machine}' in {}", path.display()))?;
        out.insert(machine, observation);
    }
    Ok(out)
}

fn read_json_map(path: &Path) -> Result<serde_json::Map<String, serde_json::Value>> {
    match std::fs::read(path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

// ── receipt → the local outbound queue ────────────────────────────────

fn receipts_path() -> PathBuf {
    node_store_dir().join("receipts.jsonl")
}

/// Apply a receipt against the local outbound queue entry it names. A receipt
/// for an unknown `ofEventId` is refused with nothing written — no
/// `receipts.jsonl` line, never a panic and never a silent state change.
fn apply_receipt(event: &NodeEvent, body: &Receipt) -> Result<ApplyResult> {
    match find_queue_peer_for_event(&body.of_event_id)? {
        Some(peer) => {
            let queue = OutboundQueue::open(&peer)?;
            match body.state.as_str() {
                "acked" => queue.mark_acked(std::slice::from_ref(&body.of_event_id))?,
                // `delivered` is validated to be the only other value.
                _ => queue.mark_delivered(std::slice::from_ref(&body.of_event_id))?,
            }
            record_receipt_observation(event, body, &peer, true)?;
            Ok(ApplyResult::Applied { consumed: true })
        }
        None => {
            // Frozen contract §3: a refused event writes nothing. An unmatched
            // receipt is refused without a `receipts.jsonl` line, so a refusal
            // is never partially applied.
            anyhow::bail!("receipt for unknown ofEventId {}", body.of_event_id)
        }
    }
}

fn record_receipt_observation(
    event: &NodeEvent,
    body: &Receipt,
    matched_peer: &str,
    matched: bool,
) -> Result<()> {
    use std::io::Write;
    let path = receipts_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let record = serde_json::json!({
        "eventId": event.event_id,
        "ofEventId": body.of_event_id,
        "ofKind": body.of_kind,
        "state": body.state,
        "actor": body.actor,
        "originMachine": body.origin_machine,
        "ts": body.ts,
        "matched": matched,
        "matchedPeer": matched_peer,
        "recordedAt": now_rfc3339_millis(),
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(serde_json::to_string(&record)?.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}

// ── revision ──────────────────────────────────────────────────────────

/// Where a revision observation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionOrigin {
    /// The workspace's own git HEAD.
    RepoHead,
    /// No revision could be observed.
    None,
}

impl RevisionOrigin {
    /// The wire/human spelling (`"repo-head"` / `"none"`).
    pub fn as_str(self) -> &'static str {
        match self {
            RevisionOrigin::RepoHead => "repo-head",
            RevisionOrigin::None => "none",
        }
    }
}

/// A local revision observation from the workspace: git HEAD, else `None`
/// (rendered as `null`, never invented).
///
/// This deliberately does **not** read `EDDA_PI_RELEASE_ID`: that is a Pi
/// package release id, not the edda revision, and substituting it is a false
/// fact in a managed-Pi shell (contract §7).
pub fn local_revision_origin(repo_root: &Path) -> (Option<String>, RevisionOrigin) {
    match git_head(repo_root) {
        Some(sha) => (Some(sha), RevisionOrigin::RepoHead),
        None => (None, RevisionOrigin::None),
    }
}

fn git_head(repo_root: &Path) -> Option<String> {
    let dot_git = repo_root.join(".git");
    let git_dir = if dot_git.is_dir() {
        dot_git
    } else {
        let text = std::fs::read_to_string(&dot_git).ok()?;
        let dir = text.strip_prefix("gitdir:")?.trim();
        let dir = if Path::new(dir).is_absolute() {
            PathBuf::from(dir)
        } else {
            repo_root.join(dir)
        };
        dir
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        let ref_path = git_dir.join(reference);
        if let Ok(value) = std::fs::read_to_string(ref_path) {
            return short_sha(value.trim());
        }
        return None;
    }
    short_sha(head)
}

fn short_sha(value: &str) -> Option<String> {
    if value.is_empty() {
        return None;
    }
    Some(value.chars().take(7).collect())
}
