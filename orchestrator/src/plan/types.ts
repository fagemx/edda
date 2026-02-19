import { z } from 'zod';

// ── Reusable primitives ──

const kebabCase = z
  .string()
  .min(1)
  .regex(/^[a-z0-9][a-z0-9-]*$/, 'must be kebab-case');

// ── Check schemas ──

const fileExistsCheck = z.object({
  type: z.literal('file_exists'),
  path: z.string().min(1),
});

const cmdSucceedsCheck = z.object({
  type: z.literal('cmd_succeeds'),
  cmd: z.string().min(1),
  timeout_sec: z.number().min(1).default(120),
});

const gitCleanCheck = z.object({
  type: z.literal('git_clean'),
  allow_untracked: z.boolean().default(false),
});

const fileContainsCheck = z.object({
  type: z.literal('file_contains'),
  path: z.string().min(1),
  pattern: z.string().min(1),
});

const eddaEventCheck = z.object({
  type: z.literal('edda_event'),
  event_type: z.string().min(1),
  after: z.string().optional(),
});

/** Inner check types (everything except wait_until) */
const innerCheckSchema = z.discriminatedUnion('type', [
  fileExistsCheck,
  cmdSucceedsCheck,
  gitCleanCheck,
  fileContainsCheck,
  eddaEventCheck,
]);

const waitUntilCheck = z.object({
  type: z.literal('wait_until'),
  check: innerCheckSchema,
  interval_sec: z.number().min(5).default(30),
  timeout_sec: z.number().min(10).max(7200).default(600),
  backoff: z.enum(['none', 'linear', 'exponential']).default('linear'),
});

/** All check types including wait_until */
export const checkSchema = z.discriminatedUnion('type', [
  fileExistsCheck,
  cmdSucceedsCheck,
  gitCleanCheck,
  fileContainsCheck,
  eddaEventCheck,
  waitUntilCheck,
]);

// ── Short format normalization ──

/**
 * Normalize a short-format check entry into object format.
 *
 * Short format examples:
 *   - `file_exists: src/db/schema.sql`
 *   - `cmd_succeeds: "cargo test auth"`
 *   - `git_clean: true`
 */
export function normalizeCheck(raw: unknown): unknown {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return raw;
  }

  const obj = raw as Record<string, unknown>;

  // Already has `type` field → object format, pass through
  if ('type' in obj) return obj;

  // Short format: single key = type, value = primary param
  const keys = Object.keys(obj);
  if (keys.length !== 1) return obj;

  const type = keys[0]!;
  const value = obj[type];

  switch (type) {
    case 'file_exists':
      return { type: 'file_exists', path: String(value) };
    case 'cmd_succeeds':
      return { type: 'cmd_succeeds', cmd: String(value) };
    case 'git_clean':
      return { type: 'git_clean', allow_untracked: false };
    case 'file_contains':
      // file_contains doesn't have a short format per spec
      return obj;
    default:
      return obj;
  }
}

// ── Phase schema ──

export const onFailSchema = z.enum(['ask', 'skip', 'abort']).default('ask');

export const phaseSchema = z.object({
  id: kebabCase,
  prompt: z.string().min(1),
  cwd: z.string().optional(),
  depends_on: z.array(z.string()).default([]),
  check: z.array(z.unknown()).default([]).transform((arr) => arr.map(normalizeCheck)),
  max_attempts: z.number().min(1).default(3),
  timeout_sec: z.number().min(10).default(1800),
  context: z.string().optional(),
  env: z.record(z.string()).default({}),
  on_fail: onFailSchema,
  allowed_tools: z.array(z.string()).optional(),
  permission_mode: z.string().default('bypassPermissions'),
});

// ── Plan schema ──

export const planSchema = z.object({
  name: kebabCase,
  phases: z.array(phaseSchema).min(1),
  description: z.string().optional(),
  cwd: z.string().optional(),
  max_attempts: z.number().min(1).default(3),
  timeout_sec: z.number().min(10).default(1800),
  budget_usd: z.number().positive().optional(),
  env: z.record(z.string()).default({}),
  on_fail: onFailSchema,
  tags: z.array(z.string()).default([]),
});

// ── Derived TypeScript types ──

export type CheckSpec = z.infer<typeof checkSchema>;
export type Phase = z.infer<typeof phaseSchema>;
export type Plan = z.infer<typeof planSchema>;

/** Phase execution status (runtime, not from YAML) */
export type PhaseStatus = 'pending' | 'running' | 'passed' | 'failed' | 'skipped' | 'blocked';

/** Plan execution status (runtime, not from YAML) */
export type PlanStatus = 'pending' | 'running' | 'completed' | 'failed' | 'aborted';

// ── Defaults (centralized) ──

export const PLAN_DEFAULTS = {
  max_attempts: 3,
  timeout_sec: 1800,
  permission_mode: 'bypassPermissions',
} as const;

export const CHECK_DEFAULTS = {
  cmd_timeout_sec: 120,
  wait_until_interval: 30,
  wait_until_timeout: 600,
  wait_until_backoff: 'linear',
} as const;
