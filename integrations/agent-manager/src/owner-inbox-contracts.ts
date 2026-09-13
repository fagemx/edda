import type { WorkView } from './workflow-contracts.js';

export type OwnerInboxKind = 'reply_ended' | 'interrupted' | 'provider_error' | 'unavailable' | 'overdue';
export interface OwnerInboxEvent {
  id: string; workId: string; projectId: string; taskId: number; bindingId: string; agentId: string; sessionId: string;
  nativeEventId: string; kind: OwnerInboxKind; at: string; summary: string;
  category: string | null; httpStatus: number | null; deliveryRecorded: boolean;
  acknowledgedAt: string | null; acknowledgementId: string | null; evidence: string | null;
  ownerAgentId: string;
}
export interface OwnerInboxView { events: OwnerInboxEvent[]; generatedAt: string; truncated: boolean }
export interface OwnerInboxAck { eventId: string; actionId: string; evidence: string }
export interface OwnerWorkContext {
  work: WorkView; summaryStale: boolean; latestChildEventAt: string | null;
  alerts: OwnerInboxEvent[];
}
export interface OwnerContext { ownerAgentId: string; works: OwnerWorkContext[]; generatedAt: string; truncated: boolean }
export interface BindingObservationState { unavailable: boolean; transition: number; latestChildEventAt: string | null }
