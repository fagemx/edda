import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { parsePlanYaml, topoSort, expandVariables } from '../src/plan/parser.js';
import { normalizeCheck } from '../src/plan/types.js';
import { OrchestratorError } from '../src/lib/errors.js';

const fixturesDir = join(import.meta.dirname, '..', 'fixtures');

function loadFixture(name: string): string {
  return readFileSync(join(fixturesDir, name), 'utf-8');
}

// ── Schema validation ──

describe('parsePlanYaml', () => {
  it('parses minimal.yaml', () => {
    const plan = parsePlanYaml(loadFixture('minimal.yaml'));
    expect(plan.name).toBe('minimal-test');
    expect(plan.phases).toHaveLength(1);
    expect(plan.phases[0]!.id).toBe('hello');
    expect(plan.phases[0]!.prompt).toContain('hello.txt');
  });

  it('parses typical.yaml with depends_on', () => {
    const plan = parsePlanYaml(loadFixture('typical.yaml'));
    expect(plan.name).toBe('add-auth-module');
    expect(plan.phases).toHaveLength(3);
    expect(plan.phases[1]!.depends_on).toEqual(['schema']);
    expect(plan.phases[2]!.depends_on).toEqual(['api']);
  });

  it('parses advanced.yaml with wait_until', () => {
    const plan = parsePlanYaml(loadFixture('advanced.yaml'));
    expect(plan.name).toBe('deploy-service');
    expect(plan.phases).toHaveLength(3);
    // verify check field exists (normalized from short format)
    expect(plan.phases[0]!.check.length).toBe(2);
  });

  it('applies defaults', () => {
    const plan = parsePlanYaml(loadFixture('minimal.yaml'));
    const phase = plan.phases[0]!;
    expect(phase.max_attempts).toBe(3);
    expect(phase.timeout_sec).toBe(1800);
    expect(phase.depends_on).toEqual([]);
    expect(phase.env).toEqual({});
    expect(phase.on_fail).toBe('ask');
    expect(phase.permission_mode).toBe('bypassPermissions');
  });

  it('rejects missing name', () => {
    expect(() => parsePlanYaml(loadFixture('invalid-no-name.yaml'))).toThrow(OrchestratorError);
    try {
      parsePlanYaml(loadFixture('invalid-no-name.yaml'));
    } catch (err) {
      expect((err as OrchestratorError).code).toBe('PLAN_VALIDATION_ERROR');
    }
  });

  it('rejects duplicate phase IDs', () => {
    expect(() => parsePlanYaml(loadFixture('invalid-dup-id.yaml'))).toThrow(OrchestratorError);
    try {
      parsePlanYaml(loadFixture('invalid-dup-id.yaml'));
    } catch (err) {
      expect((err as OrchestratorError).message).toContain('duplicate phase id');
    }
  });

  it('rejects circular dependencies', () => {
    expect(() => parsePlanYaml(loadFixture('invalid-cycle.yaml'))).toThrow(OrchestratorError);
    try {
      parsePlanYaml(loadFixture('invalid-cycle.yaml'));
    } catch (err) {
      expect((err as OrchestratorError).message).toContain('circular dependency');
    }
  });

  it('rejects invalid YAML syntax', () => {
    expect(() => parsePlanYaml('{ invalid yaml ][}')).toThrow(OrchestratorError);
    try {
      parsePlanYaml('{ invalid yaml ][]');
    } catch (err) {
      expect((err as OrchestratorError).code).toBe('PLAN_PARSE_ERROR');
    }
  });

  it('rejects non-kebab-case name', () => {
    const yaml = `name: "My Plan"\nphases:\n  - id: step1\n    prompt: "Do something"`;
    expect(() => parsePlanYaml(yaml)).toThrow(OrchestratorError);
  });

  it('rejects depends_on referencing non-existent phase', () => {
    const yaml = `name: my-plan\nphases:\n  - id: step1\n    prompt: "Do"\n    depends_on: [nonexistent]`;
    expect(() => parsePlanYaml(yaml)).toThrow(OrchestratorError);
    try {
      parsePlanYaml(yaml);
    } catch (err) {
      expect((err as OrchestratorError).message).toContain('depends_on "nonexistent" not found');
    }
  });
});

// ── Short format normalization ──

describe('normalizeCheck', () => {
  it('normalizes file_exists short format', () => {
    expect(normalizeCheck({ file_exists: 'src/main.rs' })).toEqual({
      type: 'file_exists',
      path: 'src/main.rs',
    });
  });

  it('normalizes cmd_succeeds short format', () => {
    expect(normalizeCheck({ cmd_succeeds: 'cargo test' })).toEqual({
      type: 'cmd_succeeds',
      cmd: 'cargo test',
    });
  });

  it('normalizes git_clean short format', () => {
    expect(normalizeCheck({ git_clean: true })).toEqual({
      type: 'git_clean',
      allow_untracked: false,
    });
  });

  it('passes through object format', () => {
    const obj = { type: 'file_exists', path: 'foo.txt' };
    expect(normalizeCheck(obj)).toBe(obj);
  });

  it('passes through unknown format', () => {
    const obj = { unknown_type: 'value' };
    expect(normalizeCheck(obj)).toBe(obj);
  });
});

// ── Topological sort ──

describe('topoSort', () => {
  it('sorts linear chain', () => {
    const phases = [
      { id: 'c', depends_on: ['b'] },
      { id: 'a', depends_on: [] },
      { id: 'b', depends_on: ['a'] },
    ];
    const order = topoSort(phases);
    expect(order.indexOf('a')).toBeLessThan(order.indexOf('b'));
    expect(order.indexOf('b')).toBeLessThan(order.indexOf('c'));
  });

  it('sorts diamond dependency', () => {
    const phases = [
      { id: 'd', depends_on: ['b', 'c'] },
      { id: 'a', depends_on: [] },
      { id: 'b', depends_on: ['a'] },
      { id: 'c', depends_on: ['a'] },
    ];
    const order = topoSort(phases);
    expect(order.indexOf('a')).toBe(0);
    expect(order.indexOf('d')).toBe(3);
  });

  it('returns stable order for independent phases', () => {
    const phases = [
      { id: 'z', depends_on: [] },
      { id: 'a', depends_on: [] },
      { id: 'm', depends_on: [] },
    ];
    const order = topoSort(phases);
    // alphabetical for equal in-degree
    expect(order).toEqual(['a', 'm', 'z']);
  });
});

// ── Variable expansion ──

describe('expandVariables', () => {
  it('expands env variables', () => {
    const result = expandVariables('Deploy to ${{ env.TARGET }}', { TARGET: 'staging' } as NodeJS.ProcessEnv);
    expect(result).toBe('Deploy to staging');
  });

  it('leaves unresolved variables intact', () => {
    const result = expandVariables('Key: ${{ env.MISSING }}', {} as NodeJS.ProcessEnv);
    expect(result).toBe('Key: ${{ env.MISSING }}');
  });

  it('leaves secrets marker intact (not yet supported)', () => {
    const result = expandVariables('Secret: ${{ secrets.API_KEY }}', {} as NodeJS.ProcessEnv);
    expect(result).toBe('Secret: ${{ secrets.API_KEY }}');
  });

  it('handles multiple variables', () => {
    const result = expandVariables(
      '${{ env.A }} and ${{ env.B }}',
      { A: 'hello', B: 'world' } as NodeJS.ProcessEnv,
    );
    expect(result).toBe('hello and world');
  });
});
