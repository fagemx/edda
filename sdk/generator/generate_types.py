#!/usr/bin/env python3
"""Generate Python TypedDicts from the pinned event JSON schemas."""
from __future__ import annotations

import argparse
import json
from pathlib import Path


def pascal(name: str) -> str:
    return "".join(
        part[:1].upper() + part[1:]
        for part in name.replace("-", "_").replace(".", "_").split("_")
        if part
    )


def merged_schema(base: object, patch: object) -> object:
    if not isinstance(base, dict) or not isinstance(patch, dict):
        return patch
    output = dict(base)
    for key, value in patch.items():
        if key == "properties" and isinstance(value, dict):
            base_properties = base.get("properties", {})
            if not isinstance(base_properties, dict):
                base_properties = {}
            output[key] = {
                **base_properties,
                **{
                    name: merged_schema(base_properties.get(name, {}), property_patch)
                    for name, property_patch in value.items()
                },
            }
        elif key == "required" and isinstance(value, list):
            output[key] = list(dict.fromkeys([*base.get("required", []), *value]))
        else:
            output[key] = value
    return output


class Generator:
    def __init__(self) -> None:
        self.definitions: list[str] = []
        self.used_names: set[str] = set()
        self.root_schema: dict[str, object] | None = None

    def set_root_schema(self, schema: dict[str, object]) -> None:
        self.root_schema = schema

    def resolve_local_ref(self, ref: str) -> object | None:
        if self.root_schema is None or not ref.startswith("#/"):
            return None
        value: object = self.root_schema
        for raw_part in ref[2:].split("/"):
            part = raw_part.replace("~1", "/").replace("~0", "~")
            if not isinstance(value, dict) or part not in value:
                return None
            value = value[part]
        return value

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
        if "$ref" in schema:
            resolved = self.resolve_local_ref(str(schema["$ref"]))
            return "object" if resolved is None else self.type_for(resolved, hint)
        if "const" in schema:
            return f"Literal[{schema['const']!r}]"
        if "enum" in schema:
            return "Literal[" + ", ".join(repr(value) for value in schema["enum"]) + "]"
        kind = schema.get("type")
        if kind != "object" and "anyOf" in schema:
            return " | ".join(self.type_for(part, hint) for part in schema["anyOf"])
        if kind != "object" and "oneOf" in schema:
            return " | ".join(self.type_for(part, hint) for part in schema["oneOf"])
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
            self.emit_schema(name, schema)
            return name
        return "object"

    def emit_plain_typed_dict(
        self,
        name: str,
        schema: dict[str, object],
        *,
        extra_required: set[str] | None = None,
        forbidden: set[str] | None = None,
        overrides: dict[str, object] | None = None,
    ) -> None:
        properties = schema.get("properties", {})
        assert isinstance(properties, dict)
        required = set(schema.get("required", [])) | (extra_required or set())
        forbidden = forbidden or set()
        overrides = overrides or {}
        fields: list[tuple[str, str, bool]] = []
        for key, value in properties.items():
            if key in forbidden:
                fields.append((key, "NoReturn", False))
            else:
                fields.append(
                    (
                        key,
                        self.type_for(overrides.get(key, value), name + pascal(key)),
                        key in required,
                    )
                )
        self.definitions.extend(["", f"{name} = TypedDict(", f"    {name!r},", "    {"])
        for key, annotation, is_required in fields:
            wrapper = "Required" if is_required else "NotRequired"
            self.definitions.append(f"        {key!r}: {wrapper}[{annotation}],")
        self.definitions.extend(["    },", "    total=False,", ")"])

    def emit_presence_union(self, name: str, schema: dict[str, object]) -> bool:
        dependencies = schema.get("dependentRequired")
        alternatives = schema.get("anyOf")
        if not isinstance(dependencies, dict) and not isinstance(alternatives, list):
            return False
        alternatives = alternatives if isinstance(alternatives, list) else [{}]
        triggers = set(dependencies or {})
        variants: list[str] = []

        for index, alternative in enumerate(alternatives, 1):
            if not isinstance(alternative, dict):
                continue
            variant = self.nested_name(f"{name}Uncontrolled{index}")
            self.emit_plain_typed_dict(
                variant,
                schema,
                extra_required=set(alternative.get("required", [])),
                forbidden=triggers,
            )
            variants.append(variant)

        if isinstance(dependencies, dict) and dependencies:
            controlled_required = set(triggers)
            for values in dependencies.values():
                if isinstance(values, list):
                    controlled_required.update(values)
            # Keep only minimal anyOf required sets: a stricter set is already
            # represented by the less restrictive union member.
            controlled_sets = []
            for alternative in alternatives:
                if isinstance(alternative, dict):
                    controlled_sets.append(controlled_required | set(alternative.get("required", [])))
            minimal_sets = [
                value
                for value in controlled_sets
                if not any(other < value for other in controlled_sets)
            ]
            for index, required in enumerate(
                sorted({tuple(sorted(value)) for value in minimal_sets}), 1
            ):
                variant = self.nested_name(f"{name}Controlled{index}")
                self.emit_plain_typed_dict(
                    variant,
                    schema,
                    extra_required=set(required),
                )
                variants.append(variant)

        self.definitions.extend(["", f"{name}: TypeAlias = {' | '.join(variants)}"])
        return True

    def emit_conditional_union(self, name: str, schema: dict[str, object]) -> bool:
        conditionals = schema.get("allOf")
        if not isinstance(conditionals, list) or len(conditionals) != 1:
            return False
        conditional = conditionals[0]
        if not isinstance(conditional, dict):
            return False
        if_properties = conditional.get("if", {}).get("properties", {})
        then_properties = conditional.get("then", {}).get("properties", {})
        if not isinstance(if_properties, dict) or not isinstance(then_properties, dict):
            return False
        scalar = next(
            (
                (key, value)
                for key, value in if_properties.items()
                if isinstance(value, dict) and "const" in value
            ),
            None,
        )
        nested = next(
            (
                (key, value)
                for key, value in if_properties.items()
                if isinstance(value, dict) and isinstance(value.get("properties"), dict)
            ),
            None,
        )
        if scalar is None or nested is None:
            return False
        scalar_key, scalar_condition = scalar
        nested_key, nested_condition = nested
        discriminator = next(
            (
                (key, value)
                for key, value in nested_condition["properties"].items()
                if isinstance(value, dict) and "const" in value
            ),
            None,
        )
        then_nested = then_properties.get(nested_key)
        properties = schema.get("properties", {})
        if not isinstance(properties, dict):
            return False
        nested_schema = properties.get(nested_key)
        scalar_schema = properties.get(scalar_key)
        if (
            discriminator is None
            or not isinstance(then_nested, dict)
            or not isinstance(then_nested.get("anyOf"), list)
            or not isinstance(nested_schema, dict)
            or not isinstance(nested_schema.get("oneOf"), list)
            or not isinstance(scalar_schema, dict)
        ):
            return False
        discriminator_key, discriminator_condition = discriminator
        scalar_value = scalar_condition["const"]
        variants: list[str] = []
        other_scalars = [value for value in scalar_schema.get("enum", []) if value != scalar_value]
        if other_scalars:
            variant = self.nested_name(f"{name}OtherProfile")
            self.emit_plain_typed_dict(
                variant,
                schema,
                overrides={scalar_key: {"enum": other_scalars}},
            )
            variants.append(variant)

        matching_nested: dict[str, object] | None = None
        for nested_variant in nested_schema["oneOf"]:
            if not isinstance(nested_variant, dict):
                continue
            variant_discriminator = nested_variant.get("properties", {}).get(discriminator_key, {})
            matches = (
                isinstance(variant_discriminator, dict)
                and variant_discriminator.get("const") == discriminator_condition["const"]
            )
            if matches:
                matching_nested = nested_variant
                continue
            variant = self.nested_name(f"{name}OtherProcedure")
            self.emit_plain_typed_dict(
                variant,
                schema,
                overrides={
                    scalar_key: {"const": scalar_value},
                    nested_key: nested_variant,
                },
            )
            variants.append(variant)
        if matching_nested is None:
            return False
        for index, requirement in enumerate(then_nested["anyOf"], 1):
            if not isinstance(requirement, dict):
                continue
            variant = self.nested_name(f"{name}Conditional{index}")
            self.emit_plain_typed_dict(
                variant,
                schema,
                overrides={
                    scalar_key: {"const": scalar_value},
                    nested_key: merged_schema(matching_nested, requirement),
                },
            )
            variants.append(variant)
        self.definitions.extend(["", f"{name}: TypeAlias = {' | '.join(variants)}"])
        return True

    def emit_schema(self, name: str, schema: dict[str, object]) -> None:
        if self.emit_conditional_union(name, schema):
            return
        if self.emit_presence_union(name, schema):
            return
        self.emit_plain_typed_dict(name, schema)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--spec", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()
    spec = Path(args.spec)
    registry = json.loads((spec / "registry.json").read_text(encoding="utf-8"))
    generator = Generator()

    envelope = json.loads((spec / "envelope.schema.json").read_text(encoding="utf-8"))
    generator.set_root_schema(envelope)
    generator.used_names.add("Envelope")
    generator.emit_schema("Envelope", envelope)

    stable: list[str] = []
    unstable: list[str] = []
    for entry in registry:
        schema = json.loads((spec / entry["schema"]).read_text(encoding="utf-8"))
        generator.set_root_schema(schema)
        name = pascal(entry["type"]) + "Payload"
        generator.used_names.add(name)
        generator.emit_schema(name, schema)
        (stable if entry.get("stability", "unstable") == "stable-v1" else unstable).append(name)

    lines = [
        '"""GENERATED FILE — do not edit by hand.',
        "",
        "Source: pinned event spec registry.json + *.schema.json.",
        'Layer 1 types (stability "stable-v1") are stable; Layer 2 types are experimental.',
        '"""',
        "from __future__ import annotations",
        "",
        "from typing import Literal, NoReturn, NotRequired, Required, TypeAlias, TypedDict",
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
