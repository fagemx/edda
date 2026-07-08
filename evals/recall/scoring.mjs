// scoring.mjs — deterministic scoring for decision recall eval (queue 325)
// Pure functions. Test with node --test scoring.test.mjs.

/**
 * Rank the expected event_id in a list of returned decisions.
 * @param {string} expectedEventId
 * @param {Array<{event_id:string}>} returned  Ordered top-N results.
 * @returns {number|null} 1-based rank, or null if not present.
 */
export function rankOf(expectedEventId, returned) {
  for (let i = 0; i < returned.length; i++) {
    if (returned[i]?.event_id === expectedEventId) return i + 1;
  }
  return null;
}

/**
 * Score a single query result.
 * @param {number|null} rank
 * @returns {{hit1: boolean, hit5: boolean, hit: number|null}}
 */
export function scoreOne(rank) {
  return {
    hit1: rank === 1,
    hit5: rank !== null && rank <= 5,
    hit: rank,
  };
}

/**
 * Aggregate across queries. Skipped queries (expected id not in ledger)
 * are excluded from denominators so a shrinking ledger doesn't lie about score.
 * @param {Array<{skipped?: boolean, hit1?: boolean, hit5?: boolean, hit?: number|null}>} rows
 */
export function aggregate(rows) {
  const scored = rows.filter((r) => !r.skipped);
  const n = scored.length;
  if (n === 0) return { n: 0, skipped: rows.length, r_at_1: null, r_at_5: null, misses: 0 };
  const hits1 = scored.filter((r) => r.hit1).length;
  const hits5 = scored.filter((r) => r.hit5).length;
  const misses = scored.filter((r) => r.hit === null).length;
  return {
    n,
    skipped: rows.length - n,
    r_at_1: hits1 / n,
    r_at_5: hits5 / n,
    misses,
  };
}
