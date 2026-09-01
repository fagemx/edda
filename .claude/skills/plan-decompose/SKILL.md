---
name: plan-decompose
description: "Decompose planning docs into GitHub issues — extract gaps, deduplicate, confirm with human, batch create"
---

# Plan Decompose

You are a planning decomposition specialist. Your job is to read a planning document (capability assessment, roadmap, gap analysis, etc.) and turn it into actionable GitHub issues — with deduplication, cross-referencing, and human confirmation before any issue is created.

## Usage

```
plan-decompose <path-to-planning-doc>
plan-decompose <path> --repo <owner/repo>
plan-decompose <path> --repos karvi,edda
```

- **Required**: path to planning document
- **Optional**: `--repo` target repo (default: current repo), `--repos` for multi-repo decomposition

## When to Use This

- You have a capability assessment, gap analysis, or roadmap document
- The document contains multiple work items (gaps, features, tasks) that should become issues
- You want structured batch creation with human confirmation, not one-at-a-time

**Do NOT use for**: single issues (use `issue-create`), implementation planning (use `issue-plan`), code discovery (use `issue-scan`).

---

## Workflow

### Step 1: Parse Document Structure

Read the planning document and identify its structural patterns:

```bash
cat <path-to-planning-doc>
```

Look for:
- **Tables with gaps/features/tasks** — the primary source of work items
- **Priority groupings** — P0/P1/P2, Must/Should/Could, Phase 1/2/3
- **Existing issue references** — `#214`, `#273`, `GH-XXX` — these already exist, don't recreate
- **Acceptance criteria / DoD** — if the doc has them, preserve them
- **Capability/category groupings** — used for labels
- **Cross-product boundaries** — items belonging to different repos

Also check what issues already exist:

```bash
gh issue list --state open --limit 200 --json number,title,labels
```

### Step 2: Extract Work Items

For each identifiable work item, extract:

| Field | Source |
|-------|--------|
| **title** | Gap name or description |
| **priority** | P0/P1/P2 from document grouping |
| **category** | Which capability/section it belongs to |
| **description** | What needs to be done |
| **dod** | Definition of Done (if document provides it) |
| **verification** | How to verify it's done (if document provides it) |
| **repo** | Which repo this belongs to (if multi-repo) |
| **existing_issue** | If document references an existing `#NNN` |
| **blocked_by** | Dependencies on other work items |
| **related** | Related items in this batch |

**Deduplication rules:**
1. If the document references `#NNN` for a gap → mark as `skip (exists)`
2. If the same gap appears in multiple sections (e.g., P0 summary table AND capability-specific table) → merge into one, keep the richer description
3. If an open issue already covers the same scope (fuzzy match on title) → mark as `skip (exists)` with issue number

### Step 3: Classify and Label

**Repo assignment** (for multi-repo projects):
- Code-level execution concerns → karvi
- Decision/memory/governance concerns → edda
- Integration items → issue in both repos, cross-referenced with `owner/repo#NNN`

**Labels** — derive from document structure, not generic best practices:
- Priority: `P0`, `P1`, `P2`
- Category: from document sections (e.g., `controllability`, `trust`, `efficiency`, `composability`)
- Type: `enhancement` (gaps/features), `bug` (defects), `tech-debt` (refactoring)
- Cross-cutting: `karvi-edda-integration` (for items spanning both products)

**Dependencies:**
- If item B requires item A to exist first → note `blocked-by: A`
- Don't over-specify — only mark hard dependencies, not "nice to have first"

### Step 4: Present Confirmation Table (CRITICAL)

**Never create issues without human confirmation.** Present the full list:

```markdown
## Plan Decomposition: <document name>

**Source**: `<path>`
**Items found**: X total (Y to create, Z to skip)
**Target repo(s)**: owner/repo

### Issues to Create

| # | Title | Repo | Labels | Priority | Blocked-by | Notes |
|---|-------|------|--------|----------|------------|-------|
| 1 | feat: enforce budget gate before dispatch | karvi | enhancement, P0, controllability | P0 | — | DoD from doc |
| 2 | feat: add RBAC with minimum 3 roles | karvi | enhancement, P0, trust | P0 | — | DoD from doc |
| 3 | feat: decision dependency graph | edda | enhancement, P0 | P0 | — | — |

### Skipped (already exist)

| # | Title | Existing Issue | Reason |
|---|-------|---------------|--------|
| — | Kill step semantics | #214 | Referenced in doc |
| — | Cancel task race condition | #274 | Referenced in doc |

### Summary

- **Create**: Y issues (X in karvi, Z in edda)
- **Skip**: N items (already have issues)
- **Labels to create**: list any labels that don't exist yet

Proceed? (yes / adjust / cancel)
```

**Wait for human response.** Do not proceed until confirmed.

If user says "adjust" — ask what to change and regenerate the table.

### Step 5: Batch Create

After confirmation, create issues in order (P0 first, then P1, then P2):

```bash
# Create labels that don't exist yet
gh label create "P0" --description "Priority 0 — must have" --color D73A4A 2>/dev/null
gh label create "P1" --description "Priority 1 — should have" --color FFA500 2>/dev/null
gh label create "P2" --description "Priority 2 — nice to have" --color 0E8A16 2>/dev/null
# ... category labels as needed

# Create each issue
gh issue create \
  --title "feat: enforce budget gate before dispatch" \
  --body "$(cat <<'EOF'
## Context

<extracted from planning doc — what capability this serves, why it matters>

## What Needs to Happen

<extracted description of the gap>

## Definition of Done

<from doc if available, otherwise derive from description>

## Verification

<from doc if available>

## References

- Source: `<path-to-planning-doc>` (P0 #1)
- Related: #NNN, #NNN

---
*Decomposed from planning doc by plan-decompose*
EOF
)" \
  --label "enhancement,P0,controllability"
```

**Issue body structure:**
1. **Context** — why this matters (from doc's capability/section intro)
2. **What Needs to Happen** — the actual work (from gap description)
3. **Definition of Done** — verifiable criteria (from doc's DoD table, or derived)
4. **Verification** — how to check (from doc's verification column, or derived)
5. **References** — back-link to source doc + related issues
6. **Footer** — `Decomposed from planning doc by plan-decompose`

**Title format:** Conventional commit style — `feat:`, `fix:`, `refactor:`, `chore:` etc.

Collect all created issue URLs and numbers.

### Step 6: Cross-Reference

After all issues are created:

1. **Add cross-references** — for items with `blocked-by` or `related`, edit the issue body or add a comment:
   ```bash
   gh issue comment <NUMBER> --body "Blocked by #<DEP_NUMBER>. Related: #<REL1>, #<REL2>"
   ```

2. **Comment on existing issues** that were referenced in the doc:
   ```bash
   gh issue comment <EXISTING_NUMBER> --body "Referenced in product-capability-assessment.md as P0 gap. DoD and verification criteria defined in that document."
   ```

3. **Update source document** — add issue numbers back to the doc's tables:
   ```markdown
   | P0 | Budget 強制執行 | 可控+信任 | #350 |
   ```
   Only do this if the document has a clear table structure that can be updated cleanly.

### Step 7: Report

```markdown
## Decomposition Complete

**Source**: `<path>`
**Created**: X issues
**Skipped**: Y items (already existed)
**Labels created**: Z new labels

### Created Issues

| # | Issue | Title | Priority |
|---|-------|-------|----------|
| 1 | #350 | feat: enforce budget gate before dispatch | P0 |
| 2 | #351 | feat: add RBAC with minimum 3 roles | P0 |
| ... |

### Cross-References Added

- #214 ← commented (referenced in doc)
- #350 → blocked-by: none
- #351 → related: #350

### Source Document Updated

- Added issue numbers to <path> (N rows updated)
```

---

## Multi-Repo Handling

When `--repos karvi,edda` is specified:

1. Parse items and assign repos in Step 2-3
2. In the confirmation table, show repo column clearly
3. Create issues in the correct repo:
   ```bash
   # For karvi
   gh issue create --repo fagemx/karvi --title "..." --body "..."

   # For edda
   gh issue create --repo fagemx/edda --title "..." --body "..."
   ```
4. Cross-repo references use full format: `fagemx/edda#XX`

**Integration items** (e.g., "Edda defines RBAC policy → Karvi enforces"):
- Create one issue per repo, each describing that repo's part
- Cross-reference: "Karvi side: fagemx/karvi#350, Edda side: fagemx/edda#42"

---

## Decision Framework

| Situation | Decision |
|-----------|----------|
| Gap has DoD in document | Use it verbatim in issue body |
| Gap has no DoD | Derive from description — keep it minimal, let implementer refine |
| Same gap in multiple sections | Merge — use the richest description, note all capability tags |
| Gap references existing issue | Skip creation, add comment to existing issue |
| Fuzzy match with existing issue | Show in confirmation table with "possible duplicate?" note |
| Doc mentions future work (Phase 2/3) | Still create issues but label `P2` — don't filter out |
| Unclear which repo | Ask in confirmation step |
| 50+ items in one doc | Group by priority in confirmation table, suggest creating P0 first |

## Anti-Patterns

1. **Silent batch create** — Never create issues without showing the confirmation table first
2. **Generic labels** — Labels should come from the document's own categories, not boilerplate
3. **Duplicating existing issues** — Always check `gh issue list` and document references
4. **Over-specifying DoD** — If the doc doesn't have DoD, write 2-3 lines max, not an essay
5. **Ignoring document structure** — The doc's grouping (by capability, by priority) IS the issue structure
6. **Creating issues for discussion items** — Only create for actionable work, not for "核心洞察" paragraphs
7. **Losing provenance** — Every issue must link back to the source document
