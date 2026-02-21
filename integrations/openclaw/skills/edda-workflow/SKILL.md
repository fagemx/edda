---
name: edda-workflow
description: "Post-task notification and draft approval via edda (decision memory). Use when: (1) a coding agent finishes and you need to report results, (2) checking for pending edda drafts that need approval, (3) user says 'approve', 'reject', or asks about draft status. NOT for: recording decisions (use `edda decide` directly), querying past decisions (use `edda query`), or session context (handled automatically by edda bridge)."
metadata:
  {
    "openclaw":
      {
        "emoji": "📋",
        "requires": { "bins": ["edda"] },
      },
  }
---

# Edda Workflow: Task Notification + Draft Approval

After a coding agent completes work, use this skill to report results and handle pending draft approvals.

**Context injection and session digests are handled automatically by the edda bridge plugin.** This skill only covers the notification and approval workflow.

## When to Use

- A coding agent (Codex, Claude Code, Pi) finishes a task
- User asks about pending drafts or approval status
- User wants to approve or reject a draft

## 1. Post-Task Report

After a coding agent completes, gather results and send a clear report.

### Gather info

Run these commands in the project directory:

```bash
# What changed
git diff --stat HEAD~1 2>/dev/null || git diff --stat

# Test results (if applicable)
cargo test 2>&1 | tail -5    # Rust
npm test 2>&1 | tail -10     # Node

# Pending drafts
edda draft inbox --json
```

### Report format

Send a single message with this structure:

```
Task completed: [brief description]

Changes:
- [file1]: [what changed]
- [file2]: [what changed]
(N files changed, +X insertions, -Y deletions)

Tests: [passed/failed — one line summary]

[If pending drafts exist:]
Pending approval:
- [draft_id]: [title] (stage: [stage_id], needs [N] approval(s))

Reply "approve [draft_id]" or "reject [draft_id]" to decide.
```

Keep it concise. The user is reading this on a phone.

### If task failed

```
Task failed: [brief description]

Error: [error message — keep to 2-3 lines max]

[If the error is actionable:]
Suggested fix: [one-liner]
```

### Progress updates (long tasks)

For tasks taking >2 minutes, send a brief progress update:

```
Progress: [task description]
- Completed: [what's done]
- Current: [what's running now]
- Remaining: [what's left]
```

Only send progress updates at meaningful milestones, not every 30 seconds.

## 2. Draft Approval

When the user wants to approve or reject a draft:

### Check pending drafts

```bash
edda draft inbox --json
```

Each line is a JSON object:
```json
{"draft_id":"drf_...","title":"...","branch":"main","stage_id":"lead","role":"lead","min_approvals":1,"current_approvals":0,"approvals_needed":1,"assignees":["alice"]}
```

### Approve

```bash
edda draft approve [draft_id] --by human --stage [stage_id]
```

After approval, check if all stages are now approved:
```bash
edda draft list --json
```

If status is "approved", ask the user if they want to apply:
```
Draft [draft_id] approved. All stages passed.
Apply now? (This writes the commit to the ledger.)
```

If user confirms:
```bash
edda draft apply [draft_id]
```

### Reject

```bash
edda draft reject [draft_id] --by human --stage [stage_id] --note "[reason if given]"
```

### Show draft details

```bash
edda draft show [draft_id]
```

## 3. Recognizing User Intent

The user may not use exact commands. Map natural language:

| User says | Action |
|-----------|--------|
| "approve", "ok", "lgtm", "go ahead", "核准", "通過" | `edda draft approve ...` |
| "reject", "no", "deny", "不行", "拒絕" | `edda draft reject ...` |
| "status", "what's pending", "有什麼要審的" | `edda draft inbox --json` |
| "apply", "commit it", "寫入" | `edda draft apply ...` |
| "show me the draft", "details", "看一下" | `edda draft show ...` |

If there's only one pending draft, assume the user means that one. If multiple, ask which one.

## 4. Error Handling

- **edda not found**: Tell the user `edda` is not installed or not in PATH.
- **No pending drafts**: "No pending drafts. Nothing to approve."
- **Draft already applied**: "This draft was already applied."
- **Stage not found**: List available stages and ask user to specify.
- **Branch mismatch**: "Draft is on branch X but current branch is Y. Switch first."
