import test from 'node:test';
import assert from 'node:assert/strict';
import { encodeReferences, decodeReferences } from '../src/web/continuation-drafts.js';

test('browser continuation persistence keeps only original opaque request identity, never text or encoded bundle', () => {
  const rejected = 'synthetic-sensitive-input-for-regression';
  const ref = { route: '/import' as const, actionId: 'bce0b7aa-cd93-426e-a7be-8f047aa87229', revision: 'a'.repeat(64), input: { state: { next_action: rejected } }, bundle: Buffer.from(rejected).toString('hex'), environmentEvidence: rejected };
  const stored = encodeReferences({ work: ref, unsent: null });
  assert.ok(!stored.includes(rejected)); assert.ok(!stored.includes(ref.bundle)); assert.ok(!stored.includes('input')); assert.ok(!stored.includes('Evidence'));
  const restored = decodeReferences(stored);
  assert.deepEqual(restored.work, { route: '/import', actionId: ref.actionId, revision: ref.revision });
  assert.equal(restored.unsent, undefined);
  assert.throws(() => encodeReferences({ work: { ...ref, actionId: rejected } }));
});
