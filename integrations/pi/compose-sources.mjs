import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { mkdir, open, realpath, stat, readFile, writeFile, lstat } from 'node:fs/promises';
import { resolve, join, relative, isAbsolute } from 'node:path';
import { digest, privateRoot } from './store.mjs';

const maxSourceBytes = 262144;
const exec = promisify(execFile);
export function taskId(value) {
  if (typeof value !== 'string' || !/^(0|[1-9][0-9]*)$/.test(value) || !Number.isSafeInteger(Number(value))) {
    throw new Error('Task ID must be a canonical nonnegative safe integer');
  }
  return value;
}

export async function readTask(project, id, command = { file: process.env.EDDA_BIN || 'edda', args: [] }, signal) {
  taskId(id);
  project = await realpath(resolve(project));
  if (!(await stat(project)).isDirectory()) throw new Error('Project must be a directory');
  const result = await exec(command.file, [...command.args, 'task', 'show', id, '--json'], {
    cwd: project, windowsHide: true, timeout: 15000, maxBuffer: maxSourceBytes, encoding: 'utf8', signal,
  });
  const raw = result.stdout;
  const value = JSON.parse(raw);
  if (!value || Array.isArray(value) || !Number.isSafeInteger(value.task_id) || String(value.task_id) !== id ||
    typeof value.title !== 'string' || !value.title.trim() ||
    typeof value.created_event_id !== 'string' || !value.created_event_id.trim() ||
    !['ready', 'blocked', 'running', 'done', 'failed'].includes(value.status) ||
    !Array.isArray(value.scope_paths) || value.scope_paths.some((p) => typeof p !== 'string' || !p.trim()) ||
    !Array.isArray(value.after) || value.after.some((n) => !Number.isSafeInteger(n) || n < 0)) {
    throw new Error('Edda task JSON has invalid identity or required fields');
  }
  return { project, task: value, raw };
}

export async function readBoundedFile(path) {
  const canonical = await realpath(resolve(path));
  const file = await open(canonical, 'r');
  try {
    const metadata = await file.stat();
    if (!metadata.isFile() || metadata.size > maxSourceBytes) throw new Error('Management source must be a regular file no larger than 256 KiB');
    const bytes = Buffer.alloc(metadata.size + 1);
    let total = 0;
    while (total < bytes.length) {
      const { bytesRead } = await file.read(bytes, total, bytes.length - total, total);
      if (!bytesRead) break;
      total += bytesRead;
    }
    const after = await file.stat();
    if (total !== metadata.size || after.size !== metadata.size || after.mtimeMs !== metadata.mtimeMs) {
      throw new Error('Management source changed while reading; retry with a stable file');
    }
    return { path: canonical, text: new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes.subarray(0, total)) };
  } finally { await file.close(); }
}

export async function projectBrief(project, ref) {
  if (typeof ref !== 'string' || !ref.trim()) return { status: 'absent' };
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(ref)) return { status: 'url_not_fetched' };
  let path;
  try { path = await realpath(resolve(project, ref)); }
  catch (error) {
    if (['ENOENT', 'ENOTDIR', 'EINVAL', 'ENAMETOOLONG'].includes(error.code)) return { status: 'inline_or_missing' };
    throw error;
  }
  const offset = relative(project, path);
  if (offset === '..' || offset.startsWith(`..${process.platform === 'win32' ? '\\' : '/'}`) || isAbsolute(offset)) {
    return { status: 'outside_project_not_read' };
  }
  return { status: 'read', ...await readBoundedFile(path) };
}

export async function sourceCache(root) {
  privateRoot(root);
  const directory = join(root, 'sources');
  await mkdir(directory, { recursive: true, mode: 0o700 });
  if ((await lstat(directory)).isSymbolicLink()) throw new Error('Source cache must not be a symlink');
  return async (text, extension = 'json') => {
    if (Buffer.byteLength(text) > maxSourceBytes) throw new Error('Source snapshot exceeds 256 KiB');
    const hash = digest(text);
    const path = join(directory, `${hash}.${extension}`);
    try { await writeFile(path, text, { flag: 'wx', mode: 0o600 }); }
    catch (error) {
      if (error.code !== 'EEXIST') throw error;
      const meta = await lstat(path);
      if (!meta.isFile() || meta.isSymbolicLink() || meta.size > maxSourceBytes || digest(await readFile(path, 'utf8')) !== hash) {
        throw new Error('Existing source snapshot does not match its digest');
      }
    }
    return { uri: path, revision: `sha256:${hash}` };
  };
}
