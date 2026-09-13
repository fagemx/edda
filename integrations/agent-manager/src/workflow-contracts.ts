import type { OperationStatus, SendRequest } from './contracts.js';
import type { ContinuityReference } from './continuation-contracts.js';

export interface WorkBinding { id: string; projectId: string; taskId: number; workspace: string; ownerAgentId: string }
export type WorkStage = 'uninitialized' | 'ready' | 'assigned' | 'executing' | 'awaiting_delivery' | 'delivered' | 'accepted' | 'blocked';
// Native, operator-facing phase (GH1181). Derived from the task rail, delivery
// receipts, observed sessions and owner-inbox events. It is shown before the
// manual `stage` and never lets `task running` imply that a child is working.
export type WorkPhase = 'uninitialized' | 'ready' | 'assigned' | 'working' | 'waiting' | 'interrupted' | 'delivered' | 'accepted' | 'blocked' | 'failed';
// Wait target. `'none'` means the work is provably not waiting on another party
// (it is working, or it reached a terminal phase); `null` means no target can be
// named from native evidence (unlinked / unavailable) — never a guessed role.
// `'worker'` is the executing party, which a manager-role controller can also be
// when it executes the work directly; `'verifier'` is the reviewing party.
export type WorkWaitingFor = 'worker' | 'verifier' | 'dependency' | 'user_decision' | 'tool' | 'none' | null;
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
  phase: WorkPhase; waitingFor: WorkWaitingFor; waitEvidence: string | null;
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
