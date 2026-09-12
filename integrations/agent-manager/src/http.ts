import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { timingSafeEqual } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join } from 'node:path';
import { AgentManager } from './manager.js';
import { ManagerError, MAX_BODY_BYTES } from './contracts.js';
import { parseSend, uuid } from './config.js';

function authorized(header: string | undefined, token: string): boolean {
  const expected = Buffer.from(`Bearer ${token}`), value = Buffer.from(header || '');
  return expected.length === value.length && timingSafeEqual(expected, value);
}
async function body(req: IncomingMessage): Promise<unknown> {
  if (Number(req.headers['content-length'] || 0) > MAX_BODY_BYTES) throw new ManagerError('TOO_LARGE', '請縮短訊息後再傳送。', 413);
  if (!req.headers['content-type']?.toLowerCase().startsWith('application/json')) throw new ManagerError('INVALID_REQUEST', '請使用 JSON 格式。', 400);
  const buffer = await new Promise<Buffer>((resolve, reject) => {
    const chunks: Buffer[] = []; let length = 0, overflow = false;
    req.on('data', (chunk: Buffer) => {
      if (overflow) return;
      length += chunk.length;
      if (length > MAX_BODY_BYTES) { overflow = true; chunks.length = 0; reject(new ManagerError('TOO_LARGE', '請縮短訊息後再傳送。', 413)); }
      else chunks.push(chunk);
    });
    req.once('end', () => { if (!overflow) resolve(Buffer.concat(chunks)); });
    req.once('error', reject);
    req.once('aborted', () => reject(new ManagerError('INVALID_REQUEST', '請求已中斷。')));
  });
  try { return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(buffer)) as unknown; }
  catch { throw new ManagerError('INVALID_REQUEST', '無法解析這則請求。'); }
}
export async function serve(manager: AgentManager, token: string, options: { port?: number; assetsRoot?: string; onStop?: () => void } = {}) {
  if (!/^[a-f0-9]{64}$/.test(token)) throw new Error('Invalid gateway token');
  const root = options.assetsRoot || fileURLToPath(new URL('../../', import.meta.url));
  const assets = new Map([
    ['/', { file: join(root, 'src/web/index.html'), type: 'text/html; charset=utf-8' }],
    ['/app.js', { file: join(root, 'dist/src/web/app.js'), type: 'text/javascript; charset=utf-8' }],
    ['/contracts.js', { file: join(root, 'dist/src/contracts.js'), type: 'text/javascript; charset=utf-8' }],
    ['/style.css', { file: join(root, 'src/web/style.css'), type: 'text/css; charset=utf-8' }],
  ]);
  let origin = '';
  const server = createServer({ maxHeaderSize: 8192 }, (req, res) => { void handle(req, res); });
  server.requestTimeout = 15000; server.headersTimeout = 10000; server.keepAliveTimeout = 5000;
  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    res.setHeader('Cache-Control', 'no-store'); res.setHeader('Referrer-Policy', 'no-referrer');
    res.setHeader('X-Content-Type-Options', 'nosniff'); res.setHeader('X-Frame-Options', 'DENY');
    res.setHeader('Content-Security-Policy', "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'");
    const json = (status: number, value: unknown): void => { res.writeHead(status, { 'content-type': 'application/json; charset=utf-8' }); res.end(JSON.stringify(value)); };
    try {
      if (req.headers.host !== origin.slice(7) || (req.headers.origin && req.headers.origin !== origin) || req.method === 'OPTIONS') throw new ManagerError('FORBIDDEN', '請從本機管理台開啟。', 403);
      if (!req.url || req.url.length > 2048) throw new ManagerError('INVALID_REQUEST', '請求網址過長。');
      const url = new URL(req.url, origin);
      if (url.searchParams.has('token')) throw new ManagerError('INVALID_REQUEST', '登入憑證不能放在網址查詢參數。');
      const asset = assets.get(url.pathname);
      if (req.method === 'GET' && asset) { res.writeHead(200, { 'content-type': asset.type }); res.end(readFileSync(asset.file)); return; }
      if (!authorized(req.headers.authorization, token)) throw new ManagerError('UNAUTHORIZED', '請使用啟動時提供的管理台連結。', 401);
      if (req.method === 'GET' && url.pathname === '/api/overview') { json(200, manager.overview()); return; }
      if (req.method === 'GET' && url.pathname === '/api/service') { json(200, { version: 1, startedAt: manager.startedAt, agents: manager.config.agents.length }); return; }
      const agent = /^\/api\/agents\/([a-zA-Z0-9][a-zA-Z0-9_-]{0,63})\/(conversation|messages)$/.exec(url.pathname);
      if (agent?.[1] && agent[2] === 'conversation' && req.method === 'GET') {
        const after = url.searchParams.get('after');
        if (after && after.length > 200) throw new ManagerError('INVALID_REQUEST', '對話游標過長。');
        json(200, await manager.conversation(agent[1], after || undefined)); return;
      }
      if (agent?.[1] && agent[2] === 'messages' && req.method === 'POST') {
        const request = parseSend(await body(req)); json(202, await manager.send(agent[1], request)); return;
      }
      const operation = /^\/api\/operations\/([a-fA-F0-9-]{36})$/.exec(url.pathname);
      if (req.method === 'GET' && operation?.[1]) { json(200, await manager.operation(uuid(operation[1]))); return; }
      if (req.method === 'POST' && url.pathname === '/api/service/stop' && options.onStop) {
        await body(req); json(200, { status: 'stopping', agentsStopped: false }); setImmediate(options.onStop); return;
      }
      throw new ManagerError('NOT_FOUND', '找不到這個管理項目。', 404);
    } catch (error) {
      if (res.headersSent || res.destroyed) { res.end(); return; }
      const known = error instanceof ManagerError;
      res.setHeader('Connection', 'close'); req.resume();
      json(known ? error.status : 500, { error: { code: known ? error.code : 'INTERNAL', message: known ? error.message : '管理服務暫時無法完成請求；不會自動重送訊息。' } });
    }
  }
  await new Promise<void>((resolve, reject) => { server.once('error', reject); server.listen(options.port ?? 0, '127.0.0.1', resolve); });
  const address = server.address(); if (!address || typeof address === 'string') throw new Error('Missing local address');
  origin = `http://127.0.0.1:${address.port}`;
  return { origin, port: address.port, close: async () => { await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve())); } };
}
