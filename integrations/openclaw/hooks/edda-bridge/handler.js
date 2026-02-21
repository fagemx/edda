/**
 * edda-bridge: OpenClaw ↔ Edda integration hook
 *
 * Events handled:
 * - agent:bootstrap   → inject edda context + write-back protocol + coordination into bootstrap
 * - message:sent      → scan agent responses for decisions, auto-record to ledger
 * - message:received  → (stub) log event for verification
 * - command:new       → run `edda commit` to checkpoint before session reset
 * - command:reset     → digest + commit on session reset
 * - command:stop      → digest + commit on session stop
 * - gateway:startup   → log session start to edda ledger
 */

import { execSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import os from "node:os";

// ── Config ──

const EDDA_BIN = process.platform === "win32" ? "edda.exe" : "edda";
const EDDA_DIR = ".edda";
const TAG = "[edda-bridge]";

// Decision patterns — phrases that indicate an architectural/design decision
const DECISION_PATTERNS = [
  /\bdecided?\s+(?:to\s+)?(?:use|go\s+with|switch\s+to|adopt|pick|choose|keep)\s+/i,
  /\bchose\s+(\S+)\s+over\s+/i,
  /\bgoing\s+with\s+/i,
  /\brejected?\s+(\S+)\s+(?:because|since|due\s+to|—)/i,
  /\btrade-?off\s*:/i,
  /\barchitecture\s+decision\s*:/i,
  /\bwe(?:'ll| will)\s+use\s+/i,
  /\binstead\s+of\s+\S+.*\bbecause\b/i,
];

// Minimum message length to consider for decision capture (avoid short chat)
const MIN_MSG_LENGTH = 80;

// Max decisions to capture per message
const MAX_DECISIONS_PER_MSG = 3;

// Write-back protocol — teaches the agent to use edda decide / edda note.
// Matches the Rust bridge's render_write_back_protocol() output.
const WRITE_BACK_PROTOCOL = `## Write-Back Protocol
Record architectural decisions with: \`edda decide "domain.aspect=value" --reason "justification"\`

Examples:
  \`edda decide "db.engine=postgres" --reason "need JSONB for flexible metadata"\`
  \`edda decide "auth.method=JWT" --reason "stateless, scales horizontally"\`
  \`edda decide "api.style=REST" --reason "client SDK compatibility"\`

Do NOT record: formatting changes, test fixes, minor refactors, dependency bumps.

Before ending a session, summarize open context:
  \`edda note "completed X; decided Y; next: Z" --tag session\``;

// ── Helpers ──

function resolveWorkdir(event, hookConfig) {
  if (hookConfig?.workdir) return hookConfig.workdir;
  if (event.context?.workspaceDir) return event.context.workspaceDir;
  return path.join(os.homedir(), ".openclaw", "workspace");
}

function hasEddaInit(workdir) {
  try {
    return existsSync(path.join(workdir, EDDA_DIR));
  } catch {
    return false;
  }
}

function runEdda(args, workdir, timeoutMs = 5000) {
  try {
    const result = execSync(`${EDDA_BIN} ${args.join(" ")}`, {
      cwd: workdir,
      timeout: timeoutMs,
      encoding: "utf-8",
      stdio: ["pipe", "pipe", "pipe"],
      env: { ...process.env },
    });
    return result.trim();
  } catch (err) {
    console.warn(`${TAG} edda ${args[0]} failed:`, err.message || String(err));
    return null;
  }
}

function resolveHookConfig(cfg, hookName) {
  try {
    return cfg?.hooks?.internal?.entries?.[hookName] ?? null;
  } catch {
    return null;
  }
}

function escapeShellArg(str) {
  // Escape for shell — wrap in double quotes, escape inner quotes
  return `"${str.replace(/"/g, '\\"')}"`;
}

function formatCoordinationSection(workdir) {
  const peersOutput = runEdda(["bridge", "claude", "peers"], workdir);
  if (!peersOutput) return "";

  return [
    "## Coordination",
    'Claim your scope: `edda claim "label" --paths "src/scope/*"`',
    'Message a peer: `edda request "peer-label" "your message"`',
    "",
    peersOutput,
  ].join("\n");
}

// ── Bootstrap handler: inject edda context ──

function handleBootstrap(event, hookConfig) {
  const workdir = resolveWorkdir(event, hookConfig);

  if (!hasEddaInit(workdir)) {
    console.log(`${TAG} No ${EDDA_DIR}/ in ${workdir}, skipping context injection`);
    return;
  }

  const totalBudget = hookConfig?.contextBudget ?? 8000;

  // Build tail sections (reserved — never truncated)
  const tailParts = [WRITE_BACK_PROTOCOL];
  const coordination = formatCoordinationSection(workdir);
  if (coordination) tailParts.push(coordination);
  const tail = tailParts.join("\n\n");

  // Build body (truncatable — edda context output)
  let body = runEdda(["context"], workdir) || "";
  const bodyBudget = Math.max(2000, totalBudget - tail.length - 200);
  if (body.length > bodyBudget) {
    body = body.slice(0, bodyBudget) + "\n\n... (truncated to " + bodyBudget + " chars)";
  }

  const content = [
    "# Edda Decision Context",
    "",
    "> Auto-injected by edda-bridge at session bootstrap.",
    "> These are decisions from prior sessions. Reference them to avoid re-litigating.",
    "",
    body,
    "",
    tail,
  ].join("\n");

  const bootstrapFile = {
    name: "EDDA-CONTEXT.md",
    path: path.join(workdir, EDDA_DIR, "context-snapshot.md"),
    content,
    missing: false,
  };

  if (!event.context.bootstrapFiles) {
    event.context.bootstrapFiles = [];
  }
  event.context.bootstrapFiles.push(bootstrapFile);

  console.log(`${TAG} Injected edda context (${content.length} chars, body=${body.length}, tail=${tail.length}) into bootstrap`);
}

// ── Message handler: auto-capture decisions ──

function handleMessageSent(event, hookConfig) {
  const autoCapture = hookConfig?.autoCapture ?? true;
  if (!autoCapture) return;

  const content = event.context?.content;
  if (!content || content.length < MIN_MSG_LENGTH) return;

  const workdir = resolveWorkdir(event, hookConfig);
  if (!hasEddaInit(workdir)) return;

  // Extract sentences that match decision patterns
  const sentences = content.split(/(?<=[.!?\n])\s+/);
  let captured = 0;

  for (const sentence of sentences) {
    if (captured >= MAX_DECISIONS_PER_MSG) break;

    for (const pattern of DECISION_PATTERNS) {
      if (pattern.test(sentence)) {
        // Clean up the sentence for recording
        const cleaned = sentence
          .replace(/\n/g, " ")
          .replace(/\s+/g, " ")
          .trim()
          .slice(0, 200); // cap length

        if (cleaned.length < 20) continue; // too short to be meaningful

        runEdda(
          ["note", escapeShellArg(`[auto] ${cleaned}`)],
          workdir
        );
        captured++;
        console.log(`${TAG} Auto-captured decision: ${cleaned.slice(0, 60)}...`);
        break; // one pattern match per sentence is enough
      }
    }
  }

  if (captured > 0) {
    console.log(`${TAG} Captured ${captured} decision(s) from agent response`);
  }
}

// ── Command:new handler: checkpoint ──

function handleSessionEnd(event, hookConfig) {
  const workdir = resolveWorkdir(event, hookConfig);

  if (!hasEddaInit(workdir)) {
    console.log(`${TAG} No ${EDDA_DIR}/ in ${workdir}, skipping commit`);
    return;
  }

  const source = event.context?.commandSource || "unknown";
  const ts = event.timestamp.toISOString().split("T")[1].split(".")[0];
  const msg = `session: /new via ${source} at ${ts}`;

  const result = runEdda(["commit", "-m", escapeShellArg(msg)], workdir);

  if (result) {
    console.log(`${TAG} Committed: ${result}`);
  }
}

// ── Session boundary handler: digest + commit on reset/stop ──

function handleSessionBoundary(event, hookConfig, source) {
  const workdir = resolveWorkdir(event, hookConfig);

  if (!hasEddaInit(workdir)) {
    console.log(`${TAG} No ${EDDA_DIR}/ in ${workdir}, skipping ${source} handler`);
    return;
  }

  // Run digest before commit (longer timeout — digest may be slow)
  const digestResult = runEdda(
    ["bridge", "claude", "digest", "--all"],
    workdir,
    10000
  );
  if (digestResult) {
    console.log(`${TAG} Digest (${source}): ${digestResult.slice(0, 80)}`);
  }

  const ts = event.timestamp?.toISOString?.()?.split("T")[1]?.split(".")[0] || "unknown";
  const msg = `session: /${source} at ${ts}`;
  const commitResult = runEdda(["commit", "-m", escapeShellArg(msg)], workdir);

  if (commitResult) {
    console.log(`${TAG} Committed (${source}): ${commitResult}`);
  }
}

// ── Gateway startup handler: log session start ──

function handleGatewayStartup(event, hookConfig) {
  const workdir = resolveWorkdir(event, hookConfig);

  if (!hasEddaInit(workdir)) {
    console.log(`${TAG} No ${EDDA_DIR}/ in ${workdir}, skipping gateway startup`);
    return;
  }

  runEdda(
    ["note", escapeShellArg("gateway started"), "--tag", "session"],
    workdir
  );
  console.log(`${TAG} Logged gateway startup to edda ledger`);
}

// ── Message received handler: stub for verification ──

function handleMessageReceived(event, hookConfig) {
  // Stub — log to verify this event fires in OpenClaw.
  // If confirmed, implement lightweight context refresh with dedup here.
  console.log(`${TAG} message:received fired (stub — not yet implemented)`);
}

// ── Main handler ──

const handler = async (event) => {
  const hookConfig = resolveHookConfig(event.context?.cfg, "edda-bridge");

  if (event.type === "agent" && event.action === "bootstrap") {
    handleBootstrap(event, hookConfig);
    return;
  }

  if (event.type === "message" && event.action === "sent") {
    handleMessageSent(event, hookConfig);
    return;
  }

  if (event.type === "command" && event.action === "new") {
    handleSessionEnd(event, hookConfig);
    return;
  }

  if (event.type === "command" && event.action === "reset") {
    handleSessionBoundary(event, hookConfig, "reset");
    return;
  }

  if (event.type === "command" && event.action === "stop") {
    handleSessionBoundary(event, hookConfig, "stop");
    return;
  }

  if (event.type === "gateway" && event.action === "startup") {
    handleGatewayStartup(event, hookConfig);
    return;
  }

  if (event.type === "message" && event.action === "received") {
    handleMessageReceived(event, hookConfig);
    return;
  }
};

export default handler;
