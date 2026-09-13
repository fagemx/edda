import type { OperationStatus, SendRequest } from './contracts.js';
import type { ContinuityReference } from './continuation-contracts.js';

export interface WorkBinding { id: string; projectId: string; taskId: number; workspace: string; ownerAgentId: string }
export type WorkStage = 'uninitialized' | 'ready' | 'assigned' | 'executing' | 'awaiting_delivery' | 'delivered' | 'accepted' | 'blocked';
export interface WorkInstruction { id: string; message: string; operationId: string; acknowledgedAt: string | null; evidence: string | null }
export interface WorkHistory { id: string; kind: string; at: string; summary: string }
export interface WorkSessionBinding {
  id: string; agentId: string; sessionId: string; selectionRevision: string; transport: 'pi' | 'codex';
  role: 'manager' | 'worker' | 'reviewer'; parentAgentId: string | null; reviewedSha: string | null;
  expectedEvent: string; nextExpectedAt: string | null; boundAt: string; unboundAt: string | null;
}
export interface WorkView {
  id: string; projectId: string; taskId: number; title: string; taskStatus: string; taskReceipt: string | null;
  ownerAgentId: string; assigneeAgentId: string | null; nextStep: string; stage: WorkStage; revision: string;
  evidence: string | null; waitingReason: string | null; pendingInstruction: WorkInstruction | null;
  deliveryOperationId: string | null; deliveryStatus: OperationStatus | null;
  updatedAt: string | null; error: string | null; history: WorkHistory[];
  lastActionId: string | null; confirmedActionId: string | null;
  sessions: WorkSessionBinding[];
  continuity?: ContinuityReference;
}
export interface WorksView { works: WorkView[]; generatedAt: string }
type ActionBase = { actionId: string; revision: string };
export type WorkAction = ActionBase & (
  | { kind: 'initialize'; nextStep: string }
  | { kind: 'assign'; agentId: string; nextStep: string; send: SendRequest }
  | { kind: 'intervene'; send: SendRequest }
  | { kind: 'acknowledge'; instructionId: string; evidence: string }
  | { kind: 'deliver'; evidence: string; nextStep: string }
  | { kind: 'accept'; evidence: string }
  | { kind: 'block'; reason: string; nextStep: string }
  | { kind: 'bind_session'; agentId: string; role: 'manager' | 'worker' | 'reviewer'; parentAgentId: string | null; reviewedSha: string | null; expectedEvent: string; nextExpectedAt: string | null }
  | { kind: 'unbind_session'; bindingId: string }
  | { kind: 'handoff_owner'; ownerAgentId: string; evidence: string }
  | { kind: 'attach_continuity'; reference: ContinuityReference }
);
