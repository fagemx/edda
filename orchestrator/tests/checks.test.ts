import { describe, it, expect } from 'vitest';
import { join } from 'node:path';
import { writeFileSync } from 'node:fs';
import { testContext } from './helpers/test-context.js';
import {
  checkFileExists,
  checkCmdSucceeds,
  checkFileContains,
  checkGitClean,
  runChecks,
  maskSecrets,
  isRetryable,
} from '../src/checks/engine.js';

const ctx = testContext();

// ── maskSecrets ──

describe('maskSecrets', () => {
  it('masks API keys with sk- prefix', () => {
    const input = 'Error: invalid key sk-abcdefghijklmnopqrstuvwxyz in config';
    expect(maskSecrets(input)).toContain('***');
    expect(maskSecrets(input)).not.toContain('sk-abcdef');
  });

  it('masks Bearer tokens', () => {
    const input = 'Authorization: Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9';
    expect(maskSecrets(input)).toBe('Authorization: Bearer ***');
  });

  it('masks password= values', () => {
    const input = 'DATABASE_URL=postgres://user:password=secret123 host=localhost';
    expect(maskSecrets(input)).toContain('password=***');
  });

  it('leaves normal text untouched', () => {
    expect(maskSecrets('hello world')).toBe('hello world');
  });
});

// ── isRetryable ──

describe('isRetryable', () => {
  it('cmd_succeeds timeout is retryable', () => {
    expect(isRetryable('cmd_succeeds', 'command timed out after 120s')).toBe(true);
  });

  it('cmd_succeeds exit code is not retryable', () => {
    expect(isRetryable('cmd_succeeds', 'exit 1: test failed')).toBe(false);
  });

  it('wait_until is always retryable', () => {
    expect(isRetryable('wait_until', 'anything')).toBe(true);
  });

  it('file_exists is not retryable', () => {
    expect(isRetryable('file_exists', 'file not found')).toBe(false);
  });
});

// ── checkFileExists ──

describe('checkFileExists', () => {
  it('passes when file exists', async () => {
    const dir = ctx.createTempDir();
    writeFileSync(join(dir, 'test.txt'), 'hello');
    const result = await checkFileExists({ path: 'test.txt' }, dir);
    expect(result.status).toBe('passed');
    expect(result.detail).toBeNull();
  });

  it('fails when file missing', async () => {
    const dir = ctx.createTempDir();
    const result = await checkFileExists({ path: 'missing.txt' }, dir);
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('file not found');
  });
});

// ── checkCmdSucceeds ──

describe('checkCmdSucceeds', () => {
  it('passes on exit 0', async () => {
    const dir = ctx.createTempDir();
    const result = await checkCmdSucceeds({ cmd: 'echo hello' }, dir);
    expect(result.status).toBe('passed');
    expect(result.duration_ms).toBeGreaterThanOrEqual(0);
  });

  it('fails on non-zero exit', async () => {
    const dir = ctx.createTempDir();
    const result = await checkCmdSucceeds({ cmd: 'exit 1' }, dir);
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('exit');
  });

  it('fails on timeout', async () => {
    const dir = ctx.createTempDir();
    // Use node to sleep — reliable cross-platform
    const result = await checkCmdSucceeds(
      { cmd: 'node -e "setTimeout(()=>{},10000)"', timeout_sec: 1 },
      dir,
    );
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('timed out');
  }, 10_000);
});

// ── checkFileContains ──

describe('checkFileContains', () => {
  it('passes when pattern found', async () => {
    const dir = ctx.createTempDir();
    writeFileSync(join(dir, 'code.rs'), 'fn main() { println!("hello"); }');
    const result = await checkFileContains(
      { path: 'code.rs', pattern: 'fn main()' },
      dir,
    );
    expect(result.status).toBe('passed');
  });

  it('fails when pattern not found', async () => {
    const dir = ctx.createTempDir();
    writeFileSync(join(dir, 'code.rs'), 'fn helper() {}');
    const result = await checkFileContains(
      { path: 'code.rs', pattern: 'fn main()' },
      dir,
    );
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('pattern not found');
  });

  it('fails when file missing', async () => {
    const dir = ctx.createTempDir();
    const result = await checkFileContains(
      { path: 'missing.rs', pattern: 'anything' },
      dir,
    );
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('file not found');
  });
});

// ── checkGitClean ──

describe('checkGitClean', () => {
  it('passes in a clean git repo', async () => {
    const dir = ctx.createTempDir();
    // Init a pristine repo
    const { execSync } = await import('node:child_process');
    execSync('git init && git config user.email "test@test.com" && git config user.name "Test"', { cwd: dir, stdio: 'pipe' });
    writeFileSync(join(dir, 'file.txt'), 'hello');
    execSync('git add . && git commit -m "init"', { cwd: dir, stdio: 'pipe' });

    const result = await checkGitClean({}, dir);
    expect(result.status).toBe('passed');
  });

  it('fails with dirty files', async () => {
    const dir = ctx.createTempDir();
    const { execSync } = await import('node:child_process');
    execSync('git init && git config user.email "test@test.com" && git config user.name "Test"', { cwd: dir, stdio: 'pipe' });
    writeFileSync(join(dir, 'file.txt'), 'hello');
    execSync('git add . && git commit -m "init"', { cwd: dir, stdio: 'pipe' });
    // Dirty the repo
    writeFileSync(join(dir, 'file.txt'), 'modified');

    const result = await checkGitClean({}, dir);
    expect(result.status).toBe('failed');
    expect(result.detail).toContain('dirty');
  });

  it('allow_untracked ignores ?? files', async () => {
    const dir = ctx.createTempDir();
    const { execSync } = await import('node:child_process');
    execSync('git init && git config user.email "test@test.com" && git config user.name "Test"', { cwd: dir, stdio: 'pipe' });
    writeFileSync(join(dir, 'file.txt'), 'hello');
    execSync('git add . && git commit -m "init"', { cwd: dir, stdio: 'pipe' });
    // Add untracked file only
    writeFileSync(join(dir, 'untracked.txt'), 'new');

    const result = await checkGitClean({ allow_untracked: true }, dir);
    expect(result.status).toBe('passed');
  });
});

// ── runChecks (batch) ──

describe('runChecks', () => {
  it('runs all checks and returns allPassed=true', async () => {
    const dir = ctx.createTempDir();
    writeFileSync(join(dir, 'a.txt'), 'hello');
    writeFileSync(join(dir, 'b.txt'), 'world');

    const result = await runChecks(
      [
        { type: 'file_exists', path: 'a.txt' },
        { type: 'file_exists', path: 'b.txt' },
      ],
      dir,
    );
    expect(result.allPassed).toBe(true);
    expect(result.results).toHaveLength(2);
    expect(result.error).toBeNull();
  });

  it('short-circuits on first failure', async () => {
    const dir = ctx.createTempDir();
    writeFileSync(join(dir, 'a.txt'), 'hello');

    const result = await runChecks(
      [
        { type: 'file_exists', path: 'a.txt' },
        { type: 'file_exists', path: 'missing.txt' },
        { type: 'file_exists', path: 'also-missing.txt' },
      ],
      dir,
    );
    expect(result.allPassed).toBe(false);
    expect(result.results).toHaveLength(3);
    expect(result.results[0]!.status).toBe('passed');
    expect(result.results[1]!.status).toBe('failed');
    expect(result.results[2]!.status).toBe('waiting');
    expect(result.error).not.toBeNull();
    expect(result.error!.check_index).toBe(1);
  });

  it('handles empty checks array', async () => {
    const dir = ctx.createTempDir();
    const result = await runChecks([], dir);
    expect(result.allPassed).toBe(true);
    expect(result.results).toHaveLength(0);
  });

  it('reports unknown check type', async () => {
    const dir = ctx.createTempDir();
    const result = await runChecks(
      [{ type: 'nonexistent_check' }],
      dir,
    );
    expect(result.allPassed).toBe(false);
    expect(result.error!.message).toContain('unknown check type');
  });
});
