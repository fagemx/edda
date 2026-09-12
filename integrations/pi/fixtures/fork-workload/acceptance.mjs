import assert from 'node:assert/strict';
import { pathToFileURL } from 'node:url';
import { join } from 'node:path';
const [cwd, only = 'all'] = process.argv.slice(2);
let checks = 0;
const eq = (a, b) => { assert.deepEqual(a, b); checks++; };
const load = (name) => import(pathToFileURL(join(cwd, `${name}.mjs`)));
const now = Date.parse('2026-09-12T12:00:00Z');
const config = { id: 'z', project: '/project', tasks: [{ id: 1 }, { id: 2 }, { id: 1 }, { id: '1' }] };
if (only === 'all' || only === 'cards') {
  const { normalizeCard: f } = await load('cards');
  const expected = { id: 'z', project: '/project', phase: 'unknown', stale: true, needsAttention: false,
    counts: { total: 3, ready: 0, running: 0, done: 0, unknown: 3 } };
  eq(f(config, null, now), expected);
  const state = { phase: 'running', updatedAt: '2026-09-12T11:59:00Z', tasks: [
    { id: 1, status: 'done' }, { id: 2, status: 'ready' }, { id: 1, status: 'running' },
    { id: '1', status: 'done' }, { id: 999, status: 'running' }] };
  const before = JSON.stringify({ config, state });
  eq(f(config, state, now), { ...expected, phase: 'running', stale: false,
    counts: { total: 3, ready: 1, running: 1, done: 1, unknown: 0 } });
  eq(JSON.stringify({ config, state }), before);
  for (const [updatedAt, stale] of [[null, true], [0, true], ['nonsense', true], ['2026-09-12T12:00:01Z', true],
    ['2026-09-12T11:58:59Z', true], ['2026-09-12T12:00:00Z', false]]) eq(f(config, { ...state, updatedAt }, now).stale, stale);
  for (const phase of ['starting', 'running', 'paused', 'tasks_done', 'failed', 'bogus']) {
    eq(f(config, { ...state, phase }).phase, phase === 'bogus' ? 'unknown' : phase);
    eq(f(config, { ...state, phase }, now).needsAttention, phase === 'failed');
  }
  eq(f(config, { ...state, needsAttention: 'true' }, now).needsAttention, false);
  eq(f(config, { ...state, needsAttention: true }, now).needsAttention, true);
  eq(f(config, { ...state, tasks: [{ id: 1, status: 'cancelled' }] }, now).counts, expected.counts);
  eq(f({ ...config, tasks: [] }, state, now).counts.total, 0);
  eq(f(config, state, now, 59999).stale, true);
}
if (only === 'all' || only === 'attention') {
  const { rankCards: f } = await load('attention');
  const card = (id, patch = {}) => ({ id, needsAttention: false, stale: false, phase: 'running', counts: { running: 0 }, ...patch });
  const input = [card('z'), card('b', { stale: true }), card('c', { counts: { running: 8 } }),
    card('d', { phase: 'failed' }), card('e', { needsAttention: true }), card('A')];
  const before = JSON.stringify(input);
  eq(f(input).map((c) => c.id), ['e', 'd', 'b', 'c', 'A', 'z']);
  eq(JSON.stringify(input), before);
  const replacement = card('z', { needsAttention: true });
  eq(f([input[0], replacement]).length, 1);
  eq(f([input[0], replacement])[0] === replacement, true);
  eq(f([]), []);
  eq(f([card('a', { stale: 'true' }), card('B')]).map((c) => c.id), ['B', 'a']);
  for (let n = 0; n < 8; n++) {
    const cards = Array.from({ length: 8 }, (_, i) => card(String(i), { counts: { running: (i + n) % 8 } }));
    eq(f(cards).map((c) => c.counts.running), [7, 6, 5, 4, 3, 2, 1, 0]);
  }
}
if (only === 'all' || only === 'overview') {
  const { renderOverview: f } = await load('overview');
  const cards = [{ id: '中文🌱', data: 'x'.repeat(30) }, { id: 'b' }, { id: 'c' }];
  const before = JSON.stringify(cards);
  const envelope = (n) => JSON.stringify({ total: 3, shown: n, hasMore: n < 3, cards: cards.slice(0, n) });
  const min = Buffer.byteLength(envelope(0)), max = Buffer.byteLength(envelope(3));
  for (let budget = min; budget <= max + 2; budget++) {
    let n = 0;
    for (let k = 1; k <= 3; k++) if (Buffer.byteLength(envelope(k)) <= budget) n = k;
    eq(f(cards, budget), envelope(n));
  }
  for (const budget of [-1, 1.5, NaN, Infinity, '100', Number.MAX_SAFE_INTEGER + 1, min - 1]) {
    assert.throws(() => f(cards, budget), RangeError); checks++;
  }
  eq(f([], 100), '{"total":0,"shown":0,"hasMore":false,"cards":[]}');
  eq(JSON.stringify(cards), before);
  eq(JSON.parse(f([{ id: 'x'.repeat(1000) }, { id: 'small' }], 100)).shown, 0);
}
if (only === 'all') {
  const { normalizeCard } = await load('cards');
  const { rankCards } = await load('attention');
  const { renderOverview } = await load('overview');
  const cards = [normalizeCard(config, { phase: 'failed', updatedAt: new Date(now).toISOString() }, now),
    normalizeCard({ id: 'a', project: '/a', tasks: [] }, { phase: 'tasks_done', updatedAt: new Date(now).toISOString() }, now)];
  const overview = JSON.parse(renderOverview(rankCards(cards), 10000));
  eq(overview.cards.map((c) => c.id), ['z', 'a']);
  eq(overview.cards[1].needsAttention, false);
  eq(overview.hasMore, false);
}
console.log(JSON.stringify({ passed: true, checks, bundle: only }));
