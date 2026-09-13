# Selected session evidence and owner inbox

Issue #1170 extends the merged explicit work handoff loop. An Edda task remains
the work identity; sessions are explicit replaceable execution bindings, never
implicitly discovered writers. Scope is integrations/agent-manager plus these docs.
S7 coord-run/prepare, Rust task verbs, fleet scripts and live project code are excluded.

## Contract

Selected agents support existing Pi and explicitly configured local Codex rollout
files. Codex observation is recorded evidence, not proof that a process is alive.
No process scan, remote arbitrary URL or browser-selected file is introduced.
Codex conversation is read-only in this slice; Pi retains the existing exact-target
send adapter. Hide reasoning, raw tool arguments/results and raw provider strings.
Reject identity/workspace mismatch and unsupported/corrupt records visibly.

Bind each selected execution session to a selected work through the Edda workflow
event chain. Record agent identity, role (manager/worker/reviewer), parent agent,
optional reviewed full SHA, expected next event, and explicit deadline. Preserve
binding history; replacing an instance never implies that an old operation did not
run. A binding mismatch or missing source is unknown, not permission to launch.

Project work views expose these bindings and observed session state. Observations
may produce durable, deduplicated inbox events: reply ended without delivery,
provider error, interrupted/stopped, source unavailable, and expected-event overdue.
Event keys include binding identity and native event identity, not poll time.
Owner acknowledgement records an event reference and evidence; it does not mean
successful task delivery, semantic instruction agreement or new runtime authority.
The owner's bounded API context includes unresolved inbox items, current work,
pending instructions and evidence pointers; it never copies whole transcripts.

An explicit next-event deadline distinguishes a long test from ordinary work.
Overdue means suspected stalled, never automatic kill/cancel/reassignment. Native
completion is not a product verdict. Provider errors expose bounded normalized
categories/status only. Child events newer than a manager summary mark it stale.

## Recovery and scope of delivery

Persist the inbox event before exposing it and deduplicate through restart.
Continue using original transport IDs for any explicit Pi notification; no blind
replay or automatic new assignment. The owner inbox is directly readable through
the authenticated API and work UI. Any optional wake request is separately observed;
inbox persistence must not claim that a sleeping owner has read it.

Owners can hand off the bounded work packet to a selected replacement manager while
preserving pending instructions, inbox acknowledgements and original targets. This
is an explicit handoff, not automatic model rotation based on cumulative tokens.

## Validation

Use synthetic explicit Codex rollouts with confirmed native task_started,
task_complete and turn_aborted shapes, and real selected-file read-only observation.
Test corrupt/partial tails, oversized records, append offsets, identity mismatch,
reasoning filtering and source disappearance. Test Pi provider metadata, missing
heartbeat, child completion/owner acknowledgement, restart dedup, binding replacement,
deadline passage and summary staleness. Prove unknown state never launches/sends.
Exercise the owner inbox with an isolated task and channel, then verify desktop and
mobile UI. Existing handoff tests remain green. No production synthetic messages.
