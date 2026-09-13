import type { AgentView, ConversationView, Overview, SendRequest } from '../contracts.js';
import type { WorkAction, WorkStage, WorkView, WorksView } from '../workflow-contracts.js';
import type { OwnerContext, OwnerInboxAck, OwnerInboxView, OwnerInboxEvent } from '../owner-inbox-contracts.js';

type Kind = WorkAction['kind'];
interface Draft { kind: Kind; agentId: string; message: string; nextStep: string; evidence: string; reason: string; pending: WorkAction | null; rejected?: boolean; basisRevision?: string;
  role?: 'manager' | 'worker' | 'reviewer'; reviewedSha?: string; nextExpectedAt?: string; bindingId?: string; pendingAck?: OwnerInboxAck | null }
interface Host { api<T>(path: string, body?: unknown): Promise<T>; openAgent(id: string): void }
const key = 'edda-manager-work-drafts-v1';
const labels: Record<WorkStage, string> = { uninitialized: '尚未安排下一步', ready: '待交接', assigned: '已交辦・等待接手', executing: '執行中', awaiting_delivery: '回覆結束・待交付證據', delivered: '已交付・待驗收', accepted: '已記錄驗收', blocked: '有阻塞・需要處理' };
const actions: Record<Kind, string> = { initialize: '安排下一步', assign: '交辦給代理', intervene: '變更工作指示', acknowledge: '記錄指示已確認', deliver: '記錄交付', accept: '記錄驗收與收尾', block: '回報阻塞', bind_session: '登記執行 session', unbind_session: '結束 session 追蹤', handoff_owner: '移交收尾負責人' };
function el<K extends keyof HTMLElementTagNameMap>(tag: K, text = '', cls = ''): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag); node.textContent = text; node.className = cls; return node;
}
function field(label: string, input: HTMLElement): HTMLElement { const wrap = el('label', label, 'work-field'); wrap.append(input); return wrap; }
function freshDraft(): Draft { return { kind: 'initialize', agentId: '', message: '', nextStep: '', evidence: '', reason: '', pending: null }; }
function err(e: unknown): string { return e instanceof Error ? e.message : '無法取得回覆；請保留原始請求。'; }

export class WorkBoard {
  private overview: Overview | null = null;
  private projectId: string | null = null;
  private works: WorkView[] = [];
  private inbox: OwnerInboxView | null = null;
  private ownerContext: OwnerContext | null = null;
  private selected = '';
  private drafts: Record<string, Draft> = {};
  private storageOK = true;
  private busy = false;
  private loading = false;
  private signature = '';
  private cards = el('div', '', 'work-cards');
  private details = el('div', '', 'work-details');
  private form = el('form', '', 'work-form');
  private feedback = el('p', '', 'notice');
  private title = el('h3', '工作交接');
  constructor(private root: HTMLElement, private host: Host) {
    try {
      const raw = localStorage.getItem(key);
      if (raw) {
        const value: unknown = JSON.parse(raw);
        if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error();
        for (const [id, draft] of Object.entries(value)) {
          if (!draft || typeof draft !== 'object' || !Object.hasOwn(actions, draft.kind) ||
            !['agentId', 'message', 'nextStep', 'evidence', 'reason'].every(k => typeof draft[k] === 'string') ||
            (draft.pending !== null && (typeof draft.pending !== 'object' || typeof draft.pending.actionId !== 'string' || typeof draft.pending.revision !== 'string' || !Object.hasOwn(actions, draft.pending.kind)))) throw new Error();
          this.drafts[id] = draft as Draft;
        }
      }
    } catch { this.storageOK = false; this.feedback.textContent = '工作草稿無法讀取；原始資料已保留，請先恢復瀏覽器儲存。'; }
    root.append(this.title, el('p', '交接由工作負責人持續追蹤；代理停止不等於工作完成。', 'muted'), this.cards, this.details, this.form, this.feedback);
    this.feedback.setAttribute('role', 'status');
    this.form.addEventListener('submit', event => { event.preventDefault(); void this.submit(); });
    window.addEventListener('storage', event => { if (event.key === key) { this.storageOK = false; this.feedback.textContent = '另一分頁更新了工作操作，請重新載入以讀取原始請求。'; this.renderForm(); } });
  }
  update(overview: Overview, projectId: string | null): void {
    this.overview = overview;
    if (this.projectId !== projectId) { this.projectId = projectId; this.selected = ''; this.signature = ''; }
    this.render();
  }
  async refresh(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    try {
      const [works, inbox] = await Promise.all([this.host.api<WorksView>('/api/works'), this.host.api<OwnerInboxView>('/api/owner-inbox')]);
      this.works = works.works; this.inbox = inbox; this.render();
      const owner = this.current()?.ownerAgentId;
      if (owner) this.ownerContext = await this.host.api<OwnerContext>(`/api/owners/${encodeURIComponent(owner)}/context`);
      this.render();
    }
    catch (error) { this.feedback.textContent = `${err(error)} 既有工作畫面為上次觀測。`; }
    finally { this.loading = false; }
  }
  private current(): WorkView | undefined { return this.works.find(work => work.id === this.selected); }
  private draft(): Draft { return this.drafts[this.selected] ??= freshDraft(); }
  private save(): boolean {
    if (!this.storageOK) return false;
    try { localStorage.setItem(key, JSON.stringify(this.drafts)); return true; }
    catch { this.storageOK = false; this.feedback.textContent = '工作請求無法保存，尚未傳送。'; return false; }
  }
  private name(id: string | null): string { return this.overview?.agents.find(a => a.id === id)?.name ?? id ?? '尚未指定'; }
  private render(): void {
    const visible = this.works.filter(work => !this.projectId || work.projectId === this.projectId);
    if (!visible.some(work => work.id === this.selected)) { this.selected = visible[0]?.id ?? ''; this.signature = ''; }
    const signature = JSON.stringify([visible, this.selected, this.inbox?.events, this.ownerContext?.works.map(w => [w.work.id, w.summaryStale]), this.overview?.agents.map(a => [a.id, a.name, a.instanceId, a.state, a.source])]);
    if (signature === this.signature) return;
    this.signature = signature;
    this.cards.replaceChildren();
    if (!visible.length) this.cards.append(el('p', '此專案尚未選入工作。先在本機設定加入既有 Edda 任務，代理對話仍可使用。', 'empty'));
    for (const work of visible) {
      const button = el('button', '', 'work-card'); button.type = 'button'; button.setAttribute('aria-pressed', String(work.id === this.selected));
      button.append(el('strong', `#${work.taskId} ${work.title}`), el('span', work.error ?? labels[work.stage], work.error ? 'danger' : 'work-stage'), el('span', `收尾：${this.name(work.ownerAgentId)} · 執行者：${this.name(work.assigneeAgentId)}`, 'muted'));
      button.addEventListener('click', () => { this.selected = work.id; this.signature = ''; this.render(); this.renderForm(); }); this.cards.append(button);
    }
    const work = this.current(); this.details.replaceChildren(); this.form.hidden = !work;
    if (!work) return;
    this.details.append(el('p', `下一步：${work.nextStep || '尚未安排；由收尾負責人處理。'}`, 'work-next'));
    const nextOwner = ['assigned', 'executing', 'awaiting_delivery'].includes(work.stage) ? work.assigneeAgentId : work.ownerAgentId;
    this.details.append(el('p', work.stage === 'accepted' ? `收尾紀錄由 ${this.name(work.ownerAgentId)} 負責。` : `目前由 ${this.name(nextOwner)} 接續處理。`, 'muted'));
    this.details.append(el('p', `Edda 任務狀態：${work.taskStatus} · 工作交接：${labels[work.stage]}`, 'muted'));
    if (work.waitingReason) this.details.append(el('p', work.waitingReason, 'notice'));
    if (work.error) this.details.append(el('p', work.error, 'notice'));
    if (work.evidence) this.details.append(el('p', `最近證據：${work.evidence}`, 'public-text'));
    if (work.pendingInstruction && !work.pendingInstruction.acknowledgedAt) this.details.append(el('p', `方向變更等待確認：${work.pendingInstruction.message}`, 'notice public-text'));
    if (work.deliveryStatus) this.details.append(el('p', `傳送回執：${work.deliveryStatus} · ${work.deliveryOperationId}`, 'muted identity'));
    const buttons = el('div', '', 'work-links');
    for (const [label, id] of [['與負責人對話', work.ownerAgentId], ['與接手代理對話', work.assigneeAgentId]] as const) {
      if (!id || !this.overview?.agents.some(a => a.id === id)) continue;
      const button = el('button', label); button.type = 'button'; button.addEventListener('click', () => this.host.openAgent(id)); buttons.append(button);
    }
    this.details.append(buttons);
    const history = el('details'); history.append(el('summary', '交接紀錄與任務收據'));
    if (work.taskReceipt) history.append(el('p', `Edda 任務收據：${work.taskReceipt}`, 'public-text'));
    for (const event of work.history) history.append(el('p', `${event.at} · ${event.summary}`, 'public-text'));
    this.details.append(history);
    const bound = el('section', '', 'bound-sessions'); bound.append(el('h4', '執行 session'));
    for (const session of work.sessions ?? []) {
      const source = this.overview?.agents.find(a => a.id === session.agentId);
      const row = el('article', '', 'session-row');
      const role = { manager: '管理者', worker: '工作者', reviewer: '審查者' }[session.role];
      row.append(el('strong', `${this.name(session.agentId)} · ${role}`), el('p', `${session.transport} / ${session.sessionId}`, 'identity'));
      row.append(el('p', session.unboundAt ? '已結束追蹤（保留歷史）' : source?.selectionRevision !== session.selectionRevision ? '來源綁定已變更，原 session 結果待確認' : source?.source === 'live' ? `即時觀測：${source.state}` : '僅有記錄或來源未知；不能判定程序已停止', 'muted'));
      row.append(el('p', `預期：${session.expectedEvent}${session.nextExpectedAt ? ` · ${new Date(session.nextExpectedAt).toLocaleString('zh-TW')}` : ' · 未設定期限'}`));
      if (session.reviewedSha) row.append(el('p', `審查版本：${session.reviewedSha}`, 'identity'));
      bound.append(row);
    }
    if (!work.sessions?.length) bound.append(el('p', '尚未登記執行 session；不代表無人在工作。', 'muted'));
    this.details.append(bound);
    if (this.ownerContext?.works.find(w => w.work.id === work.id)?.summaryStale) this.details.append(el('p', '管理摘要已過期：子代理有更新的事件，請先查看下方收件匣。', 'notice'));
    const inbox = el('section', '', 'owner-inbox'); inbox.append(el('h4', '負責人事件收件匣'));
    for (const event of this.inbox?.events.filter(e => e.workId === work.id) ?? []) {
      const row = el('article', '', 'session-row'); row.append(el('strong', event.summary), el('p', `${this.name(event.agentId)} · ${event.at}`, 'muted'));
      if (event.category) {
        const categories: Record<string, string> = { quota: '額度或配額限制', authentication: '認證失敗', rate_limit: '請求頻率限制', provider_unavailable: '供應商暫時不可用', network: '連線或逾時', unknown: '原因尚未分類' };
        row.append(el('p', `${categories[event.category] ?? '原因尚未分類'}${event.httpStatus ? ` · HTTP ${event.httpStatus}` : ''}`, 'notice'));
      }
      if (event.acknowledgedAt) row.append(el('p', `已讀：${event.acknowledgedAt}`, 'muted'));
      else { const ack = el('button', '標記已讀'); ack.type = 'button'; ack.disabled = this.busy; ack.addEventListener('click', () => { void this.acknowledgeEvent(event.id); }); row.append(ack); }
      inbox.append(row);
    }
    if (this.draft().pendingAck) {
      const retry = el('button', '恢復原事件確認'); retry.type = 'button'; retry.disabled = this.busy;
      retry.addEventListener('click', () => { void this.acknowledgeEvent(this.draft().pendingAck!.eventId); }); inbox.append(retry);
    }
    this.details.append(inbox);
    // Polls update evidence without replacing a form the operator is editing.
    if (this.form.dataset.workId !== work.id) this.renderForm();
    else if (!this.draft().pending) for (const button of this.form.querySelectorAll<HTMLButtonElement>('button[type="submit"]')) button.disabled = this.busy || !!work.error || !this.storageOK;
  }
  private renderForm(): void {
    const work = this.current(); if (!work) return;
    const draft = this.draft(); this.form.dataset.workId = work.id; this.form.replaceChildren();
    if (!this.storageOK) { this.form.append(el('p', '請恢復瀏覽器儲存後再操作。', 'notice')); return; }
    if (draft.pending) {
      this.form.append(el('p', '原始操作已保存；查詢與恢復都使用同一個編號。', 'notice'), el('p', draft.pending.actionId, 'identity'));
      const detail = el('details'); detail.append(el('summary', '檢視原始操作'), el('pre', JSON.stringify(draft.pending, null, 2), 'public-text')); this.form.append(detail);
      const retry = el('button', '查詢／恢復原操作', 'primary'); retry.type = 'submit'; retry.disabled = this.busy; this.form.append(retry);
      if (draft.rejected) {
        const release = el('button', '操作未被接受，保留草稿重新編輯'); release.type = 'button'; release.disabled = this.busy;
        release.addEventListener('click', () => { draft.pending = null; draft.rejected = false; draft.basisRevision = this.current()?.revision ?? work.revision; this.save(); this.renderForm(); }); this.form.append(release);
      }
      return;
    }
    if (draft.kind === 'initialize' && work.stage !== 'uninitialized') draft.kind = 'assign';
    const kind = el('select'); kind.setAttribute('aria-label', '工作操作');
    for (const [value, label] of Object.entries(actions)) { const option = el('option', label); option.value = value; kind.append(option); }
    kind.value = draft.kind; kind.addEventListener('change', () => { draft.kind = kind.value as Kind; draft.basisRevision ??= this.current()?.revision ?? work.revision; this.save(); this.renderForm(); });
    this.form.append(field('工作操作', kind));
    const k = draft.kind;
    if (['assign', 'bind_session', 'handoff_owner'].includes(k)) {
      const agent = el('select'); agent.setAttribute('aria-label', '接手代理');
      for (const a of this.overview?.agents.filter(a => a.projectId === work.projectId && (k !== 'handoff_owner' || a.role === 'manager')) ?? []) { const o = el('option', a.name); o.value = a.id; agent.append(o); }
      if (draft.agentId) agent.value = draft.agentId;
      if (!agent.value) agent.selectedIndex = 0;
      draft.agentId = agent.value; agent.addEventListener('change', () => { draft.agentId = agent.value; draft.basisRevision ??= this.current()?.revision ?? work.revision; this.save(); }); this.form.append(field('接手代理', agent));
    }
    const add = (name: 'message' | 'nextStep' | 'evidence' | 'reason', label: string, max: number) => {
      const input = el('textarea'); input.rows = name === 'message' ? 3 : 2; input.maxLength = max; input.required = true; input.value = draft[name]; input.setAttribute('aria-label', label);
      input.addEventListener('input', () => { draft[name] = input.value; draft.basisRevision ??= this.current()?.revision ?? work.revision; this.save(); }); this.form.append(field(label, input));
    };
    if (['initialize', 'assign', 'deliver', 'block'].includes(k)) add('nextStep', '下一步與完成條件', 2000);
    if (['assign', 'intervene'].includes(k)) add('message', k === 'intervene' ? '方向變更內容（傳給目前接手代理）' : '交辦訊息', 4000);
    if (['acknowledge', 'deliver', 'accept'].includes(k)) add('evidence', '證據或結果（摘要、收據位置）', 4000);
    if (k === 'block') add('reason', '等待原因／需要誰回答什麼', 2000);
    if (k === 'handoff_owner') add('evidence', '移交摘要與證據', 4000);
    if (k === 'bind_session') {
      const role = el('select'); role.setAttribute('aria-label', '執行角色');
      for (const [id, label] of [['worker', '工作者'], ['reviewer', '審查者'], ['manager', '管理者']]) { const o = el('option', label); o.value = id!; role.append(o); }
      role.value = draft.role ?? 'worker'; role.addEventListener('change', () => { draft.role = role.value as NonNullable<Draft['role']>; this.save(); }); this.form.append(field('執行角色', role));
      add('nextStep', '預期下一個回報事件', 1000);
      const sha = el('input'); sha.placeholder = '審查者必填完整 40 字元 SHA'; sha.value = draft.reviewedSha ?? ''; sha.maxLength = 40; sha.setAttribute('aria-label', '審查版本 SHA');
      sha.addEventListener('input', () => { draft.reviewedSha = sha.value; this.save(); }); this.form.append(field('審查版本 SHA', sha));
      const deadline = el('input'); deadline.type = 'datetime-local'; deadline.value = draft.nextExpectedAt ?? ''; deadline.setAttribute('aria-label', '預期回報時間');
      deadline.addEventListener('input', () => { draft.nextExpectedAt = deadline.value; this.save(); }); this.form.append(field('預期回報時間（可留空）', deadline));
      this.form.append(el('p', '登記只建立關聯，不會啟動或重複派工。逾期只提示疑似停住。', 'muted'));
    }
    if (k === 'unbind_session') {
      const session = el('select'); session.setAttribute('aria-label', '要結束追蹤的 session');
      for (const s of work.sessions.filter(s => !s.unboundAt)) { const o = el('option', `${this.name(s.agentId)} / ${s.sessionId}`); o.value = s.id; session.append(o); }
      if (draft.bindingId) session.value = draft.bindingId;
      draft.bindingId = session.value; session.addEventListener('change', () => { draft.bindingId = session.value; this.save(); }); this.form.append(field('要結束追蹤的 session', session));
    }
    if (k === 'acknowledge') this.form.append(el('p', '請確認代理已明確回覆接受這次方向變更，再記錄證據；送達不代表理解。', 'muted'));
    if (k === 'accept') this.form.append(el('p', '由收尾負責人確認既有專案驗收条件後記錄。此操作不執行合併，也不改寫 Edda 任務收據。', 'muted'));
    const send = el('button', actions[k], 'primary'); send.type = 'submit'; send.disabled = this.busy || !!work.error; this.form.append(send);
  }
  private async sendRequest(agent: AgentView, message: string, priority: boolean): Promise<SendRequest> {
    const view = await this.host.api<ConversationView>(`/api/agents/${encodeURIComponent(agent.id)}/conversation`);
    if (!view.instanceId || view.instanceId !== agent.instanceId || view.selectionRevision !== agent.selectionRevision || view.source !== 'live') throw new Error('代理實例已改變或離線，請更新狀態後再交辦。');
    return { operationId: crypto.randomUUID(), instanceId: view.instanceId, selectionRevision: view.selectionRevision, basisCursor: view.headCursor ?? view.cursor, mode: priority ? 'steer' : 'followUp', message };
  }
  private async submit(): Promise<void> {
    const work = this.current(); if (!work || this.busy || !this.storageOK) return;
    const draft = this.draft(); this.busy = true;
    try {
      if (!draft.pending) {
        const base = { actionId: crypto.randomUUID(), revision: draft.basisRevision ?? work.revision };
        let action: WorkAction;
        switch (draft.kind) {
          case 'initialize': action = { ...base, kind: 'initialize', nextStep: draft.nextStep }; break;
          case 'assign': case 'intervene': {
            const id = draft.kind === 'assign' ? draft.agentId : work.assigneeAgentId;
            const agent = this.overview?.agents.find(a => a.id === id);
            if (!agent) throw new Error('尚未指定可接收訊息的代理。');
            const send = await this.sendRequest(agent, draft.message, draft.kind === 'intervene');
            action = draft.kind === 'assign' ? { ...base, kind: 'assign', agentId: agent.id, nextStep: draft.nextStep, send } : { ...base, kind: 'intervene', send }; break;
          }
          case 'acknowledge':
            if (!work.pendingInstruction || work.pendingInstruction.acknowledgedAt) throw new Error('沒有等待確認的方向變更。');
            action = { ...base, kind: 'acknowledge', instructionId: work.pendingInstruction.id, evidence: draft.evidence }; break;
          case 'deliver': action = { ...base, kind: 'deliver', evidence: draft.evidence, nextStep: draft.nextStep }; break;
          case 'accept': action = { ...base, kind: 'accept', evidence: draft.evidence }; break;
          case 'block': action = { ...base, kind: 'block', reason: draft.reason, nextStep: draft.nextStep }; break;
          case 'bind_session': action = { ...base, kind: 'bind_session', agentId: draft.agentId, role: draft.role ?? 'worker', parentAgentId: draft.agentId === work.ownerAgentId ? null : work.ownerAgentId,
            reviewedSha: draft.reviewedSha?.trim() || null, expectedEvent: draft.nextStep, nextExpectedAt: draft.nextExpectedAt ? new Date(draft.nextExpectedAt).toISOString() : null }; break;
          case 'unbind_session': action = { ...base, kind: 'unbind_session', bindingId: draft.bindingId ?? '' }; break;
          case 'handoff_owner': action = { ...base, kind: 'handoff_owner', ownerAgentId: draft.agentId, evidence: draft.evidence }; break;
        }
        draft.pending = action;
        draft.rejected = false;
        if (!this.save()) { draft.pending = null; return; }
      }
      this.renderForm();
      const result = await this.host.api<WorkView>(`/api/works/${encodeURIComponent(work.id)}/actions`, draft.pending);
      if (result.id !== work.id || result.confirmedActionId !== draft.pending.actionId) throw new Error('工作回執與原始操作不符。');
      this.works = this.works.map(w => w.id === result.id ? result : w);
      const uncertain = ['assign', 'intervene'].includes(draft.pending.kind) && (!result.deliveryStatus || ['prepared', 'unknown', 'unconfirmed'].includes(result.deliveryStatus));
      if (!uncertain) { draft.pending = null; draft.basisRevision = result.revision; }
      this.save(); this.feedback.textContent = uncertain ? '交接意圖已記錄，傳送結果待確認；原始操作已保留。' : '工作紀錄已更新；下一步與收尾負責人已保留。'; this.signature = ''; this.render();
    } catch (error) {
      const code = error && typeof error === 'object' && 'code' in error ? String(error.code) : '';
      if (draft.pending) {
        draft.rejected = ['INVALID_DATA', 'INVALID_ID', 'INVALID_REQUEST', 'INVALID_ACTION', 'TOO_LARGE', 'WORK_EVENT_TOO_LARGE', 'STALE_WORK', 'INVALID_TRANSITION', 'WRONG_PROJECT', 'STALE_SELECTION', 'INSTANCE_CHANGED', 'UNAVAILABLE', 'WAITING_USER'].includes(code);
        this.save();
      }
      this.feedback.textContent = `${err(error)}${draft.pending ? draft.rejected ? ' 尚未接受這項操作，可保留草稿重新編輯。' : ' 原始操作編號已保留，請查詢／恢復原操作。' : ''}`;
    }
    finally { this.busy = false; this.renderForm(); }
  }
  private async acknowledgeEvent(eventId: string): Promise<void> {
    if (this.busy || !this.storageOK) return;
    const draft = this.draft();
    if (draft.pendingAck && draft.pendingAck.eventId !== eventId) { this.feedback.textContent = '請先恢復前一筆事件確認。'; return; }
    draft.pendingAck ??= { eventId, actionId: crypto.randomUUID(), evidence: '操作者於工作台確認已讀；不代表任務驗收或指示語意確認。' };
    if (!this.save()) return;
    this.busy = true;
    try {
      const receipt = await this.host.api<OwnerInboxEvent>('/api/owner-inbox/ack', draft.pendingAck);
      if (receipt.id !== draft.pendingAck.eventId || receipt.acknowledgementId !== draft.pendingAck.actionId) throw new Error('事件確認回執與原操作不一致。');
      draft.pendingAck = null; this.save(); this.feedback.textContent = '事件已標記為已讀。';
    }
    catch (error) { this.feedback.textContent = `${err(error)} 已保留原事件確認編號。`; }
    finally { this.busy = false; this.signature = ''; await this.refresh(); }
  }
}
