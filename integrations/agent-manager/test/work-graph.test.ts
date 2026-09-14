import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, selectionRevision } from '../src/config.js';
import { EddaWorkflowLedger, WorkflowLocks, type CanonicalTask, type LedgerNote, type WorkflowLedger } from '../src/edda-workflow.js';
import { ChannelAdapter, defaultPiRoot } from '../src/pi-adapter.js';
import { WorkManager } from '../src/workflow.js';
import { rootLabel, projectCandidates } from '../src/discovery.js';
import { waitTargets } from '../src/web/workboard.js';
import type { AgentBinding, AgentObservation, ManagerConfig, PiAdapter, SendRequest } from '../src/contracts.js';
import type { OwnerReturnRead, WorkAction, WorkBinding, WorkView, WorkWaitingFor } from '../src/workflow-contracts.js';

const at = (seconds: number): string => new Date(Date.UTC(2026, 8, 13, 0, 0, seconds)).toISOString();
const LATER = at(300);

/** A durable observation-only ledger shared across a simulated config change. */
class MemoryLedger implements WorkflowLedger {
  private entries = new Map<number, LedgerNote[]>();
  private clock = 0;
  status = 'running';
  returnsImpl: ((binding: WorkBinding) => Promise<OwnerReturnRead | null>) | null = null;
  async task(binding: WorkBinding): Promise<CanonicalTask> {
    return { id: binding.taskId, key: `task-${binding.taskId}`, title: `Task ${binding.taskId}`, status: this.status, receipt: null, updatedAt: at(0) };
  }
  async notes(binding: WorkBinding): Promise<LedgerNote[]> { return [...(this.entries.get(binding.taskId) ?? [])]; }
  async append(binding: WorkBinding, text: string): Promise<void> {
    const list = this.entries.get(binding.taskId) ?? [];
    this.clock += 1; list.push({ id: `evt-${this.clock}`, at: at(this.clock), text }); this.entries.set(binding.taskId, list);
  }
  async returns(binding: WorkBinding): Promise<OwnerReturnRead | null> { return this.returnsImpl ? this.returnsImpl(binding) : null; }
}

function live(sessionId: string, overrides: Partial<AgentObservation> = {}): AgentObservation {
  return { state: 'idle', instanceId: 'instance', observedAt: at(200), heartbeatAt: at(200), lastProgressAt: null, lastEvent: null,
    source: 'live', stale: false, reason: null, degraded: null, model: null, usage: null,
    capabilities: { conversation: true, send: true }, latestMessage: null, ownerMailbox: null,
    sessionEvidence: { sessionId, evidenceSource: 'live', historyComplete: false, events: [] }, ...overrides };
}
function adapterFor(observe: (binding: AgentBinding) => AgentObservation, instanceId: string): PiAdapter {
  return { observe: async (binding) => observe(binding),
    conversation: async () => ({ entries: [], instanceId, cursor: null, headCursor: null, hasMore: false, observedAt: at(200), source: 'live' }),
    send: async (binding, request: SendRequest) => ({ id: request.operationId, sessionId: binding.sessionId, instanceId, status: 'started' }),
    receipt: async () => null };
}
function open(config: ManagerConfig, ledger: WorkflowLedger, root: string, adapter: PiAdapter) {
  const store = new ManagerStore(root);
  const manager = new AgentManager(config, store, adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  return { manager, store };
}
/** A second projection over the same manager: it has its own per-work read cache,
 *  so assertions about steady state are not served by a warm cached row. */
function freshProjection(manager: AgentManager, ledger: WorkflowLedger, root: string): WorkManager {
  return new WorkManager(manager, ledger, new WorkflowLocks(join(root, 'locks')));
}
function sendFor(config: ManagerConfig, agentId: string, instanceId: string): SendRequest {
  const agent = config.agents.find((a) => a.id === agentId)!;
  return { operationId: randomUUID(), selectionRevision: selectionRevision(agent), instanceId, basisCursor: null, mode: 'followUp', message: 'Execute the bounded next step.' };
}
async function act(manager: AgentManager, id: string, partial: Record<string, unknown>): Promise<WorkView> {
  const current = (await manager.works.list()).works.find((w) => w.id === id)!;
  const revision = typeof partial.revision === 'string' ? partial.revision : current.revision;
  const { revision: _ignoredRevision, actionId: _ignoredActionId, ...rest } = partial;
  return manager.works.act(id, { actionId: randomUUID(), ...rest, revision } as unknown as WorkAction);
}

test('the role chain derives owner → worker → verifier from the recorded bindings and their parent relation', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-role-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
    { id: 'worker', name: 'Worker', projectId: 'p', role: 'worker', registryRoot: join(root, 'r'), workspace: root, sessionId: 'worker-session' },
    { id: 'reviewer', name: 'Reviewer', projectId: 'p', role: 'worker', registryRoot: join(root, 'r'), workspace: root, sessionId: 'reviewer-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  const adapter = adapterFor((binding) => live(`${binding.id}-session`, { instanceId }), instanceId);
  const { manager, store } = open(config, ledger, root, adapter);
  try {
    await act(manager, 'w', { kind: 'initialize' as const, nextStep: 'Assign implementation.' });
    // A missing relation is explicit, never a guessed role.
    const ready = (await manager.works.list()).works[0]!;
    assert.equal(ready.phase, 'ready');
    assert.equal(ready.waitingFor, null);
    await act(manager, 'w', { kind: 'bind_session', agentId: 'worker', role: 'worker', parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'deliver', nextExpectedAt: null });
    await act(manager, 'w', { kind: 'assign', agentId: 'worker', nextStep: 'Return a tested candidate.', send: sendFor(config, 'worker', instanceId) });
    const bound = (await manager.works.list()).works[0]!;
    assert.equal(bound.waitingFor, 'worker');
    await act(manager, 'w', { kind: 'deliver', evidence: 'candidate + tests', nextStep: 'Independent review' });
    // A delivery with no reviewer bound names no target at all.
    const noReviewer = (await manager.works.list()).works[0]!;
    assert.equal(noReviewer.phase, 'delivered');
    assert.equal(noReviewer.waitingFor, null);
    await act(manager, 'w', { kind: 'bind_session', agentId: 'reviewer', role: 'reviewer', parentAgentId: 'worker', reviewedSha: 'a'.repeat(40), expectedEvent: 'review the candidate', nextExpectedAt: null });
    const reviewed = (await manager.works.list()).works[0]!;
    assert.equal(reviewed.phase, 'delivered');
    assert.equal(reviewed.waitingFor, 'verifier');
    const reviewer = reviewed.sessions.find((s) => s.role === 'reviewer')!;
    assert.equal(reviewer.agentId, 'reviewer');
    assert.equal(reviewer.parentAgentId, 'worker');
    assert.equal(reviewer.reviewedSha, 'a'.repeat(40));
    // No registry path is ever serialized to the browser projection.
    assert.ok(!JSON.stringify(reviewed).includes('registryRoot'));
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('the current attempt is derived from the recorded hand-off chain, not an identifier', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-attempt-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
    { id: 'worker', name: 'Worker', projectId: 'p', role: 'worker', registryRoot: root, workspace: root, sessionId: 'worker-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  const { manager, store } = open(config, ledger, root, adapterFor((binding) => live(`${binding.id}-session`, { instanceId }), instanceId));
  try {
    await manager.refresh();
    assert.equal((await manager.works.list()).works[0]!.attempt, 0);
    await act(manager, 'w', { kind: 'initialize' as const, nextStep: 'Assign.' });
    assert.equal((await manager.works.list()).works[0]!.attempt, 0);
    await act(manager, 'w', { kind: 'assign', agentId: 'worker', nextStep: 'Return.', send: sendFor(config, 'worker', instanceId) });
    assert.equal((await manager.works.list()).works[0]!.attempt, 1);
    const intervene = { actionId: randomUUID(), kind: 'intervene' as const, revision: (await manager.works.list()).works[0]!.revision, send: sendFor(config, 'worker', instanceId) };
    await manager.works.act('w', intervene);
    assert.equal((await manager.works.list()).works[0]!.attempt, 2);
    await manager.works.act('w', { actionId: randomUUID(), kind: 'acknowledge', revision: (await manager.works.list()).works[0]!.revision,
      instructionId: intervene.actionId, evidence: 'worker confirmed the new direction' });
    await manager.works.act('w', { actionId: randomUUID(), kind: 'intervene', revision: (await manager.works.list()).works[0]!.revision, send: sendFor(config, 'worker', instanceId) });
    const final = (await manager.works.list()).works[0]!;
    assert.equal(final.attempt, 3);
    assert.equal(final.assigneeAgentId, 'worker');
    assert.equal(final.waitingFor, 'worker');
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('a single-root healthy work is in_root', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-in-root-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
    { id: 'worker', name: 'Worker', projectId: 'p', role: 'worker', registryRoot: root, workspace: root, sessionId: 'worker-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  const adapter = adapterFor((binding) => live(`${binding.id}-session`, { instanceId }), instanceId);
  const { manager, store } = open(config, ledger, root, adapter);
  try {
    await manager.refresh();
    await act(manager, 'w', { kind: 'initialize' as const, nextStep: 'Assign.' });
    await act(manager, 'w', { kind: 'bind_session', agentId: 'worker', role: 'worker', parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'deliver', nextExpectedAt: null });
    await act(manager, 'w', { kind: 'assign', agentId: 'worker', nextStep: 'Return.', send: sendFor(config, 'worker', instanceId) });
    await manager.refresh();
    const work = (await manager.works.list()).works[0]!;
    assert.equal(work.registry.relation, 'in_root');
    assert.doesNotMatch(work.registry.message, /registryRoot|C:\\|C:\//);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('an initialized work with no bound executor is unknown, never a claimed in_root', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-no-executor-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
    { id: 'worker', name: 'Worker', projectId: 'p', role: 'worker', registryRoot: root, workspace: root, sessionId: 'worker-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  const { manager, store } = open(config, ledger, root, adapterFor((binding) => live(`${binding.id}-session`, { instanceId }), instanceId));
  try {
    await manager.refresh();
    await act(manager, 'w', { kind: 'initialize' as const, nextStep: 'Assign.' });
    const unassigned = (await manager.works.list()).works[0]!;
    assert.equal(unassigned.assigneeAgentId, null);
    assert.equal(unassigned.registry.relation, 'unknown');
    assert.match(unassigned.registry.message, /尚未綁定可判定的執行 session/);
    assert.match(unassigned.registry.message, /Worker/);
    assert.match(unassigned.registry.message, /空的讀取不代表沒有子代理正在工作/);
    assert.doesNotMatch(unassigned.registry.message, /觀測到對應的執行來源/);
    // The observed-source wording appears only with a real bound executor.
    await act(manager, 'w', { kind: 'bind_session', agentId: 'worker', role: 'worker', parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'deliver', nextExpectedAt: null });
    const bound = (await manager.works.list()).works[0]!;
    assert.equal(bound.registry.relation, 'in_root');
    assert.match(bound.registry.message, /觀測到對應的執行來源/);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('a work whose other known root owns the run is not_in_root with a locator by agent name', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-not-in-root-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const rootA = join(root, 'registry-a'), rootB = join(root, 'registry-b');
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: rootA, workspace: root, sessionId: 'owner-session' },
    { id: 'workerA', name: 'Worker A', projectId: 'p', role: 'worker', registryRoot: rootA, workspace: root, sessionId: 'worker-a-session' },
    { id: 'workerB', name: 'Worker B', projectId: 'p', role: 'worker', registryRoot: rootB, workspace: root, sessionId: 'worker-b-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  // The work is recorded against Worker A, but the other known root (Worker B)
  // owns the live run; an empty read here must not read as global no-work.
  const adapter = adapterFor((binding) => binding.id === 'workerB' ? live('worker-b-session')
    : live(`${binding.id}-session`, { source: 'unavailable', stale: true, state: 'unavailable',
      sessionEvidence: { sessionId: 'unregistered-session', evidenceSource: 'unavailable', historyComplete: false, events: [] } }), instanceId);
  const { manager, store } = open(config, ledger, root, adapter);
  try {
    await act(manager, 'w', { kind: 'initialize' as const, nextStep: 'Assign.' });
    await act(manager, 'w', { kind: 'bind_session', agentId: 'workerA', role: 'worker', parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'deliver', nextExpectedAt: null });
    await manager.refresh();
    const work = (await manager.works.list()).works[0]!;
    assert.equal(work.registry.relation, 'not_in_root');
    assert.match(work.registry.message, /Worker B/);
    assert.match(work.registry.message, /空的讀取不代表沒有子代理正在工作/);
    assert.ok(work.registry.message.includes(rootLabel(rootB)));
    assert.ok(!work.registry.message.includes(rootB));
    assert.ok(!work.registry.message.includes('registryRoot'));
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('a recorded executor no longer registered for the project is root_not_registered', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-unregistered-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const full = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
    { id: 'worker', name: 'Worker', projectId: 'p', role: 'worker', registryRoot: join(root, 'r'), workspace: root, sessionId: 'worker-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  const reduced = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  let current = open(full, ledger, root, adapterFor((binding) => live(`${binding.id}-session`), instanceId));
  try {
    await act(current.manager, 'w', { kind: 'initialize' as const, nextStep: 'Assign.' });
    await act(current.manager, 'w', { kind: 'bind_session', agentId: 'worker', role: 'worker', parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'deliver', nextExpectedAt: null });
    await current.manager.stop(); current.store.close();
    current = open(reduced, ledger, root, adapterFor((binding) => live(`${binding.id}-session`), instanceId));
    await current.manager.refresh();
    const work = (await current.manager.works.list()).works[0]!;
    assert.equal(work.sessions.filter((s) => !s.unboundAt).length, 1);
    assert.equal(work.registry.relation, 'root_not_registered');
    assert.match(work.registry.message, /已選取的清單/);
  } finally { await current.manager.stop(); current.store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('a corrupt managed state.json degrades one row to recoverable through the real Pi reads', async () => {
  const base = mkdtempSync(join(tmpdir(), 'work-graph-degraded-')), runId = randomUUID(), instanceId = randomUUID(), ledger = new MemoryLedger();
  const registryRoot = join(base, 'registry'), workspace = join(base, 'project');
  mkdirSync(join(registryRoot, 'managed', runId), { recursive: true }); mkdirSync(workspace);
  writeFileSync(join(registryRoot, 'managed', runId, 'config.json'), JSON.stringify({ version: 1, runId, root: registryRoot, project: workspace, release: {} }));
  // The 2026-09-13 shape: a normal-length record whose bytes are all NUL.
  writeFileSync(join(registryRoot, 'managed', runId, 'state.json'), Buffer.alloc(2048));
  // A healthy sibling run proves a non-degraded candidate row carries `null`.
  const healthyRunId = randomUUID(), healthySessionId = randomUUID();
  mkdirSync(join(registryRoot, 'managed', healthyRunId), { recursive: true });
  writeFileSync(join(registryRoot, 'managed', healthyRunId, 'config.json'), JSON.stringify({ version: 1, runId: healthyRunId, root: registryRoot, project: workspace, release: {} }));
  writeFileSync(join(registryRoot, 'managed', healthyRunId, 'state.json'), JSON.stringify({ runId: healthyRunId, sessionId: healthySessionId, phase: 'ready', updatedAt: at(1) }));
  const activation = await import(pathToFileURL(join(defaultPiRoot(), 'activation.mjs')).href) as
    { listManagedRuns(root: string, options?: { limit?: number }): Promise<{ runs: Array<{ runId: string; project: string | null; error: { code: string; record: string; message: string } | null }> }> };
  const managedClient = await import(pathToFileURL(join(defaultPiRoot(), 'managed-client.mjs')).href) as
    { managedStatus(root: string, id: string): Promise<Record<string, unknown>> };
  const listed = await activation.listManagedRuns(registryRoot);
  const corrupt = listed.runs.find((run) => run.runId === runId)!;
  assert.equal(corrupt.project, workspace);
  assert.equal(corrupt.error?.code, 'record_unavailable');
  assert.equal(corrupt.error?.record, 'state.json');
  assert.equal(listed.runs.find((run) => run.runId === healthyRunId)?.error, null);
  const managed = await managedClient.managedStatus(registryRoot, runId);
  assert.equal(managed.runId, runId);
  assert.equal(managed.status, 'record_unavailable');
  assert.equal((managed.error as Record<string, unknown>).record, 'state.json');
  assert.equal(typeof managed.nextAction, 'string');

  const channel = await ChannelAdapter.create();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: registryRoot, workspace, sessionId: 'owner-session' },
    { id: 'runner', name: 'Runner', projectId: 'p', role: 'worker', registryRoot: registryRoot, workspace, sessionId: 'runner-session' },
    { id: 'degraded', name: 'Degraded worker', projectId: 'p', role: 'worker', registryRoot, workspace, sessionId: 'worker-session', runId },
  ], works: [{ id: 'w', projectId: 'p', taskId: 7, workspace, ownerAgentId: 'owner' }] });
  const adapter: PiAdapter = { observe: async (binding) => binding.id === 'degraded' ? channel.observe(binding) : live(`${binding.id}-session`, { instanceId }),
    discover: (roots) => channel.discover(roots),
    conversation: async () => ({ entries: [], instanceId, cursor: null, headCursor: null, hasMore: false, observedAt: at(200), source: 'live' }),
    send: async (binding, request) => ({ id: request.operationId, sessionId: binding.sessionId, instanceId, status: 'started' }), receipt: async () => null };
  const { manager, store } = open(config, ledger, base, adapter);
  try {
    // Discovery reports the per-record degradation honestly.
    const report = await channel.discover([registryRoot]);
    const run = report.runs.find((r) => r.runId === runId)!;
    assert.equal(run.degraded?.code, 'record_unavailable');
    assert.equal(run.degraded?.record, 'state.json');
    assert.match(run.reason ?? '', /state\.json/);
    // The candidate projection carries the degradation, and a healthy run's row
    // carries null.
    const candidates = projectCandidates(report, config.agents);
    const degradedCandidate = candidates.candidates.find((c) => c.runId === runId)!;
    assert.equal(degradedCandidate.degraded?.code, 'record_unavailable');
    assert.equal(degradedCandidate.degraded?.record, 'state.json');
    assert.equal(candidates.candidates.find((c) => c.runId === healthyRunId)?.degraded, null);
    await manager.refresh();
    await act(manager, 'w', { kind: 'initialize' as const, nextStep: 'Assign.' });
    await act(manager, 'w', { kind: 'bind_session', agentId: 'degraded', role: 'worker', parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'deliver', nextExpectedAt: null });
    await act(manager, 'w', { kind: 'assign', agentId: 'runner', nextStep: 'Return.', send: sendFor(config, 'runner', instanceId) });
    await manager.refresh();
    // The OwnerInbox regenerates an `unavailable` row for this exact degraded
    // binding, so the recoverable decision is asserted through a fresh projection
    // (the per-work read cache is bypassed) to prove it holds in steady state.
    assert.ok(manager.ownerInbox.list().events.some((event) => event.kind === 'unavailable' && event.agentId === 'degraded'));
    const work = (await freshProjection(manager, ledger, base).list()).works[0]!;
    assert.equal(work.error, null);
    assert.equal(work.phase, 'recoverable');
    assert.equal(work.waitingFor, 'worker');
    assert.match(work.waitEvidence ?? '', /state\.json/);
    assert.match(work.waitEvidence ?? '', /保留身分：degraded\/worker-session/);
    assert.match(work.waitEvidence ?? '', /恢復點：/);
    // The degraded pending record still outranks the inbox row it produced.
    assert.doesNotMatch(work.waitEvidence ?? '', /inbox:unavailable/);
  } finally { await manager.stop(); store.close(); rmSync(base, { recursive: true, force: true }); }
});

test('a failure while acquiring the shared observation snapshot degrades only the affected row (GH1189 F4)', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-row-error-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
  ], works: [{ id: 'w1', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' },
    { id: 'w2', projectId: 'p', taskId: 8, workspace: root, ownerAgentId: 'owner' }] });
  const { manager, store } = open(config, ledger, root, adapterFor((binding) => live(`${binding.id}-session`), instanceId));
  try {
    const original = manager.agentViews.bind(manager);
    let calls = 0;
    // The snapshot seam is `agentViews()` (in-memory; the store-reading
    // `overview()` is the browser projection and is no longer on this path), but
    // the invariant is unchanged: it is acquired inside the per-row try, so a
    // throw here degrades one row instead of rejecting the whole response.
    manager.agentViews = () => { if (calls++ === 0) throw new Error('snapshot unavailable'); return original(); };
    const snapshot = await manager.works.list();
    assert.equal(snapshot.works.length, 2);
    const errored = snapshot.works.filter((w) => w.error !== null);
    assert.equal(errored.length, 1);
    assert.equal(snapshot.works.filter((w) => w.error === null).length, 1);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('owner-bound edda return status is projected in the work view and never fails a row', async () => {
  const root = mkdtempSync(join(tmpdir(), 'work-graph-owner-return-')), instanceId = randomUUID(), ledger = new MemoryLedger();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
  ], works: [{ id: 'w1', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner', ownerRef: 'assistant/owner' },
    { id: 'w2', projectId: 'p', taskId: 8, workspace: root, ownerAgentId: 'owner' }] });
  ledger.returnsImpl = async (binding) => binding.id === 'w1'
    ? { owner: 'assistant/owner', holder: 'holder-session', pending: 1, total: 2, dropped: 0, error: null,
        matched: [{ id: 'return-1', work: '7', status: 'done', result: '已回件', postedAt: at(400) }] }
    : null;
  const { manager, store } = open(config, ledger, root, adapterFor((binding) => live(`${binding.id}-session`), instanceId));
  try {
    await act(manager, 'w1', { kind: 'initialize' as const, nextStep: 'One.' });
    await act(manager, 'w2', { kind: 'initialize' as const, nextStep: 'Two.' });
    const works = (await manager.works.list()).works;
    const first = works.find((w) => w.id === 'w1')!, second = works.find((w) => w.id === 'w2')!;
    assert.equal(first.error, null);
    assert.equal(first.ownerReturn?.owner, 'assistant/owner');
    assert.equal(first.ownerReturn?.matched[0]?.status, 'done');
    // The work row is not failed by a return read; a missing ownerRef is null.
    assert.equal(second.error, null);
    assert.equal(second.ownerReturn, null);
    ledger.returnsImpl = async () => ({ owner: 'assistant/owner', holder: null, pending: 0, total: null, matched: [], dropped: 0, error: '負責人回件狀態暫時無法讀取；未自動重試。' });
    // A fresh projection avoids the per-work read cache and re-reads the return.
    const fresh = freshProjection(manager, ledger, root);
    const degraded = (await fresh.list()).works.find((w) => w.id === 'w1')!;
    assert.equal(degraded.error, null);
    assert.match(degraded.ownerReturn?.error ?? '', /無法讀取/);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('the fixed-argument edda return read parses, filters and fails closed', async () => {
  const calls: string[][] = [];
  const ledger = new EddaWorkflowLedger(async (_cwd, args) => {
    calls.push(args);
    if (args[1] === 'status') return JSON.stringify({ owner: 'assistant/owner', holder: 'holder-session', holderSince: at(1), pending: 2, total: 3 });
    return JSON.stringify({ owner: 'assistant/owner', count: 3, pending: [
      { version: 1, id: 'm1', owner: 'assistant/owner', work: '7', status: 'done', result: 'ok', posted_by_session: 's', posted_at: at(2) },
      { version: 1, id: 'm2', owner: 'assistant/owner', work: 'other', status: 'failed', result: null, posted_by_session: 's', posted_at: at(2) },
      { version: 1, id: 'm3', owner: 'assistant/owner', work: '7', status: 'weird', result: null, posted_by_session: 's', posted_at: at(2) },
    ] });
  });
  const binding: WorkBinding = { id: 'w', projectId: 'p', taskId: 7, workspace: '/ws', ownerAgentId: 'a', ownerRef: 'assistant/owner' };
  const view = await ledger.returns(binding);
  assert.deepEqual(view, { owner: 'assistant/owner', holder: 'holder-session', pending: 2, total: 3, dropped: 1, error: null,
    matched: [{ id: 'm1', work: '7', status: 'done', result: 'ok', postedAt: at(2) }] });
  assert.deepEqual(calls[0], ['return', 'status', '--owner', 'assistant/owner', '--json']);
  assert.deepEqual(calls[1], ['return', 'pending', '--owner', 'assistant/owner', '--json']);
  const broken = new EddaWorkflowLedger(async () => { throw new Error('missing executable'); });
  const degraded = await broken.returns(binding);
  assert.equal(degraded?.error === null, false);
  assert.deepEqual(degraded?.matched, []);
  assert.equal(degraded?.dropped, 0);
  assert.equal(await broken.returns({ id: 'w', projectId: 'p', taskId: 7, workspace: '/ws', ownerAgentId: 'a' }), null);
});

test('the owner-return read matches across the scan window before bounding the display', async () => {
  const many = (count: number, work = '7', status = 'done'): unknown[] =>
    Array.from({ length: count }, (_, index) => ({ version: 1, id: `m${index}`, owner: 'assistant/owner', work, status, result: 'ok', posted_by_session: 's', posted_at: at(index + 1) }));
  const ledgerFor = (pending: unknown[]): EddaWorkflowLedger =>
    new EddaWorkflowLedger(async (_cwd, args) => args[1] === 'status'
      ? JSON.stringify({ owner: 'assistant/owner', holder: 'holder-session', pending: pending.length, total: pending.length })
      : JSON.stringify({ owner: 'assistant/owner', count: pending.length, pending }));
  const binding: WorkBinding = { id: 'w', projectId: 'p', taskId: 7, workspace: '/ws', ownerAgentId: 'a', ownerRef: 'assistant/owner' };

  // 25 matching returns: the 20 NEWEST are shown (so the phase's newest-wins rule
  // cannot lose a fresher return to the display bound) and the 5-item overflow is
  // counted rather than hidden.
  const manyView = await ledgerFor(many(25)).returns(binding);
  assert.equal(manyView?.matched.length, 20);
  assert.equal(manyView?.dropped, 5);
  assert.equal(manyView?.matched[0]?.id, 'm24');
  assert.equal(manyView?.matched[19]?.id, 'm5');

  // Another work's return is never matched and never counted as dropped.
  const otherView = await ledgerFor([...many(1, '999'), ...many(1, '999', 'weird')]).returns(binding);
  assert.equal(otherView?.matched.length, 0);
  assert.equal(otherView?.dropped, 0);

  // A matched item whose status is outside the bounded vocabulary is dropped.
  const weirdView = await ledgerFor(many(1, '7', 'weird')).returns(binding);
  assert.equal(weirdView?.matched.length, 0);
  assert.equal(weirdView?.dropped, 1);

  // Pending items past the scan bound were not examined and are counted as unsafe.
  const wideView = await ledgerFor(many(205)).returns(binding);
  assert.equal(wideView?.matched.length, 20);
  assert.equal(wideView?.dropped, 185);
});

test('the wait-target surface has no tool entry and no tool member (GH1189 F2)', () => {
  assert.ok(!Object.keys(waitTargets).includes('tool'));
  // A compile-time assertion: 'tool' is no longer assignable to WorkWaitingFor.
  // @ts-expect-error 'tool' was removed from WorkWaitingFor (GH1189 F2)
  const tool: WorkWaitingFor = 'tool';
  assert.equal(tool, 'tool');
});
