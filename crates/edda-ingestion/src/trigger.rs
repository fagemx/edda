use crate::types::TriggerResult;

/// Evaluate an ingestion trigger based on event type and source layer.
///
/// Returns the appropriate `TriggerResult`:
/// - `AutoIngest` for events that should be written immediately
/// - `SuggestIngest` for events that need human review
/// - `Skip` for events that should be silently dropped
///
/// The trigger table implements the 25 rules from the ingestion triggers spec:
/// 9 auto-ingest, 8 suggest-ingest, 8 never-ingest (mapped to Skip).
/// Any unrecognized event_type/source_layer combination also returns Skip.
pub fn evaluate_trigger(event_type: &str, source_layer: &str) -> TriggerResult {
    match (event_type, source_layer) {
        // ── Auto-ingest (9) ──────────────────────────────────────────
        ("decision.commit", "L1") => TriggerResult::AutoIngest,
        ("decision.discard", "L1") => TriggerResult::AutoIngest,
        ("decision.promotion", "L1") => TriggerResult::AutoIngest,
        ("decision.rollback", "L1") => TriggerResult::AutoIngest,
        ("outcome.harmful", "L4") => TriggerResult::AutoIngest,
        ("runtime.rollback", "L4") => TriggerResult::AutoIngest,
        ("governance.patch.v1", "L4") => TriggerResult::AutoIngest,
        ("safety.violation", "L4") => TriggerResult::AutoIngest,
        ("design.type_change", "L2") => TriggerResult::AutoIngest,

        // ── Suggest-ingest (8) ───────────────────────────────────────
        ("route.changed", "L1") => TriggerResult::SuggestIngest {
            reason: "May indicate routing anti-pattern".to_string(),
        },
        ("probe.ambiguous", "L1") => TriggerResult::SuggestIngest {
            reason: "Possible false positive pattern".to_string(),
        },
        ("candidates.pruned_batch", "L1") => TriggerResult::SuggestIngest {
            reason: "Space-builder quality issue".to_string(),
        },
        ("outcome.inconclusive", "L4") => TriggerResult::SuggestIngest {
            reason: "Metrics may be wrong, not the change".to_string(),
        },
        ("change.repeated", "L4") => TriggerResult::SuggestIngest {
            reason: "Change may not be solving the problem".to_string(),
        },
        ("chief.escalation", "L4") => TriggerResult::SuggestIngest {
            reason: "Privilege creep worth reviewing".to_string(),
        },
        ("spec.patched_repeatedly", "L2" | "L3") => TriggerResult::SuggestIngest {
            reason: "Concept instability".to_string(),
        },
        ("track.suspended", "L2" | "L3") => TriggerResult::SuggestIngest {
            reason: "Downstream instability signal".to_string(),
        },

        // ── Never-ingest (8) + everything else ───────────────────────
        // Explicitly listed for documentation; all map to Skip:
        //   followup.draft, snapshot.update, candidate.ranking,
        //   probe.draft_iteration, spec.typo_fix, task.status_change,
        //   pulse.frame, cycle.completion
        _ => TriggerResult::Skip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Auto-ingest triggers ─────────────────────────────────────────

    #[test]
    fn auto_decision_commit() {
        assert_eq!(
            evaluate_trigger("decision.commit", "L1"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_decision_discard() {
        assert_eq!(
            evaluate_trigger("decision.discard", "L1"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_decision_promotion() {
        assert_eq!(
            evaluate_trigger("decision.promotion", "L1"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_decision_rollback() {
        assert_eq!(
            evaluate_trigger("decision.rollback", "L1"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_outcome_harmful() {
        assert_eq!(
            evaluate_trigger("outcome.harmful", "L4"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_runtime_rollback() {
        assert_eq!(
            evaluate_trigger("runtime.rollback", "L4"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_governance_patch() {
        assert_eq!(
            evaluate_trigger("governance.patch.v1", "L4"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_safety_violation() {
        assert_eq!(
            evaluate_trigger("safety.violation", "L4"),
            TriggerResult::AutoIngest
        );
    }

    #[test]
    fn auto_design_type_change() {
        assert_eq!(
            evaluate_trigger("design.type_change", "L2"),
            TriggerResult::AutoIngest
        );
    }

    // ── Suggest-ingest triggers ──────────────────────────────────────

    #[test]
    fn suggest_route_changed() {
        let result = evaluate_trigger("route.changed", "L1");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_probe_ambiguous() {
        let result = evaluate_trigger("probe.ambiguous", "L1");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_candidates_pruned_batch() {
        let result = evaluate_trigger("candidates.pruned_batch", "L1");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_outcome_inconclusive() {
        let result = evaluate_trigger("outcome.inconclusive", "L4");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_change_repeated() {
        let result = evaluate_trigger("change.repeated", "L4");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_chief_escalation() {
        let result = evaluate_trigger("chief.escalation", "L4");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_spec_patched_repeatedly_l2() {
        let result = evaluate_trigger("spec.patched_repeatedly", "L2");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_spec_patched_repeatedly_l3() {
        let result = evaluate_trigger("spec.patched_repeatedly", "L3");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_track_suspended_l2() {
        let result = evaluate_trigger("track.suspended", "L2");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_track_suspended_l3() {
        let result = evaluate_trigger("track.suspended", "L3");
        assert!(matches!(result, TriggerResult::SuggestIngest { .. }));
    }

    #[test]
    fn suggest_reasons_are_nonempty() {
        let suggest_cases = [
            ("route.changed", "L1"),
            ("probe.ambiguous", "L1"),
            ("candidates.pruned_batch", "L1"),
            ("outcome.inconclusive", "L4"),
            ("change.repeated", "L4"),
            ("chief.escalation", "L4"),
            ("spec.patched_repeatedly", "L2"),
            ("track.suspended", "L3"),
        ];
        for (event_type, layer) in suggest_cases {
            if let TriggerResult::SuggestIngest { reason } = evaluate_trigger(event_type, layer) {
                assert!(
                    !reason.is_empty(),
                    "reason for ({event_type}, {layer}) should not be empty"
                );
            } else {
                panic!("expected SuggestIngest for ({event_type}, {layer})");
            }
        }
    }

    // ── Never-ingest triggers ────────────────────────────────────────

    #[test]
    fn never_followup_draft() {
        assert_eq!(
            evaluate_trigger("followup.draft", "L1"),
            TriggerResult::Skip
        );
    }

    #[test]
    fn never_snapshot_update() {
        assert_eq!(
            evaluate_trigger("snapshot.update", "L1"),
            TriggerResult::Skip
        );
    }

    #[test]
    fn never_candidate_ranking() {
        assert_eq!(
            evaluate_trigger("candidate.ranking", "L1"),
            TriggerResult::Skip
        );
    }

    #[test]
    fn never_probe_draft_iteration() {
        assert_eq!(
            evaluate_trigger("probe.draft_iteration", "L1"),
            TriggerResult::Skip
        );
    }

    #[test]
    fn never_spec_typo_fix() {
        assert_eq!(evaluate_trigger("spec.typo_fix", "L2"), TriggerResult::Skip);
    }

    #[test]
    fn never_task_status_change() {
        assert_eq!(
            evaluate_trigger("task.status_change", "L3"),
            TriggerResult::Skip
        );
    }

    #[test]
    fn never_pulse_frame() {
        assert_eq!(evaluate_trigger("pulse.frame", "L4"), TriggerResult::Skip);
    }

    #[test]
    fn never_cycle_completion() {
        assert_eq!(
            evaluate_trigger("cycle.completion", "L4"),
            TriggerResult::Skip
        );
    }

    // ── Edge cases ───────────────────────────────────────────────────

    #[test]
    fn unknown_event_type_skips() {
        assert_eq!(evaluate_trigger("unknown.event", "L1"), TriggerResult::Skip);
    }

    #[test]
    fn right_event_wrong_layer_skips() {
        // decision.commit is auto-ingest for L1, but not L4
        assert_eq!(
            evaluate_trigger("decision.commit", "L4"),
            TriggerResult::Skip
        );
    }

    #[test]
    fn empty_inputs_skip() {
        assert_eq!(evaluate_trigger("", ""), TriggerResult::Skip);
    }
}
