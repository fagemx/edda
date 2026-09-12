// Explicit actual-Pi integration/browser fixture. Never runs in the default suite.
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { ChannelAdapter, defaultPiRoot, secureRoot } from '../src/pi-adapter.js';
import { parseConfig, selectionRevision, object } from '../src/config.js';
import { ManagerStore } from '../src/store.js';
import { AgentManager } from '../src/manager.js';
import { serve } from '../src/http.js';
import type { ConversationView, OperationView, SendRequest } from '../src/contracts.js';

const entry = process.argv[2], keep = process.argv.includes('--serve');
if (!entry) throw new Error('Supply installed Pi entry; optional --serve keeps the browser fixture running');
interface Managed {
  launchManaged(root: string, input: unknown): Promise<unknown>;
  managedStatus(root: string, id: string): Promise<unknown>;
  stopManaged(root: string, id: string, options: { abort: boolean }): Promise<unknown>;
}
const api = await import(pathToFileURL(join(defaultPiRoot(), 'managed-client.mjs')).href) as Managed;
const root = mkdtempSync(join(tmpdir(), 'edda-manager-smoke-')), registry = join(root, 'registry'), workspace = join(root, 'fixture'), agentDir = join(root, 'agent');
mkdirSync(workspace); mkdirSync(agentDir); await secureRoot(root);
let runId: string | null = null, manager: AgentManager | null = null, store: ManagerStore | null = null;
let gateway: Awaited<ReturnType<typeof serve>> | null = null, closed = false;
async function close(): Promise<void> {
  if (closed) return; closed = true;
  await manager?.stop(); await gateway?.close(); store?.close();
  if (runId) await api.stopManaged(registry, runId, { abort: true });
}
try {
  const launched = object(await api.launchManaged(registry, { project: workspace, piEntry: entry, provider: 'edda-offline-test', model: 'echo',
    thinking: 'off', noTools: true, noSkills: true, agentDir, extensions: [join(defaultPiRoot(), 'fixtures/offline-provider.mjs')], prompt: 'MANAGER_SMOKE_READY' }));
  runId = String(launched.runId); const sessionId = String(launched.sessionId);
  for (let i = 0; i < 100; i++) {
    const state = object(await api.managedStatus(registry, runId));
    if (object(state.initialReceipt).status === 'settled') break;
    if (state.modelError) throw new Error('Offline provider failed');
    if (i === 99) throw new Error('Offline fixture did not settle');
    await delay(100);
  }
  const config = parseConfig({ version: 1, refreshMs: 1000, projects: [{ id: 'fixture', name: '隔離驗證', priority: 0, resources: [] }], agents: [
    { id: 'pi-fixture', name: 'Pi 收發驗證', projectId: 'fixture', role: 'worker', registryRoot: registry, workspace, sessionId, runId },
    { id: 'offline-fixture', name: '離線狀態驗證', projectId: 'fixture', role: 'manager', registryRoot: registry, workspace, sessionId: randomUUID() }] });
  store = new ManagerStore(join(root, 'manager')); manager = new AgentManager(config, store, await ChannelAdapter.create());
  await manager.start(); const token = randomBytes(32).toString('hex');
  gateway = await serve(manager, token, { onStop: () => { void close(); } });
  const origin = gateway.origin, headers = { authorization: `Bearer ${token}`, 'content-type': 'application/json' };
  const view = manager.overview().agents[0]!; assert.equal(view.source, 'live');
  assert.equal(manager.overview().agents[1]?.source, 'unavailable');
  const request: SendRequest = { operationId: randomUUID(), instanceId: view.instanceId!, selectionRevision: selectionRevision(config.agents[0]!), basisCursor: null, mode: 'followUp', message: 'MANAGER_ACTUAL_PI_ONCE' };
  for (let i = 0; i < 2; i++) {
    const response = await fetch(`${origin}/api/agents/pi-fixture/messages`, { method: 'POST', headers, body: JSON.stringify(request) });
    assert.equal(response.status, 202);
  }
  let operation: OperationView | null = null;
  for (let i = 0; i < 100; i++) {
    operation = await (await fetch(`${origin}/api/operations/${request.operationId}`, { headers })).json() as OperationView;
    if (operation.status === 'settled') break;
    await delay(100);
  }
  assert.equal(operation?.status, 'settled');
  const conversation = await (await fetch(`${origin}/api/agents/pi-fixture/conversation`, { headers })).json() as ConversationView;
  assert.equal(conversation.entries.filter((e) => e.role === 'user' && e.text.includes(request.message)).length, 1);
  assert.ok(conversation.entries.some((e) => e.role === 'assistant' && e.text.includes('OFFLINE_ACK') && e.text.includes(request.message)));
  const summary = { passed: true, actualPi: true, liveModel: false, messageDeliveredOnce: true, publicReplyVisible: true, receiptSettled: true, partialSourceFailureIsolated: true,
    root, origin, sessionId, runId, operationId: request.operationId };
  writeFileSync(join(root, 'receipt.json'), JSON.stringify(summary, null, 2));
  writeFileSync(join(root, 'browser.json'), JSON.stringify({ url: `${origin}/#token=${token}`, origin, token }, null, 2), { mode: 0o600 });
  console.log(JSON.stringify(summary));
  if (keep) {
    process.once('SIGINT', () => { void close(); }); process.once('SIGTERM', () => { void close(); });
    console.log('Browser fixture remains available; POST /api/service/stop closes only this fixture.');
  } else await close();
} catch (error) { await close(); throw error; }
