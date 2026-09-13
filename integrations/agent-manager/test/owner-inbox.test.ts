import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { ManagerStore } from '../src/store.js';
import { OwnerInbox } from '../src/owner-inbox.js';
import { AgentManager } from '../src/manager.js';
import { parseConfig, selectionRevision } from '../src/config.js';
import { WorkflowLocks, type WorkflowLedger, type LedgerNote } from '../src/edda-workflow.js';
import type { AgentView, PiAdapter } from '../src/contracts.js';
import type { WorkView, WorkSessionBinding } from '../src/workflow-contracts.js';

const at = '2026-09-13T00:00:00.000Z';
const after = '2026-09-13T00:01:00.000Z';

test('owner handoff context has a byte ceiling and points to complete work without losing stored directions', () => {
  const root = mkdtempSync(join(tmpdir(), 'owner-context-budget-')), store = new ManagerStore(root);
  try {
    const inbox = new OwnerInbox(store);
    const works = Array.from({ length: 24 }, (_, i) => ({ ...work(), id: `work-${i}`, taskId: i + 1,
      nextStep: '進度'.repeat(1000), evidence: '證據'.repeat(2000),
      pendingInstruction: { id: randomUUID(), operationId: randomUUID(), message: '待確認'.repeat(4000), acknowledgedAt: null, evidence: null },
      history: Array.from({ length: 30 }, () => ({ id: randomUUID(), kind: 'block', at, summary: '歷史'.repeat(2000) })) }));
    inbox.refresh([], { works, generatedAt: after });
    const packet = inbox.context('owner');
    assert.ok(Buffer.byteLength(JSON.stringify(packet)) <= 65536);
    assert.equal(packet.truncated, true); assert.ok(packet.works.length > 0);
    assert.equal(packet.works[0]!.detailPath, '/api/works/work-0');
    assert.equal(packet.works[0]!.work.pendingInstruction!.acknowledgedAt, null);
    assert.equal(works[0]!.pendingInstruction.message.length, 12000);
    assert.equal(store.operations().length, 0);
  } finally { store.close(); rmSync(root, { recursive: true, force: true }); }
});
function binding(): WorkSessionBinding { return { id: randomUUID(), agentId: 'worker', sessionId: 'session', selectionRevision: 'revision', transport: 'pi', role: 'worker', parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'review reply', nextExpectedAt: null, boundAt: at, unboundAt: null }; }
function work(session = binding()): WorkView { return { id: 'work', projectId: 'p', taskId: 7, title: 'Task', taskStatus: 'running', taskReceipt: null, ownerAgentId: 'owner', assigneeAgentId: 'worker', nextStep: 'Review', stage: 'executing', revision: 'r', phase: 'waiting', waitingFor: 'worker', waitEvidence: 'session:worker/worker:idle', evidence: null, waitingReason: null, pendingInstruction: null, deliveryOperationId: null, deliveryStatus: null, updatedAt: at, error: null, history: [], lastActionId: null, confirmedActionId: null, sessions: [session] }; }
function agent(): AgentView { return { id: 'worker', name: 'worker', role: 'worker', projectId: 'p', workspace: '/private/workspace', transport: 'pi', selectionRevision: 'revision', summary: null, summaryUpdatedAt: at, summaryError: null, state: 'idle', instanceId: randomUUID(), observedAt: after, heartbeatAt: after, lastProgressAt: after, lastEvent: null, source: 'live', stale: false, reason: null, model: null, usage: null, capabilities: { conversation: true, send: true }, latestMessage: null, sessionEvidence: { sessionId: 'session', evidenceSource: 'live', historyComplete: true, events: [{ id: 'native-end', kind: 'reply_ended', at: after, turnId: 'turn', category: null, httpStatus: null }] } }; }

test('owner inbox persists deduplicated native events, acknowledgement, ownership and pending direction through restart', () => {
  const root = mkdtempSync(join(tmpdir(), 'owner-inbox-'));
  let store = new ManagerStore(root);
  try {
    let inbox = new OwnerInbox(store); const w = work(), a = agent();
    w.pendingInstruction = { id: randomUUID(), operationId: randomUUID(), message: 'new direction', acknowledgedAt: null, evidence: null };
    inbox.refresh([a], { works: [w], generatedAt: after });
    inbox.refresh([a], { works: [w], generatedAt: after });
    assert.equal(inbox.list().events.length, 1); assert.equal(inbox.list().events[0]!.deliveryRecorded, false);
    assert.equal(inbox.context('owner').works[0]!.summaryStale, true);
    const event = inbox.list().events[0]!, ack = { eventId: event.id, actionId: randomUUID(), evidence: 'Observed the review reply.' };
    const first = inbox.acknowledge(ack); assert.equal(inbox.acknowledge(ack).acknowledgedAt, first.acknowledgedAt);
    assert.throws(() => inbox.acknowledge({ ...ack, evidence: 'changed' }), /編號/);
    store.close(); store = new ManagerStore(root); inbox = new OwnerInbox(store);
    w.ownerAgentId = 'replacement'; inbox.refresh([a], { works: [w], generatedAt: after });
    assert.equal(inbox.list().events.length, 1); assert.equal(inbox.list('owner').events.length, 0);
    assert.equal(inbox.list('replacement').events[0]!.acknowledgementId, ack.actionId);
    assert.equal(inbox.context('replacement').works[0]!.work.pendingInstruction!.acknowledgedAt, null);
    assert.equal(store.operations().length, 0);
  } finally { store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('unknown source and binding mismatch stay observational; availability transitions and explicit deadline dedup', () => {
  const root = mkdtempSync(join(tmpdir(), 'owner-inbox-unknown-')); let store = new ManagerStore(root);
  try {
    let inbox = new OwnerInbox(store); const w = work(), a = agent(); w.sessions[0]!.nextExpectedAt = after;
    inbox.refresh([], { works: [w], generatedAt: after }, Date.parse(after));
    inbox.refresh([], { works: [w], generatedAt: after }, Date.parse(after) + 99999);
    assert.deepEqual(inbox.list().events.map(e => e.kind).sort(), ['overdue', 'unavailable']);
    store.close(); store = new ManagerStore(root); inbox = new OwnerInbox(store);
    inbox.refresh([], { works: [w], generatedAt: after }, Date.parse(after)); assert.equal(inbox.list().events.length, 2);
    a.selectionRevision = 'replacement'; inbox.refresh([a], { works: [w], generatedAt: after });
    assert.equal(inbox.list().events.filter(e => e.kind === 'reply_ended').length, 0);
    a.selectionRevision = 'revision'; inbox.refresh([a], { works: [w], generatedAt: after });
    inbox.refresh([], { works: [w], generatedAt: after });
    assert.equal(inbox.list().events.filter(e => e.kind === 'unavailable').length, 2);
    assert.equal(store.operations().length, 0);
  } finally { store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('provider errors normalize metadata and old native events cannot contaminate a new work binding', () => {
  const root = mkdtempSync(join(tmpdir(), 'owner-inbox-provider-')), store = new ManagerStore(root);
  try {
    const inbox = new OwnerInbox(store), w = work(), a = agent();
    a.sessionEvidence!.events = [
      { id: 'old', kind: 'reply_ended', at: '2026-09-12T00:00:00Z', turnId: null, category: null, httpStatus: null },
      { id: 'failure', kind: 'provider_error', at: after, turnId: null, category: 'secret bearer /private', httpStatus: 402 },
      { id: 'stop', kind: 'interrupted', at: after, turnId: null, category: null, httpStatus: null },
    ];
    inbox.refresh([a], { works: [w], generatedAt: after });
    assert.equal(inbox.list().events.length, 2); assert.equal(inbox.list().events.find(e => e.kind === 'provider_error')!.category, 'quota');
    assert.ok(!JSON.stringify(inbox.list()).includes('secret')); assert.ok(!JSON.stringify(inbox.context('owner')).includes('/private'));
    w.sessions[0]!.unboundAt = after; a.sessionEvidence!.events.push({ id: 'later', kind: 'reply_ended', at: after, turnId: null, category: null, httpStatus: null });
    inbox.refresh([a], { works: [w], generatedAt: after }); assert.equal(inbox.list().events.length, 2);
    inbox.refresh([a], { works: [], generatedAt: after }); assert.equal(inbox.list().events.length, 0);
  } finally { store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('last recorded native stop remains evidence when endpoint is unavailable and task remapping hides old inbox', () => {
  const root = mkdtempSync(join(tmpdir(), 'owner-inbox-recorded-')), store = new ManagerStore(root);
  try {
    const inbox = new OwnerInbox(store), w = work(), a = agent();
    a.source = 'unavailable'; a.state = 'stopped'; a.sessionEvidence!.evidenceSource = 'recorded';
    a.sessionEvidence!.events = [{ id: 'recorded-stop', kind: 'interrupted', at: after, turnId: null, category: null, httpStatus: null },
      { id: 'recorded-credit', kind: 'provider_error', at: after, turnId: null, category: 'credit_or_quota', httpStatus: null }];
    inbox.refresh([a], { works: [w], generatedAt: after });
    assert.deepEqual(inbox.list().events.map(e => e.kind).sort(), ['interrupted', 'provider_error', 'unavailable']);
    const event = inbox.list().events[0]!;
    assert.equal(inbox.list().events.find(e => e.kind === 'provider_error')!.category, 'quota');
    w.taskId = 8; w.sessions = [];
    inbox.refresh([a], { works: [w], generatedAt: after }); assert.equal(inbox.list().events.length, 0);
    assert.throws(() => inbox.acknowledge({ eventId: event.id, actionId: randomUUID(), evidence: 'wrong task' }), /已選取工作/);
  } finally { store.close(); rmSync(root, { recursive: true, force: true }); }
});

test('canonical work bindings snapshot selected identity and owner handoff preserves pending instruction without a send', async () => {
  const root = mkdtempSync(join(tmpdir(), 'owner-work-binding-')), store = new ManagerStore(root), notes: LedgerNote[] = [];
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'P' }], agents: ['owner', 'replacement', 'worker'].map(id => ({ id, name: id, role: id === 'worker' ? 'worker' : 'manager', projectId: 'p', registryRoot: root, sessionId: id, workspace: root })), works: [{ id: 'work', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  const ledger: WorkflowLedger = { task: async () => ({ id: 7, key: 'canonical-7', title: 'Task', status: 'running', receipt: null, updatedAt: at }), notes: async () => [...notes], append: async (_binding, text) => { notes.push({ id: `note-${notes.length}`, at, text }); } };
  let sends = 0;
  const adapter: PiAdapter = { observe: async () => agent(), conversation: async () => { throw new Error('unused'); }, send: async () => { sends++; throw new Error('must not send'); }, receipt: async () => null };
  const manager = new AgentManager(config, store, adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  try {
    let w = (await manager.works.list()).works[0]!;
    w = await manager.works.act('work', { actionId: randomUUID(), revision: w.revision, kind: 'initialize', nextStep: 'Review' });
    const action = { actionId: randomUUID(), revision: w.revision, kind: 'bind_session' as const, agentId: 'worker', role: 'reviewer' as const, parentAgentId: 'owner', reviewedSha: 'a'.repeat(40), expectedEvent: 'review', nextExpectedAt: after };
    w = await manager.works.act('work', action); assert.equal(w.sessions[0]!.sessionId, 'worker');
    assert.equal(w.sessions[0]!.selectionRevision, selectionRevision(config.agents[2]!));
    config.agents[2]!.sessionId = 'worker-replacement';
    w = await manager.works.act('work', action); assert.equal(w.sessions[0]!.sessionId, 'worker');
    w = await manager.works.act('work', { actionId: randomUUID(), revision: w.revision, kind: 'handoff_owner', ownerAgentId: 'replacement', evidence: 'bounded packet transferred' });
    assert.equal(w.ownerAgentId, 'replacement'); assert.equal(sends, 0); assert.equal(w.taskStatus, 'running');
    assert.ok(!JSON.stringify(w).includes('registryRoot'));
    await assert.rejects(() => manager.works.act('work', { actionId: randomUUID(), revision: w.revision, kind: 'handoff_owner', ownerAgentId: 'worker', evidence: 'invalid' }), /管理者/);
    w = await manager.works.act('work', { actionId: randomUUID(), revision: w.revision, kind: 'unbind_session', bindingId: action.actionId });
    assert.equal(w.sessions[0]!.unboundAt, at);
    await assert.rejects(() => manager.works.act('work', { ...action, actionId: randomUUID(), revision: w.revision, reviewedSha: null }), /SHA/);
  } finally { await manager.stop(); store.close(); rmSync(root, { recursive: true, force: true }); }
});
