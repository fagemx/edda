import { describe, it, expect } from 'vitest';
import { testContext } from './helpers/test-context.js';
import { writeFileSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

describe('P1 Bootstrap Smoke Test', () => {
  it('testContext creates and cleans up temp dirs', () => {
    const ctx = testContext();
    const dir = ctx.createTempDir();
    expect(dir).toBeTruthy();

    // Write a file to verify real filesystem
    const testFile = join(dir, 'test.txt');
    writeFileSync(testFile, 'hello');
    expect(readFileSync(testFile, 'utf-8')).toBe('hello');
  });

  it('testContext generates unique IDs', () => {
    const ctx = testContext();
    const id1 = ctx.uniqueId('plan');
    const id2 = ctx.uniqueId('plan');
    expect(id1).not.toBe(id2);
    expect(id1).toMatch(/^plan-[0-9a-f]{8}$/);
  });

  it('types are importable', async () => {
    const types = await import('../src/plan/types.js');
    // Verify Zod schemas are exported
    expect(types.planSchema).toBeDefined();
    expect(types.phaseSchema).toBeDefined();
    expect(types.checkSchema).toBeDefined();
    // Verify normalizeCheck works
    expect(types.normalizeCheck({ file_exists: 'src/main.rs' })).toEqual({
      type: 'file_exists',
      path: 'src/main.rs',
    });
  });

  it('errors have code and hint', async () => {
    const { OrchestratorError, ErrorCode } = await import('../src/lib/errors.js');
    const err = new OrchestratorError(
      ErrorCode.PLAN_NOT_FOUND,
      'plan.yaml not found',
      'Run: orchestrate run <path-to-plan.yaml>',
    );
    expect(err.code).toBe('PLAN_NOT_FOUND');
    expect(err.hint).toContain('orchestrate run');
    expect(err).toBeInstanceOf(Error);
  });

  it('formatDuration produces human-readable output', async () => {
    const { formatDuration } = await import('../src/lib/utils/format.js');
    expect(formatDuration(500)).toBe('500ms');
    expect(formatDuration(5000)).toBe('5s');
    expect(formatDuration(90_000)).toBe('1m 30s');
    expect(formatDuration(3_600_000)).toBe('1h');
  });

  it('uniqueId generates short hex strings', async () => {
    const { uniqueId } = await import('../src/lib/utils/unique-id.js');
    const id = uniqueId();
    expect(id).toHaveLength(8);
    expect(id).toMatch(/^[0-9a-f]{8}$/);
  });

  it('fixtures are valid YAML', async () => {
    const yaml = await import('yaml');
    const { readFileSync } = await import('node:fs');
    const { join } = await import('node:path');

    const fixturesDir = join(import.meta.dirname, '..', 'fixtures');
    for (const file of ['minimal.yaml', 'typical.yaml', 'advanced.yaml']) {
      const content = readFileSync(join(fixturesDir, file), 'utf-8');
      const parsed = yaml.parse(content);
      expect(parsed.name).toBeTruthy();
      expect(parsed.phases).toBeInstanceOf(Array);
      expect(parsed.phases.length).toBeGreaterThan(0);
    }
  });
});
