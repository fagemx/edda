# Edda task-to-handoff composition

**Goal:** Build a management manifest from the existing Rust task CLI and explicit
management metadata, without guessing missing goals/authority or changing tasks.

**Scope:** Node integration only. `compose --project PATH --task ID` reads
`edda task show ID --json` using argument-safe process execution. It copies task
identity/title/scope facts, snapshots sources privately, and reads a bounded
project-contained brief or an explicitly selected context file. Structured input
is JSON or one `edda-management` fenced block; ordinary prose is not interpreted.

Context supplies `role`, `doneWhen` and `scope` (allowed, excluded, reserved,
authorityRefs). Task path facts remain visible as `scope.taskPaths`; these are
declared source facts, not a new grant or a substitute for existing scope gates.
Missing context produces `needs_context` and no prepared output. A complete
candidate uses the current manifest validator and existing prepare endpoint.

Immutable source snapshots make the manifest's source reference inspectable even
after the task changes. Output files are create-only. No source content is run,
no URL is fetched, no task/session is started, and no automatic preparation occurs.

- [x] Implement safe task JSON loading, source snapshotting and explicit metadata extraction.
- [x] Build ready/incomplete composition results and preserve scope/source provenance.
- [x] Add CLI compose, optional create-only output, sample context and documentation.
- [x] Validate process/CLI boundaries, missing metadata, identity/path/size errors,
  scope fidelity, snapshots and actual installed Edda read-only composition.
- [x] Run affected Node gates; independent local review and final receipt are
  recorded on task #183 before claiming completion.

The task engine and original ledger contracts remain Rust-owned. No Cargo build
lane, runtime adapter expansion, scheduling or other agents' task changes.
