---
name: regression-guard
description: Systematic scan → classify → batch fix → add automation to prevent regression
context: fork
---

# Regression Guard — Scan, Fix, Prevent

When you fix a bug or quality issue, don't just fix it — add automated prevention so it can never happen again. This skill follows a systematic approach: scan the entire codebase for a class of problems, classify findings, batch fix, then add tooling to prevent regression.

**Inspired by**: vm0 e7h4n's pattern — "修了就防回歸"

Your args are: `$ARGUMENTS`

Parse the args:
- `scan <category>` — Scan codebase for a specific problem category
- `guard <description>` — After fixing something, add regression prevention
- `audit` — Full audit across all categories

Categories: `type-safety`, `error-handling`, `security`, `testing`, `dead-code`, `performance`, `dependencies`

---

## Operation 1: Scan

Systematically scan the codebase for a class of problems, classify by severity, produce an actionable report.

### Step 1: Choose Scanning Tools

| Category | Tools |
|----------|-------|
| type-safety | `bun run lint`, `grep "as any\|@ts-ignore"` |
| error-handling | `grep "try.*catch"`, `grep "console.error"` |
| security | `grep "eval\|exec\|innerHTML"`, `grep "password\|secret\|token"` in non-.env |
| testing | `grep "\.skip\|\.only"`, `grep "setTimeout" in *.test.ts` |
| dead-code | `bun run knip`, `grep "// TODO\|// FIXME\|// HACK"` |
| performance | `grep "JSON.parse.*JSON.stringify"` (deep clone), `grep "\.forEach"` in hot paths |
| dependencies | `npm audit`, `bun run knip` |

### Step 2: Run Scans

Execute ALL relevant scans and collect raw output:

```bash
# Example for type-safety
bun run lint 2>&1 | grep "error\|warning" > /tmp/scan-results.txt
grep -rn "as any\|@ts-ignore\|@ts-nocheck" src/ --include="*.ts" >> /tmp/scan-results.txt
wc -l /tmp/scan-results.txt
```

### Step 3: Classify Findings

Group findings into:

```markdown
## Scan Results: <category>

### P0 — Fix Now (breaks correctness or security)
| File:Line | Issue | Pattern |
|-----------|-------|---------|
| src/X.ts:42 | ... | ... |

### P1 — Fix Soon (code smell, maintainability risk)
| File:Line | Issue | Pattern |
|-----------|-------|---------|

### P2 — Nice to Have (style, minor improvement)
| File:Line | Issue | Pattern |
|-----------|-------|---------|

### Recurring Patterns
<identify the 2-3 patterns that appear most often — these are the targets for automation>
```

### Step 4: Create Issues

For each P0/P1 group, create a batch-fix issue:

```bash
gh issue create --title "fix(<scope>): <pattern description>" --body "..."
```

Group by **pattern, not by file**. One issue per pattern, not per finding.

---

## Operation 2: Guard

After fixing a class of problems, add automated prevention.

### Step 1: Identify the Guard Type

| Problem Type | Guard Mechanism |
|-------------|----------------|
| `as any` / unsafe types | ESLint rule: `@typescript-eslint/no-explicit-any` (already have) |
| Specific anti-pattern | **Custom ESLint rule** (new plugin) |
| Missing validation | **Test case** that fails without validation |
| Architecture violation | **Import restriction** (ESLint `no-restricted-imports`) |
| Security issue | **Grep-based CI check** |
| Dead code | **Knip** in pre-commit |

### Step 2: Implement the Guard

#### Option A: Custom ESLint Rule (for code patterns)

Create or extend `eslint.config.mjs`:

```javascript
// Example: prevent db.prepare().all() without type assertion
{
  rules: {
    'no-restricted-syntax': ['error', {
      selector: 'CallExpression[callee.property.name="all"]:not([parent.type="TSAsExpression"])',
      message: 'db.prepare().all() must have a type assertion: .all() as SomeType[]'
    }]
  }
}
```

#### Option B: Import Restriction (for architecture violations)

```javascript
// Example: prevent sidecar/ from importing engine-specific code
{
  rules: {
    'no-restricted-imports': ['error', {
      patterns: [{
        group: ['cc', 'cc/*', '@cocos/*'],
        message: 'Engine imports not allowed outside adapters/'
      }]
    }]
  }
}
```

#### Option C: Test Guard (for logic invariants)

```typescript
// Example: ensure Safety Invariants can't be overridden
describe('SI override resistance', () => {
  it('permissive constitution cannot override SI-1', () => {
    // Create maximally permissive constitution
    // Verify SI-1 still blocks
  });
});
```

#### Option D: CI Script (for complex checks)

```bash
#!/bin/bash
# scripts/check-no-floating-promises.sh
VIOLATIONS=$(grep -rn "^\s*\w\+Async()" src/ --include="*.ts" | grep -v "await\|void\|\.then\|\.catch")
if [ -n "$VIOLATIONS" ]; then
  echo "Floating promises found:"
  echo "$VIOLATIONS"
  exit 1
fi
```

### Step 3: Verify the Guard Works

1. **Positive test**: Introduce the bad pattern → guard catches it
2. **Negative test**: Clean code → guard passes
3. **Integration**: Add to pre-commit (lefthook) or CI

```bash
# Test that the guard catches the bad pattern
echo "const x: any = 1;" > /tmp/test-guard.ts
bun run lint /tmp/test-guard.ts 2>&1 | grep "error"
# Should show the lint error
rm /tmp/test-guard.ts
```

### Step 4: Document the Guard

Add to CLAUDE.md or project docs:

```markdown
### Guard: <pattern name>
- **What**: <what it prevents>
- **Why**: <the incident that led to this guard>
- **How**: <ESLint rule / test / CI script>
- **Verification**: `<command to verify guard works>`
```

---

## Operation 3: Audit

Run a full audit across all categories.

### Workflow

1. Run scan for each category: type-safety, error-handling, security, testing, dead-code
2. Aggregate findings into a single report
3. Identify top 3 recurring patterns
4. For each pattern, recommend a guard mechanism

### Output

```markdown
## Full Audit Report

**Date**: YYYY-MM-DD
**Total findings**: N

### By Category
| Category | P0 | P1 | P2 | Top Pattern |
|----------|----|----|----|---------  ---|
| type-safety | 2 | 15 | 30 | unsafe DB query returns |
| error-handling | 0 | 5 | 10 | defensive try/catch |
| security | 1 | 2 | 0 | hardcoded URL |
| testing | 0 | 3 | 8 | setTimeout in tests |
| dead-code | 0 | 2 | 5 | unused exports |

### Top 3 Patterns to Guard

1. **<pattern>** — N occurrences, guard: <mechanism>
2. **<pattern>** — N occurrences, guard: <mechanism>
3. **<pattern>** — N occurrences, guard: <mechanism>

### Recommended Actions
1. [ ] Fix P0 issues immediately
2. [ ] Create batch-fix issues for P1 patterns
3. [ ] Add guards for top 3 patterns
```

---

## Key Principles

1. **Fix by pattern, not by instance** — if you find 10 occurrences of the same problem, fix all 10 in one PR and add a guard
2. **Every fix needs a guard** — the fix is only half done without regression prevention
3. **Automation > documentation** — a lint rule is better than a CLAUDE.md note; a CI check is better than a code comment
4. **Incremental hardening** — don't try to fix everything at once. Pick the top pattern, fix it, guard it, move to the next
5. **The guard IS the documentation** — the lint rule message or test name explains why the pattern is forbidden
