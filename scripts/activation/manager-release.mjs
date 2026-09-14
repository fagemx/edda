#!/usr/bin/env node
// scripts/activation/manager-release.mjs — Job C activation route, agent-manager step.
//
// The agent-manager service had no committed release/activation writer: controllers
// hand-wrote ~/.edda-agent-manager/release.json and upgrade-*.json receipts, so the
// service carrier could silently lag the merged revision. This driver is the one
// supported writer. It is stateless and only composes the manager's own primitives.
//
// Safety: it never kills a PID and never deletes old releases or backups. A live
// owner is stopped only through `cli.js stop` (which authenticates against owner.json
// and stops the console, not Pi workers); a stale owner is cleared only through
// `cli.js recover` (which refuses when the recorded PID is demonstrably alive).
//
// usage:
//   node scripts/activation/manager-release.mjs --repo <checkout> --root <serviceRoot>
//        [--revision <40-hex>] [--npm <path>] [--dry-run] [--no-restart] [--json]
import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, lstatSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';
import { randomUUID } from 'node:crypto';

const HEX40 = /^[0-9a-f]{40}$/;

const usage = `Agent-manager release/activation step (Job C)
  --repo PATH        Edda checkout that owns integrations/agent-manager (default: cwd toplevel)
  --root PATH        agent-manager service root (default: ~/.edda-agent-manager)
  --revision SHA     exact 40-hex revision to pin (default: repo HEAD)
  --npm PATH         npm executable (default: npm.cmd on Windows, npm elsewhere)
  --dry-run          print the plan; mutate nothing
  --no-restart       build and write release metadata only; never stop/start
  --json             print a JSON result
  -h, --help         this message
Exit codes: 0 done, 1 execution failure, 2 usage/refusal.`;

function parse(argv) {
  const options = { dryRun: false, noRestart: false, json: false };
  const valued = { '--repo': 'repo', '--root': 'root', '--revision': 'revision', '--npm': 'npm' };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '-h' || arg === '--help') return { help: true };
    if (arg === '--dry-run') { options.dryRun = true; continue; }
    if (arg === '--no-restart') { options.noRestart = true; continue; }
    if (arg === '--json') { options.json = true; continue; }
    const name = valued[arg];
    if (!name) throw { usage: `Unknown option ${arg}` };
    if (i + 1 >= argv.length || argv[i + 1].startsWith('--')) throw { usage: `Missing value for ${arg}` };
    if (options[name] !== undefined) throw { usage: `Duplicate option ${arg}` };
    options[name] = argv[++i];
  }
  return options;
}

function run(command, args, options = {}) {
  return execFileSync(command, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], ...options }).trim();
}

function readJsonBounded(path) {
  if (!existsSync(path)) return { present: false, value: null, invalid: false };
  const stat = lstatSync(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 262144) return { present: true, value: null, invalid: true };
  try { return { present: true, value: JSON.parse(readFileSync(path, 'utf8')), invalid: false }; }
  catch { return { present: true, value: null, invalid: true }; }
}

function pidAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try { process.kill(pid, 0); return true; } catch (error) { return error.code !== 'ESRCH'; }
}

function ownerOf(path) {
  const record = readJsonBounded(path);
  if (!record.present) return { present: false, owner: null, invalid: false };
  const value = record.value;
  const valid = !record.invalid && value && value.version === 1 && Number.isInteger(value.pid) && value.pid > 0 &&
    typeof value.instanceId === 'string' && value.instanceId && typeof value.token === 'string' && value.token &&
    typeof value.origin === 'string' && /^http:\/\/127\.0\.0\.1:[0-9]+$/.test(value.origin);
  return { present: true, owner: valid ? value : null, invalid: !valid };
}

function main(argv) {
  let options;
  try { options = parse(argv); }
  catch (error) { console.error(`manager-release: ${error.usage}`); process.exit(2); }
  if (options.help) { console.log(usage); return; }

  let repo;
  try { repo = resolve(options.repo || run('git', ['rev-parse', '--show-toplevel'])); }
  catch { console.error('manager-release: --repo is required and must be a git checkout'); process.exit(2); }
  const revision = options.revision || run('git', ['-C', repo, 'rev-parse', 'HEAD']);
  if (!HEX40.test(revision)) { console.error('manager-release: --revision must be a full 40-hex SHA'); process.exit(2); }
  const root = resolve(options.root || join(homedir(), '.edda-agent-manager'));
  const npm = options.npm || (process.platform === 'win32' ? 'npm.cmd' : 'npm');
  const packageDir = join(repo, 'integrations', 'agent-manager');
  const cli = join(packageDir, 'dist', 'src', 'cli.js');
  const releaseFile = join(root, 'release.json');
  const ownerFile = join(root, 'owner.json');

  if (!existsSync(packageDir)) { console.error(`manager-release: ${packageDir} is missing`); process.exit(2); }
  const owner = ownerOf(ownerFile);
  if (owner.invalid) { console.error('manager-release: owner.json is present but unusable; refusing to guess ownership'); process.exit(2); }

  const live = owner.owner ? pidAlive(owner.owner.pid) : false;
  const steps = [];
  steps.push({ step: 'build', commands: [[npm, ['ci', '--ignore-scripts'], packageDir], [npm, ['run', 'build'], packageDir]] });
  if (live) steps.push({ step: 'stop', commands: [[process.execPath, [cli, 'stop', '--root', root]]], owner: { pid: owner.owner.pid, instanceId: owner.owner.instanceId } });
  else if (owner.present) steps.push({ step: 'recover', commands: [[process.execPath, [cli, 'recover', '--root', root]]], owner: { pid: owner.owner?.pid ?? null, instanceId: owner.owner?.instanceId ?? null } });
  steps.push({ step: 'write-release', file: releaseFile, previous: readJsonBounded(releaseFile).present });
  if (!options.noRestart) steps.push({ step: 'start', commands: [[process.execPath, [cli, 'start', '--root', root]]] });

  const plan = { repo, root, revision, npm, entrypoint: cli, liveOwnerPid: live ? owner.owner.pid : null,
    noRestart: options.noRestart, steps: steps.map((s) => s.step) };
  if (options.dryRun) {
    if (options.json) console.log(JSON.stringify({ status: 'dry_run', ...plan }, null, 2));
    else {
      console.log(`manager-release: dry run (nothing is changed)`);
      console.log(`  repo=${repo}`);
      console.log(`  root=${root}`);
      console.log(`  revision=${revision}`);
      console.log(`  entrypoint=${cli}`);
      console.log(`  liveOwnerPid=${plan.liveOwnerPid ?? 'none'}`);
      for (const step of steps) console.log(`  plan: ${step.step}`);
    }
    return;
  }

  const executed = [];
  try {
    const packageJson = packageDir;
    if (!existsSync(join(packageJson, 'node_modules'))) { run(npm, ['ci', '--ignore-scripts'], { cwd: packageJson }); executed.push('npm-ci'); }
    run(npm, ['run', 'build'], { cwd: packageJson }); executed.push('npm-build');
    if (!existsSync(cli)) throw new Error(`build did not produce ${cli}`);

    if (live) { run(process.execPath, [cli, 'stop', '--root', root]); executed.push('stop'); }
    else if (owner.present) { run(process.execPath, [cli, 'recover', '--root', root]); executed.push('recover'); }

    const previous = readJsonBounded(releaseFile).value;
    if (existsSync(releaseFile)) {
      const backup = `${releaseFile}.before-${new Date().toISOString().replace(/[:.]/g, '-')}`;
      copyFileSync(releaseFile, backup); executed.push('backup');
    }
    const record = { version: 1, mergeCommit: revision, headSha: revision, sourceWorktree: repo,
      serviceRoot: root, entrypoint: cli, at: new Date().toISOString() };
    if (previous && previous.taskId !== undefined) record.taskId = previous.taskId;
    const tmp = join(root, `.release.json.${randomUUID()}.tmp`);
    writeFileSync(tmp, `${JSON.stringify(record, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
    renameSync(tmp, releaseFile); executed.push('write-release');

    let started = null;
    if (!options.noRestart) { started = run(process.execPath, [cli, 'start', '--root', root]); executed.push('start'); }
    let status = null;
    try { status = JSON.parse(run(process.execPath, [cli, 'status', '--root', root])); } catch { /* status is best-effort evidence */ }
    const result = { status: 'activated', ...plan, executed, started: started ? 'confirmed' : 'skipped', service: status };
    if (options.json) console.log(JSON.stringify(result, null, 2));
    else console.log(`manager-release: activated revision=${revision} root=${root} entrypoint=${cli} steps=${executed.join(',')}`);
  } catch (error) {
    const detail = { status: 'failed', ...plan, executed, error: (error.stderr || error.message || String(error)).toString().trim().slice(0, 2000) };
    if (options.json) console.log(JSON.stringify(detail, null, 2));
    else console.error(`manager-release: failed after [${executed.join(',')}]: ${detail.error}`);
    process.exit(1);
  }
}

main(process.argv.slice(2));
