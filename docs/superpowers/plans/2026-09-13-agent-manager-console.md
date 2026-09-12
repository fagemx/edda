# Agent manager console implementation plan

**Goal:** Ship a typed local multi-project progress console with direct Pi agent
conversations, safe operator messages and durable receipts.

**Architecture:** Independent TypeScript package over the existing Pi channel.
Private SQLite stores operations/events; native HTTP serves a small browser client.
Edda task truth and Pi runtime ownership stay in their existing systems.

**Tech stack:** Node24, TypeScript5.9.3 strict, native HTTP/SQLite, browser ESM,
Node test runner. No local Cargo lane; live project tests/DBs are not touched.

## Cohesive bundles

- [ ] Contract/package: `integrations/agent-manager/package.json`, lockfile,
  `tsconfig.json`, `src/contracts.ts`. Gate: `npm run typecheck`.
- [ ] Backend: `src/config.ts`, `src/pi-adapter.ts`, `src/store.ts`,
  `src/manager.ts`, `src/http.ts`, `src/cli.ts`, tests. Contract tests first for
  unknown JSON, private projections, selected identities and duplicate operations.
  Required scenarios: send timeout then restart performs zero resend; stale instance
  performs zero send; one unavailable agent leaves others usable; history omits
  reasoning/token fields; wrong Origin/Host/auth and oversized requests are rejected.
- [ ] Browser: `src/web/app.ts`, `src/web/index.html`, `src/web/style.css` in a
  separate worker worktree on the frozen contract. Show project navigation,
  status/freshness, public conversation, persistent uncertain draft and operation
  feed. Use DOM text nodes for all external content. User selects append or priority
  instruction, with the current recipient always visible. No lifecycle/approval UI.
- [ ] Integration: build with `tsc`, serve only fixed web assets, read-only import
  current registration, and add `.github/workflows/agent-manager.yml` for Node24
  typecheck/tests/build on Windows/Linux/macOS. Existing Pi suite is compatibility
  evidence, not a reason to run unrelated Rust gates locally.
- [ ] Verify once after freeze: unit/HTTP tests, real offline Pi end-to-end,
  browser desktop/mobile behavior and same-root restart. Inspect UI against actual
  selected live agents without sending work to them. Record ran/read evidence,
  independent current-head review, CI and R6 landing; preserve running services.

## UI direction

An operations desk for the user's small agent team: a restrained blue/slate palette,
white work surface, strong project names, clear warm attention color, and an inset
conversation area. Segoe UI/Microsoft JhengHei for readable Traditional Chinese;
system monospace only for optional technical identities. Left project/agent list,
central current work and conversation, compact activity rail; stack on narrow
screens. No fake percentages, theatrical charts, or generated statistics.

## Frozen API

`src/contracts.ts` is the shared interface. Routes:
`GET /api/overview`, `GET /api/agents/:id/conversation?after=cursor`,
`POST /api/agents/:id/messages`, `GET /api/operations/:id`.
All API responses require the gateway bearer. A send is an append/steer message,
not a stale-question approval; basis cursor is provenance only. Expected instance
and selection revision guard recipient identity. Body max24KiB/message max12KiB.
Errors use `{error:{code,message}}`, never raw legacy exceptions or credentials.

The independent verifier audited the baseline before implementation. Its proposed
CPU sampler and structured inbox-response UI are intentionally outside this first
slice; registered resources and ordinary direct messages have explicit semantics.
