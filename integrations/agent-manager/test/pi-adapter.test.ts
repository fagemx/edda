import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { pathToFileURL } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { ChannelAdapter, defaultPiRoot } from '../src/pi-adapter.js';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, selectionRevision } from '../src/config.js';
import type { SendRequest } from '../src/contracts.js';

interface Channel { snapshot(): { instanceId: string }; messageStarted(message: string): void; settled(): void; close(): Promise<void> }
interface ChannelModule { startChannel(options: { root: string; sessionId: string; cwd: string; deliver: (message: string) => void; getConversation: (query: { after?: string }) => unknown }): Promise<Channel> }

test('real Pi HTTP channel adapter isolates poison records, preserves receipts and rejects replacement', async () => {
  const api = await import(pathToFileURL(join(defaultPiRoot(), 'channel.mjs')).href) as ChannelModule;
  const root = mkdtempSync(join(tmpdir(), 'manager-pi-')), registry = join(root, 'pi'), workspace = join(root, 'project'), sessionId = randomUUID();
  mkdirSync(workspace); let deliveries = 0, channel: Channel;
  const page = (query: { after?: string }) => {
    if (query.after && query.after !== 'one') throw new Error('Conversation cursor is not on this branch');
    return { entries: [{ id: 'one', timestamp: new Date().toISOString(), kind: 'message', role: 'assistant', text: 'Public reply', thinking: 'PRIVATE_THOUGHT', toolCalls: [{ arguments: 'PRIVATE_ARGS' }] }], cursor: 'one', headCursor: 'one', hasMore: false };
  };
  channel = await api.startChannel({ root: registry, sessionId, cwd: workspace, getConversation: page, deliver: (message) => {
    deliveries++; setTimeout(() => { channel.messageStarted(message); channel.settled(); }, 10);
  } });
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [{ id: 'a', name: 'Selected', projectId: 'p', role: 'worker', registryRoot: registry, workspace, sessionId }] });
  const store = new ManagerStore(join(root, 'manager')), manager = new AgentManager(config, store, await ChannelAdapter.create());
  try {
    const poison = join(registry, 'a'.repeat(64)); mkdirSync(poison); writeFileSync(join(poison, 'state.json'), Buffer.alloc(16));
    await manager.refresh(); const view = manager.overview().agents[0]!;
    assert.equal(view.source, 'live'); assert.equal(view.latestMessage?.text, 'Public reply');
    const conversation = await manager.conversation('a');
    assert.ok(!JSON.stringify(conversation).includes('PRIVATE'));
    const ownerFiles = (await import('node:fs')).readdirSync(registry).filter((p) => /^[a-f0-9]{64}$/.test(p) && p !== 'a'.repeat(64));
    const owner = JSON.parse(readFileSync(join(registry, ownerFiles[0]!, 'owner.json'), 'utf8')) as { token: string };
    assert.ok(!JSON.stringify(manager.overview()).includes(owner.token));
    await assert.rejects(() => manager.conversation('a', 'abandoned'), /分支/);
    const request: SendRequest = { operationId: randomUUID(), selectionRevision: selectionRevision(config.agents[0]!), instanceId: view.instanceId!, basisCursor: 'one', mode: 'followUp', message: 'Visible operator message' };
    await assert.rejects(() => manager.send('a', { ...request, message: '"'.repeat(12288) }), /容量/);
    assert.equal(store.operations().length, 0);
    await manager.send('a', request); await manager.send('a', request); await delay(50);
    assert.equal((await manager.operation(request.operationId)).status, 'settled'); assert.equal(deliveries, 1);
    await channel.close(); channel = await api.startChannel({ root: registry, sessionId, cwd: workspace, getConversation: page, deliver: () => { deliveries++; } });
    await assert.rejects(() => manager.send('a', { ...request, operationId: randomUUID() }), /更換/);
    assert.equal(deliveries, 1);
  } finally { await manager.stop(); await channel.close(); store.close(); rmSync(root, { recursive: true, force: true }); }
});
