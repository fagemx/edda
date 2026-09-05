# Fleet 規則

管理者代理每次醒來讀這份；操作者改這份。設計稿：`docs/superpowers/specs/2026-09-02-fleet-manager-agent-design.md`。

- 版本：2026-09-05
- 優先序：帳本已 ratify 決策 > 本檔「操作者規則」 > 本檔「管理者自訂」
- 改本檔走 PR；緊急時操作者可直接 push

## 操作者規則

- **R1 認領**：起手前（R21）在 issue 留 `taking: <machine>/<role> at <ISO8601>` 留言**並**貼 `fleet:claimed` 標籤——留言加標籤合起來才是認領憑證；`lane:*` 標籤（`lane:feature`、`lane:4090`、`lane:docs`）只是路由／分類，永遠不是認領。先寫先贏；讀到別台的 taking 就不開工。完整、可查的跨機器 carrier 與交還規則是帳本 `fleet.cross-machine-claim`；本條只保留操作者使用的憑證形式。來源：#784、#682。
- **R2 重複產物**：同一 issue 有兩份產物，留 doneWhen 覆蓋較完整的那份；另一份關閉，可用部分搬過去。不因流程錯誤丟掉真工作。來源：#613 裁定、PR #670。
- **R3 停止的定義**：三證齊備才算停 —— 排程任務非 `Running`、**沒有 `edda.exe` 仍持有該 lane 的 briefs 路徑**、lane log 有 `=== EXIT ===`。`Ready` 單獨不代表停（#650 被宣告交還時 lane 仍在寫）。只殺 wrapper 不算停。來源：#672、`fleet.lane-stop-4090`。
- **R4 lane 類型**：diff 需要 cargo（Rust 原始碼、測試、fixture、`Cargo.*`，或 CI 會跑 cargo）就走 4090 build lane，否則走文書機。不看 issue 標題前綴。來源：#651。
- **R5 檔案撞車**：同一檔多方要改，改動最多且最接近推送者先做完，其餘排後 rebase；小修法以留言交持有者併入。來源：`operator-runbook.md` 三方裁示。
- **R6 合併**：LGTM 釘的 SHA 等於 current head、`CI Gate` 綠、SHA 窗為空、P0 與 P1 都是零，就 squash 合併、不刪分支。來源：`fleet.merge-authority-4090`、`ci.merge-gate`。
- **R7 不可逆**：不刪分支、worktree、來源；不 force push（含 `--force-with-lease`）；已推到 origin 的 commit 不 `commit --amend`、不 rebase——要改就加新 commit；不超每日預算；不碰認證。遇到就記 `blocked-by-rule` 跳過。來源：#906（窗內 `--amend` + `--force-with-lease` 一次）、#913。
- **R8 無規則**：管理者自訂一條寫進下方「管理者自訂」段，附案例與理由，照做；操作者可改或刪。
- **R9 身分**：所有留言、心跳、認領用 `<machine>/<role>`；不用分支名、session id、顯示名。具體形：`taking: 4090/worker-1`、`edda dispatch --machine docs/reviewer`。
- **R10 訊息**：真相先寫在 issue 或 PR，host 訊息只做門鈴；永不向人提問。
- **R11 認證失效**：該運輸的 lane 停派，在 board 記一筆 `needs-operator: relogin <tool> on <machine>`，只記一次；不重試、不換帳號。來源：#593、#669。
- **R12 核准的計畫步驟**：操作者已核准的設計稿步驟直接以 `fleet:ready` 開單。
- **R13 交還**：交還必須附 R3 三證與 `git ls-remote` 結果；交還後 15 分鐘再查一次。若是**未交付即釋放**，必須**編輯、不得刪除**原認領留言：將原本的 `taking:` 行劃線，並在**同一則留言**加 `RELEASED — this claim is withdrawn`，再移除與釋放事實矛盾的 `fleet:claimed`／機器 `lane:*` 標籤。活認領的機器判準與 `scripts/fleet-claim-issue.sh` 相同：逐行去前導空白後，只有開頭仍是 `taking:` 的行算活；獨立的 `RELEASED` 行不會使原 `taking:` 行失效。已交付的 claim 是正確歷史，進行中的 claim 不得因時間推測為過期。完整規則見帳本 `fleet.cross-machine-claim`。來源：#650、#682。
- **R14 每日預算**：管理者自身每日 5 美元；lane 照 brief。
- **R15 認證失敗的機器判準**：一輪 agent 回合若 `Cost: $0.00`，一律當失敗處理，不論 exit code。理由：認證失敗的回合成本必為零，而 `edda dispatch` 目前回 exit 0（#669）。#669 落地後改以 exit code 為準，本條保留為交叉檢查。
- **R16 存活面的已知污染**：`edda peers` 在 `cargo test -p edda-bridge-claude` 執行期間會出現非 UUID 形狀的假 session（測試 fixture 寫進真實 store，#646）。判存活時忽略 session id 不是 UUID 形狀的條目。心跳判活的視窗是 120 秒（`stale_secs()`），所以兩次查詢相隔超過該視窗可能得到不同答案 —— 這是時序，不是 store 分裂。
- **R17 心跳缺席不是死亡判決**：互動 session（Claude Code 等 hook 驅動）的心跳只在 hook 事件時寫，長工具呼叫期間零事件，120 秒後就從 `edda peers` 消失但人還活著；`edda dispatch`／`conduct` 起的 lane 由 runner 每 30 秒刷新（`crates/edda-conductor/src/runner/heartbeat.rs:25`），stale 才等於死。所以心跳缺席只是「去查 process tree」的提示，死亡判決一律走 R3。禁止把 `stale_secs` 調大來掩蓋。來源：#617、#646。
- **R15 備註**：`Cost:` 有兩條互不相交的路徑——dispatch 輸出的 `cost_usd` 是 backend 原樣透出，bridge 的 `estimate_cost` 是定價表（#677）。R15 只看前者；成本欄位缺量測時沿 CLI 的 `cost_line` 顯示 n/a，不偽造 0.0（GH-533）。
- **R21 起手守門**：任何 session 開始一張 issue 之前——`edda dispatch --issue`、fleet-worker 領單、或操作者在對話裡直接指派——先跑 `sh scripts/fleet-claim-issue.sh --check <n> <machine>/<role>`；exit 1（別條 lane 的 `taking:` 已存在，或任何 open／merged PR 指到這張 issue）就不開工，回報原因。檢查與開工在同一個工具步驟；操作者口頭指派的 session 不例外。來源：#782、#716 同機器撞車、R19 的同一原則用在實作側。
- **R22 引擎權限**：判決要不要寫 `Independent Review` status，由「引擎 × 面」決定，不由控制者決定。面按路徑分四類：**出貨面** `crates/**`、`Cargo.toml`、`Cargo.lock`、`install.sh`、`rust-toolchain*`、`clippy.toml`；**閘門面** `.github/**`、`lefthook.yml`、`scripts/lint-*.sh`、`scripts/fleet/lane-launch.ps1`、`scripts/fleet-claim-issue.sh`；**審判面** `REVIEW.md`、`docs/fleet/rules.md`、`tests/canaries/**`、`scripts/review-pr.sh`、`scripts/pr-review-watch.sh`、`scripts/review-l0.sh`（隨 #915 落地）、`scripts/reviewer-capabilities.sh`、`scripts/fleet/reviewer-capabilities.ps1`、`scripts/fleet/daily-digest.sh`，以及任何會貼 status 或執行合併的腳本；其餘全部是**內部工具面**（`scripts/fleet/**` 其他檔、`scripts/test-*.sh`、`docs/**`、`.claude/**`）。PR 只要有一條改動路徑落在出貨／閘門／審判面，整張 PR 就屬該面。引擎表：Opus（`claude-opus-5`，只經 Claude Code）與 sol（`gpt-5.6-sol`）對四面都是權威；glm-5.3-flash 對**內部工具面**權威（docs 類是帳本 `fleet.review-engine-pool` 已 ratify 的資格；scripts 類是窗期暫定，依 #884 重校與 09-07 的 compare 決定去留），對出貨／閘門／審判三面只能 SHADOW。權威輪次寫 status（帳本 `review.verdict-carrier`；#760 的 R18（未合併））；SHADOW 輪次標題尾加 ` (SHADOW)`、正文有 `shadow: true`、不貼 label、不寫 status，PR 留給權威引擎。來源：`fleet.review-engine-pool`、`fleet.window-merge-surface`、#888（09-05 兩台各跑一套）、#884。
- **R23 判決形狀**：判決留言第一行必須是 `## Code Review: Round <N> — PR #<n> @ <完整 40 位 SHA>`（SHADOW 輪加 ` (SHADOW)`），貼的是報告本身，不含審查者的敘述、思考或 transcript；第一行不是這個形狀的留言不是判決——不得由它寫 status、貼 label、或算作一輪。發判決的腳本只貼從第一個 `## Code Review: Round` 行起的文字，找不到該行就不貼並以非零退出。來源：#867（判決被貼成 transcript dump，標題在中段）、#917。
- **R24 就緒與報告**：說一張 PR「完成／可合／已審」之前，三個欄位都要查、都要寫進報告：(1) 最新判決釘的 SHA 等於 `gh pr view <n> --json headRefOid`；(2) `mergeStateStatus` 不是就緒訊號——ruleset 只保護 `main`，base 不是 `main` 的 PR 會在零判決時回報 CLEAN；(3) `mergeable` 不是 `CONFLICTING`。給操作者的報告一律是 `sh scripts/fleet/daily-digest.sh --board 888` 的輸出，不手寫；手寫摘要不是報告。來源：#914（#899 被報成完成而 head 已移動；#890 DIRTY 七小時無人察覺）、#888 day-0。
- **R25 政策面**：跨機器的政策只有一個面：本檔在 `origin/main` 的版本。控制者每次 tick 起手先 `git fetch origin`，再讀 `git show origin/main:docs/fleet/rules.md`（不讀本機 checkout——它可能在別的分支或落後）。單機帳本裡未 ratify 的控制者決策只約束記錄它的那台機器，寫進本檔或 ratify 之前不是政策；兩台機器對同一件事執行不同政策時，以本檔為準，差異記在 #888。來源：#671（帳本不跨機）、#888（09-05 兩台各跑一套：SHADOW 不合 vs 權威全合）。
- **R26 工作以 origin 為準**：lane 的每個 commit 在同一次執行內推到 origin 的同名分支；結束時還有未推的 commit 或未 commit 的檔案，就是失敗的 lane，收據必須逐一列出這些檔案與分支。沒有推到 origin 的工作視為不存在——下一個行動者不會知道它在。控制者的 digest 列出本機所有領先 origin 的分支。來源：#888（gh557／gh772／gh792 是 Opus 時代、gh881／gh882 是 flash 時代，同一種丟法）。
- **R27 運輸**：Anthropic 模型（Opus、Sonnet、Haiku）只經 Claude Code（`edda dispatch --agent claude` 或 `claude -p`），不經 pi／openrouter／任何其他 runtime——即使 pi 的 model 列表顯示 `anthropic/*` 為 ready。sol 經 pi `--model openai-codex/…` 或 codex app-server；glm 經 pi／openrouter。違反的 dispatch 要 fail-closed 拒絕，不是警告。來源：`fleet.claude-subscription-transport`（Tim 09-02 親裁）、#890 的拒絕測試。

## 管理者自訂

（空。管理者依 R8 追加，每條附日期、案例、理由。）
