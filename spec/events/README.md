# Ledger event schemas

`registry.json` enumerates current ledger event types. Its `stability` field
distinguishes the Layer 1 v1 contract from an inventory of unstable events.
`envelope.schema.json` describes the shared envelope; `<type>.schema.json`
describes its payload. These are JSON Schema draft 2020-12 documents.

See [the normative specification](../../docs/reference/ledger-event-spec.md).
Golden JSONL examples live in `tests/fixtures/events/` at repository root.
The Rust conformance tests validate these schemas and fixtures with the
`jsonschema` crate (draft 2020-12); network and file reference retrieval are
disabled. External consumers can use their draft 2020-12 validator directly.

For SDK generation, read the registry, generate the envelope and each payload
separately, and discriminate on `type`. Do not infer stability from presence
in this directory. Unknown types remain opaque; registry membership is a
repository authoring guard, not a new runtime rejection policy.
