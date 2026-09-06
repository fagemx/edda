// Deterministic synthetic MCP server for deadline/cancellation/error tests
// (newline-delimited JSON-RPC 2.0 over stdio). Answers `initialize`
// immediately, then delays every other request by --delay-ms before replying.
// --exit-immediately exits at startup — the error-path probe.
//
// TEST fixture: deterministic timing so deadline/cancellation assertions
// cannot race a fast real server. Real edda round-trip/equivalence coverage
// stays in the live contract tests.

import args from "node:process";

const argv = args.argv.slice(2);
const flag = (name) => argv.includes(name);
const num = (name, fallback) => {
  const i = argv.indexOf(name);
  return i >= 0 ? Number(argv[i + 1]) : fallback;
};

if (flag("--exit-immediately")) process.exit(3);
const delayMs = num("--delay-ms", null);

import readline from "node:readline";
const rl = readline.createInterface({ input: process.stdin });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

rl.on("line", (line) => {
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    return;
  }
  if (msg.method === "initialize") {
    process.stdout.write(
      JSON.stringify({
        jsonrpc: "2.0",
        id: msg.id,
        result: {
          protocolVersion: "2025-03-26",
          capabilities: { tools: {} },
          serverInfo: { name: "synth", version: "0" },
        },
      }) + "\n",
    );
    return;
  }
  if (msg.id === null || msg.id === undefined) return; // notification
  void (async () => {
    if (delayMs != null) await sleep(delayMs);
    process.stdout.write(
      JSON.stringify({
        jsonrpc: "2.0",
        id: msg.id,
        result: { content: [{ type: "text", text: "synth" }] },
      }) + "\n",
    );
  })();
});
