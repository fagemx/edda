export type NativeSessionEventKind = 'started' | 'reply_ended' | 'interrupted' | 'provider_error' | 'child_reference';
export interface NativeSessionEvent {
  id: string; kind: NativeSessionEventKind; at: string; turnId: string | null;
  category: string | null; httpStatus: number | null;
  childSessionId?: string;
}
export interface SessionEvidence {
  sessionId: string; evidenceSource: 'live' | 'recorded' | 'unavailable';
  historyComplete: boolean; events: NativeSessionEvent[];
}
