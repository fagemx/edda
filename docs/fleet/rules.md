# Fleet 規則

管理者代理每次醒來讀這份；操作者改這份。設計稿：`docs/superpowers/specs/2026-09-02-fleet-manager-agent-design.md`。

- 版本：2026-09-02
- 優先序：帳本已 ratify 決策 > 本檔「操作者規則」 > 本檔「管理者自訂」
- 改本檔走 PR；緊急時操作者可直接 push

## 操作者規則

- **R1 認領**：派工前在 issue 留 `taking: <machine>/<role>` 並貼 `lane:<machine>`；先寫先贏；讀到別台的 taking 就不派。來源：`fleet.cross-machine-claim`。
- **R2 重複產物**：同一 issue 有兩份產物，留 doneWhen 覆蓋較完整的那份；另一份關閉，可用部分搬過去。不因流程錯誤丟掉真工作。來源：#613 裁定、PR #670。
- **R3 停止的定義**：三證齊備才算停 —— 排程任務非 `Running`、**沒有 `edda.exe` 仍持有該 lane 的 briefs 路徑**、lane log 有 `=== EXIT ===`。`Ready` 單獨不代表停（#650 被宣告交還時 lane 仍在寫）；任務正常結束也會留孤兒（#672）。只殺 wrapper 不算停。**驗收只認複驗計數**：`Stop-ScheduledTask`／`taskkill` 的 exit code 兩個方向都不是證據（回報成功時子程序可能還在；回報失敗時目標可能已死）。來源：#672、`fleet.lane-stop-4090`。
- **R4 lane 類型**：diff 需要 cargo（Rust 原始碼、測試、fixture、`Cargo.*`，或 CI 會跑 cargo）就走 4090 build lane，否則走文書機。不看 issue 標題前綴。來源：#651。
- **R5 檔案撞車**：同一檔多方要改，改動最多且最接近推送者先做完，其餘排後 rebase；小修法以留言交持有者併入。來源：`operator-runbook.md` 三方裁示。
- **R6 合併**：LGTM 釘的 SHA 等於 current head、`CI Gate` 綠、SHA 窗為空、P0 與 P1 都是零，就 squash 合併、不刪分支。來源：`fleet.merge-authority-4090`、`ci.merge-gate`。
- **R7 不可逆**：不刪分支、worktree、來源；不 force push；不超每日預算；不碰認證。遇到就記 `blocked-by-rule` 跳過。
- **R8 無規則**：管理者自訂一條寫進下方「管理者自訂」段，附案例與理由，照做；操作者可改或刪。
- **R9 身分**：所有留言、心跳、認領用 `<machine>/<role>`；不用分支名、session id、顯示名。
- **R10 訊息**：真相先寫在 issue 或 PR，host 訊息只做門鈴；永不向人提問。
- **R11 認證失效**：該運輸的 lane 停派，在 board 記一筆 `needs-operator: relogin <tool> on <machine>`，只記一次；不重試、不換帳號。來源：#593、#669。
- **R12 核准的計畫步驟**：操作者已核准的設計稿步驟直接以 `fleet:ready` 開單。
- **R13 交還**：交還必須附 R3 三證與 `git ls-remote` 結果；交還後 15 分鐘再查一次。來源：#650。
- **R14 每日預算**：管理者自身每日 5 美元；lane 照 brief。
- **R15 認證失敗的機器判準**：一輪 agent 回合若 `Cost: $0.00`，一律當失敗處理，不論 exit code。理由：認證失敗的回合成本必為零，而 `edda dispatch` 目前回 exit 0（#669）。#669 落地後改以 exit code 為準，本條保留為交叉檢查。
- **R16 存活面的已知污染**：`edda peers` 在 `cargo test -p edda-bridge-claude` 執行期間會出現非 UUID 形狀的假 session（測試 fixture 寫進真實 store，#646）。判存活時忽略 session id 不是 UUID 形狀的條目。心跳判活的視窗是 120 秒（`stale_secs()`），所以兩次查詢相隔超過該視窗可能得到不同答案 —— 這是時序，不是 store 分裂。
- **R17 心跳缺席不是死亡判決**：互動 session（Claude Code 等 hook 驅動）的心跳只在 hook 事件時寫，長工具呼叫期間零事件，120 秒後就從 `edda peers` 消失但人還活著；`edda dispatch`／`conduct` 起的 lane 由 runner 每 30 秒刷新（`crates/edda-conductor/src/runner/heartbeat.rs:25`），stale 才等於死。所以心跳缺席只是「去查 process tree」的提示，死亡判決一律走 R3。禁止把 `stale_secs` 調大來掩蓋。來源：#617、#646。
- **R15 備註**：`Cost:` 有兩條互不相交的路徑——dispatch 輸出的 `cost_usd` 是 backend 原樣透出，bridge 的 `estimate_cost` 是定價表（#677）。R15 只看前者；成本欄位缺量測時沿 CLI 的 `cost_line` 顯示 n/a，不偽造 0.0（GH-533）。
- **R18 唯讀審查者的形式**：一律 `--tools "Read,Grep,Glob,Bash(git *),Bash(gh *),Bash(edda *),Bash(sh *)"`。裸 `Bash` 加 `--exclude-tools Edit,Write` **不是**唯讀（canary 實測：`echo x > file` 與 `sh -c` 都寫得進去）。驗收看檔案系統的 canary，不看 agent 自述。binary 或 agent 給不了要求的限制時，lane 拒絕啟動（#702）。來源：`fleet.reviewer-readonly-form`。
- **R19 閘門指令不接 pipe**：`cargo …` `sh …` 等判成敗的指令後面不接 `| tail`／`| head`，pipe 會吞掉退出碼；PowerShell 看 `$LASTEXITCODE`，bash 用 `set -o pipefail`。輸出要截就先存檔再看。來源：2026-09-03 重建 exit 0 假成功。
- **R20 換 binary**：rename-in-place 加重試，不直接 `cargo install` 進正在被使用的路徑；本 session 的 hook 每次 Bash 都會 spawn 短命 `edda.exe` 而鎖住檔案。換完驗證 `edda --version` 與所需旗標，保留 `.bak`。來源：`fleet.binary-install-window`、2026-09-03 重建紀錄。

## 管理者自訂

（空。管理者依 R8 追加，每條附日期、案例、理由。）
