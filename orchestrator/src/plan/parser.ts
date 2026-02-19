import { readFileSync } from 'node:fs';
import YAML from 'yaml';
import { ZodError } from 'zod';
import { OrchestratorError, ErrorCode } from '../lib/errors.js';
import { planSchema, checkSchema, type Plan } from './types.js';

// ── Variable expansion ──

const VARIABLE_PATTERN = /\$\{\{\s*(env|secrets)\.\s*([a-zA-Z_]\w*)\s*\}\}/g;

export function expandVariables(
  text: string,
  env: NodeJS.ProcessEnv = process.env,
): string {
  return text.replace(VARIABLE_PATTERN, (match, source: string, name: string) => {
    if (source === 'env') return env[name] ?? match;
    // secrets not yet supported — leave marker intact
    return match;
  });
}

// ── Pre-validation (catch structural errors before Zod) ──

function preValidate(raw: unknown): string | null {
  if (typeof raw !== 'object' || raw === null) {
    return '✗ plan.yaml must contain a YAML object\n  Example:\n    name: my-plan\n    phases:\n      - id: step1\n        prompt: "Do something"';
  }

  const obj = raw as Record<string, unknown>;

  if (Array.isArray(obj.phases?.[0 as keyof typeof obj.phases])) {
    return '✗ phases must be an array of objects, not nested arrays\n  Each phase needs: id, prompt';
  }

  if (typeof obj.name === 'number') {
    return '✗ plan.name must be a string, not a number\n  Example: name: "my-plan"';
  }

  return null;
}

// ── Error formatting ──

export function formatPlanError(error: ZodError): string {
  const lines: string[] = [];

  for (const issue of error.issues) {
    const path = issue.path.join('.');

    // Friendly messages for common mistakes
    if (path === 'phases' && issue.code === 'too_small') {
      lines.push('✗ plan.phases must have at least one phase');
      lines.push('  Example:\n    phases:\n      - id: my-task\n        prompt: "Do something"');
      continue;
    }

    if (path === 'name' && issue.code === 'invalid_string') {
      lines.push(`✗ plan.name must be kebab-case`);
      lines.push('  Example: add-auth-module, fix-typo, deploy-staging');
      continue;
    }

    if (issue.code === 'custom') {
      // Business rule errors from superRefine
      lines.push(`✗ ${issue.message}`);
      continue;
    }

    // Generic format
    lines.push(`✗ ${path}: ${issue.message}`);
  }

  return lines.join('\n');
}

// ── Business rule validation (superRefine) ──

const planWithRules = planSchema.superRefine((plan, ctx) => {
  const ids = plan.phases.map((p) => p.id);

  // 1. Unique phase IDs
  const seen = new Set<string>();
  for (const id of ids) {
    if (seen.has(id)) {
      ctx.addIssue({ code: 'custom', message: `duplicate phase id: "${id}"` });
    }
    seen.add(id);
  }

  // 2. depends_on references exist
  for (const phase of plan.phases) {
    for (const dep of phase.depends_on) {
      if (!ids.includes(dep)) {
        ctx.addIssue({
          code: 'custom',
          message: `phase "${phase.id}": depends_on "${dep}" not found`,
        });
      }
    }
  }

  // 3. Cycle detection (Kahn's algorithm)
  const cycle = detectCycle(plan.phases);
  if (cycle) {
    ctx.addIssue({
      code: 'custom',
      message: `circular dependency detected: ${cycle.join(' → ')}`,
    });
  }

  // 4. Validate checks against checkSchema
  for (const phase of plan.phases) {
    for (let i = 0; i < phase.check.length; i++) {
      const raw = phase.check[i];
      const result = checkSchema.safeParse(raw);
      if (!result.success) {
        for (const issue of result.error.issues) {
          const checkPath = issue.path.join('.');
          const prefix = `phase "${phase.id}": check[${i}]`;
          ctx.addIssue({
            code: 'custom',
            message: checkPath ? `${prefix}.${checkPath}: ${issue.message}` : `${prefix}: ${issue.message}`,
          });
        }
      }

      // wait_until cannot nest wait_until
      if (
        typeof raw === 'object' &&
        raw !== null &&
        'type' in raw &&
        (raw as Record<string, unknown>).type === 'wait_until' &&
        'check' in raw
      ) {
        const inner = (raw as Record<string, unknown>).check;
        if (
          typeof inner === 'object' &&
          inner !== null &&
          'type' in inner &&
          (inner as Record<string, unknown>).type === 'wait_until'
        ) {
          ctx.addIssue({
            code: 'custom',
            message: `phase "${phase.id}": wait_until cannot nest wait_until`,
          });
        }
      }
    }
  }
});

// ── Cycle detection ──

function detectCycle(phases: { id: string; depends_on: string[] }[]): string[] | null {
  const adj = new Map<string, string[]>();
  const inDegree = new Map<string, number>();

  for (const p of phases) {
    adj.set(p.id, []);
    inDegree.set(p.id, 0);
  }

  for (const p of phases) {
    for (const dep of p.depends_on) {
      adj.get(dep)?.push(p.id);
      inDegree.set(p.id, (inDegree.get(p.id) ?? 0) + 1);
    }
  }

  const queue: string[] = [];
  for (const [id, deg] of inDegree) {
    if (deg === 0) queue.push(id);
  }

  const sorted: string[] = [];
  while (queue.length > 0) {
    const node = queue.shift()!;
    sorted.push(node);
    for (const neighbor of adj.get(node) ?? []) {
      const newDeg = (inDegree.get(neighbor) ?? 1) - 1;
      inDegree.set(neighbor, newDeg);
      if (newDeg === 0) queue.push(neighbor);
    }
  }

  if (sorted.length === phases.length) return null;

  // Find which nodes are in the cycle
  const inCycle = phases.filter((p) => !sorted.includes(p.id)).map((p) => p.id);
  return inCycle;
}

// ── Topological sort ──

export function topoSort(phases: { id: string; depends_on: string[] }[]): string[] {
  const adj = new Map<string, string[]>();
  const inDegree = new Map<string, number>();

  for (const p of phases) {
    adj.set(p.id, []);
    inDegree.set(p.id, 0);
  }

  for (const p of phases) {
    for (const dep of p.depends_on) {
      adj.get(dep)?.push(p.id);
      inDegree.set(p.id, (inDegree.get(p.id) ?? 0) + 1);
    }
  }

  const queue: string[] = [];
  for (const [id, deg] of inDegree) {
    if (deg === 0) queue.push(id);
  }

  // Stable sort: process alphabetically when equal in-degree
  queue.sort();

  const result: string[] = [];
  while (queue.length > 0) {
    const node = queue.shift()!;
    result.push(node);
    const neighbors = [...(adj.get(node) ?? [])].sort();
    for (const neighbor of neighbors) {
      const newDeg = (inDegree.get(neighbor) ?? 1) - 1;
      inDegree.set(neighbor, newDeg);
      if (newDeg === 0) queue.push(neighbor);
    }
    queue.sort();
  }

  return result;
}

// ── Public API ──

/** Parse a YAML string into a validated Plan. Throws OrchestratorError on failure. */
export function parsePlanYaml(content: string): Plan {
  let raw: unknown;
  try {
    raw = YAML.parse(content);
  } catch (err) {
    throw new OrchestratorError(
      ErrorCode.PLAN_PARSE_ERROR,
      `Invalid YAML: ${err instanceof Error ? err.message : String(err)}`,
      'Check your plan.yaml for syntax errors (indentation, colons, etc.)',
    );
  }

  // Pre-validation for better error messages
  const preError = preValidate(raw);
  if (preError) {
    throw new OrchestratorError(ErrorCode.PLAN_VALIDATION_ERROR, preError);
  }

  const result = planWithRules.safeParse(raw);
  if (!result.success) {
    throw new OrchestratorError(
      ErrorCode.PLAN_VALIDATION_ERROR,
      formatPlanError(result.error),
      'Fix the issues above and try again',
    );
  }

  return result.data;
}

/** Load and parse a plan.yaml file. Throws OrchestratorError on failure. */
export function loadPlanFile(filePath: string): Plan {
  let content: string;
  try {
    content = readFileSync(filePath, 'utf-8');
  } catch {
    throw new OrchestratorError(
      ErrorCode.PLAN_NOT_FOUND,
      `plan.yaml not found: ${filePath}`,
      'Run: orchestrate run <path-to-plan.yaml>',
    );
  }

  return parsePlanYaml(content);
}

/** Load a plan file and return phases in topological order */
export function loadPlanWithOrder(filePath: string): { plan: Plan; order: string[] } {
  const plan = loadPlanFile(filePath);
  const order = topoSort(plan.phases);
  return { plan, order };
}
