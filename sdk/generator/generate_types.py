#!/usr/bin/env python3
"""Generate valid Python TypedDicts from the pinned event JSON schemas.

Usage: python generate_types.py --spec <spec-dir> --out <output.py>
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def pascal(name: str) -> str:
    return "".join(p[:1].upper() + p[1:] for p in name.replace("-", "_").replace(".", "_").split("_") if p)


class Generator:
    def __init__(self) -> None:
        self.definitions: list[str] = []
        self.used_names: set[str] = set()

    def nested_name(self, hint: str) -> str:
        base = pascal(hint) or "Object"
        name = base
        number = 2
        while name in self.used_names:
            name = f"{base}{number}"
            number += 1
        self.used_names.add(name)
        return name

    def type_for(self, schema: object, hint: str) -> str:
        if schema is True or schema is None or schema is False or schema == {}:
            return "object"
        assert isinstance(schema, dict)
        if "anyOf" in schema:
            return " | ".join(self.type_for(part, hint) for part in schema["anyOf"])
        if "oneOf" in schema:
            return " | ".join(self.type_for(part, hint) for part in schema["oneOf"])
        # Enum schemas carry their own value domain: render them as real
        # literal unions whether or not a sibling "type" key is present (the
        # pinned corpus uses bare enums and anyOf-wrapped enums).
        if "enum" in schema:
            return "Literal[" + ", ".join(repr(value) for value in schema["enum"]) + "]"
        kind = schema.get("type")
        if kind == "string":
            return "str"
        if kind == "integer":
            return "int"
        if kind == "number":
            return "float"
        if kind == "boolean":
            return "bool"
        if kind == "null":
            return "None"
        if kind == "array":
            return f"list[{self.type_for(schema.get('items', {}), hint + 'Item')}]"
        if kind == "object":
            properties = schema.get("properties", {})
            if not properties:
                return "dict[str, object]"
            name = self.nested_name(hint)
            self.emit_typed_dict(name, schema)
            return name
        return "object"

    def emit_typed_dict(self, name: str, schema: dict[str, object]) -> None:
        properties = schema.get("properties", {})
        assert isinstance(properties, dict)
        required = set(schema.get("required", []))
        # Build nested declarations first so annotations can refer to names
        # already declared in the generated module.
        fields: list[tuple[str, str, bool]] = []
        for key, value in properties.items():
            fields.append((key, self.type_for(value, name + pascal(key)), key in required))
        # Functional TypedDict preserves arbitrary JSON keys (notably `from`),
        # which Python class syntax cannot represent without changing the key.
        self.definitions.extend(["", f"{name} = TypedDict(", f"    {name!r},", "    {"])
        for key, annotation, is_required in fields:
            wrapper = "Required" if is_required else "NotRequired"
            self.definitions.append(f"        {key!r}: {wrapper}[{annotation}],")
        self.definitions.extend(["    },", "    total=False,", ")"])


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--spec", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    spec = Path(args.spec)
    registry = json.loads((spec / "registry.json").read_text(encoding="utf-8"))
    generator = Generator()

    envelope = json.loads((spec / "envelope.schema.json").read_text(encoding="utf-8"))
    generator.used_names.add("Envelope")
    generator.emit_typed_dict("Envelope", envelope)

    stable: list[str] = []
    unstable: list[str] = []
    for entry in registry:
        schema = json.loads((spec / entry["schema"]).read_text(encoding="utf-8"))
        name = pascal(entry["type"]) + "Payload"
        generator.used_names.add(name)
        generator.emit_typed_dict(name, schema)
        (stable if entry.get("stability", "unstable") == "stable-v1" else unstable).append(name)

    lines = [
        '"""GENERATED FILE — do not edit by hand.',
        "",
        "Source: pinned event spec registry.json + *.schema.json.",
        'Layer 1 types (stability "stable-v1") are stable; Layer 2 types are experimental.',
        '"""',
        "from __future__ import annotations",
        "",
        "from typing import Literal, NotRequired, Required, TypeAlias, TypedDict",
        "",
        *generator.definitions,
        "",
        "# Stability-partitioned unions (client contract §3).",
        "Layer1Payload: TypeAlias = " + (" | ".join(stable) or "Never"),
        "Layer2Payload: TypeAlias = " + (" | ".join(unstable) or "Never"),
        "",
    ]
    Path(args.out).write_text("\n".join(lines), encoding="utf-8")
    print(f"generated {args.out} from {args.spec} ({len(stable)} stable, {len(unstable)} unstable)")


if __name__ == "__main__":
    main()
