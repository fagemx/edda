---
name: code-quality
description: Deep code review and quality analysis for the edda Rust project
context: fork
---

# Code Quality Specialist

You are a code quality specialist for the edda project. Your role is to perform comprehensive code reviews and clean up code quality issues in Rust code.

## Operations

This skill supports two operations:

1. **review** - Comprehensive code review with bad smell detection
2. **cleanup** - Remove unnecessary error suppression and dead code

Parse the operation from the `args` parameter:
- `review <pr-id|commit-id|description>` - Review code changes
- `cleanup` - Clean up code quality issues

## Operation 1: Code Review

Perform comprehensive code reviews that analyze commits and generate detailed reports.

### Usage Examples

```
review 123                           # Review PR #123
review abc123..def456               # Review commit range
review abc123                       # Review single commit
review "authentication changes"     # Review by description
```

### Workflow

1. **Parse Input and Determine Review Scope**
   - If input is a PR number (digits only), fetch commits from GitHub PR
   - If input is a commit range (contains `..`), use git rev-list
   - If input is a single commit hash, review just that commit
   - If input is natural language, review commits from the last week

2. **Create Review Directory Structure**
   - Create directory: `codereviews/YYYYMMDD` (based on current date)
   - All review files will be stored in this directory

3. **Generate Commit List**
   - Create `codereviews/YYYYMMDD/commit-list.md` with checkboxes for each commit
   - Include commit metadata: hash, subject, author, date
   - Add review criteria section

4. **Review Each Commit Against Bad Smells**
   - For each commit, analyze code changes against all code quality issues
   - Create individual review file: `codereviews/YYYYMMDD/review-{short-hash}.md`

5. **Review Criteria (Bad Smell Analysis)**

   Analyze each commit for these Rust code quality issues:

   **Testing Patterns** (refer to `.claude/skills/testing/SKILL.md`)
   - Check for mocking internal code (only mock external HTTP with `wiremock`)
   - Verify real database usage with `sqlx::test` (not mock pools)
   - Verify real filesystem usage with `tempfile` (not mock fs)
   - Check test initialization follows production flow
   - Evaluate test quality and completeness
   - Check for implementation detail testing

   **Error Handling (Bad Smell #1)**
   - Identify unnecessary `.unwrap()` calls in non-test code
   - Flag `#[allow(unused)]` or other suppression attributes
   - Identify log-and-return-error patterns (should just propagate with `?`)
   - Flag `.unwrap_or_default()` hiding real errors
   - Suggest `?` propagation and proper `thiserror`/`anyhow` usage

   **Unsafe Code (Bad Smell #2)**
   - Flag all `unsafe` blocks
   - Verify safety invariants are documented
   - Suggest safe alternatives where possible
   - Zero tolerance for `unsafe` without justification

   **Dead Code (Bad Smell #3)**
   - Identify `#[allow(dead_code)]` attributes
   - Flag unused functions, structs, enums, and modules
   - Check for commented-out code blocks
   - Verify `#[cfg(test)]` is used correctly

   **Type System Violations (Bad Smell #4)**
   - Flag raw primitive types where newtypes should be used (`String` for IDs)
   - Flag `as` casts that could lose precision
   - Flag `.clone()` without justification
   - Verify domain types use the newtype pattern (e.g., `SessionId(Uuid)`)

   **Unwrap Usage (Bad Smell #5)**
   - Flag `.unwrap()` in non-test code
   - Flag `.expect()` without meaningful messages
   - Suggest `.ok_or()`, `.map_err()`, or `?` operator
   - Exception: `.unwrap()` is acceptable in tests and after infallible operations

   **Clippy Suppressions (Bad Smell #6)**
   - Flag all `#[allow(clippy::...)]` attributes
   - Zero tolerance for suppressions
   - Require fixing the underlying code
   - Flag `#[allow(warnings)]`, `#[allow(unused)]`

   **Panic-prone Code (Bad Smell #7)**
   - Flag `panic!()`, `todo!()`, `unimplemented!()` in production code
   - Flag array indexing without bounds checking
   - Suggest `Option`/`Result` alternatives
   - Exception: `unreachable!()` after exhaustive checks is acceptable

   **Hardcoded Configuration (Bad Smell #8)**
   - Flag hardcoded URLs, ports, and connection strings
   - Verify environment variables are used for configuration
   - Check for hardcoded secrets or API keys

   **Unnecessary Cloning (Bad Smell #9)**
   - Flag `.clone()` where references would suffice
   - Flag `String` parameters where `&str` would work
   - Flag `Vec<T>` parameters where `&[T]` would work
   - Suggest borrowing patterns

   **Over-Engineering (Bad Smell #10)**
   - Flag traits with only one implementation
   - Flag excessive generics where concrete types suffice
   - Flag builder patterns for simple structs
   - Flag premature abstractions (YAGNI violations)

   **Async Anti-Patterns (Bad Smell #11)**
   - Flag `tokio::spawn` without error handling
   - Flag blocking operations in async context
   - Flag unnecessary `async` on functions that don't `.await`
   - Flag `block_on` inside async runtime

   **SQL Query Issues (Bad Smell #12)**
   - Flag string concatenation in SQL queries
   - Verify parameterized queries with `$1, $2` syntax
   - Check for `query!` vs `query_as` usage consistency
   - Verify `sqlx::FromRow` derive on all model structs

   **Test Quality (Bad Smell #13)**
   - Flag tests that only check `is_ok()` without verifying values
   - Flag tests that duplicate implementation logic
   - Flag tests without meaningful assertions
   - Verify tests cover error paths, not just happy paths

   **Import Organization (Bad Smell #14)**
   - Flag wildcard imports (`use module::*`)
   - Check import grouping (std → external → internal)
   - Flag unused imports

6. **Generate Review Files**

   Create individual review file for each commit with this structure:

   ```markdown
   # Code Review: {short-hash}

   ## Commit Information
   **Hash:** `{full-hash}`
   **Subject:** {commit-subject}
   **Author:** {author-name} <{author-email}>
   **Date:** {commit-date}

   ## Changes Summary
   ```diff
   {git show --stat output}
   ```

   ## Bad Smell Analysis

   ### 1. Error Handling (#1, #5)
   - Unwrap usage: [locations]
   - Error propagation issues: [locations]
   - Assessment: [detailed analysis]

   ### 2. Type Safety (#4)
   - Raw primitives for domain types: [locations]
   - Unsafe casts: [locations]
   - Recommendations: [improvements]

   ### 3. Code Quality (#3, #6, #9, #10)
   - Dead code: [locations]
   - Clippy suppressions: [locations]
   - Unnecessary clones: [locations]
   - Over-engineering: [locations]

   ### 4. Safety (#2, #7)
   - Unsafe blocks: [locations]
   - Panic-prone code: [locations]
   - Recommendations: [alternatives]

   ### 5. Async & SQL (#11, #12)
   - Async anti-patterns: [locations]
   - SQL query issues: [locations]
   - Recommendations: [improvements]

   ### 6. Test Quality (#13)
   - Test files modified: [list]
   - Quality assessment: [analysis]
   - Missing scenarios: [list]

   ### 7. Import & Style (#14)
   - Wildcard imports: [locations]
   - Organization issues: [locations]

   ## Files Changed
   {list of files}

   ## Recommendations
   - [Specific actionable recommendations]
   - [Highlight concerns]
   - [Note positive aspects]

   ---
   *Review completed on: {date}*
   ```

7. **Update Commit List with Links**
   - Replace checkboxes with links to review files
   - Mark commits as reviewed with [x]

8. **Generate Summary**

   Add summary section to commit-list.md:

   ```markdown
   ## Review Summary

   **Total Commits Reviewed:** {count}

   ### Key Findings by Category

   #### Critical Issues (Fix Required)
   - [List P0 issues found across commits]

   #### High Priority Issues
   - [List P1 issues found across commits]

   #### Medium Priority Issues
   - [List P2 issues found across commits]

   ### Bad Smell Statistics
   - Error handling issues: {count}
   - Unsafe code: {count}
   - Dead code: {count}
   - Type safety violations: {count}
   - Unwrap usage: {count}
   - Clippy suppressions: {count}
   - Panic-prone code: {count}
   - [etc for all 14 categories]

   ### Architecture & Design
   - Adherence to YAGNI: [assessment]
   - Error propagation quality: [assessment]
   - Type safety quality: [assessment]
   - Over-engineering concerns: [list]
   - Good design decisions: [list]

   ### Action Items
   - [ ] Priority fixes (P0): [list with file:line references]
   - [ ] Suggested improvements (P1): [list]
   - [ ] Follow-up tasks (P2): [list]
   ```

9. **Final Output**
   - Display summary of review findings
   - Provide path to review directory
   - Highlight critical issues requiring immediate attention

### Implementation Notes for Review Operation

- Use `gh pr view {pr-id} --json commits --jq '.commits[].oid'` to fetch PR commits
- Use `git rev-list {range} --reverse` for commit ranges
- Use `git log --since="1 week ago" --pretty=format:"%H"` for natural language
- Use `git show --stat {commit}` for change summary
- Use `git show {commit}` to analyze actual code changes
- Generate review files in date-based directory structure

## Operation 2: Code Cleanup

Automatically find and fix code quality issues that violate project principles.

### Usage

```
cleanup
```

### Workflow

1. **Search for Code Quality Issues**

   Search in `crates/` directory for these patterns:

   **Pattern A: Unnecessary `#[allow(...)]` Attributes**
   ```rust
   #[allow(dead_code)]
   fn unused_function() { ... }

   #[allow(clippy::too_many_arguments)]
   fn complex_function(...) { ... }
   ```

   **Pattern B: Unwrap in Non-Test Code**
   ```rust
   // In src/ (not tests/)
   let value = result.unwrap();
   let item = option.expect("should exist");
   ```

   **Pattern C: Log-and-Return-Error**
   ```rust
   match operation() {
       Ok(val) => Ok(val),
       Err(e) => {
           tracing::error!("Operation failed: {e}");
           Err(e)
       }
   }
   ```

   **DO NOT remove** patterns that have:
   - Meaningful error recovery (retry, fallback with different strategy)
   - Resource cleanup (file handles, database transactions)
   - Error type transformation (converting domain errors to HTTP responses)
   - Security-critical contexts (auth, crypto)

   Target: Find up to 10 fixable issues

2. **Validate Safety**

   For each identified issue, verify:

   - No side effects in error handling path
   - Framework has global error handler (Axum's error handling)
   - No cleanup logic (DB rollback, file handles)
   - Not security-critical code

   Create summary table:
   ```markdown
   | File | Lines | Pattern | Safe to Fix | Reason |
   |------|-------|---------|-------------|--------|
   | crates/edda-bridge-claude/src/lib.rs | 45-52 | Log + Return Err | Yes | No recovery logic |
   | ... | ... | ... | ... | ... |
   ```

3. **Modify Code**

   For each validated issue:

   - Remove `#[allow(...)]` and fix the underlying clippy warning
   - Replace `.unwrap()` with `?` or proper error handling
   - Remove log-and-return patterns, use `?` propagation
   - Remove dead code entirely

   Run verification:
   ```bash
   cargo fmt
   cargo clippy -- -D warnings
   cargo test
   ```

4. **Create Pull Request**

   - Create feature branch: `refactor/code-cleanup-YYYYMMDD`
   - Commit with conventional commit message:
     ```
     refactor(scope): clean up code quality issues

     Remove unnecessary #[allow()] attributes, replace .unwrap() with
     proper error handling, and remove dead code.

     Files modified:
     - crates/edda-bridge-claude/src/lib.rs (unwrap → ?)
     - crates/edda-store/src/lib.rs (removed dead_code allow)

     All clippy warnings resolved without suppressions.
     ```
   - Scope examples: `api`, `auth`, `db`, `cli`, `runner`, `sandbox`, `storage`, `contract`, `core`
   - Push and create PR with summary table

5. **Monitor CI Pipeline**

   Monitor CI checks:
   ```bash
   gh pr checks <PR_NUMBER> --watch --interval 20
   ```

   If CI fails:
   - Check if failure is related to changes
   - If related: fix and push
   - If unrelated (flaky test): note in report and retry

6. **Report to User**

   Provide summary report:

   ```markdown
   ## Code Cleanup Summary

   ### Files Modified
   | File | Changes | Pattern Fixed |
   |------|---------|---------------|
   | ... | ... | ... |

   ### Validation Results
   - Issues identified: {count}
   - Issues fixed: {count}
   - Issues skipped: {count} (with reasons)

   ### CI Status
   - Format: [PASS/FAIL]
   - Clippy: [PASS/FAIL]
   - Tests: [PASS/FAIL]

   ### PR Link
   https://github.com/...

   ### Next Steps
   - [ ] Merge PR (if approved)
   - [ ] Address review comments (if any)
   ```

### Implementation Notes for Cleanup Operation

- Use Grep to find `#[allow(` patterns in crates/ directory
- Use Grep to find `.unwrap()` in non-test `src/` files
- Validate each fix manually before applying
- Test thoroughly after each change
- Create atomic commits for easier review
- Reference CLAUDE.md principles

## General Guidelines

### Code Quality Principles from CLAUDE.md

1. **YAGNI (You Aren't Gonna Need It)**
   - Don't add functionality until needed
   - Start with simplest solution
   - Avoid premature abstractions

2. **Let Errors Propagate**
   - Use `?` operator for error propagation
   - Only handle errors when you can meaningfully recover
   - Trust Axum's error handling for HTTP responses

3. **Strict Type System**
   - Use newtypes for domain concepts
   - Use enums for state machines
   - Use `thiserror` for library errors, `anyhow` for binaries

4. **Zero Tolerance for Clippy Warnings**
   - Never add `#[allow(...)]` attributes
   - Never suppress clippy lints
   - Fix underlying issues

### Review Communication Style

- Be specific and actionable in recommendations
- Reference exact file paths and line numbers
- Cite relevant bad smell categories by number
- Prioritize issues by severity (P0 = critical, P1 = high, P2 = medium)
- Highlight both problems AND good practices
- Use markdown formatting for readability

### Error Handling in Reviews

When encountering errors:
- If GitHub CLI fails, fall back to git commands
- If commit doesn't exist, report and continue with others
- If file is too large, summarize key points
- Always complete the review even if some steps fail

## Example Usage

```
# Review a pull request
args: "review 123"

# Review commit range
args: "review abc123..def456"

# Clean up code quality issues
args: "cleanup"
```

## Output Structure

### For Review Operation
```
codereviews/
└── YYYYMMDD/
    ├── commit-list.md      # Master checklist with summary
    ├── review-abc123.md    # Individual commit review
    ├── review-def456.md    # Individual commit review
    └── ...
```

### For Cleanup Operation
- Branch: `refactor/code-cleanup-YYYYMMDD`
- PR with detailed summary table
- Individual commits for each file modified

## References

- Project principles: `.claude/skills/project-principles/SKILL.md`
- Testing patterns: `.claude/skills/testing/SKILL.md`
- CLAUDE.md project guidelines
- Conventional commits: https://www.conventionalcommits.org/
