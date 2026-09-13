# Manager handoff implementation plan

**Goal:** Finish the approved delivery/review/closure loop without depending on a
conversation remembering the next owner.

**Architecture:** Selected existing Edda tasks plus versioned coordination notes
are the work source. The typed manager projects those notes and original message
receipts into work cards. Existing Pi delivery and SQLite idempotency remain the
transport boundary.

**Tech stack:** TypeScript, Node24 native HTTP/SQLite, Edda structured CLI, existing
Pi channel. No local Rust compilation or new runtime dependency.

## 1. Backend — task218, handoff_backend

- Add browser-safe workflow contracts and local optional work selections.
- Implement bounded task/log/note adapter with no shell or browser-selected paths.
- Implement revisioned event reducer, serialized mutation, stable action IDs,
  original recipient preservation and explicit instruction acknowledgement.
- Wire GET `/api/works` and POST `/api/works/:id/actions` into existing gateway.
- Test two same-ID actions yielding one effect, conflicting payload/stale revision,
  restart around dispatch, evidence required for delivery/acceptance, pending
  instruction acknowledgement, and per-work source failure isolation.

## 2. Operator interface — task217, parent

- Add work cards and a bounded action form beside existing conversations.
- Keep work state separate from runtime status and canonical Edda task status.
- Persist action drafts/uncertain request IDs before issuing POST; retries use the
  original request and target. Do not clear evidence on uncertain HTTP failures.
- Render external text with textContent and preserve focused forms during polling.
- Document configuration, action semantics and recovery in package README.

## 3. Integration and delivery — parent + independent reviewer

- Run `npm ci --ignore-scripts`, `npm run typecheck`, `npm test` in
  `integrations/agent-manager`; expect all old and new cases green.
- Run an isolated real Edda/channel lifecycle and browser checks; preserve receipt.
- Inspect the frozen diff, publish a PR and obtain independent current-head review.
- Read exact-head CI; run no redundant Cargo gate for this TS-only package change.
- Merge under the existing R6 gate, upgrade only the owned console, and verify
  configured work displays its actual task/owner/next-step without synthetic sends.
