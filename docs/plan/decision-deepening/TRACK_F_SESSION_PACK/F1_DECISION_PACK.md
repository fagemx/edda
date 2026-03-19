# F1: DecisionPack Type + `build_decision_pack()`

**Track**: F (Session Start Decision Pack)
**Layer**: L2 — Product-facing
**Depends on**: Track C (DecisionView + `to_view()`)

---

## Goal

Create a `DecisionPack` type and builder function in `edda-pack` that queries
active decisions, groups them by domain, and renders them as a markdown section
for session context injection.

## Target File

**Modify**: `crates/edda-pack/src/lib.rs`

Add the `DecisionPack` type and two public functions at the end of the file.

## Type Definition

```rust
use edda_ledger::view::DecisionView;

/// A pack of active decisions grouped by domain, ready for session injection.
#[derive(Debug, Clone)]
pub struct DecisionPack {
    /// Decisions grouped by domain (e.g., "db", "error", "auth")
    pub groups: Vec<DecisionGroup>,
    /// Total number of decisions included
    pub total: usize,
    /// Branch these decisions are scoped to
    pub branch: String,
}

/// A group of decisions sharing the same domain prefix.
#[derive(Debug, Clone)]
pub struct DecisionGroup {
    /// Domain name (e.g., "db", "error", "auth")
    pub domain: String,
    /// Decisions in this domain, sorted by key
    pub decisions: Vec<DecisionSummary>,
}

/// Minimal decision summary for pack rendering (avoids carrying full DecisionView).
#[derive(Debug, Clone)]
pub struct DecisionSummary {
    pub key: String,
    pub value: String,
    pub reason: String,
    pub status: String,
    pub authority: String,
    pub reversibility: String,
    pub affected_paths: Vec<String>,
}
```

## Function Signatures

### `build_decision_pack()`

```rust
use std::path::Path;

/// Build a decision pack from active decisions in the ledger.
///
/// Queries active decisions (status IN active, experimental) on the given
/// branch, groups by domain, and limits to `max_items` total decisions.
///
/// Returns a pack with 0 groups if no active decisions exist.
pub fn build_decision_pack(
    ledger_path: &Path,
    branch: &str,
    max_items: usize,  // default: 7
) -> DecisionPack {
    todo!()
}
```

### `render_decision_pack_md()`

```rust
/// Render a decision pack as a markdown section.
///
/// Returns an empty string if the pack has 0 decisions.
pub fn render_decision_pack_md(pack: &DecisionPack) -> String {
    todo!()
}
```

## Implementation Details

### `build_decision_pack()`

```rust
pub fn build_decision_pack(
    ledger_path: &Path,
    branch: &str,
    max_items: usize,
) -> DecisionPack {
    let decisions = match edda_ledger::Ledger::open_path(ledger_path) {
        Ok(ledger) => ledger
            .active_decisions(branch)
            .unwrap_or_default()
            .into_iter()
            .map(|row| edda_ledger::view::to_view(row))
            .collect::<Vec<_>>(),
        Err(_) => Vec::new(),
    };

    if decisions.is_empty() {
        return DecisionPack {
            groups: Vec::new(),
            total: 0,
            branch: branch.to_string(),
        };
    }

    // Group by domain, limit to max_items total
    let mut by_domain: std::collections::BTreeMap<String, Vec<DecisionSummary>> =
        std::collections::BTreeMap::new();
    let mut count = 0;

    for d in &decisions {
        if count >= max_items {
            break;
        }
        by_domain
            .entry(d.domain.clone())
            .or_default()
            .push(DecisionSummary {
                key: d.key.clone(),
                value: d.value.clone(),
                reason: d.reason.clone(),
                status: d.status.clone(),
                authority: d.authority.clone(),
                reversibility: d.reversibility.clone(),
                affected_paths: d.affected_paths.clone(),
            });
        count += 1;
    }

    let groups = by_domain
        .into_iter()
        .map(|(domain, mut decisions)| {
            decisions.sort_by(|a, b| a.key.cmp(&b.key));
            DecisionGroup { domain, decisions }
        })
        .collect();

    DecisionPack {
        groups,
        total: count,
        branch: branch.to_string(),
    }
}
```

**Note**: The exact query method name (`active_decisions`) depends on Track C's
implementation. The key contract: query decisions where `status IN ('active',
'experimental')` on the given branch, returned as `DecisionView` via `to_view()`.

### `render_decision_pack_md()`

```rust
pub fn render_decision_pack_md(pack: &DecisionPack) -> String {
    if pack.total == 0 {
        return String::new();
    }

    let mut lines = vec![format!(
        "## Active Decisions ({} on `{}`)",
        pack.total, pack.branch
    )];

    for group in &pack.groups {
        lines.push(format!("\n### {}", group.domain));
        for d in &group.decisions {
            let mut entry = format!("- **`{}={}`**", d.key, d.value);
            if !d.reason.is_empty() {
                entry.push_str(&format!(" — {}", d.reason));
            }
            if !d.affected_paths.is_empty() {
                entry.push_str(&format!(
                    "\n  paths: `{}`",
                    d.affected_paths.join("`, `")
                ));
            }
            lines.push(entry);
        }
    }

    lines.join("\n")
}
```

**Example output:**
```markdown
## Active Decisions (3 on `main`)

### db
- **`db.engine=sqlite`** — embedded, zero-config for MVP
  paths: `crates/edda-ledger/**`

### error
- **`error.pattern=thiserror`** — consistent error handling

### logging
- **`logging.level=debug`** — verbose during development
```

## Boundary Rules

| Rule | Requirement |
|------|-------------|
| BOUNDARY-01 | `edda-pack` must NOT import `DecisionRow` — only `DecisionView` |
| BOUNDARY-02 | Must read through `to_view()`, never parse JSON directly |

## Cargo.toml

Add `edda-ledger` as a dependency of `edda-pack` if not already present:

```toml
[dependencies]
edda-ledger = { path = "../edda-ledger" }
```

## Tests

Location: `crates/edda-pack/src/lib.rs` (inline `#[cfg(test)]`) or a new
`crates/edda-pack/src/decision_pack_tests.rs`.

### Test cases

1. **`test_empty_pack`**
   - No decisions in ledger → `build_decision_pack()` returns pack with `total: 0`
   - `render_decision_pack_md()` returns empty string

2. **`test_full_pack_grouped_by_domain`**
   - 5 decisions across 3 domains ("db", "error", "auth")
   - Assert: pack has 3 groups, decisions sorted by key within each group
   - Assert: render output has `## Active Decisions (5 on ...)` header

3. **`test_max_items_limit`**
   - 10 decisions in ledger, `max_items = 7`
   - Assert: pack contains exactly 7 decisions

4. **`test_domain_grouping_order`**
   - Decisions with domains "z_test", "a_first", "m_middle"
   - Assert: groups ordered alphabetically (BTreeMap guarantees this)

5. **`test_render_with_paths`**
   - Decision with `affected_paths: ["crates/foo/**", "src/**"]`
   - Assert: render output contains `paths: \`crates/foo/**\`, \`src/**\``

6. **`test_render_without_reason`**
   - Decision with empty reason
   - Assert: no trailing ` — ` in render output

## DoD

- [ ] `DecisionPack`, `DecisionGroup`, `DecisionSummary` types defined
- [ ] `build_decision_pack()` queries active decisions and groups by domain
- [ ] `render_decision_pack_md()` produces clean markdown (empty string when empty)
- [ ] `max_items` respected (default 7)
- [ ] No import of `DecisionRow` (BOUNDARY-01)
- [ ] `cargo build -p edda-pack` zero errors
- [ ] `cargo test -p edda-pack` all pass (TEST-01)
- [ ] `cargo clippy -p edda-pack --all-targets` zero warnings (CLIPPY-01)

## Verification

```bash
cargo build -p edda-pack
cargo test -p edda-pack
cargo clippy -p edda-pack --all-targets
# Grep to verify boundary:
grep -rn "DecisionRow" crates/edda-pack/  # Expected: 0 results
```
