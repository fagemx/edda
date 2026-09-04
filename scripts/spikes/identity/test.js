'use strict';
// GH-609 signing spike — assertion runner.
// Run: node scripts/spikes/identity/test.js   (exit 0 = all invariants hold)
//
// Uses only Node built-ins (node:crypto, node:test) — no new runtime or
// dev dependencies. All keys are ephemeral, generated in-process, and never
// written to disk. No operator keys or credentials are involved.

const assert = require('node:assert/strict');
const test = require('node:test');

const { canonicalJsonString } = require('./lib/canon');
const {
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
} = require('./lib/signing');
const { TEST_1, TEST_3, checkVector } = require('./lib/rfc8032');
const golden = require('./fixtures/golden-events.json');

const [GOLDEN_HEAD, GOLDEN_NOTE] = golden.events;

// ── Phase 0: crypto primitive matches RFC 8032 §7.1 ─────────────────────────

test('RFC 8032 §7.1 TEST 1: Node Ed25519 reproduces the RFC vector', () => {
  const r = checkVector(TEST_1);
  assert.equal(r.pubMatches, true, 'derived public key must match RFC');
  assert.equal(r.sigMatches, true, 'signature bytes must match RFC');
  assert.equal(r.verified, true, 'RFC signature must verify');
});

test('RFC 8032 §7.1 TEST 3: Node Ed25519 reproduces the RFC vector', () => {
  const r = checkVector(TEST_3);
  assert.equal(r.pubMatches, true);
  assert.equal(r.sigMatches, true);
  assert.equal(r.verified, true);
});

// ── Phase 1: Node reproduces the ACTUAL Rust canonical algorithm ────────────

test('golden event hashes produced by Rust (edda 0.4.0) reproduce in Node', () => {
  for (const ev of golden.events) {
    const recomputed = computeEventHash(ev);
    assert.equal(recomputed, ev.hash, `hash mismatch for ${ev.event_id}`);
    assert.equal(
      ev.digests[0].value,
      recomputed,
      'digests must carry the same hash value',
    );
  }
});

test('golden chain: parent_hash links head to note event', () => {
  assert.equal(GOLDEN_NOTE.parent_hash, GOLDEN_HEAD.hash);
  assert.deepEqual(verifyChain(golden.events), { ok: true });
});

test('canon matches the documented edda-canon-v1 rules', () => {
  // sorted keys, recursive, compact — mirrors crates/edda-core/src/canon.rs tests
  assert.equal(canonicalJsonString({ z: 1, a: 2, m: 3 }), '{"a":2,"m":3,"z":1}');
  assert.equal(
    canonicalJsonString({ b: { z: 1, a: 2 }, a: 1 }),
    '{"a":1,"b":{"a":2,"z":1}}',
  );
  assert.equal(canonicalJsonString({ a: [3, 1, 2] }), '{"a":[3,1,2]}');
});

// ── Phase 2: fail-first — forged author ACCEPTED by the unsigned baseline ──

test('FAIL-FIRST: forged author string passes unsigned hash-chain verification', () => {
  // The attacker rewrites who wrote the event, then recomputes the hash.
  // Under the current (unsigned) envelope that is the ONLY integrity check,
  // so the forgery is self-consistent: the chain proves "unmodified since
  // written" — and the attacker is the writer.
  const forged = {
    ...GOLDEN_NOTE,
    payload: {
      ...GOLDEN_NOTE.payload,
      role: 'operator', // impersonation: a note authored by 'user' claimed as operator
      text: 'operator approved GH-999',
    },
    event_id: 'evt_forged_by_attacker',
  };
  forged.hash = computeEventHash(forged);
  forged.digests = [{ alg: 'sha256', canon: 'edda-canon-v1', value: forged.hash }];

  // The forged event is hash-valid and chain-valid.
  assert.equal(computeEventHash(forged), forged.hash);
  assert.deepEqual(verifyChain([GOLDEN_HEAD, forged]), { ok: true });

  // And an unsigned verifier has NO reason to reject it — this is the gap.
  const verdict = verifyEvent(forged, new TrustedKeyring());
  assert.deepEqual(verdict, { ok: true, tier: 'legacy' });
});

// ── Phase 3: signed verification rejects the same forgery ───────────────────

test('SIGNED: forged-author event is rejected by signature verification', () => {
  const keyring = new TrustedKeyring();
  const operator = generateKeyPair();
  const agent = generateKeyPair();
  const opKeyId = keyring.register('operator-main', operator.pubkeyHex, 'operator');
  keyring.register('agent-fixture', agent.pubkeyHex, 'agent');

  const { event: signed } = signEvent(
    { ...GOLDEN_NOTE, payload: { ...GOLDEN_NOTE.payload, role: 'user' } },
    { actorId: 'operator-main', role: 'operator', keypair: operator },
  );
  assert.deepEqual(verifyEvent(signed, keyring), {
    ok: true,
    tier: 'signed',
    actorRole: 'operator',
  });

  // Same forgery as Phase 2, now against a signed event.
  const forged = {
    ...signed,
    payload: { ...signed.payload, role: 'operator', text: 'operator approved GH-999' },
  };
  // (even after re-forging a matching hash, the signature cannot follow)
  forged.hash = computeEventHash(forged);
  forged.digests = [{ alg: 'sha256', canon: 'edda-canon-v1', value: forged.hash }];
  const verdict = verifyEvent(forged, keyring);
  assert.equal(verdict.ok, false);
  assert.match(verdict.reason, /signature does not verify/);
  assert.equal(opKeyId, signed.key_id); // sanity: operator key was registered
});

// ── Phase 4: keyring-first — embedded attacker key is never trusted ─────────

test('SIGNED: attacker-supplied key + signature embedded in event is rejected', () => {
  const keyring = new TrustedKeyring();
  const operator = generateKeyPair();
  keyring.register('operator-main', operator.pubkeyHex, 'operator');

  const attacker = generateKeyPair();
  const { event: attackerSigned } = signEvent(
    { type: 'decision_ratify', branch: 'main', parent_hash: null, payload: { key: 'identity.signing', ratified_by: 'operator-main' } },
    { actorId: 'operator-main', role: 'operator', keypair: attacker }, // lies about actor
  );
  assert.equal(attackerSigned.actor_pubkey, attacker.pubkeyHex); // embeds own key

  const verdict = verifyEvent(attackerSigned, keyring);
  assert.equal(verdict.ok, false);
  assert.match(verdict.reason, /refusing to trust any embedded key/);
  assert.equal(
    keyIdFor(attacker.pubkeyHex),
    attackerSigned.key_id,
    'key_id is deterministic from the public key (content-addressed)',
  );
});

// ── Phase 5: signature excludes itself; hash excludes the signature ─────────

test('sig is outside its own signing input and outside the hash', () => {
  const keyring = new TrustedKeyring();
  const kp = generateKeyPair();
  keyring.register('operator-main', kp.pubkeyHex, 'operator');
  const { event } = signEvent({ type: 'note', branch: 'main', payload: { text: 'x' } }, {
    actorId: 'operator-main', role: 'operator', keypair: kp,
  });

  // hash removes the whole sig object — hash is identical with/without sig
  const withSig = { ...event };
  const withoutSig = { ...event };
  delete withoutSig.sig;
  assert.equal(computeEventHash(withSig), computeEventHash(withoutSig));
  assert.deepEqual(
    HASH_REMOVAL_SET.sort(),
    ['digests', 'hash', 'schema_version', 'sig'],
  );

  // signing input excludes sig but includes hash → signature binds the hash
  assert.equal(signingInput(event).includes(Buffer.from(event.hash)), true);
  assert.equal(JSON.parse(signingInput(event).toString()).sig, undefined);

  // re-signing is deterministic: same key + same content → same sig
  const resigned = cryptoSignTwice(event, kp);
  assert.equal(resigned, event.sig.value);
});

/** @returns {string} signature hex from signing the same input twice */
function cryptoSignTwice(event, kp) {
  const crypto = require('node:crypto');
  const sig = crypto.sign(null, signingInput(event), kp.privateKey).toString('hex');
  const sig2 = crypto.sign(null, signingInput(event), kp.privateKey).toString('hex');
  assert.equal(sig, sig2, 'Ed25519 is deterministic (RFC 8032 §4)');
  return sig;
}

// ── Phase 6: actor/key binding — hash and signature bind identity ───────────

test('actor/key binding: swapping identity between events breaks verification', () => {
  const keyring = new TrustedKeyring();
  const op = generateKeyPair();
  const agent = generateKeyPair();
  keyring.register('operator-main', op.pubkeyHex, 'operator');
  keyring.register('agent-fixture', agent.pubkeyHex, 'agent');

  const { event: evOp } = signEvent({ type: 'note', branch: 'main', payload: { text: 'op' } }, {
    actorId: 'operator-main', role: 'operator', keypair: op,
  });
  const { event: evAgent } = signEvent({ type: 'note', branch: 'main', payload: { text: 'ag' } }, {
    actorId: 'agent-fixture', role: 'agent', keypair: agent,
  });

  // Swap the claimed actor — the keyring resolves (actor, key_id) as a PAIR,
  // so the operator key_id under the agent actor fails closed; where the pair
  // does resolve, the signature would still fail. Either way: rejected.
  const swapped = { ...evOp, actor_id: 'agent-fixture' };
  swapped.hash = computeEventHash(swapped);
  const v = verifyEvent(swapped, keyring);
  assert.equal(v.ok, false);
  assert.match(v.reason, /no trusted key|signature does not verify/);

  // Swap the claimed key_id — fails closed on the keyring lookup.
  const keySwapped = { ...evOp, key_id: evAgent.key_id };
  keySwapped.hash = computeEventHash(keySwapped);
  const v2 = verifyEvent(keySwapped, keyring);
  assert.equal(v2.ok, false);
  assert.match(v2.reason, /no trusted key/);
});

// ── Phase 7: authority — agent cannot ratify self ────────────────────────────

test('agent-signed ratify verifies cryptographically but is NOT authorized', () => {
  const keyring = new TrustedKeyring();
  const op = generateKeyPair();
  const agent = generateKeyPair();
  keyring.register('operator-main', op.pubkeyHex, 'operator');
  keyring.register('agent-fixture', agent.pubkeyHex, 'agent');

  const ratifyPayload = { key: 'identity.signing', ratified_by: 'agent-fixture' };
  const { event: agentRatify } = signEvent(
    { type: 'decision_ratify', branch: 'main', payload: ratifyPayload },
    { actorId: 'agent-fixture', role: 'agent', keypair: agent },
  );

  // Cryptographic layer: perfectly valid signature by a known agent key.
  assert.deepEqual(verifyEvent(agentRatify, keyring), {
    ok: true, tier: 'signed', actorRole: 'agent',
  });

  // Authority layer: still rejected — agents cannot ratify themselves.
  const auth = authorizeRatify(agentRatify, keyring);
  assert.equal(auth.authorized, false);
  assert.match(auth.reason, /only operator keys may ratify/);

  // The same ratify signed by a trusted operator key IS authorized.
  const { event: opRatify } = signEvent(
    { type: 'decision_ratify', branch: 'main', payload: { key: 'identity.signing', ratified_by: 'operator-main' } },
    { actorId: 'operator-main', role: 'operator', keypair: op },
  );
  assert.deepEqual(authorizeRatify(opRatify, keyring), { authorized: true });
});

test('unsigned ratify is legacy-tier: no authority without a signature', () => {
  const keyring = new TrustedKeyring();
  const unsignedRatify = {
    type: 'decision_ratify', branch: 'main', parent_hash: null,
    payload: { key: 'identity.signing', ratified_by: 'operator-main' },
    hash: '', digests: [],
  };
  unsignedRatify.hash = computeEventHash(unsignedRatify);
  unsignedRatify.digests = [{ alg: 'sha256', canon: 'edda-canon-v1', value: unsignedRatify.hash }];
  const auth = authorizeRatify(unsignedRatify, keyring);
  assert.equal(auth.authorized, false);
  assert.match(auth.reason, /authority requires a verified operator signature/);
});

// ── Phase 8: legacy tier stays intact ───────────────────────────────────────

test('legacy unsigned events remain ledger-legal but carry no authority', () => {
  const keyring = new TrustedKeyring();
  // Golden events have no actor_id/key_id/sig — they verify as legacy tier.
  const verdict = verifyEvent(GOLDEN_NOTE, keyring);
  assert.deepEqual(verdict, { ok: true, tier: 'legacy' });
  // Mixed ledger: signed head + legacy tail both chain-verify.
  const op = generateKeyPair();
  const { event: signedHead } = signEvent(
    { type: 'note', branch: 'main', payload: { text: 'signed era begins' } },
    { actorId: 'operator-main', role: 'operator', keypair: op },
  );
  keyring.register('operator-main', op.pubkeyHex, 'operator');
  const legacy = {
    ...GOLDEN_NOTE,
    parent_hash: signedHead.hash,
  };
  legacy.hash = computeEventHash(legacy);
  legacy.digests = [{ alg: 'sha256', canon: 'edda-canon-v1', value: legacy.hash }];
  assert.deepEqual(verifyChain([signedHead, legacy]), { ok: true });
  assert.equal(verifyEvent(signedHead, keyring).tier, 'signed');
  assert.equal(verifyEvent(legacy, keyring).tier, 'legacy');
});
