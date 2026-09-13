import type { AgentView } from '../contracts.js';
import type { WorkView } from '../workflow-contracts.js';
import type { ContinuationPublication, ContinuationPublishRequest, ContinuationImportRequest, ContinuationTakeoverRequest } from '../continuation-contracts.js';

type Pending = { route: '' ; request: ContinuationPublishRequest } | { route: '/import'; request: ContinuationImportRequest } | { route: '/takeover'; request: ContinuationTakeoverRequest };
interface Draft { goal: string; current: string; summary: string; next: string; questions: string; bundle: string; owner: string; environment: string; release: string; pending: Pending | null; rejected?: boolean }
interface Host { api<T>(path: string, body?: unknown): Promise<T> }
const storageKey = 'edda-manager-continuity-drafts-v1';
const fresh = (): Draft => ({ goal: '', current: '', summary: '', next: '', questions: '', bundle: '', owner: '', environment: '', release: '', pending: null });
function el<K extends keyof HTMLElementTagNameMap>(tag: K, value = '', cls = ''): HTMLElementTagNameMap[K] { const node = document.createElement(tag); node.textContent = value; node.className = cls; return node; }
const definite = new Set(['INVALID_DATA', 'INVALID_ID', 'INVALID_REQUEST', 'TOO_LARGE', 'CONTINUITY_TOO_LARGE', 'CONTINUITY_INVALID', 'STALE_WORK', 'INVALID_OWNER', 'CONTEXT_NOT_ATTACHED']);

/** Human-readable native context, with durable original requests across reloads. */
export class ContinuationBoard {
  private work: WorkView | null = null;
  private agents: AgentView[] = [];
  private drafts: Record<string, Draft> = {};
  private publication: ContinuationPublication | null = null;
  private busy = false;
  private storageOK = true;
  private form = el('div', '', 'work-form');
  private result = el('div', '', 'continuation-result');
  private feedback = el('p', '', 'notice');
  private panel = el('details');
  constructor(readonly root: HTMLElement, private host: Host) {
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw) {
        const parsed: unknown = JSON.parse(raw);
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('draft');
        for (const [id, value] of Object.entries(parsed)) {
          if (!value || typeof value !== 'object' || !['goal','current','summary','next','questions','bundle','owner','environment','release'].every(k => typeof value[k] === 'string') ||
              (value.pending !== null && (!value.pending || !['', '/import', '/takeover'].includes(value.pending.route) || typeof value.pending.request?.actionId !== 'string'))) throw new Error('draft');
          this.drafts[id] = value as Draft;
        }
      }
    } catch { this.storageOK = false; }
    this.panel.append(el('summary', '必要上下文與接手'), el('p', '保存目前進度，讓新 session 從必要資訊接續。接手會記錄本機負責人；執行工作仍走原有交辦流程。', 'muted'), this.form, this.result, this.feedback);
    root.append(this.panel); this.feedback.setAttribute('role', 'status');
    this.panel.addEventListener('toggle', () => { if (this.panel.open) void this.load(); });
    window.addEventListener('storage', event => { if (event.key === storageKey) { this.storageOK = false; this.feedback.textContent = '另一分頁更新了續作請求，請重新載入。'; this.render(); } });
  }
  update(work: WorkView | null, agents: AgentView[]): void {
    const changed = work?.id !== this.work?.id;
    this.work = work; this.agents = agents; this.root.hidden = !work;
    if (changed) { this.publication = null; this.result.replaceChildren(); this.feedback.textContent = ''; this.render(); if (work && this.panel.open) void this.load(); }
  }
  private draft(): Draft { return this.drafts[this.work!.id] ??= fresh(); }
  private persist(): boolean {
    if (!this.storageOK) return false;
    try { localStorage.setItem(storageKey, JSON.stringify(this.drafts)); return true; }
    catch { this.storageOK = false; this.feedback.textContent = '瀏覽器無法保存原請求，尚未傳送。'; return false; }
  }
  private async load(): Promise<void> {
    if (!this.work || this.busy) return;
    const id = this.work.id;
    try {
      const data = await this.host.api<ContinuationPublication>(`/api/works/${id}/continuation`);
      if (this.work?.id !== id) return;
      this.publication = data; this.work = data.work; this.renderResult();
      const d = this.draft();
      if (d.pending && d.pending.route !== '/takeover' && data.operation?.actionId === d.pending.request.actionId) {
        if (data.operation.status === 'attached') { d.pending = null; this.persist(); this.render(); }
        else if (data.operation.status === 'failed') { d.rejected = true; this.persist(); this.render(); }
      }
    } catch (error) { if (this.work?.id === id) this.feedback.textContent = error instanceof Error ? error.message : '無法讀取必要上下文。'; }
  }
  private button(label: string, action: () => void): HTMLButtonElement { const b = el('button', label); b.type = 'button'; b.disabled = this.busy || !this.storageOK; b.onclick = action; return b; }
  private render(): void {
    this.form.replaceChildren(); if (!this.work) return;
    if (!this.storageOK) { this.form.append(el('p', '草稿儲存無法使用，請先恢復瀏覽器儲存。', 'notice')); return; }
    const d = this.draft();
    if (d.pending) {
      this.form.append(el('p', `原操作：${d.pending.request.actionId}`, 'identity'), this.button('查詢／恢復原續作操作', () => { void this.submit(d.pending!); }));
      if (d.rejected) this.form.append(this.button('保留內容重新編輯', () => { d.pending = null; d.rejected = false; this.persist(); this.render(); }));
      return;
    }
    const field = (label: string, key: Exclude<keyof Draft, 'pending' | 'rejected'>, rows = 2) => {
      const input = el('textarea'); input.rows = rows; input.value = d[key]; input.disabled = this.busy;
      input.setAttribute('aria-label', label); input.oninput = () => { d[key] = input.value; this.persist(); };
      const wrap = el('label', label, 'work-field'); wrap.append(input); this.form.append(wrap);
    };
    this.form.append(el('h4', '保存進度'));
    field('目標', 'goal'); field('目前做到哪裡', 'current'); field('已驗證結果與證據位置', 'summary'); field('下一個具體動作', 'next'); field('未解問題（每行一項）', 'questions');
    this.form.append(this.button('保存必要上下文', () => {
      if (!d.next.trim()) { this.feedback.textContent = '請填寫下一個具體動作。'; return; }
      void this.submit({ route: '', request: { actionId: crypto.randomUUID(), revision: this.work!.revision, input: { capsule_version: 1, state: { title: this.work!.title, goal: d.goal, current: d.current, summary: d.summary, next_action: d.next, open_questions: d.questions.split('\n').filter(s => s.trim()) } } } });
    }));
    this.form.append(el('h4', '接續既有上下文'));
    field('貼上原生上下文 JSON', 'bundle', 3);
    this.form.append(this.button('匯入並核對', () => {
      try { const bundle = JSON.parse(d.bundle) as ContinuationImportRequest['bundle']; void this.submit({ route: '/import', request: { actionId: crypto.randomUUID(), revision: this.work!.revision, bundle } }); }
      catch { this.feedback.textContent = 'JSON 格式不正確，尚未送出。'; }
    }));
    const owner = el('select'); owner.setAttribute('aria-label', '接手負責人');
    for (const a of this.agents.filter(a => a.projectId === this.work!.projectId && a.role === 'manager')) { const option = el('option', a.name); option.value = a.id; owner.append(option); }
    owner.value = d.owner || this.work.ownerAgentId; d.owner = owner.value; owner.onchange = () => { d.owner = owner.value; this.persist(); };
    const label = el('label', '接手負責人', 'work-field'); label.append(owner); this.form.append(label);
    field('本機環境與產物已確認的證據', 'environment'); field('原執行者已停止寫入或釋出工作的證據', 'release');
    this.form.append(this.button('記錄接手', () => {
      const capsuleId = this.publication?.context?.capsule.capsule_id;
      if (!capsuleId) { this.feedback.textContent = '請先保存或匯入上下文，並讀取核對結果。'; return; }
      if (this.work!.stage === 'uninitialized') { this.feedback.textContent = '請先在工作交接中安排下一步，再記錄接手。'; return; }
      if (!d.environment.trim() || !d.release.trim()) { this.feedback.textContent = '請填寫環境與原執行者釋出證據。'; return; }
      void this.submit({ route: '/takeover', request: { actionId: crypto.randomUUID(), revision: this.work!.revision, capsuleId, ownerAgentId: d.owner, environmentEvidence: d.environment, releaseEvidence: d.release } });
    }), this.button('重新讀取已保存上下文', () => { void this.load(); }));
  }
  private renderResult(): void {
    this.result.replaceChildren(); const p = this.publication; if (!p) return;
    if (p.operation) this.result.append(el('p', p.operation.notice, 'notice'));
    if (p.operation && ['unknown', 'saved'].includes(p.operation.status)) {
      const exact = el('input'); exact.placeholder = 'cap_…（選填）'; exact.setAttribute('aria-label', '核對原操作的 capsule ID');
      const actionId = p.operation.actionId;
      this.result.append(exact, this.button('核對原生紀錄並恢復連結', () => { void this.recover(actionId, exact.value.trim()); }));
    }
    if (!p.context) { this.result.append(el('p', '此工作尚無可讀取的必要上下文。', 'muted')); return; }
    const c = p.context.capsule, s = c.state;
    const readable = [`# ${s.title || this.work?.title || '續作上下文'}`, `Capsule: ${c.capsule_id}`, 'Authority: data_only', `目標：${s.goal || ''}`, `目前：${s.current || ''}`, `證據：${s.summary || ''}`, `下一步：${s.next_action}`, `未解問題：\n${(s.open_questions ?? []).join('\n')}`, `假設：\n${(s.hypotheses ?? []).join('\n')}`, `已排除：\n${(s.rejected ?? []).map(r => `${r.hypothesis}: ${r.reason}`).join('\n')}`, `Git: ${c.git.branch || ''} @ ${c.git.head_sha || ''}`, `Tasks: ${(c.references.task_ids ?? []).join(', ')}`, `Events: ${(c.references.event_ids ?? []).join(', ')}`, ...p.context.warnings.map(w => `警告：${w}`)].join('\n\n');
    this.result.append(el('h4', '已保存的必要上下文'), el('pre', readable, 'public-text'));
    if (c.truncation?.length) this.result.append(el('p', `原生上下文有截短紀錄：${JSON.stringify(c.truncation)}`, 'notice'));
    this.result.append(this.button('複製給新 session', () => { void this.copy(readable); }));
    if (p.bundle) {
      this.result.append(this.button('複製原生上下文 JSON', () => { void this.copy(JSON.stringify(p.bundle)); }));
      const raw = el('details'), area = el('textarea'); area.readOnly = true; area.rows = 4; area.value = JSON.stringify(p.bundle); area.setAttribute('aria-label', '原生上下文 JSON（可手動複製）');
      raw.append(el('summary', '檢視原生 JSON'), area); this.result.append(raw);
    }
    else this.result.append(el('p', '這份上下文目前只可在原儲存庫讀取；原生匯出不可用。', 'muted'));
  }
  private async copy(value: string): Promise<void> {
    try { await navigator.clipboard.writeText(value); this.feedback.textContent = '已複製。'; }
    catch { const area = el('textarea'); area.value = value; area.readOnly = true; area.setAttribute('aria-label', '可手動複製的上下文'); this.result.append(area); area.select(); this.feedback.textContent = '剪貼簿不可用，已選取內容供手動複製。'; }
  }
  private async recover(actionId: string, capsuleId: string): Promise<void> {
    if (!this.work || this.busy) return;
    const id = this.work.id; this.busy = true; this.render();
    try {
      const data = await this.host.api<ContinuationPublication>(`/api/works/${id}/continuation/recover`, { actionId, ...(capsuleId ? { capsuleId } : {}) });
      if (this.work?.id === id) { this.publication = data; this.work = data.work; const d = this.draft(); if (data.operation?.status === 'attached' && d.pending?.request.actionId === actionId) d.pending = null; this.persist(); }
    } catch (error) { if (this.work?.id === id) this.feedback.textContent = error instanceof Error ? error.message : '無法核對原生紀錄。'; }
    finally { this.busy = false; this.render(); this.renderResult(); }
  }
  private async submit(pending: Pending): Promise<void> {
    if (!this.work || this.busy) return;
    const id = this.work.id, d = this.draft(); d.pending = pending; d.rejected = false;
    if (!this.persist()) return;
    this.busy = true; this.feedback.textContent = '正在核對並保存原操作…'; this.render();
    try {
      const response = await this.host.api<ContinuationPublication | WorkView>(`/api/works/${id}/continuation${pending.route}`, pending.request);
      if ('work' in response) {
        if (this.work?.id === id) { this.publication = response; this.work = response.work; }
        if (response.operation?.status === 'attached') d.pending = null;
        else if (response.operation?.status === 'failed') d.rejected = true;
      } else { if (response.confirmedActionId === pending.request.actionId) d.pending = null; if (this.work?.id === id) this.work = response; }
      this.persist(); if (this.work?.id === id) this.feedback.textContent = d.pending ? '結果仍待确认，原操作編號已保存；請核對紀錄。' : '已記錄完成，可讀取上下文或接續工作交辦。';
    } catch (error) {
      // Validation codes from a POST may also follow a native effect. Query the
      // durable operation before offering edits; never infer no effect from HTTP.
      const code = (error as { code?: string }).code;
      if (code && definite.has(code)) {
        try { const latest = await this.host.api<ContinuationPublication>(`/api/works/${id}/continuation`); d.rejected = latest.operation?.actionId !== pending.request.actionId; if (this.work?.id === id) this.publication = latest; } catch { /* retain original intent */ }
      }
      this.persist(); if (this.work?.id === id) this.feedback.textContent = error instanceof Error ? error.message : '回覆未確認；請保留原操作。';
    } finally { this.busy = false; this.render(); this.renderResult(); }
  }
}
