import { randomUUID } from 'node:crypto';
import { ManagerError, type AgentBinding, type AgentView, type RuntimeState } from './contracts.js';
import { hash, object, parseConfig, parseSend, selectionRevision, slug, text, uuid } from './config.js';
import { EddaWorkflowLedger, WorkflowLocks, type CanonicalTask, type WorkflowLedger } from './edda-workflow.js';
import type { AgentManager } from './manager.js';
import type { OwnerInboxEvent, OwnerInboxKind } from './owner-inbox-contracts.js';
import type { WorkAction, WorkBinding, WorkPhase, WorkSessionBinding, WorkView, WorkWaitingFor, WorksView } from './workflow-contracts.js';

const PREFIX = 'edda.manager-work.v1 ';
interface WorkEvent { version: 1; taskKey: string; previous: string | null; action: WorkAction; fingerprint: string; target: AgentBinding | null; transportStoreId: string; priorFailedOperation: string | null }
interface ReadWork { task: CanonicalTask; events: Array<WorkEvent & { id: string; at: string }>; view: WorkView }
export function parseWorkAction(input: unknown): WorkAction {
  const r = object(input), base = { actionId: uuid(r.actionId), revision: text(r.revision, 64) };
  switch (r.kind) {
    case 'attach_continuity': {
      const ref = object(r.reference), capsuleId = text(ref.capsuleId, 100), localEventId = text(ref.localEventId, 100), originEventId = text(ref.originEventId, 100);
      if (!/^cap_[a-z0-9]+$/.test(capsuleId) || !/^evt_[a-z0-9]+$/.test(localEventId) || !/^evt_[a-z0-9]+$/.test(originEventId)) throw new ManagerError('INVALID_CONTINUITY_REFERENCE', '原生 capsule 參照格式不正確。');
      return { ...base, kind: r.kind, reference: { capsuleId, localEventId, originEventId } };
    }
    case 'initialize': return { ...base, kind: r.kind, nextStep: text(r.nextStep, 2000) };
    case 'assign': return { ...base, kind: r.kind, agentId: slug(r.agentId), nextStep: text(r.nextStep, 2000), send: parseSend(r.send) };
    case 'intervene': return { ...base, kind: r.kind, send: parseSend(r.send) };
    case 'acknowledge': return { ...base, kind: r.kind, instructionId: uuid(r.instructionId), evidence: text(r.evidence, 4000) };
    case 'deliver': return { ...base, kind: r.kind, evidence: text(r.evidence, 4000), nextStep: text(r.nextStep, 2000) };
    case 'accept': return { ...base, kind: r.kind, evidence: text(r.evidence, 4000) };
    case 'block': return { ...base, kind: r.kind, reason: text(r.reason, 2000), nextStep: text(r.nextStep, 2000) };
    case 'bind_session': {
      if (!['manager', 'worker', 'reviewer'].includes(String(r.role))) throw new ManagerError('INVALID_ROLE', '不支援的 session 角色。');
      const reviewedSha = r.reviewedSha == null ? null : text(r.reviewedSha, 40);
      if ((reviewedSha !== null && !/^[a-f0-9]{40}$/.test(reviewedSha)) || (r.role === 'reviewer' && reviewedSha === null)) throw new ManagerError('INVALID_SHA', '審查 session 需要完整 SHA。');
      const nextExpectedAt = r.nextExpectedAt == null ? null : text(r.nextExpectedAt, 40);
      if (nextExpectedAt !== null && !Number.isFinite(Date.parse(nextExpectedAt))) throw new ManagerError('INVALID_DEADLINE', '預期回報時間無效。');
      return { ...base, kind: r.kind, agentId: slug(r.agentId), role: r.role as 'manager' | 'worker' | 'reviewer', parentAgentId: r.parentAgentId == null ? null : slug(r.parentAgentId), reviewedSha, expectedEvent: text(r.expectedEvent, 500), nextExpectedAt };
    }
    case 'unbind_session': return { ...base, kind: r.kind, bindingId: uuid(r.bindingId) };
    case 'handoff_owner': return { ...base, kind: r.kind, ownerAgentId: slug(r.ownerAgentId), evidence: text(r.evidence, 4000) };
    default: throw new ManagerError('INVALID_ACTION', '不支援的工作操作。');
  }
}
function empty(binding: WorkBinding): WorkView {
  return { id: binding.id, projectId: binding.projectId, taskId: binding.taskId, title: `Edda #${binding.taskId}`, taskStatus: 'unknown', taskReceipt: null,
    ownerAgentId: binding.ownerAgentId, assigneeAgentId: null, nextStep: '設定下一步並開始追蹤。', stage: 'uninitialized', revision: '', evidence: null,
    phase: 'uninitialized', waitingFor: null, waitEvidence: null,
    waitingReason: null, pendingInstruction: null, deliveryOperationId: null, deliveryStatus: null, updatedAt: null, error: null, history: [], lastActionId: null, confirmedActionId: null, sessions: [] };
}
// --- Native phase projection (GH1181) ---------------------------------------
// The `manager-work` ledger records what a human or agent *said*; the native
// task rail, delivery receipts, observed sessions and owner-inbox events record
// what is *observed*. `phase`/`waitingFor` are semantic and come from those
// native sources; heartbeat, source and staleness stay process liveness and are
// never folded into the wait reason.
const INTERRUPTING_INBOX: readonly OwnerInboxKind[] = ['interrupted', 'provider_error', 'unavailable', 'overdue'];
const PROGRESS_STATES: readonly RuntimeState[] = ['running', 'executing_tool'];
const MAX_WAIT_EVIDENCE = 300;
export interface NativeWorkInputs { task: CanonicalTask; view: WorkView; agents: AgentView[]; inbox: OwnerInboxEvent[] }
export interface NativeWorkProgress { phase: WorkPhase; waitingFor: WorkWaitingFor; waitEvidence: string | null }
interface BoundSession { session: WorkSessionBinding; agent: AgentView | null }
/** A binding is only this session when the observed agent still carries its
 *  selection revision, transport and session id — the identity rule the owner
 *  inbox already applies. Anything else is `unlinked` and never live evidence. */
function linkedAgent(view: WorkView, agents: AgentView[], session: WorkSessionBinding): AgentView | null {
  const agent = agents.find(a => a.id === session.agentId && a.projectId === view.projectId);
  if (!agent || agent.selectionRevision !== session.selectionRevision || agent.transport !== session.transport) return null;
  return agent.sessionEvidence?.sessionId === session.sessionId ? agent : null;
}
const progressing = (agent: AgentView | null): boolean => !!agent && agent.source === 'live' && !agent.stale && PROGRESS_STATES.includes(agent.state);
const observedAs = (agent: AgentView | null, state: RuntimeState): boolean => !!agent && agent.source === 'live' && !agent.stale && agent.state === state;
/** Wait target from the recorded role chain. A missing relation resolves to
 *  `null` with an `unlinked` evidence line — never to a guessed role. */
function pendingSide(view: WorkView, bound: BoundSession[], agents: AgentView[]): { side: WorkWaitingFor; evidence: string | null } {
  if (view.stage === 'uninitialized') return { side: null, evidence: 'ledger:uninitialized' };
  if (view.stage === 'ready') return { side: null, evidence: 'ledger:ready（尚未指派執行者）' };
  if (view.stage === 'accepted') return { side: 'none', evidence: 'ledger:accepted' };
  if (view.stage === 'blocked') return { side: 'dependency', evidence: view.waitingReason ? `ledger:blocked ${view.waitingReason}` : 'ledger:blocked' };
  const reviewer = bound.find(b => b.session.role === 'reviewer');
  if (view.stage === 'delivered') return reviewer
    ? { side: 'verifier', evidence: `session:${reviewer.session.agentId}/reviewer${reviewer.agent ? '' : '（unlinked）'}` }
    : { side: null, evidence: 'ledger:delivered（未登記審查 session；等待原負責人驗收）' };
  const executor = bound.find(b => b.session.agentId === view.assigneeAgentId && b.session.role !== 'reviewer') ?? bound.find(b => b.session.role === 'worker');
  if (executor) return { side: 'worker', evidence: `session:${executor.session.agentId}/${executor.session.role}${executor.agent ? '' : '（unlinked）'}` };
  const assignee = view.assigneeAgentId ? agents.find(a => a.id === view.assigneeAgentId && a.projectId === view.projectId) : undefined;
  if (assignee) return { side: 'worker', evidence: `unlinked:assignee ${assignee.id}/${assignee.role} 沒有 session 綁定` };
  return { side: null, evidence: view.assigneeAgentId ? `unlinked:${view.assigneeAgentId} 不在已選取的代理中` : 'unlinked:沒有可判定的執行角色' };
}
export function deriveWorkProgress(input: NativeWorkInputs): NativeWorkProgress {
  const { task, view, agents, inbox } = input;
  const parts: string[] = [];
  const note = (value: string | null | undefined): void => { if (value) parts.push(value); };
  const finish = (phase: WorkPhase, waitingFor: WorkWaitingFor): NativeWorkProgress =>
    ({ phase, waitingFor, waitEvidence: parts.length ? parts.join(' · ').slice(0, MAX_WAIT_EVIDENCE) : null });

  const bound: BoundSession[] = view.sessions.filter(s => !s.unboundAt).map(session => ({ session, agent: linkedAgent(view, agents, session) }));
  const busy = bound.filter(b => progressing(b.agent));
  const pending = pendingSide(view, bound, agents);

  if (task.status === 'done') { note('task:done（原生任務已完成）'); return finish('accepted', 'none'); }

  // An interruption is a native signal, but only while it is fresher than the
  // last recorded manual action; a session observed after it supersedes it.
  const ledgerAt = view.updatedAt ? Date.parse(view.updatedAt) : Number.NaN;
  const interruption = inbox.filter(e => INTERRUPTING_INBOX.includes(e.kind) && Number.isFinite(Date.parse(e.at)) &&
    (!Number.isFinite(ledgerAt) || Date.parse(e.at) > ledgerAt)).sort((a, b) => b.at.localeCompare(a.at))[0];
  let liveAt = Number.NEGATIVE_INFINITY;
  for (const b of busy) liveAt = Math.max(liveAt, Date.parse(b.agent?.heartbeatAt ?? b.agent?.observedAt ?? ''));
  if (interruption && !(busy.length && liveAt >= Date.parse(interruption.at))) {
    note(`inbox:${interruption.kind} @ ${interruption.at}`);
    note(interruption.summary.slice(0, 160));
    note(pending.evidence);
    return finish('interrupted', null);
  }
  if (busy.length) {
    // Only a live, non-stale session that is running (or using a tool) is work.
    const first = busy[0]!;
    note(`session:${first.session.agentId}/${first.session.role}:${first.agent?.state ?? 'unknown'}`);
    if (busy.length > 1) note(`同時有 ${busy.length} 個進行中的 session`);
    return finish('working', busy.some(b => b.agent?.state === 'executing_tool') ? 'tool' : 'none');
  }
  const waitingUser = bound.find(b => observedAs(b.agent, 'waiting_user'));
  if (waitingUser) { note(`session:${waitingUser.session.agentId}/${waitingUser.session.role}:waiting_user`); return finish('waiting', 'user_decision'); }
  if (task.status === 'failed') { note('task:failed（原生任務已失敗）'); return finish('failed', 'none'); }
  if (task.status === 'blocked') { note('task:blocked（原生任務已阻塞）'); return finish('blocked', 'dependency'); }
  const stopped = bound.find(b => b.agent?.state === 'stopped');
  if (stopped) { note(`session:${stopped.session.agentId}/${stopped.session.role}:stopped`); note(stopped.agent?.reason?.slice(0, 160)); return finish('interrupted', null); }
  if (view.deliveryStatus === 'failed') { note('delivery:failed'); note(view.waitingReason?.slice(0, 160)); return finish('failed', 'user_decision'); }
  if (view.stage === 'accepted') { note('ledger:accepted（原生任務狀態尚未更新）'); return finish('accepted', 'none'); }
  if (view.stage === 'delivered') { note(pending.evidence); note(deliveryNote(view)); return finish('delivered', pending.side); }
  if (view.stage === 'blocked') { note(pending.evidence); return finish('blocked', 'dependency'); }
  if (view.stage === 'ready') { note(pending.evidence); return finish('ready', null); }
  if (view.stage === 'uninitialized') { note(pending.evidence); return finish('uninitialized', null); }
  // assigned / executing / awaiting_delivery: the task rail is not asked to
  // imply that a child works — only the observed session decides that above.
  // The evidence names the semantic state only; source, staleness, heartbeat and
  // last progress stay separate process-liveness fields.
  note(pending.evidence); note(deliveryNote(view));
  for (const b of bound) note(`observation:${b.session.agentId}/${b.session.role}:${b.agent?.state ?? 'unlinked'}`);
  const waiting = view.stage !== 'assigned' || view.deliveryStatus === 'started' || view.deliveryStatus === 'settled';
  return finish(waiting ? 'waiting' : 'assigned', pending.side);
}
function deliveryNote(view: WorkView): string { return view.deliveryStatus ? `delivery:${view.deliveryStatus}` : 'delivery:沒有交辦回執'; }
function apply(view: WorkView, action: WorkAction, at: string, target: AgentBinding | null): void {
  switch (action.kind) {
    case 'attach_continuity': view.continuity = action.reference; break;
    case 'initialize': view.stage = 'ready'; view.nextStep = action.nextStep; break;
    case 'assign':
      view.stage = 'assigned'; view.assigneeAgentId = action.agentId; view.nextStep = action.nextStep;
      view.deliveryOperationId = action.send.operationId; view.deliveryStatus = 'unknown'; view.evidence = null; view.waitingReason = null; view.pendingInstruction = null; break;
    case 'intervene':
      view.pendingInstruction = { id: action.actionId, operationId: action.send.operationId, message: action.send.message, acknowledgedAt: null, evidence: null };
      view.deliveryOperationId = action.send.operationId; view.deliveryStatus = 'unknown';
      view.evidence = null; view.stage = 'assigned'; view.waitingReason = null; view.nextStep = '確認方向變更，依新指示交付並回報。'; break;
    case 'acknowledge':
      if (view.pendingInstruction) view.pendingInstruction = { ...view.pendingInstruction, acknowledgedAt: at, evidence: action.evidence }; break;
    case 'deliver': view.stage = 'delivered'; view.evidence = action.evidence; view.nextStep = action.nextStep; view.waitingReason = null; break;
    case 'accept': view.stage = 'accepted'; view.evidence = `${view.evidence}\n驗收：${action.evidence}`; view.nextStep = '本次交接已驗收；Edda 任務狀態依原有流程更新。'; break;
    case 'block': view.stage = 'blocked'; view.waitingReason = action.reason; view.nextStep = action.nextStep; break;
    case 'bind_session':
      if (!target) throw new ManagerError('LEDGER_INVALID', 'Session 綁定缺少原始身分。', 409);
      view.sessions.push({ id: action.actionId, agentId: target.id, sessionId: target.sessionId, selectionRevision: selectionRevision(target), transport: target.transport ?? 'pi', role: action.role, parentAgentId: action.parentAgentId, reviewedSha: action.reviewedSha, expectedEvent: action.expectedEvent, nextExpectedAt: action.nextExpectedAt, boundAt: at, unboundAt: null }); break;
    case 'unbind_session': view.sessions = view.sessions.map(s => s.id === action.bindingId ? { ...s, unboundAt: at } : s); break;
    case 'handoff_owner': view.ownerAgentId = action.ownerAgentId; break;
  }
  view.updatedAt = at; view.lastActionId = action.actionId;
}
export class WorkManager {
  private transportStoreId: string;
  private cache = new Map<string, { at: number; view: WorkView }>();
  private reads = new Map<string, Promise<ReadWork>>();
  private queue = new Map<string, Promise<unknown>>();
  private refreshing: Promise<void> | null = null;
  private closing = false;
  constructor(private manager: AgentManager, private ledger: WorkflowLedger = new EddaWorkflowLedger(), private locks = new WorkflowLocks()) {
    this.transportStoreId = manager.store.ensureSetting('workflow-transport-id', randomUUID());
  }
  private binding(id: string): WorkBinding {
    const binding = this.manager.config.works?.find((w) => w.id === id);
    if (!binding) throw new ManagerError('NOT_FOUND', '此工作未加入管理清單。', 404);
    return binding;
  }
  async list(): Promise<WorksView> {
    const bindings = this.manager.config.works ?? [];
    if (!this.refreshing && !this.closing) {
      let index = 0;
      const worker = async () => { while (index < bindings.length && !this.closing) {
        const binding = bindings[index++]!;
        const cached = this.cache.get(binding.id);
        if (cached && Date.now() - cached.at < 10000) continue;
        try { await this.read(binding); }
        catch (error) {
          const view = { ...(cached?.view ?? empty(binding)), error: error instanceof ManagerError ? error.message : '此工作的 Edda 紀錄暫時無法讀取。' };
          this.cache.set(binding.id, { at: Date.now(), view });
        }
      } };
      this.refreshing = Promise.all([worker(), worker()]).then(() => {}).finally(() => { this.refreshing = null; });
    }
    // Slow/offline projects cannot hold an HTTP response for a whole fleet.
    let timer: ReturnType<typeof setTimeout> | undefined;
    await Promise.race([this.refreshing, new Promise<void>((resolve) => { timer = setTimeout(resolve, 1500); })]);
    clearTimeout(timer);
    return { works: bindings.map((b) => this.cache.get(b.id)?.view ?? { ...empty(b), error: '正在取得此工作的 Edda 紀錄。' }), generatedAt: new Date().toISOString() };
  }
  async stop(): Promise<void> { this.closing = true; await Promise.allSettled([...this.queue.values(), ...this.reads.values(), ...(this.refreshing ? [this.refreshing] : [])]); }
  private async read(binding: WorkBinding): Promise<ReadWork> {
    const pending = this.reads.get(binding.id); if (pending) return pending;
    const promise = this.doRead(binding).finally(() => this.reads.delete(binding.id)); this.reads.set(binding.id, promise); return promise;
  }
  private async doRead(binding: WorkBinding): Promise<ReadWork> {
    const [task, notes] = await Promise.all([this.ledger.task(binding), this.ledger.notes(binding)]);
    const events: ReadWork['events'] = [];
    for (const note of notes) {
      if (!note.text.startsWith(PREFIX)) continue;
      try {
        const raw = object(JSON.parse(note.text.slice(PREFIX.length)) as unknown);
        if (raw.version !== 1 || raw.taskKey !== task.key) throw new Error('task/schema');
        const action = parseWorkAction(raw.action), fingerprint = hash(JSON.stringify(action));
        if (raw.fingerprint !== fingerprint || (raw.previous !== null && typeof raw.previous !== 'string')) throw new Error('chain');
        let target: AgentBinding | null = null;
        if ('send' in action || action.kind === 'bind_session' || action.kind === 'handoff_owner') {
          const targetInput = object(raw.target);
          target = parseConfig({ version: 1, projects: [{ id: binding.projectId, name: 'Work project' }], agents: [targetInput] }).agents[0]!;
          if (target.projectId !== binding.projectId || ('agentId' in action && target.id !== action.agentId)) throw new Error('target');
          if (action.kind === 'handoff_owner' && (target.id !== action.ownerAgentId || target.role !== 'manager')) throw new Error('owner');
          if (action.kind === 'bind_session' && action.role === 'manager' && target.role !== 'manager') throw new Error('manager role');
        } else if (raw.target !== null) throw new Error('unexpected target');
        const priorFailedOperation = raw.priorFailedOperation == null ? null : uuid(raw.priorFailedOperation);
        if (priorFailedOperation && action.kind !== 'assign') throw new Error('unexpected failure');
        events.push({ version: 1, taskKey: task.key, previous: raw.previous as string | null, action, fingerprint, target,
          transportStoreId: uuid(raw.transportStoreId), priorFailedOperation, id: note.id, at: note.at });
      } catch { throw new ManagerError('LEDGER_INVALID', '工作交接紀錄格式不一致，已停止套用，請檢查原始 Edda 紀錄。', 409); }
    }
    const ordered: ReadWork['events'] = []; let previous: string | null = null;
    while (ordered.length < events.length) {
      const next = events.filter((e) => e.previous === previous);
      if (next.length !== 1 || ordered.some((e) => e.id === next[0]?.id)) throw new ManagerError('LEDGER_FORK', '此工作出現衝突或不完整的交接分支，需要代管者裁定；未自行選取其中一份。', 409);
      ordered.push(next[0]!); previous = next[0]!.id;
    }
    if (new Set(ordered.map((e) => e.action.actionId)).size !== ordered.length) throw new ManagerError('LEDGER_FORK', '工作操作編號重複，需要檢查交接紀錄。', 409);
    const view = empty(binding);
    view.title = task.title; view.taskStatus = task.status; view.taskReceipt = task.receipt;
    for (const event of ordered) {
      if (event.priorFailedOperation) {
        if (event.priorFailedOperation !== view.deliveryOperationId) throw new ManagerError('LEDGER_INVALID', '交接失敗證據與前次訊息不一致。', 409);
        view.deliveryStatus = 'failed';
      }
      if (event.action.kind === 'intervene' && event.target?.id !== view.assigneeAgentId) throw new ManagerError('LEDGER_INVALID', '指示的執行者與工作不一致。', 409);
      try { this.validate(view, event.action); }
      catch { throw new ManagerError('LEDGER_INVALID', '交接紀錄含不合法的狀態轉換；已停止套用。', 409); }
      apply(view, event.action, event.at, event.target);
      view.history.push({ id: event.action.actionId, kind: event.action.kind, at: event.at,
        summary: event.action.kind === 'attach_continuity' ? `連結原生上下文 ${event.action.reference.capsuleId}` : event.action.kind === 'intervene' ? event.action.send.message : 'nextStep' in event.action ? event.action.nextStep : 'evidence' in event.action ? event.action.evidence : event.action.kind === 'bind_session' ? event.action.expectedEvent : '解除 session 綁定' });
    }
    view.revision = hash(JSON.stringify([binding, task.key, task.updatedAt, previous]));
    if (view.deliveryOperationId) {
      const event = ordered.findLast((e) => 'send' in e.action && e.action.send.operationId === view.deliveryOperationId);
      const op = event ? this.associatedOperation(event) : null;
      view.deliveryStatus = op?.status ?? 'unknown';
      if (view.stage === 'assigned') {
        if (op?.status === 'started') view.stage = 'executing';
        else if (op?.status === 'settled') view.stage = 'awaiting_delivery';
        else if (op?.status === 'failed') view.waitingReason = op.notice;
      }
    }
    // Derive the operator-facing phase last: it reads the native task rail,
    // delivery receipt, observed sessions and recorded owner-inbox events, so a
    // fresher native signal overrides a stale manual stage/waiting reason.
    const native = deriveWorkProgress({ task, view, agents: this.manager.overview().agents,
      inbox: this.manager.store.inboxEvents(view.id, binding.projectId, binding.taskId, 201) });
    view.phase = native.phase; view.waitingFor = native.waitingFor; view.waitEvidence = native.waitEvidence;
    this.cache.set(binding.id, { at: Date.now(), view }); return { task, events: ordered, view };
  }
  async continuationSnapshot(id: string): Promise<{ taskKey: string; view: WorkView; actions: WorkAction[] }> {
    const state = await this.read(this.binding(id)); return { taskKey: state.task.key, view: state.view, actions: state.events.map(e => e.action) };
  }
  async act(id: string, input: WorkAction): Promise<WorkView> {
    if (this.closing) throw new ManagerError('STOPPING', '管理服務正在關閉，請稍後查詢原操作。', 503);
    const action = parseWorkAction(input), binding = this.binding(id);
    const prior = this.queue.get(id) ?? Promise.resolve();
    const promise = prior.catch(() => {}).then(async () => {
      const task = await this.ledger.task(binding);
      return this.locks.run(task.key, () => 'send' in action ? this.locks.run(`operation:${action.send.operationId}`, () => this.perform(binding, action)) : this.perform(binding, action));
    });
    this.queue.set(id, promise);
    try { return await promise; } finally { if (this.queue.get(id) === promise) this.queue.delete(id); }
  }
  private async perform(binding: WorkBinding, action: WorkAction): Promise<WorkView> {
    // Await and discard any pre-lock read before obtaining the mutation snapshot.
    await this.reads.get(binding.id)?.catch(() => {});
    const current = await this.doRead(binding), fingerprint = hash(JSON.stringify(action));
    const duplicate = current.events.find((e) => e.action.actionId === action.actionId);
    if (duplicate) {
      if (duplicate.fingerprint !== fingerprint) throw new ManagerError('ACTION_CONFLICT', '這個交接編號已有不同內容。', 409);
      if ('send' in action && current.events.at(-1)?.id === duplicate.id) {
        const existing = this.associatedOperation(duplicate);
        if (existing) await this.manager.operation(existing.id).catch(() => {});
        else if (duplicate.transportStoreId === this.transportStoreId && duplicate.target) {
          const selected = this.manager.binding(duplicate.target.id);
          if (JSON.stringify(selected) !== JSON.stringify(duplicate.target)) throw new ManagerError('RECOVERY_TARGET_CHANGED', '原交辦的代理綁定已變更，保留原指示，未傳送給新代理。', 409);
          // Explicit same-ID recovery only: this very store has no pre-effect
          // intent, so send() could not have reached the adapter before crash.
          await this.manager.send(selected.id, action.send);
        }
      }
      return { ...(await this.doRead(binding)).view, confirmedActionId: action.actionId };
    }
    if (current.view.revision !== action.revision) throw new ManagerError('STALE_WORK', '工作已被更新，請查看最新狀態後再操作。', 409);
    this.validate(current.view, action);
    let target: AgentBinding | null = null;
    if (action.kind === 'bind_session' || action.kind === 'handoff_owner') {
      target = this.manager.binding(action.kind === 'bind_session' ? action.agentId : action.ownerAgentId);
      if (target.projectId !== binding.projectId) throw new ManagerError('WRONG_PROJECT', 'Session 必須屬於同一專案。', 409);
      if ((action.kind === 'handoff_owner' || action.role === 'manager') && target.role !== 'manager') throw new ManagerError('INVALID_OWNER', '負責人必須是已選取的專案管理者。', 409);
      if (action.kind === 'bind_session' && action.parentAgentId) {
        const parent = this.manager.binding(action.parentAgentId);
        if (parent.projectId !== binding.projectId || parent.id === target.id) throw new ManagerError('INVALID_PARENT', '上層代理必須是同專案的其他代理。', 409);
      }
    }
    if (action.kind === 'assign' || action.kind === 'intervene') {
      const agentId = action.kind === 'assign' ? action.agentId : current.view.assigneeAgentId!;
      target = this.manager.binding(agentId);
      if (target.projectId !== binding.projectId) throw new ManagerError('WRONG_PROJECT', '只能交辦給同專案已選取的代理。', 409);
      if (this.manager.store.operation(action.send.operationId) || current.events.some((e) => 'send' in e.action && e.action.send.operationId === action.send.operationId)) throw new ManagerError('OPERATION_CONFLICT', '這則訊息編號已被其他交辦使用。', 409);
      await this.manager.validateSend(agentId, action.send);
    }
    const event: WorkEvent = { version: 1, taskKey: current.task.key, previous: current.events.at(-1)?.id ?? null, action, fingerprint, target, transportStoreId: this.transportStoreId,
      priorFailedOperation: action.kind === 'assign' && current.view.deliveryStatus === 'failed' ? current.view.deliveryOperationId : null };
    const encoded = PREFIX + JSON.stringify(event);
    // Keep the Windows execFile quoted command line bounded even after quote /
    // backslash expansion. This also bounds each event on every platform.
    if (encoded.length > 12000) throw new ManagerError('WORK_EVENT_TOO_LARGE', '交接內容與目標資料合計過長，請縮短訊息或證據。', 413);
    await this.ledger.append(binding, encoded);
    // Re-read the ledger before the effect: detect an external writer that
    // ignored the mutex. An uncertain append/send is never retried automatically.
    const written = await this.doRead(binding);
    if (written.events.at(-1)?.action.actionId !== action.actionId) throw new ManagerError('LEDGER_CONFLICT', '交接紀錄已被其他管理者更新，尚未傳送訊息。', 409);
    if (target && (action.kind === 'assign' || action.kind === 'intervene')) {
      try { await this.manager.send(target.id, action.send); }
      catch (error) {
        // Durable ledger intent remains authoritative even if preflight changes
        // between recording and send. Absence of transport evidence is unknown.
        const view = (await this.doRead(binding)).view;
        return { ...view, error: error instanceof ManagerError ? error.message : '傳送結果尚待確認；請查詢原交接編號。', confirmedActionId: action.actionId };
      }
    }
    return { ...(await this.doRead(binding)).view, confirmedActionId: action.actionId };
  }
  private validate(view: WorkView, action: WorkAction): void {
    const fail = (message: string): never => { throw new ManagerError('INVALID_TRANSITION', message, 409); };
    if (action.kind === 'attach_continuity') return;
    if (action.kind === 'initialize') { if (view.stage !== 'uninitialized') fail('工作已開始追蹤。'); return; }
    if (view.stage === 'uninitialized') fail('請先設定此工作的下一步。');
    if (action.kind === 'bind_session') {
      if (view.sessions.filter(s => !s.unboundAt).length >= 32 || view.sessions.length >= 128) fail('Session 綁定數量已達上限。');
      if (view.sessions.some(s => !s.unboundAt && s.agentId === action.agentId)) fail('請先解除此代理的舊 session 綁定。');
    } else if (action.kind === 'unbind_session') {
      if (!view.sessions.some(s => s.id === action.bindingId && !s.unboundAt)) fail('此 session 綁定不存在或已解除。');
    }
    if (action.kind === 'assign') {
      if (view.pendingInstruction && !view.pendingInstruction.acknowledgedAt) fail('先記錄目前指示的接手證據，再轉交工作。');
      if (view.deliveryOperationId && !['delivered', 'accepted'].includes(view.stage) && view.deliveryStatus !== 'failed') fail('上一份交辦尚未交付，請先確認結果，避免重複派工。');
    } else if (action.kind === 'intervene') {
      if (!view.assigneeAgentId) fail('先指定執行者才能介入。');
      if (view.pendingInstruction && !view.pendingInstruction.acknowledgedAt) fail('上一則指示尚未確認接手，請先追蹤原指示。');
    } else if (action.kind === 'acknowledge') {
      if (!view.pendingInstruction || view.pendingInstruction.id !== action.instructionId || view.pendingInstruction.acknowledgedAt) fail('此指示不存在或已確認接手。');
    } else if (action.kind === 'deliver') {
      if (!view.assigneeAgentId || view.stage === 'accepted') fail('此工作目前沒有可記錄的交付。');
      if (view.pendingInstruction && !view.pendingInstruction.acknowledgedAt) fail('指示變更尚未確認接手，請先記錄證據。');
    } else if (action.kind === 'accept') {
      if (view.stage !== 'delivered' || !view.evidence) fail('必須先記錄交付證據，才能記錄驗收。');
      if (view.pendingInstruction && !view.pendingInstruction.acknowledgedAt) fail('仍有尚未確認的指示。');
    }
  }
  private associatedOperation(event: WorkEvent) {
    if (!event.target || !('send' in event.action) || event.transportStoreId !== this.transportStoreId) return null;
    try {
      const operation = this.manager.store.existing(event.target.id, event.action.send);
      if (operation && JSON.stringify(this.manager.store.target(operation.id)) !== JSON.stringify(event.target)) throw new Error('target mismatch');
      return operation;
    } catch { throw new ManagerError('OPERATION_CONFLICT', '交辦與訊息回執的身分不一致，未套用其他工作的進度。', 409); }
  }
}
