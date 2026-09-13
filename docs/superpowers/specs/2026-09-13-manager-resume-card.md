# Necessary context in the workbench

## Existing foundation

PR #1156 already provides native continuity save, restore, export and import.
ContextCapsuleInputV1 records goal, current state, hypotheses, rejected options,
open questions, next action and task/event references. Edda adds Git state,
repository identity and provenance; its portable bundle validates digests and
has data_only authority. Pi compose/prepare/brief and task prepare already
provide execution handoffs. Reuse these boundaries instead of adding another
canonical TypeScript capsule or repository-identity implementation.

## Scope of #1173

Connect a selected work to native continuity through a configured local Edda
executable. The workbench edits bounded native input, saves it, displays exact-ID
restore results, and copies readable context or the native portable bundle.
Another configured workspace can import the bundle through Edda and inspect
native restore warnings. A local-only capsule remains useful without export.
Only capsule/event references belong in the workflow history.

The PATH executable on the development machine reports version 0.6.1 but lacks
the continuity command. Probe capabilities, allow an explicit executable in local
configuration, and report unavailable if missing. Do not silently invent a fallback
format or replace the global executable. Deployment uses a pinned private copy of
a verified executable, not a mutable build-lane path.

Existing handoff_owner records local responsibility after explicit environment
and original-writer release evidence. Native dirty/missing-commit warnings remain
visible; they are not new blanket clean-tree or exact-checkout gates. Context
prose cannot grant execution authority, launch a session or prove remote fencing.
No project task is paused, resumed or dispatched by this integration.

## Persistence and limits

Persist request IDs and write intent before native save/import. An unknown write
must not be blindly replayed. A known capsule ID can be restored and linked after
an interrupted workflow append. Preserve actionable native failure information.
The form accepts at most 8 KiB of input; native bundle limits remain bounded at
528 KiB with a route-specific HTTP request limit. No full transcript, credentials,
machine backup or filesystem contents are copied.

## Validation and delivery

Test capability absence, duplicate/conflicting requests, interrupted native writes,
known-ID recovery, native warnings, and same-repository import between two isolated
temporary clones with separate stores. Test the actual browser save/copy/import
flow and existing HTTP authorization boundaries. Do not use Character/Edda active
tasks as fixtures or claim a physical second-computer test.

Freeze the implementation, run package gates and exact-head CI, obtain independent
review, then merge under R6 and upgrade the owned 4390 service preserving its state.

Automatic remote synchronization and restoration of unsaved source files remain
outside this slice; necessary context points to artifacts and does not replace them.

## 2026-09-13 capability/layer verification

The operator asked to recheck completeness, interruption, recovery and orchestration,
then explicitly warned against an upstream workflow overwriting downstream runtime.
Inspection found two different coord-orchestrate copies: canonical tracked source
and .claude projection include delivery-flow/1 and native control; local .agents
projection is older. This implementation uses the canonical layer boundary.

Verified owners:
- Native controlled task: edda control owns the sealed manifest, attempt/lease,
  deterministic dispatch identity, receipts, verification and adjudication.
- Pi managed/supervised task: existing managed run/supervisor owns its process and
  explicitly selected task set. No migration into native control is inferred.
- Host/manual task: actual host/controller and observed task/session mapping.
- Workbench: observation, explicit intervention, continuity reference and local owner.
- Development controller/worker/verifier: development roles, not runtime authority.

This PR completes the native necessary-context UI and console crash/start recovery.
It does not add a common scheduler, alter task leases, restart workers or change
existing downstream review/merge procedures. Capability inventory must distinguish
existing downstream capability from a workbench integration that is still absent.
