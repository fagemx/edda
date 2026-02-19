import { execSync } from 'node:child_process';
import { openSync, writeSync, closeSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';

// ── edda availability check ──

export function checkEddaAvailable(): boolean {
  try {
    execSync('edda --help', { encoding: 'utf-8', timeout: 3000, stdio: 'pipe' });
    return true;
  } catch {
    return false;
  }
}

// ── First-failure-fatal hook wrapper ──

let hookHealthy: boolean | null = null;

/** Reset hook health state (for testing) */
export function resetHookHealth(): void {
  hookHealthy = null;
}

/**
 * Create a edda hook handler for the given hook event name.
 * Uses first-failure-fatal pattern: if the first call fails, all subsequent calls are no-ops.
 */
export function eddaHook(hookName: string) {
  return async (input: unknown): Promise<Record<string, unknown>> => {
    // First-failure-fatal: once broken, stay broken
    if (hookHealthy === false) return {};

    try {
      const stdout = execSync('edda hook claude', {
        input: JSON.stringify({
          ...(input as object),
          hook_event_name: hookName,
        }),
        encoding: 'utf-8',
        timeout: 5000,
        stdio: ['pipe', 'pipe', 'pipe'],
      });

      hookHealthy = true;

      if (stdout?.trim()) {
        return JSON.parse(stdout) as Record<string, unknown>;
      }
      return {};
    } catch (err) {
      if (hookHealthy === null) {
        // First call failed — fatal, disable hooks
        hookHealthy = false;
        console.warn(
          `[orchestrate] edda hook ${hookName} failed on first call. Hooks disabled.`,
        );
        console.warn(
          '[orchestrate] Agent will run without edda context injection.',
        );
        console.warn(
          `[orchestrate] Error: ${err instanceof Error ? err.message : String(err)}`,
        );
      } else {
        // Previously succeeded — non-fatal warning
        console.warn(
          `[orchestrate] edda hook ${hookName} failed (non-fatal):`,
          err instanceof Error ? err.message : String(err),
        );
      }
      return {};
    }
  };
}

// ── Hook config builder ──

/**
 * Agent SDK HookConfig type (minimal definition).
 * Full type comes from @anthropic-ai/claude-agent-sdk — not yet installed.
 */
export interface HookEntry {
  matcher?: string;
  hooks: Array<(input: unknown) => Promise<Record<string, unknown>>>;
}

export type HookConfig = Record<string, HookEntry[]>;

/** Build edda hook config for Agent SDK */
export function buildEddaHooks(): HookConfig {
  return {
    SessionStart: [{ hooks: [eddaHook('SessionStart')] }],
    PreCompact: [{ hooks: [eddaHook('PreCompact')] }],
    UserPromptSubmit: [{ hooks: [eddaHook('UserPromptSubmit')] }],
    PreToolUse: [{
      matcher: 'Edit|Write|Bash',
      hooks: [eddaHook('PreToolUse')],
    }],
    PostToolUse: [{ hooks: [eddaHook('PostToolUse')] }],
    PostToolUseFailure: [{ hooks: [eddaHook('PostToolUseFailure')] }],
    SessionEnd: [{ hooks: [eddaHook('SessionEnd')] }],
  };
}

// ── Session ID ──

/** Generate a deterministic session ID for a phase run */
export function phaseSessionId(planName: string, phaseId: string, attempt: number): string {
  return `orchestrate-${planName}-${phaseId}-${attempt}`;
}

// ── TranscriptWriter (buffered JSONL) ──

const FLUSH_THRESHOLD = 50;
const FLUSH_INTERVAL_MS = 2000;

export class TranscriptWriter {
  private buffer: string[] = [];
  private timer: ReturnType<typeof setTimeout> | null = null;
  private fd: number;

  readonly path: string;

  constructor(filePath: string) {
    this.path = filePath;
    mkdirSync(dirname(filePath), { recursive: true });
    this.fd = openSync(filePath, 'a');
  }

  /** Append a message to the buffer. Auto-flushes at threshold or timer. */
  append(msg: unknown): void {
    this.buffer.push(JSON.stringify(msg));
    if (this.buffer.length >= FLUSH_THRESHOLD) {
      this.flush();
    } else if (!this.timer) {
      this.timer = setTimeout(() => this.flush(), FLUSH_INTERVAL_MS);
    }
  }

  /** Flush all buffered lines to disk */
  flush(): void {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (this.buffer.length === 0) return;
    const chunk = this.buffer.join('\n') + '\n';
    this.buffer = [];
    writeSync(this.fd, chunk);
  }

  /** Close the writer — flush remaining buffer + close fd */
  close(): void {
    this.flush();
    closeSync(this.fd);
  }
}
