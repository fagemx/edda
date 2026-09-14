import type { OperationStatus, SendRequest } from './contracts.js';
import type { ContinuityReference } from './continuation-contracts.js';

export interface WorkBinding { id: string; projectId: string; taskId: number; workspace: string; ownerAgentId: string; ownerRef?: string | null; ownerRoot?: string | null }
export type WorkStage = 'uninitialized' | 'ready' | 'assigned' | 'executing' | 'awaiting_delivery' | 'delivered' | 'accepted' | 'blocked';
// Native, operator-facing phase (GH1181). Derived from the task rail, delivery
// receipts, observed sessions and owner-inbox events. It is shown before the
// manual `stage` and never lets `task running` imply that a child is working.
// `completed`/`failed` are native terminal facts; `recoverable` means the native
// record for the pending session could not be read but its identity is preserved;
// `launched` means the delivery receipt was accepted without an observed start.
export type WorkPhase = 'uninitialized' | 'ready' | 'assigned' | 'launched' | 'working' | 'waiting' | 'interrupted'
  | 'recoverable' | 'delivered' | 'accepted' | 'blocked' | 'completed' | 'failed';
// Wait target. `'none'` means the work is provably not waiting on another party
// (it is working, or it reached a terminal phase). `null` means this projection
// will not name a target, and `waitEvidence` says which case it is: the session
// relation is unlinked/unavailable, or the recorded stage leaves the next actor
// to the operator (uninitialized/ready, a delivery with no reviewer bound). A
// role is never guessed. Native terminal facts name their own target: `task
// failed` is `'none'`, `delivery failed` is `'user_decision'`.
// A live tool call is `working` with its process liveness shown as a separate
// field, so there is no `'tool'` wait target (GH1189 F2).
// `'worker'` is the executing party, which a manager-role controller can also be
// when it executes the work directly; `'verifier'` is the reviewing party.
export type WorkWaitingFor = 'worker' | 'verifier' | 'dependency' | 'user_decision' | 'none' | null;
// The bounded relation between this work's executor source and the registry
// roots this project already knows. It is never a registry path.
export type WorkRootRelation = 'in_root' | 'not_in_root' | 'root_not_registered' | 'unknown';
export interface WorkRegistryRelation { relation: WorkRootRelation; message: string }
// Owner-bound `edda return` facts for one work, read through the fixed-argument
// CLI. `matched` is the bounded, newest-first subset of pending returns whose
// `work` matches this work's task id or work id.
export interface OwnerReturnFact { id: string; work: string; status: 'done' | 'failed'; result: string | null; postedAt: string }
// `dropped` counts pending items that could not be used and may belong to this
// work — matched-but-unusable facts, matched facts beyond the display bound, and
// pending items beyond the scan bound — so the card never reports a healthy count
// while hiding one.
export interface OwnerReturnRead { owner: string; holder: string | null; pending: number; total: number | null; matched: OwnerReturnFact[]; dropped: number; error: string | null }
// Which mailbox root the read resolved to and whether that root actually held a
// mailbox layout. `label` is an opaque hash, never a path.
export type OwnerMailboxKind = 'binding' | 'env' | 'managed' | 'workspace';
export interface OwnerReturnMailbox { kind: OwnerMailboxKind; label: string; present: boolean }
// The projected read plus the mailbox it came from and a bounded human notice.
// A missing layout at the highest-precedence root is honest unavailability, never
// a healthy zero: `present: false` pairs with a non-null `error`.
export interface OwnerReturnView extends OwnerReturnRead { mailbox: OwnerReturnMailbox; notice: string | null }
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
  // The current attempt: how many hand-off operations (`assign`/`intervene` sends)
  // this work's recorded chain contains. 0 before the first hand-off, so the
  // operator reads "第 N 次嘗試" without needing a run or operation id.
  attempt: number;
  phase: WorkPhase; waitingFor: WorkWaitingFor; waitEvidence: string | null;
  ownerReturn: OwnerReturnView | null; registry: WorkRegistryRelation;
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
