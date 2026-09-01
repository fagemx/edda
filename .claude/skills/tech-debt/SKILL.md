---
name: tech-debt
description: Technical debt management - scan Rust codebase for bad smells and create tracking issues
context: fork
---

# Technical Debt Management Skill

You are a technical debt management specialist for the edda project. Your role is to scan the Rust codebase for code quality issues and help track technical debt systematically.

## Operations

This skill supports two operations:

1. **research** - Fast scan to locate suspicious files and detailed analysis
2. **issue** - Create GitHub issue based on research findings

Parse the operation from the `args` parameter:
- `research` - Scan codebase and generate detailed report
- `issue` - Create GitHub issue from research results (auto-runs research if not done)

## Operation 1: Research

Perform a comprehensive scan of the codebase to identify technical debt using fast pattern matching followed by detailed analysis.

### Usage

```
research
```

### Workflow

#### Phase 1: Fast Scan

Use fast pattern matching to locate suspicious files. Search in the `crates/` directory for:

**1. Large Files (>500 lines)**
```bash
find crates -type f -name "*.rs" -exec wc -l {} + | awk '$1 > 500 {print $1, $2}' | sort -rn
```

**2. TODO/FIXME/HACK Comments**
```bash
grep -rn "TODO\|FIXME\|HACK\|XXX\|STUB" crates/ --include="*.rs"
```

**3. Unimplemented/Todo Macros**
```bash
grep -rn "todo!()\|unimplemented!()\|panic!(\"not implemented" crates/ --include="*.rs"
```

**4. Dead Code Markers**
```bash
grep -rn "#\[allow(dead_code)\]\|#\[allow(unused" crates/ --include="*.rs"
```

**5. Unwrap/Expect in Non-Test Code**
```bash
# Find unwrap/expect in production code (exclude test modules)
grep -rn "\.unwrap()\|\.expect(" crates/ --include="*.rs" | grep -v "#\[cfg(test)\]" | grep -v "tests/" | grep -v "_test.rs"
```

**6. Clippy Suppressions**
```bash
grep -rn "#\[allow(clippy::" crates/ --include="*.rs"
```

**7. Large Match Arms / Deeply Nested Code**
```bash
# Files with deep nesting (proxy for complexity)
grep -rn "                    " crates/ --include="*.rs" -l
```

**8. Hardcoded Values**
```bash
# Hardcoded paths, URLs, magic numbers
grep -rn 'http://\|https://\|/tmp/\|/var/' crates/ --include="*.rs" | grep -v "test\|example\|doc"
```

**9. Clone Usage (potential performance issues)**
```bash
grep -rn "\.clone()" crates/ --include="*.rs" -l
```

**10. Unsafe Code**
```bash
grep -rn "unsafe " crates/ --include="*.rs"
```

**11. Missing Error Context**
```bash
# Bare ? without .context() or .map_err()
grep -rn "\?\s*$\|?\s*;" crates/ --include="*.rs" | grep -v "context\|map_err" | head -30
```

**12. Compilation Warnings**
```bash
cargo check 2>&1 | grep "warning"
cargo clippy 2>&1 | grep "warning"
```

#### Phase 2: Detailed Analysis

For each file identified in Phase 1, perform detailed analysis:

1. **Read the full file content**
2. **Categorize issues** by bad smell type
3. **Calculate severity** (Critical/High/Medium/Low)
4. **Identify specific violations** with line numbers
5. **Suggest remediation** strategies

**Severity Levels:**
- **Critical (P0)**: Zero-tolerance violations that must be fixed
  - `unwrap()` / `expect()` in runtime paths (not tests)
  - `panic!()` in library code
  - `unsafe` without safety comment
  - Dead code suppressions hiding real issues
- **High (P1)**: Significant issues that should be fixed soon
  - Files >800 lines (needs splitting)
  - Missing error context (bare `?`)
  - Hardcoded values that should be configurable
  - `todo!()` / `unimplemented!()` in shipped code
- **Medium (P2)**: Issues that should be addressed
  - Files >500 lines
  - Excessive `.clone()` usage
  - Clippy suppressions
  - TODO/FIXME comments
- **Low (P3)**: Minor issues or code smells
  - Deep nesting (>4 levels)
  - Inconsistent naming
  - Missing documentation on public APIs

#### Phase 3: Generate Report

Create detailed report in `/tmp/tech-debt-YYYYMMDD/`:

**Directory Structure:**
```
/tmp/tech-debt-YYYYMMDD/
├── summary.md              # Executive summary
├── statistics.md           # Statistics and metrics
├── critical/               # P0 issues
│   ├── unwrap-usage.md
│   ├── unsafe-code.md
│   └── dead-code.md
├── high/                   # P1 issues
│   ├── large-files.md
│   ├── missing-context.md
│   └── hardcoded-values.md
├── medium/                 # P2 issues
│   ├── clone-usage.md
│   ├── clippy-suppressions.md
│   └── todo-comments.md
└── low/                    # P3 issues
    ├── deep-nesting.md
    └── missing-docs.md
```

**File Format for Each Issue:**

```markdown
# [Issue Type] - [Severity]

## Overview
- **Total Files Affected:** {count}
- **Total Violations:** {count}
- **Estimated Effort:** {hours/days}

## Affected Files

### {file-path}
**Lines:** {line-count}
**Violations:** {count}

**Issues:**
1. Line {number}: {description}
   ```rust
   {code-snippet}
   ```
   **Remediation:** {suggestion}

2. Line {number}: {description}
   ...

---

### {next-file}
...

## Remediation Strategy
{overall-strategy-for-this-issue-type}

## References
- Severity: P{n}
- Related skill: {link}
```

**Summary Report Format:**

```markdown
# Technical Debt Analysis Summary

**Scan Date:** {date}
**Scan Scope:** crates/ directory
**Total Files Scanned:** {count}
**Total Files with Issues:** {count}

## Executive Summary

{2-3 paragraph overview of findings}

## Statistics by Severity

| Severity | Files | Violations | Est. Effort |
|----------|-------|------------|-------------|
| Critical | {n}   | {n}        | {hours}     |
| High     | {n}   | {n}        | {hours}     |
| Medium   | {n}   | {n}        | {hours}     |
| Low      | {n}   | {n}        | {hours}     |
| **Total**| {n}   | {n}        | {hours}     |

## Top Issues

### Critical Issues (Must Fix)
1. **Unwrap in runtime**: {count} files, {violations} violations
2. **Unsafe code**: {count} files
3. **Dead code suppressions**: {count} files

### High Priority Issues
1. **Large files**: {count} files
2. **Missing error context**: {count} files
3. **Todo/unimplemented macros**: {count} files

### Medium Priority Issues
1. **Clone usage**: {count} files
2. **Clippy suppressions**: {count} files
3. **TODO comments**: {count} files

## File Statistics

### Largest Files (Top 10)
1. {file-path} - {lines} lines
2. {file-path} - {lines} lines
...

### Most Violations (Top 10)
1. {file-path} - {count} violations
2. {file-path} - {count} violations
...

## Recommended Action Plan

### Phase 1: Critical Issues (1 week)
- [ ] Replace unwrap/expect with proper error handling
- [ ] Add safety comments to unsafe blocks
- [ ] Remove dead code suppressions (delete the dead code)

### Phase 2: High Priority (2 weeks)
- [ ] Split large files into focused modules
- [ ] Add .context() to bare ? operators
- [ ] Replace hardcoded values with config

### Phase 3: Medium Priority (1 month)
- [ ] Reduce clone() usage
- [ ] Address clippy suppressions
- [ ] Resolve TODO/FIXME comments

### Phase 4: Low Priority (ongoing)
- [ ] Reduce deep nesting
- [ ] Add missing public API docs

## Detailed Reports

- [Critical Issues](./critical/)
- [High Priority Issues](./high/)
- [Medium Priority Issues](./medium/)
- [Low Priority Issues](./low/)

---
*Generated by tech-debt skill on {date}*
```

#### Phase 4: User Report

After generating detailed reports, provide a **medium-detail summary** to the user:

```markdown
# Technical Debt Scan Complete

## Scan Results

**Total Files Scanned:** {count}
**Files with Issues:** {count}
**Total Violations:** {count}

## By Severity

- **Critical (P0):** {count} files, {violations} violations
- **High (P1):** {count} files, {violations} violations
- **Medium (P2):** {count} files, {violations} violations
- **Low (P3):** {count} files, {violations} violations

## Top 5 Critical Issues

1. **Unwrap in runtime** - {count} files
   - {file-path}:{line} - {brief-description}
   - {file-path}:{line} - {brief-description}

2. **Unsafe code** - {count} files
   - {file-path}:{line} - {brief-description}

3. **Dead code suppressions** - {count} files
   - {file-path}:{line} - {brief-description}

4. **Large files** - {count} files
   - {file-path} - {lines} lines
   - {file-path} - {lines} lines

5. **Missing error context** - {count} files
   - {file-path}:{line} - {brief-description}

## Detailed Reports

All detailed analysis has been saved to `/tmp/tech-debt-{date}/`

- Summary: `/tmp/tech-debt-{date}/summary.md`
- Statistics: `/tmp/tech-debt-{date}/statistics.md`
- Critical issues: `/tmp/tech-debt-{date}/critical/`
- High priority: `/tmp/tech-debt-{date}/high/`

## Next Steps

Run `tech-debt issue` to create a GitHub issue tracking these findings.
```

### Implementation Notes

**Efficiency Tips:**
- Use `grep -l` (files only) for fast scanning
- Use `wc -l` for line counts
- Combine multiple greps with parallel execution
- Only read files that match patterns
- Cache scan results for issue operation

**Accuracy Tips:**
- Exclude `target/` and `.git` directories
- Exclude generated files
- Use word boundaries in regex for precision
- Verify matches by reading actual file content
- Distinguish test code (`#[cfg(test)]`) from production code

---

## Operation 2: Issue

Create a GitHub issue based on research findings. If research hasn't been run, automatically run it first.

### Usage

```
issue
```

### Workflow

#### Step 1: Check for Existing Research

```bash
# Check if research was already done today
LATEST_REPORT=$(ls -td /tmp/tech-debt-* 2>/dev/null | head -1)

if [ -z "$LATEST_REPORT" ]; then
  echo "No research found. Running research first..."
  # Run research operation
else
  echo "Using existing research from: $LATEST_REPORT"
fi
```

#### Step 2: Prepare Issue Content

Read research reports and prepare GitHub issue content:

**Issue Title:**
```
[Tech Debt] Codebase Quality Scan - {date}
```

**Issue Body Structure:**

```markdown
# Technical Debt Analysis - {date}

This issue tracks technical debt identified through automated codebase scanning.

## Executive Summary

{paste-from-summary.md}

## Statistics

{paste-from-statistics.md}

## Critical Issues (P0) - Must Fix

{paste-critical-issues-summary}

<details>
<summary>Detailed Critical Issues</summary>

{paste-from-critical/*.md}

</details>

## High Priority Issues (P1)

{paste-high-issues-summary}

<details>
<summary>Detailed High Priority Issues</summary>

{paste-from-high/*.md}

</details>

## Medium Priority Issues (P2)

{paste-medium-issues-summary}

<details>
<summary>Detailed Medium Priority Issues</summary>

{paste-from-medium/*.md}

</details>

## Action Plan

### Phase 1: Critical Issues (Target: 1 week)
- [ ] Replace unwrap/expect with proper error handling ({count} files)
- [ ] Add safety docs to unsafe blocks ({count} files)
- [ ] Remove dead code suppressions ({count} files)

### Phase 2: High Priority (Target: 2 weeks)
- [ ] Split large files ({count} files)
- [ ] Add error context to bare ? ({count} files)
- [ ] Replace hardcoded values ({count} files)

### Phase 3: Medium Priority (Target: 1 month)
- [ ] Reduce clone() usage ({count} files)
- [ ] Address clippy suppressions ({count} files)
- [ ] Resolve TODO/FIXME comments ({count} files)

## Labels

`tech-debt` `quality` `refactoring`

---

**Scan Details:**
- Date: {date}
- Scope: crates/ directory
- Total files scanned: {count}
- Total violations: {count}

**References:**
- Code quality skill: `.claude/skills/code-quality/SKILL.md`
- Project principles: `.claude/skills/project-principles/SKILL.md`
```

#### Step 3: Create GitHub Issue

**Single Issue Strategy:**

If total content is under GitHub issue size limit (~65K characters):

```bash
gh issue create \
  --repo fagemx/edda \
  --title "[Tech Debt] Codebase Quality Scan - $(date +%Y-%m-%d)" \
  --body-file /tmp/tech-debt-{date}/github-issue-body.md \
  --label "tech-debt,quality,refactoring"
```

**Multiple Comments Strategy:**

If content exceeds size limit, create issue with summary and post detailed sections as comments:

```bash
# 1. Create issue with executive summary
ISSUE_URL=$(gh issue create \
  --repo fagemx/edda \
  --title "[Tech Debt] Codebase Quality Scan - $(date +%Y-%m-%d)" \
  --body-file /tmp/tech-debt-{date}/github-issue-summary.md \
  --label "tech-debt,quality,refactoring")

ISSUE_NUMBER=$(echo $ISSUE_URL | grep -oP '\d+$')

# 2. Post critical issues as comment
gh issue comment $ISSUE_NUMBER \
  --repo fagemx/edda \
  --body-file /tmp/tech-debt-{date}/github-comment-critical.md

# 3. Post high priority issues as comment
gh issue comment $ISSUE_NUMBER \
  --repo fagemx/edda \
  --body-file /tmp/tech-debt-{date}/github-comment-high.md

# 4. Post medium priority issues as comment
gh issue comment $ISSUE_NUMBER \
  --repo fagemx/edda \
  --body-file /tmp/tech-debt-{date}/github-comment-medium.md

# 5. Post action plan as comment
gh issue comment $ISSUE_NUMBER \
  --repo fagemx/edda \
  --body-file /tmp/tech-debt-{date}/github-comment-action-plan.md
```

**Comment Size Limits:**
- Each comment should be <65K characters
- If a section exceeds limit, split into multiple comments
- Use clear headers to indicate which part of the report each comment contains

#### Step 4: Report to User

```markdown
# GitHub Issue Created

**Issue URL:** {url}
**Issue Number:** #{number}

## Content Posted

✅ Issue created with executive summary
✅ Posted {n} comments with detailed findings

## Issue Structure

- Main issue: Executive summary and statistics
- Comment 1: Critical Issues (P0)
- Comment 2: High Priority Issues (P1)
- Comment 3: Medium Priority Issues (P2)
- Comment 4: Action Plan

## Next Steps

1. Review the GitHub issue
2. Prioritize which issues to tackle first
3. Create separate issues for specific refactoring tasks if needed
4. Track progress using the checklist in the action plan

## Local Reports

Detailed reports are also available in:
`/tmp/tech-debt-{date}/`
```

### Implementation Notes

**GitHub CLI Usage:**
- Use `gh issue create` to create issues
- Use `gh issue comment` to add comments
- Verify gh is authenticated: `gh auth status`
- Use `--body-file` for large content

**Content Preparation:**
- Inline all report content (don't use file links)
- Use markdown collapsible sections (`<details>`) for long content
- Include syntax highlighting for code snippets
- Add line breaks for readability

**Error Handling:**
- Check if gh CLI is installed and authenticated
- Verify issue creation succeeded before posting comments
- If comment posting fails, save remaining content to file and report to user
- Don't fail entire operation if one comment fails

---

## General Guidelines

### Scanning Principles

1. **Comprehensive Coverage**
   - Scan all source files in crates/ directory
   - Include both production and test code
   - Check all .rs files

2. **Efficient Execution**
   - Use fast pattern matching first (grep, find)
   - Only read files that match patterns
   - Run searches in parallel when possible
   - Cache results between operations

3. **Accurate Analysis**
   - Read full file content for matched files
   - Verify patterns in context (e.g., unwrap in test vs production)
   - Avoid false positives
   - Include line numbers for all findings

4. **Actionable Reporting**
   - Provide specific file paths and line numbers
   - Include code snippets for context
   - Suggest concrete remediation steps
   - Estimate effort for fixes

### Quality Standards

**Zero Tolerance Issues (P0):**
- `unwrap()` / `expect()` in runtime paths
- `panic!()` in library code
- `unsafe` without safety documentation

**High Priority Issues (P1):**
- Files >800 lines
- Missing error context
- `todo!()` / `unimplemented!()` in shipped code
- Hardcoded configuration values

**Medium Priority Issues (P2):**
- Files >500 lines
- Excessive `.clone()` calls
- Clippy suppressions
- TODO/FIXME comments

**Low Priority Issues (P3):**
- Deep nesting
- Missing public API documentation
- Inconsistent naming patterns

### Communication Style

**To User:**
- Medium level of detail (not too brief, not overwhelming)
- Focus on most important findings
- Provide clear next steps
- Use markdown formatting

**In Reports:**
- High level of detail
- Include all findings with evidence
- Provide remediation guidance
- Use consistent formatting

**In GitHub Issues:**
- Balance detail with readability
- Use collapsible sections (`<details>`) for long content
- Include actionable checklists
- Add appropriate labels

---

## Error Handling

When encountering errors:
- If grep/find fails, report and continue with other checks
- If file is unreadable, note it and continue
- If directory doesn't exist, report and skip
- If gh CLI fails, report and save content to file
- Always complete scan even if some steps fail
- Provide partial results if full scan can't complete

---

## Example Usage

```
# Run research scan
args: "research"

# Create GitHub issue from research
args: "issue"
```

---

## References

- Code quality skill: `.claude/skills/code-quality/SKILL.md`
- Project principles: `.claude/skills/project-principles/SKILL.md`
- Testing patterns: `.claude/skills/testing/SKILL.md`
