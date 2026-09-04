#!/usr/bin/env node
// Review elapsed evidence from the same pi JSONL file used for model observation.
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

export function sessionElapsed(raw) {
  const messages = [];
  try {
    for (const line of raw.split(/\r?\n/).filter(line => line.trim())) {
      const row = JSON.parse(line);
      if (row.type !== 'message') continue;
      const value = row.timestamp ?? row.message?.timestamp;
      const timestamp = typeof value === 'number' ? value
        : typeof value === 'string' ? Date.parse(value) : NaN;
      if (!Number.isFinite(timestamp)) return { elapsed_ms: null, elapsed_measured: false };
      messages.push(timestamp);
    }
  } catch {
    return { elapsed_ms: null, elapsed_measured: false };
  }
  const elapsed = messages.length > 1 ? messages.at(-1) - messages[0] : NaN;
  return Number.isSafeInteger(elapsed) && elapsed >= 0
    ? { elapsed_ms: elapsed, elapsed_measured: true }
    : { elapsed_ms: null, elapsed_measured: false };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv.includes('--help')) {
    console.log('Usage: node scripts/pi-session-elapsed.mjs <pi-session.jsonl>');
  } else {
    let result;
    try { result = sessionElapsed(readFileSync(process.argv[2], 'utf8')); }
    catch { result = { elapsed_ms: null, elapsed_measured: false }; }
    console.log(JSON.stringify(result));
  }
}
