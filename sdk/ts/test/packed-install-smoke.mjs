// Verifies the packed artifact, not the repository checkout, on supported Node.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const packed = JSON.parse(readFileSync(0, "utf8"));
const tarball = resolve(packed[0].filename);
const root = mkdtempSync(join(tmpdir(), "edda-sdk-packed-"));
// Read the installed path from the manifest rather than spelling the package
// name here: hard-coded scope segments silently outlive a rename, and this
// test is the only thing that would have caught one.
const pkgName = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")).name;
const installedDir = join(root, "node_modules", ...pkgName.split("/"));
const npm = process.env.npm_execpath ? [process.execPath, process.env.npm_execpath] : ["npm"];
const npmRun = (args, options) => execFileSync(npm[0], [...npm.slice(1), ...args], options);
try {
  npmRun(["init", "-y"], { cwd: root, stdio: "ignore" });
  npmRun(["install", "--offline", "--ignore-scripts", "--no-audit", "--no-fund", tarball], { cwd: root, stdio: "inherit" });
  const sdk = await import(pathToFileURL(join(installedDir, "dist", "src", "index.js")).href);
  assert.equal(typeof sdk.canonicalizeText, "function");
  assert.equal(typeof sdk.HttpTransport, "function");
  assert.ok(readFileSync(join(installedDir, "dist", "src", "types.gen.d.ts"), "utf8").includes("Layer1Payload"));
} finally {
  rmSync(root, { recursive: true, force: true });
}
