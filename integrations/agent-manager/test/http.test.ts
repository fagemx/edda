import test from 'node:test';
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { request as httpRequest } from 'node:http';
import { serve } from '../src/http.js';
import { ManagerStore } from '../src/store.js';
import { AgentManager } from '../src/manager.js';
import { parseConfig, selectionRevision } from '../src/config.js';
import type { PiAdapter, SendRequest } from '../src/contracts.js';

test('loopback gateway rejects foreign/auth/body/target misuse and preserves message identity', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-http-')), token = randomBytes(32).toString('hex'), instanceId = randomUUID(); let effects = 0;
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'a', name: 'Agent', role: 'worker', projectId: 'p', registryRoot: root, workspace: root, sessionId: 'selected' }] });
  const adapter: PiAdapter = { observe: async () => ({ state: 'running', instanceId, observedAt: new Date().toISOString(), heartbeatAt: null, lastProgressAt: null, lastEvent: null,
    source: 'live', stale: false, reason: null, model: null, usage: null, capabilities: { send: true, conversation: true }, latestMessage: null }),
    conversation: async () => ({ instanceId, entries: [], cursor: null, headCursor: null, hasMore: false, observedAt: new Date().toISOString(), source: 'live' }),
    send: async (b, r) => { effects++; return { id: r.operationId, sessionId: b.sessionId, instanceId, status: 'started' }; }, receipt: async () => null };
  const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter);
  const gateway = await serve(manager, token); const auth = { authorization: `Bearer ${token}` };
  try {
    await manager.refresh();
    assert.equal((await fetch(`${gateway.origin}/api/overview`)).status, 401);
    assert.equal((await fetch(`${gateway.origin}/api/overview`, { headers: { ...auth, origin: 'https://evil.invalid' } })).status, 403);
    assert.equal((await fetch(`${gateway.origin}/api/overview`, { method: 'OPTIONS', headers: auth })).status, 403);
    const badHost = await new Promise<number>((resolve, reject) => {
      const req = httpRequest(`${gateway.origin}/api/overview`, { headers: { ...auth, host: 'evil.invalid' } }, (res) => { res.resume(); resolve(res.statusCode || 0); }); req.on('error', reject); req.end();
    });
    assert.equal(badHost, 403);
    const overview = await fetch(`${gateway.origin}/api/overview`, { headers: auth });
    assert.equal(overview.headers.get('cache-control'), 'no-store');
    const data = await overview.text(); assert.ok(!data.includes('registryRoot')); assert.ok(!data.includes(token));
    const request: SendRequest = { operationId: randomUUID(), selectionRevision: selectionRevision(config.agents[0]!), instanceId, basisCursor: null, mode: 'followUp', message: '<script>operator text</script>' };
    const send = (payload: unknown, target = 'a') => fetch(`${gateway.origin}/api/agents/${target}/messages`, { method: 'POST', headers: { ...auth, 'content-type': 'application/json' }, body: JSON.stringify(payload) });
    assert.equal((await send(request, 'unknown')).status, 404);
    assert.equal((await send({ ...request, message: 'x'.repeat(30000) })).status, 413);
    assert.equal((await send(request)).status, 202);
    assert.equal((await send(request)).status, 202); assert.equal(effects, 1);
    assert.equal((await send({ ...request, message: 'different' })).status, 409);
    assert.equal((await send({ ...request, operationId: randomUUID(), message: '文'.repeat(4096) })).status, 202);
    assert.equal((await send({ ...request, operationId: randomUUID(), message: '文'.repeat(4096) + 'x' })).status, 413);
    assert.equal((await fetch(`${gateway.origin}/api/operations/${request.operationId}`, { headers: auth })).status, 200);
    assert.equal((await fetch(`${gateway.origin}/../config.json`, { headers: auth })).status, 404);
  } finally { await manager.stop(); await gateway.close(); store.close(); rmSync(root, { recursive: true, force: true }); }
});
