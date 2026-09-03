# Fleet 規則

管理者代理每次醒來讀這份；操作者改這份。設計稿：`docs/superpowers/specs/2026-09-02-fleet-manager-agent-design.md`。

- 版本：2026-09-03
- 優先序：帳本已 ratify 決策 > 本檔「操作者規則」 > 本檔「管理者自訂」
- 改本檔走 PR；緊急時操作者可直接 push

## 操作者規則

- **R1 認領**：派工前在 issue 留 `taking: <machine>/<role>` 並貼 `lane:<machine>`；先寫先贏；讀到別台的 taking 就不派。來源：`fleet.cross-machine-claim`。
- **R2 重複產物**：同一 issue 有兩份產物，留 doneWhen 覆蓋較完整的那份；另一份關閉，可用部分搬過去。不因流程錯誤丟掉真工作。來源：#613 裁定、PR #670。
- **R3 停止的定義**：三證齊備才算停 —— 排程任務非 `Running`、**沒有 `edda.exe` 仍持有該 lane 的 briefs 路徑**、lane log 有 `=== EXIT ===`。`Ready` 單獨不代表停（#650 被宣告交還時 lane 仍在寫）。只殺 wrapper 不算停。來源：#672、`fleet.lane-stop-4090`。
- **R4 lane 類型**：diff 需要 cargo（Rust 原始碼、測試、fixture、`Cargo.*`，或 CI 會跑 cargo）就走 4090 build lane，否則走文書機。不看 issue 標題前綴。來源：#651。
- **R5 檔案撞車**：同一檔多方要改，改動最多且最接近推送者先做完，其餘排後 rebase；小修法以留言交持有者併入。來源：`operator-runbook.md` 三方裁示。
- **R6 合併**：LGTM 釘的 SHA 等於 current head、`CI Gate` 綠、**該 head 的 `Independent Review` status 為 success**、SHA 窗為空、P0 與 P1 都是零，就 squash 合併、不刪分支。條件全是機械的，任何控制者都可執行（冪等）；ruleset 沒有 bypass，admin 帳號也一樣被擋——所有代理都以同一帳號行動，bypass 等於沒有閘（#758 就是這樣在沒有 status 的情況下合進去的）。來源：`review.auto-merge`、`review.verdict-carrier`、`ci.merge-gate`。
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

- **R18 獨立審查 status**：`Independent Review` commit status 是判決的機器形式。success 只在 `LGTM (P0=0, P1=0)` 且同一 SHA 沒有其他非 LGTM 判決（union 規則）；failure 是其他判決；error 是沒有判決。只由**非作者路徑**貼（控制者的 review-queue、之後的 watcher）；作者 session 轉貼的判決不翻 status。push 產生新 SHA、status 消失，判決自動作廢。`pr-review-loop` 之類的 self-check 不是判決。來源：`review.verdict-carrier`、#742、#743。
- **R19 派審先佔**：派審查 lane 之前先在 head 貼 `Independent Review = pending`，description 寫 `claimant=<machine>` 與 nonce；貼完重讀，最新的 pending 不是自己的就退出。head 已有合格的非 self 判決就只同步 status、不派 lane；head 已有 Changes Requested 就貼 failure 等 push。別台開的 PR，head 60 分鐘內有 push 就不碰。檢查與 launch 在同一個工具步驟裡，不在控制者的記憶裡。來源：`review.dispatch-claim`、#754 兩台同時審的撞車。
- **R20 L1 歸屬**：fix lane 只跑 L0（touched crates）、push、貼 `Review Response`，不跑 workspace 閘。L1 = exact-head CI（Linux/macOS 全 workspace ＋ Windows 7 crate 子集）＋ verifier 對 Windows 子集外的 touched crate 各做一次 `cargo test -p`，每個凍結 SHA 一次。來源：`verification.l1-owner`、#748（三條 lane 死在 L1 上）、#753。

## 管理者自訂

（空。管理者依 R8 追加，每條附日期、案例、理由。）
