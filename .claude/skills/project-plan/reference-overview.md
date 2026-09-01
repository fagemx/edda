# Reference: 00_OVERVIEW.md

> This is a real planning pack overview from a production project (gctx — Git-style Context Controller).
> Use this as the quality bar when generating 00_OVERVIEW.md.

---

# gctx — Planning Pack

## Goal

一個完整的 Git-style Context Controller CLI 工具（Rust），為 LLM agent 提供：
- **可重放（replayable）** 的 append-only event ledger + content-addressable blob store
- **可重建（rebuildable）** 的 derived views（commit.md, log.md, main.md, metadata.yaml）
- **可治理（governable）** 的 Draft → Policy Gate → Approval → Apply 工作流
- **可整合（integrable）** 的 Agent Bridge（Claude Code hooks / Codex / Cursor）

## Dependency DAG

```
L0 基礎設施
  [A] Ledger + Blob Store
   │
   ▼
L1 工作記憶
  [B] Commit + Derive Views
   │
   ▼
  [C] Branch / Switch / Merge
   │
   ▼
L2 自動化
  [D] Auto Evidence
   │
   ▼
  [E] Draft System
   │
   ▼
L3 治理層
  [F] Policy Gate
   │
   ├──────────────────────┐
   ▼                      ▼
  [G] Approval Routing   [H] Agent Bridge
```

**關鍵依賴說明**：
- A 是所有 Track 的前提（ledger 必須先存在）
- B-C-D-E 形成嚴格線性依賴鏈
- F 是治理層的基礎
- G 和 H 皆依賴 F，可並行開發

## Track Summary

| Track | Name | Layer | Tasks | Dependencies | Status |
|-------|------|-------|-------|-------------|--------|
| A | Ledger + Blob Store | L0 | 3 | — | ☐ |
| B | Commit + Derive Views | L1 | 3 | A | ☐ |
| C | Branch / Switch / Merge | L1 | 2 | B | ☐ |
| D | Auto Evidence Collection | L2 | 1 | C | ☐ |
| E | Draft Propose / Apply | L2 | 2 | D | ☐ |
| F | Policy Gate + Approval Queue | L3 | 1 | E | ☐ |
| G | Multi-Level Approval Routing | L3 | 2 | F | ☐ |
| H | Agent Bridge | L4 | 4 | F | ☐ |

**Total: 8 Tracks, 18 Tasks**

## Parallel Execution Timeline

```
Batch 1（無依賴）：
  Agent 1 → Track A: A1 → A2 → A3

Batch 2（依賴 A）：
  Agent 1 → Track B: B1 → B2 → B3

Batch 3（依賴 B）：
  Agent 1 → Track C: C1 → C2

Batch 4-6（線性）：
  Track D → Track E → Track F

Batch 7（依賴 F，可並行）：
  Agent 1 → Track G: G1 → G2
  Agent 2 → Track H: H1 → H2 → H3
```

## Progress Tracking

### Batch 1
- [ ] Track A: Ledger + Blob Store
  - [ ] A1: Core Types + Canonical JSON + Hash
  - [ ] A2: Ledger Store + Blob Store + Lock
  - [ ] A3: CLI Skeleton + init/note/run

### Batch 2
- [ ] Track B: Commit + Derive Views
  - [ ] B1: Commit/Rebuild Event Builders
  - [ ] B2: Derive Views Engine
  - [ ] B3: CLI: status/commit/context/rebuild

*(...continues for all batches)*

## Module Map

### Core (Track A-G)

| Module | Introduced | Responsibility |
|--------|-----------|----------------|
| `gctx-core` | A1 | Event model, hash chain, canonical JSON, ID generation |
| `gctx-ledger` | A2 | Append-only JSONL ledger, blob store, workspace lock |
| `gctx-derive` | B2 | Derive views, auto-evidence, context builder |
| `gctx-cli` | A3 | CLI entry point + all subcommand dispatch |

## Event Type Registry

所有 event type 的正式清單（跨所有 Track）。

| type | Introduced | Description |
|------|-----------|-------------|
| `note` | A1 | Free-form annotation (role, tag, blob_ref) |
| `cmd` | A1 | Command execution record (exit_code, stdout/stderr blob refs) |
| `commit` | B1 | Working memory milestone (title, purpose, evidence, labels) |
| `branch_create` | C1 | Create new branch (from_event, purpose) |
| `merge` | C1 | Fast-forward merge (src → dst, adopted_commits) |

## Data File Layout

```
.gctx/
  ledger/
    events.jsonl          # append-only event stream (hash chain)
    blobs/<sha256>        # content-addressable blob store
  branches/
    <name>/
      main.md             # derived: current working memory
      commit.md           # derived: milestone list
      log.md              # derived: readable trace
  refs/
    HEAD                  # current branch name
    branches.json         # branch cache
  drafts/
    <draft_id>.json       # draft proposals
  policy.yaml             # governance rules
  LOCK
```

---

> **Pattern notes for generation:**
> - Goal section uses bullet points with bold keywords (可重放, 可重建, etc.)
> - DAG is ASCII art with layer labels
> - Track summary includes ALL columns: Track letter, Name, Layer, Task count, Dependencies, Status checkbox
> - Parallel timeline groups into numbered batches with agent assignment
> - Progress tracking is nested checkboxes matching batches
> - Module map shows when each module is introduced
> - Event/data registries give cross-cutting visibility
