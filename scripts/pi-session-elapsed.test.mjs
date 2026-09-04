import { test } from 'node:test';
import assert from 'node:assert/strict';
import { sessionElapsed } from './pi-session-elapsed.mjs';

test('message timestamps exclude session header and preserve measured zero', () => {
  const raw = [
    { type: 'session', timestamp: '2020-01-01T00:00:00Z' },
    { type: 'message', timestamp: '2026-09-04T00:00:00Z' },
    { type: 'message', timestamp: '2026-09-04T00:00:05Z' },
  ].map(JSON.stringify).join('\n');
  assert.deepEqual(sessionElapsed(raw), { elapsed_ms: 5000, elapsed_measured: true });
  const equal = [1, 1].map(timestamp => JSON.stringify({ type: 'message', message: { timestamp } })).join('\n');
  assert.deepEqual(sessionElapsed(equal), { elapsed_ms: 0, elapsed_measured: true });
});

test('missing, malformed, one-message and backwards evidence stays unmeasured', () => {
  for (const raw of ['', 'bad JSON', '{"type":"message"}',
    '{"type":"message","timestamp":1}',
    '{"type":"message","timestamp":2}\n{"type":"message","timestamp":1}']) {
    assert.deepEqual(sessionElapsed(raw), { elapsed_ms: null, elapsed_measured: false });
  }
});
