import { ManagerError, type AgentView } from './contracts.js';
import { hash, object, text, uuid } from './config.js';
import type { ManagerStore } from './store.js';
import type { WorkView, WorksView } from './workflow-contracts.js';
import type { OwnerContext, OwnerInboxAck, OwnerInboxEvent, OwnerInboxKind, OwnerInboxView } from './owner-inbox-contracts.js';

export function parseOwnerInboxAck(input: unknown): OwnerInboxAck {
  const r = object(input), eventId = text(r.eventId, 64);
  if (!/^[a-f0-9]{64}$/.test(eventId)) throw new ManagerError('INVALID_EVENT', '通知編號無效。');
  return { eventId, actionId: uuid(r.actionId), evidence: text(r.evidence, 4000) };
}

/** Observation-only: deliberately has no adapter, send or process authority. */
export class OwnerInbox {
  private works: WorkView[] = [];
  private agents: AgentView[] = [];
  constructor(private store: ManagerStore) {}
  refresh(agents: AgentView[], works: WorksView, now = Date.now()): void {
    this.agents = agents; this.works = works.works;
    for (const work of works.works) {
      // An unavailable ledger snapshot must not fabricate bindings or deadlines.
      if (work.error) continue;
      for (const binding of work.sessions.filter(s => !s.unboundAt)) {
        const stateKey = hash(JSON.stringify([work.id, binding.id]));
        const state = this.store.bindingObservation(stateKey), pending: OwnerInboxEvent[] = [];
        const agent = agents.find(a => a.id === binding.agentId && a.projectId === work.projectId);
        const evidence = agent?.sessionEvidence;
        const identityMatches = agent?.selectionRevision === binding.selectionRevision && evidence?.sessionId === binding.sessionId && agent.transport === binding.transport;
        const unavailable = !identityMatches || agent?.source === 'unavailable' || evidence?.evidenceSource === 'unavailable';
        const add = (kind: OwnerInboxKind, nativeEventId: string, at: string, summary: string, category: string | null = null, httpStatus: number | null = null) => {
          pending.push({ id: hash(JSON.stringify([work.id, binding.id, kind, nativeEventId])), workId: work.id, projectId: work.projectId, taskId: work.taskId, bindingId: binding.id,
            agentId: binding.agentId, sessionId: binding.sessionId, nativeEventId, kind, at, summary, category, httpStatus,
            deliveryRecorded: ['delivered', 'accepted'].includes(work.stage), acknowledgedAt: null, acknowledgementId: null, evidence: null, ownerAgentId: work.ownerAgentId });
        };
        if (unavailable && !state.unavailable) {
          state.transition++;
          add('unavailable', `availability:${state.transition}`, new Date(now).toISOString(), 'Session 證據不可用或身分已改變；未自動重新派工。');
        }
        state.unavailable = unavailable;
        if (identityMatches && evidence && evidence.evidenceSource !== 'unavailable') {
          for (const event of evidence.events.slice(-100)) {
            if (!event.id || event.id.length > 512 || !Number.isFinite(Date.parse(event.at)) || Date.parse(event.at) < Date.parse(binding.boundAt)) continue;
            if (binding.role !== 'manager' && (!state.latestChildEventAt || Date.parse(event.at) > Date.parse(state.latestChildEventAt))) state.latestChildEventAt = event.at;
            if (event.kind === 'reply_ended') add(event.kind, event.id, event.at, ['delivered', 'accepted'].includes(work.stage) ? '代理本輪回覆結束；事件記錄時已有獨立工作交付紀錄。' : '代理本輪回覆結束；事件記錄時尚無工作交付證據。');
            else if (event.kind === 'interrupted') add(event.kind, event.id, event.at, '代理回合被中斷；工作結果需要確認。');
            else if (event.kind === 'provider_error') {
              const status = typeof event.httpStatus === 'number' && Number.isInteger(event.httpStatus) && event.httpStatus >= 400 && event.httpStatus <= 599 ? event.httpStatus : null;
              const category = status === 402 || event.category === 'credit_or_quota' ? 'quota' : status === 401 || status === 403 ? 'authentication' : status === 429 ? 'rate_limit' : status !== null && status >= 500 ? 'provider_unavailable' : event.category === 'timeout' ? 'network' : ['quota', 'authentication', 'rate_limit', 'provider_unavailable', 'network', 'unknown'].includes(event.category ?? '') ? event.category! : 'unknown';
              add(event.kind, event.id, event.at, '模型供應商回報錯誤；未自動重送。', category, status);
            }
          }
        }
        // The explicit expectation remains unresolved until the operator changes
        // or retires its binding. A random tool event cannot satisfy free text.
        if (binding.nextExpectedAt && Date.parse(binding.nextExpectedAt) <= now && !['delivered', 'accepted'].includes(work.stage)) {
          add('overdue', `deadline:${binding.nextExpectedAt}`, binding.nextExpectedAt, '已超過明確預期回報時間；可能停滯，需確認，未終止或重派。');
        }
        this.store.recordInboxObservation(stateKey, state, pending);
      }
    }
  }
  list(ownerAgentId?: string): OwnerInboxView {
    const selected = this.works.filter(w => ownerAgentId === undefined || w.ownerAgentId === ownerAgentId);
    const events = selected.flatMap(work => this.store.inboxEvents(work.id, work.projectId, work.taskId, 201).filter(event => work.sessions.some(s => s.id === event.bindingId)).map(event => ({ ...event, ownerAgentId: work.ownerAgentId })));
    events.sort((a, b) => Number(Boolean(a.acknowledgedAt)) - Number(Boolean(b.acknowledgedAt)) || b.at.localeCompare(a.at) || a.id.localeCompare(b.id));
    return { events: events.slice(0, 200), generatedAt: new Date().toISOString(), truncated: events.length > 200 };
  }
  acknowledge(input: OwnerInboxAck): OwnerInboxEvent {
    const request = parseOwnerInboxAck(input), event = this.store.inboxEvent(request.eventId);
    const work = event && this.works.find(w => w.id === event.workId && w.projectId === event.projectId && w.taskId === event.taskId && w.sessions.some(s => s.id === event.bindingId));
    if (!work) throw new ManagerError('NOT_FOUND', '此通知不屬於已選取工作。', 404);
    if (work.error) throw new ManagerError('WORK_UNAVAILABLE', '工作紀錄不可用，請先確認目前負責人。', 409);
    return { ...this.store.acknowledgeInbox(request), ownerAgentId: work.ownerAgentId };
  }
  context(ownerAgentId: string): OwnerContext {
    const selected = this.works.filter(w => w.ownerAgentId === ownerAgentId), inbox = this.list(ownerAgentId);
    const owner = this.agents.find(a => a.id === ownerAgentId);
    const works = selected.slice(0, 20).map(work => {
      const latest = work.sessions.filter(s => s.role !== 'manager' && !s.unboundAt).map(s => this.store.bindingObservation(hash(JSON.stringify([work.id, s.id]))).latestChildEventAt).filter((s): s is string => s !== null).sort((a, b) => Date.parse(a) - Date.parse(b)).at(-1) ?? null;
      return { work: { ...work, title: work.title.slice(0, 300), nextStep: work.nextStep.slice(0, 600), evidence: work.evidence?.slice(0, 600) ?? null,
        taskReceipt: work.taskReceipt?.slice(0, 600) ?? null,
        pendingInstruction: work.pendingInstruction ? { ...work.pendingInstruction, message: work.pendingInstruction.message.slice(0, 1000), evidence: work.pendingInstruction.evidence?.slice(0, 300) ?? null } : null,
        sessions: work.sessions.filter(s => !s.unboundAt).slice(-8).map(s => ({ ...s, expectedEvent: s.expectedEvent.slice(0, 300) })),
        history: work.history.slice(-3).map(e => ({ ...e, summary: e.summary.slice(0, 300) })) }, latestChildEventAt: latest,
        detailPath: `/api/works/${encodeURIComponent(work.id)}`,
        summaryStale: latest !== null && (!owner?.summaryUpdatedAt || Date.parse(latest) > Date.parse(owner.summaryUpdatedAt)),
        alerts: inbox.events.filter(e => e.workId === work.id && !e.acknowledgedAt).slice(0, 10) };
    });
    const result: OwnerContext = { ownerAgentId, works: [], generatedAt: new Date().toISOString(), truncated: selected.length > 20 || inbox.truncated };
    for (const packet of works) {
      if (Buffer.byteLength(JSON.stringify({ ...result, works: [...result.works, packet] })) > 65536) { result.truncated = true; break; }
      result.works.push(packet);
      const original = selected.find(w => w.id === packet.work.id)!;
      if (JSON.stringify(original) !== JSON.stringify(packet.work) || inbox.events.filter(e => e.workId === original.id && !e.acknowledgedAt).length > 10) result.truncated = true;
    }
    return result;
  }
}
