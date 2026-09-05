// Contract tests against a REAL built edda binary on a temp repo with an
// isolated store (EDDA_STORE_ROOT). Skipped unless EDDA_BIN is set — CI sets
// it after building the binary; see ../..//../.github/workflows/contract.yml.
//
// Also emits a normalized scenario transcript when EDDA_SCENARIO_OUT is set,
// which the cross-language runner compares byte-wise against the Python SDK's
// transcript (structural equivalence, contract §7).

import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { dirname } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
import { spawnSync } from "node:child_process";
import { createServer } from "node:http";
import { EddaClient, OPERATIONS } from "../src/client.js";
import { HttpTransport } from "../src/transport-http.js";
import { CapabilityNotAvailable, HttpWriteRefused, TimeoutError, CancelledError } from "../src/errors.js";

const EDDA_BIN = process.env.EDDA_BIN ?? "";

function makeEnv(): { root: string; storeRoot: string; env: Record<string, string> } {
  const root = mkdtempSync(join(tmpdir(), "edda-contract-ts-"));
  const storeRoot = join(root, "store");
  const env = { EDDA_STORE_ROOT: storeRoot };
  const r = spawnSync(EDDA_BIN, ["init"], { cwd: root, env, encoding: "utf8" });
  if (r.status !== 0) {
    throw new Error(`edda init failed: ${r.stderr}`);
  }
  return { root, storeRoot, env };
}

test("contract: capabilities probe is honest about gaps", { skip: !EDDA_BIN && "EDDA_BIN not set" }, async () => {
  const { root, storeRoot } = makeEnv();
  const { client, cleanup } = await makeClient(root, storeRoot);
  try {
    const caps = await client.capabilities({ timeoutMs: 20000 });
    // Every contracted operation is now exposed as an MCP tool:
    for (const op of OPERATIONS) {
      assert.equal(caps[op], true, `capability ${op} should be available`);
    }
  } finally {
    await cleanup();
  }
});

test("contract: ask/note/decide round-trip over MCP", { skip: !EDDA_BIN && "EDDA_BIN not set" }, async () => {
  const { root, storeRoot } = makeEnv();
  const { client, cleanup } = await makeClient(root, storeRoot);
  try {
    await client.call("note", { note: "contract-test note ts" }, { timeoutMs: 20000 });
    await client.call("decide", { key: "sdk.contract.ts", value: "ok", reason: "round-trip" }, { timeoutMs: 20000 });
    const ask = (await client.call("ask", { query: "sdk.contract.ts" }, { timeoutMs: 20000 })) as {
      decisions?: Array<Record<string, unknown>>;
    };
    assert.ok(Array.isArray(ask.decisions));
    assert.ok(ask.decisions.some((d) => String(d.key ?? d.decision ?? "").includes("sdk.contract.ts") || JSON.stringify(d).includes("sdk.contract.ts")));
  } finally {
    await cleanup();
  }
});

test("contract: timeout and cancellation are typed, deterministic", { skip: !EDDA_BIN && "EDDA_BIN not set" }, async () => {
  // Deterministic: a synthetic MCP server that delays every response — no
  // race against a fast real CLI. Deadline, cancellation and child-reaping
  // are asserted against the REAL transport machinery; the real edda
  // round-trip stays in the live tests above.
  const synth = join(here, "synth-mcp-server.mjs");
  const client = new EddaClient({ mcp: { command: process.execPath, args: [synth, "--delay-ms", "30000"] } });
  try {
    await assert.rejects(
      client.call("ask", { query: "timeout-probe" }, { timeoutMs: 300 }),
      (err: unknown) => (err as { kind?: string }).kind === "Timeout",
    );
    const ac = new AbortController();
    setTimeout(() => ac.abort(), 300);
    await assert.rejects(
      client.call("ask", { query: "cancel-probe" }, { signal: ac.signal, timeoutMs: 30000 }),
      (err: unknown) => (err as { kind?: string }).kind === "Cancelled",
    );
  } finally {
    await client.close();
  }
});

test("contract: dead child surfaces TransportError and close reaps", async () => {
  const synth = join(here, "synth-mcp-server.mjs");
  const client = new EddaClient({ mcp: { command: process.execPath, args: [synth, "--exit-immediately"] } });
  await assert.rejects(
    client.call("ask", { query: "error-probe" }, { timeoutMs: 10000 }),
    (err: unknown) => (err as { kind?: string }).kind === "TransportError",
  );
  await client.close();
});

test("HTTP transport sends configured bearer auth to local remote middleware", async () => {
  let authorization: string | undefined;
  const server = createServer((req, res) => {
    authorization = req.headers.authorization;
    res.writeHead(200, { "content-type": "application/json" });
    res.end('{"ok":true}');
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  try {
    const address = server.address();
    assert.ok(address && typeof address !== "string");
    const http = new HttpTransport(`http://127.0.0.1:${address.port}`, 1_000, "test-token");
    assert.deepEqual(await http.health(), { ok: true });
    assert.equal(authorization, "Bearer test-token");
  } finally {
    await new Promise<void>((resolve, reject) => server.close((error) => (error ? reject(error) : resolve())));
  }
});

test("contract: HTTP transport is read-only and reads work", { skip: !EDDA_BIN && "EDDA_BIN not set" }, async () => {
  const { root, storeRoot } = makeEnv();
  const port = 17400 + Math.floor(Math.random() * 500);
  const proc = await import("node:child_process").then(({ spawn }) =>
    spawn(EDDA_BIN, ["serve", "--port", String(port)], {
      cwd: root,
      env: { ...process.env, EDDA_STORE_ROOT: storeRoot },
      stdio: "ignore",
      windowsHide: true,
    }),
  );
  const http = new HttpTransport(`http://127.0.0.1:${port}`);
  try {
    // wait for readiness
    let up = false;
    for (let i = 0; i < 50 && !up; i++) {
      await new Promise((r) => setTimeout(r, 200));
      try {
        await http.health({ timeoutMs: 1000 });
        up = true;
      } catch {}
    }
    assert.ok(up, "edda serve did not become healthy");
    await http.status({ timeoutMs: 5000 });
    await http.decisions("", { timeoutMs: 5000 });
    // Writes refused by construction (§4):
    await assert.rejects(
      (http as unknown as { request: (m: string, p: string) => Promise<unknown> }).request("POST", "/api/note"),
      (err: unknown) => err instanceof HttpWriteRefused,
    );
  } finally {
    proc.kill();
    let exited = false;
    await Promise.race([
      new Promise<void>((resolve) => proc.once("exit", () => { exited = true; resolve(); })),
      new Promise<void>((resolve) => setTimeout(resolve, 2000)),
    ]);
    if (!exited && proc.exitCode === null) {
      proc.kill("SIGKILL");
      await new Promise<void>((resolve) => proc.once("exit", resolve));
    }
    rmSync(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
  }
});

test("contract: task new/start/done + receipt + verify over MCP", { skip: !EDDA_BIN && "EDDA_BIN not set" }, async () => {
  const { root, storeRoot } = makeEnv();
  const { client, cleanup } = await makeClient(root, storeRoot);
  try {
    const created = (await client.call("task.new", { title: "contract task", idempotency_key: "ts-1" }, { timeoutMs: 20000 })) as {
      task_id: number;
      status: string;
      deduped: boolean;
    };
    assert.equal(created.task_id, 1);
    assert.equal(created.deduped, false);
    // idempotency: same key reuses, never twins
    const again = (await client.call("task.new", { title: "contract task", idempotency_key: "ts-1" }, { timeoutMs: 20000 })) as {
      task_id: number;
      deduped: boolean;
    };
    assert.equal(again.task_id, 1);
    assert.equal(again.deduped, true);
    // start before done (start/done pairing enforced by the shared state machine)
    await assert.rejects(
      client.call("task.done", { id: 1, receipt: "premature" }, { timeoutMs: 20000 }),
      /not been started/,
    );
    await client.call("task.start", { id: created.task_id }, { timeoutMs: 20000 });
    const done = (await client.call(
      "task.done",
      { id: created.task_id, receipt: "contract receipt ts", evidence_paths: ["sdk/ts"] },
      { timeoutMs: 20000 },
    )) as { unlocked: unknown[] };
    assert.ok(Array.isArray(done.unlocked));
    const receipt = (await client.call("receipt", { task_id: 1 }, { timeoutMs: 20000 })) as {
      receipt: string;
      status: string;
    };
    assert.equal(receipt.receipt, "contract receipt ts");
    assert.equal(receipt.status, "done");
    const verify = (await client.call("verify", {}, { timeoutMs: 20000 })) as {
      ok: boolean;
      events: number;
    };
    assert.equal(verify.ok, true);
    assert.ok(verify.events > 0);
  } finally {
    await cleanup();
  }
});

test("contract: claim over MCP writes and reads back", { skip: !EDDA_BIN && "EDDA_BIN not set" }, async () => {
  const { root, storeRoot } = makeEnv();
  const { client, cleanup } = await makeClient(root, storeRoot);
  try {
    const claim = (await client.call(
      "claim",
      { label: "ts-scope", paths: ["sdk/ts/*"], session: "ts-contract-session" },
      { timeoutMs: 20000 },
    )) as { label: string; replaced: null };
    assert.equal(claim.label, "ts-scope");
    assert.equal(claim.replaced, null);
  } finally {
    await cleanup();
  }
});

// ── scenario transcript for cross-language equivalence ──

test("scenario: normalized transcript", { skip: !EDDA_BIN && "EDDA_BIN not set" }, async () => {
  const { root, storeRoot } = makeEnv();
  const { client, cleanup } = await makeClient(root, storeRoot);
  try {
    await client.call("note", { note: "scenario note" }, { timeoutMs: 20000 });
    await client.call("decide", { key: "sdk.scenario.alpha", value: "one" }, { timeoutMs: 20000 });
    await client.call("decide", { key: "sdk.scenario.beta", value: "two", reason: "r" }, { timeoutMs: 20000 });
    // task flow through the shared state machine
    await client.call("task.new", { title: "scenario task", idempotency_key: "scenario-1" }, { timeoutMs: 20000 });
    await client.call("task.start", { id: 1 }, { timeoutMs: 20000 });
    await client.call("task.done", { id: 1, receipt: "scenario receipt", evidence_paths: ["evidence"] }, { timeoutMs: 20000 });
    const receipt = (await client.call("receipt", { task_id: 1 }, { timeoutMs: 20000 })) as { receipt: string; status: string };
    await client.call("claim", { label: "scenario-scope", paths: ["sdk/*"] }, { timeoutMs: 20000 });
    const verify = (await client.call("verify", {}, { timeoutMs: 20000 })) as { ok: boolean; events: number };
    const ask = (await client.call("ask", { query: "sdk.scenario" }, { timeoutMs: 20000 })) as {
      decisions?: Array<Record<string, unknown>>;
    };
    const caps = await client.capabilities({ timeoutMs: 20000 });
    const transcript = {
      sdk: "ts",
      capabilities: caps,
      decisions: (ask.decisions ?? [])
        .map((d) => ({ key: d.key ?? null, value: d.value ?? null }))
        .sort((a, b) => String(a.key).localeCompare(String(b.key))),
      task: { task_id: 1, receipt: receipt.receipt, status: receipt.status },
      verify: verify,
    };
    const out = process.env.EDDA_SCENARIO_OUT;
    if (out) {
      const { writeFileSync } = await import("node:fs");
      writeFileSync(out, JSON.stringify(transcript, null, 2));
    }
    assert.ok(Array.isArray(transcript.decisions));
  } finally {
    await cleanup();
  }
});

// ── helpers ──

async function makeClient(
  root: string,
  storeRoot: string,
): Promise<{ client: EddaClient; cleanup: () => Promise<void> }> {
  const client = new EddaClient({
    mcp: {
      command: EDDA_BIN,
      args: ["mcp", "serve"],
      cwd: root,
      env: { EDDA_STORE_ROOT: storeRoot },
    },
  });
  const cleanup = async () => {
    await client.close();
    rmSync(root, { recursive: true, force: true, maxRetries: 3, retryDelay: 200 });
  };
  return { client, cleanup };
}
