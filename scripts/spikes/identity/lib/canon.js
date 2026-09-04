'use strict';
// edda-canon-v1 canonical JSON — Node mirror of crates/edda-core/src/canon.rs.
//
// Rules (verified against the Rust implementation by the golden fixtures in
// ../fixtures/golden-events.json, whose `hash` values were produced by the
// actual Rust algorithm via the edda 0.4.0 binary):
//   * object keys sorted lexicographically, recursively
//   * arrays preserve order
//   * no whitespace (compact separators)
//   * scalars serialized as serde_json would (JSON.stringify matches for the
//     string/number/bool/null domain; unicode escapes differ in theory — see
//     docs/architecture/actor-signing.md §"Honest boundaries" for why golden
//     fixtures are the guard, not a prose description)

/**
 * Canonicalize a parsed-JSON value to edda-canon-v1 bytes.
 * @param {unknown} value
 * @returns {Buffer}
 */
function canonicalJsonBytes(value) {
  return Buffer.from(canonicalJsonString(value), 'utf8');
}

/**
 * Canonicalize to a string (same encoding as canonicalJsonBytes).
 * @param {unknown} value
 * @returns {string}
 */
function canonicalJsonString(value) {
  return serialize(value);
}

/** @param {unknown} v @returns {string} */
function serialize(v) {
  if (v === null || typeof v === 'boolean' || typeof v === 'number') {
    return JSON.stringify(v);
  }
  if (typeof v === 'string') {
    return JSON.stringify(v);
  }
  if (Array.isArray(v)) {
    return '[' + v.map(serialize).join(',') + ']';
  }
  if (typeof v === 'object') {
    const keys = Object.keys(v).sort();
    const body = keys.map((k) => JSON.stringify(k) + ':' + serialize(v[k])).join(',');
    return '{' + body + '}';
  }
  throw new TypeError('cannot canonicalize value of type ' + typeof v);
}

module.exports = { canonicalJsonBytes, canonicalJsonString };
