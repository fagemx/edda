import { lstatSync, readFileSync } from 'node:fs';
import { ManagerError, type AgentBinding, type AgentView, type CandidateListView, type ConversationView, type ManagerConfig, type OperationStatus, type OperationView, type Overview, type PiAdapter, type RegisterCandidateRequest, type SendRequest } from './contracts.js';
import { hash, parseConfig, selectionRevision } from './config.js';
import { candidateId, dedupeRuns, projectCandidates } from './discovery.js';
import { unavailable } from './pi-adapter.js';
import { ManagerStore } from './store.js';
import { WorkManager } from './workflow.js';
import type { WorkflowLedger, WorkflowLocks } from './edda-workflow.js';
import { OwnerInbox } from './owner-inbox.js';
import { ContinuationService } from './continuation.js';

const notices: Record<OperationStatus, string> = {
  prepared: '操作已記錄，傳送結果尚未確認。', unconfirmed: '通道已收件，尚未確認代理開始處理。',
  accepted: '通道已接受，尚未確認代理開始處理。', queued: '訊息已排入佇列，等待代理處理。',
  started: '代理已開始處理這則訊息。', settled: '本輪回覆已結束；工作結果仍需驗收。',
  failed: '這次訊息未能執行，請查看原因。', unknown: '結果尚待確認；保留原編號，不會自動重送。',
};
const labels: Record<AgentView['state'], string> = { running: '執行中', executing_tool: '正在使用工具', idle: '本輪回覆已結束，結果待確認', waiting_user: '等待原介面的選項', stopped: '已停止', unavailable: '暫時無法取得狀態', unknown: '狀態尚待確認' };
function summary(binding: AgentBinding): Pick<AgentView, 'summary' | 'summaryUpdatedAt' | 'summaryError'> {
  if (!binding.summaryFile) return { summary: null, summaryUpdatedAt: null, summaryError: null };
  try {
    const stat = lstatSync(binding.summaryFile);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 65536) throw new Error('Invalid summary');
    const value = new TextDecoder('utf-8', { fatal: true }).decode(readFileSync(binding.summaryFile));
    return { summary: value.slice(0, 12000), summaryUpdatedAt: stat.mtime.toISOString(), summaryError: value.length > 12000 ? '摘要過長，只顯示前段。' : null };
  } catch { return { summary: null, summaryUpdatedAt: null, summaryError: '目前沒有可讀取的主管摘要。' }; }
}
export class AgentManager {
  readonly works: WorkManager;
  readonly ownerInbox: OwnerInbox;
  readonly continuation: ContinuationService;
  readonly startedAt = new Date().toISOString();
  private views = new Map<string, AgentView>();
  private refreshing: Promise<void> | null = null;
  private checking = new Map<string, Promise<OperationView>>();
  private timer: ReturnType<typeof setInterval> | undefined;
  constructor(readonly config: ManagerConfig, readonly store: ManagerStore, private adapter: PiAdapter, workflow?: { ledger?: WorkflowLedger; locks?: WorkflowLocks }) {
    store.putSetting('config', JSON.stringify(config));
    this.works = new WorkManager(this, workflow?.ledger, workflow?.locks);
    this.ownerInbox = new OwnerInbox(store);
    this.continuation = new ContinuationService(this, config.continuityExecutable ? { executable: config.continuityExecutable } : {});
  }
  binding(id: string): AgentBinding {
    const binding = this.config.agents.find((a) => a.id === id);
    if (!binding) throw new ManagerError('NOT_FOUND', '此代理未加入管理清單。', 404);
    return binding;
  }
  private piRoots(): string[] {
    const roots = this.config.agents.filter((a) => (a.transport ?? 'pi') === 'pi').map((a) => a.registryRoot);
    const effective = this.adapter.defaultRegistryRoot?.();
    if (effective) roots.push(effective);
    return [...new Set(roots)];
  }
  // Read-only. The bounded root set is the effective default Pi registry plus the
  // registry roots already present in the explicit operator selection; there is
  // no other home/project scan.
  async candidates(): Promise<CandidateListView> {
    const report = await this.adapter.discover?.(this.piRoots()) ?? { runs: [], failures: [] };
    return projectCandidates(report, this.config.agents);
  }
  // Explicit operator action. It adds the discovered run to this running
  // process only; it never edits config.json, restarts a service, assigns work,
  // changes an owner, launches a session or sends a message.
  async registerCandidate(request: RegisterCandidateRequest): Promise<AgentView> {
    if (!this.config.projects.some((p) => p.id === request.projectId)) throw new ManagerError('NOT_FOUND', '此專案未登記，無法加入代理。', 404);
    if (this.config.agents.some((a) => a.id === request.id)) throw new ManagerError('CONFLICT', '此代理識別碼已存在。', 409);
    const report = await this.adapter.discover?.(this.piRoots()) ?? { runs: [], failures: [] };
    const run = dedupeRuns(report).find((candidate) => candidateId(candidate) === request.candidateId);
    if (!run) throw new ManagerError('NOT_FOUND', '找不到這個候選執行；請重新整理候選清單。', 404);
    if (!run.sessionId) throw new ManagerError('UNAVAILABLE', '這個管理執行尚無可綁定的 session；未加入管理。', 409);
    if (this.config.agents.some((a) => a.registryRoot === run.registryRoot && a.sessionId === run.sessionId)) throw new ManagerError('ALREADY_CONFIGURED', '這個執行已在管理清單中。', 409);
    if (!run.workspace) throw new ManagerError('UNAVAILABLE', '此候選沒有可驗證的工作目錄，未加入管理。', 409);
    const binding: AgentBinding = { id: request.id, name: request.name, role: request.role, projectId: request.projectId,
      registryRoot: run.registryRoot, sessionId: run.sessionId, runId: run.runId, workspace: run.workspace, summaryFile: null };
    // Reuse the same strict configuration validation as config.json; it also
    // enforces unique agent ids and identity roots before anything is added.
    const parsed = parseConfig({ version: 1, projects: this.config.projects, agents: [...this.config.agents, binding],
      refreshMs: this.config.refreshMs, ...(this.config.works ? { works: this.config.works } : {}) });
    const added = parsed.agents.at(-1);
    if (!added || added.id !== request.id) throw new ManagerError('INVALID_CONFIG', '候選資料驗證失敗，未加入管理。');
    this.config.agents.push(added);
    const observation = await this.adapter.observe(added).catch(() => unavailable());
    const view = this.view(added, observation);
    this.views.set(added.id, view);
    this.store.observation(added.id, hash(JSON.stringify([added.sessionId, 'registered'])), `${added.name}：已從候選加入管理`);
    return view;
  }
  private view(binding: AgentBinding, observation = unavailable()): AgentView {
    return { ...observation, id: binding.id, name: binding.name, role: binding.role, projectId: binding.projectId,
      workspace: binding.workspace, transport: binding.transport ?? 'pi', selectionRevision: selectionRevision(binding), ...summary(binding) };
  }
  overview(): Overview {
    const selected = new Set(this.config.agents.map((a) => a.id));
    return { version: 1, generatedAt: new Date().toISOString(), startedAt: this.startedAt, refreshMs: this.config.refreshMs,
      projects: this.config.projects, agents: this.config.agents.map((a) => this.views.get(a.id) || this.view(a)),
      events: this.store.events().filter((e) => selected.has(e.agentId)), operations: this.store.operations().filter((op) => selected.has(op.agentId)) };
  }
  async refresh(): Promise<void> {
    if (this.refreshing) return this.refreshing;
    this.refreshing = this.doRefresh().finally(() => { this.refreshing = null; });
    return this.refreshing;
  }
  private async doRefresh(): Promise<void> {
    await Promise.all(this.config.agents.map(async (binding) => {
      let observation;
      try { observation = await this.adapter.observe(binding); } catch { observation = unavailable(); }
      const prior = this.views.get(binding.id);
      if (observation.source === 'unavailable' && prior) observation = { ...observation, latestMessage: observation.latestMessage ?? prior.latestMessage, model: observation.model ?? prior.model,
        usage: observation.usage ?? prior.usage, lastProgressAt: observation.lastProgressAt ?? prior.lastProgressAt, heartbeatAt: observation.heartbeatAt ?? prior.heartbeatAt };
      const view = this.view(binding, observation);
      this.views.set(binding.id, view);
      const fingerprint = hash(JSON.stringify([view.instanceId, view.state, view.source, view.stale, view.reason, view.latestMessage?.id]));
      this.store.observation(binding.id, fingerprint, `${binding.name}：${labels[view.state]}`);
    }));
    await Promise.all(this.store.operations(100).filter((op) => this.config.agents.some((a) => a.id === op.agentId) && !['settled', 'failed'].includes(op.status))
      .map((op) => this.reconcile(op).catch(() => op)));
    await this.refreshOwnerInbox();
  }
  async refreshOwnerInbox(): Promise<void> { this.ownerInbox.refresh(this.overview().agents, await this.works.list()); }
  async start(): Promise<void> { await this.refresh(); this.timer = setInterval(() => { void this.refresh().catch(() => {}); }, this.config.refreshMs); }
  async stop(): Promise<void> { clearInterval(this.timer); await this.continuation.stop(); await this.works.stop(); if (this.refreshing) await this.refreshing; await Promise.allSettled(this.checking.values()); }
  async conversation(id: string, after?: string): Promise<ConversationView> {
    const binding = this.binding(id), result = await this.adapter.conversation(binding, after);
    return { ...result, agentId: id, selectionRevision: selectionRevision(binding) };
  }
  async send(id: string, request: SendRequest): Promise<OperationView> {
    const binding = this.binding(id);
    const existing = this.store.existing(id, request);
    if (existing) return existing; // Never replay a duplicate, including an uncertain intent.
    await this.validateSend(id, request);
    const concurrent = this.store.existing(id, request);
    if (concurrent) return concurrent;
    const operation = this.store.begin(binding, request);
    try {
      const receipt = await this.adapter.send(binding, request);
      if (receipt.id !== operation.id || receipt.sessionId !== binding.sessionId || receipt.instanceId !== operation.instanceId) return this.store.update(operation.id, 'unknown', notices.unknown);
      return this.store.update(operation.id, receipt.status, notices[receipt.status]);
    } catch (error) {
      const definite = error instanceof ManagerError && ['UNAVAILABLE', 'INSTANCE_CHANGED', 'WAITING_USER'].includes(error.code);
      return this.store.update(operation.id, definite ? 'failed' : 'unknown', definite ? error.message : notices.unknown);
    }
  }
  async validateSend(id: string, request: SendRequest): Promise<void> {
    const binding = this.binding(id);
    this.adapter.validateMessage?.(request);
    if (request.selectionRevision !== selectionRevision(binding)) throw new ManagerError('STALE_SELECTION', '代理綁定已變更，請更新畫面後再傳送。', 409);
    const state = await this.adapter.observe(binding);
    if (state.source !== 'live' || !state.capabilities.send) throw new ManagerError('UNAVAILABLE', '代理目前無法接收訊息，尚未傳送。', 503);
    if (state.instanceId !== request.instanceId) throw new ManagerError('INSTANCE_CHANGED', '代理已更換執行實例，請先更新畫面。', 409);
    if (state.state === 'waiting_user') throw new ManagerError('WAITING_USER', '請先處理代理原介面上的結構化選項，這次尚未傳送。', 409);
  }
  async operation(id: string): Promise<OperationView> {
    const operation = this.store.operation(id);
    if (!operation) throw new ManagerError('NOT_FOUND', '尚未找到這次傳送的紀錄；請保留原編號。', 404);
    this.binding(operation.agentId);
    return this.reconcile(operation);
  }
  private async reconcile(operation: OperationView): Promise<OperationView> {
    if (['settled', 'failed'].includes(operation.status)) return operation;
    const checking = this.checking.get(operation.id); if (checking) return checking;
    const promise = (async () => {
      // Receipt identity belongs to the immutable original binding, even if the
      // operator later assigns the display agent to a new Pi session.
      const target = this.store.target(operation.id);
      if (!target) return operation;
      const r = await this.adapter.receipt(target, operation);
      if (!r || r.id !== operation.id || r.sessionId !== target.sessionId || r.instanceId !== operation.instanceId) return this.store.operation(operation.id) || operation;
      return this.store.update(operation.id, r.status, notices[r.status]);
    })().finally(() => { this.checking.delete(operation.id); });
    this.checking.set(operation.id, promise); return promise;
  }
}
