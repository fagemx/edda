# Bridge core extraction reconnaissance

Issue: #801

Status: proposal; no extraction or behavior change is implemented here.
Basis: `467e8be02eb98fb47d0eca82e9dd400d91d67e6a`.

## Scope and counting

At this basis, `crates/edda-bridge-claude/src` contains **45 Rust files and
31,861 physical lines**, including tests, blank lines and comments. The
measurements in #801 describe an older basis. Counts below are whole-file
physical lengths, measured by enumerating these 45 files and counting their
lines; production-only budgets are stated separately under Slices.

Classification: **A** = harness-neutral, move; **B** = Claude-specific, stay;
**C** = mixed, split as stated. A Claude reference in a comment is not itself
a runtime dependency. Conversely, a module with few keyword matches can call
Claude-specific code indirectly. Unless a full crate path is written, every
source reference in the inventory is relative to
`crates/edda-bridge-claude/src/`. Destinations are proposed module names, not
claims that those modules already exist.

Explicit non-goals: no behavior change, no hook format change, no CLI verb
rename. Preserve historical transcript decoding, digest watermarks, cost
semantics, failure signaling and the current Claude plans-directory fallback.
This proposal does not supersede an existing decision's authority; in
particular, the current extraction location recorded by `bg.extraction_crate`
is an existing implementation choice, and changing it requires a subsequent
implementation decision. No operator ratification is claimed here.

## Module inventory

| Module | Lines | Class | Evidence and split | Proposed destination |
|---|---:|:---:|---|---|
| `admin.rs` | 694 | B | `admin.rs:8` defines the Claude hook command; `admin.rs:45` selects `.claude/settings.local.json`; installation and CLAUDE.md onboarding are harness-owned. | Claude bridge |
| `agent_phase.rs` | 622 | A | `agent_phase.rs:7` imports neutral core phase types; `agent_phase.rs:181` builds a phase map from stored state and peer liveness. | Core `agent_phase` |
| `bg_detect.rs` | 1270 | A | `bg_detect.rs:178` orchestrates deterministic signals and correlation; `bg_detect.rs:18` imports extraction-provider helpers, not hook envelopes. Move that prerequisite first. | Core `background::detect` |
| `bg_digest.rs` | 518 | A | `bg_digest.rs:78` reads stored transcript and writes a summary; `bg_digest.rs:15` imports provider helpers. Historical transcript decoding is a separate dependency below. | Core `background::digest` |
| `bg_extract.rs` | 1406 | C | `bg_extract.rs:547` decodes historical `type=human/assistant` and `message.content`; `bg_extract.rs:836` calls Anthropic HTTP. Separate these from extraction/draft state at `bg_extract.rs:181` and `bg_extract.rs:300`. | Core extraction/drafts; historical decoder in `edda-transcript`; distinct core Anthropic provider module |
| `bg_index.rs` | 69 | A | `bg_index.rs:20` checks index state; `bg_index.rs:37` runs index maintenance. | Core background indexing |
| `bg_scan.rs` | 1266 | A | `bg_scan.rs:292` assembles a project snapshot; `bg_scan.rs:459` renders decision authority from ledger views. Provider helpers come from bg-extract. | Core `background::scan` |
| `controls_suggest.rs` | 689 | A | `controls_suggest.rs:95` evaluates threshold rules; `controls_suggest.rs:289` applies a patch to Karvi. Karvi integration is not Claude-hook coupling. | Core controls |
| `decision_warning.rs` | 330 | A | `decision_warning.rs:52` accepts repo/path/branch and returns Markdown. The `additionalContext` reference at `decision_warning.rs:5` describes its caller; JSON wrapping is outside the function. | Core decision warnings |
| `digest/extract.rs` | 624 | C | `digest/extract.rs:20`, `digest/extract.rs:29`, `digest/extract.rs:55` normalize event/tool names across harnesses; `digest/extract.rs:209` reads stored `hook_event_name`. Separate historical envelope decoding from aggregation. `digest/extract.rs:258` calls the legacy noise-file policy. | Core digest aggregation and explicit historical-envelope compatibility codec |
| `digest/helpers.rs` | 247 | C | `digest/helpers.rs:3`, `digest/helpers.rs:39` read raw envelope shapes; `digest/helpers.rs:162` uses shared usage/pricing. Separate envelope readers from cost/time/path helpers. | Core digest helpers and compatibility codec |
| `digest/mod.rs` | 163 | A | `digest/mod.rs:81` defines `SessionStats`; `digest/mod.rs:134` defines `DigestWatermark`. Neither needs hook dispatch. | Core digest public types |
| `digest/orchestrate.rs` | 1138 | A | `digest/orchestrate.rs:384` digests previous sessions; `digest/orchestrate.rs:873` is the manual trigger. Shared usage read at `digest/orchestrate.rs:720` must move first. | Core digest orchestration |
| `digest/prev.rs` | 353 | A | `digest/prev.rs:134` writes previous-session digest; `digest/prev.rs:340` consumes shared usage state. | Core previous-session state |
| `digest/render.rs` | 165 | A | `digest/render.rs:10` builds ledger digest events; `digest/render.rs:118` builds command milestones. | Core digest event rendering |
| `digest/tests.rs` | 3260 | C | `digest/tests.rs:21` mutates store environment; `digest/tests.rs:41` uses the crate lock. Partition aggregation, codec and state cases with their owning functions; retain store/env isolation. | Core digest tests and decoder-owned codec tests |
| `dispatch/events.rs` | 271 | C | `dispatch/events.rs:3` imports Claude parsing helpers; generic commit/task/subagent event writers start at `dispatch/events.rs:13`, `dispatch/events.rs:170`, `dispatch/events.rs:212`. Split raw decoding from typed emission. | Core typed event helpers; raw hook decoding stays in Claude |
| `dispatch/helpers.rs` | 403 | C | `dispatch/helpers.rs:20` chooses `.claude/plans`; `dispatch/helpers.rs:30` renders an explicit directory. Keep selection and skill/system-reminder wording at `dispatch/helpers.rs:117` in adapters. Extract explicit-input rendering, digest trigger at `dispatch/helpers.rs:134`, prior-session lookup at `dispatch/helpers.rs:183`, Karvi brief lookup at `dispatch/helpers.rs:372`. | Core composition/helpers; Claude selection and skill wording stay |
| `dispatch/mod.rs` | 397 | C | `dispatch/mod.rs:32` is `HookResult`; `dispatch/mod.rs:116` parses/routes Claude hooks. State wrappers already delegate. Move hot-pack read at `dispatch/mod.rs:382` to remove render-to-dispatch dependency. | Claude router/result; core hot-pack reader |
| `dispatch/session.rs` | 981 | C | `dispatch/session.rs:18` ingests/builds a pack; `dispatch/session.rs:195`, `dispatch/session.rs:256` emit `additionalContext`; `dispatch/session.rs:611` syncs lessons to CLAUDE.md. Extract stored-input composition and neutral finalization/notification, retaining hook JSON and CLAUDE.md target selection. | Core composition/finalization; Claude hook adapter |
| `dispatch/tests.rs` | 3238 | C | `dispatch/mod.rs:395` includes this test module. Move helper tests with extracted functions; keep complete hook-entrypoint/output tests in Claude. | Split core and Claude tests |
| `dispatch/tools.rs` | 617 | C | Hook tool dispatch starts at `dispatch/tools.rs:18`, `dispatch/tools.rs:323`; `dispatch/tools.rs:96` reads `EDDA_CLAUDE_AUTO_APPROVE`; `dispatch/tools.rs:205` checks peer claims. Extract query/typed helpers, keep hook policy and JSON. | Core claim query/helpers; Claude hook policy |
| `issue_proposal.rs` | 623 | A | `issue_proposal.rs:75` persists proposals; `issue_proposal.rs:213` consumes a scan gap. | Core proposals |
| `lib.rs` | 54 | C | `lib.rs:1` declares mixed modules; `lib.rs:28` and `lib.rs:29` export installer/hook entrypoint; `lib.rs:34` begins test guard support. | Claude facade; core exports; test support preserved per binary |
| `narrative.rs` | 478 | A | `narrative.rs:15` composes stored signals; `narrative.rs:82` reads peer board state. Move shared signal types/state first. | Core narrative |
| `nudge.rs` | 644 | A | `nudge.rs:26` detects signals from the shape supplied by all four thin adapters; `nudge.rs:123` renders text. Preserve accepted JSON/tool-name shapes. | Core nudge |
| `parse.rs` | 180 | C | `parse.rs:9` defines Claude `EventEnvelope`; `parse.rs:32`, `parse.rs:40` parse input/casing. Timestamp at `parse.rs:69` and project resolution at `parse.rs:75` are neutral. Envelope append at `parse.rs:85` follows envelope ownership. | Claude parser/envelope; core neutral utilities |
| `pattern.rs` | 239 | A | `pattern.rs:51`, `pattern.rs:79`, `pattern.rs:109` load, match and render path patterns. `additionalContext` is rendering commentary. | Core patterns |
| `peers/autoclaim.rs` | 250 | A | `peers/autoclaim.rs:92` derives scope from shared file counts; `peers/autoclaim.rs:123` writes claims. | Core peers |
| `peers/board.rs` | 363 | A | `peers/board.rs:14` derives board state; `peers/board.rs:233` partitions requests. | Core peers |
| `peers/discovery.rs` | 207 | A | `peers/discovery.rs:13` discovers peers using store heartbeats/liveness. | Core peers |
| `peers/heartbeat.rs` | 606 | A | `peers/heartbeat.rs:159` writes minimal heartbeat; `peers/heartbeat.rs:313` writes claims. Claude comments at `peers/heartbeat.rs:136`, `peers/heartbeat.rs:184` describe callers; data uses store-owned heartbeat types. | Core peers |
| `peers/helpers.rs` | 228 | A | `peers/helpers.rs:8`, `peers/helpers.rs:19` resolve labels; remaining helpers normalize paths/timestamps. Shared signal types move first. | Core peers |
| `peers/liveness.rs` | 176 | A | `peers/liveness.rs:54` classifies heartbeat at an explicit time; `peers/liveness.rs:83` supplies current time. | Core peers |
| `peers/mod.rs` | 268 | A | `peers/mod.rs:98`, `peers/mod.rs:159` define claim/board types; `peers/mod.rs:45` reads a generic label environment variable. | Core peers facade |
| `peers/render_coord.rs` | 534 | A | `peers/render_coord.rs:28` renders coordination; `peers/render_coord.rs:110` accepts explicit peer/board inputs. | Core peers rendering |
| `peers/render_fleet.rs` | 377 | A | `peers/render_fleet.rs:43` collects sibling briefs; `peers/render_fleet.rs:142` renders them. | Core fleet rendering |
| `peers/tests.rs` | 3588 | A | `peers/mod.rs:266` includes this module. Sample Claude crate paths in `peers/tests.rs:148` and `peers/tests.rs:153` are path fixtures, not adapter ownership. | Core peer tests |
| `plan.rs` | 674 | A | `plan.rs:43` parses supplied Markdown; `plan.rs:251` renders progress from shared task/commit state. It does not select the Claude directory. | Core plan rendering |
| `redact.rs` | 227 | A | `redact.rs:62` redacts text; `redact.rs:73` recursively redacts JSON. Neither requires Claude wire semantics. | Core redaction |
| `render.rs` | 448 | C | Shared renderer, but `render.rs:356` calls dispatch's hot-pack reader and `render.rs:363` calls Claude plan selection. Core owns pack reading and explicit-directory rendering; adapters select historical paths. | Core rendering; adapter-owned plan selector |
| `signals.rs` | 2252 | C | Shared types at `signals.rs:15`, state at `signals.rs:703`, pricing at `signals.rs:804`, blocking/focus at `signals.rs:901`, `signals.rs:998`; Claude transcript parser at `signals.rs:87` and subagent discovery/decoder at `signals.rs:401` stay. Legacy `.claude/skills` noise policy at `signals.rs:985` is also consumed by digest. | Core types/state/pricing/rendering and explicit legacy noise policy; Claude transcript/subagent decoder stays |
| `state.rs` | 339 | A | `state.rs:13`, `state.rs:97`, `state.rs:169` implement shared counters, injection dedupe, compact-pending state. | Core state |
| `task_nudge.rs` | 346 | C | Selection/text at `task_nudge.rs:22`, `task_nudge.rs:38`; watermark at `task_nudge.rs:62`, `task_nudge.rs:73`. Stop dispatch at `task_nudge.rs:86` uses Claude `HookResult` and block-decision JSON. | Core selection/text/watermark; Claude Stop wrapper |
| `watch.rs` | 39 | A | `watch.rs:15` returns a peer/board snapshot. | Core watch facade |

Here and below, core means the proposed `edda-bridge-core`, not the existing
deterministic `edda-core` crate. The Anthropic HTTP provider is provider-specific
but is not a Claude Code hook adapter. A named provider module avoids a reverse
core-to-hook dependency while retaining background behavior. Replacing the
provider or expanding transcript formats is outside this extraction. Historical
multi-harness envelope decoding remains a compatibility codec in bridge core;
it does not invoke a hook adapter. The bg-extract transcript decoder can move to
`edda-transcript` because it consumes JSON and returns text, without importing
bridge-core types. Claude session/subagent decoders instead stay in the adapter
and import the new core's shared signal types.

## Minimum public API

The illustrative import list in #801 omits calls through imported `render` and
`state` modules and OpenClaw's previous-session digest path. This is the exact
union of existing thin-dispatch references, including test consumers; it is a
minimum public surface, not a promise to delete other CLI/serve exports.

Consumer paths in this table are full repository-relative paths. Existing
symbols must remain reachable through compatibility re-exports until their
callers migrate to core.

| Family | Existing symbols | Direct consumer evidence |
|---|---|---|
| Render | `render::{workspace, pack, plan, writeback, context_budget, apply_budget, wrap_boundary}` | `crates/edda-bridge-codex/src/dispatch.rs:179`; `crates/edda-bridge-cursor/src/dispatch.rs:243`; `crates/edda-bridge-hermes/src/dispatch.rs:194`; `crates/edda-bridge-openclaw/src/dispatch.rs:133` |
| Config | `render::config_bool` | `crates/edda-bridge-openclaw/src/dispatch.rs:349`, `crates/edda-bridge-openclaw/src/dispatch.rs:362` |
| Render test constants | `render::{BOUNDARY_START, BOUNDARY_END}` | `crates/edda-bridge-openclaw/src/dispatch.rs:444`, `crates/edda-bridge-openclaw/src/dispatch.rs:448` |
| State | `state::{increment_counter, read_counter, should_nudge, mark_nudge_sent, is_same_as_last_inject, write_inject_hash, set_compact_pending, take_compact_pending}` | `crates/edda-bridge-codex/src/dispatch.rs:232`, `crates/edda-bridge-codex/src/dispatch.rs:297`; `crates/edda-bridge-cursor/src/dispatch.rs:67`, `crates/edda-bridge-cursor/src/dispatch.rs:389`; `crates/edda-bridge-openclaw/src/dispatch.rs:123` |
| Peers | `peers::{touch_heartbeat, write_heartbeat_minimal, remove_heartbeat, write_unclaim, render_coordination_protocol}` | `crates/edda-bridge-codex/src/dispatch.rs:130`, `crates/edda-bridge-codex/src/dispatch.rs:158`, `crates/edda-bridge-codex/src/dispatch.rs:200`, `crates/edda-bridge-codex/src/dispatch.rs:341` |
| Peer test surface | `peers::{write_claim, compute_board_state}`, returned `BoardState.claims` and claim entries | `crates/edda-bridge-codex/src/dispatch.rs:467`; `crates/edda-bridge-cursor/src/dispatch.rs:494`; `crates/edda-bridge-hermes/src/dispatch.rs:555`; `crates/edda-bridge-openclaw/src/dispatch.rs:972` |
| Nudge | `nudge::{detect_signal, format_nudge, NudgeSignal}`, including `NudgeSignal::SelfRecord` | `crates/edda-bridge-codex/src/dispatch.rs:292`; `crates/edda-bridge-cursor/src/dispatch.rs:150`; `crates/edda-bridge-hermes/src/dispatch.rs:324`; `crates/edda-bridge-openclaw/src/dispatch.rs:218` |
| Digest | `digest::digest_session_manual` | `crates/edda-bridge-codex/src/dispatch.rs:327`; `crates/edda-bridge-cursor/src/dispatch.rs:106`; `crates/edda-bridge-hermes/src/dispatch.rs:362`; `crates/edda-bridge-openclaw/src/dispatch.rs:300` |
| Previous-session digest | `digest::{digest_previous_sessions_with_opts, DigestResult}`; preserve the full enum including payload-bearing `Written` and `PermanentFailure` | `crates/edda-bridge-openclaw/src/dispatch.rs:365`, `crates/edda-bridge-openclaw/src/dispatch.rs:372`, `crates/edda-bridge-openclaw/src/dispatch.rs:376` |

Thin bridges currently obtain Claude-plan behavior through `render::plan`.
The final core API should accept the selected directory: proposed name
`render::plan_from_dir`, not an existing function. Each adapter preserves its
current `EDDA_PLANS_DIR` override and `.claude/plans` fallback. Change these calls
together with dependency removal. Calling the implicit selector neutral would
conceal coupling; changing its default would violate the behavior non-goal.

### ACP needs

#800 has no ACP implementation at this basis. The following are proposed
requirements, not claims of a shipped ACP API:

- **Brief:** use #793's single shared task-block renderer for SessionStart,
  task-start stdout and ACP's first prompt. Preserve guarded output, label
  resolution, truncation and unreadable-file semantics. Its final exported
  symbol depends on #793; no nonexistent renderer name is asserted here.
- **Identity/task input:** existing `peers::{compute_board_state, read_heartbeat,
  resolve_session_label}` and ledger `TaskView`; reuse the current label mechanism.
- **Claim/liveness query:** `peers::{discover_active_peers, compute_board_state,
  classify_session_liveness_at}` and a proposed public explicit-input matcher
  extracted from `crates/edda-bridge-claude/src/dispatch/tools.rs:205`.
- **Heartbeat/finalization:** existing `write_heartbeat_minimal`,
  `touch_heartbeat`, `remove_heartbeat`, and `write_unclaim` on the runner's
  existing ownership/finalization paths.
- **Digest trigger:** existing `digest_session_manual`; runner logging/receipts
  must preserve its failure signal.
- **Composition:** core render/budget/pack primitives; ACP/session wire-envelope
  construction belongs to conductor.

The existing `check_offlimits` is **not a complete ACP authorization policy**.
It uses a cached peer-count short circuit, checks other claims, and explicitly
treats repository-wide `**/*` claims as advisory
(`crates/edda-bridge-claude/src/dispatch/tools.rs:205`). It does not enforce the
task's own scope or verifier execution denial. #800 must compose its required
deny policy from current claim/liveness facts plus task role and scope.
Exporting the old hook helper alone does not meet #800.

## Dependency graph and cycle check

Arrows mean depends on. The final state has no bridge-to-bridge dependency:

```text
edda CLI -> five hook bridges, edda-conductor, edda-bridge-core

edda-bridge-claude   -> edda-bridge-core
edda-bridge-codex    -> edda-bridge-core
edda-bridge-cursor   -> edda-bridge-core
edda-bridge-hermes   -> edda-bridge-core
edda-bridge-openclaw -> edda-bridge-core
edda-conductor      -> edda-bridge-core

edda-bridge-core -> edda-pack, edda-transcript
                -> edda-core, edda-store, edda-ledger
                -> existing aggregate/index/search/notify/postmortem/derive
                   dependencies where needed
```

Core must not depend on conductor, a hook bridge, CLI or serve. Existing
CLI/serve consumers may retain Claude compatibility re-exports while migrating.

A read-only DFS checked the current workspace's manifest path-dependency graph,
added core with Claude's lower-level dependencies, replaced the four thin
bridges' Claude edge with core, and added Claude/conductor-to-core edges.
Result: **acyclic across the proposed 24 workspace packages**. The conservative
core dependency set checked was `edda-aggregate`, `edda-core`, `edda-store`,
`edda-transcript`, `edda-index`, `edda-pack`, `edda-ledger`, `edda-search-fts`,
`edda-notify`, `edda-postmortem`, `edda-derive`. This proves the proposed crate
direction, not compilation of unimplemented code. The two render-to-dispatch
edges and digest-to-signals references must be removed before the public switch.

### Pack ownership

`edda-pack` already owns `PackSection`/`PackItem`
(`crates/edda-pack/src/lib.rs:288`, `crates/edda-pack/src/lib.rs:316`), ordered
rendering (`crates/edda-pack/src/lib.rs:459`), transcript-pack rendering/writing
(`crates/edda-pack/src/lib.rs:511`, `crates/edda-pack/src/lib.rs:527`) and
doctrine/decision-pack construction (`crates/edda-pack/src/lib.rs:558`,
`crates/edda-pack/src/lib.rs:649`, `crates/edda-pack/src/lib.rs:717`).

Keep that lower-level engine there. Gathering peer state, task briefs,
recent-session state and adapter-selected plans belongs in bridge core, which
calls `edda-pack`. Moving peer discovery down into `edda-pack` would invert the
boundary. For #681, generic section types/renderers still belong in `edda-pack`;
writers that gather bridge/session context belong in bridge core. Composition
extraction does not mean moving the pack engine wholesale.

## Slices

Each extraction PR moves at most about 3,000 production lines. Budgets below
count production-source lines including comments and glue, excluding test-only
files and `#[cfg(test)]` bodies. Recount the actual diff at its frozen basis.
Preserve old exports through forward wrappers/re-exports between slices. No core
module may reverse-import Claude. If concurrent work grows a slice past its
ceiling, split at its stated dependency boundary before opening that PR.

| Slice | Budget | Contents and order |
|---|---:|---|
| S1: leaf state/data | <=2000 | Create core; state (228 pre-test lines), nudge (331), redact (105), pattern (145), decision warning (165), bg-index (50), <=700 shared signal type/state/pricing/noise-policy lines; remaining budget for facade and neutral timestamp/project helpers. Claude transcript parsing stays and imports core types. |
| S2: coordination | <=2800 | Peers production surface (2516), watch (39), <=245 exports/helper glue. Requires S1 shared types and utilities. |
| S3: composition/brief | <=2600 | Plan (363), narrative (166), agent phase (357), render (366), explicit-directory plan helper, hot-pack reader, neutral task-nudge operations. Reserve <=500 for #793's renderer if available; remaining budget for glue. Keep Claude Stop JSON and implicit path selection in adapters. |
| S4: digest | <=2900 | Digest production code (2613), <=287 glue. Preserve historical codec, prefix hashes, watermarks, failure variants and cost semantics. Requires S1 usage state/noise policy. |
| S5: extraction/background digest | <=2000 | Bg-extract pre-test code (1012), bg-digest (316), controls-suggest (327), <=345 glue. Historical transcript decoder goes to the lower transcript layer; Anthropic provider is named explicitly. |
| S6: scans/proposals | <=2200 | Bg-detect (793), bg-scan (814), issue-proposal (315), <=278 glue. Requires S5 provider support. |
| S7: remaining dispatch helpers | <=2900 | Only neutral event/finalization/claim/brief helpers from session/tools/events/helpers/parser. Those files' entire current production source is under 2400 lines; move a subset with <=500 glue. HookResult, raw hook parsing, JSON, auto-approve, skill wording and CLAUDE.md target selection stay. |
| S8: consumer switch | <=1000 changed production lines | Four thin bridge imports/manifests and adapter-owned plan selectors; ready conductor/CLI consumers; remove cross-bridge dependency edges. Retain stable Claude re-exports for other CLI/serve consumers. Split further if the measured diff requires it. |

Each slice must pass its applicable gates and leave the workspace green before
the next builds on it. Forward re-exports/wrappers preserve intermediate callers;
temporary compile failures are not an acceptable slicing technique. Follow the
single canonical verification ladder in `.claude/CLAUDE.md`, rather than copying
its procedure here.

Windows CI currently tests seven packages, including **both
`edda-bridge-claude` and `edda-conductor`**
(`.github/workflows/ci.yml:159`, `.github/workflows/ci.yml:160`). The contrary
sentence in #800 is stale. The new core is outside that subset unless a separate
CI change adds it, so moving tests out of Claude removes their current Windows-CI
execution. The canonical verifier Windows-gap selector therefore applies to
core; it also applies to touched transcript/thin-bridge crates in S5/S8.
Workspace-wide Clippy does not substitute for these runtime tests.

### Test movement and #757

| Test source (under the Claude source directory) | Slice and destination |
|---|---|
| `state.rs` inline tests | S1 -> core state |
| `signals.rs` inline tests | S1 moves shared state/pricing/noise-policy tests; transcript/subagent tests remain with the Claude decoder |
| `agent_phase.rs` inline tests | S3 -> core phase |
| `render.rs` inline tests | S3 -> core rendering; adapter plan-selection tests stay with adapters |
| `bg_digest.rs` inline tests | S5 -> core background digest |
| `bg_detect.rs`, `bg_scan.rs` inline tests | S6 -> core detect/scan |
| `dispatch/tests.rs` | S3/S7 move neutral-helper tests only; complete Claude protocol tests remain |
| `digest/tests.rs` | S4 -> core digest; preserve historical codec cases |
| `peers/tests.rs` | S2 -> core peers |

The first seven rows map #757's reported failure-name set. The final two are
also relevant environment/store consumers. This recon neither fixes #757 nor
claims its 20-run acceptance. Coordinate with its implementation before moving
tests. Preserve the injected config/store fixture or lock arrangement it
establishes; do not introduce separate per-module locks for tests that still
share process-global environment. A core crate's `#[cfg(test)]` guard is not
automatically available when core is compiled as another crate's normal
dependency.

## Evidence and delivery boundary

READ baseline: **CI run 33850770481 @
467e8be02eb98fb47d0eca82e9dd400d91d67e6a**. Format and CI Gate passed; code jobs
skipped correctly for that docs-only head. This is a baseline docs receipt,
not proof of newly run workspace code gates. The documentation PR records its
own exact-head CI receipt separately, so no self-referential source commit is
needed to add a run ID.

This deliverable is docs-only: exact-head CI is its gate, no local Cargo gate
and no regression test apply. Source inspection, line counts, manifest-graph DFS
and document validation are the recon evidence. Independent review is required;
the recon author does not review their own proposal.

No extraction is shipped here, and no completion is claimed for #793, #800,
#610, #681 or #757. #800 may use its explicitly permitted temporary Claude-crate
imports without waiting for extraction. #610 can use this inventory to define
adapter obligations without treating a hook adapter as the shared core.
