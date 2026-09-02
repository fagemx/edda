# 操作者 Runbook：一個人怎麼跑這支 fleet（2026-09-02 版）

> 這頁回答一個問題：**早上坐下來，打什麼、在哪裡打、會看到什麼。**
> 入口不是一個指令，是**一個 Claude Code session 當「控制者」**；skill 是控制者照著做的劇本；
> pi／codex／claude 是被派出去的引擎。你跟控制者說話，控制者派引擎。
>
> 每一條指令都在 2026-09-02 於這台工作站核對過旗標。標「缺」的欄位對應到已開的 issue——
> 那些就是今天還要人手做的地方，也是這頁之所以還不夠短的原因。
>
> **路徑是這台工作站的**（`C:\ai_agent\edda`、`~/.codex/hooks.json`、lane root）。換機器時把它們換成該機的路徑；
> 帳本（`.edda/`）不進 git，另一台機器是一本新帳，binding 決策要照 handoff issue 重記。
> 在**沒有 session 身分的程序**裡（例如排程任務起的 lane），`edda coord` 會拒絕：「cannot prove which live session belongs to this process, so --session is required」——
> 依決策 `coord.session-identity`，需要身分的動詞要帶 `--session <id>` 或在環境設 `EDDA_SESSION_ID`；`edda status` 與 `edda peers` 不需要身分，照常可用（2026-09-02 實跑確認）。
> 控制層的 `watch` / `report` / `promote` / `intake` 是**概念動詞**（定義在 `docs/superpowers/specs/2026-09-02-control-layer-and-l2-shapes-design.md` §2.1）；§五列的是**現有指令**，其中 `edda watch`（TUI）與 `edda intake github` 與概念動詞同名但範圍不同，勿混用。

---

## 一、三個角色

| 角色 | 在哪 | 做什麼 | 用什麼 |
|---|---|---|---|
| **你（操作者）** | 跟控制者的對話 | 定方向、promote、ratify、合併授權、看板 | 說話；`gh issue edit`；`edda ratify`；`edda watch` |
| **控制者** | 一個 Claude Code session，開在 `C:\ai_agent\edda` | 判併行、寫 brief、派 lane、派審查、盯、貼判決、開單、合併 | skill：`/fleet-orchestrate` `/parallel-wave` `/fleet-review` `/fleet-pr-loop` `/issue-intake`；指令：`edda dispatch` `edda conduct` `edda peers` `edda task` `gh` |
| **Lane（引擎）** | pi／codex／claude 各自的 session，各在自己的 worktree | 照 brief 做一件事、開 PR、停 | 不需要會 skill；brief 就是全部。codex／claude 開場會被 hook 注入帳本 pack；pi 沒有 bridge（#577），只看得到 brief |

**Codex 跟 skill 的關係**：skill 給控制者用，Codex 是被派的。你不用二選一。
（`fleet-orchestrate` 附 `agents/openai.yaml`，理論上 Codex 也能當控制者；今天驗證過的路是 Claude 當控制者。）

**控制者不寫產品碼。** 它派 lane 寫。想動手＝起草 brief 派出去。

---

## 二、你的四件事

| 事 | 指令 | 備註 |
|---|---|---|
| promote（撕 ready） | `gh issue edit <N> --add-label fleet:ready --remove-label fleet:pending` | 或跟控制者說「這幾張 promote」。批次表見 #599 |
| ratify（讓決策 binding） | `edda ratify <key> --note "<為什麼>"` | agent 記的決策全是 unratified；`edda ask "<domain>"` 先看 |
| 合併授權 | 對控制者說「LGTM 就合」（standing）或逐張放行 | 前置：final current-head LGTM、P0=0/P1=0、7 格 CI 綠、SHA 窗檢查（見 §六） |
| 看板 | `edda watch`（即時 peer／事件 TUI）、`gh pr list`、`edda task list` | dispatch 出去的 lane 目前不出現在 peers（#569） |

---

## 三、控制者的一天

1. **開場**：在 `C:\ai_agent\edda` 開 Claude Code。pack 自動列出決策、peers、任務。說：
   「`/fleet-orchestrate` 今天跑 ready 的單」。控制者先做 fleet-orchestrate 的 controller sequence
   第 1–2 步：定目標、排除、證據門檻、開單與合併授權、停止條件；看 revision、dirty state、peers、claims、issue/PR 狀態。
2. **判併行**：`/parallel-wave`——每張 ready issue 推 predicted write surface，兩兩交集：
   disjoint → 一起派；同檔不同符號 → 兩邊 brief 寫 FORBIDDEN 符號清單；同符號 → 串成一條；scope 太糊 → 退回佇列。
   `edda claim check`（#576，2026-09-02 已合進 main）把這步變成機器判——**但要用從 main 重建的二進位**：
   PATH 上的 `edda.exe` 可能比 #576 舊，`edda claim --help` 沒列出 `check` 就是舊的（它會把 `check` 當成 claim 的 label）。
   重建：`cargo install --path crates/edda-cli --force`，再 `edda claim --help` 確認。
3. **每張一個 plan、一個 worktree、一條 lane**：
   ```bash
   git worktree add C:/ai_agent/edda-wt-ghNNN -b <branch> origin/main
   edda claim "ghNNN" --paths "crates/<crate>/src/*"
   edda conduct run <plan.yaml> --agent pi --cwd C:/ai_agent/edda-wt-ghNNN      # 多 phase
   edda dispatch --agent pi --prompt-file brief.md --cwd C:/ai_agent/edda-wt-ghNNN --budget-usd 5   # 單輪
   ```
   plan YAML 放 scratchpad 或 `.tmp/plans/`，不進 repo。Rust lane 設 `CARGO_TARGET_DIR` 為
   `$env:LOCALAPPDATA\fleet-workstation\lanes\worker-1|worker-2`（verifier 用 `verifier|verifier-2`）。
   lane 的**啟動方式**見 §六（Task Scheduler，不是 nohup）。
4. **PR 一開就派審（自動，不用人手）**：本機 watcher（`scripts/pr-review-watch.sh`，由
   `scripts/pr-review-launch.ps1` 註冊成隱藏排程任務 `edda-pr-review-watcher`）每 60 秒掃 open PR：
   非 draft、head 沒審過的 PR 在 **3 分鐘內**自動起唯讀審查者（gpt-5.6-sol，Task Scheduler 隱藏視窗，
   worktree 在 `$EDDA_FLEET_SCRATCH/wt-review-prN`）並貼確認留言 `review: started on <full sha>`；
   判決（含 observed model、cost、釘死的 head SHA）在審查者跑完後（約 5–15 分鐘）自動貼上 PR，
   並加 label `review:lgtm`／`review:changes-requested`；push 後 head 變了自動再審一輪。
   檢查方式：`Get-ScheduledTask edda-pr-review-watcher`、`tail ~/.edda/fleet/watch.log`、PR 留言與 label。
   provider 過載時：pi 重試一次，仍沒有判決就標 `review:unreviewed` 並對該 head 停手
   （v1 無 codex 後備——它做不到唯讀；§六 `fleet.review-provider-overload` 的決策全文仍可 `edda ask` 查）。
   啟停、狀態檔與疑難排解見 `docs/guides/pr-review-watcher.md`。watcher **不合併**——合併仍在第 6 步、要授權。
5. **收斂**：`/fleet-pr-loop` 的 bash driver 吐 `ACTION: REVIEW | FIX | DONE | BLOCKED`，照做到 LGTM；driver 不合併。
6. **合併**（有授權時）：`git diff <LGTM 的 SHA>..origin/<branch>` 必須為空（判決還在），`gh pr checks` 7 綠，才合。合併後對剩下的 PR 做 Layer-3 交集：不相交直接合，相交要 rebase → 判決失效 → 再一輪。
7. **開單**：審查 exhaust、runtime 的傷、重複兩次的手動步驟，當場 `/issue-intake`／`/issue-create`（含四問接線審計）。不要留在對話裡。
8. **收工**：`edda note "completed X; decided Y; next: Z" --tag session`；回報你：合了什麼、開了什麼、等你什麼。

---

## 四、Lane 的三件事

1. 在指定 worktree 與分支上做 brief 說的那一件事——不 checkout main、不 pull、不開別的分支。
2. L0 閘（`cargo fmt --all --check`；`cargo clippy -p <crate> --all-targets -- -D warnings`；`cargo test -p <crate>`），
   凍結 SHA 前跑一次 L1（`CARGO_INCREMENTAL=0`，workspace 全套），記 gate receipt（SHA、閘、toolchain、lane、結果）。
3. `git push -u origin <branch>`、開 PR、**停**。不合併、不刪分支、不刪 worktree。

Brief 必含：assigned build lane、verification budget（L0 while iterating；L1 once per frozen SHA）、cleanup authority（build cache 可清；worktree／branch／source 不刪）。

---

## 五、每個機制今天在哪、缺什麼

| 機制 | 今天用什麼 | 缺（單號） |
|---|---|---|
| 觀測 | `edda watch`、`edda peers`、`edda conduct status`、`gh pr checks`、`edda status` | dispatch lane 不在 peers（#569）；統一狀態面（#567）；孤兒回收（#573）；freshness（#604） |
| 進度追蹤 | issue 標籤（pending → ready → PR → merged）；`edda task new <title> --after <id> --assignee <label>`、`edda task start <id>`、`edda task done <id> --receipt "<可驗的話>" --evidence <path>`；PR 上的審查輪 | 成本與模型不進帳本（#582、#574） |
| 派發 | `edda dispatch --agent <claude|pi|codex> --prompt-file <f> [--session-id] [--cwd] [--budget-usd] [--timeout-sec] [--permission-mode] [--json]`；`edda conduct run <plan> --agent <x> [--cwd] [--dry-run] [--tmux] [--json]`；審查由本機 watcher 自動起並貼判決（`scripts/pr-review-watch.sh`，#632） | 選模型/思考深度/工具(#574);角色 profile(#593);批次發射(`edda wave`,等 #576 與 #599) |
| 討論提問 | 你 ↔ 控制者對話；控制者 ↔ 其他 Claude session 用跨 session 訊息；對 lane 用 `edda request "<label>" "<msg>"`（門鈴；lane 沒心跳時要 `--force` 排隊）；耐久的寫 issue／PR 留言 | 事件驅動門鈴（#545）；lane 心跳（#569） |
| 決策 | `edda ask "<domain>"` → `edda decide "k=v" --reason "…"`（agent，unratified）→ `edda ratify <key>`（你） | 簽章身分（#609） |
| 開單 | `/issue-intake`、`/issue-create`（四問接線審計必填） | 批次進料與確認表（#599）；驗收端 wiring verdict（#594） |
| 通知 | 背景任務完成會叫醒控制者；`edda notify` 存在 | 事件驅動（#545） |
| 成本 | `--budget-usd`；plan 級 measured-ness（#533 已合） | 讀端報表（#582）；digest 成本 0.0 哨兵（#585）；conductor 散文成本（#584） |

**存在但本頁未驗證是否符合現行流程的動詞**：`edda pipeline`（skill chain with approval gates）、`edda intake`（外部任務進帳）、`edda prs`（掃 GitHub PR 事件）、`edda bundle`（審查 bundle）、`edda scan`（能力掃描）、`edda brief`（任務 brief 檢視）。用之前先 `--help` 並確認有讀者。

---

## 六、今天的硬規則（來源＝帳本決策，`edda ask` 可查全文）

| 規則 | 決策 key |
|---|---|
| 執行用便宜模型（pi 預設 glm-5.3-flash）；**審查一律 gpt-5.6-sol**（codex／pi 皆是） | `fleet.agent-model-split` |
| 審查 provider 過載：**改運輸不降模型**——(1) 同 `--model` 先用 `--thinking minimal` 探測再重試 pi；(2) `edda dispatch --agent codex`；(3) 都不行就把 PR 標為**未審查**並停——未審查是誠實狀態，便宜模型的判決不是 | `fleet.review-provider-overload` |
| **lane 啟動走 Task Scheduler，不走 nohup／Start-Process**：Claude Code 的工具 shell 在 Windows Job Object 裡，nohup 的子程序仍隨 session 死。`Register-ScheduledTask` + `Start-ScheduledTask`（父程序是 svchost）；該環境 `CARGO_TARGET_DIR` 與 `HOME` 為空，lane wrapper 必須顯式設；`Get-ScheduledTaskInfo` 可輪詢，`Unregister-ScheduledTask` 清理。重派前先讀 worktree／branch／PR 狀態，不信任 live handle | `fleet.lane-launch`、`fleet.lane-dispatch` |
| 一 issue ＝ 一單 phase plan ＝ 一 worktree ＝ 一 build lane；並行在 plan 之間；plan 裡不寫沒理由的 `depends_on`；並行 plan 不用 verdict gate | `cleanup.parallel-exec`、`cleanup.review-gate` |
| build lane 只用 `worker-1|worker-2|verifier|verifier-2`；永不建 ad-hoc `CARGO_TARGET_DIR`；L1 與 verifier 設 `CARGO_INCREMENTAL=0` | `verification.cost-discipline` |
| 審查釘 full SHA；**每次 push 使前一個判決失效**；一個 PR 一個審查者身分 | `fleet.review-protocol` |
| 合併＝final current-head LGTM、P0=0/P1=0、7 格 required CI 綠（Format＋三平台 Clippy／Test）、SHA 窗檢查為空 | `pr.merge-policy`、`ci.merge-gate` |
| worktree／branch／source 永不刪；build cache 可清、按年齡回收 | CLAUDE.md Build lanes |
| 決策 recorded ≠ ratified；agent 不 ratify 自己的決策 | README 兩層授權 |
| 審查 brief 用「驗證清單」框架（契約＋要確認的輸入形狀），不用攻擊計畫框架——後者會被 provider 拒收、燒掉一輪 | `fleet.review-brief-framing` |
| brief 要先自己跑過一輪才交付（**非帳本決策**：來源是探索場 31 號第零條；edda 側尚無對應決策，要立法先開單） | — |

---

## 七、礦（探索場）怎麼用同一套

- 在乾淨 repo（提案 `C:\ai_project\hybrid-kiln`）`edda init`；Codex 的 edda hook 已裝（`~/.codex/hooks.json`），開 Codex 就會被注入你簽過的法。
- Codex 在那裡**既是控制者也是引擎**，直到 edda 給它 dispatcher。
- 一爐＝一張 task：`edda task new "R1 考次 2" --after <上一爐的 id>` → 跑 → `edda task done <id> --receipt "走私 x% 盲配 y% 真異 z% 加冕 n 每冠 $c" --evidence <判定.md>`。
  上一級沒有收據，下一級開不了——「考不過不蓋」變成系統行為。
- 法：`edda decide "kiln.<domain>=<v>" --reason "<催生它的卷>"`；你 `edda ratify`。卷仍是 markdown；證物仍在資料夾，帳本只記路徑與收據。
- 判卷席：獨立 session id，任務書不給引擎名（紀律版）；結構版等 #574 的 `--exclude-tools`。

---

## 八、為什麼還模糊：手動點對照單

| 今天要人手做的 | 解它的單 |
|---|---|
| 每次審查手動起 pi、手動貼判決、手動轉給實作者 | #574（審查走 dispatch）、#545／#567（門鈴與狀態面） |
| 逐張 promote | #599（確認表批次） |
| 不知道 lane 死了沒 | #569、#573、#604 |
| 不知道花了多少、跑了什麼模型 | #582、#584、#585、#574 |
| 判併行靠控制者看 | #576（claim check） |
| 唯讀審查靠 brief 文字 | #574（`--exclude-tools`）、#593（profile） |
| 審查沒檢查「接得上沒」 | #594 |
| 跨機器／跨人不能信收據 | #608、#609 |

這張表清空的那天，本頁會縮成一屏。
