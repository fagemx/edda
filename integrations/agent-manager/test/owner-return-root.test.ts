import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig } from '../src/config.js';
import { EddaWorkflowLedger, WorkflowLocks, type EddaRunner } from '../src/edda-workflow.js';
import { ChannelAdapter, defaultPiRoot } from '../src/pi-adapter.js';
import { rootLabel } from '../src/discovery.js';
import { OWNER_MAILBOX_SOURCE } from '../src/workflow.js';
import { OWNER_MAILBOX_LABELS } from '../src/web/workboard.js';
import type { AgentBinding, AgentObservation, PiAdapter, SendRequest } from '../src/contracts.js';

const at = (seconds: number): string => new Date(Date.UTC(2026, 8, 13, 0, 0, seconds)).toISOString();
interface CallRecord { cwd: string; args: string[]; env: Record<string, string> | undefined }

const TASK = JSON.stringify({ task_id: 7, created_event_id: 'evt-1', title: 'Task 7', status: 'running', updated_ts: at(0) });
function mailbox(root: string): void { mkdirSync(join(root, '.edda', 'returns'), { recursive: true }); }
function live(overrides: Partial<AgentObservation> = {}): AgentObservation {
  return { state: 'idle', instanceId: 'instance', observedAt: at(200), heartbeatAt: at(200), lastProgressAt: null, lastEvent: null,
    source: 'live', stale: false, reason: null, degraded: null, model: null, usage: null,
    capabilities: { conversation: true, send: true }, latestMessage: null, ownerMailbox: null, ...overrides };
}
function adapterFor(observe: (binding: AgentBinding) => AgentObservation, instanceId: string): PiAdapter {
  return { observe: async (binding) => observe(binding),
    conversation: async () => ({ entries: [], instanceId, cursor: null, headCursor: null, hasMore: false, observedAt: at(200), source: 'live' }),
    send: async (binding, request: SendRequest) => ({ id: request.operationId, sessionId: binding.sessionId, instanceId, status: 'started' }),
    receipt: async () => null };
}
/** A runner that answers the fixed vectors and records `(cwd, args, env)` per call. */
function runner(calls: CallRecord[], returns: 'with-return' | 'empty' = 'with-return'): EddaRunner {
  return async (cwd, args, env) => {
    calls.push({ cwd, args, env });
    if (args[0] === 'task') return TASK;
    if (args[0] === 'log') return '';
    if (args[1] === 'status' && returns === 'empty') return JSON.stringify({ owner: 'assistant/owner', holder: null, pending: 0, total: 0 });
    if (args[1] === 'status') return JSON.stringify({ owner: 'assistant/owner', holder: 'holder-session', pending: 1, total: 1 });
    if (args[1] === 'pending' && returns === 'empty') return JSON.stringify({ owner: 'assistant/owner', count: 0, pending: [] });
    return JSON.stringify({ owner: 'assistant/owner', count: 1, pending: [
      { version: 1, id: 'm1', owner: 'assistant/owner', work: '7', status: 'done', result: 'ok', posted_by_session: 's', posted_at: at(2) }] });
  };
}
function workspaceConfig(base: string, workspace: string, extra: Record<string, unknown> = {}): ReturnType<typeof parseConfig> {
  return parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: join(base, 'registry'), workspace, sessionId: 'owner-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace, ownerAgentId: 'owner', ownerRef: 'assistant/owner', ...extra }] });
}
async function withEnv<T>(value: string | undefined, action: () => Promise<T>): Promise<T> {
  const prior = process.env.EDDA_RETURN_ROOT;
  if (value === undefined) delete process.env.EDDA_RETURN_ROOT; else process.env.EDDA_RETURN_ROOT = value;
  try { return await action(); } finally { if (prior === undefined) delete process.env.EDDA_RETURN_ROOT; else process.env.EDDA_RETURN_ROOT = prior; }
}

test('1. a pinned works[].ownerRoot mailbox shows holder/pending and the child env pins it', async () => {
  const base = mkdtempSync(join(tmpdir(), 'owner-root-binding-')), pinned = join(base, 'pinned'), workspace = join(base, 'workspace');
  mkdirSync(workspace); mailbox(pinned);
  const calls: CallRecord[] = [];
  const ledger = new EddaWorkflowLedger(runner(calls));
  const config = workspaceConfig(base, workspace, { ownerRoot: pinned });
  const store = new ManagerStore(base);
  const manager = new AgentManager(config, store, adapterFor(() => live(), randomUUID()), { ledger, locks: new WorkflowLocks(join(base, 'locks')) });
  try {
    await withEnv(undefined, async () => {
      const work = (await manager.works.list()).works[0]!;
      assert.equal(work.ownerReturn?.mailbox.kind, 'binding');
      assert.equal(work.ownerReturn?.mailbox.present, true);
      assert.equal(work.ownerReturn?.error, null);
      assert.equal(work.ownerReturn?.holder, 'holder-session');
      assert.equal(work.ownerReturn?.pending, 1);
      assert.equal(work.ownerReturn?.matched[0]?.work, '7');
      assert.equal(work.ownerReturn?.matched[0]?.status, 'done');
      const env = calls.find((call) => call.args[0] === 'return' && call.args[1] === 'status')?.env;
      assert.equal(env?.EDDA_RETURN_ROOT, pinned);
    });
  } finally { await manager.stop(); store.close(); rmSync(base, { recursive: true, force: true }); }
});

test('2. a managed run observation derives the mailbox and the return is projected', async () => {
  const base = mkdtempSync(join(tmpdir(), 'owner-root-managed-')), managedRoot = join(base, 'owner-mailbox'), workspace = join(base, 'workspace');
  mkdirSync(workspace); mailbox(managedRoot);
  const calls: CallRecord[] = [];
  const ledger = new EddaWorkflowLedger(runner(calls));
  const config = workspaceConfig(base, workspace);
  const store = new ManagerStore(base);
  const manager = new AgentManager(config, store,
    adapterFor(() => live({ ownerMailbox: { ref: 'assistant/owner', root: managedRoot } }), randomUUID()),
    { ledger, locks: new WorkflowLocks(join(base, 'locks')) });
  try {
    await withEnv(undefined, async () => {
      await manager.refresh();
      const work = (await manager.works.list()).works[0]!;
      assert.equal(work.ownerReturn?.mailbox.kind, 'managed');
      assert.equal(work.ownerReturn?.mailbox.present, true);
      assert.equal(work.ownerReturn?.error, null);
      assert.equal(work.ownerReturn?.matched[0]?.status, 'done');
      const env = calls.find((call) => call.args[0] === 'return' && call.args[1] === 'status')?.env;
      assert.equal(env?.EDDA_RETURN_ROOT, managedRoot);
    });
  } finally { await manager.stop(); store.close(); rmSync(base, { recursive: true, force: true }); }
});

test('3. an absolute EDDA_RETURN_ROOT in the service environment is used', async () => {
  const base = mkdtempSync(join(tmpdir(), 'owner-root-env-')), envRoot = join(base, 'env-mailbox'), workspace = join(base, 'workspace');
  mkdirSync(workspace); mailbox(envRoot);
  const calls: CallRecord[] = [];
  const ledger = new EddaWorkflowLedger(runner(calls));
  const config = workspaceConfig(base, workspace);
  const store = new ManagerStore(base);
  const manager = new AgentManager(config, store, adapterFor(() => live(), randomUUID()), { ledger, locks: new WorkflowLocks(join(base, 'locks')) });
  try {
    await withEnv(envRoot, async () => {
      const work = (await manager.works.list()).works[0]!;
      assert.equal(work.ownerReturn?.mailbox.kind, 'env');
      assert.equal(work.ownerReturn?.mailbox.present, true);
      assert.equal(work.ownerReturn?.error, null);
      assert.equal(work.ownerReturn?.matched[0]?.work, '7');
      const env = calls.find((call) => call.args[0] === 'return' && call.args[1] === 'status')?.env;
      assert.equal(env?.EDDA_RETURN_ROOT, envRoot);
    });
  } finally { await manager.stop(); store.close(); rmSync(base, { recursive: true, force: true }); }
});

test('4. a missing pinned mailbox falls back to the workspace with a notice, never a healthy zero', async () => {
  const base = mkdtempSync(join(tmpdir(), 'owner-root-fallback-')), pinned = join(base, 'absent-pinned'), workspace = join(base, 'workspace');
  mkdirSync(workspace); mailbox(workspace);
  const calls: CallRecord[] = [];
  const ledger = new EddaWorkflowLedger(runner(calls));
  const config = workspaceConfig(base, workspace, { ownerRoot: pinned });
  const store = new ManagerStore(base);
  const manager = new AgentManager(config, store, adapterFor(() => live(), randomUUID()), { ledger, locks: new WorkflowLocks(join(base, 'locks')) });
  try {
    await withEnv(undefined, async () => {
      const work = (await manager.works.list()).works[0]!;
      assert.equal(work.ownerReturn?.mailbox.kind, 'workspace');
      assert.equal(work.ownerReturn?.mailbox.present, true);
      assert.equal(work.ownerReturn?.error, null);
      assert.equal(work.ownerReturn?.matched[0]?.status, 'done');
      assert.match(work.ownerReturn?.notice ?? '', /沒有信箱紀錄/);
      assert.ok(work.ownerReturn?.notice?.includes(rootLabel(pinned)));
      assert.ok(!work.ownerReturn?.notice?.includes(pinned));
    });
  } finally { await manager.stop(); store.close(); rmSync(base, { recursive: true, force: true }); }
});

test('5. no mailbox at any candidate is honest unavailability, not a healthy zero', async () => {
  const base = mkdtempSync(join(tmpdir(), 'owner-root-none-')), pinned = join(base, 'absent-pinned'), workspace = join(base, 'workspace');
  mkdirSync(workspace);
  const calls: CallRecord[] = [];
  const ledger = new EddaWorkflowLedger(runner(calls, 'empty'));
  const config = workspaceConfig(base, workspace, { ownerRoot: pinned });
  const store = new ManagerStore(base);
  const manager = new AgentManager(config, store, adapterFor(() => live(), randomUUID()), { ledger, locks: new WorkflowLocks(join(base, 'locks')) });
  try {
    await withEnv(undefined, async () => {
      const work = (await manager.works.list()).works[0]!;
      const read = work.ownerReturn!;
      assert.equal(read.mailbox.present, false);
      assert.equal(read.mailbox.kind, 'binding');
      assert.notEqual(read.error, null);
      assert.match(read.notice ?? '', /找不到任何 owner mailbox/);
      // Explicitly not the healthy-zero shape the defect produced.
      assert.ok(!(read.pending === 0 && read.error === null));
    });
  } finally { await manager.stop(); store.close(); rmSync(base, { recursive: true, force: true }); }
});

interface Channel { snapshot(): { instanceId: string }; close(): Promise<void> }
interface ChannelModule { startChannel(options: { root: string; sessionId: string; cwd: string; deliver: () => void; getConversation: () => unknown }): Promise<Channel> }
function managedRun(registryRoot: string, runId: string, config: Record<string, unknown>, state: Record<string, unknown> | 'corrupt'): void {
  const dir = join(registryRoot, 'managed', runId);
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'config.json'), JSON.stringify({ version: 1, runId, root: registryRoot, project: registryRoot, release: {}, ...config }));
  if (state === 'corrupt') writeFileSync(join(dir, 'state.json'), Buffer.alloc(2048));
  else writeFileSync(join(dir, 'state.json'), JSON.stringify({ runId, ...state }));
}

test('6. ownerMailbox is projected from the managed record on the live and degraded paths', async () => {
  const base = mkdtempSync(join(tmpdir(), 'owner-root-observe-')), registry = join(base, 'registry'),
    workspace = join(base, 'workspace'), explicitRoot = join(base, 'explicit-mailbox'), sessionId = randomUUID();
  mkdirSync(workspace); mailbox(explicitRoot);
  const explicit = randomUUID(), defaulted = randomUUID(), returnOwner = randomUUID(), noOwner = randomUUID(), degraded = randomUUID();
  managedRun(registry, explicit, { owner: 'assistant/owner', ownerRoot: explicitRoot }, { sessionId, phase: 'ready', owner: 'assistant/owner', ownerRoot: explicitRoot });
  managedRun(registry, defaulted, { owner: 'assistant/owner' }, { sessionId, phase: 'ready', owner: 'assistant/owner' });
  managedRun(registry, returnOwner, { returnOwner: 'assistant/return' }, { sessionId, phase: 'ready', returnOwner: 'assistant/return' });
  managedRun(registry, noOwner, {}, { sessionId, phase: 'ready' });
  managedRun(registry, degraded, { owner: 'assistant/owner' }, 'corrupt');
  const channel = await (await import(pathToFileURL(join(defaultPiRoot(), 'channel.mjs')).href) as ChannelModule)
    .startChannel({ root: registry, sessionId, cwd: workspace, getConversation: () => ({ entries: [] }), deliver: () => {} });
  const adapter = await ChannelAdapter.create();
  const binding = (runId: string): AgentBinding => ({ id: runId, name: 'Run', role: 'worker', projectId: 'p', registryRoot: registry, sessionId, runId, workspace, summaryFile: null });
  try {
    const pinned = await adapter.observe(binding(explicit));
    assert.equal(pinned.source, 'live');
    assert.deepEqual(pinned.ownerMailbox, { ref: 'assistant/owner', root: explicitRoot });
    assert.deepEqual((await adapter.observe(binding(defaulted))).ownerMailbox, { ref: 'assistant/owner', root: join(registry, 'owner-mailbox') });
    assert.deepEqual((await adapter.observe(binding(returnOwner))).ownerMailbox, { ref: 'assistant/return', root: join(registry, 'owner-mailbox') });
    assert.equal((await adapter.observe(binding(noOwner))).ownerMailbox, null);
    await channel.close();
    // The degraded (record_unavailable) path still carries the managed record.
    const recordUnavailable = await adapter.observe(binding(degraded));
    assert.equal(recordUnavailable.source, 'unavailable');
    assert.equal(recordUnavailable.degraded?.code, 'record_unavailable');
    assert.deepEqual(recordUnavailable.ownerMailbox, { ref: 'assistant/owner', root: join(registry, 'owner-mailbox') });
  } finally { await channel.close(); rmSync(base, { recursive: true, force: true }); }
});

test('7. the card mailbox source labels are pinned to the server names', () => {
  // The browser bundle cannot import workflow.ts (node built-ins), so the display
  // map is a second literal; this pins it to the server map so it cannot drift.
  assert.deepEqual(OWNER_MAILBOX_LABELS, OWNER_MAILBOX_SOURCE);
  assert.deepEqual(Object.keys(OWNER_MAILBOX_LABELS).sort(), ['binding', 'env', 'managed', 'workspace']);
});
