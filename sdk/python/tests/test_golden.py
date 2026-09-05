# Golden tests: canonical vectors (spec/events/canonical-v1.json) and event
# hashes over the spec's golden fixture exports. The digest rule is
# re-implemented in Python — no Rust code, no shelling out.
#
# Sources (resolved in order):
#   EDDA_SPEC_DIR  — pinned spec checkout dir (contains spec/events/ and
#                    tests/fixtures/events/)
#   sdk/spec-pin/  — pinned copy created by generator/pin-spec.sh

import json
import os
import unittest
from pathlib import Path

from edda_sdk.canon import (
    canonicalize_text,
    compute_event_hash,
)

_HERE = Path(__file__).resolve().parent


def _spec_dir():
    env = os.environ.get("EDDA_SPEC_DIR")
    if env and Path(env).is_dir():
        return Path(env)
    pinned = _HERE.parent.parent / "spec-pin"
    if pinned.is_dir():
        return pinned
    return None


_SPEC = _spec_dir()


class CanonicalVectorTests(unittest.TestCase):
    @unittest.skipIf(_SPEC is None, "spec not pinned yet (waiting on controller handoff)")
    def test_canonical_vectors_from_spec(self):
        vectors = json.loads((_SPEC / "spec" / "events" / "canonical-v1.json").read_text(encoding="utf-8"))
        self.assertGreaterEqual(len(vectors), 5)
        for n, v in enumerate(vectors):
            self.assertEqual(canonicalize_text(v["input"]), v["canonical"], f"vector {n}: {v['input']}")

    def test_crafted_numeric_limit_vectors(self):
        # decimal/exponential switch at dec_exp in [-5, 15] (zmij FIXED_DEC_EXP f64)
        self.assertEqual(canonicalize_text("[1e15,1e16,1e-5,1e-6]"), "[1000000000000000.0,1e+16,0.00001,1e-6]")
        # f64 shortest roundtrip digits
        self.assertEqual(canonicalize_text("[0.1,0.3,1.234e33]"), "[0.1,0.3,1.234e+33]")
        # integer u64/i64 exactness; out-of-range falls back to f64
        self.assertEqual(
            canonicalize_text("[18446744073709551615,-9223372036854775808,18446744073709551616]"),
            "[18446744073709551615,-9223372036854775808,1.8446744073709552e+19]",
        )
        # -0 lexeme normalization vs float negative zero
        self.assertEqual(canonicalize_text("[-0,-0.0]"), "[0,-0.0]")
        # 1.00 normalizes (lexeme NOT raw-preserved)
        self.assertEqual(canonicalize_text("[1.00,1e2]"), "[1.0,100.0]")
        # string escape normalization: \u00e9 -> raw é; / unescaped; control shorthands; 中 raw
        self.assertEqual(
            canonicalize_text('["\\u00e9","\\u0000\\b\\t\\n\\f\\r\\u001f/\\"\\\\\\u4e2d"]'),
            '["é","\\u0000\\b\\t\\n\\f\\r\\u001f/\\"\\\\中"]',
        )
        # unicode scalar key order: U+0065 U+0301 < U+00E9 < U+E000 < U+10000
        self.assertEqual(
            canonicalize_text('{"\\ud800\\udc00":1,"\\ue000":2,"\\u00e9":3,"e\\u0301":4}'),
            '{"é":4,"é":3,"\ue000":2,"𐀀":1}'.encode("utf-8").decode("utf-8"),
        )
        # non-finite refused honestly
        with self.assertRaises(ValueError):
            canonicalize_text("[1e999]")

    def test_negative_and_malformed_json_match_rust_rejection_boundary(self):
        # serde_json (the Rust oracle) rejects each malformed JSON grammar
        # case; valid negative forms have their canonical outputs pinned here.
        self.assertEqual(canonicalize_text("[-1.5,-1e-7,-1e16]"), "[-1.5,-1e-7,-1e+16]")
        for raw in ("1 2", "01", "-", "1.", "1e", '"unterminated', '"\\u12', "[1,", "{\"a\":1"):
            with self.assertRaises((ValueError, IndexError), msg=raw):
                canonicalize_text(raw)


@unittest.skipIf(_SPEC is None, "spec not pinned yet (waiting on controller handoff)")
class GoldenFixtureTests(unittest.TestCase):
    def test_recomputed_event_hashes_match_pinned_digests(self):
        directory = _SPEC / "tests" / "fixtures" / "events"
        checked = 0
        for path in sorted(directory.glob("*.jsonl")):
            for line in path.read_text(encoding="utf-8").splitlines():
                if not line.strip():
                    continue
                event = json.loads(line)
                self.assertTrue(event.get("hash"), f"fixture {path.name}/{event.get('event_id')} has no hash")
                self.assertGreaterEqual(len(event.get("digests", [])), 1)
                recomputed = compute_event_hash(line.strip())
                self.assertEqual(recomputed, event["hash"], f"hash mismatch for {path.name}/{event.get('event_id')}")
                self.assertEqual(event["digests"][0]["value"], recomputed)
                self.assertEqual(event["digests"][0]["alg"], "sha256")
                self.assertEqual(event["digests"][0]["canon"], "edda-canon-v1")
                checked += 1
        self.assertGreater(checked, 0, "no golden fixtures found")


if __name__ == "__main__":
    unittest.main()
