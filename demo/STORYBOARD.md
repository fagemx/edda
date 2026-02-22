# Edda Demo 影片分鏡

> 目標：30 秒 GIF（README/Reddit）+ 90 秒加長版（Twitter/YouTube）
> 核心訊息：「你的 coding agent 做了決策。你的 AI 助手已經知道了。」

---

## 格式

**畫面分割**：左邊終端機（Claude Code），右邊手機（Telegram / OpenClaw）

為什麼這樣分？因為「終端機 + 手機」是兩個完全不同的世界。不是兩個 CLI 並排（無聊），而是你的工作環境 → 你的日常生活。視覺衝擊力最大。

---

## 30 秒 GIF 版（精簡版）

### Frame 1 — 開場（0-3 秒）

```
┌─────────────────────────────────────────────────┐
│                                                 │
│     edda — Decision memory for AI agents        │
│     "Decisions that survive"                    │
│                                                 │
└─────────────────────────────────────────────────┘
```

黑底白字，簡潔。停留 2 秒。

---

### Frame 2 — Claude Code 做決策（3-15 秒）

畫面：終端機全屏

```
~/api-project $ claude

You: Should we use REST or GraphQL for the new API?

Claude: Given your 3-person backend team, I'd recommend REST.
        GraphQL adds schema complexity that's hard to justify
        at this scale. REST with OpenAPI gives you...

You: Agreed. Let's go with REST.
```

游標閃爍，打字動畫。真實感。

然後出現 edda 自動記錄的提示（hook 觸發）：

```
📖 edda: recorded — "Use REST over GraphQL (team size, simplicity)"
```

底部一行小字一閃而過，不搶戲。

---

### Frame 3 — 切換到手機（15-17 秒）

**轉場動畫**：終端機淡出，右邊滑入手機畫面

或者更簡潔：直接 cut 到手機畫面。

上方加一行小字：`Next day...`

---

### Frame 4 — 問 AI 助手（17-28 秒）

畫面：Telegram 對話（手機 UI）

```
You:  昨天 API 的架構決定了什麼？

Nox:  昨天在 Claude Code 裡做了 3 個決策：
      
      📌 API 用 REST（不用 GraphQL）— 團隊小，REST 夠用
      📌 資料庫用 Postgres on Railway
      📌 測試框架用 pytest
      
      要我繼續昨天的進度嗎？
```

打字動畫，Nox 的回覆逐行浮出。

---

### Frame 5 — 結尾（28-30 秒）

```
┌─────────────────────────────────────────────────┐
│                                                 │
│     edda — Decision memory for AI agents        │
│                                                 │
│     cargo install edda                          │
│     github.com/fagemx/edda                      │
│                                                 │
└─────────────────────────────────────────────────┘
```

---

## 90 秒加長版（完整版）

### Act 1 — 問題（0-20 秒）

#### Frame 1.1 — 開場問題（0-5 秒）

黑底白字：

```
You use AI to code.
But every new session, it forgets.
```

#### Frame 1.2 — 痛點演示（5-20 秒）

終端機畫面：

```
~/api-project $ claude

You: Let's work on the API endpoints.

Claude: Sure! Should we use REST or GraphQL?

You: ...we decided this yesterday. REST. 
     Because the team is small.

Claude: Ah right, sorry about that! Let's use REST then...
```

用戶嘆氣的感覺要出來。也許加一個 `(sigh)` 或者一個短暫的停頓。

然後疊加文字：

```
                    ↓
        You explain. Again. And again.
```

---

### Act 2 — 解決方案（20-55 秒）

#### Frame 2.1 — 介紹 Edda（20-25 秒）

```
┌─────────────────────────────────────────────────┐
│                                                 │
│  Meet edda.                                     │
│  Decision memory for AI coding agents.          │
│                                                 │
└─────────────────────────────────────────────────┘
```

#### Frame 2.2 — 正常工作流程（25-45 秒）

終端機畫面（這次有 edda）：

```
~/api-project $ claude

You: Should we use REST or GraphQL for the new API?

Claude: Given your 3-person backend team, I'd recommend REST.
        GraphQL adds schema complexity...

You: Agreed. Let's go with REST.

📖 edda: recorded — "Use REST over GraphQL (team size, simplicity)"
```

繼續工作：

```
You: What database should we use?

Claude: For a small REST API, Postgres is solid.
        Railway makes deployment easy...

You: Good. Postgres on Railway.

📖 edda: recorded — "Postgres on Railway (simple deployment)"
```

節奏：不需要看完整對話，快速剪輯，focus 在決策被記錄的瞬間。

#### Frame 2.3 — edda log（45-50 秒）

```
~/api-project $ edda log

  2026-02-16 14:32  Use REST over GraphQL — team size, simplicity
  2026-02-16 14:45  Postgres on Railway — simple deployment
  2026-02-16 15:10  pytest for testing — team already knows it
  2026-02-16 15:33  Tailwind CSS — faster than custom CSS

4 decisions recorded
```

乾淨的 timeline。視覺上很漂亮。

---

### Act 3 — 魔法時刻（55-80 秒）

#### Frame 3.1 — 轉場（55-58 秒）

文字疊加：`Next morning...`

畫面從終端機過渡到手機。

#### Frame 3.2 — Telegram 對話（58-75 秒）

```
You:  昨天那個 API 專案做到哪了？

Nox:  📖 從 edda 拉了昨天的決策紀錄：
      
      你在 Claude Code 裡做了 4 個架構決策：
      • REST over GraphQL（團隊小）
      • Postgres on Railway（部署簡單）
      • pytest（團隊已熟悉）
      • Tailwind CSS（比手寫快）
      
      昨天停在 API endpoint 的實作，
      /users 和 /projects 完成了，
      /billing 還沒開始。
      
      要繼續嗎？
```

#### Frame 3.3 — 回到終端機（75-80 秒）

```
~/api-project $ claude

Claude: Welcome back. I see from edda that you've decided on
        REST + Postgres + pytest. Last session stopped at
        /billing endpoints. Want to continue there?

You: Yes, let's go.
```

**這是第二個 aha moment**：不只是 OpenClaw 知道，Claude Code 回來也知道。雙向。

---

### Act 4 — 收尾（80-90 秒）

```
┌─────────────────────────────────────────────────┐
│                                                 │
│  edda — Decision memory for AI coding agents    │
│                                                 │
│  Your agents decide.                            │
│  Edda remembers.                                │
│  Across tools. Across sessions. Across time.    │
│                                                 │
│  cargo install edda                             │
│  github.com/fagemx/edda                         │
│                                                 │
└─────────────────────────────────────────────────┘
```

---

## 追加場景：Conductor（`--verbose` 即時輸出）

> 這是 2026-02-16 實測的真實輸出，展示 Conductor 編排多個 agent。

### 場景 D：Conductor 多 agent 編排

tmux 分割畫面，左邊 Conductor，右邊檔案樹。

```
~/api-project $ edda conduct run plan.yaml --verbose

Starting plan "api-decisions" (4 phases)

▶ Phase "scaffold" (attempt 1)
  🔌 Model: claude-opus-4-6
  💬 I'll create the Python REST API project structure...
  🔧 Bash: mkdir -p src tests
  📝 Write: pyproject.toml
  📝 Write: src/main.py
  📝 Write: src/models.py
  📝 Write: tests/test_health.py
  💬 All verification checks pass...
  📊 Result: success ($0.156)
  ✓ Phase "scaffold" passed

▶ Phase "endpoints" (attempt 1)
  🔌 Model: claude-opus-4-6
  📖 Read: src/main.py
  📖 Read: src/models.py
  📝 Write: src/routes.py
  ✏️ Edit: src/main.py
  📊 Result: success ($0.192)
  ✓ Phase "endpoints" passed

▶ Phase "docs" (attempt 1)
  🔌 Model: claude-opus-4-6
  📖 Read: src/main.py, routes.py, models.py, pyproject.toml
  📝 Write: README.md
  📊 Result: success ($0.131)
  ✓ Phase "docs" passed

▶ Phase "tests" (attempt 1)
  🔌 Model: claude-opus-4-6
  📖 Read: src/main.py, routes.py, models.py
  ✏️ Edit: pyproject.toml
  📝 Write: tests/test_api.py
  📊 Result: success ($0.208)
  ✓ Phase "tests" passed

✓ Plan "api-decisions" completed (4 passed)
  Total: $0.687, 4 agents, 0 retries
```

**這裡的賣點**：你看到 4 個獨立 agent 接力工作，每個都讀取前面的成果、繼續建設。Conductor 即時印出每個 agent 在做什麼 — 寫哪個檔案、跑哪個指令、花多少錢。

---

## 製作備註

### 工具建議

| 用途 | 工具 | 備註 |
|------|------|------|
| 終端機錄製 | [asciinema](https://asciinema.org/) | 純文字錄製，可後製速度 |
| 終端機 → GIF | [agg](https://github.com/asciinema/agg) 或 [gifski](https://gif.ski/) | asciinema 轉 GIF |
| 手機畫面 | 截圖 + 動態模板 | 或用真實 Telegram 錄屏 |
| 合成 | ffmpeg 或 ScreenStudio | 左右分割、轉場 |
| 30 秒 GIF | LICEcap (Win) / Kap (Mac) | 直接錄全螢幕 |

### 視覺風格

- **終端機**：Dracula 或 Nord 配色（高對比，好看）
- **字型**：JetBrains Mono 或 Fira Code
- **手機**：真實 Telegram 截圖（不要用 mockup，要真實感）
- **轉場**：簡單 fade 或 cut（不要花哨）
- **文字疊加**：白色無襯線字體，半透明黑底

### 節奏

- 打字速度：比正常快 2-3 倍（人不想等你打字）
- 回覆速度：即時出現或快速打字動畫
- 停頓：只在「aha moment」停 1-2 秒（edda 記錄、Nox 回覆）
- **整體節奏偏快**，寧可看不清要倒回去看，也不要無聊

### 語言

- 終端機內容：**全英文**（目標市場）
- Telegram 對話：**中文**（Nox 本來就用中文）
- 這個中英混合反而加分——展示 Edda 不限語言
- 如果要純英文版，Nox 的回覆改英文就好

### 音樂（90 秒版）

- Lo-fi coding beats 或安靜的電子音樂
- 不要配音（國際觀眾 + 開發者偏好靜音看）
- 或者完全無音樂（更技術感）

---

## 可選追加場景

### 場景 B：Codex 跨工具（如果 MCP 做好了）

```
~/api-project $ codex

You: What architectural decisions have we made so far?

Codex: [calling edda_ask...]

Based on your decision history:
- REST API (decided 2026-02-16)
- Postgres on Railway (decided 2026-02-16)
- pytest for testing (decided 2026-02-16)

These decisions were made during a Claude Code session yesterday.
```

### 場景 C：edda search（CLI power user）

```
$ edda search "database"

  2026-02-16 14:45  Postgres on Railway — simple deployment
  2026-01-20 09:12  Rejected MongoDB — team prefers SQL
  2026-01-15 16:30  Evaluated Supabase — too many abstractions

3 results
```

展示歷史搜尋的力量——所有決策都可以追溯。

---

## 分發策略

| 版本 | 用途 | 格式 |
|------|------|------|
| 30 秒 GIF | GitHub README 頂部 | GIF, < 5MB |
| 30 秒 GIF | Reddit 帖子嵌入 | GIF |
| 90 秒影片 | Twitter launch thread | MP4 |
| 90 秒影片 | YouTube（unlisted） | MP4 |
| 90 秒影片 | HN Show HN 連結 | MP4 |
