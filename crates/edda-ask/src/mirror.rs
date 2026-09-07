//! Cross-machine mirror provenance at the **read** end (GH-671).
//!
//! `edda sync --from-mirror` warns about a dead mirror at import time. That
//! signal dies with the command: once the rows are in the ledger, `edda ask`
//! on machine B renders a decision that arrived over a three-week-old mirror
//! exactly like one decided here this morning. The doneWhen clause this module
//! answers is "讀端過期時 `edda ask` 輸出有標示" — the marking belongs at the
//! read end, not only at the write end.
//!
//! Mechanism: the mirror import stamps `payload["mirror"]` on its
//! `decision_import` event (`edda_ledger::sync::make_import_event`), and the
//! imported row's `event_id` **is** that event's id, so a hit resolves its own
//! provenance with one `get_event`. Nothing is written at query time — the
//! same query-time-derivation contract [`crate::staleness`] follows.
//!
//! Staleness matches [`edda_ledger::sync::MirrorFreshness::is_stale`]: older
//! than the threshold, **or unreadable**. Unknown freshness must be visible,
//! never silently fresh.

use crate::DecisionHit;
use edda_ledger::sync::DEFAULT_MIRROR_STALE_HOURS;
use edda_ledger::Ledger;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// The mirror a decision arrived over, and how dead it was.
#[derive(Debug, Clone, Serialize)]
pub struct MirrorOrigin {
    /// Exporting machine from the mirror's `INDEX.md`, or the directory name
    /// when the stamp named none.
    pub machine: String,
    /// The mirror's `- **Exported at**:` stamp; absent when it was missing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exported_at: Option<String>,
    /// Age of that stamp in hours at query time; `None` when unparseable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_hours: Option<f64>,
    pub is_stale: bool,
    pub threshold_hours: i64,
}

/// Resolve each hit's mirror provenance. Non-mirror decisions map to `None`,
/// which is every decision in a single-machine project.
///
/// Best-effort by construction: a hit whose event cannot be read maps to
/// `None` rather than failing the query — an unreadable event is not evidence
/// that a decision came from a mirror.
pub fn origins_for_hits(ledger: &Ledger, hits: &[DecisionHit]) -> Vec<Option<MirrorOrigin>> {
    let now = OffsetDateTime::now_utc();
    hits.iter()
        .map(|h| {
            ledger
                .get_event(&h.event_id)
                .ok()
                .flatten()
                .and_then(|e| origin_from_payload(&e.payload, now))
        })
        .collect()
}

/// Annotate an in-memory hit list with the origins from [`origins_for_hits`].
pub fn annotate_hits(hits: &mut [DecisionHit], origins: &[Option<MirrorOrigin>]) {
    for (hit, origin) in hits.iter_mut().zip(origins.iter()) {
        hit.mirror = origin.clone();
    }
}

/// The pure half: read `payload["mirror"]` and age its stamp against `now`.
fn origin_from_payload(payload: &serde_json::Value, now: OffsetDateTime) -> Option<MirrorOrigin> {
    let mirror = payload.get("mirror")?.as_object()?;
    let machine = mirror
        .get("machine")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let exported_at = mirror
        .get("exported_at")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let age_hours = exported_at.as_deref().and_then(|ts| {
        OffsetDateTime::parse(ts, &Rfc3339)
            .ok()
            .map(|t| (now - t).as_seconds_f64() / 3600.0)
    });
    Some(MirrorOrigin {
        machine,
        exported_at,
        // Same rule as the import-time warning: unknown age is stale.
        is_stale: match age_hours {
            Some(h) => h >= DEFAULT_MIRROR_STALE_HOURS as f64,
            None => true,
        },
        age_hours,
        threshold_hours: DEFAULT_MIRROR_STALE_HOURS,
    })
}

/// The human line `edda ask` prints under a decision that rode a dead mirror.
pub fn stale_hint(origin: &MirrorOrigin) -> String {
    let age = match origin.age_hours {
        Some(h) => format!("{h:.1}h old"),
        None => "stamp missing or unreadable".to_string(),
    };
    format!(
        "⚠ stale-mirror hint: from {} — exported {} ({age}, threshold {}h). Re-export on the source machine and pull.",
        origin.machine,
        origin.exported_at.as_deref().unwrap_or("?"),
        origin.threshold_hours,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ts: &str) -> OffsetDateTime {
        OffsetDateTime::parse(ts, &Rfc3339).unwrap()
    }

    fn payload(exported_at: Option<&str>) -> serde_json::Value {
        match exported_at {
            Some(ts) => serde_json::json!({"mirror": {"machine": "4090", "exported_at": ts}}),
            None => serde_json::json!({"mirror": {"machine": "4090", "exported_at": null}}),
        }
    }

    #[test]
    fn a_decision_that_never_rode_a_mirror_has_no_origin() {
        // The single-machine case: every decision, and the reason the marker
        // does not become noise in a solo project.
        let local = serde_json::json!({"role": "system", "decision": {"key": "db.engine"}});
        assert!(origin_from_payload(&local, at("2026-09-07T00:00:00Z")).is_none());
    }

    #[test]
    fn a_fresh_mirror_is_not_marked() {
        let o = origin_from_payload(
            &payload(Some("2026-09-07T00:00:00Z")),
            at("2026-09-07T06:00:00Z"),
        )
        .expect("mirror payload");
        assert_eq!(o.machine, "4090");
        assert!(!o.is_stale, "6h < 24h threshold");
        assert!((o.age_hours.expect("parsed stamp") - 6.0).abs() < 0.001);
    }

    #[test]
    fn a_mirror_past_the_threshold_is_marked_stale() {
        let o = origin_from_payload(
            &payload(Some("2026-09-01T00:00:00Z")),
            at("2026-09-07T00:00:00Z"),
        )
        .expect("mirror payload");
        assert!(o.is_stale, "144h >= 24h threshold");
        assert!(stale_hint(&o).contains("4090"));
        assert!(stale_hint(&o).contains("144.0h old"));
    }

    #[test]
    fn exactly_at_the_threshold_is_stale() {
        // Boundary matches `MirrorFreshness::is_stale`: `>=`, not `>`.
        let o = origin_from_payload(
            &payload(Some("2026-09-06T00:00:00Z")),
            at("2026-09-07T00:00:00Z"),
        )
        .expect("mirror payload");
        assert!(o.is_stale);
    }

    #[test]
    fn an_unreadable_stamp_is_stale_not_silently_fresh() {
        // Death visibility: unknown freshness must be visible.
        let missing = origin_from_payload(&payload(None), at("2026-09-07T00:00:00Z"))
            .expect("mirror payload");
        assert!(missing.is_stale);
        assert!(missing.age_hours.is_none());
        assert!(stale_hint(&missing).contains("stamp missing or unreadable"));

        let garbage = origin_from_payload(
            &payload(Some("not-a-timestamp")),
            at("2026-09-07T00:00:00Z"),
        )
        .expect("mirror payload");
        assert!(garbage.is_stale);
        assert!(garbage.age_hours.is_none());
    }
}
