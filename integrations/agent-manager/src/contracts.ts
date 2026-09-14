export const MAX_MESSAGE_BYTES = 12 * 1024;
// JSON escaping can expand each control character to six ASCII bytes.
export const MAX_BODY_BYTES = 96 * 1024;
export type RuntimeState = 'running' | 'executing_tool' | 'idle' | 'waiting_user' | 'stopped' | 'unavailable' | 'unknown';
export type MessageMode = 'followUp' | 'steer';
export type OperationStatus = 'prepared' | 'unconfirmed' | 'accepted' | 'queued' | 'started' | 'settled' | 'failed' | 'unknown';
export interface ResourceView { id: string; name: string; kind: string; details: string; owner: string; source: string }
export interface ProjectView { id: string; name: string; priority: number; resources: ResourceView[] }
export interface AgentBinding {
  id: string; name: string; projectId: string; role: 'manager' | 'worker';
  registryRoot: string; sessionId: string; runId: string | null; workspace: string;
  summaryFile: string | null;
  transport?: 'pi' | 'codex'; transcriptFile?: string;
}
export interface ManagerConfig { version: 1; projects: ProjectView[]; agents: AgentBinding[]; refreshMs: number; continuityExecutable?: string; works?: import('./workflow-contracts.js').WorkBinding[] }
export interface ModelView { provider: string; id: string }
export interface UsageView { tokens: number | null; reportedCost: number | null }
export interface PublicEntry {
  id: string; timestamp: string | null; kind: 'message' | 'tool_result';
  role: 'user' | 'assistant' | null; text: string; truncated: boolean;
  toolName: string | null; toolError: boolean;
}
// A bounded, honest projection of a native record the Pi read could not use.
// `recovery` names the strongest available continuation point (the bounded
// `nextAction` the Pi read returns), or null when the source cannot name one.
// It carries no record bytes and preserves the identity from the run config.
export interface RecordDegradation { code: string; record: string; message: string; recovery: string | null }
export interface AgentObservation {
  state: RuntimeState; instanceId: string | null; observedAt: string;
  heartbeatAt: string | null; lastProgressAt: string | null; lastEvent: string | null;
  source: 'live' | 'recorded' | 'unavailable'; stale: boolean; reason: string | null;
  degraded: RecordDegradation | null;
  model: ModelView | null; usage: UsageView | null;
  capabilities: { conversation: boolean; send: boolean };
  latestMessage: PublicEntry | null;
  sessionEvidence?: import('./session-contracts.js').SessionEvidence;
}
export interface AgentView extends AgentObservation {
  id: string; name: string; role: 'manager' | 'worker'; projectId: string;
  workspace: string; transport: 'pi' | 'codex'; selectionRevision: string;
  summary: string | null; summaryUpdatedAt: string | null; summaryError: string | null;
}
export interface ConversationView {
  agentId: string; selectionRevision: string; instanceId: string | null;
  entries: PublicEntry[]; cursor: string | null; headCursor: string | null;
  hasMore: boolean; observedAt: string; source: 'live' | 'recorded' | 'unavailable';
}
export interface SendRequest {
  operationId: string; selectionRevision: string; instanceId: string;
  basisCursor: string | null; mode: MessageMode; message: string;
}
export interface OperationView {
  id: string; agentId: string; instanceId: string; basisCursor: string | null;
  mode: MessageMode; message: string; status: OperationStatus;
  createdAt: string; updatedAt: string; notice: string;
}
export interface ManagerEvent { id: number; at: string; agentId: string; kind: 'observation' | 'message'; summary: string; operationId: string | null }
export interface Overview {
  version: 1; generatedAt: string; startedAt: string; refreshMs: number;
  projects: ProjectView[]; agents: AgentView[]; events: ManagerEvent[]; operations: OperationView[];
}
// A run projected from the bounded Pi session/channel listing. registryRoot is
// internal to the manager and must never be serialized to the browser.
export interface DiscoveredRun {
  registryRoot: string; sessionId: string | null; runId: string | null; instanceId: string | null;
  state: RuntimeState; live: boolean; source: 'live' | 'recorded';
  workspace: string | null; lastProgressAt: string | null; reason: string | null;
  degraded?: RecordDegradation | null;
}
export interface DiscoveryReport { runs: DiscoveredRun[]; failures: Array<{ registryRoot: string; message: string }> }
// Public candidate projection. Deliberately omits registryRoot; `id` is a stable
// opaque handle the operator can register without seeing any private path.
export interface CandidateView {
  id: string; sessionId: string | null; runId: string | null; instanceId: string | null; state: RuntimeState; live: boolean;
  source: 'live' | 'recorded'; workspace: string | null; lastProgressAt: string | null;
  reason: string | null; degraded: RecordDegradation | null; configuredAgentId: string | null;
}
export interface CandidateListView { candidates: CandidateView[]; issues: Array<{ label: string; message: string }>; generatedAt: string }
export interface RegisterCandidateRequest { candidateId: string; id: string; name: string; role: 'manager' | 'worker'; projectId: string }
export interface AdapterReceipt { status: OperationStatus; instanceId: string; sessionId: string; id: string }
export interface PiAdapter {
  validateMessage?(request: SendRequest): void;
  defaultRegistryRoot?(): string | null;
  discover?(registryRoots: string[]): Promise<DiscoveryReport>;
  observe(binding: AgentBinding): Promise<AgentObservation>;
  conversation(binding: AgentBinding, after?: string): Promise<Omit<ConversationView, 'agentId' | 'selectionRevision'>>;
  send(binding: AgentBinding, request: SendRequest): Promise<AdapterReceipt>;
  receipt(binding: AgentBinding, operation: OperationView): Promise<AdapterReceipt | null>;
}
export class ManagerError extends Error {
  constructor(public code: string, message: string, public status = 400) { super(message); }
}
