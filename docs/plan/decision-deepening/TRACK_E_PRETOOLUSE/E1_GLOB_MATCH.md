# E1: Glob Match Engine — `decision_file_warning()`

**Track**: E (PreToolUse File Warning)
**Layer**: L2 — Product-facing
**Depends on**: Track C (DecisionView + `to_view()` + `query_by_paths()`)

---

## Goal

Create a function that checks whether a file being edited is governed by any
active decision. If so, return a formatted markdown warning string. This is the
pure-logic core — hook integration is E2.

## Target File

**New file**: `crates/edda-bridge-claude/src/decision_warning.rs`

Add `mod decision_warning;` to `crates/edda-bridge-claude/src/lib.rs` (or
the appropriate module root).

## Function Signature

```rust
use edda_ledger::view::DecisionView;
use std::path::Path;

/// Check if `file_path` is governed by any active decision.
///
/// Returns a formatted markdown warning listing matching decisions,
/// or `None` if no decisions match.
///
/// Performance: must complete in < 100ms (CONTRACT PERF-01).
/// Uses `DECISION_CACHE` to avoid re-querying within the same session.
pub(crate) fn decision_file_warning(
    ledger_path: &Path,
    file_path: &str,
    branch: &str,
) -> Option<String> {
    // 1. Load active decisions (cached per session)
    // 2. Filter to decisions with non-empty affected_paths
    // 3. Glob match file_path against each decision's affected_paths
    // 4. Format and return warning markdown
    todo!()
}
```

## Implementation Details

### 1. Session-scoped decision cache

To meet PERF-01 (< 100ms), active decisions must be cached so that repeated
`PreToolUse` calls within the same session don't re-query SQLite:

```rust
use std::sync::Mutex;
use std::sync::LazyLock;

struct DecisionCache {
    /// Cache key: (ledger_path, branch) to detect invalidation
    key: Option<(String, String)>,
    /// Cached decisions with non-empty affected_paths
    decisions: Vec<DecisionView>,
    /// Timestamp of last load, for TTL-based expiration
    loaded_at: std::time::Instant,
}

static DECISION_CACHE: LazyLock<Mutex<DecisionCache>> = LazyLock::new(|| {
    Mutex::new(DecisionCache {
        key: None,
        decisions: Vec::new(),
        loaded_at: std::time::Instant::now(),
    })
});

const CACHE_TTL_SECS: u64 = 120; // 2 minutes — balances freshness vs perf
```

Cache invalidation: re-query when `(ledger_path, branch)` changes OR when
`loaded_at` exceeds TTL.

### 2. Query active decisions

Use the `DecisionView` read path from Track C (BOUNDARY-01, BOUNDARY-02):

```rust
fn load_decisions_cached(ledger_path: &Path, branch: &str) -> Vec<DecisionView> {
    let mut cache = DECISION_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let key = (ledger_path.display().to_string(), branch.to_string());

    if let Some(ref cached_key) = cache.key {
        if *cached_key == key && cache.loaded_at.elapsed().as_secs() < CACHE_TTL_SECS {
            return cache.decisions.clone();
        }
    }

    // Query via edda-ledger view API (Track C provides this)
    let decisions = match edda_ledger::Ledger::open_path(ledger_path) {
        Ok(ledger) => ledger
            .active_decisions_with_paths(branch)
            .unwrap_or_default()
            .into_iter()
            .map(|row| edda_ledger::view::to_view(row))
            .filter(|v| !v.affected_paths.is_empty())
            .collect(),
        Err(_) => Vec::new(),
    };

    cache.key = Some(key);
    cache.decisions = decisions.clone();
    cache.loaded_at = std::time::Instant::now();
    decisions
}
```

**Note**: The exact query method name (`active_decisions_with_paths`) depends on
Track C's implementation. The key contract is:
- Query active decisions (status IN `active`, `experimental`)
- Filter to those with non-empty `affected_paths`
- Return as `Vec<DecisionView>` (parsed arrays, not JSON strings)

### 3. Glob matching

Use `globset::Glob` (already a dependency — see `tools.rs` line 4) to match
`file_path` against each decision's `affected_paths` patterns:

```rust
fn matches_any_path(file_path: &str, affected_paths: &[String]) -> bool {
    // Normalize path separators (Windows compat)
    let normalized = file_path.replace('\\', "/");

    for pattern in affected_paths {
        if let Ok(glob) = globset::Glob::new(pattern) {
            if let Ok(matcher) = glob.compile_matcher() {
                if matcher.is_match(&normalized) {
                    return true;
                }
            }
        }
    }
    false
}
```

### 4. Format warning markdown

```rust
fn format_warning(matches: &[&DecisionView]) -> String {
    let mut lines = vec!["**[edda] Active decisions governing this file:**".to_string()];
    for d in matches {
        let reason_suffix = if d.reason.is_empty() {
            String::new()
        } else {
            format!(" — {}", d.reason)
        };
        lines.push(format!("  - `{}={}` [{}]{}", d.key, d.value, d.status, reason_suffix));
    }
    lines.join("\n")
}
```

**Example output:**
```markdown
**[edda] Active decisions governing this file:**
  - `db.engine=sqlite` [active] — embedded, zero-config for MVP
  - `error.pattern=thiserror` [active] — consistent error handling
```

## Boundary Rules

| Rule | Requirement |
|------|-------------|
| BOUNDARY-01 | This file must NOT import `DecisionRow` — only `DecisionView` |
| BOUNDARY-02 | Must read through `to_view()`, never parse `affected_paths` JSON directly |
| PERF-01 | Total time < 100ms — use `DECISION_CACHE` |

## Tests

Location: `crates/edda-bridge-claude/src/decision_warning.rs` (inline `#[cfg(test)]`)

### Test cases

1. **`test_no_decisions_returns_none`** — Empty ledger → `decision_file_warning()` returns `None`
2. **`test_no_matching_paths_returns_none`** — Active decisions exist but `affected_paths` don't match → `None`
3. **`test_matching_glob_returns_warning`** — Decision with `affected_paths: ["crates/edda-ledger/**"]`, file `crates/edda-ledger/src/lib.rs` → `Some(warning)`
4. **`test_multiple_matches`** — Two decisions match → warning lists both
5. **`test_matches_any_path_normalization`** — Backslash paths on Windows match forward-slash globs
6. **`test_cache_reuse`** — Second call with same ledger/branch returns cached result (assert no re-query)

## DoD

- [ ] `decision_file_warning()` returns formatted warning when file matches active decision paths
- [ ] Returns `None` when no decisions match (zero-overhead fast path)
- [ ] Session-scoped cache avoids repeated SQLite queries (PERF-01)
- [ ] No import of `DecisionRow` (BOUNDARY-01)
- [ ] No direct JSON parsing of `affected_paths` (BOUNDARY-02)
- [ ] `cargo clippy -p edda-bridge-claude` zero warnings (CLIPPY-01)
- [ ] All tests pass: `cargo test -p edda-bridge-claude -- decision_warning`

## Verification

```bash
cargo build -p edda-bridge-claude
cargo test -p edda-bridge-claude -- decision_warning
cargo clippy -p edda-bridge-claude --all-targets
# Grep to verify boundary:
grep -rn "DecisionRow" crates/edda-bridge-claude/  # Expected: 0 results
```
