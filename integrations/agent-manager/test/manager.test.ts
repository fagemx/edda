import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, selectionRevision } from '../src/config.js';
import { publicEntries } from '../src/pi-adapter.js';
import type { AgentObservation, PiAdapter, SendRequest } from '../src/contracts.js';

test('two simultaneous sends create one effect; unknown survives restart and uses original binding', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-effect-')), instanceId = randomUUID(); let effects = 0, settled = false;
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'a', name: 'Agent', projectId: 'p', role: 'worker', registryRoot: root, workspace: root, sessionId: 'original' }] });
  const binding = config.agents[0]!;
  const observation: AgentObservation = { state: 'running', instanceId, observedAt: new Date().toISOString(), heartbeatAt: null, lastProgressAt: null, lastEvent: null,
    source: 'live', stale: false, reason: null, model: null, usage: null, capabilities: { send: true, conversation: true }, latestMessage: null };
  const adapter: PiAdapter = { observe: async () => observation,
    conversation: async () => ({ entries: [], instanceId, cursor: null, headCursor: null, hasMore: false, observedAt: new Date().toISOString(), source: 'live' }),
    send: async () => { effects++; throw new Error('Simulate a timeout after delivery'); },
    receipt: async (target, op) => { assert.equal(target.sessionId, 'original'); return settled ? { id: op.id, sessionId: target.sessionId, instanceId, status: 'settled' } : null; } };
  let store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
  const request: SendRequest = { operationId: randomUUID(), instanceId, selectionRevision: selectionRevision(binding), basisCursor: 'old-context-is-provenance', mode: 'followUp', message: 'Continue scoped work' };
  try {
    await Promise.all([manager.send('a', request), manager.send('a', request)]);
    assert.equal(effects, 1); assert.equal((await manager.operation(request.operationId)).status, 'unknown');
    await assert.rejects(() => manager.send('a', { ...request, message: 'different' }), /不同/);
    store.close(); store = new ManagerStore(root);
    const replaced = { ...config, agents: [{ ...binding, sessionId: 'replacement' }] };
    manager = new AgentManager(replaced, store, adapter); settled = true;
    assert.equal((await manager.operation(request.operationId)).status, 'settled');
    assert.equal(effects, 1);
    assert.ok(!JSON.stringify(manager.overview()).includes('registryRoot'));
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('stale identity is definitive before intent and a failed source does not hide other agents', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-partial-')), instanceId = randomUUID(); let effects = 0;
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: ['a', 'b'].map((id) => (
    { id, name: id, projectId: 'p', role: 'worker', registryRoot: root, workspace: root, sessionId: id })) });
  const adapter: PiAdapter = { observe: async (b) => {
    if (b.id === 'b') throw new Error('poisoned registry');
    return { state: 'idle', instanceId, observedAt: new Date().toISOString(), heartbeatAt: null, lastProgressAt: null, lastEvent: null, source: 'live', stale: false,
      reason: null, model: null, usage: null, capabilities: { send: true, conversation: true }, latestMessage: null };
  }, conversation: async () => { throw new Error('unused'); }, send: async () => { effects++; throw new Error('unused'); }, receipt: async () => null };
  const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
  try {
    await manager.refresh();
    assert.deepEqual(manager.overview().agents.map((a) => a.state), ['idle', 'unavailable']);
    const request: SendRequest = { operationId: randomUUID(), instanceId: randomUUID(), selectionRevision: selectionRevision(config.agents[0]!), basisCursor: null, mode: 'steer', message: 'new message' };
    await assert.rejects(() => manager.send('a', request), /更換/);
    assert.equal(store.operations().length, 0); assert.equal(effects, 0);
    await manager.refresh(); assert.equal(store.events().length, 2);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('public projection omits reasoning, tool arguments and credentials, even on malformed entries', () => {
  const result = publicEntries([{ id: '1', kind: 'message', role: 'assistant', text: '<script>hello</script>', thinking: 'SECRET_THOUGHT', token: 'SECRET_TOKEN', toolCalls: [{ arguments: 'SECRET_ARGS' }] },
    { id: '2', kind: 'tool_result', toolName: 'bash', content: 'SECRET_RESULT', isError: true }, { id: '3', kind: 'message', role: 'system', text: 'SECRET_SYSTEM' }]);
  assert.equal(result.length, 2); assert.equal(result[0]?.text, '<script>hello</script>');
  assert.ok(!JSON.stringify(result).includes('SECRET')); assert.equal(result[1]?.toolError, true);
});
