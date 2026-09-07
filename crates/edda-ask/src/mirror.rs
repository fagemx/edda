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

/// The only event type that can carry `payload["mirror"]`.
///
/// `edda_ledger::sync::make_import_event` is the sole writer of that stamp and
/// always writes it onto a `decision_import` event, which is what makes "this
/// ledger has no `decision_import` events" a sound proxy for "no hit here can
/// have mirror provenance". A future writer that stamps some other event type
/// has to be taught to this constant too.
const MIRROR_STAMP_EVENT_TYPE: &str = "decision_import";

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
    // `edda ask` runs this twice per query (decisions + timeline) and once more
    // per project under `--fleet`, so the per-hit `get_event` below is paid ~2N
    // times — for a field that is `None` on every row of a single-machine
    // project, because such a project has no `decision_import` events for a
    // lookup to find. One index-backed probe (`idx_events_type`) answers that
    // for the whole list, so N primary-key lookups collapse into one seek that
    // reads no rows at all. A ledger that does have imports takes exactly the
    // per-hit path it took before.
    if hits.is_empty() || !may_hold_mirror_origins(ledger) {
        return vec![None; hits.len()];
    }
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

/// Whether a per-hit lookup could find any provenance at all.
///
/// Same best-effort rule as the lookups it guards: a probe that could not be
/// read answers `true`, because failing to read the ledger is not evidence that
/// nothing arrived over a mirror. The cost of being wrong that way is the
/// per-hit path that ran before this short-circuit existed.
fn may_hold_mirror_origins(ledger: &Ledger) -> bool {
    match ledger.iter_events_by_type(MIRROR_STAMP_EVENT_TYPE) {
        Ok(imports) => !imports.is_empty(),
        Err(_) => true,
    }
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
    use edda_core::event::{finalize_event, new_note_event};
    use edda_core::Event;
    use edda_ledger::ledger::{init_branches_json, init_head, init_workspace};
    use edda_ledger::paths::EddaPaths;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// A real on-disk ledger — this workspace does not mock internal crates,
    /// and the short-circuit below is a claim about what SQLite holds.
    fn setup() -> Ledger {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let tmp = std::env::temp_dir().join(format!("edda_ask_mirror_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let paths = EddaPaths::discover(&tmp);
        init_workspace(&paths).unwrap();
        init_head(&paths, "main").unwrap();
        init_branches_json(&paths, "main").unwrap();
        Ledger::open(&tmp).unwrap()
    }

    fn append(ledger: &Ledger, event: &Event) -> String {
        let mut chained = event.clone();
        chained.parent_hash = ledger.last_event_hash().unwrap();
        finalize_event(&mut chained).unwrap();
        ledger.append_event(&chained).unwrap();
        chained.event_id
    }

    fn note(text: &str) -> Event {
        new_note_event("main", None, "system", text, &[]).unwrap()
    }

    /// A `decision_import` carrying the stamp `sync::make_import_event` writes.
    fn mirror_import(machine: &str, exported_at: &str) -> Event {
        let mut e = note("[sync] imported db.engine=sqlite");
        e.event_type = "decision_import".to_string();
        e.payload["mirror"] = serde_json::json!({
            "machine": machine,
            "exported_at": exported_at,
        });
        e
    }

    fn hit(event_id: &str) -> DecisionHit {
        DecisionHit {
            event_id: event_id.to_string(),
            key: "db.engine".to_string(),
            value: "sqlite".to_string(),
            reason: String::new(),
            domain: "db".to_string(),
            branch: "main".to_string(),
            ts: "2026-09-07T00:00:00Z".to_string(),
            is_active: true,
            governance: crate::DecisionGovernance::default(),
            tags: Vec::new(),
            village_id: None,
            staleness: None,
            mirror: None,
        }
    }

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

    /// The half of the short-circuit that must NOT fire: a ledger holding a
    /// mirror import still resolves provenance one hit at a time. The
    /// locally-decided row beside it stays `None`, so a passing probe cannot be
    /// mistaken for a blanket "everything here came over a mirror".
    #[test]
    fn a_ledger_with_a_mirror_import_annotates_exactly_the_imported_hit() {
        let ledger = setup();
        let local = append(&ledger, &note("decided here this morning"));
        let imported = append(&ledger, &mirror_import("4090", "2026-01-01T00:00:00Z"));

        let mut hits = vec![hit(&imported), hit(&local), hit("evt_not_in_this_ledger")];
        let origins = origins_for_hits(&ledger, &hits);
        annotate_hits(&mut hits, &origins);

        let o = hits[0].mirror.as_ref().expect("the import carries a stamp");
        assert_eq!(o.machine, "4090");
        assert_eq!(o.exported_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert!(o.is_stale, "a stamp from 2026-01-01 is long past 24h");
        assert!(
            hits[1].mirror.is_none(),
            "a locally-decided row in a mirror-fed ledger did not ride a mirror"
        );
        assert!(
            hits[2].mirror.is_none(),
            "an event that cannot be read is not evidence of a mirror"
        );
    }

    /// The single-machine case the short-circuit exists for. The `None`s here
    /// have to be the same `None`s the per-hit path produced, so the test pins
    /// both halves: the events really are in the ledger (a lookup would have
    /// found them and still answered `None`), and there is no
    /// `decision_import` for one to find.
    #[test]
    fn a_ledger_with_no_mirror_import_answers_none_for_every_hit() {
        let ledger = setup();
        let local = append(&ledger, &note("decided here this morning"));

        assert!(
            ledger.get_event(&local).unwrap().is_some(),
            "the row is present, so `None` below is the short-circuit's answer \
             and not a lookup that missed"
        );
        assert!(
            ledger
                .iter_events_by_type(MIRROR_STAMP_EVENT_TYPE)
                .unwrap()
                .is_empty(),
            "the condition the short-circuit keys on"
        );

        let mut hits = vec![hit(&local), hit("evt_not_in_this_ledger")];
        let origins = origins_for_hits(&ledger, &hits);
        assert_eq!(
            origins.len(),
            hits.len(),
            "one answer per hit, short-circuit or not — `annotate_hits` zips \
             the two and would silently drop the tail"
        );
        assert!(origins.iter().all(Option::is_none));
        annotate_hits(&mut hits, &origins);
        assert!(hits.iter().all(|h| h.mirror.is_none()));

        assert!(
            origins_for_hits(&ledger, &[]).is_empty(),
            "no hits, no probe: a query that matched nothing did no ledger \
             work before this short-circuit and must do none after"
        );
    }
}
