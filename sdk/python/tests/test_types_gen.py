# Generated-type probe: representative enum fields in the pinned schema
# corpus must generate as real literal unions (typing.Literal), never
# degrade to object, and keep their requiredness. The contract runner
# (../run-contract-tests.mjs) regenerates types_gen.py from the pinned spec
# and then runs this suite, so CI fails on any enum field that renders as
# Required[object] / object | None instead of a literal union.

import unittest
from typing import Literal, NoReturn, NotRequired, Required, get_args, get_origin

from edda_sdk import types_gen as t


def _unwrap(annotation):
    """Strip a Required/NotRequired wrapper, returning (origin, inner)."""
    origin = get_origin(annotation)
    if origin in (Required, NotRequired):
        return origin, get_args(annotation)[0]
    return None, annotation


def _literal_args(tp):
    """Return the Literal members of tp, or None if tp is not a Literal."""
    if get_origin(tp) is Literal:
        return get_args(tp)
    return None


def _variants(tp):
    """Return union members, or the one supplied type."""
    members = get_args(tp)
    return members if members else (tp,)


def _matches_typed_dict(value, holder):
    """Runtime presence/literal probe for generated discriminated variants."""
    if not holder.__required_keys__.issubset(value):
        return False
    for key, annotation in holder.__annotations__.items():
        _, inner = _unwrap(annotation)
        if key in value and inner is NoReturn:
            return False
        literals = _literal_args(inner)
        if key in value and literals is not None and value[key] not in literals:
            return False
    return True


class GeneratedEnumTypesTests(unittest.TestCase):
    def test_bare_enum_required_field_is_literal(self):
        # ingestion.triggerType — bare enum, required.
        origin, inner = _unwrap(t.IngestionPayload.__annotations__["triggerType"])
        self.assertIs(origin, Required)
        self.assertEqual(_literal_args(inner), ("auto", "suggested", "manual"))

    def test_bare_enum_required_layer_field_is_literal(self):
        # ingestion.sourceLayer — bare enum, required.
        origin, inner = _unwrap(t.IngestionPayload.__annotations__["sourceLayer"])
        self.assertIs(origin, Required)
        self.assertEqual(_literal_args(inner), ("L0", "L1", "L2", "L3", "L4", "L5"))

    def test_bare_enum_decision_field_is_literal(self):
        # verdict.recorded.decision — bare enum, required.
        origin, inner = _unwrap(t.VerdictRecordedPayload.__annotations__["decision"])
        self.assertIs(origin, Required)
        self.assertEqual(_literal_args(inner), ("approved", "rejected"))

    def test_bare_enum_nested_object_field_is_literal(self):
        # review_bundle.risk_assessment.level — bare enum nested in an object
        # property, required inside the nested TypedDict.
        origin, inner = _unwrap(t.ReviewBundlePayloadRiskAssessment.__annotations__["level"])
        self.assertIs(origin, Required)
        self.assertEqual(_literal_args(inner), ("low", "medium", "high", "critical"))

    def test_bare_enum_nested_array_item_field_is_literal(self):
        # review_bundle.risk_assessment.factors[].level — bare enum nested in
        # an array item schema.
        origin, inner = _unwrap(t.ReviewBundlePayloadRiskAssessmentFactorsItem.__annotations__["level"])
        self.assertIs(origin, Required)
        self.assertEqual(_literal_args(inner), ("low", "medium", "high", "critical"))

    def test_bare_enum_suggested_action_is_literal(self):
        # review_bundle.suggested_action — bare enum, required.
        origin, inner = _unwrap(t.ReviewBundlePayload.__annotations__["suggested_action"])
        self.assertIs(origin, Required)
        self.assertEqual(_literal_args(inner), ("approve", "review", "request_changes", "reject"))

    def test_anyof_wrapped_enum_is_literal_or_none_union(self):
        # decision_import.decision.scope — anyOf[enum, null] must render as
        # Literal[...] | None (typing.Optional), not object | None.
        origin, inner = _unwrap(t.DecisionImportPayloadDecision.__annotations__["scope"])
        self.assertIs(origin, NotRequired)
        members = get_args(inner)
        self.assertEqual(len(members), 2)
        self.assertEqual(_literal_args(members[0]), ("local", "shared", "global"))
        self.assertIs(members[1], type(None))

    def test_anyof_wrapped_enum_note_scope_is_literal_or_none_union(self):
        # note.decision.scope — the second anyOf-wrapped enum in the corpus.
        origin, inner = _unwrap(t.NotePayloadDecision.__annotations__["scope"])
        self.assertIs(origin, NotRequired)
        members = get_args(inner)
        self.assertEqual(len(members), 2)
        self.assertEqual(_literal_args(members[0]), ("local", "shared", "global"))
        self.assertIs(members[1], type(None))

    def test_const_fields_are_exact_literals(self):
        fields = [
            (t.ContinuityCapsulePayload, "data_authority", ("data_only",)),
            (
                t.ContinuityCapsulePayloadContinuity,
                "record_version",
                (1,),
            ),
            (
                t.ContinuityCapsulePayloadContinuity,
                "data_authority",
                ("data_only",),
            ),
            (
                t.ContinuityCapsulePayloadContinuityCapsule,
                "capsule_version",
                (1,),
            ),
        ]
        for holder, field, expected in fields:
            origin, inner = _unwrap(holder.__annotations__[field])
            self.assertIs(origin, Required, f"{holder.__name__}.{field}")
            self.assertEqual(
                _literal_args(inner), expected, f"{holder.__name__}.{field}"
            )

    def test_execution_brief_local_refs_and_literals_are_typed(self):
        origin, inner = _unwrap(t.ExecutionBriefPayload.__annotations__["trust"])
        self.assertIs(origin, Required)
        self.assertEqual(_literal_args(inner), ("locally_accepted",))
        profiles = set()
        for brief in _variants(t.ExecutionBriefPayloadExecutionBriefBrief):
            origin, inner = _unwrap(brief.__annotations__["runtime_profile"])
            self.assertIs(origin, Required)
            profiles.update(_literal_args(inner) or ())
        self.assertEqual(profiles, {"strong", "flash"})

    def test_task_session_dependent_requirements_are_discriminated(self):
        variants = _variants(t.TaskSessionPayload)
        self.assertEqual(len(variants), 3)
        controlled = [
            variant
            for variant in variants
            if "brief_event_id" in variant.__required_keys__
        ]
        self.assertEqual(len(controlled), 1)
        self.assertTrue(
            {
                "task_id", "agent_kind", "session_id", "attempt",
                "brief_event_id", "brief_digest", "lease_owner",
            }.issubset(controlled[0].__required_keys__)
        )
        for variant in variants:
            self.assertIn("task_id", variant.__required_keys__)
            if variant is not controlled[0]:
                for field in ("brief_event_id", "brief_digest", "lease_owner"):
                    origin, inner = _unwrap(variant.__annotations__[field])
                    self.assertIs(origin, NotRequired)
                    self.assertIs(inner, NoReturn)

        partial = {"task_id": 1, "brief_event_id": "evt_one"}
        self.assertFalse(any(_matches_typed_dict(partial, item) for item in variants))
        missing_session = {"task_id": 1}
        self.assertFalse(any(_matches_typed_dict(missing_session, item) for item in variants))
        complete = {
            "task_id": 1,
            "agent_kind": "acp:grok",
            "session_id": "session-one",
            "attempt": 1,
            "brief_event_id": "evt_one",
            "brief_digest": "a" * 64,
            "lease_owner": "owner",
        }
        self.assertTrue(any(_matches_typed_dict(complete, item) for item in variants))

    def test_flash_controller_requirement_is_a_discriminated_variant(self):
        variants = _variants(t.ExecutionBriefPayloadExecutionBriefBrief)
        flash_controllers = []
        for variant in variants:
            _, profile = _unwrap(variant.__annotations__["runtime_profile"])
            if _literal_args(profile) != ("flash",):
                continue
            _, procedure = _unwrap(variant.__annotations__["procedure"])
            if hasattr(procedure, "__required_keys__") and "authored_by" in procedure.__required_keys__:
                flash_controllers.append(procedure)
        self.assertEqual(len(flash_controllers), 2)
        required_work = {
            next(
                field
                for field in ("probe_cards", "implementation_steps")
                if field in procedure.__required_keys__
            )
            for procedure in flash_controllers
        }
        self.assertEqual(required_work, {"probe_cards", "implementation_steps"})

        empty = {
            "runtime_profile": "flash",
            "procedure": {"kind": "controller_authored", "authored_by": "controller"},
        }
        # Probe only the conditional keys: neither generated Flash/controller
        # variant accepts the schema-invalid empty procedure.
        self.assertFalse(
            any(
                _matches_typed_dict(empty["procedure"], procedure)
                for procedure in flash_controllers
            )
        )

    def test_attempt_bound_task_done_fields_are_typed(self):
        for field, expected in [
            ("attempt", int),
            ("session_id", str),
            ("brief_event_id", str),
            ("brief_digest", str),
            ("lease_owner", str),
            ("outcome_code", str),
        ]:
            origin, inner = _unwrap(
                t.TaskDonePayloadControlledCompletion.__annotations__[field]
            )
            self.assertIs(origin, Required)
            self.assertIs(inner, expected)

    def test_control_events_preserve_authority_provenance_and_commitments(self):
        manifest = t.ControlManifestPayloadControlManifest
        authority = t.ControlManifestPayloadControlManifestAuthority
        adjudication = t.ControlManifestPayloadControlManifestAdjudication
        intent = t.ControlIntentPayloadControlIntent
        receipt = t.ControlReceiptPayloadControlReceipt

        self.assertIn("authority", manifest.__required_keys__)
        self.assertIn("adjudication", manifest.__optional_keys__)
        for field in (
            "reason_code", "evidence", "prior_state_version", "prior_manifest_digest"
        ):
            self.assertIn(field, adjudication.__required_keys__)
        self.assertEqual(
            _literal_args(_unwrap(authority.__annotations__["permitted_action"])[1]),
            ("control_compile", "control_adjudicate"),
        )
        self.assertEqual(
            _literal_args(_unwrap(intent.__annotations__["action_kind"])[1]),
            ("complete", "needs_decision"),
        )
        self.assertEqual(
            _literal_args(_unwrap(receipt.__annotations__["next_state"])[1]),
            ("needs_decision", "completed"),
        )
        self.assertIn("action_token", intent.__required_keys__)
        self.assertIn("action_token", receipt.__required_keys__)

    def test_no_enum_field_degrades_to_object(self):
        # Sweep every annotation that is or wraps a Literal: none may also
        # expose a bare object member (the old degradation mode).
        for name, value in vars(t).items():
            if not hasattr(value, "__annotations__"):
                continue
            for field, annotation in value.__annotations__.items():
                origin, inner = _unwrap(annotation)
                members = get_args(inner) if get_origin(inner) is not None else (inner,)
                for member in members:
                    if _literal_args(member) is not None:
                        self.assertIsNot(
                            member, object, f"{name}.{field} degrades to object"
                        )

    def test_requiredness_preserved_on_literal_fields(self):
        # The enum fields above are required in their schemas; the literal
        # rewrite must not soften them to optional.
        for holder, field in [
            (t.IngestionPayload, "triggerType"),
            (t.IngestionPayload, "sourceLayer"),
            (t.VerdictRecordedPayload, "decision"),
            (t.ReviewBundlePayload, "suggested_action"),
        ]:
            origin, _ = _unwrap(holder.__annotations__[field])
            self.assertIs(origin, Required, f"{holder.__name__}.{field}")
        # anyOf-wrapped enums stay optional as the schema declares them.
        for holder, field in [
            (t.DecisionImportPayloadDecision, "scope"),
            (t.NotePayloadDecision, "scope"),
        ]:
            origin, _ = _unwrap(holder.__annotations__[field])
            self.assertIs(origin, NotRequired, f"{holder.__name__}.{field}")

    def test_existing_non_enum_unions_unchanged(self):
        # reason: anyOf[string, null] must stay str | None — the enum fix
        # must not alter neighbouring union shapes.
        origin, inner = _unwrap(t.DecisionImportPayloadDecision.__annotations__["reason"])
        self.assertIs(origin, NotRequired)
        members = get_args(inner)
        self.assertEqual(len(members), 2)
        self.assertIs(members[0], str)
        self.assertIs(members[1], type(None))


if __name__ == "__main__":
    unittest.main()
