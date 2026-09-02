# PR 審查 watcher：一開 PR 就有審查（#632）

> 這支 watcher 回答一個問題：**PR 一開就有人審，不用控制者接力。**
> 它每 60 秒掃一次 open PR，對沒審過的 head 自動起唯讀審查者（gpt-5.6-sol），
> 把 SHA-pinned 判決貼上 PR、加 label。它**不合併**——合併仍照 `pr.merge-policy`
> 由操作者授權。

## 它做什麼

1. `gh pr list --state open` 對每張**非 draft**、head SHA 沒審過（狀態檔比對）的 PR，
   呼叫 `scripts/review-pr.sh`：
   - 從 PR body 的 `Issue: #N` 行抽出 issue，取其 `## doneWhen`；
   - 從 PR changed files 推導 allowed surface；
   - 產生「驗證清單」框架的審查 brief（決策 `fleet.review-brief-framing`：
     契約項目＋零裁量指令檢查＋severity 規則＋固定的 `<<<VERDICT ... VERDICT>>>` 輸出形狀）；
   - 在 `$EDDA_FLEET_SCRATCH/wt-review-prN` 建立/更新 PR head 的 detached worktree；
   - Windows 上照決策 `fleet.lane-launch` 用 Task Scheduler 起隱藏排程任務
     `edda-review-prN-rR`（wrapper 設 `HOME`、UTF-8；父程序是 svchost，不受
     session 的 Job Object 牽連）；Linux 上退化成 `nohup`。
2. 判決出現後（log 裡 `<<<VERDICT ... VERDICT>>>` 區塊），貼 PR 留言：表頭一行含
   **reviewer model（從 pi session 檔觀察，不是從 brief 宣稱）**、cost（session 檔加總）、
   釘死的 head SHA；然後加 label `review:lgtm` 或 `review:changes-requested`。
3. head 被 push 之後（SHA 變了）自動再審一輪（round+1，delta brief，`prev-sha` 帶入）。

## Provider 過載怎麼辦（決策 `fleet.review-provider-overload`：改運輸，不降模型）

判決死掉或空白（log 沒有 verdict 區塊、zero-byte log、或超過 45 分鐘沒有 `.done`）時：

1. **pi 重試一次**（同一個 `--model`，session 續用）；
2. 還是死的話改運輸：`edda dispatch --agent codex`（同一個訂閱的 gpt-5.6-sol，走另一條路）；
3. 兩條都拿不到判決：label `review:unreviewed`，**該 head 停手**。
   未審查是誠實的狀態，便宜模型的判決不是。

`review:unreviewed` 只擋它記錄的那個 head；之後 push 新 head，watcher 會自動再審
（先摘掉 label）。

## 啟動 / 停止

```powershell
# 註冊成隱藏排程任務 edda-pr-review-watcher，登入時自動重啟，並立刻啟動
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\pr-review-launch.ps1

# 停止並解除註冊
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts\pr-review-launch.ps1 -Stop
```

參數（都可省略，預設值見腳本）：`-RepoRoot`（repo 主 checkout）、`-Scratch`
（預設 `$env:USERPROFILE\.edda\fleet`）、`-BashPath`（Git Bash 的 bash.exe）、
`-Repo`（預設 `fagemx/edda`）、`-Model`（預設 `openai-codex/gpt-5.6-sol`）。

不註冊、手動跑（或試跑）：

```bash
scripts/pr-review-watch.sh                 # 前景無限輪詢
scripts/pr-review-watch.sh --once          # 只跑一輪
scripts/pr-review-watch.sh --once --dry-run   # 只印會做什麼，不動 GitHub、不發審查
scripts/review-pr.sh 625 --dry-run         # 只產 brief 並印出會註冊什麼
```

## 環境變數

| 變數 | 預設 | 意義 |
|---|---|---|
| `EDDA_REPO` | `fagemx/edda` | owner/repo |
| `EDDA_FLEET_ROOT` | 自 git 推導（主 checkout） | 建立 detached worktree 用的主 repo |
| `EDDA_FLEET_SCRATCH` | `$HOME/.edda/fleet` | 狀態檔、brief、log、worktree 的目錄（**不進 git**） |
| `EDDA_REVIEW_MODEL` | `openai-codex/gpt-5.6-sol` | 審查模型（決策 `fleet.agent-model-split`：審查一律訂閱的 gpt-5.6-sol） |
| `EDDA_REVIEW_POLL_SECONDS` | `60` | 輪詢間隔 |

## 狀態檔（`$EDDA_FLEET_SCRATCH` 下，不進 git）

| 檔案 | 內容 |
|---|---|
| `review-state.tsv` | `pr<TAB>reviewed_sha<TAB>round`——每張 PR 已審到哪個 head |
| `review-pending.tsv` | `pr<TAB>round<TAB>sha<TAB>attempts<TAB>launched`——審查在途帳本（attempts 0=首次 pi，1=pi 重試，2=codex 運輸） |
| `watch.log` | watcher 每一步的時間戳紀錄 |
| `review-prN-rR-brief.md` / `.log` / `.done` / `-verdict.md` / `-comment.md` | 每輪審查的 brief、pi 轉錄、結束旗標、抽出來的判決、貼出的留言 |
| `wt-review-prN/` | 該 PR 的 detached worktree |

## Labels（watcher 啟動時自動建立，缺了才建）

| Label | 意義 |
|---|---|
| `review:lgtm` | 判決 LGTM（P0=0, P1=0） |
| `review:changes-requested` | 判決 Changes Requested |
| `review:unreviewed` | pi＋codex 兩條運輸都拿不到判決；該 head 停手 |

Label 只在「留言當下 PR head 仍等於被審的 SHA」時才加——head 已經跑走的話，
判決照貼（它釘死在被審的 SHA），但 label 留給下一輪。

## 它不做什麼

- **不合併**。合併永遠照 `pr.merge-policy`（final current-head LGTM、P0=0/P1=0、
  7 格 CI 綠、SHA 窗檢查）由操作者授權後執行。
- 不 push、不開 issue、不回留言、不動 `.github/workflows/**`。
- 不審 draft PR；fork PR 的 head 拿不到 worktree 時會在 watch.log 留下失敗紀錄。

## Troubleshooting

| 症狀 | 查什麼 |
|---|---|
| 懷疑 watcher 沒在跑 | `Get-ScheduledTask edda-pr-review-watcher`（State 應為 Running）；`Get-ScheduledTaskInfo edda-pr-review-watcher`（LastTaskResult、LastRunTime） |
| PR 開了 5 分鐘沒審 | `tail $EDDA_FLEET_SCRATCH/watch.log`——看是沒掃到（draft？label `review:unreviewed`？head 等於 reviewed_sha？）還是 review-pr.sh 失敗（其輸出也接進同一個 log） |
| 想看審查者實際在幹嘛 | `$EDDA_FLEET_SCRATCH/review-prN-rR.log`（pi 轉錄全文）；pi session 檔在 `~/.pi/agent/sessions/*/…_review-prN.jsonl`（model_observed 與 cost 的來源） |
| 任務卡住不結束 | pi 的排程任務有 30 分鐘上限；watcher 45 分鐘沒看到 `.done` 會自動當死掉的判決，走過載改道鏈。手動救：`Get-ScheduledTask edda-review-prN-rR`、`Stop-ScheduledTask`、刪掉該輪 `.done` 後重跑 `scripts/review-pr.sh N R` |
| 想重審某個 head | 改 `review-state.tsv` 裡該 PR 的 `reviewed_sha`（或刪掉該行），下一輪就會重審 |
| `review:unreviewed` 之後想手動審 | 判決手動貼上 PR 後，把 `review-state.tsv` 補成該 head 並摘 label，watcher 就不會再起一輪 |
