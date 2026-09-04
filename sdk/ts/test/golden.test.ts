// Golden tests: canonical vectors (spec/events/canonical-v1.json) and event
// hashes over the spec's golden fixture exports. The digest rule is
// re-implemented in TypeScript — no Rust code, no shelling out.
//
// Sources (resolved in order):
//   EDDA_SPEC_DIR  — pinned spec checkout dir (contains spec/events/ and
//                    tests/fixtures/events/)
//   sdk/spec-pin/  — pinned copy created by generator/pin-spec.sh

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { canonicalizeText, computeEventHash } from "../src/canon.ts";

const here = dirname(fileURLToPath(import.meta.url));

function specDir(): string | null {
  const env = process.env.EDDA_SPEC_DIR;
  if (env && existsSync(env)) return env;
  const pinned = join(here, "..", "..", "spec-pin");
  if (existsSync(pinned)) return pinned;
  return null;
}

test("canonical vectors from spec/events/canonical-v1.json", { skip: specDir() === null && "spec not pinned yet (waiting on controller handoff)" }, () => {
  const vectors = JSON.parse(
    readFileSync(join(specDir()!, "spec", "events", "canonical-v1.json"), "utf8"),
  ) as Array<{ input: string; canonical: string }>;
  assert.ok(vectors.length >= 5, "expected the canonical vector set");
  for (const [n, v] of vectors.entries()) {
    assert.equal(canonicalizeText(v.input), v.canonical, `vector ${n}: ${v.input}`);
  }
});

test("crafted numeric-limit vectors match serde_json/zmij semantics", () => {
  // decimal/exponential switch at dec_exp in [-5, 15] (zmij FIXED_DEC_EXP f64)
  assert.equal(canonicalizeText("[1e15,1e16,1e-5,1e-6]"), "[1000000000000000.0,1e+16,0.00001,1e-6]");
  // f64 shortest roundtrip digits
  assert.equal(canonicalizeText("[0.1,0.3,1.234e33]"), "[0.1,0.3,1.234e+33]");
  // integer u64/i64 exactness; out-of-range falls back to f64
  assert.equal(
    canonicalizeText("[18446744073709551615,-9223372036854775808,18446744073709551616]"),
    "[18446744073709551615,-9223372036854775808,1.8446744073709552e+19]",
  );
  // -0 lexeme normalization vs float negative zero
  assert.equal(canonicalizeText("[-0,-0.0]"), "[0,-0.0]");
  // 1.00 normalizes (lexeme NOT raw-preserved)
  assert.equal(canonicalizeText("[1.00,1e2]"), "[1.0,100.0]");
  // string escape normalization: \u00e9 -> raw é; / unescaped; control shorthands; 中 raw
  assert.equal(
    canonicalizeText('["\\u00e9","\\u0000\\b\\t\\n\\f\\r\\u001f/\\"\\\\\\u4e2d"]'),
    '["é","\\u0000\\b\\t\\n\\f\\r\\u001f/\\"\\\\中"]',
  );
  // unicode scalar key order: U+0065 U+0301 < U+00E9 < U+E000 < U+10000
  assert.equal(
    canonicalizeText('{"\\ud800\\udc00":1,"\\ue000":2,"\\u00e9":3,"e\\u0301":4}'),
    '{"e\u0301":4,"\u00e9":3,"\ue000":2,"\ud800\udc00":1}',
  );
  // non-finite refused honestly
  assert.throws(() => canonicalizeText("[1e999]"), /out of range/);
});

test("golden fixtures: recomputed event hashes match pinned digests", { skip: specDir() === null && "spec not pinned yet (waiting on controller handoff)" }, async () => {
  const dir = join(specDir()!, "tests", "fixtures", "events");
  let checked = 0;
  for (const file of readdirSync(dir).filter((f) => f.endsWith(".jsonl"))) {
    const lines = readFileSync(join(dir, file), "utf8").split("\n").filter((l) => l.trim());
    for (const line of lines) {
      const event = JSON.parse(line);
      assert.ok(event.hash, `fixture ${file}/${event.event_id} has no hash`);
      assert.ok(Array.isArray(event.digests) && event.digests.length >= 1);
      const recomputed = await computeEventHash(line.trim());
      assert.equal(recomputed, event.hash, `hash mismatch for ${file}/${event.event_id}`);
      assert.equal(event.digests[0].value, recomputed, `digest mismatch for ${file}/${event.event_id}`);
      assert.equal(event.digests[0].alg, "sha256");
      assert.equal(event.digests[0].canon, "edda-canon-v1");
      checked++;
    }
  }
  assert.ok(checked > 0, "no golden fixtures found");
});
