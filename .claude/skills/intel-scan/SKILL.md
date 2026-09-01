---
name: intel-scan
description: "Dual-agent PR intelligence scan — discover contribution opportunities from any repo's PR history"
---

# Intel Scan Skill

You are a PR intelligence coordinator. Your job is to scan a target repo's PR history using two parallel agents (technical + social), synthesize their findings, and identify contribution opportunities.

This skill works with ANY GitHub repo, not just a specific project.

## Argument Parsing

Parse the `args` parameter. Format:

```
<owner/repo> [options]
```

| Argument | Default | Description |
|----------|---------|-------------|
| `owner/repo` | (required) | Target GitHub repo |
| `--author <name>` | all | Only scan PRs by this author |
| `--limit <n>` | `30` | Scan last N merged PRs |
| `--since <date>` | last scan time | Scan PRs merged after this date (YYYY-MM-DD) |
| `--topic <keyword>` | all | Filter PRs by keyword in title or changed files |
| `--full` | false | Ignore history, full rescan |

Examples:
```
anthropics/claude-code
anthropics/claude-code --author durner
anthropics/claude-code --limit 50 --topic mcp
anthropics/claude-code --full
fagemx/edda --limit 20
```

## Storage Strategy

Three storage locations, each with a different purpose:

### 1. Machine State — `$APPDATA/edda/intel/<owner>/<repo>/state.json`

Machine-readable only. Used for incremental scanning (knowing where to resume).

```json
{
  "last_scan": "2026-02-12T10:00:00Z",
  "last_pr_number": 2897,
  "known_authors": ["author1", "author2", "author3"],
  "threads": { ... },
  "profiles": { ... },
  "opportunities": { ... }
}
```

### 2. Structured Report — intel branch on user's fork of target repo

Human-readable + git-diffable. Stored on a dedicated `intel` branch in the user's fork.

Determine the fork repo by checking `git remote -v` for `origin` in the local clone of the target repo. If working directory is the target repo, use `origin`. Otherwise, derive from the target repo (e.g., `anthropics/claude-code` → user's fork `fagemx/claude-code`).

**Branch**: `intel` (create if not exists, never PR to upstream)
**Directory**: `intel/`

```
intel/
  report-latest.md     ← full report (overwritten each scan)
  profiles/
    maintainer1.md     ← per-maintainer profile
    maintainer2.md
  threads/
    runner-rustification.md   ← per-thread history (append-only)
```

Workflow to update the intel branch:
```bash
# In the local clone of the target repo
cd <local-clone-path>
git fetch origin
git checkout intel 2>/dev/null || git checkout --orphan intel
# Write/update files
git add intel/
git commit -m "intel: scan <date>, PR #<from>~#<to>"
git push origin intel
git checkout -  # return to previous branch
```

### 3. Scan Summary — GitHub issue comment on edda repo

Append-only timeline of scan summaries. Each scan adds a comment to a tracking issue.

**Repo**: `fagemx/edda`
**Issue**: One issue per target repo, titled `intel: <owner/repo> scan log`

First scan → create the issue with full report as body.
Subsequent scans → add comment with incremental summary.

```bash
# First scan: create issue
gh issue create --repo fagemx/edda \
  --title "intel: anthropics/claude-code scan log" \
  --body "<full report>" \
  --label "intel"

# Subsequent scans: add comment
gh issue comment <issue-number> --repo fagemx/edda \
  --body "<incremental summary>"
```

## Workflow

### Step 1: Read State

```bash
STATE_DIR="$APPDATA/edda/intel/<owner>/<repo>"
mkdir -p "$STATE_DIR"
```

Read `state.json` if it exists. If not, this is a first scan.

Also check if a tracking issue exists:
```bash
gh issue list --repo fagemx/edda --label intel --search "intel: <owner/repo>" --json number,title --limit 1
```

### Step 2: Fetch PR List

Build the `gh` command based on parsed arguments:

```bash
gh pr list --repo <owner/repo> --state merged --json number,title,author,mergedAt,labels,url --limit <limit>
```

Filter results:
- If NOT `--full` and `state.json` exists: only keep PRs with number > `last_pr_number`
- If `--author` specified: filter by author login
- If `--topic` specified: filter by keyword match in title
- Remove bot PRs (author.is_bot == true, or title starts with "chore: release")

If no new PRs found, report "No new PRs since last scan" and stop.

### Step 3: Dispatch Agents

Launch two Task agents **in parallel** (both as background tasks).

Agent A reads code (diffs) and outputs compressed finding cards.
Agent B reads social signals (comments, reviews) and outputs profiles.
Neither agent does cross-thread analysis — that's YOUR job in Step 4.

#### Agent A Prompt Template (Code Reader)

```
You are Agent A (Code Reader) analyzing {owner/repo}.

Your job is to READ ACTUAL CODE (diffs, not just PR descriptions) and output compressed finding cards. You do NOT synthesize across threads — the Coordinator does that.

## Already Known (from previous scans)
{Insert known threads and their PR chains from state.json, or "First scan — no prior history" if new}

## New PRs to Analyze
{Insert the filtered PR list with numbers and titles}

## Your Tasks

### For each PR, do:

1. **Read the full diff** (CRITICAL — do not skip):
   ```
   gh pr diff <number> --repo <owner/repo>
   ```
   If the diff is very large (1000+ lines), read it in sections or focus on non-test source files.

2. **Read PR metadata**:
   ```
   gh pr view <number> --repo <owner/repo> --json body,files
   ```

3. **Analyze the code** — look for:
   - `unwrap()` / `expect()` / `panic!()` in runtime paths (not tests)
   - Missing error handling or swallowed errors
   - Hardcoded values that should be configurable
   - Duplicated logic (same pattern in multiple files)
   - New public functions/traits/types without tests
   - TODO / FIXME / HACK in the actual diff
   - Unchecked items in the PR test plan — **but verify with CI** (see step 3b)

3b. **Validate unchecked test plan items** (CRITICAL — prevents false positives):
   Unchecked items do NOT automatically mean there's a problem. Before flagging:
   ```
   gh run list --repo <owner/repo> --branch main --limit 10 --json conclusion,name
   ```
   - If CI is all green since that PR merged → the unchecked item is "unconfirmed", not "broken"
   - Only flag as opportunity if there are ACTUAL failures or the gap is logically provable from code alone
   - "No tests exist for new public API" is valid. "Test plan checkbox unchecked" alone is NOT.

3c. **Detect infrastructure barriers**:
   For PRs touching low-level infra (sandbox, VM, kernel, hardware), check:
   - CI config: does the test job require special runners (self-hosted, KVM, GPU)?
   - Cargo.toml / package.json: dependencies on platform-specific tools (firecracker, QEMU, /dev/kvm)?
   - README or CONTRIBUTING.md: documented setup requirements?
   If external contributors cannot run the tests locally, flag as HIGH BARRIER in the finding card.

4. **For high-churn threads (3+ PRs)**, also check current file state:
   ```
   gh api repos/<owner>/<repo>/contents/<path> --jq .content | base64 -d
   ```
   Look for leftover artifacts from rapid changes.

## Output Format — Finding Cards

For EACH PR, output exactly ONE finding card in this format:

~~~markdown
### PR #<number> — <title>
- **Author**: <login>
- **Scope**: +<add>/-<del>, <N> files
- **Thread**: <thread-name> (PR N/M) | Independent
- **Code findings**:
  - `<file>:<line>`: <what's wrong> — `<code snippet>`
  - `<file>:<line>`: <what's wrong> — `<code snippet>`
- **Test gaps**: <new public API without tests, unchecked test plan items>
  - Mark each gap as: `[CI-verified]` (actual failures seen) or `[unconfirmed]` (checkbox only)
- **Infra barrier**: None | LOW | HIGH (<reason>) — can external contributors run these tests?
- **Velocity note**: <if part of a fast chain, note merge-to-merge time>
~~~

Rules:
- **Every finding must cite a specific file and line** (or diff hunk)
- **Include a short code snippet** (the problematic line, not the whole function)
- If a PR has NO code findings, write: `- **Code findings**: None (clean PR)`
- Do NOT write cross-thread analysis — that's the Coordinator's job
- Keep each card to 5-15 lines — compress, don't dump
```

#### Agent B Prompt Template (Social Reconnaissance)

```
You are Agent B (Social Reconnaissance) analyzing {owner/repo}.

## Already Known (from previous scans)
{Insert known profiles from state.json, or "First scan — no prior profiles" if new}

## New PRs to Analyze
{Insert the filtered PR list with numbers, titles, and authors}

## Your Tasks
1. For each PR, run:
   - `gh pr view <number> --repo <owner/repo> --comments` for discussion
   - `gh api repos/<owner>/<repo>/pulls/<number>/comments --jq '.[] | "\(.user.login): \(.body[0:200])"'` for inline review comments
2. For each active author, update their profile:
   - Active territories (which areas they're working on)
   - Review preferences (what they push back on)
   - Explicit signals ("we should do X", "TODO", "need help")
3. Analyze interaction patterns:
   - Who reviews whom
   - Who merges their own PRs vs needs approval
   - How external contributors are treated

## Output Format
Return a structured markdown report:
- Updated maintainer profiles
- New signals detected (with exact quotes)
- Social risk assessment for contribution
```

### Step 4: Synthesize (Coordinator)

When both agents return, the Coordinator (you) performs three layers of analysis.

Agent A gives you: compressed finding cards (per-PR code evidence).
Agent B gives you: maintainer profiles and social signals.

#### 4a. Thread grouping

Group Agent A's finding cards into threads:
- Match to known threads from state.json
- Create new threads for related PR clusters
- Note thread velocity (merge timestamps from PR metadata)

#### 4b. Cross-thread pattern analysis

This is YOUR job — Agent A deliberately does not do this. Look across all threads for:

1. **Convergence needs**: Multiple threads solving similar problems differently
   - e.g., retry logic in guest-download vs reconnect handling in ably-subscriber
2. **Structural debt**: Workarounds that indicate a deeper systemic issue
   - e.g., a workaround PR to trigger release → release pipeline config problem
   - **BUT**: Before flagging, check project convention docs (`.claude/skills/`, `CONTRIBUTING.md`, `docs/`)
     to verify it's actually a problem and not an intentional design decision
3. **Quality gradient**: Some areas getting heavy attention while adjacent areas rot
4. **Missing abstractions**: Same code pattern appearing 2+ times across threads
5. **Velocity risk**: Threads with 5+ PRs in 24h — joints between PRs often have gaps

#### 4c. Score and prioritize

For each code finding (from Agent A) and each cross-thread pattern (from 4b), check Agent B's social data:

```
Code evidence + maintainer explicitly mentioned it = ⭐⭐⭐⭐⭐
Code evidence + TODO/FIXME in actual code           = ⭐⭐⭐⭐
Code evidence + natural extension of thread         = ⭐⭐⭐
Code evidence + no social signal                    = ⭐⭐
Code evidence + in someone's active territory       = ⭐ (risky)
```

**False positive filters** (MUST check before scoring — learned from Scan #1 validation):

| Pattern | Looks like opportunity | Actually is | How to detect |
|---------|----------------------|-------------|---------------|
| Unchecked test plan | "Tests missing!" | Nobody confirmed yet, CI is green | `gh run list` — all green = not broken |
| Workaround PR | "Config is broken!" | Intentional design decision | Search `.claude/skills/`, `docs/`, `CONTRIBUTING.md` for documentation |
| Infra code without tests | "No test coverage!" | Requires special hardware/env | Check CI for self-hosted runners, check deps for KVM/QEMU/GPU |
| Self-merge author territory | "I can help with tests!" | Author may not welcome external PR | Check if author has EVER reviewed/merged external contributions |

**Scoring penalty for false positive risk:**
- Finding is `[unconfirmed]` (not `[CI-verified]`) → cap at ⭐⭐⭐ max
- Infra barrier is HIGH → cap at ⭐⭐ max (external contributor can't even test)
- Documented as intentional design → ⭐ zero, skip entirely

#### 4d. Write actionable briefs (CRITICAL)

For each opportunity rated ⭐⭐⭐+, write a full brief:

```markdown
## Opportunity: <name>

### Background
<2-3 sentences: what happened upstream, why this gap exists, why it matters>

### Evidence
- `<file>:<line>`: `<code snippet>` — <what's wrong>
- PR #XXXX diff: <description of the problematic pattern>
- PR #XXXX review: "<exact quote from maintainer>"

### What to do
1. <Concrete, verifiable step>
2. <Concrete, verifiable step>
3. <Concrete, verifiable step>

### Social feasibility
- ✅/⚠️/❌ <Who owns this area, how they'll react>
- ✅/⚠️/❌ <Historical pattern — how similar PRs were received>
- ✅/⚠️/❌ <Risk factors — territory conflict, roadmap alignment>

### Verdict
✅ **直接做** / ⚠️ **先開 issue 討論** / ❌ **Skip** — <one line reasoning>
```

Rules:
- **Every brief must have code-level evidence** (file:line or diff hunk). No brief based solely on PR descriptions.
- **Evidence from Agent A's finding cards**, not from re-reading PR descriptions.
- **Social feasibility from Agent B's profiles**, not from assumptions.

#### 4e. Update existing opportunities

- Still active? Keep.
- Someone did it? Mark `taken` with PR reference.
- Thread went inactive? Mark `stale`.

### Step 5: Save State & Reports

#### 5a. Update machine state
Update `$APPDATA/edda/intel/<owner>/<repo>/state.json` with:
- `last_scan` timestamp
- `last_pr_number` (highest PR number scanned)
- Updated threads, profiles, opportunities

#### 5b. Update intel branch
In the local clone of the target repo:

1. Checkout or create the `intel` branch (orphan branch — no shared history with main)
2. Write/update files:
   - `intel/report-latest.md` — full synthesized report (overwrite)
   - `intel/profiles/<login>.md` — per-maintainer profile (overwrite with latest)
   - `intel/threads/<thread-name>.md` — per-thread history (append new scan section)
3. Commit and push to origin

```bash
cd <local-clone-path>
git stash --include-untracked 2>/dev/null
git fetch origin
git checkout intel 2>/dev/null || git checkout --orphan intel && git rm -rf . 2>/dev/null
mkdir -p intel/profiles intel/threads
# ... write files ...
git add intel/
git commit -m "intel: scan $(date +%Y-%m-%d), PR #<from>~#<to>"
git push origin intel
git checkout - 2>/dev/null
git stash pop 2>/dev/null
```

#### 5c. Update edda issue
On `fagemx/edda` repo:

- **First scan**: Create issue with full report
  ```bash
  gh issue create --repo fagemx/edda \
    --title "intel: <owner/repo> scan log" \
    --body "<full report markdown>" \
    --label "intel"
  ```

- **Subsequent scans**: Add comment with incremental summary
  ```bash
  gh issue comment <number> --repo fagemx/edda \
    --body "<incremental summary: new threads, opportunity updates, maintainer dynamics>"
  ```

### Step 6: Output Summary

Present to user in this format:

```markdown
## Intel Scan: <owner/repo>
**Scanned**: PR #<from> ~ #<to> (<count> PRs, <date range>)
**Previous scan**: <date> | **First scan**: yes/no

### Thread Updates
- <thread-name>: +N PRs (<brief>)
- 🆕 <new-thread>: N PRs (<brief>)

### Actionable Opportunities

For each ⭐⭐⭐+ opportunity, present the full brief (from Step 4b):
- Background, Evidence, What to do, Social feasibility, Verdict

### Skipped / Closed
| Opportunity | Status | Reason |
|------------|--------|--------|
| <name> | taken | seven332 did it in #XXXX |
| <name> | skip | in someone's territory, no signal |

### Top Recommendation
> <One sentence: the single best contribution opportunity right now, with verdict>

**Saved to**:
- State: `$APPDATA/edda/intel/<owner>/<repo>/state.json`
- Report: `<fork-repo>` branch `intel` → `intel/report-latest.md`
- Log: `fagemx/edda#<issue-number>` (new comment added)
```

## Important Rules

1. **Never create issues or PRs on the TARGET repo** — only analyze and report
2. **Always use `--repo` flag** with `gh` commands — don't assume current directory
3. **Quote maintainer words exactly** — signals must be traceable to source
4. **Agents work in parallel** — use Task tool with `run_in_background: true` for both
5. **Incremental by default** — only rescan everything if `--full` is specified
6. **Generic** — works with any public GitHub repo, no hardcoded project knowledge
7. **Intel branch is orphan** — never shares history with main, never PR to upstream
8. **Issue comments are append-only** — never edit previous comments, only add new ones
9. **Ask user before creating intel branch or edda issue for the first time** — confirm fork repo and branch name
