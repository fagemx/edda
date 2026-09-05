#!/usr/bin/env node
// Generate TypeScript types from the canonical JSON Schema published by the
// event spec repo (GH-608) at a PINNED commit. No dependencies.
//
// Usage:
//   node generate-types.mjs --spec <spec-dir> --out <output.ts>
//
// <spec-dir> must contain registry.json and the *.schema.json files it
// references. When the controller hands off the pinned commit, <spec-dir> is
// sdk/spec-pin/ (created by pin-spec.sh). Until then a LOCAL UNCOMMITTED
// checkout may be used for development; generated files are gitignored and
// must not be frozen while the spec is still moving.

import { readFileSync, writeFileSync, readdirSync } from "node:fs";
import { join, basename } from "node:path";

function arg(name, fallback) {
  const i = process.argv.indexOf(`--${name}`);
  return i >= 0 ? process.argv[i + 1] : fallback;
}

const specDir = arg("spec");
const outFile = arg("out");
if (!specDir || !outFile) {
  console.error("usage: node generate-types.mjs --spec <dir> --out <file.ts>");
  process.exit(2);
}

const registry = JSON.parse(readFileSync(join(specDir, "registry.json"), "utf8"));

function pascal(name) {
  return name
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map((p) => p[0].toUpperCase() + p.slice(1))
    .join("");
}

/** Map a JSON Schema (draft 2020-12 subset) to a TS type expression. */
function tsType(schema, indent = "", nameHint = "") {
  if (schema === true || schema === undefined || schema === false) return "unknown";
  if (schema.anyOf) {
    const parts = schema.anyOf.map((s) => tsType(s, indent, nameHint));
    return parts.join(" | ");
  }
  if (schema.oneOf) {
    return schema.oneOf.map((s) => tsType(s, indent, nameHint)).join(" | ");
  }
  // Enum schemas carry their own value domain: render them as real string-literal
  // unions whether or not a sibling "type" key is present (the pinned corpus
  // uses bare enums and anyOf-wrapped enums).
  if (schema.enum) {
    return schema.enum.map((e) => JSON.stringify(e)).join(" | ");
  }
  switch (schema.type) {
    case "string":
      return "string";
    case "integer":
      return "number /* integer */";
    case "number":
      return "number";
    case "boolean":
      return "boolean";
    case "null":
      return "null";
    case "array":
      return `Array<${tsType(schema.items ?? {}, indent, nameHint)}>`;
    case "object": {
      const props = schema.properties ?? {};
      const required = new Set(schema.required ?? []);
      const lines = Object.entries(props).map(([k, v]) => {
        const opt = required.has(k) ? "" : "?";
        const desc = v && v.description ? ` /** ${String(v.description).split("\n")[0]} */ ` : "";
        return `${indent}  ${JSON.stringify(k)}${opt}:${desc} ${tsType(v, indent + "  ", k)};`;
      });
      if (schema.additionalProperties === true || (schema.additionalProperties && typeof schema.additionalProperties === "object")) {
        const ap = schema.additionalProperties === true ? "unknown" : tsType(schema.additionalProperties, indent, nameHint);
        lines.push(`${indent}  [k: string]: ${ap};`);
      }
      if (lines.length === 0) return "Record<string, unknown>";
      return `{\n${lines.join("\n")}\n${indent}}`;
    }
    default:
      return "unknown";
  }
}

const header = `// GENERATED FILE — do not edit by hand.
// Source: event spec (GH-608) registry.json + *.schema.json
// Layer 1 types (stability "stable-v1") are stable; Layer 2 ("unstable")
// types are experimental and may change in any release
// (docs/reference/client-contract.md §3).
`;

let out = header;
out += "\n// ── Event envelope (stable) ──\n\n";
const envelopeType = tsType(JSON.parse(readFileSync(join(specDir, "envelope.schema.json"), "utf8")));
out += envelopeType.startsWith("{") ? `export interface Envelope ${envelopeType}\n` : `export type Envelope = ${envelopeType};\n`;

const stable = [];
const unstable = [];
for (const entry of registry) {
  const schemaPath = join(specDir, entry.schema);
  const schema = JSON.parse(readFileSync(schemaPath, "utf8"));
  const name = pascal(entry.type) + "Payload";
  const stability = entry.stability ?? "unstable";
  (stability === "stable-v1" ? stable : unstable).push({ entry, name });
  out += `\n/** Event type \`${entry.type}\` — stability: ${stability} (source: ${entry.source ?? "n/a"}). */\n`;
  const payloadType = tsType(schema);
  out += payloadType.startsWith("{") ? `export interface ${name} ${payloadType}\n` : `export type ${name} = ${payloadType};\n`;
}

out += "\n// ── Stability-partitioned unions (contract §3) ──\n\n";
out += `/** Layer 1 stable event payload union (registry stability "stable-v1"). */\n`;
out += `export type Layer1Payload = ${stable.map((s) => s.name).join(" | ") || "never"};\n\n`;
out += `/** Layer 2 experimental payload union (registry stability "unstable") — may change in any release. */\n`;
out += `export type Layer2Payload = ${unstable.map((s) => s.name).join(" | ") || "never"};\n`;

writeFileSync(outFile, out);
console.log(`generated ${outFile} from ${specDir} (${stable.length} stable, ${unstable.length} unstable)`);
