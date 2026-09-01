---
name: fleet-triad
description: Use when coordinating engineering work across mixed agent runtimes on this machine — pi terminal sessions as concurrent workers, a separate Claude session as independent verifier, this session as controller — or when you can only reach a target agent via terminal paste, Run-button relay, or cross-session SendMessage (dispatch, monitor, freeze, verdict, merge).
---

# Fleet Triad（controller-Claude ／ verifier-Claude ／ workers-pi）

## Overview

fleet-orchestrate 的**傳輸綁定層**：契約（ledger-first、SHA 綁定 verdict、receipt≠驗收、worktree 隔離）不變，本 skill 只補「異質 runtime 怎麼實際接上」與本機環境的實戰坑。
**REQUIRED BACKGROUND:** fleet-orchestrate（controller 契約與 playbook 全文適用）。

## When to Use
- 執行工作要派給 pi（或其他 TUI agent），驗收要派給另一個 Claude session，自己當 controller
- 目標代理只能靠終端貼字、使用者轉貼、或 SendMessage 觸及
- 不適用：單 writer 純本 session 工作（直接用 superpowers 執行鏈）

## 角色 × 傳輸

| 角色 | 傳輸 | 回程 |
|---|---|---|
| worker=pi（可多個，各自 worktree+分支） | 自足 brief **檔案入 repo** → 終端貼一段指標式 kickoff | 終端輸出 REPORT/FREEZE/BLOCKED；**使用者轉貼當門鈴**，controller 用 read_terminal 按需查看（不開背景輪詢） |
| verifier=獨立 Claude session | SendMessage：brief 全文＋`notify_when_idle:true` | verdict **檔案 commit** ＋ 回訊一行；idle 通知只是備援 |
| controller=本 session | git/board 寫入、裁定 d-NNN | — |

## Dispatch 配方

**pi kickoff（貼進 TUI 的那段）必含**：身分代號、先讀哪些檔（brief/board/plan 路徑）、第一個 git 動作（含「分支已存在則續作，從 checkbox 與 git log 判斷進度」的重啟韌性句）、回報格式、freeze 點、「現在開始」。長內容一律放 repo 檔案，kickoff 只放指標。
**貼字機制**：先查 pi 的 CLI 有無 headless/rpc（`pi --help`：`-p`/`--session-id`/`--mode rpc`）再考慮 UI automation；UI 路徑＝剪貼簿→點輸入框→**ctrl+shift+v**（xterm 系終端 ctrl+v 無效）→ read_terminal 確認上屏才送出。
**Run 按鈕通道**：controller 訊息裡的 `bash` fence 會有 Run 鈕，按下＝把文字打進**作用中**終端分頁——是給使用者轉貼 kickoff 的捷徑，也是誤擊風險（merge 指令變成 pi 訊息）；給使用者的 Run 塊旁必附「先確認目標分頁作用中」。

**verifier 派工訊息必含**：凍結 **full SHA**＋delta 起點 SHA、審查面（IN SCOPE）、領地表與 doneWhen 的檔案指標、verdict 檔路徑（verifier 唯一可寫檔）、「裸跑禁管線」、回訊格式一行。首次委派另附契約：read-only、不修所審之物、任何 push 使 verdict 失效、結案與 merge 權在 controller。

## Freeze 視窗紀律
worker 回報 FREEZE 起至 verdict 出爐止，**controller 不得 commit 該候選分支**（board 更新持有到 verdict 後）。違反＝SHA 漂移，verifier 得重綁或重跑。

## Common Mistakes

| 症狀 | 真相／修法 |
|---|---|
| idle 通知來了就當完工 | idle≠done，且 **idle=對方上一件事完成、非佇列清空**（佇列派工在對方回合邊界才 drain，「收到、現在開始」可能只是意圖宣告）。完工訊號一律=約定產物 commit（verdict 檔）；先查真相層，空手才發狀態詢問並重掛訂閱 |
| 對方沒回訊＝沒在做 | 跨 session 回訊可能因權限層被扣住只記錄不送達——所以要**雙通道**：檔案 commit 為主、回訊為門鈴、idle 訂閱為備援 |
| 背景每 N 分鐘輪詢 worker | 反模式。門鈴=使用者轉貼 FREEZE/BLOCKED；需要時才 read_terminal |
| gate 大面積紅→怪代碼 | 先查環境：Docker 停了？restart 政策的容器搶埠？共享測試庫被並行 suite 互踩（advisory lock 排隊≠掛掉）？ |
| 驗收鏈用 `\| tail` 組管線 | pipeline 回傳碼取自最後一段，`&&` 把關全廢且吞掉失敗輸出。驗收指令一律裸跑 |
| 把含反引號的文檔文字嵌進 bash 雙引號字串 | command substitution 會把 markdown 當 script 執行——曾真實誤切分支、產生重導向垃圾檔、觸發文檔內 gate 指令（兩次事故：一次吞 SHA、一次執行檔案）。**board/文檔寫入一律 Edit/Write 工具**；bash 字串只放路徑與旗標，永不放內容 |
| 測試綠了就信護欄 | 要求敏感度對照：刻意破壞被測物，測試必須變紅，否則是恆真假護欄 |
| 進 repo 不看當前分支就 commit | 任何 repo 先 `git branch --show-current`——目標 repo 可能 checkout 在別人的活分支上（本病曾在兩個 repo 各犯一次） |
| 兩 worker 共用 worktree | 一個 worktree 一個寫手；`git worktree add` 分開，且建完 worktree 後若又 commit 了 brief，記得 ff 各分支讓樹裡看得到 |

## Example：verifier 派工骨架

```text
V1 round N 開跑：凍結 full SHA <sha>（delta 起點 <base-sha>）。
delta 摘要：<一句>。驗收：(1) <唯一/主要標準> (2) gate 裸跑 (3) 出界檢查照 <brief 路徑> §領地。
產出：<verdict 檔路徑> 追加 Round N 節（綁 SHA、RAN/READ、P0/P1、verdict）→ commit →
SendMessage 回 <controller session 名> 一行「<代號> verdict: <LGTM|Changes Requested> @ <sha>」。
```
