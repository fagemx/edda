//! Durable outbound queue, one file per peer (frozen contract §5).
//!
//! `<store root>/node/queue/<peer-alias>.jsonl`, one JSON object per line. The
//! queue survives a process or machine restart: nothing is lost and a failed
//! send is never deleted — the peer dedupes a resend.
//!
//! States:
//!
//! - `sent` — queued and (re)sent, awaiting a 2xx from the peer;
//! - `delivered` — the peer returned 2xx for that `eventId`;
//! - `acked` — the receiving side recorded consumption.
//!
//! The file is bounded by [`MAX_QUEUE_ENTRIES`]; a queue that would grow past
//! the bound fails closed instead of growing without limit.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use super::envelope::NodeEvent;
use super::{now_rfc3339_millis, write_bytes_atomic, write_json_atomic};

/// Documented maximum entries per peer. Above this the queue fails closed.
pub const MAX_QUEUE_ENTRIES: usize = 10_000;

/// Queue entry state.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum QueueState {
    Sent,
    Delivered,
    Acked,
}

/// One durable queue entry.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueueEntry {
    pub event_id: String,
    pub kind: String,
    /// The [`super::envelope::NodeEventBody`]-shaped value (no `eventId`/`kind`).
    pub payload: serde_json::Value,
    pub state: QueueState,
    pub attempts: u32,
    pub first_seen_at: String,
    pub last_attempt_at: String,
}

impl QueueEntry {
    /// Rebuild the wire event value from this entry: the body payload plus the
    /// stored `eventId` and `kind`. The receiver re-parses and re-validates it.
    pub fn to_wire_value(&self) -> serde_json::Value {
        let mut object = match &self.payload {
            serde_json::Value::Object(object) => object.clone(),
            _ => serde_json::Map::new(),
        };
        object.insert(
            "eventId".to_string(),
            serde_json::Value::String(self.event_id.clone()),
        );
        object.insert(
            "kind".to_string(),
            serde_json::Value::String(self.kind.clone()),
        );
        serde_json::Value::Object(object)
    }
}

/// Aggregate counts for one peer's queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerQueueStatus {
    /// Entries still in `sent` (not yet delivered).
    pub pending: usize,
    /// Every event the sender ever queued for this peer.
    pub sent: usize,
    pub delivered: usize,
    pub acked: usize,
    pub last_success_at: Option<String>,
}

/// A per-peer durable outbound queue.
#[derive(Debug)]
pub struct OutboundQueue {
    peer: String,
    path: PathBuf,
}

impl OutboundQueue {
    /// `<store root>/node/queue/`.
    pub fn queue_dir() -> PathBuf {
        super::node_store_dir().join("queue")
    }

    /// Open (or create on first write) the queue for `peer`.
    ///
    /// `peer` must be a bare machine label: a name that contains a path
    /// separator, `..`, `:` or an absolute path is refused before any file is
    /// touched. Defence in depth: the resolved path's parent must be the queue
    /// directory, so no wire value can ever name a file elsewhere.
    pub fn open(peer: &str) -> Result<Self> {
        super::validate_machine_label(peer).with_context(|| {
            format!("refusing to open an outbound queue for non-label peer '{peer}'")
        })?;
        let dir = Self::queue_dir();
        let path = dir.join(format!("{peer}.jsonl"));
        if path.parent() != Some(dir.as_path()) {
            bail!(
                "refusing to open outbound queue path outside {}: {}",
                dir.display(),
                path.display()
            );
        }
        Ok(Self {
            peer: peer.to_string(),
            path,
        })
    }

    pub fn peer(&self) -> &str {
        &self.peer
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Every entry currently on disk, in file order.
    pub fn entries(&self) -> Result<Vec<QueueEntry>> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| format!("read {}", self.path.display()))
            }
        };
        let text = String::from_utf8(bytes)
            .with_context(|| format!("{} is not valid UTF-8", self.path.display()))?;
        let mut entries = Vec::new();
        for (line_number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: QueueEntry = serde_json::from_str(line).with_context(|| {
                format!("parse {} line {}", self.path.display(), line_number + 1)
            })?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Enqueue `event`, idempotent on `eventId` (a repeat is a no-op).
    pub fn enqueue(&self, event: &NodeEvent) -> Result<()> {
        let mut entries = self.entries()?;
        if entries.iter().any(|entry| entry.event_id == event.event_id) {
            return Ok(());
        }
        if entries.len() >= MAX_QUEUE_ENTRIES {
            bail!(
                "outbound queue for peer '{}' is at its {MAX_QUEUE_ENTRIES}-entry bound; refusing to grow",
                self.peer
            );
        }
        let stamp = now_rfc3339_millis();
        entries.push(QueueEntry {
            event_id: event.event_id.clone(),
            kind: event.kind().to_string(),
            payload: event.body.to_value(),
            state: QueueState::Sent,
            attempts: 0,
            first_seen_at: stamp.clone(),
            last_attempt_at: stamp,
        });
        self.rewrite(&entries)
    }

    /// Entries still waiting for a 2xx from the peer.
    pub fn pending(&self) -> Result<Vec<QueueEntry>> {
        Ok(self
            .entries()?
            .into_iter()
            .filter(|entry| entry.state == QueueState::Sent)
            .collect())
    }

    /// Mark events delivered (a 2xx from the peer). Ids not present are ignored.
    pub fn mark_delivered(&self, event_ids: &[String]) -> Result<()> {
        self.transition(event_ids, QueueState::Delivered)
    }

    /// Mark events acked (the receiving side recorded consumption).
    pub fn mark_acked(&self, event_ids: &[String]) -> Result<()> {
        self.transition(event_ids, QueueState::Acked)
    }

    /// Record a failed delivery attempt: the entry stays `sent`, its attempt
    /// count grows, and the caller may back off. Nothing is deleted.
    pub fn mark_attempt(&self, event_ids: &[String]) -> Result<()> {
        let mut entries = self.entries()?;
        let stamp = now_rfc3339_millis();
        for entry in &mut entries {
            if event_ids.contains(&entry.event_id) {
                entry.attempts = entry.attempts.saturating_add(1);
                entry.last_attempt_at = stamp.clone();
            }
        }
        self.rewrite(&entries)
    }

    fn transition(&self, event_ids: &[String], state: QueueState) -> Result<()> {
        let mut entries = self.entries()?;
        let stamp = now_rfc3339_millis();
        for entry in &mut entries {
            if event_ids.contains(&entry.event_id) {
                // Monotonic: acked is never demoted back to delivered.
                if state == QueueState::Delivered && entry.state == QueueState::Acked {
                    continue;
                }
                entry.state = state;
                entry.attempts = entry.attempts.saturating_add(1);
                entry.last_attempt_at = stamp.clone();
            }
        }
        self.rewrite(&entries)
    }

    /// Counts by state plus the most recent successful transition time.
    pub fn status(&self) -> Result<PeerQueueStatus> {
        let entries = self.entries()?;
        let mut status = PeerQueueStatus {
            pending: 0,
            sent: entries.len(),
            delivered: 0,
            acked: 0,
            last_success_at: None,
        };
        for entry in &entries {
            match entry.state {
                QueueState::Sent => status.pending += 1,
                QueueState::Delivered => status.delivered += 1,
                QueueState::Acked => status.acked += 1,
            }
            if entry.state != QueueState::Sent {
                let newer = status
                    .last_success_at
                    .as_deref()
                    .is_none_or(|current| entry.last_attempt_at.as_str() > current);
                if newer {
                    status.last_success_at = Some(entry.last_attempt_at.clone());
                }
            }
        }
        Ok(status)
    }

    /// Whole-file atomic rewrite. Bounded by [`MAX_QUEUE_ENTRIES`].
    fn rewrite(&self, entries: &[QueueEntry]) -> Result<()> {
        if entries.len() > MAX_QUEUE_ENTRIES {
            bail!(
                "outbound queue for peer '{}' would exceed its {MAX_QUEUE_ENTRIES}-entry bound",
                self.peer
            );
        }
        let mut text = String::new();
        for entry in entries {
            text.push_str(&serde_json::to_string(entry)?);
            text.push('\n');
        }
        write_bytes_atomic(&self.path, text.as_bytes())
    }
}

/// Convenience for one peer's aggregate status without holding the queue open.
pub fn peer_queue_status(peer: &str) -> Result<PeerQueueStatus> {
    OutboundQueue::open(peer)?.status()
}

/// Find the peer whose outbound queue holds `event_id`, if any. Used to apply a
/// transported `receipt` to the entry it names.
pub fn find_queue_peer_for_event(event_id: &str) -> Result<Option<String>> {
    let dir = OutboundQueue::queue_dir();
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Some(peer) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        // A stray file whose stem is not a machine label is skipped, never
        // opened: the queue directory is not a place a wire value may name.
        if super::validate_machine_label(peer).is_err() {
            continue;
        }
        let queue = OutboundQueue::open(peer)?;
        if queue
            .entries()?
            .iter()
            .any(|entry| entry.event_id == event_id)
        {
            return Ok(Some(peer.to_string()));
        }
    }
    Ok(None)
}

/// Persist one peer's last-seen observation (`<store root>/node/peers.json`),
/// read back by `edda node status` and `GET /api/node/status`.
pub fn record_peer_observation(
    peer: &str,
    reachable: bool,
    reason: &str,
    observed_at: &str,
) -> Result<()> {
    super::validate_machine_label(peer)
        .with_context(|| format!("refusing to record an observation for peer '{peer}'"))?;
    let path = super::node_store_dir().join("peers.json");
    let mut map: serde_json::Map<String, serde_json::Value> = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("parse peer observations {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::Map::new(),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    map.insert(
        peer.to_string(),
        serde_json::json!({
            "reachable": reachable,
            "reason": reason,
            "lastSeenAt": observed_at,
        }),
    );
    write_json_atomic(&path, &map)
}

/// The persisted last-seen observations, keyed by peer alias.
pub fn peer_observations() -> Result<serde_json::Map<String, serde_json::Value>> {
    let path = super::node_store_dir().join("peers.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("parse peer observations {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}
