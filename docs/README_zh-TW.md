<h1 align="center">Edda</h1>

<p align="center">
  <strong>你的 agent 做的工作，不該跟著 session 一起消失。</strong><br/>
  Edda 是給 coding agent 的本地、防篡改帳本：<br/>
  決策跨 session 活著，協調跨 agent 活著。<br/>
  支援 Claude Code、Cursor、Codex、OpenClaw 和任何 MCP 客戶端。
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
  <a href="#第一層記憶跨-session-活著">記憶</a> ·
  <a href="#第二層協調跨-agent-活著">艦隊</a> ·
  <a href="#安裝">安裝</a> ·
  <a href="#快速開始">快速開始</a> ·
  <a href="#運作原理">運作原理</a> ·
  <a href="#比較">比較</a> ·
  <a href="#整合">整合</a> ·
  <a href="#架構">架構</a>
</p>

<p align="center">
  <a href="../README.md">English</a> · 繁體中文
</p>

<p align="center">
  <img src="https://github.com/user-attachments/assets/03180d1f-5943-4a62-808b-0b8d159a94db" width="700" alt="Edda 概覽" />
</p>

---

## 為什麼需要 Edda？

Agent 的工作有兩種消失法。

**Session 死了，決策跟著死。** 昨天你和 agent 把利弊吵完，定案用 SQLite。今天開新 session——它又提議 Postgres。又來一次。推理跟著 transcript 一起消失了，context 壓縮救不回來。

**Agent 死了，工作狀態跟著死。** 你同時跑兩三個 agent，其中一個 session 半路掛掉。它剛剛在做什麼？做完了哪些？有沒有做到一半的？如果答案只存在一個已經不存在的程序裡，你就得手工重建現場——或更糟，重做已經做完的工。

Edda 用同一個原語治這兩種病：**一本 append-only、hash-chained 的帳本，放在 `.edda/`、在你自己的機器上，比任何 session、任何 agent、任何工具都活得久。** 一本帳，兩層應用：

| 層 | 回答的問題 | 原語 |
|---|---|---|
| **記憶** | *決定了什麼、為什麼？* | 決策、筆記、session 摘要、自動注入 |
| **艦隊** | *誰在做什麼、實際發生了什麼？* | claims、任務＋receipt、計畫、gate、verdict |

第一層從第一天就能單獨用。第二層不用另外裝——同一本帳、同一個 CLI——當你開始跑多個 agent，它就在那裡。

## 第一層——記憶跨 session 活著

Hooks 看著你的 session，把每個決策連同理由記進帳本，在下一個 session 開始前交到它手上。agent 從此不再失憶。

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

### 一份記憶，每個 agent 都看得到

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

## 第二層——協調跨 agent 活著

跑第二個 agent 的那一刻起，記憶就不再是最難的問題。難的變成：*誰在動哪裡、誰用什麼權限決定了什麼、你沒在看的時候實際發生了什麼。* Edda 用同一本帳回答這三題。

**Claims——誰在動哪裡。** 每個 session 宣告自己的工作範圍；所有 peer 都會在 context 裡看到，而且 guard 會在 agent 要動到別人 claim 的路徑之前警告。併行 session 不再互踩。

```bash
edda claim "auth-refactor" --paths "src/auth/*"
```

**任務帶 receipt——實際發生了什麼。** 工作在 task rail 上交接，任務要附證據才算完成。沒有 receipt 的 `done` 不存在。

```bash
edda task new "run integration tests" --assignee tester
edda task done 13 --receipt "110/601 green, artifact in dist/"
```

**計畫帶 gate——會停下來等判斷的多階段工作。** `edda conduct` 逐階段執行 YAML 計畫：每個 phase 派一個 agent、用 check 驗證產出，並且可以在任何不可逆或昂貴的動作前停在 **verdict gate**。gate 釘住它核准的那個 git SHA——新的 push 依構造就讓舊 verdict 失效。

```bash
edda conduct run plan.yaml         # 執行多階段計畫
edda conduct status                # 每個計畫現在跑到哪
edda verdict approve my-plan/impl --sha <完整40位SHA>
edda verdict reject  my-plan/impl --sha <sha> --comment "tests missing"
```

**脫離的 lane——比啟動者活得久的工作。** `edda dispatch` 把單一 agent 回合（Claude、Codex 或 pi）跑成自足的工作單元，可以脫離啟動它的 session。lane 不依賴派它出去的那個 session：當 controller session 半路死掉——我們自己一天死了三次——派出去的工作照跑，事後從 branch、PR 和帳本把結果撿回來，而不是靠任何人的記憶。

```bash
edda dispatch --agent codex --prompt-file task.md
```

**兩層權限——記錄不等於生效。** Agent 可以自由記錄決策，但記錄下來的決策在操作者用 `edda ratify` 追認之前都是 *unratified*。你的艦隊可以整天提案；只有你能把它變成政策。加上 hash chain，這就是整層存在的目的：**agent 跑在你的機器上，而你永遠查得到它做過什麼——按順序、防篡改、附權限軌跡。**

> **成熟度說明：** 第一層是穩定的日用等級。第二層今天就能用（claims、task、conduct、dispatch、verdict gate、ratify），也是 edda 成長最快的地方——進行中的 liveness 心跳、統一艦隊狀態面、事件推播都追蹤在 [#560](https://github.com/fagemx/edda/issues/560)。它是用出來的：edda 自己的多 agent 艦隊就用它蓋 edda。

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
        │  ├── 記錄決策 / 筆記 / claims / peer 訊號
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

Edda 將每個事件以 hash-chained JSON 記錄儲存在本地 SQLite 資料庫中。事件包括決策、筆記、session 摘要、任務、claims、verdicts 和指令輸出。Hash chain 讓歷史記錄防篡改、檢索確定性——同一個查詢永遠得同一個答案，迴圈裡沒有 LLM。

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
| **多 agent 協調？** | 否 | 否 | 否 | **是**（claims、任務＋receipt、gates） |
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
edda log --type decision   # 篩選事件日誌
edda watch                 # 即時 TUI：peers、事件、決策
```

<details>
<summary>所有指令</summary>

**記憶與檢索**

| 指令 | 說明 |
|------|------|
| `edda init` | 初始化 `.edda/`（偵測到 `.claude/` 時自動安裝 hooks） |
| `edda decide` | 記錄決策（agent 記錄；`edda ratify` 後才生效） |
| `edda note` | 記錄筆記 |
| `edda ask` | 查詢決策、歷史和對話 |
| `edda search` | 全文搜尋 transcripts（Tantivy） |
| `edda log` | 用篩選條件查詢事件（類型、日期、標籤、分支） |
| `edda context` | 輸出 context snapshot（agent 看到的內容） |
| `edda status` | 顯示 workspace 狀態 |
| `edda run` | 執行指令並記錄輸出 |
| `edda commit` | 建立 commit 事件 |

**艦隊與編排**

| 指令 | 說明 |
|------|------|
| `edda claim` | 宣告工作範圍，peers 不互踩 |
| `edda task` | Task rail：建立、交接、追蹤任務（`new/start/done/fail/list/show`） |
| `edda ratify` | 操作者追認決策（記錄 ≠ 生效） |
| `edda conduct` | 多階段計畫編排，含 check 與 gate |
| `edda dispatch` | 單回合 agent 派工（claude / codex / pi） |
| `edda verdict` | 核准／駁回被 gate 的對象，釘住 git SHA |
| `edda watch` | 即時 TUI：peers、事件、決策 |
| `edda draft` | 提案 / 列表 / 批准 / 拒絕 drafts |

**Workspace 與底層**

| 指令 | 說明 |
|------|------|
| `edda bridge` | 安裝/移除工具 hooks |
| `edda doctor` | 健康檢查 |
| `edda config` | 讀寫 workspace 設定 |
| `edda pattern` | 管理分類模式 |
| `edda mcp` | 啟動 MCP server（stdio JSON-RPC 2.0） |
| `edda plan` | 計畫鷹架和範本 |
| `edda branch` / `edda switch` / `edda merge` | Ledger 分支操作 |
| `edda blob` | 管理 blob metadata |
| `edda gc` | 垃圾回收過期內容 |

</details>

## 架構

Cargo workspace，一個 crate 一個器官：

| Crate | 功能 |
|-------|------|
| `edda-core` | 事件模型、hash chain、schema、provenance |
| `edda-ledger` | Append-only ledger（SQLite）、blob store、locking |
| `edda-cli` | 所有指令 + TUI（`tui` feature，預設開啟） |
| `edda-bridge-claude` | Claude Code hooks、transcript 攝取、context 注入 |
| `edda-bridge-cursor` | Cursor 原生 hooks、context 注入、生命週期追蹤 |
| `edda-bridge-codex` | Codex hooks 與 context 注入 |
| `edda-bridge-openclaw` | OpenClaw hooks 和插件 |
| `edda-bridge-hermes` | Hermes agent shell hooks 與 context 注入 |
| `edda-mcp` | MCP server（7 個工具） |
| `edda-serve` | Workspace 的 HTTP API server |
| `edda-ask` | 跨來源決策查詢引擎 |
| `edda-aggregate` | 跨 repo 聚合查詢與 rollup 統計 |
| `edda-derive` | View 重建、分層歷史 |
| `edda-chronicle` | Chronicle 綜合引擎——回顧與認知變焦 |
| `edda-pack` | Context 生成、預算控制 |
| `edda-transcript` | Transcript delta 攝取、分類 |
| `edda-ingestion` | 攝取觸發評估引擎 |
| `edda-store` | 每用戶 store、原子寫入 |
| `edda-search-fts` | 全文搜尋（Tantivy） |
| `edda-index` | Transcript 索引 |
| `edda-postmortem` | L3 事後分析、learned rules（TTL 衰減） |
| `edda-notify` | Workspace 事件推播 |
| `edda-conductor` | 多階段計畫編排——只管自域 phase pipeline；任務派發歸 [bryti](https://github.com/fagemx/bryti)，conductor 永不碰外部工作佇列 |

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

已出貨：

- [x] 發行面——預編譯二進位檔（macOS、Linux、Windows）、一行安裝腳本、Homebrew tap
- [x] v0.2.0——`edda watch` TUI、`edda ask`、peers／協調指令、sub-agent 可見性、session hooks 記錄 model／token／成本、使用者層 store（`~/.edda/`）、post-mortem learned rules
- [x] Decision deepening——`--paths` 範圍決策、PreToolUse 守護警告、session 開始注入決策 pack、決策狀態生命週期
- [x] 艦隊原語——`edda conduct` 多階段計畫、`edda dispatch` 單回合 lane、verdict gate 釘 SHA、task rail 帶 receipt、`edda ratify` 兩層權限

接下來——把艦隊智慧層折進產品（[#560](https://github.com/fagemx/edda/issues/560)）：

- [ ] Surface-aware 排程——phase 宣告自己擁有的路徑；claim 帶著路徑；重疊的派工被結構性拒絕
- [ ] 事件驅動交付——phase 終態與 gate 進入透過 `edda-notify` 推播，不再輪詢 stdout
- [ ] 艦隊可觀察性——進行中心跳，以及同時涵蓋 `conduct` 計畫與 `dispatch` lane 的單一狀態面
- [ ] 跨 repo 決策查詢面——使用者層 store 已跨專案聚合；缺的是第一級的跨 repo search／ask 介面
- [ ] 決策回憶指標——量測注入的決策實際改變行為的頻率

## 貢獻

歡迎貢獻。請參閱 [CONTRIBUTING.md](../CONTRIBUTING.md) 了解開發環境設定。

## 社群

- [GitHub Issues](https://github.com/fagemx/edda/issues) — 回報 bug 和功能請求
- [Releases](https://github.com/fagemx/edda/releases) — 更新日誌和二進位檔

## 授權

MIT OR Apache-2.0

---

*別再重教 agent 你已經決定過的事——也別再只憑艦隊的一面之詞相信它做了什麼。*
