#!/usr/bin/env python3
"""Generate Python TypedDicts from the canonical JSON Schema published by the
event spec repo (GH-608) at a PINNED commit. No dependencies.

Usage:
    python generate_types.py --spec <spec-dir> --out <output.py>

<spec-dir> must contain registry.json and the *.schema.json files it
references. Generated files are gitignored until the spec commit is pinned
by the controller (SDK_HANDOFF.md).
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def pascal(name: str) -> str:
    return "".join(p[:1].upper() + p[1:] for p in name.replace("-", "_").replace(".", "_").split("_") if p)


def py_type(schema: object, indent: str = "") -> str:
    if schema is True or schema is None or schema is False or schema == {}:
        return "object"
    assert isinstance(schema, dict)
    if "anyOf" in schema:
        return " | ".join(py_type(s, indent) for s in schema["anyOf"])
    if "oneOf" in schema:
        return " | ".join(py_type(s, indent) for s in schema["oneOf"])
    t = schema.get("type")
    if t == "string":
        if "enum" in schema:
            return " | ".join(json.dumps(e) for e in schema["enum"])
        return "str"
    if t == "integer":
        return "int  # integer"
    if t == "number":
        return "float"
    if t == "boolean":
        return "bool"
    if t == "null":
        return "None"
    if t == "array":
        inner = py_type(schema.get("items", {}), indent)
        return f"list[{inner}]"
    if t == "object":
        props = schema.get("properties", {})
        if not props:
            return "dict[str, object]"
        lines = []
        for k, v in props.items():
            doc = ""
            if isinstance(v, dict) and v.get("description"):
                doc = f"  # {str(v['description']).splitlines()[0]}"
            lines.append(f'{indent}    "{k}": {py_type(v, indent + "    ")}{doc}')
        return "{\n" + "\n".join(lines) + f"\n{indent}}}"
    return "object"


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--spec", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    spec = Path(args.spec)
    registry = json.loads((spec / "registry.json").read_text(encoding="utf-8"))

    lines = [
        '"""GENERATED FILE — do not edit by hand.',
        "",
        "Source: event spec (GH-608) registry.json + *.schema.json",
        'Layer 1 types (stability "stable-v1") are stable; Layer 2 ("unstable")',
        "types are experimental and may change in any release",
        "(docs/reference/client-contract.md §3).",
        '"""',
        "",
        "from __future__ import annotations",
        "",
        "from typing import TypedDict",
        "",
        "",
        'class Envelope(TypedDict, total=False):',
        '    """Event envelope (stable). All keys optional at the type level;'
        " readers must treat documented-required keys as required at runtime.\"\"\"",
        "",
    ]
    envelope = json.loads((spec / "envelope.schema.json").read_text(encoding="utf-8"))
    props = envelope.get("properties", {})
    required = set(envelope.get("required", []))
    for k, v in props.items():
        doc = ""
        if isinstance(v, dict) and v.get("description"):
            doc = f"  # {str(v['description']).splitlines()[0]}"
        req = ""  # TypedDict total=False; required keys documented via comment
        marker = "  # required" if k in required else ""
        lines.append(f'    "{k}"{req}: {py_type(v, "    ")}{doc}{marker}')

    stable, unstable = [], []
    for entry in registry:
        schema = json.loads((spec / entry["schema"]).read_text(encoding="utf-8"))
        name = pascal(entry["type"]) + "Payload"
        stability = entry.get("stability", "unstable")
        (stable if stability == "stable-v1" else unstable).append(name)
        lines += [
            "",
            f'class {name}(TypedDict, total=False):',
            f'    """Event type ``{entry["type"]}`` — stability: {stability}'
            f" (source: {entry.get('source', 'n/a')}).\"\"\"",
            "",
        ]
        for k, v in schema.get("properties", {}).items():
            doc = ""
            if isinstance(v, dict) and v.get("description"):
                doc = f"  # {str(v['description']).splitlines()[0]}"
            lines.append(f'    "{k}": {py_type(v, "    ")}{doc}')

    lines += [
        "",
        "# ── Stability-partitioned unions (contract §3) ──",
        "",
        "Layer1Payload = " + " | ".join(stable + ["object"]),
        '"""Layer 1 stable event payload union (registry stability "stable-v1")."""',
        "",
        "Layer2Payload = " + " | ".join(unstable + ["object"]),
        '"""Layer 2 experimental payload union — may change in any release."""',
        "",
    ]

    Path(args.out).write_text("\n".join(lines), encoding="utf-8")
    print(f"generated {args.out} from {args.spec} ({len(stable)} stable, {len(unstable)} unstable)")


if __name__ == "__main__":
    main()
