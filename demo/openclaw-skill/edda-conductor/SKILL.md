---
name: edda-conductor
description: "Multi-phase AI plan orchestration via edda-conductor. Use when: (1) user wants to run a multi-phase plan, (2) checking conductor status or progress, (3) handling blocked/failed phases (retry, skip, abort), (4) generating plan.yaml from natural language. NOT for: single-task execution (use coding-agent), decision recording (use edda decide), session context (handled by edda bridge)."
metadata:
  {
    "openclaw":
      {
        "emoji": "🎭",
        "requires": { "bins": ["edda"] },
      },
  }
---

# Edda Conductor: Multi-Phase Plan Orchestration

Orchestrate multi-phase AI plans using `edda conduct`. Each phase spawns a Claude Code agent with specific instructions and automated checks.

## When to Use

- User asks to run a multi-step plan (e.g., "build the API project")
- Heartbeat detects active conductor state
- User asks about plan status or progress
- A phase fails and needs a decision (retry, skip, abort)
- User wants to create a plan.yaml from a description

## 1. Start a Plan

### From an existing plan.yaml

```bash
# Interactive (foreground, shows live output)
edda conduct run plan.yaml

# Background (for unattended execution)
nohup edda conduct run plan.yaml --quiet > /dev/null 2>&1 &
```

### Verify it started

```bash
edda conduct status --json
```

### Report to user

```
Plan started: [name]
Phases: [N] total
Running in background.

I'll monitor progress via heartbeat.
```

### Important flags

- `--quiet`: Suppress live agent output (use for background)
- `--dry-run`: Preview plan without executing (always use before first run)
- `--cwd <path>`: Override working directory

## 2. Check Status

Run this command to get machine-readable status:

```bash
edda conduct status --json
```

### Interpret the JSON output

The output is a `PlanState` object (or array if multiple plans):

```json
{
  "plan_name": "api-decisions",
  "plan_status": "running",
  "total_cost_usd": 0.42,
  "phases": [
    {"id": "scaffold", "status": "passed", "attempts": 1},
    {"id": "endpoints", "status": "running", "attempts": 1},
    {"id": "tests", "status": "pending", "attempts": 0}
  ]
}
```

### Status values

**Plan status:** `pending`, `running`, `blocked`, `completed`, `aborted`
**Phase status:** `pending`, `running`, `checking`, `passed`, `failed`, `skipped`, `stale`

### Report format (phone-optimized)

```
Plan: [name] — [status]
Progress: [passed]/[total] phases
Current: [current_phase_id]
Cost: $X.XX

[If blocked:]
BLOCKED: phase "[id]" failed
Error: [error message]
Reply: retry [id] / skip [id] / abort
```

Keep it concise. The user reads this on a phone.

### When status command shows no state

```
No active conductor plans.
```

## 3. Generate plan.yaml

When the user describes a multi-step task, convert it to plan.yaml format.

### Workflow

1. Understand the user's goal
2. Break into logical phases with clear prompts
3. Add checks for each phase (file_exists, file_contains)
4. Write the plan.yaml file
5. Validate: `edda conduct run plan.yaml --dry-run`
6. Show dry-run output to user for approval
7. Only start after user confirms

### Template

```yaml
name: short-kebab-name
description: |
  One-line description of what this plan does.
max_attempts: 2
timeout_sec: 300

phases:
  - id: phase-one
    prompt: |
      Clear instructions for the first phase.
      Be specific about what files to create/modify.
    check:
      - type: file_exists
        path: expected/output/file.py
      - type: file_contains
        path: expected/output/file.py
        pattern: "expected content"

  - id: phase-two
    prompt: |
      Instructions for the second phase.
    depends_on: [phase-one]
    check:
      - type: file_exists
        path: another/file.py
```

### Check types available

- `file_exists`: Verify a file was created (`path` required)
- `file_contains`: Verify file contains a pattern (`path` + `pattern` required)

### Tips for good plans

- Each phase should produce verifiable output (files, content)
- Use `depends_on` to express ordering between phases
- Keep prompts specific — the agent has no context from previous phases
- Set `max_attempts: 2` so failed phases get one retry automatically
- Add `timeout_sec` appropriate to phase complexity (300s for small, 600s for large)

## 4. Handle Errors

When plan status is `blocked`, a phase has failed beyond max attempts.

### Read error details

```bash
edda conduct status --json
```

Look for phases with `"status": "failed"` and their `error` field:

```json
{
  "id": "tests",
  "status": "failed",
  "attempts": 2,
  "error": {
    "error_type": "check_failed",
    "message": "file tests/test_api.py does not contain pattern 'test_'",
    "retryable": true
  }
}
```

### Report to user

```
Plan BLOCKED: [plan_name]
Phase "[id]" failed after [N] attempts.
Error: [message]

Options:
- retry [id] — reset and try again
- skip [id] — skip this phase, continue
- abort — stop the whole plan
```

### Execute user decision

**Retry:**
```bash
edda conduct retry [phase-id]
edda conduct run plan.yaml --quiet
```

**Skip:**
```bash
edda conduct skip [phase-id] --reason "user: [reason if given]"
edda conduct run plan.yaml --quiet
```

**Abort:**
```bash
edda conduct abort
```

After retry or skip, you must re-run `edda conduct run` to resume execution.

## 5. User Intent Mapping

The user may not use exact commands. Map natural language:

| User says | Action |
|-----------|--------|
| "run the plan", "start", "execute" | Start plan (Section 1) |
| "status", "progress", "how's it going" | Check status (Section 2) |
| "make a plan for X", "plan out X" | Generate plan.yaml (Section 3) |
| "retry", "try again" | Retry failed phase (Section 4) |
| "skip", "move on", "next" | Skip phase (Section 4) |
| "abort", "stop", "cancel" | Abort plan (Section 4) |
| "dry run", "preview" | `edda conduct run plan.yaml --dry-run` |

If there's only one active plan, assume the user means that one. If multiple, ask which one.

## 6. Error Handling

- **edda not found**: Tell the user `edda` is not installed or not in PATH.
- **No active plans**: "No active conductor plans."
- **Multiple plans**: List them by name, ask user which one.
- **Stale status**: If `updated_at` in runner-status.json is older than `timeout_sec`, the conductor process likely crashed. Report: "Plan status is stale (last update: [time]). The conductor may have crashed. Re-run to resume."
- **Conductor not built**: If `edda conduct` fails, suggest `cargo build -p edda-cli`.

## 7. Heartbeat Integration

When a conductor plan is actively running, add the following to the project's HEARTBEAT.md:

```markdown
### Conductor monitoring
Check edda conductor status for active plans:
1. Run `edda conduct status --json` in [project-dir]
2. If status changed since last check, report the change
3. If blocked, report error and ask user for decision
4. If completed, report summary with total cost
5. If no active plans, respond with HEARTBEAT_OK
```

### Behavior rules

- **No change since last check** → respond with `HEARTBEAT_OK` (no Telegram delivery, saves cost)
- **Phase completed** → report which phase passed and what's running next
- **Plan completed** → report summary: phases passed, total cost, time elapsed
- **Plan blocked** → report error details with retry/skip/abort options
- **No active plans** → respond with `HEARTBEAT_OK`

### Lifecycle

- Add monitoring instructions to HEARTBEAT.md when a plan starts
- Remove them when the plan completes or is aborted
- If user starts a new plan, update the project directory in the instructions
