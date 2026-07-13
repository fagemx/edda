<h1 align="center">Edda</h1>

<p align="center">
  <strong>你的 agent 的決策，不該每開新 session 就歸零。</strong><br/>
  Edda 給 coding agent 一份本地、自動的記憶：記住決定了什麼——以及為什麼。<br/>
  支援 Claude Code、Codex、OpenClaw 和任何 MCP 客戶端。
</p>

<p align="center">
  <a href="https://crates.io/crates/edda"><img src="https://img.shields.io/crates/v/edda?style=flat-square" alt="crates.io" /></a>
  <a href="https://github.com/fagemx/edda/releases"><img src="https://img.shields.io/github/v/release/fagemx/edda?style=flat-square&label=release" alt="Release" /></a>
  <a href="https://github.com/fagemx/edda/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/fagemx/edda/ci.yml?style=flat-square&label=CI" alt="CI" /></a>
  <a href="https://github.com/fagemx/edda/blob/main/LICENSE-MIT"><img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-blue?style=flat-square" alt="License" /></a>
  <a href="https://github.com/fagemx/edda/stargazers"><img src="https://img.shields.io/github/stars/fagemx/edda?style=flat-square" alt="Stars" /></a>
</p>

<p align="center">
  <a href="#為什麼需要-edda">為什麼需要 Edda？</a> ·
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

## 為什麼需要 Edda？

昨天你和 agent 把利弊吵完，定案用 SQLite。今天開新 session——它又提議 Postgres。又來一次。推理跟著 transcript 一起消失了，context 壓縮救不回來。

Edda 治的就是這個：hooks 看著你的 session，把每個決策連同理由記進本地 ledger，在下一個 session 開始前交到它手上。agent 從此不再失憶。

```
沒有 edda                             有 edda
────────                              ───────
Session 2 開場：                      Session 2 開場：
  「我建議這裡用 Postgres——             「延續 SQLite（昨天已定案：
    它有 JSONB，而且…」                   單一寫入者、不需要 JSONB）…」
你：「這我們昨天就定案了！」
```

**你不需要做任何事。** `edda init` 之後，hooks 會處理一切：

| 時機 | Edda 做什麼 | 你做什麼 |
|------|------------|---------|
| Session 開始 | 消化前一次 session，注入過去的決策到 context | 什麼都不用做 |
| Agent 做決策 | Hooks 從 transcript 中偵測並提取 | 什麼都不用做 |
| Session 結束 | 將 session 摘要寫入 ledger | 什麼都不用做 |
| 下次 Session 開始 | Agent 看到所有過去 session 的相關決策 | 什麼都不用做 |

**資料都在本地** — ledger 存在 `.edda/`（SQLite + 本地檔案），沒有雲端、沒有帳號。核心迴圈（記錄、檢索、注入）是確定性的、永不外呼。**可選的 LLM 增強**（session 摘要、決策萃取、模式關聯）需設 `EDDA_LLM_API_KEY` 且有每日預算上限——不設 key，edda 就是完全零 egress。

## 一份記憶，每個 agent 都看得到

越來越多開發者在 agent 之間交替——這個任務用 Claude Code，下個任務找 Codex 要第二意見。兩邊的模型都強，壞掉的是記憶：每家工具的記憶都是自家孤島，每切換一次，就得從零重講一次專案。

Edda 的 ledger 是工具中立的本地檔案。兩邊的 bridge 讀寫同一個 `.edda/`——在一個 agent 裡做的決策，另一個 agent 開場時就已經在了：

```
Claude Code（早上）                  Codex（下午）
  edda decide "auth=JWT"       →      開場就知道 auth=JWT
          └────────── 同一本本地 ledger (.edda/) ──────────┘
```

同一套接線也涵蓋「一個寫、一個審」的工作流：兩個模型用同一份決策史對質，而不是各抱一本私帳。

<details>
<summary><strong>只用 Claude Code 的話，需要它嗎？</strong></summary>

誠實回答：**不一定需要。** 單人、單工具、一次只開一個 session 的輕量專案，Claude Code 內建的記憶就夠了。

以下任一情況成立時，edda 才開始值回票價：

| 情況 | edda 加了什麼 |
|---|---|
| 決策要連「為什麼」一起留下來 | 結構化帳本贏過散文筆記——理由、日期、範圍，下個 session 自動注入 |
| 同時開多個 session | peers/claims 協調：session 看得見誰在動哪裡，不互踩 |
| 用多個工具（Claude Code + Codex…） | 一本本地帳，兩邊共讀共寫 |
| 在 Claude Code「裡面」切換模型（router 類工具） | 正交不競爭：edda 掛在 hook 層，誰在開車都照記——而切完模型，新模型正是最需要舊模型決策的那個 |
| session 跑在 container 裡 | 每個 container 都是孤島；你要 mount 的那份共享狀態，就是 `.edda/` |

</details>

## 安裝

```bash
# 一行安裝（Linux / macOS）
curl -sSf https://raw.githubusercontent.com/fagemx/edda/main/install.sh | sh

# macOS / Linux（Homebrew）
brew install fagemx/tap/edda

# crates.io
cargo install edda

# 或下載預編譯的二進位檔
# → https://github.com/fagemx/edda/releases
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
   Bridge hooks（確定性，永遠開）
        │  ├── 記錄決策 / 筆記 / peer 訊號
        │  ├── session 開始注入前次 context
        │  └── 可選：注入 havamal doctrine pack
        ▼
   ┌─────────┐
   │  .edda/  │  ← append-only SQLite ledger
   │  ledger  │  ← hash-chained 事件
   └─────────┘
        │
   Session 結束
        │  ├── 確定性摘要（永遠）
        │  └── LLM 摘要 + 模式偵測（可選、預算上限）
        ▼
   下次 session 看到全部
```

Edda 將每個事件以 hash-chained JSON 記錄儲存在本地 SQLite 資料庫中。事件包括決策、筆記、session 摘要和指令輸出。Hash chain 讓歷史記錄防篡改、檢索確定性——同一個查詢永遠得同一個答案，迴圈裡沒有 LLM。

每次 session 開始時，edda 從 ledger 組裝 context snapshot 並注入——agent 看到最近的決策、進行中的任務、peer 協調狀態，以及（若有配置）來自 [havamal](https://github.com/fagemx/havamal) 的判斷層 pack，不需要閱讀舊 transcript。

**LLM 只在這裡用（皆為可選）：** 長 transcript 決策萃取、更豐富的 session 結束摘要、跨 session 模式關聯——分別住在 `bg_extract` / `bg_digest` / `bg_detect`。三者皆需 `EDDA_LLM_API_KEY` 且套用每日預算；沒 key 時 edda 降級為確定性 heuristic。

## 比較

|  | MEMORY.md | RAG / 向量資料庫 | LLM 摘要 | **Edda** |
|--|-----------|-----------------|---------|----------|
| **儲存** | Markdown 檔案 | 向量 embeddings | LLM 生成的文字 | Append-only SQLite |
| **檢索** | Agent 讀取整個檔案 | 語意相似度 | LLM 重新摘要 | Tantivy 全文搜尋 + 結構化查詢 |
| **需要 LLM？** | 否 | 是（embeddings） | 是（每次讀寫） | **核心不用；摘要可選** ¹ |
| **需要向量資料庫？** | 否 | 是 | 否 | **否** |
| **防篡改？** | 否 | 否 | 否 | **是**（hash chain） |
| **追蹤「為什麼」？** | 偶爾 | 否 | 有損 | **是**（理由 + 被拒絕的方案） |
| **跨 Session？** | 手動複製 | 是 | Session 範圍內 | **是**（自動） |
| **跨 Agent？** | 否——單一工具的檔案 | 每個 app 各自整合 | 否——vendor 孤島 | **是**（Claude Code、Codex、OpenClaw、MCP） |
| **每次查詢成本** | 免費 | Embedding API 呼叫 | LLM API 呼叫 | **免費**（本地 SQLite）；可選 LLM 摘要有預算上限 |
| **範例** | Claude Code 內建、OpenClaw | mem0、Zep、Chroma | ChatGPT Memory、Copilot | — |

每次 ledger 查詢都在本地 SQLite 上執行 — 每次都得到相同答案，毫秒級，零成本。

¹ *LLM 增強預設關閉。設 `EDDA_LLM_API_KEY` 啟用：session 結束摘要、長 transcript 決策萃取、跨 session 模式關聯，每個呼叫皆套每日預算上限。核心迴圈——記錄決策、hash chain、檢索、hook 注入——永不呼叫 LLM。*

## 整合

**Claude Code** — 透過 bridge hooks 完整支援。自動捕捉決策、消化 session、注入 context。

```bash
edda init    # 偵測 Claude Code，自動安裝 hooks
```

**Cursor** — 透過原生 Cursor hooks 支援。Session 開始時會把既有 hot pack、doctrine 與 workspace context 推送進 Agent 模型。

```bash
edda bridge cursor install      # 安裝 ~/.cursor/hooks.json 條目
edda doctor cursor              # 驗證 PATH、hooks 與 store 可寫性
```

Cursor v1 與 Codex bridge 共用相同的讀取路徑。Cursor 在 `sessionStart` 可能送出 `transcript_path: null`，因此 bridge 會讀取既有 hot pack，不會宣稱在該時點重建 Cursor transcript。

**Codex** — 透過原生 hooks 支援，並共用 Edda 的 context 機制。

```bash
edda bridge codex install
```

**OpenClaw** — 透過 bridge 插件支援。

```bash
edda bridge openclaw install    # 安裝全域插件
```

**Havamal**（判斷層）— 在 repo 放一個 `.havamal-pack.md`，edda 會在 session 開始自動注入為 doctrine 段。見 [havamal](https://github.com/fagemx/havamal)——事實走 edda，判斷簽核進場。

<details>
<summary><strong>一定要一起用嗎？</strong></summary>

短答：**不用——edda 自己就有用**。兩個都在時會自動接上,但誰也不依賴誰。

| 你的痛 | 用 |
|---|---|
| 「上次 session 做的決策，開新 session 就消失。」 | **只用 edda** |
| 「agent 不知道我這個專案在乎什麼、拒絕什麼、試過什麼。」 | **只用 havamal**（寫 doctrine，在 `CLAUDE.md` / `AGENTS.md` 裡引用） |
| 兩個都有，尤其是長專案跨很多 session | **兩個都用**——edda 自動注入 havamal pack，跳過「先讀 doctrine」的手動步驟 |

Havamal 因為契約是純 markdown 檔，可獨立配任何 harness（Claude Code、Codex、Cursor、Gemini CLI）。edda 也獨立可用——記錄決策和注入功能不需要 doctrine 存在也能運作。
</details>

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

16 個 Rust crates：

| Crate | 功能 |
|-------|------|
| `edda-core` | 事件模型、hash chain、schema、provenance |
| `edda-ledger` | Append-only ledger（SQLite）、blob store、locking |
| `edda-cli` | 所有指令 + TUI（`tui` feature，預設開啟） |
| `edda-bridge-claude` | Claude Code hooks、transcript 攝取、context 注入 |
| `edda-bridge-cursor` | Cursor 原生 hooks、context 注入、生命週期追蹤 |
| `edda-bridge-codex` | Codex hooks 與 context 注入 |
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

*別再重教 agent 你已經決定過的事。*
