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
  const [eventFamily, eventLevel] = classifyEventType(event.type);
  const staged = {
    ...event,
    ...(eventFamily === undefined ? {} : { event_family: eventFamily, event_level: eventLevel }),
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
  const identity = classifyIdentityFields(event);
  if (identity.tier === 'legacy') {
    // Legacy is reserved for an entirely absent identity group. A stripped or
    // malformed signed tuple must never downgrade to this path.
    return { ok: true, tier: 'legacy' };
  }
  if (identity.error) return { ok: false, reason: identity.error, tier: 'signed' };
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
    Buffer.from(event.sig.value, 'hex'),
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
    const expectedHash = computeEventHash(ev);
    const [family, level] = classifyEventType(ev.type);
    const expectedDigests = [{ alg: 'sha256', canon: 'edda-canon-v1', value: expectedHash }];
    if (ev.event_family !== family || ev.event_level !== level) {
      return { ok: false, reason: `taxonomy mismatch at ${ev.event_id}` };
    }
    if (ev.hash !== expectedHash || !sameJson(ev.digests, expectedDigests)) {
      return { ok: false, reason: `hash or digest mismatch at ${ev.event_id}` };
    }
    if ((ev.parent_hash ?? null) !== parent) {
      return { ok: false, reason: `parent_hash mismatch at ${ev.event_id}` };
    }
    parent = ev.hash;
  }
  return { ok: true };
}

/** Legacy is legal only when every signature-related envelope field is absent. */
function classifyIdentityFields(event) {
  const fields = ['actor_id', 'key_id', 'sig', 'actor_pubkey'];
  const present = fields.filter((field) => Object.hasOwn(event, field));
  if (present.length === 0) return { tier: 'legacy' };
  const required = ['actor_id', 'key_id', 'sig'];
  const missing = required.filter((field) => !Object.hasOwn(event, field));
  if (missing.length) return { tier: 'signed', error: `partial identity tuple: missing ${missing.join(', ')}` };
  if (typeof event.actor_id !== 'string' || event.actor_id.length === 0) return { tier: 'signed', error: 'malformed actor_id' };
  if (typeof event.key_id !== 'string' || !/^ek_[0-9a-f]{16}$/.test(event.key_id)) return { tier: 'signed', error: 'malformed key_id' };
  if (!isRecord(event.sig) || typeof event.sig.alg !== 'string' || typeof event.sig.value !== 'string' || !/^[0-9a-f]{128}$/.test(event.sig.value)) return { tier: 'signed', error: 'malformed signature' };
  if (Object.hasOwn(event, 'actor_pubkey') && (typeof event.actor_pubkey !== 'string' || !/^[0-9a-f]{64}$/.test(event.actor_pubkey))) return { tier: 'signed', error: 'malformed actor_pubkey' };
  return { tier: 'signed' };
}

function classifyEventType(type) {
  switch (type) {
    case 'note': case 'checkpoint': return ['signal', 'info'];
    case 'cmd': return ['signal', 'trace'];
    case 'commit': case 'merge': return ['milestone', 'milestone'];
    case 'rebuild': return ['admin', 'trace'];
    case 'branch_create': case 'branch_switch': case 'device_pair': case 'device_revoke': case 'task.requeued': return ['admin', 'info'];
    case 'approval': case 'approval_request': case 'approval_policy_match': case 'decision_import': case 'decision_ratify': case 'verdict.recorded': return ['governance', 'governance'];
    case 'task_intake': case 'agent_phase_change': case 'cycle_telemetry': case 'task.created': case 'task.started': case 'task.failed': return ['signal', 'info'];
    case 'review_bundle': case 'decide_snapshot': return ['governance', 'milestone'];
    case 'pr': case 'task.done': return ['milestone', 'milestone'];
    case 'task.session': return ['signal', 'trace'];
    default: return [undefined, undefined];
  }
}

function isRecord(value) { return value !== null && typeof value === 'object' && !Array.isArray(value); }
function sameJson(left, right) { return JSON.stringify(left) === JSON.stringify(right); }

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
  classifyIdentityFields,
  computeEventHash,
  generateKeyPair,
  keyIdFor,
  signEvent,
  signingInput,
  verifyChain,
  verifyEvent,
};
