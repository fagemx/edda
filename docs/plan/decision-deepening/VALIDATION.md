# Decision Deepening — Validation Plan

## Track Acceptance Criteria

### Track A: Schema V10 Migration

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| Build | `cargo build -p edda-ledger` zero errors | `cargo build -p edda-ledger 2>&1` |
| Tests | All existing tests pass | `cargo test -p edda-ledger` |
| New columns | `status`, `authority`, `affected_paths`, `tags`, `review_after`, `reversibility` exist | `sqlite3 ledger.db ".schema decisions"` |
| Defaults | New columns have sensible defaults | `SELECT status, authority, affected_paths FROM decisions LIMIT 1` → `active, human, []` |
| Backward compat | `WHERE is_active = TRUE` returns same results as before | Compare query results pre/post migration |
| Version | Schema version bumped | `SELECT value FROM schema_meta WHERE key='version'` → `10` |

### Track B: Mutation Contract

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| Status sync | New decisions have `status` set | `edda decide "t=v" && sqlite3 ledger.db "SELECT status FROM decisions WHERE key='t'"` → `active` |
| Invariant | COMPAT-01 holds | SQL invariant check query → 0 rows |
| New fields | `affected_paths`, `tags` written when provided | Test with programmatic API |
| Default compat | Existing callers compile without changes | `cargo build --workspace` |

### Track C: Decision View

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| to_view() | Parses `affected_paths` JSON → Vec<String> | Unit test |
| to_view() | Renames `scope` → `propagation` | Unit test |
| to_view() | Drops `is_active` field | Compile check (field not on DecisionView) |
| query_by_paths | `"crates/edda-ledger/**"` matches `"crates/edda-ledger/src/lib.rs"` | Unit test |
| query_by_paths | No match → empty vec | Unit test |
| query_by_paths | Only returns active/experimental decisions | Unit test |

### Track D: CLI --paths flag

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| --paths arg | `edda decide "k=v" --paths "src/**"` writes `["src/**"]` | `sqlite3 ledger.db "SELECT affected_paths ..."` |
| --tags arg | `edda decide "k=v" --tags "arch,db"` writes `["arch","db"]` | `sqlite3 ledger.db "SELECT tags ..."` |
| No --paths | Default `[]` | Check default behavior |
| Help text | `edda decide --help` shows --paths and --tags | `edda decide --help` |

### Track E: PreToolUse Warning

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| Match found | Warning injected when editing guarded file | Manual test with Claude Code |
| No match | Zero overhead when no decisions match | Timing log < 1ms |
| Latency | Total < 100ms | `decision_warning_ms` debug log |
| Only Edit | Warning only on Edit tool, not Bash/Read | Manual test |
| Format | Markdown with decision key=value + reason | Visual check |

### Track F: Session Start Pack

| Item | Pass Criteria | Verification |
|------|--------------|-------------|
| Injection | Active decisions appear in session context | Start session, check `<!-- edda:start -->` block |
| Max items | At most 7 decisions | Create 10 decisions, verify only 7 shown |
| Empty | No section when 0 active decisions with paths | Clean project, start session |
| Integration | Existing hot.md still works | Verify turns/narrative still present |

---

## Golden Path Scenarios

### GP-1: 最小閉迴路 — 寫 decision → 被警告（Track A + B + C + D + E）

**Description**: 從零到第一次被 Edda「卡住」。

**Steps**:
1. `edda decide "core.no_llm=true" --reason "keep core deterministic" --paths "crates/edda-core/**"`
2. 開啟 Claude Code session
3. 嘗試編輯 `crates/edda-core/src/types.rs`
4. PreToolUse hook 應噴出：
   ```
   [edda] Active decisions governing this file:
     - core.no_llm=true — keep core deterministic
   ```

**Verification**: 用戶看到警告，知道這個檔案被決策守護。

---

### GP-2: Session Start 自動注入（Track A + B + C + F）

**Description**: 開 session 時自動看到 relevant decisions。

**Steps**:
1. `edda decide "db.engine=sqlite" --reason "embedded" --paths "crates/edda-ledger/**"`
2. `edda decide "error.pattern=thiserror" --reason "consistent across crates"`
3. 開啟 Claude Code session
4. Session context 應包含 `## Active Decisions` section

**Verification**: Session start 輸出含決策列表，不用手動 `edda ask`。

---

### GP-3: Supersede + 警告更新（Track A + B + D + E）

**Description**: 改變心意後，警告自動更新。

**Steps**:
1. `edda decide "db.engine=sqlite" --reason "embedded" --paths "crates/edda-ledger/**"`
2. 編輯 `crates/edda-ledger/src/lib.rs` → 看到 `db.engine=sqlite` 警告
3. `edda decide "db.engine=postgres" --reason "need multi-user" --paths "crates/edda-ledger/**"`
4. 再次編輯 `crates/edda-ledger/src/lib.rs` → 看到 `db.engine=postgres` 警告（不是 sqlite）

**Verification**: Supersede 後，舊 decision 不再出現在警告中。

---

## Quality Benchmarks

| CONTRACT Rule | Metric | Baseline | Verification |
|--------------|--------|----------|-------------|
| COMPAT-01 | is_active ↔ status agreement | 0 violations | SQL invariant check |
| COMPAT-02 | Existing query backward compat | All tests pass | `cargo test --workspace` |
| MUTATION-01 | No direct UPDATE outside contract | 0 rogue UPDATE | `grep` check |
| BOUNDARY-01 | Injection never imports DecisionRow | 0 imports | `grep` check |
| PERF-01 | PreToolUse latency | < 100ms | Debug timing log |
| CLIPPY-01 | Clippy warnings | 0 | `cargo clippy --workspace` |
| TEST-01 | Test pass rate | 100% | `cargo test --workspace` |
