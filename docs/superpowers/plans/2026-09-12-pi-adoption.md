# Pi session adoption implementation plan

**Goal:** One CLI operation connects an existing Pi session to declared task context
and explicit dependency observation, with usable preflight and failure reporting.

**Architecture:** Reuse compose, enrollment, handoff and dependency endpoints.
Resolve exact IDs or unique prefixes of at least eight characters across the whole
registry, including offline entries. Read the chosen task and transitive `after`
dependencies; optional extra roots cover review/correction tasks with explicit
links. No name-based or receipt-text inference. Node.js 24; no new dependency.

## Contract

`adopt SESSION --task ID [--project PATH] [--include ID,ID] [--context FILE]
[--scope TEXT] [--notify] [--max-notifications 10] [--preview] [--expected REVISION]`

- Project defaults to the live session cwd. Exact IDs beat prefix matching;
  ambiguity, missing session, offline, reload-required and busy have distinct results.
- Discovery visits root plus explicit include roots and transitive prerequisites,
  rejects cycles, missing tasks and more than eight unique tasks without silently
  truncating. Return provenance edges and coverage `explicit_after_snapshot`.
- Preview performs source reads/cache only. Validate complete context, enrollment
  scope, cap and all dependency sources before managed mutations. Missing role,
  doneWhen or authority references remains `needs_context` with source details.
- Apply uses the same frozen instance throughout. Prepare an idle handoff, enroll
  if needed, configure observation. Each completed step is reported. Failure after
  a write is partial/unknown, not rolled back or automatically retried.
- Reuse an existing current handoff if run identity, goal, role, doneWhen and scope
  match; its immutable source snapshot remains visible. Task status changes alone
  must not replace the manifest or reset the observer notification cap. A different
  handoff or stale instance binding requires its explicit expected revision.
- Adopt does not imply work started. It reports observed runtime and notification
  receipt separately; observe-only sends no messages. Repeat configuration preserves
  existing observer cap/uncertainty behavior.
- Dependencies are a snapshot at adoption; new review tasks and graph edits require
  another adoption. No reload/hot-update mechanism, remote transport, authority
  resolver, runtime replacement or standalone package distribution in this slice.

## Implementation and validation

- [ ] Add `adoption.mjs`: selector, bounded graph discovery, preflight and apply.
- [ ] Extend `enroll` with optional instance binding; add CLI adopt route and help.
- [ ] Add `adoption.test.mjs` with actual loopback channel and external task-reader
  fixture: ambiguity/offline collisions, root/diamond/cycle/overflow/missing tasks,
  preview and missing metadata without enrollment/handoff, successful adopt,
  busy/stale CAS, repeated status change preserving notification cap, partial errors.
- [ ] Add README start-here adoption examples and honest remaining boundaries.
- [ ] Run focused Node adoption tests; then full npm suite once at frozen code.
- [ ] Run actual Pi offline-provider smoke through adoption; no user tasks/paid calls.
- [ ] Obtain independent PR-visible SHA-pinned review, exact-head CI and R6 window;
  merge under standing operator authorization. Preserve installed source worktree.
