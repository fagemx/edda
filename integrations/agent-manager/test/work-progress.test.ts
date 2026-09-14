import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { AgentManager } from '../src/manager.js';
import { ManagerStore } from '../src/store.js';
import { parseConfig, selectionRevision } from '../src/config.js';
import { WorkManager, deriveWorkProgress } from '../src/workflow.js';
import { WorkflowLocks, type CanonicalTask, type LedgerNote, type WorkflowLedger } from '../src/edda-workflow.js';
import type { AgentObservation, AgentView, PiAdapter, SendRequest } from '../src/contracts.js';
import type { OwnerInboxEvent, OwnerInboxKind } from '../src/owner-inbox-contracts.js';
import type { NativeSessionEvent } from '../src/session-contracts.js';
import type { WorkAction, WorkSessionBinding, WorkView } from '../src/workflow-contracts.js';

const at = (seconds: number): string => new Date(Date.UTC(2026, 8, 13, 0, 0, seconds)).toISOString();
const LATER = at(300);

function task(status = 'running'): CanonicalTask {
  return { id: 7, key: 'task-7', title: 'Task 7', status, receipt: null, updatedAt: at(0) };
}
function binding(role: WorkSessionBinding['role'], agentId: string): WorkSessionBinding {
  return { id: `binding-${agentId}`, agentId, sessionId: `${agentId}-session`, selectionRevision: 'revision', transport: 'pi', role,
    parentAgentId: null, reviewedSha: role === 'reviewer' ? 'a'.repeat(40) : null, expectedEvent: 'deliver the candidate',
    nextExpectedAt: null, boundAt: at(1), unboundAt: null };
}
function view(overrides: Partial<WorkView> = {}): WorkView {
  return { id: 'work', projectId: 'p', taskId: 7, title: 'Task 7', taskStatus: 'running', taskReceipt: null,
    ownerAgentId: 'owner', assigneeAgentId: 'worker', nextStep: 'Deliver', stage: 'executing', revision: 'r',
    phase: 'uninitialized', waitingFor: null, waitEvidence: null, attempt: 1, ownerReturn: null, registry: { relation: 'in_root', message: '測試用來源關聯' },
    evidence: null, waitingReason: null, pendingInstruction: null,
    deliveryOperationId: 'op-1', deliveryStatus: 'started', updatedAt: at(2), error: null, history: [], lastActionId: null,
    confirmedActionId: null, sessions: [binding('worker', 'worker')], ...overrides };
}
function observation(id: string, overrides: Partial<AgentView> = {}): AgentView {
  return { id, name: id, role: 'worker', projectId: 'p', workspace: '/workspace', transport: 'pi', selectionRevision: 'revision',
    summary: null, summaryUpdatedAt: null, summaryError: null, state: 'idle', instanceId: 'instance', observedAt: at(200),
    heartbeatAt: at(200), lastProgressAt: null, lastEvent: null, source: 'live', stale: false, reason: null, degraded: null, model: null, usage: null,
    capabilities: { conversation: true, send: true }, latestMessage: null, ownerMailbox: null,
    sessionEvidence: { sessionId: `${id}-session`, evidenceSource: 'live', historyComplete: false, events: [] }, ...overrides };
}
function inboxEvent(kind: OwnerInboxKind, when: string, summary = '原生事件', bindingId = 'binding-worker'): OwnerInboxEvent {
  return { id: `event-${kind}-${when}-${bindingId}`, workId: 'work', projectId: 'p', taskId: 7, bindingId, agentId: 'worker',
    sessionId: 'worker-session', nativeEventId: `native-${kind}`, kind, at: when, summary, category: null, httpStatus: null,
    deliveryRecorded: false, acknowledgedAt: null, acknowledgementId: null, evidence: null, ownerAgentId: 'owner' };
}
const derive = (work: WorkView, agents: AgentView[] = [], inbox: OwnerInboxEvent[] = []) => deriveWorkProgress({ task: task(work.taskStatus), view: work, agents, inbox });

test('task running with no live bound child is never working and names the pending role', () => {
  const work = view({ stage: 'assigned', deliveryStatus: 'started' });
  const unavailableAgent = observation('worker', { state: 'unavailable', source: 'unavailable', stale: true, heartbeatAt: null });
  delete unavailableAgent.sessionEvidence;
  const waiting = derive(work, [unavailableAgent]);
  assert.notEqual(waiting.phase, 'working');
  assert.equal(waiting.phase, 'waiting');
  assert.equal(waiting.waitingFor, 'worker');
  assert.match(waiting.waitEvidence ?? '', /delivery:started/);

  const settled = derive(view({ stage: 'awaiting_delivery', deliveryStatus: 'settled' }), [observation('worker')]);
  assert.equal(settled.phase, 'waiting');
  assert.equal(settled.waitingFor, 'worker');

  // Handed over but no observed start yet, and no child is claimed to work.
  const assigned = derive(view({ stage: 'assigned', deliveryStatus: 'prepared' }), [observation('worker')]);
  assert.equal(assigned.phase, 'assigned');
  assert.equal(assigned.waitingFor, 'worker');
});

test('the role chain separates waiting on the worker from waiting on the verifier', () => {
  const worker = derive(view({ stage: 'executing', sessions: [binding('worker', 'worker')] }), [observation('worker')]);
  assert.equal(worker.waitingFor, 'worker');

  const review = view({ stage: 'delivered', sessions: [binding('worker', 'worker'), binding('reviewer', 'reviewer')], deliveryStatus: 'settled' });
  const verifier = derive(review, [observation('worker'), observation('reviewer', { role: 'worker' })]);
  assert.equal(verifier.phase, 'delivered');
  assert.equal(verifier.waitingFor, 'verifier');
  assert.match(verifier.waitEvidence ?? '', /reviewer/);

  // A delivery with no reviewer and no live worker names no target at all.
  const ownerAccepts = derive(view({ stage: 'delivered', sessions: [binding('worker', 'worker')], deliveryStatus: 'settled' }), [observation('worker')]);
  assert.equal(ownerAccepts.phase, 'delivered');
  assert.equal(ownerAccepts.waitingFor, null);
  assert.match(ownerAccepts.waitEvidence ?? '', /未登記審查 session/);
});

test('a fresher native interruption overrides a stale manual executing stage', () => {
  const work = view({ stage: 'executing', updatedAt: at(2) });
  const interrupted = derive(work, [observation('worker')], [inboxEvent('interrupted', LATER, '代理回合被中斷；工作結果需要確認。')]);
  assert.equal(interrupted.phase, 'interrupted');
  // An interruption preserves the pending hand-off instead of dropping it.
  assert.equal(interrupted.waitingFor, 'worker');
  assert.match(interrupted.waitEvidence ?? '', new RegExp(`inbox:interrupted @ ${LATER}`));
  assert.match(interrupted.waitEvidence ?? '', /工作結果需要確認/);

  // An older interruption does not override a newer manual record.
  const stale = derive(work, [observation('worker')], [inboxEvent('interrupted', at(1))]);
  assert.equal(stale.phase, 'waiting');
  assert.equal(stale.waitingFor, 'worker');
  assert.doesNotMatch(stale.waitEvidence ?? '', /inbox:/);
});

test('liveness stays a separate process fact from the wait reason', () => {
  const work = view({ stage: 'executing' });
  const staleWorker = observation('worker', { state: 'executing_tool', stale: true, heartbeatAt: at(30), lastProgressAt: at(31) });
  const stale = derive(work, [staleWorker]);
  assert.notEqual(stale.phase, 'working');
  assert.equal(stale.phase, 'waiting');
  assert.equal(stale.waitingFor, 'worker');
  assert.doesNotMatch(stale.waitEvidence ?? '', /2026-09-13T00:00:30/);
  assert.doesNotMatch(stale.waitEvidence ?? '', /heartbeat|stale|observedAt|source/i);

  const fresh = derive(work, [observation('worker', { state: 'executing_tool', heartbeatAt: at(200) })]);
  assert.equal(fresh.phase, 'working');
  // A working child is not waiting on the operator; the tool call is liveness.
  assert.equal(fresh.waitingFor, 'none');
  assert.doesNotMatch(fresh.waitEvidence ?? '', /2026-09-13T00:03:20/);

  const running = derive(work, [observation('worker', { state: 'running' })]);
  assert.equal(running.phase, 'working');
  assert.equal(running.waitingFor, 'none');
});

test('an unlinked or unavailable observation never guesses a role', () => {
  const unlinked = derive(view({ stage: 'executing', sessions: [binding('worker', 'worker')] }), []);
  assert.equal(unlinked.phase, 'waiting');
  assert.equal(unlinked.waitingFor, 'worker'); // the recorded binding is the role, not a guess
  assert.match(unlinked.waitEvidence ?? '', /unlinked/);

  const changedInstance = derive(view({ stage: 'executing', sessions: [binding('worker', 'worker')] }), [observation('worker', { selectionRevision: 'other-revision' })]);
  assert.match(changedInstance.waitEvidence ?? '', /unlinked/);

  const nothingBound = view({ stage: 'executing', assigneeAgentId: null, sessions: [], deliveryStatus: null });
  const unknown = derive(nothingBound, []);
  assert.equal(unknown.waitingFor, null);
  assert.match(unknown.waitEvidence ?? '', /unlinked:沒有可判定的執行角色/);
});

test('terminal native task and delivery states are projected explicitly', () => {
  const done = derive(view({ taskStatus: 'done' }));
  assert.equal(done.phase, 'completed');
  assert.equal(done.waitingFor, 'none');

  const failed = derive(view({ taskStatus: 'failed' }));
  assert.equal(failed.phase, 'failed');
  assert.equal(failed.waitingFor, 'none');

  const blocked = derive(view({ taskStatus: 'blocked' }));
  assert.equal(blocked.phase, 'blocked');
  assert.equal(blocked.waitingFor, 'dependency');

  const undeliverable = derive(view({ stage: 'assigned', deliveryStatus: 'failed', waitingReason: '模型供應商回報錯誤；未自動重送。' }), [observation('worker')]);
  assert.equal(undeliverable.phase, 'failed');
  assert.equal(undeliverable.waitingFor, 'user_decision');
  assert.match(undeliverable.waitEvidence ?? '', /delivery:failed/);

  const waitingUser = derive(view({ stage: 'executing' }), [observation('worker', { state: 'waiting_user' })]);
  assert.equal(waitingUser.phase, 'waiting');
  assert.equal(waitingUser.waitingFor, 'user_decision');

  const stopped = derive(view({ stage: 'executing' }), [observation('worker', { state: 'stopped', source: 'unavailable', stale: true, reason: '上次管理紀錄為已停止；目前沒有即時連線。' })]);
  assert.equal(stopped.phase, 'interrupted');
  assert.equal(stopped.waitingFor, 'worker');
  assert.match(stopped.waitEvidence ?? '', /worker:stopped/);
});

test('a stopped session only interrupts the role the work waits on, never a terminal or delivered phase', () => {
  const stoppedWorker = observation('worker', { state: 'stopped', source: 'unavailable', stale: true, reason: '執行 session 已停止。' });
  const stoppedReviewer = observation('reviewer', { role: 'worker', state: 'stopped', source: 'unavailable', stale: true, reason: '審查 session 已停止。' });
  const delivered = view({ stage: 'delivered', deliveryStatus: 'settled', sessions: [binding('worker', 'worker'), binding('reviewer', 'reviewer')] });

  // Reproduced from the Round-1 review: a delivered work awaiting a verifier must
  // stay delivered and name the verifier whether the stopped session is the
  // reviewer or another role.
  const reviewerDown = derive(delivered, [observation('worker'), stoppedReviewer]);
  assert.equal(reviewerDown.phase, 'delivered');
  assert.equal(reviewerDown.waitingFor, 'verifier');
  const workerDown = derive(delivered, [stoppedWorker, observation('reviewer', { role: 'worker' })]);
  assert.equal(workerDown.phase, 'delivered');
  assert.equal(workerDown.waitingFor, 'verifier');

  // A terminal or delivery fact is never overridden by a stopped session.
  assert.equal(derive(view({ stage: 'accepted' }), [stoppedWorker]).phase, 'accepted');
  assert.equal(derive(view({ stage: 'assigned', deliveryStatus: 'failed' }), [stoppedWorker]).phase, 'failed');

  // The active hand-off is the one case where a stopped pending session interrupts.
  const activeStopped = derive(view({ stage: 'executing' }), [stoppedWorker]);
  assert.equal(activeStopped.phase, 'interrupted');
  assert.equal(activeStopped.waitingFor, 'worker');
});

test('an unparseable observation timestamp cannot let an interruption override live work', () => {
  const work = view({ stage: 'executing', updatedAt: at(2) });
  const live = observation('worker', { state: 'running', heartbeatAt: null, observedAt: 'not-a-time' });
  const result = derive(work, [live], [inboxEvent('interrupted', LATER)]);
  assert.equal(result.phase, 'working');
  assert.equal(result.waitingFor, 'none');
});

test('a fresh inbox liveness event does not override a delivered phase, but does interrupt an active one', () => {
  // The wired OwnerInbox records a fresh `unavailable` row for exactly a bound
  // session whose source is unavailable; for a delivered work that is liveness,
  // not a new phase, and the pending verifier hand-off must be preserved.
  const delivered = view({ stage: 'delivered', deliveryStatus: 'settled', updatedAt: at(2), sessions: [binding('worker', 'worker'), binding('reviewer', 'reviewer')] });
  const stayed = derive(delivered, [observation('worker'), observation('reviewer', { role: 'worker', source: 'unavailable', stale: true })],
    [inboxEvent('unavailable', LATER, 'Session 證據不可用或身分已改變；未自動重新派工。')]);
  assert.equal(stayed.phase, 'delivered');
  assert.equal(stayed.waitingFor, 'verifier');

  // The same fresh event on an active hand-off is still an interruption.
  const active = derive(view({ stage: 'executing', updatedAt: at(2) }), [observation('worker')], [inboxEvent('unavailable', LATER)]);
  assert.equal(active.phase, 'interrupted');
  assert.equal(active.waitingFor, 'worker');
});

class ClockLedger implements WorkflowLedger {
  private entries: LedgerNote[] = [];
  private clock = 0;
  async task(): Promise<CanonicalTask> { return task(); }
  async notes(): Promise<LedgerNote[]> { return [...this.entries]; }
  async append(_binding: unknown, text: string): Promise<void> { this.clock += 1; this.entries.push({ id: `evt-${this.clock}`, at: at(this.clock), text }); }
}

test('the manager wires native task/session/inbox evidence into the projected phase', async () => {
  const root = mkdtempSync(join(tmpdir(), 'manager-progress-')), instanceId = randomUUID(), ledger = new ClockLedger();
  const config = parseConfig({ version: 1, projects: [{ id: 'p', name: 'Project' }], agents: [
    { id: 'owner', name: 'Owner', projectId: 'p', role: 'manager', registryRoot: root, workspace: root, sessionId: 'owner-session' },
    { id: 'worker', name: 'Worker', projectId: 'p', role: 'worker', registryRoot: join(root, 'registry'), workspace: root, sessionId: 'worker-session' },
  ], works: [{ id: 'work', projectId: 'p', taskId: 7, workspace: root, ownerAgentId: 'owner' }] });
  const observations = new Map<string, AgentObservation>();
  const fresh = (id: string, events: NativeSessionEvent[]): AgentObservation => ({
    state: 'idle', instanceId, observedAt: at(200), heartbeatAt: at(200), lastProgressAt: null, lastEvent: null, source: 'live', stale: false,
    reason: null, degraded: null, model: null, usage: null, capabilities: { send: true, conversation: true }, latestMessage: null, ownerMailbox: null,
    sessionEvidence: { sessionId: `${id}-session`, evidenceSource: 'live', historyComplete: false, events } });
  observations.set('owner', fresh('owner', [])); observations.set('worker', fresh('worker', []));
  const adapter: PiAdapter = { observe: async (target) => observations.get(target.id)!,
    conversation: async () => ({ entries: [], instanceId, cursor: null, headCursor: null, hasMore: false, observedAt: at(200), source: 'live' }),
    send: async (target, request: SendRequest) => ({ id: request.operationId, sessionId: target.sessionId, instanceId, status: 'started' }), receipt: async () => null };
  const store = new ManagerStore(root), manager = new AgentManager(config, store, adapter, { ledger, locks: new WorkflowLocks(join(root, 'locks')) });
  const projection = () => new WorkManager(manager, ledger, new WorkflowLocks(join(root, 'locks')));
  try {
    await manager.refresh();
    let work = (await manager.works.list()).works[0]!;
    work = await manager.works.act('work', { kind: 'initialize', actionId: randomUUID(), revision: work.revision, nextStep: 'Assign implementation.' });
    work = await manager.works.act('work', { kind: 'bind_session', actionId: randomUUID(), revision: work.revision, agentId: 'worker', role: 'worker',
      parentAgentId: 'owner', reviewedSha: null, expectedEvent: 'deliver the candidate', nextExpectedAt: null });
    assert.equal(work.sessions.filter((s) => !s.unboundAt).length, 1);
    const worker = config.agents.find((a) => a.id === 'worker')!;
    const send: SendRequest = { operationId: randomUUID(), selectionRevision: selectionRevision(worker), instanceId, basisCursor: null, mode: 'followUp', message: 'Execute the bounded next step.' };
    const assign: WorkAction = { kind: 'assign', actionId: randomUUID(), revision: work.revision, agentId: 'worker', nextStep: 'Return a tested candidate.', send };
    work = await manager.works.act('work', assign);
    assert.equal(work.stage, 'executing');
    assert.equal(work.deliveryStatus, 'started');

    // The task rail says running and the manual ledger says executing, but no
    // bound child is working: the native phase must not claim `working`.
    const idle = (await projection().list()).works[0]!;
    assert.equal(idle.taskStatus, 'running');
    assert.equal(idle.phase, 'waiting');
    assert.equal(idle.waitingFor, 'worker');

    // A live session observed after the manual record is what makes it `working`.
    observations.set('worker', { ...observations.get('worker')!, state: 'running' });
    await manager.refresh();
    const busy = (await projection().list()).works[0]!;
    assert.equal(busy.phase, 'working');
    assert.equal(busy.waitingFor, 'none');

    // A fresher native interruption is observed through the owner inbox and wins.
    observations.set('worker', fresh('worker', [{ id: 'native-1', kind: 'interrupted', at: LATER, turnId: null, category: null, httpStatus: null }]));
    await manager.refresh();
    assert.ok(manager.ownerInbox.list().events.some((event) => event.kind === 'interrupted'));
    const interrupted = (await projection().list()).works[0]!;
    assert.equal(interrupted.stage, 'executing');
    assert.equal(interrupted.phase, 'interrupted');
    assert.equal(interrupted.waitingFor, 'worker');
    assert.match(interrupted.waitEvidence ?? '', /inbox:interrupted/);
  } finally {
    await manager.stop(); store.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test('an accepted delivery receipt without an observed start is launched, not assigned or working', () => {
  for (const status of ['accepted', 'queued', 'unconfirmed'] as const) {
    const launched = derive(view({ stage: 'assigned', deliveryStatus: status }), [observation('worker')]);
    assert.equal(launched.phase, 'launched');
    assert.equal(launched.waitingFor, 'worker');
    assert.match(launched.waitEvidence ?? '', new RegExp(`delivery:${status}`));
  }
  for (const status of ['prepared', 'unknown'] as const) {
    const assigned = derive(view({ stage: 'assigned', deliveryStatus: status }), [observation('worker')]);
    assert.equal(assigned.phase, 'assigned');
    assert.equal(assigned.waitingFor, 'worker');
  }
});

test('a delivered hand-off stays delivered when the bound worker waits on the operator (GH1189 F1)', () => {
  const work = view({ stage: 'delivered', deliveryStatus: 'settled', sessions: [binding('worker', 'worker'), binding('reviewer', 'reviewer')] });
  const result = derive(work, [observation('worker', { state: 'waiting_user' }), observation('reviewer', { role: 'worker' })]);
  assert.equal(result.phase, 'delivered');
  assert.equal(result.waitingFor, 'verifier');
  assert.doesNotMatch(result.waitEvidence ?? '', /waiting_user/);
});

test('a live bound child never masks a terminal native task fact (GH1181 leftover)', () => {
  const live = [observation('worker', { state: 'running' })];
  const failed = derive(view({ taskStatus: 'failed' }), live);
  assert.equal(failed.phase, 'failed');
  assert.equal(failed.waitingFor, 'none');
  const blocked = derive(view({ taskStatus: 'blocked' }), live);
  assert.equal(blocked.phase, 'blocked');
  assert.equal(blocked.waitingFor, 'dependency');
  const done = derive(view({ taskStatus: 'done', stage: 'executing' }), live);
  assert.equal(done.phase, 'completed');
  const receipt = derive(view({ taskStatus: 'running', stage: 'executing', deliveryStatus: 'failed' }), live);
  assert.equal(receipt.phase, 'failed');
  assert.equal(receipt.waitingFor, 'user_decision');
});

test('a degraded pending session is recoverable with its identity, the unreadable record and a recovery point', () => {
  const degraded = observation('worker', { state: 'unavailable', source: 'unavailable', stale: true,
    degraded: { code: 'record_unavailable', record: 'state.json', message: 'State record unreadable', recovery: 'Use the identity shown; inspect the owned session directory before any resume.' } });
  const result = derive(view({ stage: 'executing' }), [degraded]);
  assert.equal(result.phase, 'recoverable');
  assert.equal(result.waitingFor, 'worker');
  assert.match(result.waitEvidence ?? '', /state\.json/);
  assert.match(result.waitEvidence ?? '', /保留身分：worker\/worker-session/);
  assert.match(result.waitEvidence ?? '', /恢復點：Use the identity shown/);
});

test('only an interruption for the pending binding interrupts the hand-off (GH1189 F3)', () => {
  const work = view({ stage: 'executing', updatedAt: at(2), sessions: [binding('worker', 'worker'), binding('reviewer', 'reviewer')] });
  const agents = [observation('worker'), observation('reviewer', { role: 'worker' })];
  const other = derive(work, agents, [inboxEvent('unavailable', LATER, '其他綁定的事件', 'binding-reviewer')]);
  assert.equal(other.phase, 'waiting');
  assert.equal(other.waitingFor, 'worker');
  const overdue = derive(work, agents, [inboxEvent('overdue', LATER, '其他綁定逾期', 'binding-reviewer')]);
  assert.equal(overdue.phase, 'waiting');
  const pending = derive(work, agents, [inboxEvent('unavailable', LATER, '待接手綁定的事件', 'binding-worker')]);
  assert.equal(pending.phase, 'interrupted');
  assert.equal(pending.waitingFor, 'worker');
});

test('a degraded pending session stays recoverable even with the inbox row it produces (#1196 F1)', () => {
  const degraded = observation('worker', { state: 'unavailable', source: 'unavailable', stale: true,
    degraded: { code: 'record_unavailable', record: 'state.json', message: 'State record unreadable', recovery: 'Use the identity shown.' } });
  // The OwnerInbox regenerates exactly this row for the same degraded binding.
  const inbox = [inboxEvent('unavailable', LATER, 'Session 證據不可用或身分已改變；未自動重新派工。', 'binding-worker')];
  const result = derive(view({ stage: 'executing', updatedAt: at(2) }), [degraded], inbox);
  assert.equal(result.phase, 'recoverable');
  assert.equal(result.waitingFor, 'worker');
  assert.match(result.waitEvidence ?? '', /state\.json/);
  assert.match(result.waitEvidence ?? '', /恢復點：/);
  assert.doesNotMatch(result.waitEvidence ?? '', /inbox:unavailable/);

  // A stopped (not degraded) or healthy pending session still reads through the
  // pending-scoped inbox interruption it produced.
  const stopped = observation('worker', { state: 'stopped', source: 'unavailable', stale: true, reason: '執行 session 已停止。' });
  assert.equal(derive(view({ stage: 'executing', updatedAt: at(2) }), [stopped], inbox).phase, 'interrupted');
  assert.equal(derive(view({ stage: 'executing', updatedAt: at(2) }), [observation('worker', { state: 'idle' })], inbox).phase, 'interrupted');
});

test('a fresher owner return outranks only the stale manual stage', () => {
  const ownerReturn = { owner: 'owner-ref', holder: null, pending: 1, total: 2, dropped: 0, error: null,
    mailbox: { kind: 'workspace' as const, label: 'a1b2c3d4e5f6', present: true }, notice: null,
    matched: [{ id: 'return-1', work: '7', status: 'done' as const, result: '原生回件已完成。', postedAt: at(400) }] };
  const fresher = derive(view({ stage: 'executing', updatedAt: at(300), ownerReturn }), [observation('worker')]);
  assert.equal(fresher.phase, 'completed');
  assert.equal(fresher.waitingFor, 'none');
  assert.match(fresher.waitEvidence ?? '', /return:done/);
  assert.match(fresher.waitEvidence ?? '', /return:等待負責人領取/);
  // An older return never overrides a newer manual action.
  const older = derive(view({ stage: 'executing', updatedAt: at(300), ownerReturn: { ...ownerReturn, matched: [{ ...ownerReturn.matched[0]!, postedAt: at(1) }] } }), [observation('worker')]);
  assert.equal(older.phase, 'waiting');
  // A native task fact still wins over the freshest return.
  const rail = derive(view({ taskStatus: 'failed', updatedAt: at(300), ownerReturn }), [observation('worker')]);
  assert.equal(rail.phase, 'failed');
  // A task rail `done` with a stale manual executing stage projects completed.
  const railDone = derive(view({ taskStatus: 'done', stage: 'executing', updatedAt: at(300), ownerReturn }), [observation('worker')]);
  assert.equal(railDone.phase, 'completed');
  // A failing return read is ignored rather than failing the projection.
  const failedRead = derive(view({ stage: 'executing', updatedAt: at(300), ownerReturn: { ...ownerReturn, matched: [], error: '暫時無法讀取。' } }), [observation('worker')]);
  assert.equal(failedRead.phase, 'waiting');
});
