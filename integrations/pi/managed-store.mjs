import { existsSync, mkdirSync, readdirSync, readFileSync, lstatSync, writeFileSync, renameSync, rmSync } from 'node:fs';
import { dirname, join, resolve, relative, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { randomUUID } from 'node:crypto';
import { digest, privateRoot, readJson, validateId } from './store.mjs';

export function inside(parent, child) {
  const path = relative(resolve(parent), resolve(child));
  return path !== '..' && !path.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) && !isAbsolute(path);
}
export function managedDir(root, id, create = false) {
  id = validateId(id);
  const base = join(resolve(root), 'managed'), dir = join(base, id);
  if (create) privateRoot(root);
  for (const path of [base, dir]) {
    if (create) mkdirSync(path, { recursive: true, mode: 0o700 });
    if (existsSync(path) && (!lstatSync(path).isDirectory() || lstatSync(path).isSymbolicLink())) throw new Error('Managed directory must not be a link');
  }
  return dir;
}
export function verifyRelease(release) {
  if (!release || !/^[0-9a-f]{64}$/.test(release.id) || !isAbsolute(release.path)) throw new Error('Invalid runtime release');
  const manifest = readJson(join(release.path, 'release.json'));
  if (!manifest || manifest.id !== release.id || digest(JSON.stringify(manifest.files)) !== release.id) throw new Error('Runtime release manifest mismatch');
  for (const [name, hash] of Object.entries(manifest.files)) {
    if (!/^[a-z0-9.-]+$/.test(name)) throw new Error('Invalid runtime filename');
    const path = join(release.path, name), stat = lstatSync(path);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024 || digest(readFileSync(path)) !== hash) throw new Error(`Runtime file mismatch: ${name}`);
  }
  return manifest;
}
export function installRuntime(root, source = dirname(fileURLToPath(import.meta.url))) {
  privateRoot(root);
  const releases = join(resolve(root), 'releases');
  mkdirSync(releases, { recursive: true, mode: 0o700 });
  if (lstatSync(releases).isSymbolicLink()) throw new Error('Release directory must not be a link');
  const names = readdirSync(source).filter((name) => name === 'package.json' || name.endsWith('.ps1') ||
    (name.endsWith('.mjs') && !name.endsWith('.test.mjs') && !name.includes('smoke'))).sort();
  const files = {}, bytes = new Map();
  for (const name of names) {
    const path = join(source, name), stat = lstatSync(path);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024 * 1024) throw new Error('Invalid runtime source file');
    const content = readFileSync(path); files[name] = digest(content); bytes.set(name, content);
  }
  if (!files['managed-runner.mjs'] || !files['extension.mjs'] || !files['package.json']) throw new Error('Incomplete runtime source');
  const id = digest(JSON.stringify(files)), target = join(releases, id);
  const release = { id, path: target, version: JSON.parse(bytes.get('package.json').toString()).version };
  if (!existsSync(target)) {
    const stage = join(releases, `.staging-${randomUUID()}`);
    mkdirSync(stage, { mode: 0o700 });
    try {
      for (const [name, content] of bytes) writeFileSync(join(stage, name), content, { flag: 'wx', mode: 0o600 });
      writeFileSync(join(stage, 'release.json'), JSON.stringify({ version: 1, id, files }, null, 2), { flag: 'wx', mode: 0o600 });
      try { renameSync(stage, target); }
      catch (error) { if (!existsSync(target)) throw error; }
    } finally {
      if (existsSync(stage) && inside(releases, stage)) rmSync(stage, { recursive: true, force: true });
    }
  }
  verifyRelease(release);
  return release;
}
export function findPiEntry(explicit) {
  const packagePath = '@earendil-works/pi-coding-agent';
  const candidates = explicit ? [resolve(explicit)] : process.env.EDDA_PI_ENTRY ? [resolve(process.env.EDDA_PI_ENTRY)] : [
    join(dirname(process.execPath), 'node_modules', packagePath, 'dist/bundle/cli.js'),
    join(dirname(process.execPath), '../lib/node_modules', packagePath, 'dist/bundle/cli.js'),
  ];
  if (!explicit && !process.env.EDDA_PI_ENTRY) {
    try { candidates.unshift(join(dirname(createRequire(import.meta.url).resolve(packagePath)), 'bundle/cli.js')); } catch { /* optional local package */ }
  }
  for (const entry of candidates) {
    if (!existsSync(entry) || !lstatSync(entry).isFile()) continue;
    const metadata = readJson(join(dirname(dirname(dirname(entry))), 'package.json'));
    if (metadata?.name === packagePath) return { entry: resolve(entry), version: metadata.version };
  }
  throw new Error('Pi installation not found; supply --pi-entry with its installed dist/bundle/cli.js');
}
export function alive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try { process.kill(pid, 0); return true; }
  catch (error) { return error.code !== 'ESRCH'; }
}

// Pi RPC is LF-only JSONL: Unicode U+2028/U+2029 inside strings are not delimiters.
export function rpcFrames(onMessage, onError, maxBytes = 4 * 1024 * 1024) {
  let buffer = Buffer.alloc(0), failed = false;
  return (chunk) => {
    if (failed) return;
    buffer = Buffer.concat([buffer, chunk]);
    let end;
    while ((end = buffer.indexOf(10)) >= 0) {
      const line = buffer.subarray(0, end); buffer = buffer.subarray(end + 1);
      if (line.length > maxBytes) { failed = true; onError(new Error('Pi RPC frame exceeds limit')); return; }
      if (!line.length) continue;
      try { onMessage(JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(line))); }
      catch (error) { failed = true; onError(new Error(`Invalid Pi RPC frame: ${error.name}`)); return; }
    }
    if (buffer.length > maxBytes) { failed = true; onError(new Error('Pi RPC frame exceeds limit')); }
  };
}
