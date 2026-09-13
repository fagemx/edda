# Manager session events implementation plan

**Goal:** Bind actual selected sessions to work and retain child events for its owner.
**Architecture:** Edda workflow events hold execution bindings and owner handoffs;
native adapters return bounded public observations; the local observational journal
deduplicates owner inbox events. Existing task/acceptance and send boundaries remain.
**Tech stack:** TypeScript/Node24, native SQLite, existing Pi channel, selected Codex JSONL.

## Independent units

1. Codex adapter: explicit file/identity/workspace validation, bounded incremental
   public event projection, read-only conversation and synthetic/native-read checks.
2. Session/work backend: versioned binding/owner handoff and acknowledgement actions,
   durable inbox projection, expectation/deadline and summary-stale derivation,
   bounded owner context endpoints, concurrency/restart/unknown-state tests.
3. Parent integration: Pi error/native observation fields, composite adapter wiring,
   selected-agent configuration, work UI/session/inbox controls, docs and isolated
   integrated browser checks. Agree exact contracts before parallel edits.
4. Freeze: strict typecheck/package tests, real isolated observation, independent
   full scoped review and exact-head CI; then R6 merge and owned console upgrade.

No implementation unit compiles Rust. Preserve current live manager-handoff source
and all unrelated worktrees. Do not start S7 or mutate project worker code. Read
existing evidence before rerunning a gate; one reviewer per frozen candidate.
