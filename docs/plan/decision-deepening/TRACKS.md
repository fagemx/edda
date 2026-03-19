# Decision Deepening — Track 拆解

## 層級定義

- **L0 地基**：Schema V10 migration — 新欄位加入 SQLite，所有後續工作的前提
- **L1 契約**：Mutation contract + DecisionView — 寫入側和讀取側的 API 邊界
- **L2 產品面**：CLI `--paths` + PreToolUse warning + Session Start pack — 用戶直接感受到的改動

## DAG

```
L0 地基
  [A] Schema V10 Migration
   │
   ├────────────────────┐
   ▼                    ▼
L1 契約（可並行）
  [B] Mutation          [C] Decision View
  Contract              + to_view()
   │                    │
   ▼                    ├──────────┐
L2 產品面              ▼          ▼
  [D] --paths flag     [E] Pre    [F] Session
  on edda decide       ToolUse    Start Pack
                       Warning
```

**關鍵依賴說明**：
- A 是所有 Track 的前提（`affected_paths`, `status`, `tags` 欄位必須存在）
- B 和 C 可完全並行（B 改寫入邏輯，C 加讀取投影，互不干擾）
- D 只依賴 B（CLI 寫入 `affected_paths` 需要 mutation contract 支持）
- E 和 F 只依賴 C（讀取 `DecisionView` + glob matching / pack building）
- D、E、F 三個 Track 可完全並行（分屬不同 crate）

## Track → Step 對照

### A: Schema V10 Migration（L0）
```
TRACK_A_SCHEMA_V10/
  A1_DDL_MIGRATION.md            ← ALTER TABLE + backfill + version bump
  A2_INDEXES_AND_VERIFY.md       ← 新 index + 驗證 query + 測試
```

### B: Mutation Contract（L1）
```
TRACK_B_MUTATION_CONTRACT/
  B1_STATUS_SYNC.md              ← insert_decision 寫入 status + is_active 同步
  B2_CREATE_CANDIDATE.md         ← create_candidate() 接受 authority/paths/tags/reversibility
```

### C: Decision View（L1）
```
TRACK_C_DECISION_VIEW/
  C1_VIEW_AND_CONVERT.md         ← DecisionView struct + to_view() + 測試
  C2_QUERY_BY_PATHS.md           ← query_by_paths() glob matching + 測試
```

### D: CLI --paths flag（L2）
```
TRACK_D_CLI_PATHS/
  D1_PATHS_AND_TAGS.md           ← --paths/--tags args + 寫入 affected_paths
```

### E: PreToolUse Warning（L2）
```
TRACK_E_PRETOOLUSE/
  E1_GLOB_MATCH.md               ← 從 active decisions 中 glob match 檔案路徑
  E2_HOOK_INTEGRATION.md         ← 接入 dispatch_pre_tool_use + 格式化警告
```

### F: Session Start Pack（L2）
```
TRACK_F_SESSION_PACK/
  F1_DECISION_PACK.md            ← DecisionPack type + build_pack() + 排序
  F2_HOOK_INTEGRATION.md         ← 接入 dispatch_session_start + markdown render
```

## Module Import 路徑

```
crates/
  edda-core/src/
    types.rs                     ← DecisionPayload 新欄位（B2）
  edda-ledger/src/
    sqlite_store.rs              ← SCHEMA_V10_SQL, insert 新欄位（A1, A2, B1, B2）
    ledger.rs                    ← query_by_paths(), active_decisions_with_paths()（C2）
    view.rs                      ← DecisionView struct, to_view()（C1）【新檔案】
  edda-cli/src/
    main.rs                      ← Decide command 新增 --paths/--tags args（D1）
    cmd_bridge.rs                ← decide() 傳入 affected_paths/tags（D1）
  edda-pack/src/
    lib.rs                       ← DecisionPack type, build_decision_pack()（F1）
  edda-bridge-claude/src/
    dispatch/
      tools.rs                   ← decision_file_warning()（E1, E2）
      session.rs                 ← inject_decision_pack()（F2）
```

## 跨模組依賴圖（import 方向）

```
edda-core (types)
  ← edda-ledger (storage + query)
       ← edda-cli (CLI commands)
       ← edda-bridge-claude (hooks)
       ← edda-pack (pack generation)
       ← edda-ask (query engine, 不改)

edda-ledger::view (DecisionView)
  ← edda-pack (build_decision_pack)
  ← edda-bridge-claude::dispatch (PreToolUse + SessionStart)
```

**規則**：
- `edda-bridge-claude` 和 `edda-pack` 只 import `DecisionView`（from `edda-ledger::view`），不碰 `DecisionRow`
- `edda-ledger::view` 是 `DecisionRow` → `DecisionView` 的唯一轉換點
- `edda-ask` 不改動 — Injection 是 parallel query path，不是 extension

---

## Track A: Schema V10 Migration

**Layer**: L0
**Goal**: 在 SQLite decisions table 加入 `status`, `authority`, `affected_paths`, `tags`, `review_after`, `reversibility` 六個欄位

**Input**:
- `docs/decision/decision-model/schema-v0.md` §5（V10 migration SQL）
- 現有 `crates/edda-ledger/src/sqlite_store.rs`（V9 schema）

**Output**:
- decisions table 有 6 個新欄位，帶 default 值
- 既有 `is_active` 查詢不受影響
- `schema_version = 10`

**Dependencies**:
- blocks: B, C, D, E, F
- blocked-by: none（可立即開始）

**DoD**:
- [ ] `cargo build -p edda-ledger` zero errors
- [ ] `cargo test -p edda-ledger` all pass
- [ ] `SELECT status, authority, affected_paths FROM decisions LIMIT 1` 回傳 `'active', 'human', '[]'`
- [ ] 既有 `is_active` 查詢結果不變

**Smoke Test**:
```bash
cargo build -p edda-ledger
cargo test -p edda-ledger
# Open any existing ledger.db:
sqlite3 /path/to/ledger.db "SELECT status, authority, affected_paths, tags, reversibility FROM decisions LIMIT 3;"
```

**Task Count**: 2

---

## Track B: Mutation Contract

**Layer**: L1
**Goal**: 讓寫入路徑正確設定 `status` + `is_active` 同步，並讓 `create_candidate()` 接受新欄位

**Input**:
- Track A 完成（V10 欄位存在）
- `docs/decision/decision-model/api.md`（mutation contract spec）

**Output**:
- `insert_decision()` 寫入 `status` 並同步 `is_active`
- DecisionPayload struct 接受 `authority`, `affected_paths`, `tags`, `review_after`, `reversibility`
- 既有 caller 傳 None → 使用 default

**Dependencies**:
- blocks: D
- blocked-by: A

**DoD**:
- [ ] `cargo test -p edda-ledger` all pass
- [ ] 新 decision 的 `status` 欄位被正確寫入
- [ ] `is_active ↔ status` invariant（CONTRACT COMPAT-01）通過
- [ ] 既有 caller 不改動也能編譯

**Smoke Test**:
```bash
cargo test -p edda-ledger
cargo test -p edda-cli
edda decide "test.key=value" --reason "test"
sqlite3 ledger.db "SELECT status, authority FROM decisions WHERE key='test.key';"
# Expected: active, human
```

**Task Count**: 2

---

## Track C: Decision View + to_view()

**Layer**: L1
**Goal**: 建立 `DecisionView` 讀取投影和 `to_view()` 轉換函數，以及 `query_by_paths()` glob matching

**Input**:
- Track A 完成（`affected_paths` 欄位存在）
- `docs/decision/decision-model/shared-types.md` §2.3（DecisionView spec）

**Output**:
- `edda-ledger` 新增 `view.rs` module
- `DecisionView` struct（parsed arrays，不是 JSON strings）
- `to_view(row: DecisionRow) → DecisionView` 函數
- `query_by_paths(ledger, paths, branch, limit) → Vec<DecisionView>` 函數

**Dependencies**:
- blocks: E, F
- blocked-by: A

**DoD**:
- [ ] `cargo build -p edda-ledger` zero errors
- [ ] `to_view()` 正確 parse `affected_paths` 和 `tags` 從 JSON string → Vec<String>
- [ ] `query_by_paths()` 對 glob pattern matching 正確（`"crates/edda-ledger/**"` matches `"crates/edda-ledger/src/lib.rs"`）
- [ ] `DecisionView` 不含 `is_active`（只有 `status`）
- [ ] `DecisionView` 用 `propagation` 而非 `scope`

**Smoke Test**:
```bash
cargo test -p edda-ledger -- view
# Unit tests for to_view() and query_by_paths()
```

**Task Count**: 2

---

## Track D: CLI --paths flag

**Layer**: L2
**Goal**: 讓 `edda decide` 支援 `--paths` 和 `--tags` 參數，寫入 `affected_paths` 和 `tags`

**Input**:
- Track B 完成（mutation contract 接受新欄位）

**Output**:
- `edda decide "db.engine=sqlite" --reason "..." --paths "crates/edda-ledger/**" --tags architecture,storage`
- `affected_paths` 和 `tags` 正確寫入 decisions table

**Dependencies**:
- blocks: none（這是終端 Track）
- blocked-by: B

**DoD**:
- [ ] `edda decide "test.x=y" --paths "src/**" --tags test` 寫入正確
- [ ] `sqlite3 ledger.db "SELECT affected_paths, tags FROM decisions WHERE key='test.x';"` → `'["src/**"]', '["test"]'`
- [ ] 不帶 `--paths` 時 default 為 `[]`

**Smoke Test**:
```bash
edda decide "test.paths=yes" --reason "test" --paths "crates/foo/**" --tags arch
sqlite3 /path/to/ledger.db "SELECT affected_paths, tags FROM decisions WHERE key='test.paths';"
```

**Task Count**: 1

---

## Track E: PreToolUse File Warning

**Layer**: L2
**Goal**: 在 Claude Code 的 PreToolUse hook 中，當 agent 要編輯被 decision 守護的檔案時，注入警告

**Input**:
- Track C 完成（`query_by_paths()` 和 `DecisionView` 可用）

**Output**:
- 編輯 `crates/edda-ledger/src/lib.rs` 時，如果有 decision 的 `affected_paths` 含 `"crates/edda-ledger/**"`，注入：
  ```
  [edda] Active decisions governing this file:
    - db.engine=sqlite — embedded, zero-config
  ```

**Dependencies**:
- blocks: none
- blocked-by: C

**DoD**:
- [ ] PreToolUse hook 在 Edit tool 觸發時檢查 `affected_paths`
- [ ] 有匹配時注入 markdown 警告
- [ ] 無匹配時不注入（零開銷 fast path）
- [ ] 整體 < 100ms（CONTRACT PERF-01）
- [ ] 不影響 Bash/Read/Write 等其他 tool 的 PreToolUse

**Smoke Test**:
```bash
edda decide "test.guard=on" --reason "test" --paths "crates/edda-ledger/**"
# Then trigger a Claude Code session and edit crates/edda-ledger/src/lib.rs
# Expect: edda hook output contains decision warning
```

**Task Count**: 2

---

## Track F: Session Start Decision Pack

**Layer**: L2
**Goal**: 在 SessionStart hook 自動注入 top-N active decisions 作為 context

**Input**:
- Track C 完成（`query_by_paths()` 和 `DecisionView` 可用）

**Output**:
- SessionStart 時，根據 branch + recent files 查詢 relevant decisions
- 注入 3-7 條 active decisions 到 session context（markdown 格式）
- 與現有 hot.md pack 整合（不取代，追加）

**Dependencies**:
- blocks: none
- blocked-by: C

**DoD**:
- [ ] SessionStart hook 注入 `## Active Decisions` section
- [ ] 最多 7 條，按 domain 分組
- [ ] 空時不注入（不浪費 context budget）
- [ ] 與現有 `render_pack()` 整合，不破壞現有 hot.md 結構

**Smoke Test**:
```bash
edda decide "db.engine=sqlite" --reason "embedded" --paths "crates/edda-ledger/**"
edda decide "error.pattern=thiserror" --reason "consistent"
# Start a new Claude Code session
# Expect: session context includes Active Decisions section
```

**Task Count**: 2
