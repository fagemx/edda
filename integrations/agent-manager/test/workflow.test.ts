import test from 'node:test';
import assert from 'node:assert/strict';
import { randomBytes, randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, selectionRevision } from '../src/config.js';
import { EddaWorkflowLedger, WorkflowLocks, type CanonicalTask, type LedgerNote, type WorkflowLedger } from '../src/edda-workflow.js';
import { serve } from '../src/http.js';
import type { AgentObservation, PiAdapter, SendRequest } from '../src/contracts.js';
import type { WorkAction, WorkBinding, WorkView } from '../src/workflow-contracts.js';

class MemoryLedger implements WorkflowLedger {
  notesByTask = new Map<number, LedgerNote[]>();
  failTasks = new Set<number>();
  failAfterAppend = false;
  async task(binding: WorkBinding): Promise<CanonicalTask> {
    if (this.failTasks.has(binding.taskId)) throw new Error('source unavailable');
    return { id: binding.taskId, key: `task-${binding.taskId}`, title: `Task ${binding.taskId}`,
      status: 'running', receipt: null, updatedAt: '2026-09-13T00:00:00Z' };
  }
  async notes(binding: WorkBinding): Promise<LedgerNote[]> { return [...(this.notesByTask.get(binding.taskId) ?? [])]; }
  async append(binding: WorkBinding, text: string): Promise<void> {
    const notes = this.notesByTask.get(binding.taskId) ?? [];
    notes.push({ id: `evt-${notes.length + 1}`, at: `2026-09-13T00:00:0${notes.length + 1}Z`, text });
    this.notesByTask.set(binding.taskId, notes);
    if (this.failAfterAppend) { this.failAfterAppend = false; throw new Error('simulated crash after append'); }
  }
}

function fixture(root: string, ledger = new MemoryLedger(), secondWork = false) {
  const instanceId = randomUUID(); let effects = 0, timeoutAfterEffect = false;
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
    { id: 'worker', name: 'Worker', projectId: 'p', role: 'worker', registryRoot: join(root, 'registry'), workspace: root, sessionId: 'worker-session' },
  ], works: [{ id: 'work', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' },
    ...(secondWork ? [{ id: 'bad', projectId: 'p', taskId: 8, workspace: join(root, 'bad'), ownerAgentId: 'owner' }] : [])] });
  const observation: AgentObservation = { state: 'running', instanceId, observedAt: new Date().toISOString(), heartbeatAt: null,
    lastProgressAt: null, lastEvent: null, source: 'live', stale: false, reason: null, degraded: null, model: null, usage: null,
    capabilities: { send: true, conversation: true }, latestMessage: null, ownerMailbox: null };
  const adapter: PiAdapter = { observe: async () => observation,
    conversation: async () => ({ entries: [], instanceId, cursor: null, headCursor: null, hasMore: false, observedAt: new Date().toISOString(), source: 'live' }),
    send: async (binding, request) => { effects++; if (timeoutAfterEffect) throw new Error('timeout after delivery');
      return { id: request.operationId, sessionId: binding.sessionId, instanceId, status: 'started' }; }, receipt: async () => null };
  return { config, ledger, adapter, instanceId, effects: () => effects, timeout: () => { timeoutAfterEffect = true; } };
}

function send(config: ReturnType<typeof parseConfig>, instanceId: string, operationId = randomUUID(), mode: 'followUp' | 'steer' = 'followUp'): SendRequest {
  const worker = config.agents.find(agent => agent.id === 'worker')!;
  return { operationId, selectionRevision: selectionRevision(worker), instanceId, basisCursor: 'cursor', mode, message: 'Execute the bounded next step.' };
}

test('work lifecycle keeps canonical task state separate and requires acknowledged direction changes', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-workflow-')), f = fixture(root);
  const store = new ManagerStore(root), manager = new AgentManager(f.config, store, f.adapter,
    { ledger: f.ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  try {
    let work = (await manager.works.list()).works[0]!;
    assert.equal(work.stage, 'uninitialized'); assert.equal(work.taskStatus, 'running');
    work = await manager.works.act('work', { kind: 'initialize', actionId: randomUUID(), revision: work.revision, nextStep: 'Assign implementation.' });
    const assign: WorkAction = { kind: 'assign', actionId: randomUUID(), revision: work.revision, agentId: 'worker', nextStep: 'Return a tested candidate.', send: send(f.config, f.instanceId) };
    const [first, duplicate] = await Promise.all([manager.works.act('work', assign), manager.works.act('work', assign)]);
    assert.equal(f.effects(), 1); assert.equal(first.confirmedActionId, assign.actionId); assert.equal(duplicate.confirmedActionId, assign.actionId);
    work = (await manager.works.list()).works[0]!; assert.equal(work.stage, 'executing'); assert.equal(work.assigneeAgentId, 'worker');
    await assert.rejects(() => manager.works.act('work', { kind: 'block', actionId: randomUUID(), revision: 'stale', reason: 'x', nextStep: 'y' }), /更新/);

    const intervention: WorkAction = { kind: 'intervene', actionId: randomUUID(), revision: work.revision, send: send(f.config, f.instanceId, randomUUID(), 'steer') };
    work = await manager.works.act('work', intervention); assert.equal(work.pendingInstruction?.id, intervention.actionId);
    await assert.rejects(() => manager.works.act('work', { kind: 'deliver', actionId: randomUUID(), revision: work.revision, evidence: 'candidate', nextStep: 'review' }), /指示變更/);
    work = await manager.works.act('work', { kind: 'acknowledge', actionId: randomUUID(), revision: work.revision,
      instructionId: intervention.actionId, evidence: 'Agent explicitly acknowledged the changed direction.' });
    assert.ok(work.pendingInstruction?.acknowledgedAt);
    work = await manager.works.act('work', { kind: 'deliver', actionId: randomUUID(), revision: work.revision, evidence: 'SHA and tests', nextStep: 'Independent review' });
    assert.equal(work.stage, 'delivered');
    work = await manager.works.act('work', { kind: 'accept', actionId: randomUUID(), revision: work.revision, evidence: 'Review LGTM' });
    assert.equal(work.stage, 'accepted'); assert.equal(work.taskStatus, 'running'); assert.equal(work.taskReceipt, null);
    assert.ok(!JSON.stringify(work).includes('registryRoot'));
    work = await manager.works.act('work', { kind: 'intervene', actionId: randomUUID(), revision: work.revision, send: send(f.config, f.instanceId) });
    assert.equal(work.stage, 'executing'); assert.match(work.nextStep, /確認方向變更/); assert.equal(work.evidence, null);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('ledger-first recovery reuses one action and original target without replaying uncertain sends', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-workflow-recovery-')), ledger = new MemoryLedger(), f = fixture(root, ledger);
  let store = new ManagerStore(root), manager = new AgentManager(f.config, store, f.adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  try {
    let work = (await manager.works.list()).works[0]!;
    work = await manager.works.act('work', { kind: 'initialize', actionId: randomUUID(), revision: work.revision, nextStep: 'Assign.' });
    const action: WorkAction = { kind: 'assign', actionId: randomUUID(), revision: work.revision, agentId: 'worker', nextStep: 'Deliver.', send: send(f.config, f.instanceId) };
    ledger.failAfterAppend = true;
    await assert.rejects(() => manager.works.act('work', action), /simulated crash/); assert.equal(f.effects(), 0);
    store.close(); store = new ManagerStore(root);
    manager = new AgentManager(f.config, store, f.adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
    const recovered = await manager.works.act('work', action);
    assert.equal(recovered.confirmedActionId, action.actionId); assert.equal(f.effects(), 1);
    await assert.rejects(() => manager.works.act('work', { ...action, nextStep: 'Changed payload' }), /不同內容/);

    f.timeout(); work = recovered;
    const unknown: WorkAction = { kind: 'intervene', actionId: randomUUID(), revision: work.revision, send: send(f.config, f.instanceId, randomUUID(), 'steer') };
    const first = await manager.works.act('work', unknown); assert.equal(first.deliveryStatus, 'unknown'); assert.equal(f.effects(), 2);
    const retry = await manager.works.act('work', unknown); assert.equal(retry.deliveryStatus, 'unknown'); assert.equal(f.effects(), 2);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('one unavailable Edda task does not hide healthy work and CLI arguments stay fixed', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-workflow-partial-')), ledger = new MemoryLedger(), f = fixture(root, ledger, true);
  ledger.failTasks.add(8);
  const store = new ManagerStore(root), manager = new AgentManager(f.config, store, f.adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  try {
    const works = (await manager.works.list()).works;
    assert.equal(works.length, 2); assert.equal(works[0]?.error, null); assert.match(works[1]?.error ?? '', /無法讀取/);
    const calls: Array<{ cwd: string; args: string[] }> = [];
    const cli = new EddaWorkflowLedger(async (cwd, args) => {
      calls.push({ cwd, args });
      if (args[0] === 'task') return JSON.stringify({ task_id: 7, created_event_id: 'evt-task', title: 'Canonical', status: 'running', receipt: null, updated_ts: 'now' });
      if (args[0] === 'log') return `${JSON.stringify({ event_id: 'evt-note', ts: 'then', payload: { text: 'event body' } })}\n`;
      return '';
    });
    const binding = f.config.works![0]!;
    assert.equal((await cli.task(binding)).title, 'Canonical'); assert.equal((await cli.notes(binding))[0]?.text, 'event body');
    await cli.append(binding, 'literal data; never shell');
    assert.deepEqual(calls[2], { cwd: binding.workspace, args: ['note', '--role', 'user', '--tag', 'manager-work-7', '--', 'literal data; never shell'] });
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('authenticated work HTTP endpoints expose the workflow without a path or command proxy', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-workflow-http-')), f = fixture(root), token = randomBytes(32).toString('hex');
  const store = new ManagerStore(root), manager = new AgentManager(f.config, store, f.adapter, { ledger: f.ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  const gateway = await serve(manager, token); const auth = { authorization: `Bearer ${token}`, 'content-type': 'application/json' };
  try {
    assert.equal((await fetch(`${gateway.origin}/api/works`)).status, 401);
    let response = await fetch(`${gateway.origin}/api/works`, { headers: auth }); assert.equal(response.status, 200);
    const initial = (await response.json()) as { works: WorkView[] };
    const action = { kind: 'initialize', actionId: randomUUID(), revision: initial.works[0]!.revision, nextStep: 'Bounded next step.' };
    response = await fetch(`${gateway.origin}/api/works/work/actions`, { method: 'POST', headers: auth, body: JSON.stringify(action) });
    assert.equal(response.status, 200); assert.equal(((await response.json()) as WorkView).stage, 'ready');
    assert.equal((await fetch(`${gateway.origin}/api/works/unknown/actions`, { method: 'POST', headers: auth, body: JSON.stringify(action) })).status, 404);
  } finally { await manager.stop(); await gateway.close(); store.close(); rmSync(root, { recursive: true, force: true }); }
});
