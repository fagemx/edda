# Innovation Phase: Per-File Edit Tracking (Issue #163) — Updated

## Approach Evaluation

### Option A: Fix and merge PR #181 as-is
**Strategy**: Address CI failures and agent counting bug in PR #181.

**Pros**:
- Most of the work is done (206 lines added)
- Structural changes are sound
- Test already covers core parsing logic

**Cons**:
- CI failures are from pre-existing `edda-serve` issues (not PR's fault, but needs fixing)
- Agent counting lacks session deduplication
- No revert detection (reverts field always 0)
- Format issues need cleanup

**Verdict**: Best path forward with targeted fixes.

### Option B: Rewrite from scratch
**Strategy**: Close PR #181, implement fresh on a new branch.

**Pros**: Clean slate
**Cons**: Duplicates 80% of correct work already done
**Verdict**: Unnecessary.

### Option C: Extend PR #181 with session dedup + revert detection
**Strategy**: Fix PR #181's bugs AND add missing features.

**Pros**: Complete implementation matching issue spec
**Cons**: Larger scope
**Verdict**: Ideal if scoped carefully.

## Recommended Approach: Option A (Fix PR #181) with minimal additions

### Rationale
1. PR #181's structural changes (types, rollup integration, CLI) are correct
2. The agent counting can be fixed with a `HashSet<String>` tracking sessions per file per date
3. Revert detection can be deferred — store the field as 0, implement in a follow-up
4. CI failures in `edda-serve` are unrelated and may already be fixed on main

### Design Decisions

#### 1. Session Deduplication for Agent Counting

**Problem**: Current PR increments `agents` per event, not per unique session.

**Solution**: Track `BTreeMap<(date, file), HashSet<session_id>>` during aggregation, then set `agents = sessions.len()` at the end.

```rust
// Track unique sessions per (date, file)
let mut file_sessions: BTreeMap<String, BTreeMap<String, HashSet<String>>> = BTreeMap::new();

// During iteration: insert session_id into the set
// After iteration: set agents = sessions.len()
```

**Trade-off**: Extra memory for HashSet, but session counts per day are typically small (< 50).

#### 2. Backward Compatibility

**Problem**: Existing `rollup.json` files don't have `file_edits` field.

**Solution**: Use `#[serde(default)]` on the `file_edits` field. PR #181 does NOT have this annotation — will cause deserialization failures when reading old rollup.json files.

**Fix needed**: Add `#[serde(default)]` to `file_edits` fields on DayStat, WeekStat, MonthStat.

#### 3. Revert Detection — Deferred

**Rationale**: Accurate revert detection is non-trivial. The `reverts` field exists with default 0. Heatmap visualization works without revert data. Implement in follow-up.

#### 4. Rollup Size Management

**Decision**: No top-N filtering for MVP. Most sessions edit < 20 files. Can add filtering later.

### Test Strategy

1. **Unit test**: `file_edits_by_date()` with mock events containing `file_edit_counts`
2. **Unit test**: Session deduplication — same session_id doesn't double-count agents
3. **Unit test**: `merge_file_edits()` correctly sums edits and agents
4. **Unit test**: Backward compat — deserialize DayStat without `file_edits` field
5. **Existing tests**: Ensure all existing rollup tests pass with updated structs

### Risk Assessment

| Risk | Impact | Likelihood | Mitigation |
|------|--------|------------|------------|
| Old rollup.json deserialization fails | High | High (if no serde default) | Add `#[serde(default)]` |
| Agent double-counting on merge | Low | Medium | Document as known limitation |
| Large rollup files | Low | Low | Defer top-N filtering |
| CI failures block merge | High | Medium | Fix edda-serve dead code separately |
