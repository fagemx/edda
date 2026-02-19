import { describe, it, expect, vi, beforeEach } from 'vitest';
import { join } from 'node:path';
import { readFileSync, rmSync } from 'node:fs';
import { testContext } from './helpers/test-context.js';
import {
  checkEddaAvailable,
  phaseSessionId,
  TranscriptWriter,
  buildEddaHooks,
} from '../src/agent/hooks.js';

// ── checkEddaAvailable ──

describe('checkEddaAvailable', () => {
  it('returns a boolean', () => {
    const result = checkEddaAvailable();
    expect(typeof result).toBe('boolean');
  });
});

// ── phaseSessionId ──

describe('phaseSessionId', () => {
  it('generates deterministic session ID', () => {
    expect(phaseSessionId('add-auth', 'schema', 1)).toBe(
      'orchestrate-add-auth-schema-1',
    );
  });

  it('includes attempt number', () => {
    expect(phaseSessionId('plan', 'phase', 2)).toBe(
      'orchestrate-plan-phase-2',
    );
  });
});

// ── buildEddaHooks ──

describe('buildEddaHooks', () => {
  it('returns config with all expected hook names', () => {
    const config = buildEddaHooks();
    expect(config).toHaveProperty('SessionStart');
    expect(config).toHaveProperty('PreCompact');
    expect(config).toHaveProperty('UserPromptSubmit');
    expect(config).toHaveProperty('PreToolUse');
    expect(config).toHaveProperty('PostToolUse');
    expect(config).toHaveProperty('PostToolUseFailure');
    expect(config).toHaveProperty('SessionEnd');
  });

  it('PreToolUse has Edit|Write|Bash matcher', () => {
    const config = buildEddaHooks();
    expect(config.PreToolUse![0]!.matcher).toBe('Edit|Write|Bash');
  });
});

// ── TranscriptWriter ──

describe('TranscriptWriter', () => {
  const ctx = testContext();

  it('writes JSONL lines on flush', () => {
    const dir = ctx.createTempDir();
    const path = join(dir, 'transcript.jsonl');
    const writer = new TranscriptWriter(path);

    writer.append({ role: 'user', content: 'hello' });
    writer.append({ role: 'assistant', content: 'hi' });
    writer.flush();
    writer.close();

    const lines = readFileSync(path, 'utf-8').trim().split('\n');
    expect(lines).toHaveLength(2);
    expect(JSON.parse(lines[0]!)).toEqual({ role: 'user', content: 'hello' });
    expect(JSON.parse(lines[1]!)).toEqual({ role: 'assistant', content: 'hi' });
  });

  it('auto-flushes on close', () => {
    const dir = ctx.createTempDir();
    const path = join(dir, 'transcript2.jsonl');
    const writer = new TranscriptWriter(path);

    writer.append({ msg: 'buffered' });
    // Don't call flush — close should handle it
    writer.close();

    const content = readFileSync(path, 'utf-8').trim();
    expect(content).toBe('{"msg":"buffered"}');
  });

  it('creates parent directories', () => {
    const dir = ctx.createTempDir();
    const deepPath = join(dir, 'nested', 'deep', 'transcript.jsonl');
    const writer = new TranscriptWriter(deepPath);

    writer.append({ test: true });
    writer.close();

    expect(readFileSync(deepPath, 'utf-8')).toContain('{"test":true}');
  });

  it('exposes path as readonly', () => {
    const dir = ctx.createTempDir();
    const path = join(dir, 'readable.jsonl');
    const writer = new TranscriptWriter(path);
    expect(writer.path).toBe(path);
    writer.close();
  });
});
