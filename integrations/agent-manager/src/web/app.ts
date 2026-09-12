import type { AgentView, ConversationView, MessageMode, OperationStatus, OperationView, Overview, PublicEntry, RuntimeState, SendRequest } from '../contracts.js';
import { MAX_MESSAGE_BYTES } from '../contracts.js';

// Everything from the gateway and browser storage is rendered as text, never HTML.
function element<K extends keyof HTMLElementTagNameMap>(tag: K, text = '', className = ''): HTMLElementTagNameMap[K] {
  const result = document.createElement(tag);
  result.textContent = text;
  result.className = className;
  return result;
}
function get<K extends HTMLElement = HTMLElement>(id: string): K {
  const result = document.getElementById(id);
  if (!result) throw new Error(`Missing interface element: ${id}`);
  return result as K;
}
function notice(id: string, text: string): void { get(id).textContent = text; get(id).hidden = !text; }
const tokenKey = 'edda-manager-token';
const stateKey = 'edda-manager-drafts-v1';
let storageWorks = true;
function stored(key: string): string | null {
  try { return localStorage.getItem(key); } catch { storageWorks = false; return null; }
}
function save(key: string, value: string): boolean {
  try { localStorage.setItem(key, value); return true; }
  catch { storageWorks = false; notice('global-notice', '瀏覽器無法保存資料。草稿暫存於本頁；請先恢復儲存空間或權限，再傳送訊息。'); return false; }
}
const fragmentToken = new URLSearchParams(location.hash.slice(1)).get('token');
// Remove the launcher credential before any request or further initialization.
if (location.hash) history.replaceState(null, '', location.pathname + location.search);
let token = fragmentToken ?? stored(tokenKey) ?? '';
if (fragmentToken) save(tokenKey, fragmentToken);

interface Pending { request: SendRequest; targetName: string; rejected: boolean; notice: string }
interface Draft { agentId: string; message: string; mode: MessageMode; pending: Pending | null }
const drafts = new Map<string, Draft>();
let selectedId = '';
function record(value: unknown): value is Record<string, unknown> { return typeof value === 'object' && value !== null; }
function isRequest(value: unknown): value is SendRequest {
  return record(value) && typeof value.operationId === 'string' && typeof value.selectionRevision === 'string' && typeof value.instanceId === 'string'
    && (value.basisCursor === null || typeof value.basisCursor === 'string') && (value.mode === 'followUp' || value.mode === 'steer') && typeof value.message === 'string';
}
function restore(): void {
  const raw = stored(stateKey);
  if (!raw) return;
  try {
    const state: unknown = JSON.parse(raw);
    if (!record(state) || !Array.isArray(state.drafts)) throw new Error('Invalid saved drafts');
    if (typeof state.selectedId === 'string') selectedId = state.selectedId;
    for (const value of state.drafts) {
      if (!record(value) || typeof value.agentId !== 'string' || typeof value.message !== 'string' || (value.mode !== 'followUp' && value.mode !== 'steer')) throw new Error('Invalid saved draft');
      let pending: Pending | null = null;
      if (value.pending !== null) {
        if (!record(value.pending) || !isRequest(value.pending.request) || typeof value.pending.targetName !== 'string' || typeof value.pending.rejected !== 'boolean' || typeof value.pending.notice !== 'string') throw new Error('Invalid pending request');
        pending = { request: value.pending.request, targetName: value.pending.targetName, rejected: value.pending.rejected, notice: value.pending.notice };
      }
      drafts.set(value.agentId, { agentId: value.agentId, message: value.message, mode: value.mode, pending });
    }
  } catch {
    // Do not overwrite unreadable pending-operation evidence with a new request.
    storageWorks = false;
    notice('global-notice', '已保存的草稿無法讀取。原始資料已保留，傳送功能暫停；請先檢查瀏覽器儲存資料。');
  }
}
restore();
function persist(): boolean {
  if (!storageWorks) return false;
  return save(stateKey, JSON.stringify({ selectedId, drafts: [...drafts.values()] }));
}
function draftFor(id: string): Draft {
  let draft = drafts.get(id);
  if (!draft) { draft = { agentId: id, message: '', mode: 'followUp', pending: null }; drafts.set(id, draft); }
  return draft;
}
let overview: Overview | null = null;
let conversation: ConversationView | null = null;
let generation = 0;
let overviewBusy = false;
let conversationBusy: number | null = null;
let connected = false;
const sending = new Set<string>();
const checking = new Set<string>();
const entries = new Map<string, PublicEntry>();
let conversationSignature = '';
let projectSignature = '';
const agentAges = new Map<string, HTMLElement>();
const railSignatures = new Map<string, string>();
function selectedAgent(): AgentView | undefined { return overview?.agents.find(agent => agent.id === selectedId); }
function time(value: string | null): string {
  if (!value) return '未提供';
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '時間未知' : date.toLocaleString('zh-TW', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });
}
function age(value: string): string {
  const seconds = Math.max(0, Math.floor((Date.now() - Date.parse(value)) / 1000));
  return Number.isNaN(seconds) ? '時間未知' : seconds < 60 ? `${seconds} 秒前` : `${Math.floor(seconds / 60)} 分鐘前`;
}
const runtimeLabels: Record<RuntimeState, string> = { running: '執行中', executing_tool: '工具執行中', idle: '閒置・待確認', waiting_user: '等待回覆', stopped: '已停止', unavailable: '無法連線', unknown: '狀態未知' };
const operationLabels: Record<OperationStatus, string> = { prepared: '已記錄送出意圖', unconfirmed: '尚未確認送達', accepted: '已接收訊息', queued: '已排入佇列', started: '已開始處理', settled: '執行已結束', failed: '執行失敗', unknown: '結果不明' };
function badge(text: string, tone = ''): HTMLElement { return element('span', text, `badge ${tone}`); }
function runtimeBadge(agent: AgentView): HTMLElement {
  const tone = agent.stale || ['waiting_user', 'unknown', 'unavailable'].includes(agent.state) ? 'attention' : ['running', 'executing_tool'].includes(agent.state) ? 'active' : '';
  return badge(runtimeLabels[agent.state] ?? '狀態未知', tone);
}
function operationBadge(operation: OperationView): HTMLElement {
  return badge(operationLabels[operation.status] ?? '結果不明', operation.status === 'failed' ? 'danger' : ['prepared', 'unknown', 'unconfirmed'].includes(operation.status) ? 'attention' : 'active');
}
class ApiError extends Error { constructor(public code: string, message: string, public status: number) { super(message); } }
async function api<T>(path: string, body?: SendRequest): Promise<T> {
  const headers: Record<string, string> = { Authorization: `Bearer ${token}` };
  if (body) headers['Content-Type'] = 'application/json';
  const response = await fetch(path, { method: body ? 'POST' : 'GET', headers, ...(body ? { body: JSON.stringify(body) } : {}), cache: 'no-store', credentials: 'omit', signal: AbortSignal.timeout(15000) });
  if (!response.ok) {
    let code = 'http_error'; let message = `工作台回應錯誤（${response.status}）`;
    try { const data: unknown = await response.json(); if (record(data) && record(data.error)) { if (typeof data.error.code === 'string') code = data.error.code; if (typeof data.error.message === 'string') message = data.error.message; } } catch { /* Keep the bounded status message. */ }
    throw new ApiError(code, message, response.status);
  }
  return await response.json() as T;
}
function errorText(error: unknown): string { return error instanceof ApiError ? error.message : '連線中斷或回應逾時。'; }
function renderProjects(): void {
  if (!overview) return;
  for (const agent of overview.agents) { const stamp = agentAges.get(agent.id); if (stamp) stamp.textContent = agent.stale ? '資料已過期' : age(agent.observedAt); }
  const signature = JSON.stringify([overview.projects.map(project => [project.id, project.name, project.priority]), overview.agents.map(agent => [agent.id, agent.name, agent.projectId, agent.role, agent.state, agent.stale, agent.latestMessage?.text, !!drafts.get(agent.id)?.pending]), selectedId]);
  if (signature === projectSignature) return;
  projectSignature = signature;
  const focusedId = document.activeElement instanceof HTMLElement ? document.activeElement.dataset.agentId : undefined;
  agentAges.clear();
  get('project-count').textContent = `${overview.projects.length} 個`;
  const nodes: HTMLElement[] = [];
  for (const project of [...overview.projects].sort((a, b) => a.priority - b.priority)) {
    const group = element('section', '', 'project-group');
    group.append(element('h2', project.name, 'project-name'));
    const agents = overview.agents.filter(agent => agent.projectId === project.id);
    for (const agent of agents) {
      const button = element('button', '', 'agent-option'); button.type = 'button'; button.dataset.agentId = agent.id; button.setAttribute('aria-current', String(agent.id === selectedId));
      const top = element('span', '', 'agent-option-top'); top.append(element('span', agent.name, 'agent-option-name'), element('span', agent.role === 'manager' ? '管理者' : '工作者', 'agent-role'));
      const stamp = element('span', agent.stale ? '資料已過期' : age(agent.observedAt)); agentAges.set(agent.id, stamp);
      const bottom = element('span', '', 'agent-option-bottom'); bottom.append(runtimeBadge(agent), stamp);
      const pending = drafts.get(agent.id)?.pending;
      button.append(top, element('div', agent.latestMessage?.text || '尚無公開回覆', 'agent-preview'), bottom);
      if (pending) button.append(element('div', '有待確認的訊息收據', 'agent-preview'));
      button.addEventListener('click', () => selectAgent(agent.id)); group.append(button);
    }
    if (!agents.length) group.append(element('p', '此專案尚未選入代理。', 'empty'));
    nodes.push(group);
  }
  get('projects').replaceChildren(...(nodes.length ? nodes : [element('p', '尚未登錄專案。請在工作台設定中選入專案與代理。', 'empty')]));
  if (focusedId) for (const button of get('projects').querySelectorAll<HTMLButtonElement>('button')) { if (button.dataset.agentId === focusedId) button.focus({ preventScroll: true }); }
}
function replaceRail(id: string, nodes: HTMLElement[]): void {
  const signature = nodes.map(node => node.textContent).join('\n');
  if (railSignatures.get(id) === signature) return;
  railSignatures.set(id, signature);
  const open = new Set([...get(id).querySelectorAll<HTMLDetailsElement>('details[open]')].map(item => item.dataset.operationId));
  for (const node of nodes) for (const detail of node.querySelectorAll<HTMLDetailsElement>('details')) { if (open.has(detail.dataset.operationId)) detail.open = true; }
  get(id).replaceChildren(...nodes);
}
function renderAgent(): void {
  const agent = selectedAgent();
  get('welcome').hidden = !!agent; get('agent-panel').hidden = !agent;
  if (!agent) return;
  get('agent-project').textContent = overview?.projects.find(project => project.id === agent.projectId)?.name ?? agent.projectId;
  get('agent-name').textContent = agent.name;
  get('agent-identity').textContent = `${agent.id} / ${agent.workspace}`;
  get('agent-status').replaceChildren(runtimeBadge(agent));
  get('agent-freshness').replaceChildren(...[
    `觀測 ${time(agent.observedAt)}`, `心跳 ${time(agent.heartbeatAt)}`, `進度 ${time(agent.lastProgressAt)}`,
    ...(agent.model ? [`模型 ${agent.model.provider} / ${agent.model.id}`] : []),
    ...(agent.usage?.tokens !== null && agent.usage?.tokens !== undefined ? [`已回報用量 ${agent.usage.tokens.toLocaleString('zh-TW')} tokens`] : []),
    ...(agent.usage?.reportedCost !== null && agent.usage?.reportedCost !== undefined ? [`已回報成本 ${agent.usage.reportedCost}（供應商單位）`] : []),
  ].map(value => element('span', value)));
  notice('agent-warning', agent.stale || agent.source === 'unavailable' ? `目前資料${agent.stale ? '已過期' : '無法更新'}。${agent.reason ?? '等待來源恢復；其他代理仍可使用。'}` : agent.reason ?? '');
  get('latest-response').textContent = agent.latestMessage?.text || '尚無公開回覆。';
  get('recipient').textContent = `${agent.name} (${agent.id})`;
  renderCompose();
}
function renderRail(): void {
  if (!overview) return;
  const agent = selectedAgent(); const project = overview.projects.find(item => item.id === agent?.projectId);
  const managers = overview.agents.filter(item => item.projectId === project?.id && item.role === 'manager');
  replaceRail('summaries', (managers.length ? managers.map(manager => {
    const item = element('article', '', 'summary-item'); item.append(element('h3', manager.name), element('p', manager.summary || '尚未提供管理摘要。', 'public-text'), element('p', `更新 ${time(manager.summaryUpdatedAt)}`, 'muted'));
    if (manager.summaryError) item.append(element('p', manager.summaryError, 'notice'));
    return item;
  }) : [element('p', project ? '此專案尚未選入管理者。' : '選擇代理後顯示專案摘要。', 'empty')]));
  replaceRail('resources', (project?.resources.length ? project.resources.map(resource => {
    const item = element('article', '', 'resource'); item.append(element('h3', resource.name), element('p', resource.details, 'public-text'), element('p', `${resource.kind} / 歸屬：${resource.owner}`, 'muted'), element('p', `來源：${resource.source}`, 'muted')); return item;
  }) : [element('p', project ? '尚未登錄共用資源。' : '選擇代理後顯示登錄資源。', 'empty')]));
  const relevant = (id: string): boolean => !project || overview!.agents.some(item => item.id === id && item.projectId === project.id);
  const operations = overview.operations.filter(item => relevant(item.agentId)).slice(0, 12);
  replaceRail('operations', (operations.length ? operations.map(operation => {
    const item = element('article', '', 'operation'); const heading = element('div', '', 'operation-heading');
    heading.append(element('span', overview!.agents.find(item => item.id === operation.agentId)?.name ?? operation.agentId, 'operation-name'), operationBadge(operation));
    const detail = element('details'); detail.dataset.operationId = operation.id; detail.append(element('summary', operation.mode === 'steer' ? '優先指示' : '接續目前工作'), element('p', operation.message, 'public-text'), element('p', operation.id, 'identity'));
    item.append(heading, element('p', operation.notice), element('p', time(operation.updatedAt), 'muted'), detail);
    if (operation.status === 'settled') item.append(element('p', '此為執行收據，工作仍需驗收。', 'muted'));
    return item;
  }) : [element('p', '尚無訊息收據。', 'empty')]));
  const events = overview.events.filter(item => relevant(item.agentId)).slice(0, 15);
  replaceRail('events', (events.length ? events.map(event => {
    const item = element('article', '', 'event'); const stamp = element('time', time(event.at)); stamp.dateTime = event.at;
    item.append(element('p', overview!.agents.find(item => item.id === event.agentId)?.name ?? event.agentId, 'muted'), element('p', event.summary), stamp); return item;
  }) : [element('p', '尚無持久活動紀錄。', 'empty')]));
}
function renderConversation(): void {
  const signature = JSON.stringify([...entries.values()]);
  if (signature !== conversationSignature) {
    const pane = get('conversation'); const atEnd = pane.scrollHeight - pane.scrollTop - pane.clientHeight < 70; const oldScroll = pane.scrollTop;
    const opened = new Set([...pane.querySelectorAll<HTMLDetailsElement>('details[data-tool-group][open]')].map(item => item.dataset.toolGroup));
    const nodes: HTMLElement[] = []; let group: HTMLDetailsElement | null = null; let count = 0;
    for (const entry of entries.values()) {
      if (entry.kind === 'tool_result') {
        if (!group) { group = element('details', '', 'entry tool'); group.dataset.toolGroup = entry.id; group.open = opened.has(entry.id); group.append(element('summary', '', 'entry-meta')); nodes.push(group); count = 0; }
        count++; group.querySelector('summary')!.textContent = `工具活動（${count} 筆）`;
        group.append(element('p', `${entry.toolName ?? '工具'}${entry.toolError ? '：回報錯誤' : '：已返回'} · ${time(entry.timestamp)}`, 'muted'));
        continue;
      }
      if (!entry.text.trim()) continue;
      group = null;
      const item = element('article', '', `entry ${entry.role === 'user' ? 'user' : 'assistant'}`);
      const meta = element('div', '', 'entry-meta'); meta.append(element('span', entry.role === 'user' ? '操作訊息' : '代理回覆', 'entry-role'), element('time', time(entry.timestamp)));
      item.append(meta, element('div', entry.text, 'entry-body'));
      if (entry.truncated) item.append(element('p', '此則內容已截短。', 'entry-truncated'));
      nodes.push(item);
    }
    pane.replaceChildren(...(nodes.length ? nodes : [element('p', '尚無公開對話。可在下方開始新訊息。', 'empty')]));
    pane.scrollTop = atEnd ? pane.scrollHeight : oldScroll; conversationSignature = signature;
  }
  get('conversation-meta').textContent = conversation ? `觀測 ${time(conversation.observedAt)}` : '讀取中';
  get('load-more').hidden = !conversation?.hasMore;
  renderCompose();
}
function renderCompose(): void {
  const agent = selectedAgent(); if (!agent) return;
  const draft = draftFor(agent.id); const pending = draft.pending; const message = get<HTMLTextAreaElement>('message');
  const viewMatches = conversation?.agentId === agent.id && conversation.instanceId === agent.instanceId && conversation.selectionRevision === agent.selectionRevision;
  // Old heartbeat metadata is advisory when an authenticated current conversation
  // confirms the same instance; the server checks that instance again on send.
  const canSend = connected && storageWorks && !!token && agent.capabilities.send && agent.source === 'live' && !!agent.instanceId && viewMatches && conversation?.source === 'live';
  const tooLong = new TextEncoder().encode(draft.message).length > MAX_MESSAGE_BYTES;
  message.disabled = !!pending;
  get<HTMLSelectElement>('mode').disabled = !!pending;
  get<HTMLButtonElement>('send').disabled = !canSend || !!pending || sending.has(agent.id) || !draft.message.trim() || tooLong;
  get('mode-help').textContent = draft.mode === 'steer' ? '在下一個安全處理點優先讀取這則指示；不會強制終止正在執行的工具。' : '目前工作結束後，再處理這則訊息。';
  get('draft-status').textContent = pending ? '原始訊息與收據識別碼已保留' : !storageWorks ? '草稿僅暫存於本頁' : draft.message ? '草稿已保存在此瀏覽器' : '草稿依收件人自動保存';
  get('message-size').textContent = tooLong ? '訊息超過 12 KiB，請縮短內容' : `${new TextEncoder().encode(draft.message).length.toLocaleString('zh-TW')} / ${MAX_MESSAGE_BYTES.toLocaleString('zh-TW')} bytes`;
  if (!pending && !canSend) notice('send-notice', !storageWorks ? '請先恢復瀏覽器儲存功能。' : !connected ? '工作台連線中斷，草稿已保留。' : '等待此代理的最新對話與可傳送狀態。');
  else notice('send-notice', '');
  get('pending').hidden = !pending;
  if (pending) {
    get('pending-title').textContent = pending.rejected ? '請檢查這則訊息的結果' : '送出結果尚待確認';
    get('pending-notice').textContent = `${pending.targetName}：${pending.notice} ${pending.rejected ? '可保留草稿，編輯後另送新訊息。' : '請查詢原始收據，確認前不會再次送出。'}`;
    get('pending-id').textContent = pending.request.operationId;
    get('pending-message').textContent = pending.request.message;
    get<HTMLButtonElement>('check-receipt').disabled = checking.has(agent.id) || sending.has(agent.id) || !connected;
    get('release-rejected').hidden = !pending.rejected;
  }
}
function selectAgent(id: string): void {
  if (id === selectedId && conversation) return;
  selectedId = id; generation++; conversation = null; entries.clear(); conversationSignature = 'reset'; persist();
  const draft = draftFor(id); get<HTMLTextAreaElement>('message').value = draft.message; get<HTMLSelectElement>('mode').value = draft.mode;
  notice('conversation-notice', ''); renderProjects(); renderAgent(); renderRail(); renderConversation();
  void refreshConversation();
}
function applyOperation(operation: OperationView): void {
  const draft = drafts.get(operation.agentId); const pending = draft?.pending;
  if (!draft || !pending || pending.request.operationId !== operation.id) return;
  // A receipt must belong to the persisted recipient and immutable request.
  const request = pending.request;
  if (operation.instanceId !== request.instanceId || operation.mode !== request.mode || operation.message !== request.message || operation.basisCursor !== request.basisCursor) {
    pending.rejected = false; pending.notice = '收據內容與原始請求不一致，請保留資料並檢查工作台。'; persist(); return;
  }
  if (['accepted', 'queued', 'started', 'settled'].includes(operation.status)) {
    if (draft.message === request.message && draft.mode === request.mode) draft.message = '';
    draft.pending = null;
    if (selectedId === operation.agentId) get<HTMLTextAreaElement>('message').value = draft.message;
  } else { pending.notice = `${operationLabels[operation.status]}。${operation.notice}`; pending.rejected = operation.status === 'failed'; }
  persist();
}
async function refreshOverview(): Promise<void> {
  if (overviewBusy || !token) return;
  overviewBusy = true;
  try {
    const result = await api<Overview>('/api/overview');
    const before = selectedAgent(); overview = result; connected = true;
    get('auth').hidden = true; get('desk').hidden = false;
    get('connection-status').textContent = `已連線 · 更新 ${time(result.generatedAt)}`;
    if (storageWorks) notice('global-notice', '');
    for (const operation of result.operations) applyOperation(operation);
    const after = selectedAgent();
    if (selectedId && after && (!before || before.instanceId !== after.instanceId || before.selectionRevision !== after.selectionRevision)) selectAgentAfterIdentityChange();
    if (selectedId && !after) { generation++; conversation = null; entries.clear(); notice('global-notice', '原收件人已移出選取清單；其草稿與待確認請求仍保留。請選擇其他代理。'); }
    renderProjects(); renderAgent(); renderRail();
    if (!selectedId && result.agents.length) {
      const first = result.agents.find(a => a.projectId === result.projects[0]?.id && a.role === 'manager') ?? result.agents[0];
      if (first) selectAgent(first.id);
    }
  } catch (error) {
    connected = false; get('connection-status').textContent = '連線中斷';
    notice('global-notice', `${errorText(error)} 現有畫面為上次觀測，草稿與原始送出請求已保留。`);
    if (error instanceof ApiError && error.status === 401) { get('auth').hidden = false; get('desk').hidden = true; }
    renderCompose();
  } finally { overviewBusy = false; }
}
function selectAgentAfterIdentityChange(): void {
  generation++; conversation = null; entries.clear(); conversationSignature = 'reset';
  const draft = draftFor(selectedId); get<HTMLTextAreaElement>('message').value = draft.message; get<HTMLSelectElement>('mode').value = draft.mode;
  notice('conversation-notice', '正在讀取目前代理實例的對話；草稿已保留。'); renderConversation();
}
async function refreshConversation(reset = false): Promise<void> {
  const agent = selectedAgent(); const epoch = generation;
  if (!agent || !connected || !agent.capabilities.conversation || conversationBusy === epoch) return;
  conversationBusy = epoch;
  const cursor = reset ? null : conversation?.cursor;
  try {
    const result = await api<ConversationView>(`/api/agents/${encodeURIComponent(agent.id)}/conversation${cursor ? `?after=${encodeURIComponent(cursor)}` : ''}`);
    if (epoch !== generation || selectedId !== agent.id) return;
    const current = selectedAgent();
    if (result.agentId !== agent.id || result.selectionRevision !== current?.selectionRevision || result.instanceId !== current.instanceId) {
      conversation = null; notice('conversation-notice', '代理實例已更新，等待重新讀取對話。'); renderCompose(); return;
    }
    if (reset || !conversation) entries.clear();
    for (const entry of result.entries) entries.set(entry.id, entry);
    conversation = result;
    notice('conversation-notice', result.source === 'unavailable' ? '目前無法讀取對話；保留上次觀測與草稿。' : '');
    renderConversation();
  } catch (error) {
    if (epoch !== generation) return;
    if (cursor && error instanceof ApiError && (error.status === 409 || /cursor|branch/i.test(error.code))) {
      conversation = null; conversationBusy = null; notice('conversation-notice', '對話分支已改變，重新讀取目前對話；草稿已保留。');
      await refreshConversation(true); return;
    }
    conversation = null; notice('conversation-notice', `${errorText(error)} 草稿已保留，可更新對話後再試。`); renderCompose();
  } finally { if (conversationBusy === epoch) conversationBusy = null; }
}
async function sendMessage(event: SubmitEvent): Promise<void> {
  event.preventDefault();
  const agent = selectedAgent(); if (!agent || get<HTMLButtonElement>('send').disabled || sending.has(agent.id)) return;
  const draft = draftFor(agent.id); const view = conversation;
  if (draft.pending || !view?.instanceId || view.agentId !== agent.id || view.instanceId !== agent.instanceId || view.selectionRevision !== agent.selectionRevision) return;
  const request: SendRequest = { operationId: crypto.randomUUID(), selectionRevision: view.selectionRevision, instanceId: view.instanceId, basisCursor: view.headCursor ?? view.cursor, mode: draft.mode, message: draft.message };
  draft.pending = { request, targetName: `${agent.name} (${agent.id})`, rejected: false, notice: '請求已保存，正在等待工作台收據。' };
  // No side effect is allowed until the complete request is durable in this browser.
  if (!persist()) { draft.pending = null; renderCompose(); return; }
  sending.add(agent.id); renderCompose();
  try {
    const operation = await api<OperationView>(`/api/agents/${encodeURIComponent(agent.id)}/messages`, request);
    applyOperation(operation);
  } catch (error) {
    if (draft.pending?.request.operationId === request.operationId) {
      draft.pending.notice = errorText(error);
      // A conflict may refer to an existing effect; never release its UUID blindly.
      draft.pending.rejected = error instanceof ApiError && ([400, 401, 403, 404, 413, 422].includes(error.status)
        || (error.status === 409 && ['INSTANCE_CHANGED', 'STALE_SELECTION', 'WAITING_USER'].includes(error.code)));
      persist();
    }
  } finally { sending.delete(agent.id); renderCompose(); renderProjects(); void refreshOverview(); }
}
async function checkReceipt(): Promise<void> {
  const id = selectedId; const pending = drafts.get(id)?.pending;
  if (!pending || checking.has(id)) return;
  checking.add(id); renderCompose();
  try { applyOperation(await api<OperationView>(`/api/operations/${encodeURIComponent(pending.request.operationId)}`)); }
  catch (error) { pending.notice = error instanceof ApiError && error.status === 404 ? '工作台目前找不到原始收據。送出結果仍不明，原始請求已保留。' : errorText(error); persist(); }
  finally { checking.delete(id); renderCompose(); renderProjects(); void refreshOverview(); }
}
get('compose').addEventListener('submit', event => { void sendMessage(event as SubmitEvent); });
get('message').addEventListener('input', () => { const draft = draftFor(selectedId); if (draft.pending) return; draft.message = get<HTMLTextAreaElement>('message').value; persist(); renderCompose(); });
get('mode').addEventListener('change', () => { const draft = draftFor(selectedId); if (draft.pending) return; draft.mode = get<HTMLSelectElement>('mode').value === 'steer' ? 'steer' : 'followUp'; persist(); renderCompose(); });
get('check-receipt').addEventListener('click', () => { void checkReceipt(); });
get('release-rejected').addEventListener('click', () => { const draft = draftFor(selectedId); if (!draft.pending?.rejected) return; draft.pending = null; persist(); renderCompose(); get('message').focus(); });
get('conversation-refresh').addEventListener('click', () => { void refreshConversation(); });
get('load-more').addEventListener('click', () => { void refreshConversation(); });
async function refresh(): Promise<void> { await refreshOverview(); await refreshConversation(); }
get('refresh').addEventListener('click', () => { void refresh(); });
get('auth-form').addEventListener('submit', event => { event.preventDefault(); token = get<HTMLInputElement>('token').value.trim(); if (!token) return; save(tokenKey, token); get<HTMLInputElement>('token').value = ''; void refresh(); });
window.addEventListener('online', () => { void refresh(); });
window.addEventListener('offline', () => { connected = false; get('connection-status').textContent = '離線'; renderCompose(); });
// Another tab changing persistent evidence cannot safely share this tab's drafts.
window.addEventListener('storage', event => { if (event.key === stateKey) { storageWorks = false; notice('global-notice', '另一個分頁已更新草稿或送出紀錄。本頁暫停傳送，請重新載入以讀取最新紀錄。'); renderCompose(); } });
if (!token) { get('auth').hidden = false; get('desk').hidden = true; get('connection-status').textContent = '尚未連接'; }
if (!storageWorks) notice('global-notice', '瀏覽器儲存資料目前無法使用；傳送功能暫停，既有資料不會被覆寫。');
void refresh();
window.setInterval(() => { if (!document.hidden) void refresh(); }, 3000);
document.addEventListener('visibilitychange', () => { if (!document.hidden) void refresh(); });
