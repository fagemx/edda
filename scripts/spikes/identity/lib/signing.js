'use strict';
// Event hashing, Ed25519 signing/verification, and the trusted keyring —
// the executable sketch of docs/architecture/actor-signing.md (GH-609).
//
// Design invariants exercised by ../spike.js and ../test.js:
//
//   1. hash   = SHA-256(canon(event minus {hash, digests, schema_version, sig}))
//              — the legacy removal set (event.rs `finalize`) plus `sig`,
//              so the signature never feeds its own hash.
//   2. sig    = Ed25519_sign(privkey, canon(event minus {sig}))
//              — the signing input INCLUDES `hash`, so the signature binds
//              actor_id, key_id, and every payload byte through the hash.
//   3. verify = keyring-first. The event's (actor_id, key_id) MUST resolve to
//              a key in the operator-managed trusted keyring. An embedded
//              `actor_pubkey` is discovery metadata only and is NEVER trusted
//              for verification.
//   4. authority = role check. Signature validity proves authorship; only a
//              keyring role of "operator" may ratify. Agents cannot ratify
//              themselves even with a valid signature.
//   5. Unsigned events (no actor_id/key_id/sig) stay ledger-legal as the
//              legacy tier: hash-chain integrity holds, but they are
//              "recorded, unattributable" and carry no authority.

const crypto = require('node:crypto');
const { canonicalJsonBytes } = require('./canon');

const HASH_REMOVAL_SET = ['hash', 'digests', 'schema_version', 'sig'];

// ── hashing ──────────────────────────────────────────────────────────────────

/**
 * SHA-256 of the canonical form with the removal set dropped.
 * Mirrors event.rs `finalize` extended with `sig` (design §4.2).
 * @param {Record<string, unknown>} event
 * @returns {string} lowercase hex
 */
function computeEventHash(event) {
  const stripped = stripFields(event, HASH_REMOVAL_SET);
  return sha256Hex(canonicalJsonBytes(stripped));
}

/**
 * Canonical signing input: canon(event minus {sig}) — includes `hash`.
 * @param {Record<string, unknown>} event
 * @returns {Buffer}
 */
function signingInput(event) {
  return canonicalJsonBytes(stripFields(event, ['sig']));
}

// ── keys ─────────────────────────────────────────────────────────────────────

/** Generate an ephemeral Ed25519 keypair (development/testing only). */
function generateKeyPair() {
  const { publicKey, privateKey } = crypto.generateKeyPairSync('ed25519');
  return {
    publicKey,
    privateKey,
    /** raw 32-byte public key, hex */
    pubkeyHex: rawPublicBytes(publicKey).toString('hex'),
  };
}

/**
 * Content-addressed key id: `ek_` + first 16 hex chars of SHA-256(raw pubkey).
 * Deterministic across processes and languages (no RNG, no timestamp).
 * @param {string} pubkeyHex
 * @returns {string}
 */
function keyIdFor(pubkeyHex) {
  return 'ek_' + sha256Hex(Buffer.from(pubkeyHex, 'hex')).slice(0, 16);
}

/** @param {import('node:crypto').KeyObject} publicKey @returns {Buffer} */
function rawPublicBytes(publicKey) {
  const der = publicKey.export({ type: 'spki', format: 'der' });
  return der.subarray(der.length - 32);
}

// ── sign / verify ────────────────────────────────────────────────────────────

/**
 * Sign an event in place: fills `hash` and `sig`, returns the event.
 * The signature input excludes `sig` itself (invariant 2).
 * @param {Record<string, unknown>} event
 * @param {{ actorId: string, role: 'operator'|'agent', keypair: ReturnType<generateKeyPair> }} identity
 */
function signEvent(event, identity) {
  const { actorId, role, keypair } = identity;
  const staged = {
    ...event,
    actor_id: actorId,
    key_id: keyIdFor(keypair.pubkeyHex),
    // Advisory discovery metadata; NEVER used by verify (invariant 3).
    actor_pubkey: keypair.pubkeyHex,
  };
  staged.hash = computeEventHash(staged);
  staged.digests = [
    { alg: 'sha256', canon: 'edda-canon-v1', value: staged.hash },
  ];
  staged.sig = { alg: 'ed25519', value: '' }; // placeholder excluded from input
  const input = signingInput(staged);
  const signature = crypto.sign(null, input, keypair.privateKey);
  staged.sig = { alg: 'ed25519', value: signature.toString('hex') };
  return { event: staged, role };
}

/**
 * Keyring-first verification (invariant 3).
 *
 * Fails closed: an event that claims an (actor_id, key_id) identity must
 * resolve in the trusted keyring; a mismatched or missing entry is a hard
 * reject — the event's embedded `actor_pubkey` is ignored for trust.
 *
 * @param {Record<string, unknown>} event
 * @param {TrustedKeyring} keyring
 * @returns {{ ok: boolean, reason?: string, tier: 'signed'|'legacy' }}
 */
function verifyEvent(event, keyring) {
  if (!event.sig || !event.actor_id || !event.key_id) {
    // Legacy tier: unsigned events verify by hash chain only (caller runs
    // verifyChain); they are recorded but unattributable (invariant 5).
    return { ok: true, tier: 'legacy' };
  }
  if (event.sig.alg !== 'ed25519') {
    return { ok: false, reason: `unsupported sig alg ${event.sig.alg}`, tier: 'signed' };
  }
  const trusted = lookupTrustedKey(keyring, String(event.actor_id), String(event.key_id));
  if (!trusted) {
    return {
      ok: false,
      reason: `no trusted key ${event.key_id} for actor ${event.actor_id} — refusing to trust any embedded key`,
      tier: 'signed',
    };
  }
  const hashOk = computeEventHash(event) === event.hash;
  if (!hashOk) {
    return { ok: false, reason: 'hash mismatch — content altered', tier: 'signed' };
  }
  const sigOk = crypto.verify(
    null,
    signingInput(event),
    trusted.publicKey,
    Buffer.from(String(event.sig.value), 'hex'),
  );
  if (!sigOk) {
    return { ok: false, reason: 'signature does not verify under trusted key', tier: 'signed' };
  }
  return { ok: true, tier: 'signed', actorRole: trusted.role };
}

/**
 * Trusted keyring: actor_id → key_id → { publicKey, role }.
 * In production this is the operator-managed actor registry (`edda actor`
 * plus key verbs); here it is in-memory with ephemeral keys.
 */
class TrustedKeyring {
  constructor() {
    /** @type {Map<string, Map<string, {publicKey: import('node:crypto').KeyObject, role: 'operator'|'agent'}>>} */
    this.actors = new Map();
  }
  /**
   * @param {string} actorId
   * @param {string} pubkeyHex
   * @param {'operator'|'agent'} role
   */
  register(actorId, pubkeyHex, role) {
    const keyId = keyIdFor(pubkeyHex);
    if (!this.actors.has(actorId)) this.actors.set(actorId, new Map());
    const der = Buffer.concat([
      Buffer.from('302a300506032b6570032100', 'hex'),
      Buffer.from(pubkeyHex, 'hex'),
    ]);
    this.actors.get(actorId).set(keyId, {
      publicKey: crypto.createPublicKey({ key: der, format: 'der', type: 'spki' }),
      role,
    });
    return keyId;
  }
}

/**
 * @param {TrustedKeyring} keyring
 * @param {string} actorId
 * @param {string} keyId
 */
function lookupTrustedKey(keyring, actorId, keyId) {
  return keyring.actors.get(actorId)?.get(keyId) ?? null;
}

// ── authority (invariant 4) ──────────────────────────────────────────────────

/**
 * Ratify authorization = verification + operator-role check.
 * A valid agent signature verifies cryptographically but fails here: agents
 * cannot ratify themselves.
 * @param {Record<string, unknown>} ratifyEvent
 * @param {TrustedKeyring} keyring
 */
function authorizeRatify(ratifyEvent, keyring) {
  const verdict = verifyEvent(ratifyEvent, keyring);
  if (!verdict.ok) return { authorized: false, reason: verdict.reason };
  if (verdict.tier === 'legacy') {
    return { authorized: false, reason: 'unsigned ratify is legacy-tier — authority requires a verified operator signature' };
  }
  if (verdict.actorRole !== 'operator') {
    return { authorized: false, reason: `signing key role is '${verdict.actorRole}' — only operator keys may ratify (agent cannot ratify self)` };
  }
  return { authorized: true };
}

// ── chain (legacy integrity) ─────────────────────────────────────────────────

/**
 * Hash-chain walk (ledger.rs verify_chain, Node mirror). Verifies each
 * event's hash and the parent_hash linkage. This is the UNSIGNED baseline:
 * it proves ordering and completeness, never authorship.
 * @param {Array<Record<string, unknown>>} events
 */
function verifyChain(events) {
  let parent = null;
  for (const ev of events) {
    if (computeEventHash(ev) !== ev.hash) {
      return { ok: false, reason: `hash mismatch at ${ev.event_id}` };
    }
    if ((ev.parent_hash ?? null) !== parent) {
      return { ok: false, reason: `parent_hash mismatch at ${ev.event_id}` };
    }
    parent = ev.hash;
  }
  return { ok: true };
}

// ── helpers ──────────────────────────────────────────────────────────────────

/** @param {Record<string, unknown>} obj @param {string[]} fields */
function stripFields(obj, fields) {
  const out = { ...obj };
  for (const f of fields) delete out[f];
  return out;
}

/** @param {Buffer} bytes */
function sha256Hex(bytes) {
  return crypto.createHash('sha256').update(bytes).digest('hex');
}

module.exports = {
  HASH_REMOVAL_SET,
  TrustedKeyring,
  authorizeRatify,
  computeEventHash,
  generateKeyPair,
  keyIdFor,
  signEvent,
  signingInput,
  verifyChain,
  verifyEvent,
};
