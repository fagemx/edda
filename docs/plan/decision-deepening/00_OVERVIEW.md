# Decision Deepening — Planning Pack

## Goal

把 Edda 從「決策存檔層」推到「決策施壓層」：
- **有 scope** 的 decision — `edda decide "X=Y" --paths "crates/foo/**"` 讓每條決策守住明確的檔案範圍
- **會浮出** 的 decision — 改到被守護的檔案時自動噴警告，不用手動查
- **有生命週期** 的 decision — status enum 取代 `is_active: bool`，支持 proposed/active/superseded

聚焦「20% 讓用戶有感」：Schema V10 → `--paths` flag → PreToolUse 檔案警告 → Session Start 決策 pack 注入。

## Dependency DAG

```
L0 地基
  [A] Schema V10 Migration
   │
   ├────────────────────┐
   ▼                    ▼
L1 寫入                L1 讀取
  [B] Mutation          [C] Decision View
  Contract              + to_view()
   │                    │
   ├────────────────────┤
   ▼                    ▼
L2 CLI                 L2 Hook 注入
  [D] --paths flag      [E] PreToolUse Warning
  on edda decide        │
                        ▼
                       [F] Session Start Pack
```

**關鍵依賴說明**：
- A 是所有 Track 的前提（新欄位必須先存在）
- B 和 C 同屬 L1，可並行（B 寫入側，C 讀取側）
- D 依賴 B（CLI 呼叫 mutation contract 寫入 `affected_paths`）
- E 依賴 C（hook 呼叫 `to_view()` + glob matching）
- F 依賴 C（session start 呼叫 query + pack generation）
- E 和 F 可並行

## Track Summary

| Track | Name | Layer | Tasks | Dependencies | Status |
|-------|------|-------|-------|-------------|--------|
| A | Schema V10 Migration | L0 | 2 | — | ☐ |
| B | Mutation Contract | L1 | 2 | A | ☐ |
| C | Decision View + to_view() | L1 | 2 | A | ☐ |
| D | CLI --paths flag | L2 | 1 | B | ☐ |
| E | PreToolUse File Warning | L2 | 2 | C | ☐ |
| F | Session Start Decision Pack | L2 | 2 | C | ☐ |

**Total: 6 Tracks, 11 Tasks**

## Parallel Execution Timeline

```
Batch 1（無依賴）：
  Agent 1 → Track A: A1 → A2

Batch 2（依賴 A，可並行）：
  Agent 1 → Track B: B1 → B2
  Agent 2 → Track C: C1 → C2

Batch 3（依賴 B/C，可並行）：
  Agent 1 → Track D: D1
  Agent 2 → Track E: E1 → E2
  Agent 3 → Track F: F1 → F2
```

## Progress Tracking

### Batch 1
- [ ] Track A: Schema V10 Migration
  - [ ] A1: DDL Migration (add columns with defaults)
  - [ ] A2: Backfill + Index + Version bump

### Batch 2
- [ ] Track B: Mutation Contract
  - [ ] B1: status + is_active sync in insert_decision
  - [ ] B2: create_candidate() accepts new fields
- [ ] Track C: Decision View + to_view()
  - [ ] C1: DecisionView struct + to_view() function
  - [ ] C2: query_by_paths() function

### Batch 3
- [ ] Track D: CLI --paths flag
  - [ ] D1: Add --paths and --tags to edda decide
- [ ] Track E: PreToolUse File Warning
  - [ ] E1: Glob match engine (affected_paths vs file path)
  - [ ] E2: Hook integration (dispatch_pre_tool_use)
- [ ] Track F: Session Start Decision Pack
  - [ ] F1: DecisionPack type + build_pack() function
  - [ ] F2: Hook integration (dispatch_session_start)

## Module Map

| Module | Track | Change Type | Responsibility |
|--------|-------|-------------|----------------|
| `edda-ledger/src/sqlite_store.rs` | A1, A2, B1, B2 | Modify | Schema migration + insert logic |
| `edda-ledger/src/ledger.rs` | B2, C2 | Modify | New query methods |
| `edda-core/src/types.rs` | B2 | Modify | DecisionPayload 新欄位 |
| `edda-ledger/src/view.rs` | C1 | **New** | DecisionView struct + to_view() |
| `edda-cli/src/cmd_bridge.rs` | D1 | Modify | --paths/--tags CLI args |
| `edda-cli/src/main.rs` | D1 | Modify | Decide command args |
| `edda-bridge-claude/src/dispatch/tools.rs` | E2 | Modify | PreToolUse decision warning |
| `edda-bridge-claude/src/dispatch/session.rs` | F2 | Modify | Session start pack injection |
| `edda-pack/src/lib.rs` | F1 | Modify | DecisionPack type + builder |
