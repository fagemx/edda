#!/usr/bin/env node
// Cross-language contract runner (contract §7): generates SDK types from the
// PINNED spec (sdk/spec-pin + SPEC_PIN.json), runs both SDK test suites
// against a real edda on isolated stores, and requires structural equivalence
// of the two scenario transcripts — including the task/receipt/claim/verify
// flow through the shared state machine.
//
// Usage:
//   EDDA_BIN=/path/to/edda node run-contract-tests.mjs
//
// Spec resolution (no optional skip — the pin is a checked-in manifest):
//   1. EDDA_SPEC_DIR env (dev convenience against a local checkout)
//   2. sdk/spec-pin/ (created by generator/pin-spec.sh at SPEC_PIN.json's sha)

import { spawnSync } from "node:child_process";
import { existsSync, readFileSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const EDDA_BIN = process.env.EDDA_BIN;
const npmCli = process.env.npm_execpath ?? join(dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js");
const npmCommand = process.platform === "win32" ? process.execPath : "npm";
const npmPrefix = process.platform === "win32" ? [npmCli] : [];

function fail(msg) {
  console.error(`FAIL: ${msg}`);
  process.exit(1);
}

if (!EDDA_BIN) fail("EDDA_BIN not set — point it at a built edda binary");
if (!existsSync(EDDA_BIN) && !["edda", "edda.exe"].includes(EDDA_BIN)) {
  fail(`EDDA_BIN=${EDDA_BIN} does not exist`);
}

// 0. resolve the pinned spec (manifest is the source of truth)
const manifestPath = join(here, "SPEC_PIN.json");
if (!existsSync(manifestPath)) fail("sdk/SPEC_PIN.json missing — the spec pin is a required manifest");
const pin = JSON.parse(readFileSync(manifestPath, "utf8"));
if (!/^[0-9a-f]{40}$/.test(pin.spec_sha)) fail(`SPEC_PIN.json has no valid 40-hex spec_sha (got ${pin.spec_sha})`);

let specDir;
if (process.env.EDDA_SPEC_DIR) {
  specDir = resolve(process.env.EDDA_SPEC_DIR);
  console.log(`note: dev override EDDA_SPEC_DIR=${specDir} (pin manifest: ${pin.spec_sha})`);
} else {
  specDir = join(here, "spec-pin");
}
if (!existsSync(join(specDir, "spec", "events", "registry.json"))) {
  fail(
    `spec schemas unavailable at ${specDir}/spec/events — run ` +
      `sdk/generator/pin-spec.sh ${pin.spec_sha} (dev) or let CI check out the pinned sha`,
  );
}
if (!existsSync(join(specDir, "spec", "events", "canonical-v1.json"))) {
  fail(`canonical vectors missing at ${specDir}/spec/events/canonical-v1.json`);
}
const fixturesDir = join(specDir, "tests", "fixtures", "events");
if (!existsSync(fixturesDir)) fail(`golden fixtures missing at ${fixturesDir}`);

// 1. generate types from the pinned schemas
const tsOut = join(here, "ts", "src", "types.gen.ts");
const pyOut = join(here, "python", "src", "edda_sdk", "types_gen.py");
for (const [cmd, args] of [
  ["node", [join(here, "generator", "generate-types.mjs"), "--spec", join(specDir, "spec", "events"), "--out", tsOut]],
  ["python", [join(here, "generator", "generate_types.py"), "--spec", join(specDir, "spec", "events"), "--out", pyOut]],
]) {
  const r = spawnSync(cmd, args, { stdio: "inherit" });
  if (r.status !== 0) fail(`type generation failed for ${cmd}`);
}

// 2. compile/import generated types, then golden + vector tests.
const goldenEnv = { ...process.env, EDDA_SPEC_DIR: specDir };
const pyTypes = spawnSync("python", ["-c", [
  "import sys; sys.path.insert(0, 'sdk/python/src'); import edda_sdk.types_gen as t;",
  "assert 'event_id' in t.Envelope.__required_keys__;",
  "assert 'refs' in t.Envelope.__optional_keys__;",
  "assert not any(member is object for member in t.Layer1Payload.__args__);",
  "assert not any(member is object for member in t.Layer2Payload.__args__);",
  "assert any(hasattr(value, '__required_keys__') for value in vars(t).values())",
].join(" ")], { stdio: "inherit", env: goldenEnv });
if (pyTypes.status !== 0) fail("generated Python types do not compile/import with required nested TypedDicts");
const tsBuild = spawnSync(npmCommand, [...npmPrefix, "--prefix", join(here, "ts"), "run", "build"], { stdio: "inherit", env: goldenEnv });
if (tsBuild.status !== 0) fail("TypeScript package build failed");
const tsPackSmoke = spawnSync(npmCommand, [...npmPrefix, "--prefix", join(here, "ts"), "run", "pack-smoke"], { stdio: "inherit", env: goldenEnv });
if (tsPackSmoke.status !== 0) fail("TypeScript packed-install smoke test failed");
const tsGolden = spawnSync("node", ["--test", join(here, "ts", "dist", "test", "golden.test.js")], {
  stdio: "inherit",
  env: goldenEnv,
});
if (tsGolden.status !== 0) fail("TS golden/vector tests failed");

const pyGolden = spawnSync("python", ["-m", "unittest", "discover", "-s", join(here, "python", "tests"), "-p", "test_golden.py"], {
  stdio: "inherit",
  env: { ...goldenEnv, PYTHONPATH: join(here, "python", "src"), PYTHONWARNINGS: "error::ResourceWarning" },
});
if (pyGolden.status !== 0) fail("Python golden/vector tests failed");

// 3. live contract tests against the real binary, capturing transcripts
const tsOutFile = join(mkdtempSync(join(tmpdir(), "edda-scenario-")), "ts.json");
const pyOutFile = join(mkdtempSync(join(tmpdir(), "edda-scenario-")), "py.json");

const tsContract = spawnSync("node", ["--test", join(here, "ts", "dist", "test", "contract.test.js")], {
  stdio: "inherit",
  env: { ...goldenEnv, EDDA_SCENARIO_OUT: tsOutFile },
});
if (tsContract.status !== 0) fail("TS contract tests failed");

const pyContract = spawnSync("python", ["-m", "unittest", "discover", "-s", join(here, "python", "tests"), "-p", "test_contract.py"], {
  stdio: "inherit",
  env: { ...goldenEnv, EDDA_SCENARIO_OUT: pyOutFile, PYTHONPATH: join(here, "python", "src"), PYTHONWARNINGS: "error::ResourceWarning" },
});
if (pyContract.status !== 0) fail("Python contract tests failed");

// 4. structural equivalence of transcripts
const ts = JSON.parse(readFileSync(tsOutFile, "utf8"));
const py = JSON.parse(readFileSync(pyOutFile, "utf8"));
const normalize = (t) =>
  JSON.stringify({
    capabilities: t.capabilities,
    decisions: t.decisions,
    task: t.task,
    verify: t.verify,
  });
if (normalize(ts) !== normalize(py)) {
  console.error("TS transcript:   ", normalize(ts));
  console.error("Python transcript:", normalize(py));
  fail("TS and Python scenario transcripts are not structurally equivalent");
}

console.log("cross-language contract: EQUIVALENT");
console.log(`(ts sdk=${ts.sdk}, python sdk=${py.sdk}; capabilities+decisions+task+verify match)`);
