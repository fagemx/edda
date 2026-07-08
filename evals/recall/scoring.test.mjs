// scoring.test.mjs — TDD for scoring.mjs
import { test } from "node:test";
import assert from "node:assert/strict";
import { rankOf, scoreOne, aggregate } from "./scoring.mjs";

test("rankOf: expected in list returns 1-based rank", () => {
  const list = [{ event_id: "a" }, { event_id: "b" }, { event_id: "c" }];
  assert.equal(rankOf("a", list), 1);
  assert.equal(rankOf("b", list), 2);
  assert.equal(rankOf("c", list), 3);
});

test("rankOf: not in list returns null", () => {
  assert.equal(rankOf("z", [{ event_id: "a" }]), null);
  assert.equal(rankOf("a", []), null);
});

test("scoreOne: rank 1 hits both", () => {
  const s = scoreOne(1);
  assert.equal(s.hit1, true);
  assert.equal(s.hit5, true);
});

test("scoreOne: rank 5 hits only @5", () => {
  const s = scoreOne(5);
  assert.equal(s.hit1, false);
  assert.equal(s.hit5, true);
});

test("scoreOne: rank 6 hits neither", () => {
  const s = scoreOne(6);
  assert.equal(s.hit1, false);
  assert.equal(s.hit5, false);
});

test("scoreOne: null miss hits neither", () => {
  const s = scoreOne(null);
  assert.equal(s.hit1, false);
  assert.equal(s.hit5, false);
  assert.equal(s.hit, null);
});

test("aggregate: skipped rows excluded from denominators (shrinking ledger honesty)", () => {
  const rows = [
    { hit1: true, hit5: true, hit: 1 },
    { hit1: false, hit5: true, hit: 3 },
    { skipped: true },
    { hit1: false, hit5: false, hit: null },
  ];
  const agg = aggregate(rows);
  assert.equal(agg.n, 3);
  assert.equal(agg.skipped, 1);
  assert.equal(agg.r_at_1, 1 / 3);
  assert.equal(agg.r_at_5, 2 / 3);
  assert.equal(agg.misses, 1);
});

test("aggregate: all-skipped honest zero (no divide-by-zero)", () => {
  const agg = aggregate([{ skipped: true }, { skipped: true }]);
  assert.equal(agg.n, 0);
  assert.equal(agg.r_at_1, null);
  assert.equal(agg.r_at_5, null);
});

test("aggregate: perfect run reports 1.0/1.0", () => {
  const rows = [
    { hit1: true, hit5: true, hit: 1 },
    { hit1: true, hit5: true, hit: 1 },
  ];
  const agg = aggregate(rows);
  assert.equal(agg.r_at_1, 1);
  assert.equal(agg.r_at_5, 1);
  assert.equal(agg.misses, 0);
});
