'use strict';
// edda-canon-v1's deliberately supported Node subset. The source contract and
// byte vectors are #608's spec/events/canonical-v1.json. Rust sorts UTF-8
// Unicode scalar values; JavaScript's default UTF-16 sort is not equivalent.
//
// This spike accepts no JSON Number values. A parsed JavaScript Number cannot
// faithfully retain Rust's i64/u64/f64 domain (notably 1.0, -0.0, exponents,
// and 64-bit boundaries), so accepting it would falsely claim parity. Numeric
// vectors are retained in test.js as proof of the intentionally rejected
// domain. Production needs a representation that preserves those Rust values.

/** @param {unknown} value @returns {Buffer} */
function canonicalJsonBytes(value) {
  return Buffer.from(canonicalJsonString(value), 'utf8');
}

/** @param {unknown} value @returns {string} */
function canonicalJsonString(value) {
  return serialize(value);
}

/** Compare strings in Unicode scalar (UTF-8) order, as Rust String::cmp. */
function compareUnicodeScalars(left, right) {
  const a = Array.from(left);
  const b = Array.from(right);
  for (let i = 0; i < Math.min(a.length, b.length); i += 1) {
    const delta = a[i].codePointAt(0) - b[i].codePointAt(0);
    if (delta !== 0) return delta;
  }
  return a.length - b.length;
}

/** @param {unknown} v @returns {string} */
function serialize(v) {
  if (v === null || typeof v === 'boolean' || typeof v === 'string') {
    return JSON.stringify(v);
  }
  if (typeof v === 'number') {
    throw new TypeError('edda-canon-v1 Node spike rejects JSON Number values; use Rust canonical vectors');
  }
  if (Array.isArray(v)) return '[' + v.map(serialize).join(',') + ']';
  if (typeof v === 'object') {
    const keys = Object.keys(v).sort(compareUnicodeScalars);
    return '{' + keys.map((k) => JSON.stringify(k) + ':' + serialize(v[k])).join(',') + '}';
  }
  throw new TypeError('cannot canonicalize value of type ' + typeof v);
}

module.exports = { canonicalJsonBytes, canonicalJsonString, compareUnicodeScalars };
