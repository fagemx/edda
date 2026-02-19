import { execSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { ErrorInfo, CheckResult } from '../plan/state.js';

// ── Check output ──

export interface CheckOutput {
  status: 'passed' | 'failed';
  detail: string | null;
  duration_ms: number;
}

export interface CheckRunResult {
  allPassed: boolean;
  results: CheckResult[];
  error: ErrorInfo | null;
}

// ── Secret masking ──

export function maskSecrets(text: string): string {
  return text
    .replace(/(?:sk-|pk-|token_)[a-zA-Z0-9]{20,}/g, '***')
    .replace(/(?:Bearer|Basic)\s+\S{20,}/g, 'Bearer ***')
    .replace(/(?:password|secret|key|token)=\S+/gi, (m) => m.split('=')[0] + '=***');
}

// ── Retryable judgment ──

export function isRetryable(specType: string, detail: string | null): boolean {
  if (specType === 'cmd_succeeds' && detail?.includes('timed out')) {
    return true;
  }
  if (specType === 'wait_until') {
    return true;
  }
  return false;
}

// ── Individual check executors ──

export async function checkFileExists(
  spec: { path: string },
  cwd: string,
): Promise<CheckOutput> {
  const fullPath = resolve(cwd, spec.path);
  const exists = existsSync(fullPath);
  return {
    status: exists ? 'passed' : 'failed',
    detail: exists ? null : `file not found: ${spec.path}`,
    duration_ms: 0,
  };
}

export async function checkCmdSucceeds(
  spec: { cmd: string; timeout_sec?: number },
  cwd: string,
): Promise<CheckOutput> {
  const timeout = (spec.timeout_sec ?? 120) * 1000;
  const start = Date.now();

  try {
    execSync(spec.cmd, {
      cwd,
      timeout,
      encoding: 'utf-8',
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    return {
      status: 'passed',
      detail: null,
      duration_ms: Date.now() - start,
    };
  } catch (err: unknown) {
    const duration_ms = Date.now() - start;
    const execErr = err as { killed?: boolean; signal?: string; status?: number; stderr?: string; code?: string };

    // Timeout detection: killed flag, ETIMEDOUT code, or signal
    const isTimeout = execErr.killed || execErr.code === 'ETIMEDOUT' || execErr.signal === 'SIGTERM';
    if (isTimeout) {
      return {
        status: 'failed',
        detail: `command timed out after ${spec.timeout_sec ?? 120}s: ${spec.cmd}`,
        duration_ms,
      };
    }

    const stderr = maskSecrets((execErr.stderr ?? '').toString().slice(0, 500));
    return {
      status: 'failed',
      detail: `exit ${execErr.status ?? '?'}: ${stderr}`.trim(),
      duration_ms,
    };
  }
}

export async function checkFileContains(
  spec: { path: string; pattern: string },
  cwd: string,
): Promise<CheckOutput> {
  const fullPath = resolve(cwd, spec.path);
  if (!existsSync(fullPath)) {
    return {
      status: 'failed',
      detail: `file not found: ${spec.path}`,
      duration_ms: 0,
    };
  }
  const content = readFileSync(fullPath, 'utf-8');
  const found = content.includes(spec.pattern);
  return {
    status: found ? 'passed' : 'failed',
    detail: found ? null : `pattern not found in ${spec.path}: "${spec.pattern}"`,
    duration_ms: 0,
  };
}

export async function checkGitClean(
  spec: { allow_untracked?: boolean },
  cwd: string,
): Promise<CheckOutput> {
  const start = Date.now();
  try {
    const output = execSync('git status --porcelain', {
      cwd,
      encoding: 'utf-8',
      stdio: ['pipe', 'pipe', 'pipe'],
    });
    const lines = output.trim().split('\n').filter(Boolean);
    const dirty = spec.allow_untracked
      ? lines.filter((l) => !l.startsWith('??'))
      : lines;

    return {
      status: dirty.length === 0 ? 'passed' : 'failed',
      detail: dirty.length === 0 ? null : `${dirty.length} dirty files`,
      duration_ms: Date.now() - start,
    };
  } catch {
    return {
      status: 'failed',
      detail: 'git status failed (not a git repo?)',
      duration_ms: 0,
    };
  }
}

export async function checkEddaEvent(
  spec: { event_type: string; after?: string },
  cwd: string,
  phaseStartedAt?: string,
): Promise<CheckOutput> {
  const start = Date.now();
  const after = spec.after === '$phase_start' ? phaseStartedAt : spec.after;
  try {
    const afterFlag = after ? ` --after "${after}"` : '';
    const cmd = `edda log --json --type ${spec.event_type}${afterFlag} --limit 1`;

    const output = execSync(cmd, { cwd, encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
    const events = JSON.parse(output);
    return {
      status: Array.isArray(events) && events.length > 0 ? 'passed' : 'failed',
      detail: Array.isArray(events) && events.length > 0 ? null : `no "${spec.event_type}" event found`,
      duration_ms: Date.now() - start,
    };
  } catch (err: unknown) {
    return {
      status: 'failed',
      detail: `edda log failed: ${err instanceof Error ? err.message : String(err)}`,
      duration_ms: 0,
    };
  }
}

// ── Check type registry ──

type CheckFn = (spec: Record<string, unknown>, cwd: string, phaseStartedAt?: string) => Promise<CheckOutput>;

// Lazy import to avoid circular dependency
async function checkWaitUntilWrapper(spec: Record<string, unknown>, cwd: string): Promise<CheckOutput> {
  const { checkWaitUntil } = await import('./wait-until.js');
  return checkWaitUntil(spec as unknown as Parameters<typeof checkWaitUntil>[0], cwd);
}

const CHECK_REGISTRY: Record<string, CheckFn> = {
  file_exists: checkFileExists as CheckFn,
  cmd_succeeds: checkCmdSucceeds as CheckFn,
  file_contains: checkFileContains as CheckFn,
  git_clean: checkGitClean as CheckFn,
  edda_event: checkEddaEvent as CheckFn,
  wait_until: checkWaitUntilWrapper as CheckFn,
};

export async function runCheck(
  spec: Record<string, unknown>,
  cwd: string,
  phaseStartedAt?: string,
): Promise<CheckOutput> {
  const type = spec.type as string;
  const fn = CHECK_REGISTRY[type];
  if (!fn) {
    return {
      status: 'failed',
      detail: `unknown check type: ${type}`,
      duration_ms: 0,
    };
  }
  return fn(spec, cwd, phaseStartedAt);
}

// ── Batch runner (short-circuit on first failure) ──

export async function runChecks(
  checks: Record<string, unknown>[],
  cwd: string,
  phaseStartedAt?: string,
): Promise<CheckRunResult> {
  const results: CheckResult[] = [];

  for (let i = 0; i < checks.length; i++) {
    const spec = checks[i]!;
    const type = (spec.type as string) ?? 'unknown';
    const output = await runCheck(spec, cwd, phaseStartedAt);

    results.push({
      type,
      status: output.status,
      detail: output.detail,
    });

    if (output.status === 'failed') {
      // Short-circuit: mark remaining checks as waiting
      for (let j = i + 1; j < checks.length; j++) {
        results.push({
          type: (checks[j]!.type as string) ?? 'unknown',
          status: 'waiting',
          detail: null,
        });
      }
      return {
        allPassed: false,
        results,
        error: {
          type: 'check_failed',
          message: output.detail ?? `check "${type}" failed`,
          retryable: isRetryable(type, output.detail),
          check_index: i,
          timestamp: new Date().toISOString(),
        },
      };
    }
  }

  return { allPassed: true, results, error: null };
}
