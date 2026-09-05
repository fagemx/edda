'use strict';
// GH-609 signing spike — narrative demo (same checks as test.js, told as a
// story). Run: node scripts/spikes/identity/spike.js
//
// Demonstrates, in order:
//   A. the crypto primitive matches RFC 8032 §7.1 (primary source)
//   B. Node reproduces golden event hashes from the ACTUAL Rust algorithm
//   C. FAIL-FIRST: a forged author string is accepted by the unsigned
//      baseline (hash-chain-only verification)
//   D. the same forgery is REJECTED once events are signed and verification
//      is keyring-first
//   E. agent cannot ratify self — signature validity ≠ authority

const { checkVector, TEST_1, TEST_3 } = require('./lib/rfc8032');
const {
  TrustedKeyring,
  authorizeRatify,
  computeEventHash,
  generateKeyPair,
  signEvent,
  verifyChain,
  verifyEvent,
} = require('./lib/signing');
const golden = require('./fixtures/golden-events.json');

const line = '─'.repeat(72);
const fail = (msg) => {
  console.error('SPIKE FAILURE: ' + msg);
  process.exitCode = 1;
};

console.log(line);
console.log('GH-609 signing spike — ephemeral keys only, no operator credentials');
console.log(line);

// ── A. RFC 8032 conformance ──────────────────────────────────────────────────
console.log('\nA. Ed25519 primitive vs RFC 8032 §7.1 test vectors');
for (const [name, tv] of [['TEST 1', TEST_1], ['TEST 3', TEST_3]]) {
  const r = checkVector(tv);
  console.log(
    `   ${name}: pubkey ${r.pubMatches ? 'MATCH' : 'MISMATCH'}, ` +
      `signature ${r.sigMatches ? 'MATCH' : 'MISMATCH'}, verify ${r.verified ? 'OK' : 'FAIL'}`,
  );
  if (!r.pubMatches || !r.sigMatches || !r.verified) fail(`RFC 8032 ${name}`);
}

// ── B. golden events from the actual Rust algorithm ─────────────────────────
console.log('\nB. Golden events (hashes produced by edda 0.4.0 Rust binary)');
for (const ev of golden.events) {
  const h = computeEventHash(ev);
  const ok = h === ev.hash;
  console.log(`   ${ev.event_id}: ${ok ? 'MATCH' : 'MISMATCH'} ${h.slice(0, 16)}…`);
  if (!ok) fail(`golden hash mismatch for ${ev.event_id}`);
}
console.log(`   chain: ${verifyChain(golden.events).ok ? 'OK' : 'BROKEN'}`);

// ── C. fail-first: unsigned baseline accepts a forged author ────────────────
console.log('\nC. FAIL-FIRST — unsigned baseline (today\'s envelope)');
const note = golden.events[1];
const forged = {
  ...note,
  payload: {
    ...note.payload,
    role: 'operator',
    text: 'operator approved GH-999',
  },
  event_id: 'evt_forged_by_attacker',
};
forged.hash = computeEventHash(forged);
forged.digests = [{ alg: 'sha256', canon: 'edda-canon-v1', value: forged.hash }];
const baselineVerdict = verifyEvent(forged, new TrustedKeyring());
console.log(`   forged payload:  role='${forged.payload.role}' text='${forged.payload.text}'`);
console.log(`   hash chain     : ${verifyChain([golden.events[0], forged]).ok ? 'ACCEPTED (self-consistent)' : 'rejected'}`);
console.log(`   baseline check : ${baselineVerdict.ok ? 'ACCEPTED — identity is a claim, integrity proves nothing about authorship' : 'rejected'}`);
if (!baselineVerdict.ok) fail('unsigned baseline should accept the forged author');

// ── D. signed verification rejects the same forgery ─────────────────────────
console.log('\nD. Signed envelope + keyring-first verification');
const keyring = new TrustedKeyring();
const operator = generateKeyPair();
const agent = generateKeyPair();
keyring.register('operator-main', operator.pubkeyHex, 'operator');
keyring.register('agent-fixture', agent.pubkeyHex, 'agent');

const unsignedNote = { ...note };
delete unsignedNote.schema_version; // Node spike rejects JSON Number values.
const { event: signed } = signEvent(
  { ...unsignedNote, payload: { ...note.payload, role: 'user' } },
  { actorId: 'operator-main', role: 'operator', keypair: operator },
);
const sv = verifyEvent(signed, keyring);
console.log(`   genuine signed event : ${sv.ok ? `VERIFIED (tier=${sv.tier}, actor=${signed.actor_id}, key=${signed.key_id})` : 'REJECTED'}`);
if (!sv.ok) fail('genuine signed event must verify');

const forgedSigned = {
  ...signed,
  payload: { ...signed.payload, role: 'operator', text: 'operator approved GH-999' },
};
forgedSigned.hash = computeEventHash(forgedSigned);
forgedSigned.digests = [{ alg: 'sha256', canon: 'edda-canon-v1', value: forgedSigned.hash }];
const fv = verifyEvent(forgedSigned, keyring);
console.log(`   forged signed event  : ${fv.ok ? 'ACCEPTED — SPIKE FAILURE' : `REJECTED (${fv.reason})`}`);
if (fv.ok) fail('signed verification must reject the forged event');

const attacker = generateKeyPair();
const { event: attackerSigned } = signEvent(
  { type: 'decision_ratify', branch: 'main', payload: { key: 'identity.signing', ratified_by: 'operator-main' } },
  { actorId: 'operator-main', role: 'operator', keypair: attacker },
);
const av = verifyEvent(attackerSigned, keyring);
console.log(`   attacker re-signs with own key, embeds it in event: ${av.ok ? 'ACCEPTED — SPIKE FAILURE' : `REJECTED (${av.reason})`}`);
if (av.ok) fail('embedded attacker key must never be trusted');

// ── E. authority: agent cannot ratify self ──────────────────────────────────
console.log('\nE. Authority layer — signature validity is not authorization');
const { event: agentRatify } = signEvent(
  { type: 'decision_ratify', branch: 'main', payload: { key: 'identity.signing', ratified_by: 'agent-fixture' } },
  { actorId: 'agent-fixture', role: 'agent', keypair: agent },
);
const cryptoOk = verifyEvent(agentRatify, keyring);
const auth = authorizeRatify(agentRatify, keyring);
console.log(`   agent-signed ratify: signature ${cryptoOk.ok ? 'VALID' : 'invalid'}, authorized: ${auth.authorized ? 'YES — SPIKE FAILURE' : `NO (${auth.reason})`}`);
if (!cryptoOk.ok || auth.authorized) fail('agent must verify but not authorize');

const { event: opRatify } = signEvent(
  { type: 'decision_ratify', branch: 'main', payload: { key: 'identity.signing', ratified_by: 'operator-main' } },
  { actorId: 'operator-main', role: 'operator', keypair: operator },
);
const opAuth = authorizeRatify(opRatify, keyring);
console.log(`   operator-signed ratify: authorized: ${opAuth.authorized ? 'YES' : 'NO — SPIKE FAILURE'}`);
if (!opAuth.authorized) fail('operator ratify must authorize');

console.log('\n' + line);
if (process.exitCode) {
  console.log('RESULT: FAILED');
} else {
  console.log('RESULT: all invariants hold — unsigned history stays legacy-tier, signed events authenticate authorship, authority stays operator-only.');
}
console.log(line);
