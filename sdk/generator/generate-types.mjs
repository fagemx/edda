#!/usr/bin/env node
// Generate TypeScript types from the canonical JSON Schema published by the
// event spec repo (GH-608) at a PINNED commit. No dependencies.

import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

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
let rootSchema;

function resolveLocalRef(ref) {
  if (!rootSchema || !ref.startsWith("#/")) return undefined;
  return ref
    .slice(2)
    .split("/")
    .map((part) => part.replaceAll("~1", "/").replaceAll("~0", "~"))
    .reduce((value, part) => value?.[part], rootSchema);
}

function pascal(name) {
  return name
    .split(/[^a-zA-Z0-9]+/)
    .filter(Boolean)
    .map((part) => part[0].toUpperCase() + part.slice(1))
    .join("");
}

function mergedSchema(base, patch) {
  const output = { ...base };
  for (const [key, value] of Object.entries(patch ?? {})) {
    if (key === "properties") {
      output.properties = { ...(base.properties ?? {}) };
      for (const [name, propertyPatch] of Object.entries(value ?? {})) {
        output.properties[name] = mergedSchema(
          base.properties?.[name] ?? {},
          propertyPatch,
        );
      }
    } else if (key === "required") {
      output.required = [...new Set([...(base.required ?? []), ...value])];
    } else {
      output[key] = value;
    }
  }
  return output;
}

function objectShape(schema, indent, nameHint) {
  const props = schema.properties ?? {};
  const required = new Set(schema.required ?? []);
  const lines = Object.entries(props).map(([key, value]) => {
    const optional = required.has(key) ? "" : "?";
    const desc = value?.description
      ? ` /** ${String(value.description).split("\n")[0]} */ `
      : "";
    return `${indent}  ${JSON.stringify(key)}${optional}:${desc} ${tsType(value, `${indent}  `, key)};`;
  });
  if (
    schema.additionalProperties === true ||
    (schema.additionalProperties && typeof schema.additionalProperties === "object")
  ) {
    const additional =
      schema.additionalProperties === true
        ? "unknown"
        : tsType(schema.additionalProperties, indent, nameHint);
    lines.push(`${indent}  [k: string]: ${additional};`);
  }
  if (lines.length === 0) return "Record<string, unknown>";
  return `{\n${lines.join("\n")}\n${indent}}`;
}

function requiredBranchType(root, branch, indent, nameHint) {
  const properties = {};
  for (const key of branch.required ?? []) {
    properties[key] = mergedSchema(root.properties?.[key] ?? {}, branch.properties?.[key] ?? {});
  }
  return objectShape(
    {
      type: "object",
      properties,
      required: branch.required ?? [],
      additionalProperties: false,
    },
    indent,
    nameHint,
  );
}

function dependentRequiredType(schema, indent, nameHint) {
  const parts = Object.entries(schema.dependentRequired ?? {}).map(([trigger, dependencies]) => {
    const absent = objectShape(
      {
        type: "object",
        properties: { [trigger]: { const: undefined } },
        additionalProperties: false,
      },
      indent,
      nameHint,
    ).replace("undefined", "never");
    const required = [...new Set([trigger, ...dependencies])];
    return `(${absent} | ${requiredBranchType(schema, { required }, indent, nameHint)})`;
  });
  return parts.length > 0 ? parts.join(" & ") : undefined;
}

function conditionalType(schema, conditional, indent, nameHint) {
  const ifProperties = conditional.if?.properties ?? {};
  const thenProperties = conditional.then?.properties ?? {};
  const scalarEntry = Object.entries(ifProperties).find(([, value]) =>
    Object.hasOwn(value, "const"),
  );
  const nestedEntry = Object.entries(ifProperties).find(([, value]) => value?.properties);
  if (!scalarEntry || !nestedEntry) return undefined;

  const [scalarKey, scalarCondition] = scalarEntry;
  const [nestedKey, nestedCondition] = nestedEntry;
  const nestedDiscriminator = Object.entries(nestedCondition.properties ?? {}).find(
    ([, value]) => Object.hasOwn(value, "const"),
  );
  const thenNested = thenProperties[nestedKey];
  const nestedSchema = schema.properties?.[nestedKey];
  if (!nestedDiscriminator || !thenNested?.anyOf || !nestedSchema?.oneOf) return undefined;

  const [discriminatorKey, discriminatorCondition] = nestedDiscriminator;
  const scalarSchema = schema.properties?.[scalarKey] ?? {};
  const scalarValues = scalarSchema.enum ?? [];
  const alternatives = [];
  const otherScalarValues = scalarValues.filter((value) => value !== scalarCondition.const);
  if (otherScalarValues.length > 0) {
    alternatives.push(
      objectShape(
        {
          type: "object",
          properties: { [scalarKey]: { enum: otherScalarValues } },
          required: [scalarKey],
          additionalProperties: false,
        },
        indent,
        nameHint,
      ),
    );
  }

  const matchingNested = nestedSchema.oneOf.find(
    (variant) =>
      variant.properties?.[discriminatorKey]?.const === discriminatorCondition.const,
  );
  const otherNested = nestedSchema.oneOf.filter(
    (variant) =>
      variant.properties?.[discriminatorKey]?.const !== discriminatorCondition.const,
  );
  for (const variant of otherNested) {
    alternatives.push(
      objectShape(
        {
          type: "object",
          properties: {
            [scalarKey]: { const: scalarCondition.const },
            [nestedKey]: variant,
          },
          required: [scalarKey, nestedKey],
          additionalProperties: false,
        },
        indent,
        nameHint,
      ),
    );
  }
  if (!matchingNested) return undefined;
  for (const requirement of thenNested.anyOf) {
    alternatives.push(
      objectShape(
        {
          type: "object",
          properties: {
            [scalarKey]: { const: scalarCondition.const },
            [nestedKey]: mergedSchema(matchingNested, requirement),
          },
          required: [scalarKey, nestedKey],
          additionalProperties: false,
        },
        indent,
        nameHint,
      ),
    );
  }
  return alternatives.length > 0 ? `(${alternatives.join(" | ")})` : undefined;
}

/** Map the event-spec draft 2020-12 subset to a TypeScript type expression. */
function tsType(schema, indent = "", nameHint = "") {
  if (schema === true || schema === undefined || schema === false) return "unknown";
  if (schema.$ref) {
    const resolved = resolveLocalRef(schema.$ref);
    return resolved === undefined ? "unknown" : tsType(resolved, indent, nameHint);
  }
  if (Object.hasOwn(schema, "const")) return JSON.stringify(schema.const);
  if (schema.enum) return schema.enum.map((value) => JSON.stringify(value)).join(" | ");
  if (schema.type !== "object" && schema.anyOf) {
    return schema.anyOf.map((part) => tsType(part, indent, nameHint)).join(" | ");
  }
  if (schema.type !== "object" && schema.oneOf) {
    return schema.oneOf.map((part) => tsType(part, indent, nameHint)).join(" | ");
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
    case "array": {
      const item = tsType(schema.items ?? {}, indent, nameHint);
      return (schema.minItems ?? 0) > 0 ? `[${item}, ...Array<${item}>]` : `Array<${item}>`;
    }
    case "object": {
      const base = objectShape(schema, indent, nameHint);
      const constraints = [];
      if (schema.anyOf) {
        constraints.push(
          `(${schema.anyOf
            .map((branch) => requiredBranchType(schema, branch, indent, nameHint))
            .join(" | ")})`,
        );
      }
      const dependencies = dependentRequiredType(schema, indent, nameHint);
      if (dependencies) constraints.push(dependencies);
      for (const conditional of schema.allOf ?? []) {
        const rendered = conditionalType(schema, conditional, indent, nameHint);
        if (rendered) constraints.push(rendered);
      }
      return constraints.length > 0 ? `(${base} & ${constraints.join(" & ")})` : base;
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
rootSchema = JSON.parse(readFileSync(join(specDir, "envelope.schema.json"), "utf8"));
const envelopeType = tsType(rootSchema);
out += envelopeType.startsWith("{")
  ? `export interface Envelope ${envelopeType}\n`
  : `export type Envelope = ${envelopeType};\n`;

const stable = [];
const unstable = [];
for (const entry of registry) {
  const schema = JSON.parse(readFileSync(join(specDir, entry.schema), "utf8"));
  rootSchema = schema;
  const name = pascal(entry.type) + "Payload";
  const stability = entry.stability ?? "unstable";
  (stability === "stable-v1" ? stable : unstable).push({ name });
  out += `\n/** Event type \`${entry.type}\` — stability: ${stability} (source: ${entry.source ?? "n/a"}). */\n`;
  const payloadType = tsType(schema);
  const constrained = schema.anyOf || schema.dependentRequired || schema.allOf;
  out += payloadType.startsWith("{") && !constrained
    ? `export interface ${name} ${payloadType}\n`
    : `export type ${name} = ${payloadType};\n`;
}

out += "\n// ── Stability-partitioned unions (contract §3) ──\n\n";
out += '/** Layer 1 stable event payload union (registry stability "stable-v1"). */\n';
out += `export type Layer1Payload = ${stable.map((item) => item.name).join(" | ") || "never"};\n\n`;
out += '/** Layer 2 experimental payload union (registry stability "unstable") — may change in any release. */\n';
out += `export type Layer2Payload = ${unstable.map((item) => item.name).join(" | ") || "never"};\n`;

writeFileSync(outFile, out);
console.log(
  `generated ${outFile} from ${specDir} (${stable.length} stable, ${unstable.length} unstable)`,
);
