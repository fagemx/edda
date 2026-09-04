# Research, carrier and freshness design delivery

This bundle answers the design acceptance in #602, #603 and #604. Decisions
were recorded 2026-09-04 after querying the project ledger; they are
agent-authored/unratified, not operator ratifications.

| Design | Decision key | Contract | Implementation follow-ups |
|---|---|---|---|
| #602 | finding.model | [finding schema, matrix, lifecycle](finding.md) | [#844 finding events](https://github.com/fagemx/edda/issues/844), [#846 promotion and consumers](https://github.com/fagemx/edda/issues/846) |
| #604 | signal.freshness | [cadence table and pack proof](freshness.md) | [#848 registry and consumers](https://github.com/fagemx/edda/issues/848) |
| #603 | conductor.carrier | [carrier/check/stamp schema](carrier.md) | [#849 runtime carriers](https://github.com/fagemx/edda/issues/849) |

Only the conductor dry-run schema preview and fail-closed runtime rejection
ship here. The following three commands validate the draft fields and legacy
plan topology without a network check or agent dispatch:

```sh
edda conduct run docs/design/infra-contracts/coding.yaml --dry-run
edda conduct run docs/design/infra-contracts/research.yaml --dry-run
edda conduct run docs/design/infra-contracts/loop.yaml --dry-run
```

These plans fail before execution when run without `--dry-run`, because runtime
carrier support is intentionally deferred. Existing plans that omit
the new declarations still run. A successful preview proves schema validity,
not acceptance of the referenced artifact. Independent review and exact-head
CI receipts are published on the delivery PR, pinned to its full SHA.
