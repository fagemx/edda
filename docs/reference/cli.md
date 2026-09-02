---
title: CLI Reference
---

# CLI Reference

Reference for the public `edda` command surface. The command index below is
checked against `edda --help` in CI so newly added top-level commands cannot be
silently omitted from this page.

## Command index

| Area | Commands |
|------|----------|
| Setup and health | `edda init`, `edda setup`, `edda status`, `edda doctor`, `edda config`, `edda user`, `edda rules` |
| Memory and decisions | `edda actor`, `edda note`, `edda checkpoint`, `edda decide`, `edda ratify`, `edda group`, `edda sync`, `edda ask`, `edda recap`, `edda context`, `edda log`, `edda commit` |
| Tasks and coordination | `edda task`, `edda claim`, `edda unclaim`, `edda request`, `edda request-ack`, `edda peers`, `edda coord`, `edda reconcile`, `edda verdict` |
| Branches and exchange | `edda branch`, `edda switch`, `edda merge`, `edda draft`, `edda export`, `edda brief`, `edda bundle` |
| Search and maintenance | `edda search`, `edda index`, `edda blob`, `edda pattern`, `edda rebuild`, `edda gc`, `edda scan` |
| Agent integration | `edda bridge`, `edda hook`, `edda mcp`, `edda run`, `edda dispatch`, `edda pair`, `edda serve`, `edda skill`, `edda tool-tier` |
| Planning and automation | `edda plan`, `edda conduct`, `edda intake`, `edda phase`, `edda prs`, `edda pipeline`, `edda policy`, `edda watch`, `edda notify`, `edda propose-issue`, `edda propose-patch` |

## Getting started

### `edda init`

Initialize a new `.edda/` workspace in the current directory.

```bash
edda init [--no-hooks] [--force-skills]
```

| Option | Description |
|--------|-------------|
| `--no-hooks` | Skip auto-detection and installation of bridge hooks |
| `--force-skills` | Refresh generated `coord-*` skills, overwriting local edits to those generated files |

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

Record a decision in the workspace ledger and coordination layer. Agent and
system decisions remain unratified until an operator explicitly runs
`edda ratify`.

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

---

## Reasoning, tasks & coordination (0.3–0.4)

### `edda checkpoint`

Record portable reasoning state in the ledger. Hypotheses, rejected paths, and
open questions are repeatable; `--next` is required.

```bash
edda checkpoint \
  --hypothesis "The registry is stale" \
  --rejected "README typo|crates.io reports 0.2.1" \
  --open "Is the publish token available?" \
  --next "Package every workspace crate"
```

### `edda ratify`

Confer operator authority on the currently active decision for a key. `--by`
is audit metadata and is self-asserted; it is not an identity check.

```bash
edda ratify <KEY> [--note <NOTE>] [--by <ACTOR>] [--session <SESSION>]
```

### `edda task`

Create and execute dependency-aware, hash-chained tasks.

```bash
edda task new <TITLE> [--assignee <LABEL>] [--agent <KIND>] \
  [--after <TASK_ID>]... [--path <PATH>]... [--plan <PLAN>] \
  [--work-unit <UNIT>] [--brief <BRIEF>] [--key <IDEMPOTENCY_KEY>]
edda task start <ID> [--lease-ttl <SECONDS>]
edda task done <ID> --receipt <RECEIPT> [--evidence <PATH>]...
edda task fail <ID> --reason <REASON>
edda task list [--assignee <LABEL>] [--status <STATUS>] [--json] [--fleet]
edda task show <ID> [--json]
```

A completed task requires a receipt. Task status is derived from ledger events;
`--after` dependencies keep a task blocked until its predecessors finish.

### Claims and peer state

Claim write scope before editing, inspect intersections without mutating state,
then release the claim during teardown.

```bash
edda claim <LABEL> --paths <PATH_OR_GLOB> [--session <SESSION>]
edda claim check <PATH_OR_GLOB>... [--json]
edda unclaim [--session <SESSION>] [--if-claimed]
edda peers [--json]
edda coord [--session <SESSION>]
```

`edda claim check` exits `0` when scopes are disjoint, `1` for a conflict, and
`2` for a usage or runtime error. `--if-claimed` makes teardown idempotent once
session attribution is unambiguous; pass `--session` when multiple live sessions
make implicit identity ambiguous.

### `edda dispatch`

Run exactly one agent turn without a plan file or conductor state machine. The
prompt is read verbatim from a file; the caller owns any outer loop.

```bash
edda dispatch \
  --agent <claude|pi|codex> \
  --prompt-file <PATH> \
  [--session-id <ID>] \
  [--cwd <DIR>] \
  [--budget-usd <USD>] \
  [--timeout-sec <SECONDS>] \
  [--permission-mode <MODE>] \
  [--json]
```

Omitting `--session-id` generates and prints one for later continuation. Claude,
Pi, and Codex persist continuity through their own backend mechanisms; Codex
stores the session-to-thread map in Edda's per-user store. Codex reports cost
when available but cannot enforce `--budget-usd`.

| Exit | Outcome |
|------|---------|
| `0` | Agent completed |
| `1` | Crash, pre-dispatch error, or other failure |
| `2` | Timeout |
| `3` | Budget exceeded |
| `4` | Maximum turns reached |

With `--json`, a dispatched turn renders exactly one stdout object containing
`outcome`, `result_text`, `cost_usd`, `session_id`, and `error`. A pre-dispatch
failure, such as an unreadable prompt file, exits `1` and reports to stderr
before JSON rendering begins.

### `edda verdict` and `edda phase`

Verdicts are append-only operator decisions pinned to a full Git SHA.

```bash
edda verdict approve <SUBJECT> --sha <FULL_SHA> [--comment <TEXT>] \
  [--session <SESSION>]
edda verdict reject <SUBJECT> --sha <FULL_SHA> --comment <TEXT> \
  [--session <SESSION>]
```

For a live conductor gate, `edda phase` resolves the gate SHA and session from
persisted conductor state. Explicit `--sha` and `--session` overrides remain
available.

```bash
edda phase [--json]
edda phase approve <PLAN>/<PHASE> [--comment <TEXT>] \
  [--sha <FULL_SHA>] [--session <SESSION>]
edda phase reject <PLAN>/<PHASE> --comment <TEXT> \
  [--sha <FULL_SHA>] [--session <SESSION>]
```

With the default `on_reject: redispatch`, a rejection comment becomes the next
prompt while attempt and redispatch bounds remain. With `on_reject: halt`, or
after those bounds are exhausted, the phase fails instead of launching another
turn.

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
edda conduct run <PLAN.yaml> [--agent <claude|pi|codex>]
edda conduct status              # show running/completed plans
edda conduct retry <PLAN>        # reset a failed phase
edda conduct skip <PLAN>         # skip a phase
edda conduct abort <PLAN>        # abort a running plan
```

`--agent` selects the backend for every phase and defaults to `claude`. Use
`--dry-run` to inspect the phase graph and any operator gates before execution.
