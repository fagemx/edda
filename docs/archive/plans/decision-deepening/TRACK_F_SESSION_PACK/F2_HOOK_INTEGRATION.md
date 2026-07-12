# F2: Hook Integration — Session Start Decision Pack

**Track**: F (Session Start Decision Pack)
**Layer**: L2 — Product-facing
**Depends on**: F1 (`DecisionPack` + `build_decision_pack()` + `render_decision_pack_md()`)

---

## Goal

Wire `build_decision_pack()` and `render_decision_pack_md()` into the existing
`dispatch_session_start` function so that active decisions are injected into
every session's context automatically.

## Target File

**Modify**: `crates/edda-bridge-claude/src/dispatch/session.rs`

## Integration Point

`dispatch_session_start()` (line 611) builds a `content` variable by appending
sections in order:

```
1. Hot pack (read_hot_pack)              ← existing
2. Skill guide directive                 ← existing (optional)
3. Active plan                           ← existing
4. L1 narrative                          ← existing
5. Karvi brief                           ← existing
6. Project state                         ← existing
7. ★ Decision pack ★                     ← INSERT HERE
8. Prior session last message            ← existing (truncatable)
9. Digest warning                        ← existing (truncatable)
--- body_budget cutoff ---
10. Tail (write-back, phase, coord)      ← existing (reserved)
```

The decision pack is inserted **after project state** (line 677) and **before
the truncatable tail sections** (line 714). This placement ensures:
- It appears after hot.md context (which the agent relies on for continuity)
- It appears before truncatable sections (so it survives budget cuts)
- It is part of the `body_budget` allocation (not the reserved tail)

## Implementation

### Step 1: Add import

At the top of `session.rs`, add `edda_pack` functions to imports:

```rust
// Already imported via edda_pack::render_pack, build_turns, etc.
// Add: build_decision_pack, render_decision_pack_md
```

### Step 2: Inject decision pack

Insert after the project state block (after line 677) and before the
"Previous session context" comment (line 679):

```rust
    // Inject active decisions as context (Track F — Decision Deepening)
    {
        let decision_pack_md = {
            let cwd_path = std::path::Path::new(cwd);
            match edda_ledger::EddaPaths::find_root(cwd_path) {
                Some(root) => {
                    let ledger_path = root.join(".edda").join("ledger.db");
                    let branch = edda_ledger::Ledger::open(&root)
                        .and_then(|l| l.head_branch())
                        .unwrap_or_default();
                    let max_items: usize = std::env::var("EDDA_DECISION_PACK_MAX")
                        .ok()
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(7);
                    let pack = edda_pack::build_decision_pack(&ledger_path, &branch, max_items);
                    let md = edda_pack::render_decision_pack_md(&pack);
                    if md.is_empty() { None } else { Some(md) }
                }
                None => None,
            }
        };

        if let Some(dp) = decision_pack_md {
            content = Some(match content {
                Some(c) => format!("{c}\n\n{dp}"),
                None => dp,
            });
        }
    }
```

### Step 3: Context budget accounting

The decision pack is part of the `content` (body) variable, which is subject
to `apply_context_budget(&ctx, body_budget)` at line 740. No separate budget
deduction is needed — the existing budget mechanism handles it:

```rust
// Line 736-740 (existing — no change needed):
let total_budget = context_budget(cwd);
let body_budget = total_budget.saturating_sub(tail.len());

if let Some(ctx) = content {
    let budgeted_body = apply_context_budget(&ctx, body_budget);
    // ...
}
```

The decision pack markdown is typically 200-500 chars (7 decisions), well
within the default body budget (~12,000 chars). If the body exceeds budget,
`apply_context_budget` truncates from the end — the decision pack's position
(before truncatable sections) means it survives moderate budget pressure.

### Empty pack handling

`render_decision_pack_md()` returns an empty string when the pack has 0
decisions. The `if md.is_empty() { None }` check ensures nothing is appended
to `content` when there are no decisions. Zero overhead for projects without
decisions.

## Design Decisions

### Placement rationale

The decision pack is placed **after** project state and **before** prior
session context because:
1. Decisions are project-level context (like project state), not session-level
2. They should be visible even when body is truncated (prior session message
   is more expendable than active decisions)
3. They should NOT be in the reserved tail (tail is for agent-functional
   sections like coordination protocol)

### Environment variable: `EDDA_DECISION_PACK_MAX`

Allows tuning the number of decisions injected (default 7). This follows the
existing pattern of `EDDA_PACK_TURNS`, `EDDA_PACK_BUDGET_CHARS`, etc.

### Conductor mode

The decision pack is injected regardless of `conductor_mode`. Active decisions
are relevant to all agents, including conductor-spawned sub-agents. If this
needs to change, add a `if !conductor_mode { ... }` guard later.

## Existing SessionStart Logic — What NOT to Touch

| Logic | Lines | Purpose |
|-------|-------|---------|
| Hot pack read | 621-634 | Load pre-built turn summary |
| Active plan | 638-645 | Cross-session plan continuity |
| L1 narrative | 647-661 | Focus + blocking + tasks |
| Karvi brief | 663-669 | Karvi task context |
| Project state | 671-677 | Board summary, project config |
| Tail sections | 684-711 | Write-back, phase, coordination |
| Budget application | 735-753 | Truncation + tail append |

## Tests

Location: `crates/edda-bridge-claude/src/dispatch/tests.rs`

### Test cases

1. **`test_session_start_includes_decision_pack`**
   - Set up a ledger with 3 active decisions
   - Call `dispatch_session_start()`
   - Assert: output `additionalContext` contains `"## Active Decisions"`

2. **`test_session_start_empty_pack_no_injection`**
   - Empty ledger (no decisions)
   - Call `dispatch_session_start()`
   - Assert: output does NOT contain `"## Active Decisions"`

3. **`test_session_start_decision_pack_respects_budget`**
   - Set `EDDA_CONTEXT_BUDGET_CHARS` to a very small value
   - Set up decisions + other content that exceeds budget
   - Assert: output is truncated but does not error

4. **`test_session_start_decision_pack_max_items`**
   - Set `EDDA_DECISION_PACK_MAX=3`
   - 10 decisions in ledger
   - Assert: at most 3 decisions appear in output

## DoD

- [ ] `dispatch_session_start` injects decision pack markdown after project state
- [ ] Empty pack produces no injection (no `## Active Decisions` section)
- [ ] Decision pack participates in body budget (truncated if needed)
- [ ] `EDDA_DECISION_PACK_MAX` env var controls max items (default 7)
- [ ] No import of `DecisionRow` in `session.rs` (BOUNDARY-01)
- [ ] Existing SessionStart sections unchanged
- [ ] `cargo test -p edda-bridge-claude` all pass (TEST-01)
- [ ] `cargo clippy -p edda-bridge-claude --all-targets` zero warnings (CLIPPY-01)

## Verification

```bash
cargo build -p edda-bridge-claude
cargo test -p edda-bridge-claude
cargo clippy -p edda-bridge-claude --all-targets

# Manual smoke test:
edda decide "db.engine=sqlite" --reason "embedded" --paths "crates/edda-ledger/**"
edda decide "error.pattern=thiserror" --reason "consistent"
# Start a new Claude Code session
# Expect: session context includes "## Active Decisions (2 on main)" section

# Grep to verify boundary:
grep -rn "DecisionRow" crates/edda-bridge-claude/  # Expected: 0 results
```
