import type { WorkView } from './workflow-contracts.js';

/** Native continuity wire shapes. Edda owns validation, identity and storage. */
export interface NativeCapsuleInput {
  capsule_version: 1;
  state: { title?: string; summary?: string; goal?: string; current?: string; hypotheses?: string[]; rejected?: Array<{ hypothesis: string; reason: string }>; open_questions?: string[]; next_action: string };
  references?: { task_ids?: string[]; event_ids?: string[] };
}
export interface NativeCapsule {
  capsule_version: 1; capsule_id: string; created_at: string;
  source: { machine_alias?: string; actor?: string };
  repository: { portable_repo_id?: string; display_hint?: string; local_only_reason?: string };
  state: Required<NativeCapsuleInput['state']>;
  git: { branch?: string; head_sha?: string; detached?: boolean; tree_dirty?: boolean; dirty_paths?: string[]; dirty_paths_truncated: boolean };
  references: { task_ids?: string[]; event_ids?: string[] };
  truncation?: Array<{ field: string; omitted_chars: number; omitted_items: number }>;
}
export interface NativeRestore {
  data_authority: 'data_only'; local_event_id: string; origin_event_id: string;
  imported: boolean; legacy_partial: boolean; capsule: NativeCapsule; warnings: string[];
}
export interface PortableBundle {
  bundle_version: 1; portable_repo_id: string; origin_capsule_id: string; origin_event_id: string;
  capsule_sha256: string; capsule_bytes_hex: string; bundle_sha256: string; data_authority: 'data_only';
}
export interface ContinuityReference { capsuleId: string; localEventId: string; originEventId: string }
export interface ContinuationOperation { actionId: string; status: 'unknown' | 'saved' | 'attached' | 'failed'; capsuleId: string | null; notice: string; nativeStatus?: string }
export interface ContinuationPublication { operation: ContinuationOperation | null; context: NativeRestore | null; bundle: PortableBundle | null; work: WorkView }
export interface ContinuationPublishRequest { actionId: string; revision: string; input: NativeCapsuleInput }
export interface ContinuationImportRequest { actionId: string; revision: string; bundle: PortableBundle }
export interface ContinuationTakeoverRequest { actionId: string; revision: string; capsuleId: string; ownerAgentId: string; environmentEvidence: string; releaseEvidence: string }
export interface ContinuationRecoverRequest { actionId: string; capsuleId?: string }
export const MAX_CONTINUATION_INPUT_BYTES = 8192;
export const MAX_CONTINUITY_BUNDLE_BYTES = 256 * 1024 * 2 + 16 * 1024;
