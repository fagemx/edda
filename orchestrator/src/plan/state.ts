import { writeFileSync, readFileSync, renameSync, mkdirSync, existsSync, readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import type { Plan } from './types.js';

// ── Types ──

export type PhaseStatus = 'pending' | 'running' | 'checking' | 'passed' | 'failed' | 'skipped' | 'stale';
export type PlanStatus = 'pending' | 'running' | 'blocked' | 'completed' | 'aborted';
export type CheckStatus = 'waiting' | 'running' | 'passed' | 'failed';
export type ErrorType = 'agent_crash' | 'check_failed' | 'timeout' | 'user_abort';

export interface ErrorInfo {
  type: ErrorType;
  message: string;
  retryable: boolean;
  check_index?: number;
  timestamp: string;
}

export interface CheckResult {
  type: string;
  status: CheckStatus;
  detail: string | null;
}

export interface PhaseState {
  id: string;
  status: PhaseStatus;
  started_at: string | null;
  completed_at: string | null;
  attempts: number;
  checks: CheckResult[];
  error: ErrorInfo | null;
  skip_reason?: string;
}

export interface PlanState {
  plan_name: string;
  plan_file: string;
  status: PlanStatus;
  started_at: string | null;
  completed_at: string | null;
  aborted_at: string | null;
  phases: PhaseState[];
  version: number;
}

export interface RunnerStatus {
  mode: 'running' | 'idle' | 'completed' | 'aborted';
  plan_name: string;
  current_phase: string | null;
  phases_completed: number;
  phases_total: number;
  updated_at: string;
}

// ── State directory ──

export function stateDir(planName: string): string {
  return resolve('.edda', 'orchestrator', planName);
}

export function statePath(planName: string): string {
  return resolve(stateDir(planName), 'plan-state.json');
}

export function runnerStatusPath(planName: string): string {
  return resolve(stateDir(planName), 'runner-status.json');
}

// ── Plan status derivation ──

export function derivePlanStatus(phases: PhaseState[]): PlanStatus {
  if (phases.some((p) => p.status === 'running' || p.status === 'checking')) {
    return 'running';
  }
  if (phases.some((p) => p.status === 'failed' || p.status === 'stale')) {
    return 'blocked';
  }
  if (phases.every((p) => p.status === 'passed' || p.status === 'skipped')) {
    return 'completed';
  }
  return 'pending';
}

// ── Initialize state from plan ──

export function initPlanState(plan: Plan, planFile: string): PlanState {
  return {
    plan_name: plan.name,
    plan_file: planFile,
    status: 'pending',
    started_at: null,
    completed_at: null,
    aborted_at: null,
    phases: plan.phases.map((p) => ({
      id: p.id,
      status: 'pending' as PhaseStatus,
      started_at: null,
      completed_at: null,
      attempts: 0,
      checks: [],
      error: null,
    })),
    version: 1,
  };
}

// ── Persistence (atomic write) ──

export function savePlanState(state: PlanState, path?: string): void {
  const target = path ?? statePath(state.plan_name);
  mkdirSync(dirname(target), { recursive: true });
  const json = JSON.stringify(state, null, 2) + '\n';
  const tmp = `${target}.tmp`;
  writeFileSync(tmp, json);
  renameSync(tmp, target);
}

export function loadPlanState(path: string): PlanState | null {
  try {
    const raw = readFileSync(path, 'utf-8');
    return JSON.parse(raw) as PlanState;
  } catch {
    return null;
  }
}

/** Load existing state or initialize from plan */
export function loadOrInitState(plan: Plan, planFile: string): PlanState {
  const path = statePath(plan.name);
  const existing = loadPlanState(path);
  if (existing && existing.plan_name === plan.name) {
    // Detect stale phases on resume
    detectStalePhases(existing, plan);
    return existing;
  }
  return initPlanState(plan, planFile);
}

// ── State transitions (CAS guard) ──

/** Valid transition map: from → allowed to states */
const VALID_TRANSITIONS: Record<PhaseStatus, PhaseStatus[]> = {
  pending: ['running', 'skipped'],
  running: ['checking', 'failed', 'stale'],
  checking: ['passed', 'failed'],
  passed: [],      // terminal
  failed: ['pending'],  // retry
  skipped: [],     // terminal
  stale: ['pending'],   // retry
};

/**
 * CAS-like state transition. Only succeeds if current status matches `from`.
 * Returns true if transition was applied, false if rejected.
 */
export function transition(
  state: PlanState,
  phaseId: string,
  from: PhaseStatus,
  to: PhaseStatus,
  sideEffect?: Partial<PhaseState>,
): boolean {
  const phase = state.phases.find((p) => p.id === phaseId);
  if (!phase) {
    throw new Error(`phase "${phaseId}" not found in state`);
  }

  // Guard: current state must match
  if (phase.status !== from) {
    return false;
  }

  // Guard: transition must be valid
  if (!VALID_TRANSITIONS[from]?.includes(to)) {
    throw new Error(
      `invalid transition: "${phaseId}" ${from} → ${to}`,
    );
  }

  // Apply transition
  phase.status = to;
  if (sideEffect) {
    Object.assign(phase, sideEffect);
  }

  // Update derived plan status
  state.status = state.status === 'aborted' ? 'aborted' : derivePlanStatus(state.phases);

  return true;
}

// ── Stale detection ──

export function detectStalePhases(state: PlanState, plan: Plan): void {
  const now = Date.now();

  for (const phase of state.phases) {
    if (phase.status !== 'running' && phase.status !== 'checking') continue;
    if (!phase.started_at) continue;

    const planPhase = plan.phases.find((p) => p.id === phase.id);
    const timeoutMs = (planPhase?.timeout_sec ?? 1800) * 1000;
    const elapsed = now - new Date(phase.started_at).getTime();

    if (elapsed > timeoutMs) {
      phase.status = 'stale';
      phase.error = {
        type: 'timeout',
        message: `phase "${phase.id}" was running for ${Math.round(elapsed / 1000)}s (timeout: ${planPhase?.timeout_sec ?? 1800}s)`,
        retryable: true,
        timestamp: new Date().toISOString(),
      };
    }
  }

  state.status = state.status === 'aborted' ? 'aborted' : derivePlanStatus(state.phases);
}

// ── Runner status ──

export function saveRunnerStatus(planName: string, status: RunnerStatus): void {
  const path = runnerStatusPath(planName);
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, JSON.stringify(status, null, 2) + '\n');
}

export function buildRunnerStatus(state: PlanState): RunnerStatus {
  const running = state.phases.find(
    (p) => p.status === 'running' || p.status === 'checking',
  );
  const completed = state.phases.filter(
    (p) => p.status === 'passed' || p.status === 'skipped',
  ).length;

  let mode: RunnerStatus['mode'];
  if (state.status === 'aborted') mode = 'aborted';
  else if (state.status === 'completed') mode = 'completed';
  else if (state.status === 'running') mode = 'running';
  else mode = 'idle';

  return {
    mode,
    plan_name: state.plan_name,
    current_phase: running?.id ?? null,
    phases_completed: completed,
    phases_total: state.phases.length,
    updated_at: new Date().toISOString(),
  };
}

// ── Helper: get phase state ──

export function getPhase(state: PlanState, phaseId: string): PhaseState {
  const phase = state.phases.find((p) => p.id === phaseId);
  if (!phase) throw new Error(`phase "${phaseId}" not found`);
  return phase;
}

/** Find the next pending phase (considering topo order) */
export function nextPendingPhase(state: PlanState, order: string[]): string | null {
  for (const id of order) {
    const phase = state.phases.find((p) => p.id === id);
    if (phase?.status === 'pending') return id;
  }
  return null;
}

// ── Plan scanning ──

/** List all plan states from the orchestrator state directory */
export function listPlanStates(cwd?: string): PlanState[] {
  const baseDir = resolve(cwd ?? '.', '.edda', 'orchestrator');
  if (!existsSync(baseDir)) return [];

  const entries = readdirSync(baseDir, { withFileTypes: true });
  const states: PlanState[] = [];

  for (const entry of entries) {
    if (!entry.isDirectory()) continue;
    const stateFile = resolve(baseDir, entry.name, 'plan-state.json');
    const state = loadPlanState(stateFile);
    if (state) states.push(state);
  }

  return states;
}

/** Find a plan state that contains a specific phase */
export function findPlanForPhase(
  phaseId: string,
  planName?: string,
  cwd?: string,
): PlanState | null {
  const states = listPlanStates(cwd);
  const candidates = planName
    ? states.filter((s) => s.plan_name === planName)
    : states;

  for (const s of candidates) {
    if (s.phases.some((p) => p.id === phaseId)) return s;
  }
  return null;
}
