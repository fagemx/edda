---
title: CLI Reference
---

# CLI Reference

Complete reference for all `edda` commands.

> Documented for edda 0.4 — the surface below was re-derived from the 0.4.0
> binary, not copied from an older release. `scripts/check-cli-docs.sh`
> enforces that every verb the binary exposes is documented here, either as a
> full section or as a row in the [Internal / experimental](#internal--experimental-commands)
> table.

## Getting started

### `edda init`

Initialize a new `.edda/` workspace in the current directory.

```bash
edda init [--no-hooks]
```

| Option | Description |
|--------|-------------|
| `--no-hooks` | Skip auto-detection and installation of bridge hooks |

Creates `.edda/` with an empty ledger. If `.claude/` is detected, automatically installs Claude Code hooks and adds decision-tracking instructions to `CLAUDE.md`.

### `edda status`

Show workspace status — ledger stats, active branches, hook status.

```bash
edda status
```

### `edda doctor`

Health check for bridge integration.

```bash
edda doctor claude     # check Claude Code hooks
edda doctor cursor     # check Cursor native hooks
edda doctor codex      # check Codex hooks
edda doctor openclaw   # check OpenClaw hooks
```

---

## Memory & querying

### `edda ask`

Query past decisions, history, and conversations.

```bash
edda ask [QUERY] [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `QUERY` | Keyword, domain, or exact key (e.g. `"db.engine"`) |
| `--limit N` | Max results per section (default: 20) |
| `--json` | Output as JSON |
| `--all` | Include superseded decisions |
| `--branch NAME` | Filter by branch |

```bash
edda ask "cache"             # keyword search
edda ask "db.engine"         # exact key lookup
edda ask                     # all active decisions
edda ask --all "auth"        # include superseded
```

### `edda context`

Output the context snapshot — what the agent sees at session start.

```bash
edda context [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--branch NAME` | Branch name (defaults to HEAD) |
| `--depth N` | Number of recent commits/signals to show (default: 5) |

### `edda log`

Query events from the ledger with filters.

```bash
edda log [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--type TYPE` | Filter by event type: `note`, `cmd`, `commit`, `merge`, etc. |
| `--family FAMILY` | Filter by family: `signal`, `milestone`, `admin`, `governance` |
| `--tag TAG` | Filter by tag (matches `payload.tags` array) |
| `--keyword TEXT` | Case-insensitive payload text search |
| `--after DATE` | Events after this date (ISO 8601, e.g. `2026-02-13`) |
| `--before DATE` | Events before this date |
| `--branch NAME` | Filter by branch |
| `--limit N` | Max events to show (default: 50, `0` = unlimited) |
| `--json` | Output as JSON lines |

```bash
edda log                           # recent events
edda log --tag decision            # decisions only
edda log --type cmd                # command events
edda log --after 2026-02-20        # events this week
edda log --keyword "auth" --json   # search + JSON output
```

### `edda search`

Full-text search across transcripts and events (powered by Tantivy).

```bash
edda search index          # build/update search index
edda search query "auth"   # search for text
edda search show TURN_ID   # show full turn content
```

---

## Recording

### `edda note`

Record a note event.

```bash
edda note <TEXT> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--role ROLE` | `user`, `assistant`, or `system` (default: `user`) |
| `--tag TAG` | Tags for the note (repeatable) |

```bash
edda note "completed auth refactor; next: rate limiting" --tag session
edda note "switching to Redis for pub/sub support" --tag decision
```

### `edda decide`

Record a binding decision. Writes to both the workspace ledger and the coordination layer.

```bash
edda decide <DECISION> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `DECISION` | Key=value format (e.g. `"db.engine=postgres"`) |
| `--reason TEXT` | Reason for the decision |
| `--session ID` | Explicit session attribution; otherwise uses process-carried `EDDA_SESSION_ID` |

```bash
edda decide "db.engine=sqlite" --reason "embedded, zero-config"
edda decide "auth.strategy=JWT" --reason "stateless, scales horizontally"
```

### `edda ratify`

Ratify an active decision — confer operator authority (GH-401). An
agent-authored decision from `edda decide` is unratified; ratification is
what makes it binding.

```bash
edda ratify [OPTIONS] <KEY>
```

| Argument / Option | Description |
|-------------------|-------------|
| `KEY` | Decision key to ratify (e.g. `"db.engine"`) |
| `--note TEXT` | Optional note recorded with the ratification |
| `--by TEXT` | Who ratified — recorded for audit; self-asserted, not verified (identity enforcement is a policy-layer concern). Defaults to the resolved session label |
| `--session ID` | Session ID (uses `EDDA_SESSION_ID`; `--session` required when identity is ambiguous) |

```bash
edda ratify "demo.engine" --note "confirmed after load test" --by operator
```

Output:

```
Ratified 'demo.engine' (by operator) — now binding.
  note: confirmed after load test
```

### `edda checkpoint`

Record a vendor-neutral reasoning checkpoint — current hypotheses, rejected
hypotheses with reasons, open questions, and the next action. Use it to make
an investigation resumable by any agent, not only the one that wrote it.

```bash
edda checkpoint [OPTIONS] --next <NEXT>
```

| Option | Description |
|--------|-------------|
| `--next TEXT` | Next checkpoint action (required) |
| `--hypothesis TEXT` | Current hypotheses (repeatable) |
| `--rejected HYPOTHESIS\|REASON` | Rejected hypothesis and reason, separated by `\|` (repeatable) |
| `--open TEXT` | Open questions (repeatable) |
| `--role ROLE` | Author role (default: `agent`) |

```bash
edda checkpoint \
  --hypothesis "lock contention on the ledger" \
  --rejected "sqlite busy_timeout|already configured" \
  --open "does fs2 lock survive fork?" \
  --next "profile the write path under 4 workers"
```

Output (the event id differs per run):

```
Wrote CHECKPOINT evt_01m1h59pn7nn115e3pqvz9cpc8
```

### `edda commit`

Create a commit event in the ledger.

```bash
edda commit --title "Add JWT middleware" [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--title TEXT` | Commit title (required) |
| `--purpose TEXT` | Purpose of this commit |
| `--contrib TEXT` | Contribution description (defaults to title) |
| `--evidence REF` | Evidence refs: `evt_*` or `blob:sha256:*` (repeatable) |
| `--label LABEL` | Labels (repeatable) |
| `--auto` | Enable auto-evidence collection |
| `--dry-run` | Preview without writing to ledger |

### `edda run`

Run a command and record its output in the ledger.

```bash
edda run -- cargo test
edda run -- npm run build
```

---

## Coordination (multi-agent)

Hook integrations carry their session ID into shell commands as
`EDDA_SESSION_ID`; an explicit `--session` overrides it. Without either, the
CLI uses a deterministic `cli-<label>` identity only when no live session is
present. Beside one or more live sessions it refuses before mutating state and
asks for `--session`, because a heartbeat cannot prove which process owns it.
These IDs provide attribution, not authentication or authorization.

### `edda claim`

Claim a scope for coordination. Other agents see claimed paths as off-limits.

```bash
edda claim <LABEL> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `LABEL` | Short label (e.g. `"auth"`, `"billing"`) |
| `--paths PATTERN` | One file path pattern; repeat for multiple patterns |
| `--session ID` | Explicit session attribution; otherwise uses process-carried `EDDA_SESSION_ID` |

```bash
edda claim "auth" --paths "src/auth/*"
edda claim "billing" --paths "src/billing/*" --paths "src/invoice/*"
```

A session holds one active claim. Running `edda claim` again from the same
session replaces its previous label and complete path list; it does not add a
second claim. Pass each path pattern with its own `--paths` flag; comma-separated
values are not split.

### `edda request`

Send a request to another active session.

```bash
edda request <TO> <MESSAGE> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `TO` | Target session label |
| `MESSAGE` | Request message |
| `--session ID` | Explicit session attribution; otherwise uses process-carried `EDDA_SESSION_ID` |
| `--force` | Send even when no active session answers to `TO` |

```bash
edda request "billing" "Please expose invoice total as a public method"
```

`TO` is resolved against active sessions before the request is recorded. A
label nobody answers to is an error — usually a typo — and `--force` queues it
anyway for a peer that has not started yet. A label held by more than one
session is a warning: all of them will see the message.

Unacked requests expire after 7 days (`EDDA_REQUEST_TTL_SECS`), after which
they are reported as expired and dropped by the next `edda gc`.

### `edda request-ack`

Acknowledge requests from a peer.

```bash
edda request-ack <FROM>
```

Rendering a request into an agent's context is delivery, not acknowledgement:
the request keeps appearing until it is acked here, and the ack covers only the
messages outstanding at that moment — later ones from the same peer still
arrive.

### `edda peers`

Show active peer sessions — the read-only view of who else is live, what
each session has claimed, and which requests are outstanding. Shortcut for
`bridge claude peers`.

```bash
edda peers [--json]
```

| Option | Description |
|--------|-------------|
| `--json` | Output sessions, claims, requests, and acknowledgements as JSON |

```bash
edda peers
```

Output (empty workspace):

```
No active sessions.
```

### `edda watch`

Launch the real-time TUI showing active sessions, events, and coordination state.

```bash
edda watch
```

### `edda reconcile`

Recover unfinished Task Rail attempts and dispatch ready work. The ledger,
claims, leases, attempts, and receipts remain authoritative; reconciliation is
safe to invoke repeatedly.

```bash
edda reconcile
edda reconcile --install-scheduler
edda reconcile --uninstall-scheduler
```

| Option | Description |
|--------|-------------|
| `--max-workers N` | Maximum concurrent workers (default: 3) |
| `--max-attempts N` | Retry cap per task (default: 3) |
| `--lease-ttl-s N` | Runner lease lifetime in seconds (default: 300) |
| `--codex-bin PATH` | Codex executable used for runners |
| `--install-scheduler` | On Windows, create or replace this project's one-minute scheduler task |
| `--uninstall-scheduler` | On Windows, remove this project's exact scheduler task if present |

The lifecycle flags are explicit machine mutations and never run from
`edda init` or ordinary reconciliation. They are mutually exclusive and return
without dispatching workers. Windows registration calls `schtasks.exe`
directly under the current user with `LIMITED`; it does not use `SYSTEM`,
`HIGHEST`, a password, shell wrapper, or background daemon.

Each project uses the exact name `Edda-Reconcile-<32-lowercase-hex-project-id>`.
Install uses `/F`, so repeating it replaces only that name. Uninstall queries
and deletes only that exact name and is idempotent; it never terminates a
running reconciler or worker. The compact scheduled command contains canonical
Edda and manifest paths; the repository path lives in the validated manifest,
so linked worktrees share one task and execution does not depend on the
scheduler's working directory.

Scheduler lifecycle is Windows-only. A local exact-name missing-task HRESULT
lifecycle run reported the expected signed and hexadecimal result codes, but
its raw command, output, and Query XML artifacts were not preserved. D1 stopped
`RED / BLOCKED` on a scheduler re-entry defect, and D2–D8 were not run. The
incomplete evidence and rerun requirements are tracked in
[`P2_DRILL_2026-08-16.md`](../plan/task-rail/P2_DRILL_2026-08-16.md); neither the
lifecycle nor the controller-loss drills are release-accepted.

---

## Branches & drafts

### `edda branch`

Branch operations.

```bash
edda branch create <NAME>
```

### `edda switch`

Switch to another branch.

```bash
edda switch <NAME>
```

### `edda merge`

Merge a source branch into a destination branch.

```bash
edda merge <SRC> <DST> --reason "feature complete"
```

### `edda draft`

Draft commit operations — propose changes for review before writing to ledger.

```bash
edda draft propose --title "Add caching layer" [OPTIONS]
edda draft list
edda draft show <DRAFT_ID>
edda draft apply <DRAFT_ID>
edda draft approve <DRAFT_ID>
edda draft reject <DRAFT_ID>
edda draft delete <DRAFT_ID>
edda draft inbox              # show pending approval items
```

---

## Integration

### `edda bridge`

Install or uninstall bridge hooks.

```bash
edda bridge claude install      # install Claude Code hooks
edda bridge claude uninstall    # remove hooks
edda bridge cursor install      # install native Cursor hooks
edda bridge cursor uninstall
edda bridge codex install       # install Codex hooks
edda bridge codex uninstall
edda bridge openclaw install    # install OpenClaw plugin
edda bridge openclaw uninstall
```

### `edda mcp`

Start MCP server (stdio transport, JSON-RPC 2.0).

```bash
edda mcp serve
```

Exposes 7 tools: `edda_status`, `edda_note`, `edda_decide`, `edda_ask`, `edda_log`, `edda_context`, `edda_draft_inbox`.

---

## Maintenance

### `edda config`

Read or write workspace config (`.edda/config.json`).

```bash
edda config list
edda config get <KEY>
edda config set <KEY> <VALUE>
```

### `edda pattern`

Manage classification patterns (`.edda/patterns/`).

```bash
edda pattern add <NAME> --glob "*.test.ts" --class test
edda pattern remove <NAME>
edda pattern list
edda pattern test <FILE_PATH>
```

### `edda rebuild`

Rebuild derived views from the ledger.

```bash
edda rebuild                  # rebuild HEAD branch
edda rebuild --all            # rebuild all branches
edda rebuild --branch main
```

### `edda gc`

Garbage collect expired blobs and transcripts.

```bash
edda gc                          # interactive
edda gc --dry-run                # preview only
edda gc --force                  # skip confirmation
edda gc --keep-days 30           # override retention
edda gc --global                 # also clean global transcript store
edda gc --include-sessions       # also clean session ledgers and stale files
edda gc --archive                # archive instead of delete
edda gc --purge-archive          # purge expired archived blobs
```

### `edda blob`

Manage blob metadata.

```bash
edda blob info <HASH>
edda blob stats
edda blob classify <HASH> --class artifact
edda blob pin <HASH>
edda blob unpin <HASH>
edda blob tombstones
```

### `edda index`

Index operations.

```bash
edda index verify    # verify index entries match store records
```

### `edda verify`

Verify the ledger hash chain — the tamper-evidence check over all events in
`.edda/ledger.db` (parent linkage + canonical hashes). Read-only: the command
never creates, migrates, or writes to the ledger — a missing or unreadable
`.edda/ledger.db` is reported, never silently rebuilt as an empty one.

```bash
edda verify          # human-readable one-line report
edda verify --json   # {"ok": ..., "events": ..., "first_bad_event": ...}
```

Exit codes (same convention as `edda claim check`):

- `0` — chain intact (an empty ledger is OK, not an error)
- `1` — chain broken; the report names the first broken event (including a
  row whose payload is no longer valid JSON — the unreadable row is named)
- `2` — the ledger could not be opened or read (not an edda workspace, or
  `.edda/ledger.db` missing/unreadable)

Example output on a tampered ledger:

```
ledger chain BROKEN at event evt_01J… (3 event(s) scanned): event evt_01J… has invalid hash or digest
```

---

## Task rail, dispatch & gates

### `edda task`

Task rail — create, hand off, and track tasks on the ledger. Agent verbs
(`new`, `start`, `done`, `fail`) mutate tasks; user verbs (`list`, `show`)
are read-only.

```bash
edda task new <TITLE> [OPTIONS]          # create a task (agent verb)
edda task start <ID> [--lease-ttl S]     # take the lease, mark running (agent verb)
edda task done <ID> --receipt TEXT       # complete: done + receipt; successors become ready
edda task fail <ID> --reason TEXT        # mark failed (agent verb)
edda task list [--status S] [--assignee L] [--json] [--fleet]
edda task show <ID> [--json]
```

`edda task new` options:

| Option | Description |
|--------|-------------|
| `--assignee LABEL` | Agent label this task is assigned to (e.g. `worker-1`) |
| `--agent KIND` | Agent transport kind (e.g. `claude-acp`, `codex-acp`) |
| `--after ID` | Task id that must be done first (repeatable — dependencies) |
| `--path PATTERN` | Paths this task may write (repeatable — scope) |
| `--plan PLAN` | Plan this task belongs to |
| `--work-unit UNIT` | Work unit this task delivers |
| `--brief REF` | Brief reference (path or free text) for whoever picks this up |
| `--key KEY` | Idempotency key — the same key never creates a twin task |

`edda task done` requires `--receipt` ("no receipt, no done") and accepts
repeatable `--evidence` paths. `edda task start` records a lease (default
TTL 3600 s, enforced by the P2 reconciler).

```bash
edda task new "Wire up rate limiter" --assignee worker-1 \
  --brief "docs/briefs/rate-limit.md" --key demo-001
```

Output:

```
Created task #1 'Wire up rate limiter' [ready]
```

```bash
edda task list
```

Output:

```
#1 [ready] Wire up rate limiter (assignee: worker-1)
```

```bash
edda task show 1
```

Output:

```
Task #1: Wire up rate limiter
  status:   ready
  assignee: worker-1
  brief:    docs/briefs/rate-limit.md
  created:  2026-09-02T13:35:22.9345246Z
  updated:  2026-09-02T13:35:22.9345246Z
```

### `edda dispatch`

Run one agent turn with no plan file, DAG, or state machine. Reads the
prompt from `--prompt-file` and runs exactly one turn through the selected
backend; loop control stays with the caller.

```bash
edda dispatch --agent <AGENT> --prompt-file <FILE> [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--agent AGENT` | Backend that runs the turn: `claude` (default), `pi`, or `codex` |
| `--prompt-file FILE` | Path to the file containing the prompt, read verbatim (required) |
| `--session-id ID` | Session id passed to the backend verbatim; generated and printed when omitted so the caller can reuse it on the next call. pi and codex resume a prior conversation by repeating the id; claude refuses an id that already exists (`Session ID <id> is already in use`) and needs `--resume` |
| `--resume` | Continue the conversation `--session-id` names instead of starting a new one (`claude --resume <id>`). claude only — pi and codex resume by repeating `--session-id` alone and refuse this flag. Requires `--session-id` |
| `--cwd DIR` | Working directory for the agent (default: current directory) |
| `--budget-usd N` | Per-turn budget in USD (codex cannot enforce budgets) |
| `--timeout-sec S` | Turn timeout in seconds (default: 1800, like a conduct phase) |
| `--permission-mode MODE` | Permission mode carried on the synthetic phase verbatim (default `bypassPermissions`); only the claude backend consumes it today, pi and codex ignore it |
| `--json` | Print exactly one JSON object to stdout instead of text lines |

With `--json` the object has the shape
`{"outcome":"done\|crash\|timeout\|max_turns\|budget_exceeded", "result_text":string\|null, "cost_usd":number\|null, "session_id":string, "error":string\|null}`.

Exit codes:

| Code | Meaning |
|------|---------|
| `0` | agent done |
| `1` | agent crash or any other failure (including pre-dispatch errors) |
| `2` | timeout |
| `3` | budget exceeded |
| `4` | max turns |

A one-turn dispatch against the `pi` backend, where `prompt.txt` contains a
trivial instruction (real transcript, edda 0.4.0):

```bash
edda dispatch --agent pi --prompt-file prompt.txt
```

```
pong
Cost: $0.00
Session: 86929af4-8f4c-58d1-9742-1ba96c1eba94
```

The same turn with `--json` prints exactly one object (real transcript):

```
{"cost_usd":0.000341235,"error":null,"outcome":"done","result_text":"ok","session_id":"37b8267b-e87b-56a4-bbe6-cff142f1f427"}
```

A pre-dispatch failure exits `1` per the table above (real transcript; the
OS-error line is locale-dependent):

```bash
edda dispatch --agent pi --prompt-file missing.txt
```

```
Error: --prompt-file not readable: missing.txt

Caused by:
    系統找不到指定的檔案。 (os error 2)
```

### `edda verdict`

Issue a verdict on a gated subject (approve/reject) — GH-519. The subject is
free-form; for conductor gates it is `<plan-name>/<phase-id>`. Approving a
gate may resume the waiting agent; rejecting feeds the comment back into the
gated agent session as its next turn.

```bash
edda verdict approve [OPTIONS] --sha <SHA> <SUBJECT>
edda verdict reject --sha <SHA> --comment <COMMENT> <SUBJECT>
```

| Argument / Option | Description |
|-------------------|-------------|
| `SUBJECT` | Gated subject (argument); `<plan>/<phase>` for conductor gates |
| `--sha SHA` | Full 40-hex git SHA the verdict applies to |
| `--comment TEXT` | Optional for approve; **required** for reject (fed back to the agent) |
| `--session ID` | Session ID (uses `EDDA_SESSION_ID`; `--session` required when identity is ambiguous) |

```bash
edda verdict approve "demo/phase-1" \
  --sha 0000000000000000000000000000000000000000 --comment "sanity"
```

Output:

```
Verdict recorded: approved demo/phase-1 @ 0000000000000000000000000000000000000000
  event: evt_01m1h59q2exp6xm7h2jyv9ks4r
  comment: sanity
```

> **Freshness (GH-519 D6):** a verdict only satisfies a waiting gate if it
> postdates the gate's `gate_entered_at`. Pre-recording a verdict — approving
> a known SHA *before* the gate opens — does not work: the gate ignores any
> verdict recorded before it entered `AWAITING_VERDICT`, even for the
> matching SHA. Wait for the gate to open, then run the `edda verdict`
> command the conductor prints.

### `edda phase`

Agent phase map, plus approve/reject sugar over `edda verdict` (GH-547).
The status view shows per-plan/per-phase agent state; `phase approve` and
`phase reject` resolve `gate_sha` and session from the persisted conductor
state instead of requiring `--sha` (both remain available as explicit
overrides).

```bash
edda phase [--json]                          # status view
edda phase approve <plan>/<phase> [--comment TEXT]
edda phase reject <plan>/<phase> --comment TEXT
```

`phase reject --comment` is mandatory: the comment becomes the redispatch
prompt for the gated agent session.

```bash
edda phase
```

Output (no conductor state in this workspace):

```
No agent phase data found.
Phase detection runs automatically during Claude Code hook dispatch.
```

---

## Orchestration

### `edda plan`

Plan scaffolding and templates.

```bash
edda plan init     # generate plan.yaml from template
edda plan scan     # scan codebase and suggest a plan
```

### `edda conduct`

Multi-phase AI plan conductor.

```bash
edda conduct run <PLAN.yaml>     # run a plan
edda conduct status              # show running/completed plans
edda conduct retry <PLAN>        # reset a failed phase
edda conduct skip <PLAN>         # skip a phase
edda conduct abort <PLAN>        # abort a running plan
```

---

## Internal / experimental commands

The verbs below exist in the binary but are not part of the recommended
daily surface. Each is either internal plumbing — meant to be invoked by
hooks, schedulers, or other verbs rather than by hand — or experimental,
with semantics that may still change. They are listed here so the
documented surface cannot silently drift from the binary;
`scripts/check-cli-docs.sh` enforces the same invariant.

| Command | What it does | Why not for direct use |
|---------|--------------|------------------------|
| `actor` | Manage project actors (add, remove, list, grant, revoke) | Identity grants are policy-layer plumbing; manage access through operator workflow, not ad-hoc CLI calls |
| `group` | Manage project groups for cross-project sync | Experimental multi-repo grouping; semantics not settled |
| `sync` | Pull shared decisions from group members | Depends on the experimental `group` setup |
| `unclaim` | Release this session's coordination scope | Counterpart of `edda claim`; hooks and reconciliation release scopes, so manual use is rarely needed |
| `coord` | Show coordination state | Shortcut for `bridge claude render-coordination`; rendering plumbing meant for hooks |
| `setup` | Setup a bridge integration | Shortcut for `bridge <platform> install`; use `edda init` or `edda bridge` instead |
| `recap` | Chronicle synthesis — cognitive zoom across sessions | Experimental; output shape may change |
| `export` | Export the ledger as human-readable Markdown (read-only projection; SQLite stays authoritative) | A convenience projection; automation should query the ledger or `edda log`, not parse exports |
| `hook` | Hook entrypoint (called by supported coding-agent hooks) | Internal: expects a hook payload on stdin; hand-invocation writes events with wrong attribution |
| `intake` | Task intake — ingest external tasks into the ledger | Experimental ingest surface |
| `prs` | Scan and record PR events from GitHub | Needs network and a token; normally driven by the scheduler or `edda watch` |
| `pipeline` | Auto-execution pipeline — skill chain with approval gates | Experimental orchestration layer; prefer `edda plan` / `edda conduct` for reviewed plans |
| `bundle` | Create and manage review bundles for rapid approval | Experimental review packaging |
| `brief` | View task engineering briefs (materialized from ledger events) | Read-only viewer normally consumed via `edda task` workflows |
| `policy` | Approval policy management (show, check, init) | Changes gate semantics; edit policy deliberately, not ad hoc |
| `notify` | Push notification management | Plumbing for other verbs; needs a configured channel |
| `pair` | Device pairing and token management | Security-sensitive: tokens grant access; manage from the pairing device |
| `serve` | Start HTTP API server | Long-running process; run it as a managed service, not ad hoc in a shell |
| `user` | User-level aggregation (cross-repo queries, rollup, config) | Experimental cross-project surface |
| `rules` | L3 post-mortem learned rules management | Written by post-mortem runs; hand edits can break TTL-decay semantics |
| `scan` | Capability scanner — identify gaps via LLM analysis | Costs tokens; experimental |
| `propose-issue` | Issue proposal workflow — draft, review, and create GitHub issues | Experimental; requires `gh` authentication |
| `propose-patch` | Controls patch workflow — evaluate quality rules and propose Karvi controls adjustments | Niche governance surface, experimental (references the retired Karvi workflow) |
| `skill` | Manage skill registry (scan, list, show, search) | Experimental registry |
| `tool-tier` | Tool tier governance — query and manage tool risk classifications | Governance plumbing consumed by other tools |
