#!/usr/bin/env node
// run.mjs — recall eval runner for queue 325 EDDA-RECALL-EVAL1
//
// Contract:
//   - Read a corpus of JSONL {id, question, expected_key, notes?}.
//   - For each row, resolve expected_key → event_id via `edda ask <key> --json`
//     against the current workspace (missing key ⇒ skipped, honest).
//   - Run `edda ask <question> --json --limit K` and score rank of expected event_id.
//   - Emit deterministic markdown + JSON reports; same corpus + same ledger + same
//     edda binary ⇒ identical output (no timestamps in report body; committed
//     summary line carries the run date separately).
//
// Design notes:
//   - Corpus and runner are separate. Questions are hand-written without reading
//     the target decision's value/reason (anti-overfit); only key/domain used to
//     identify which decision.
//   - Ledger discovery: runs `edda ask` from --workspace (defaults to cwd).
//     Different workspaces ⇒ different event_ids ⇒ skipped rows, not lies.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { rankOf, scoreOne, aggregate } from "./scoring.mjs";

const K = 5;
const DEFAULT_LIMIT = 20;

function parseArgs(argv) {
  const args = { corpus: null, out: null, workspace: process.cwd(), edda: "edda", limit: DEFAULT_LIMIT };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--corpus") args.corpus = argv[++i];
    else if (a === "--out") args.out = argv[++i];
    else if (a === "--workspace") args.workspace = argv[++i];
    else if (a === "--edda") args.edda = argv[++i];
    else if (a === "--limit") args.limit = Number(argv[++i]);
    else if (a === "--help" || a === "-h") {
      console.log("usage: run.mjs --corpus <path.jsonl> --out <dir> [--workspace <path>] [--edda <path>] [--limit <n>]");
      process.exit(0);
    }
  }
  if (!args.corpus || !args.out) {
    console.error("--corpus and --out are required. See --help.");
    process.exit(2);
  }
  return args;
}

function loadCorpus(path) {
  const lines = readFileSync(path, "utf8").split(/\r?\n/).filter((l) => l.trim() && !l.startsWith("#"));
  return lines.map((line, i) => {
    let row;
    try { row = JSON.parse(line); }
    catch { throw new Error(`corpus line ${i + 1}: invalid JSON`); }
    if (!row.id || !row.question || !row.expected_key) {
      throw new Error(`corpus line ${i + 1}: missing id/question/expected_key`);
    }
    return row;
  });
}

function eddaAsk(args, query, extra = []) {
  const out = execFileSync(args.edda, ["ask", query, "--json", "--limit", String(args.limit), ...extra], {
    cwd: args.workspace,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    maxBuffer: 16 * 1024 * 1024,
  });
  return JSON.parse(out);
}

function resolveEventId(args, key) {
  // `edda ask <key>` with exact key input returns matching decisions.
  const res = eddaAsk(args, key);
  const hit = (res.decisions || []).find((d) => d.key === key);
  return hit?.event_id ?? null;
}

function runQuery(args, question) {
  const res = eddaAsk(args, question);
  // Rank against the union of decisions + timeline, keeping decisions-first ordering.
  const seen = new Set();
  const merged = [];
  for (const bucket of [res.decisions ?? [], res.timeline ?? []]) {
    for (const d of bucket) {
      if (d?.event_id && !seen.has(d.event_id)) {
        seen.add(d.event_id);
        merged.push({ event_id: d.event_id, key: d.key });
      }
    }
  }
  return merged;
}

function renderJson(rows, agg, meta) {
  return JSON.stringify({ schema: "edda-recall-eval.v1", ...meta, aggregate: agg, rows }, null, 2);
}

function renderMd(rows, agg, meta) {
  const pct = (v) => (v === null ? "n/a" : (v * 100).toFixed(1) + "%");
  const lines = [
    "# edda decision recall — baseline",
    "",
    `- workspace: \`${meta.workspace_label}\``,
    `- corpus: \`${meta.corpus_label}\` (${rows.length} questions)`,
    `- edda: \`${meta.edda_label}\` limit=${meta.limit}`,
    "",
    "## Aggregate",
    "",
    `- **R@1**: ${pct(agg.r_at_1)}  (${scored(agg, "hit1")}/${agg.n})`,
    `- **R@5**: ${pct(agg.r_at_5)}  (${scored(agg, "hit5")}/${agg.n})`,
    `- misses (target not in top ${meta.limit}): ${agg.misses}`,
    `- skipped (expected_key not in ledger): ${agg.skipped}`,
    "",
    "Honesty rules: skipped rows are excluded from the denominator, so a shrinking",
    "or different ledger cannot inflate the score. Baseline is **BM25-style ask**",
    "(structured domain + keyword timeline), no LLM in the loop.",
    "",
    "## Per-query",
    "",
    "| id | expected_key | rank | @1 | @5 | notes |",
    "|---|---|---|---|---|---|",
  ];
  for (const r of rows) {
    if (r.skipped) {
      lines.push(`| ${r.id} | \`${r.expected_key}\` | skipped | — | — | ${r.notes ?? ""} |`);
    } else {
      const rank = r.hit === null ? `miss` : String(r.hit);
      lines.push(`| ${r.id} | \`${r.expected_key}\` | ${rank} | ${r.hit1 ? "✓" : "·"} | ${r.hit5 ? "✓" : "·"} | ${r.notes ?? ""} |`);
    }
  }
  return lines.join("\n") + "\n";
}

function scored(agg, k) {
  return Math.round((agg[k === "hit1" ? "r_at_1" : "r_at_5"] ?? 0) * agg.n);
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  const corpus = loadCorpus(args.corpus);
  const rows = [];
  for (const row of corpus) {
    const expectedId = resolveEventId(args, row.expected_key);
    if (!expectedId) {
      rows.push({ id: row.id, expected_key: row.expected_key, notes: row.notes, skipped: true, reason: "expected_key not in ledger" });
      continue;
    }
    const returned = runQuery(args, row.question);
    const rank = rankOf(expectedId, returned);
    const s = scoreOne(rank);
    rows.push({
      id: row.id,
      question: row.question,
      expected_key: row.expected_key,
      expected_event_id: expectedId,
      returned_top: returned.slice(0, K).map((x) => x.key),
      notes: row.notes,
      ...s,
    });
  }
  const agg = aggregate(rows);
  const meta = {
    workspace_label: args.workspace.replace(/\\/g, "/").split("/").slice(-2).join("/"),
    corpus_label: args.corpus.replace(/\\/g, "/").split("/").pop(),
    edda_label: args.edda,
    limit: args.limit,
  };
  mkdirSync(args.out, { recursive: true });
  writeFileSync(join(args.out, "results.json"), renderJson(rows, agg, meta) + "\n");
  writeFileSync(join(args.out, "results.md"), renderMd(rows, agg, meta));
  const pct = (v) => (v === null ? "n/a" : (v * 100).toFixed(1) + "%");
  console.log(`R@1=${pct(agg.r_at_1)} R@5=${pct(agg.r_at_5)} n=${agg.n} skipped=${agg.skipped} misses=${agg.misses}`);
}

main();
