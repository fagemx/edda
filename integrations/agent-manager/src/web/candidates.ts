import type { AgentView, CandidateListView, CandidateView, Overview, RegisterCandidateRequest, RuntimeState } from '../contracts.js';

interface Host { api<T>(path: string, body?: unknown): Promise<T>; refresh(): void }
const stateLabels: Record<RuntimeState, string> = { running: '執行中', executing_tool: '工具執行中', idle: '閒置・待確認', waiting_user: '等待回覆', stopped: '已停止', unavailable: '無法連線', unknown: '狀態未知' };

function el<K extends keyof HTMLElementTagNameMap>(tag: K, text = '', cls = ''): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag); node.textContent = text; node.className = cls; return node;
}
function err(error: unknown): string { return error instanceof Error ? error.message : '無法取得回覆；請保留原始請求。'; }
type CandidateFormFields = { name: string; id: string; projectId: string; role: 'manager' | 'worker' };

// Candidate runs are a read-only projection of the bounded session listing.
// Nothing here binds, assigns, wakes, launches or sends; the only mutation is an
// explicit operator click that adds a run to this running process.
export class CandidatePanel {
  private overview: Overview | null = null;
  private view: CandidateListView | null = null;
  private busy = false;
  private loading = false;
  private openForm = '';
  // A failed registration must not erase what the operator typed: the draft is
  // keyed by candidate id, prefilled into the rebuilt form, and cleared only on
  // success or cancel.
  private draft: (CandidateFormFields & { candidateId: string }) | null = null;
  private feedback = el('p', '', 'notice');
  private list = el('div', '', 'candidate-list');
  private refreshButton = el('button', '更新候選', 'quiet');
  private title = el('h3', '候選執行');
  constructor(private root: HTMLElement, private host: Host) {
    this.feedback.setAttribute('role', 'status');
    this.refreshButton.type = 'button';
    this.refreshButton.addEventListener('click', () => { void this.refresh(); });
    const heading = el('div', '', 'candidate-heading');
    heading.append(this.title, this.refreshButton);
    root.append(heading, el('p', '只列出本機已明確設定來源中的 Pi 執行；唯讀顯示，加入管理後仍需自行交辦與綁定。', 'muted'), this.feedback, this.list);
  }
  // Do not rebuild the list under an open form: a periodic overview refresh would
  // reset the operator's focus mid-edit. The captured draft still restores every
  // typed field when the form is rebuilt after a failure or an explicit refresh.
  update(overview: Overview): void { this.overview = overview; if (this.view && !this.openForm) this.render(); }
  async refresh(): Promise<void> {
    if (this.loading) return;
    this.loading = true;
    try { this.view = await this.host.api<CandidateListView>('/api/candidates'); this.feedback.textContent = ''; this.render(); }
    catch (error) { this.feedback.textContent = `${err(error)} 既有候選清單為上次觀測。`; }
    finally { this.loading = false; }
  }
  private render(): void {
    const view = this.view;
    this.list.replaceChildren();
    if (!view) { this.list.append(el('p', '尚未讀取候選執行。', 'empty')); return; }
    if (view.issues.length) this.list.append(el('p', `部分來源未納入本次候選。${view.issues.map((issue) => issue.message).join(' ')}`, 'notice'));
    if (!view.candidates.length) this.list.append(el('p', '沒有可顯示的候選執行。連線或離線的已註冊 session 都會列在這裡。', 'empty'));
    const selected = this.openForm;
    for (const candidate of view.candidates) this.list.append(this.row(candidate, selected));
  }
  private row(candidate: CandidateView, selected: string): HTMLElement {
    const item = el('article', '', 'candidate');
    const heading = el('div', '', 'candidate-title');
    heading.append(el('span', stateLabels[candidate.state] ?? '狀態未知', `badge ${candidate.live ? 'active' : 'attention'}`),
      el('span', candidate.live ? '即時' : '記錄／離線', 'muted'));
    item.append(heading, el('p', candidate.sessionId ?? candidate.runId ?? '尚未有 session 識別', 'identity'));
    if (candidate.workspace) item.append(el('p', candidate.workspace, 'muted'));
    item.append(el('p', candidate.reason ?? (candidate.lastProgressAt ? `最近進度 ${candidate.lastProgressAt}` : '尚無進度紀錄'), candidate.reason ? 'notice' : 'muted'));
    // A per-record degradation is shown next to the reason; the row identity above
    // stays the run identity even when the record itself is unreadable.
    if (candidate.degraded) item.append(el('p', `原生紀錄受損：${candidate.degraded.record}（${candidate.degraded.code}）· ${candidate.degraded.message}`, 'notice'));
    if (candidate.configuredAgentId) { item.append(el('p', `已加入管理：${candidate.configuredAgentId}`, 'muted')); return item; }
    if (!candidate.sessionId || !candidate.workspace) { item.append(el('p', '此項目尚無可綁定的 session 或工作目錄，只能顯示。', 'notice')); return item; }
    if (selected !== candidate.id) {
      const open = el('button', '加入管理'); open.type = 'button';
      open.addEventListener('click', () => { this.openForm = candidate.id; this.render(); });
      item.append(open); return item;
    }
    item.append(this.form(candidate)); return item;
  }
  private form(candidate: CandidateView): HTMLElement {
    const form = el('form', '', 'candidate-form');
    const draft = this.draft?.candidateId === candidate.id ? this.draft : null;
    const nameInput = el('input'); nameInput.required = true; nameInput.maxLength = 200; nameInput.placeholder = '顯示名稱'; nameInput.setAttribute('aria-label', '代理名稱');
    nameInput.value = draft?.name ?? '';
    const idInput = el('input'); idInput.required = true; idInput.maxLength = 64; idInput.pattern = '[a-zA-Z0-9][a-zA-Z0-9_-]*'; idInput.placeholder = '識別碼（英數、-、_）'; idInput.setAttribute('aria-label', '代理識別碼');
    idInput.value = draft?.id ?? '';
    const project = el('select'); project.setAttribute('aria-label', '所屬專案');
    for (const value of this.overview?.projects ?? []) { const option = el('option', value.name); option.value = value.id; project.append(option); }
    if (draft && [...project.options].some((option) => option.value === draft.projectId)) project.value = draft.projectId;
    const role = el('select'); role.setAttribute('aria-label', '代理角色');
    for (const [value, label] of [['worker', '工作者'], ['manager', '管理者']] as const) { const option = el('option', label); option.value = value; role.append(option); }
    if (draft && [...role.options].some((option) => option.value === draft.role)) role.value = draft.role;
    const submit = el('button', '確認加入', 'primary'); submit.type = 'submit'; submit.disabled = this.busy || !project.value;
    const cancel = el('button', '取消'); cancel.type = 'button'; cancel.addEventListener('click', () => { this.openForm = ''; this.draft = null; this.render(); });
    // Capture every edit, not just the submit: a periodic overview refresh or a
    // failed registration rebuilds this form, and the operator's typed values
    // must come back with it.
    const capture = (): CandidateFormFields => {
      const fields: CandidateFormFields = { name: nameInput.value, id: idInput.value, projectId: project.value, role: role.value as 'manager' | 'worker' };
      this.draft = { candidateId: candidate.id, ...fields };
      return fields;
    };
    nameInput.addEventListener('input', capture); idInput.addEventListener('input', capture);
    project.addEventListener('change', capture); role.addEventListener('change', capture);
    form.addEventListener('submit', (event) => { event.preventDefault(); void this.register(candidate, capture()); });
    for (const field of [nameInput, idInput, project, role]) { const label = el('label', field.getAttribute('aria-label') ?? '', 'candidate-field'); label.append(field); form.append(label); }
    const actions = el('div', '', 'candidate-actions'); actions.append(submit, cancel); form.append(actions);
    form.append(el('p', `加入後會出現在左側清單，可用既有「登記執行 session」綁定到工作；不會自動交辦或喚醒。`, 'muted'));
    return form;
  }
  private async register(candidate: CandidateView, fields: CandidateFormFields): Promise<void> {
    if (this.busy) return;
    const request: RegisterCandidateRequest = { candidateId: candidate.id, id: fields.id.trim(), name: fields.name.trim(), role: fields.role, projectId: fields.projectId };
    if (!request.id || !request.name) { this.feedback.textContent = '請填寫代理名稱與識別碼。'; return; }
    this.busy = true; this.feedback.textContent = '正在加入管理…';
    try {
      const agent = await this.host.api<AgentView>('/api/candidates/register', request);
      this.feedback.textContent = `${agent.name} 已加入目前的工作台；請用既有的工作操作交辦或綁定。`;
      this.openForm = ''; this.draft = null; this.success();
    } catch (error) { this.feedback.textContent = `${err(error)} 表單內容已保留，可修正後再試。`; }
    finally { this.busy = false; this.render(); }
  }
  // A registered run is only added to this process; reload the agent list and
  // candidate list so the operator sees the live state, never a stale duplicate.
  private success(): void { this.host.refresh(); void this.refresh(); }
}
