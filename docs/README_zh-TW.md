<h1 align="center">Edda</h1>

<p align="center">
  <strong>Claude Code 的自動決策記憶系統，支援跨 session 持續性。</strong><br/>
  本地優先、確定性查詢、零 API 成本 — 不需要 LLM、不需要 embeddings、不需要雲端。
</p>

<p align="center">
  <a href="https://github.com/fagemx/edda/releases"><img src="https://img.shields.io/github/v/release/fagemx/edda?style=flat-square&label=release" alt="Release" /></a>
  <a href="https://github.com/fagemx/edda/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/fagemx/edda/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="https://github.com/fagemx/edda/blob/main/LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue?style=flat-square" alt="License" /></a>
  <a href="https://github.com/fagemx/edda/stargazers"><img src="https://img.shields.io/github/stars/fagemx/edda?style=flat-square" alt="Stars" /></a>
</p>

<p align="center">
  <a href="#edda-是什麼">Edda 是什麼？</a> ·
  <a href="#安裝">安裝</a> ·
  <a href="#快速開始">快速開始</a> ·
  <a href="#運作原理">運作原理</a> ·
  <a href="#比較">比較</a> ·
  <a href="#整合">整合</a> ·
  <a href="#架構">架構</a>
</p>

<p align="center">
  <a href="./README.md">English</a> · 繁體中文
</p>

<p align="center">
  <img src="https://github.com/user-attachments/assets/03180d1f-5943-4a62-808b-0b8d159a94db" width="700" alt="Edda 概覽" />
</p>

---

## Edda 是什麼？

Claude Code 可以在 session 內壓縮 context，但重要的決策仍然會被淹沒在雜訊中，而且 context 預設不會跨 session 保留。

Edda 採取不同的方式：不是壓縮所有內容，而是提取關鍵決策和其理由，儲存在本地決策 ledger 中，讓未來的 session 可以取用。

當新 session 開始時，agent 可以取得過去的相關決策，了解之前決定了什麼、為什麼這樣決定，然後帶著更好的延續性繼續工作。

Edda 也支援 OpenClaw 和任何 MCP 客戶端。

**你不需要做任何事。** `edda init` 之後，hooks 會處理一切：

| 時機 | Edda 做什麼 | 你做什麼 |
|------|------------|---------|
| Session 開始 | 消化前一次 session，注入過去的決策到 context | 什麼都不用做 |
| Agent 做決策 | Hooks 從 transcript 中偵測並提取 | 什麼都不用做 |
| Session 結束 | 將 session 摘要寫入 ledger | 什麼都不用做 |
| 下次 Session 開始 | Agent 看到所有過去 session 的相關決策 | 什麼都不用做 |

```
Session 1                          Session 2
  Agent 決定 "db=SQLite"             Agent 開始
  Agent 決定 "cache=Redis"    →      Edda 自動注入 context
  Session 結束                       Agent 看到：db=SQLite, cache=Redis
  Edda 消化 transcript               Agent 從上次中斷的地方繼續
```

**所有資料都在本地** — 資料存在 `.edda/`（SQLite + 本地檔案），沒有雲端、沒有帳號、沒有 API 呼叫。

## 安裝

```bash
# 一行安裝（Linux / macOS）
curl -sSf https://raw.githubusercontent.com/fagemx/edda/main/install.sh | sh

# macOS / Linux（Homebrew）
brew install fagemx/tap/edda

# 或下載預編譯的二進位檔
# → https://github.com/fagemx/edda/releases

# 或從原始碼編譯
cargo install --git https://github.com/fagemx/edda edda
```

## 快速開始

```bash
edda init    # 自動偵測 Claude Code，安裝 hooks
# 完成。開始寫程式。Edda 在背景運作。
```

`edda init` 做三件事：

1. 建立 `.edda/`，包含空的 ledger
2. 將 lifecycle hooks 安裝到 `.claude/settings.local.json`
3. 在 `.claude/CLAUDE.md` 加入決策追蹤指引

CLAUDE.md 的段落會教 agent 何時以及如何記錄決策：

```markdown
## 決策追蹤（edda）

當你做出架構決策時，記錄下來：
  edda decide "domain.aspect=value" --reason "why"

在結束 session 前，總結你做了什麼：
  edda note "completed X; decided Y; next: Z" --tag session
```

這是 Edda 自動化的關鍵 — agent 在對話中自然地呼叫 `edda decide`，hooks 會捕捉其他一切。

## 運作原理

```
Claude Code session
        │
   Bridge hooks（自動）
        │
        ▼
   ┌─────────┐
   │  .edda/  │  ← append-only SQLite ledger
   │  ledger  │  ← hash-chained 事件
   └─────────┘
        │
   Context 注入（下次 session）
        │
        ▼
   Agent 看到所有過去的決策
```

Edda 將每個事件以 hash-chained JSON 記錄儲存在本地 SQLite 資料庫中。事件包括決策、筆記、session 摘要和指令輸出。Hash chain 讓歷史記錄具有防篡改性。

在每次 session 開始時，Edda 從 ledger 組裝 context snapshot 並注入 — agent 可以看到最近的決策、進行中的任務和相關歷史，不需要閱讀舊的 transcript。

## 比較

|  | MEMORY.md | RAG / 向量資料庫 | LLM 摘要 | **Edda** |
|--|-----------|-----------------|---------|----------|
| **儲存** | Markdown 檔案 | 向量 embeddings | LLM 生成的文字 | Append-only SQLite |
| **檢索** | Agent 讀取整個檔案 | 語意相似度 | LLM 重新摘要 | Tantivy 全文搜尋 + 結構化查詢 |
| **需要 LLM？** | 否 | 是（embeddings） | 是（每次讀寫） | **否** |
| **需要向量資料庫？** | 否 | 是 | 否 | **否** |
| **防篡改？** | 否 | 否 | 否 | **是**（hash chain） |
| **追蹤「為什麼」？** | 偶爾 | 否 | 有損 | **是**（理由 + 被拒絕的方案） |
| **跨 Session？** | 手動複製 | 是 | Session 範圍內 | **是**（自動） |
| **每次查詢成本** | 免費 | Embedding API 呼叫 | LLM API 呼叫 | **免費**（本地 SQLite） |
| **範例** | Claude Code 內建、OpenClaw | mem0、Zep、Chroma | ChatGPT Memory、Copilot | — |

每次查詢都在本地 SQLite 上執行 — 每次都得到相同答案，毫秒級，零成本。

## 整合

**Claude Code** — 透過 bridge hooks 完整支援。自動捕捉決策、消化 session、注入 context。

```bash
edda init    # 偵測 Claude Code，自動安裝 hooks
```

**OpenClaw** — 透過 bridge 插件支援。

```bash
edda bridge openclaw install    # 安裝全域插件
```

**任何 MCP 客戶端**（Cursor、Windsurf 等）— 透過 MCP server 提供 7 個工具：

```bash
edda mcp serve    # stdio JSON-RPC 2.0
# 工具：edda_status, edda_note, edda_decide, edda_ask, edda_log, edda_context, edda_draft_inbox
```

## 手動工具

大多數時候 hooks 會自動處理一切。當你想手動記錄或查詢時，可以使用這些指令：

```bash
edda ask "cache"           # 查詢過去的決策
edda search query "auth"   # 全文搜尋 transcripts
edda context               # 查看 agent 在 session 開始時看到什麼
edda log --tag decision    # 篩選事件日誌
edda watch                 # 即時 TUI：peers、事件、決策
```

<details>
<summary>所有指令</summary>

| 指令 | 說明 |
|------|------|
| `edda init` | 初始化 `.edda/`（偵測到 `.claude/` 時自動安裝 hooks） |
| `edda decide` | 記錄一個 binding decision |
| `edda note` | 記錄筆記 |
| `edda ask` | 查詢決策、歷史和對話 |
| `edda search` | 全文搜尋 transcripts（Tantivy） |
| `edda log` | 用篩選條件查詢事件（類型、日期、標籤、分支） |
| `edda context` | 輸出 context snapshot（agent 看到的內容） |
| `edda status` | 顯示 workspace 狀態 |
| `edda watch` | 即時 TUI：peers、事件、決策 |
| `edda commit` | 建立 commit 事件 |
| `edda branch` | 分支操作 |
| `edda switch` | 切換分支 |
| `edda merge` | 合併分支 |
| `edda draft` | 提案 / 列表 / 批准 / 拒絕 drafts |
| `edda bridge` | 安裝/移除工具 hooks |
| `edda doctor` | 健康檢查 |
| `edda config` | 讀寫 workspace 設定 |
| `edda pattern` | 管理分類模式 |
| `edda mcp` | 啟動 MCP server（stdio JSON-RPC 2.0） |
| `edda conduct` | 多階段計畫編排 |
| `edda plan` | 計畫鷹架和範本 |
| `edda run` | 執行指令並記錄輸出 |
| `edda blob` | 管理 blob metadata |
| `edda gc` | 垃圾回收過期內容 |

</details>

## 架構

14 個 Rust crates：

| Crate | 功能 |
|-------|------|
| `edda-core` | 事件模型、hash chain、schema、provenance |
| `edda-ledger` | Append-only ledger（SQLite）、blob store、locking |
| `edda-cli` | 所有指令 + TUI（`tui` feature，預設開啟） |
| `edda-bridge-claude` | Claude Code hooks、transcript 攝取、context 注入 |
| `edda-bridge-openclaw` | OpenClaw hooks 和插件 |
| `edda-mcp` | MCP server（7 個工具） |
| `edda-ask` | 跨來源決策查詢引擎 |
| `edda-derive` | View 重建、分層歷史 |
| `edda-pack` | Context 生成、預算控制 |
| `edda-transcript` | Transcript delta 攝取、分類 |
| `edda-store` | 每用戶 store、原子寫入 |
| `edda-search-fts` | 全文搜尋（Tantivy） |
| `edda-index` | Transcript 索引 |
| `edda-conductor` | 多階段計畫編排 |

<details>
<summary>.edda/ 裡面有什麼</summary>

```
.edda/
├── ledger.db             # SQLite：事件、HEAD、分支（append-only、hash-chained）
├── ledger/
│   └── blobs/            # 大型 payloads
├── branches/             # 分支 metadata
├── drafts/               # 待處理的提案
├── patterns/             # 分類模式
├── actors.yaml           # 角色（lead、reviewer）
├── policy.yaml           # 批准規則
└── config.json           # Workspace 設定
```

每個事件遵循 hash-chained JSON schema（儲存在本地 SQLite ledger 中）：

```json
{
  "event_id": "evt_01khj03c1bteqm3ffrv57adtmt",
  "ts": "2026-02-16T01:12:38.187Z",
  "type": "note",
  "branch": "main",
  "parent_hash": "217456ef...",
  "hash": "2dfe06e7...",
  "payload": {
    "role": "user",
    "tags": [],
    "text": "Phase 0 complete: edda in PATH, hooks installed"
  },
  "refs": {}
}
```

</details>

## 路線圖

- [x] 預編譯二進位檔（macOS、Linux、Windows）
- [x] 一行安裝腳本（`curl | sh`）
- [x] Homebrew tap（`brew install fagemx/tap/edda`）
- [ ] 決策回憶指標
- [ ] 跨專案決策搜尋
- [ ] 基於 tmux 的多面板 TUI（L3）

## 貢獻

歡迎貢獻。請參閱 [CONTRIBUTING.md](CONTRIBUTING.md) 了解開發環境設定。

## 社群

- [GitHub Issues](https://github.com/fagemx/edda/issues) — 回報 bug 和功能請求
- [Releases](https://github.com/fagemx/edda/releases) — 更新日誌和二進位檔

## 授權

MIT OR Apache-2.0

---

*你的 agent 的架構決策不應該每次 session 都重置。*
