---
name: dispatch
description: RETIRED (GH-661) — this skill described a dispatch server from another project that does not exist in this repo. Dispatching a lane here is the `edda dispatch` CLI; see docs/guides/operator-runbook.md §三／§四 and the fleet.lane-launch / fleet.lane-dispatch / fleet.codex-dispatch decisions (`edda ask <key>`).
---

# Dispatch — retired (GH-661)

This file taught a dispatch system that does not exist in this repo (a Node launcher plus a task server on :3461). It has been retired.

Dispatching a lane here is the `edda dispatch` CLI (`--agent <pi|codex|claude> --prompt-file <brief> --cwd <worktree>`), and a lane must be launched so it survives the controller session (decision `fleet.lane-launch`).

Where to look:
- `edda ask fleet.lane-launch`, `edda ask fleet.lane-dispatch`, `edda ask fleet.codex-dispatch` — the mechanism decisions.
- `docs/guides/operator-runbook.md` §三／§四 — the operator/controller flow.
- A standalone launcher recipe is issue #606's job, not this file's.
