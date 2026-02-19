import { execSync } from 'node:child_process';
import { appendFileSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { formatDuration } from '../lib/utils/format.js';

// ── Event types ──

export type OrchestratorEvent =
  | {
      type: 'plan:start';
      plan_name: string;
      phase_count: number;
      plan_file: string;
      seq: number;
      ts: string;
    }
  | {
      type: 'phase:passed';
      plan_name: string;
      phase_id: string;
      duration_ms: number;
      attempts: number;
      cost_usd?: number;
      seq: number;
      ts: string;
    }
  | {
      type: 'phase:failed';
      plan_name: string;
      phase_id: string;
      duration_ms: number;
      attempts: number;
      cost_usd?: number;
      error_type: string;
      seq: number;
      ts: string;
    }
  | {
      type: 'phase:skipped';
      plan_name: string;
      phase_id: string;
      skip_reason: string;
      seq: number;
      ts: string;
    }
  | {
      type: 'plan:completed';
      plan_name: string;
      duration_ms: number;
      phases_passed: number;
      phases_skipped: number;
      total_attempts: number;
      total_cost_usd?: number;
      seq: number;
      ts: string;
    }
  | {
      type: 'plan:aborted';
      plan_name: string;
      phases_passed: number;
      phases_pending: number;
      seq: number;
      ts: string;
    };

// ── Event writer ──

export interface EventWriterOptions {
  eddaEnabled: boolean;
  jsonLogPath: string;
  cwd?: string;
}

let eventSequence = 0;

/** Reset sequence counter (for testing) */
export function resetEventSequence(): void {
  eventSequence = 0;
}

/** Distributive Omit for union types */
type DistributiveOmit<T, K extends keyof T> = T extends unknown ? Omit<T, K> : never;

export type OrchestratorEventInput = DistributiveOmit<OrchestratorEvent, 'seq' | 'ts'>;

/** Record an event with dual-write: JSONL (primary) + edda note (secondary) */
export function recordEvent(options: EventWriterOptions, event: OrchestratorEventInput): void {
  const fullEvent = {
    ...event,
    seq: eventSequence++,
    ts: new Date().toISOString(),
  } as OrchestratorEvent;

  // Primary: ALWAYS append to JSONL (local, reliable)
  mkdirSync(dirname(options.jsonLogPath), { recursive: true });
  appendFileSync(options.jsonLogPath, JSON.stringify(fullEvent) + '\n');

  // Secondary: edda note (best-effort, fire-and-forget + 1 retry)
  if (options.eddaEnabled) {
    void eddaNote(fullEvent, options.cwd);
  }
}

// ── edda note (secondary writer) ──

async function eddaNote(event: OrchestratorEvent, cwd?: string): Promise<void> {
  const message = formatEventMessage(event);
  const tags = formatEventTags(event);

  for (let attempt = 0; attempt < 2; attempt++) {
    try {
      const tagFlags = tags.map((t) => `--tag "${t}"`).join(' ');
      execSync(`edda note "${message}" ${tagFlags}`, {
        encoding: 'utf-8',
        timeout: 5000,
        cwd: cwd ?? process.cwd(),
        stdio: 'pipe',
      });
      return;
    } catch {
      if (attempt === 0) {
        await new Promise((r) => setTimeout(r, 500));
      } else {
        console.error('[orchestrate] Failed to record event to edda after retry');
      }
    }
  }
}

// ── Message formatting ──

export function formatEventMessage(event: OrchestratorEvent): string {
  switch (event.type) {
    case 'plan:start':
      return `plan:start ${event.plan_name} (${event.phase_count} phases)`;

    case 'phase:passed':
      return `phase:passed ${event.plan_name}/${event.phase_id} (${formatDuration(event.duration_ms)}, attempt ${event.attempts})`;

    case 'phase:failed':
      return `phase:failed ${event.plan_name}/${event.phase_id} — ${event.error_type}`;

    case 'phase:skipped':
      return `phase:skipped ${event.plan_name}/${event.phase_id} — ${event.skip_reason}`;

    case 'plan:completed': {
      const parts = [`${event.phases_passed} passed`];
      if (event.phases_skipped > 0) parts.push(`${event.phases_skipped} skipped`);
      return `plan:completed ${event.plan_name} (${formatDuration(event.duration_ms)}, ${parts.join(', ')})`;
    }

    case 'plan:aborted':
      return `plan:aborted ${event.plan_name} (${event.phases_passed} passed, ${event.phases_pending} pending)`;
  }
}

// ── Tag formatting ──

export function formatEventTags(event: OrchestratorEvent): string[] {
  const tags = ['orchestrator', `plan:${event.plan_name}`];

  if ('phase_id' in event) {
    tags.push(`phase:${event.phase_id}`);
  }

  return tags;
}
