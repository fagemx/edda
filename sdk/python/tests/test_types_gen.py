# Generated-type probe: representative enum fields in the pinned schema
# corpus must generate as real literal unions (typing.Literal), never
# degrade to object, and keep their requiredness. The contract runner
# (../run-contract-tests.mjs) regenerates types_gen.py from the pinned spec
# and then runs this suite, so CI fails on any enum field that renders as
# Required[object] / object | None instead of a literal union.

import unittest
from typing import Literal, NotRequired, Required, get_args, get_origin

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
