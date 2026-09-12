# Pi dependency alerts and usability

**Goal:** A waiting Pi learns that selected upstream task/review facts changed,
without a manager replaying every conversation or sending repeated continue prompts.

**Architecture:** Reuse the Pi extension's lifetime and existing authenticated
message/receipt path. A deterministic observer reads selected tasks through the
Rust CLI, records semantic changes, coalesces updates while the receiver is busy,
and optionally sends one evidence-bearing notification for the current revision.
No task state machine or acceptance judgement is added to JavaScript.

## Acceptance

- `doctor` distinguishes offline, missing/reload-required capability, missing
  enrollment/handoff, observing and notifying. `follow` uses existing enrollment
  or an explicit scope; `--notify` separately opts into model-waking messages.
- Polling lives in the already-running Pi instance. Same inputs do not wake the
  model. Initial state is reported once in notify mode; later semantic changes
  include status/attempt/receipt/evidence/failure changes, not timestamp-only noise.
- Changes remain pending while busy. Notification rechecks current instance,
  enabled enrollment and exact scope, and uses the existing durable receipt flow.
  Intent is persisted first; ambiguous sends never replay automatically.
- Task done is not review acceptance. Receipt excerpts are marked untrusted data;
  the receiver verifies existing gates/authority and does not reopen completed work.
- Explicit pause works offline. Reload requires explicit re-follow; no inherited
  pending notification is replayed into a replacement instance. Notifications are
  capped per subscription (default 10); no dollar-budget guarantee is claimed.
- Tests exercise same-state quietness, receipt-only change, A-B-A revisions, busy
  coalescing, pause/scope revocation, stale instance and unknown delivery. An actual
  Pi offline-provider smoke proves observation -> notification -> real same-session
  response without touching user tasks or paid models.

## Remaining product gaps after this stage

| Capability | Current boundary |
| --- | --- |
| Automatic monitoring | This slice handles explicitly selected task IDs only; no inferred review graph |
| Onboarding | Capability diagnostics and one follow command; existing old runtimes still need idle reload |
| Handoff context | Explicit plan metadata supported; arbitrary prose/authority is not inferred |
| Other runtimes | Codex native tools can send; Claude/Hermes recipient adapters and reverse wake routing remain |
| Management judgement | No autonomous grant resolver or Flash/strong-model routing |
| Remote operation | Same-machine runtime; host sleep/offline and cross-machine transport remain |
| Distribution | Local Pi package path is usable; standalone package publishing/installer remains |

Implement and verify this one stage, obtain independent PR-visible review, then
merge under existing R6 per operator standing authorization. No Cargo build lane.
