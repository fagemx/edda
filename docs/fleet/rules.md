# Fleet 規則

管理者代理每次醒來讀這份；操作者改這份。設計稿：`docs/superpowers/specs/2026-09-02-fleet-manager-agent-design.md`。

- 版本：2026-09-02
- 優先序：帳本已 ratify 決策 > 本檔「操作者規則」 > 本檔「管理者自訂」
- 改本檔走 PR；緊急時操作者可直接 push

## 操作者規則

- **R1 認領**：派工前在 issue 留 `taking: <machine>/<role>` 並貼 `lane:<machine>`；先寫先贏；讀到別台的 taking 就不派。來源：`fleet.cross-machine-claim`。
- **R2 重複產物**：同一 issue 有兩份產物，留 doneWhen 覆蓋較完整的那份；另一份關閉，可用部分搬過去。不因流程錯誤丟掉真工作。來源：#613 裁定、PR #670。
- **R3 停止的定義**：排程任務非 Running、process tree 無活程序、lane log 有 `=== EXIT ===`，三者齊備才算停；只殺 wrapper 不算。來源：#672。
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

## 管理者自訂

（空。管理者依 R8 追加，每條附日期、案例、理由。）
