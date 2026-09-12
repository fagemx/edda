import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { ManagerStore } from '../src/store.js';
import { parseConfig, parseSend } from '../src/config.js';
import type { SendRequest } from '../src/contracts.js';

test('durable intent rejects changed duplicate and restart never invents no-send', () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-store-'));
  let store = new ManagerStore(root);
  try {
    const request: SendRequest = { operationId: randomUUID(), selectionRevision: 'a'.repeat(64), instanceId: randomUUID(), basisCursor: null, mode: 'followUp', message: '保留既有修改，請回報進度。' };
    const op = store.begin({ id: 'character', name: 'Character', role: 'manager', projectId: 'p', registryRoot: root, sessionId: 'original-session', runId: null, workspace: root, summaryFile: null }, request);
    assert.equal(store.existing('character', request)?.id, op.id);
    assert.throws(() => store.existing('edda', request), /不同/);
    assert.throws(() => store.existing('character', { ...request, message: 'different' }), /不同/);
    store.close(); store = new ManagerStore(root);
    assert.equal(store.operation(op.id)?.status, 'unknown');
    assert.equal(store.target(op.id)?.sessionId, 'original-session');
    assert.ok(!JSON.stringify(store.operations()).includes('registryRoot'));
    store.update(op.id, 'started', 'Started');
    assert.equal(store.update(op.id, 'queued', 'late old response').status, 'started');
    store.update(op.id, 'settled', '回覆已結束，結果待驗收。');
    assert.equal(store.update(op.id, 'unknown', 'stale').status, 'settled');
    assert.equal(store.operations().length, 1);
    assert.equal(store.events().length, 4);
  } finally { store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('configuration binds exact selected agents, validates projects and byte limits', () => {
  const workspace = tmpdir();
  const input = { version: 1, projects: [{ id: 'p', name: 'Project', priority: 0 }], agents: [
    { id: 'a', name: 'Agent', role: 'worker', projectId: 'p', workspace, registryRoot: workspace, sessionId: 'session-a' }] };
  const config = parseConfig(input);
  assert.equal(config.agents[0]?.runId, null);
  assert.throws(() => parseConfig({ ...input, agents: [input.agents[0], { ...input.agents[0], id: 'b' }] }), /重複/);
  assert.throws(() => parseConfig({ ...input, agents: [{ ...input.agents[0], projectId: 'missing' }] }), /未登記/);
  assert.throws(() => parseConfig({ ...input, agents: [{ ...input.agents[0], registryRoot: '../escape' }] }), /絕對/);
  assert.throws(() => parseSend({ operationId: randomUUID(), instanceId: randomUUID(), selectionRevision: 'x', basisCursor: null, mode: 'followUp', message: '文'.repeat(5000) }), /12 KiB/);
});
