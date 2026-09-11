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

## Delivery entry 與 transport index（`delivery-flow/1`）

Generic 流程唯一 authored source 是
`crates/edda-cli/src/skills/coord-orchestrate.md`；本 repo tracked
`.claude/skills/coord-orchestrate/SKILL.md` 是 byte-identical projection。
`edda init` 只把 embedded bytes 寫給偵測到的新 host，既有 custom skill 預設保留；
不要為更新單一 copy 對未知 checkout 使用 `--force-skills`，因為它會覆寫**每一份**
embedded project skill（包含未來或非 coordination 新增項）。`.agents/` 是
local/generated state，不 force-track。

| 進來的情況 | 路由 |
|---|---|
| 已 assigned／resume | `edda task show <id>`，讀 reachable brief、舊 result 與 source，做原角色；不重開 planning／formation |
| controller resume 已有 plan | 先讀 rail owner、active map、task JSON、dispatch/session 與 PR/source evidence，再選 next action |
| `issue-pipeline --skip-plan` | 重用已有 acceptance，不是略過 acceptance；`--no-merge` 只停止，不給 merge authority |
| `issue-action` | 保留 issue acceptance／owner／fix context，再進 generic flow；若 caller 是 active `edda pipeline`，implementation 必須建立／更新唯一 open PR，並讓 `closingIssuesReferences` 連到 numeric issue |
| `edda pipeline` | product template 透過 `issue-action` 實作；依本 repo focused L0 驗證；implementation 的 `cmd_succeeds` machine check 以 issue ID 要求唯一 linked open PR 與 exact GitHub PR URL，review phase 不接前一 phase output，而是獨立重跑相同 lookup；`gh` error／零筆／多筆／malformed URL 都拒絕進 `/pr-review`；pipeline／worker／reviewer 都不合併 |
| `pr-review-loop` | 只有 author self-check/fix；不是 independent verdict 或 merge loop |
| 小型或一條 cohesive writer chain | 原 session 普通實作，不為儀式建 rail／fleet |
| 兩個以上真正可並行 writers | 一個 controller 才啟用 `coord-orchestrate` formation；只有實際 artifact dependency 等待 |

同一 repository task rail 先選**一個** owner。Manual 模式使用 exact key
`delivery.rail-owner=manual:<controller-session>`，建立／啟動前掃所有 plan/task/peer，
並證明 scheduler、one-off reconcile process 與舊 reconcile attempt 都不在執行；不明就
拒絕。對 **legacy/uncontrolled task**，Reconcile 模式由現有 runner 全權負責 Codex
start/retry/settlement，不混入 manual start、Pi／ACP／host 或 no-retry task。受 accepted
`ExecutionBriefV1` 綁定的 **controlled task** 不走這條 legacy lifecycle：目前 accepted product
只能驗證／綁定後回傳 `execution: "none"` 的 descriptor，不會 launch；在 authorized S6
capability 出現前，direct controlled execution unavailable。Literal `CONTROL_UNAVAILABLE` 僅是
future planned S6 contract，不是 current evidence。Descriptor、caller-authored event 或 ordinary
task command 都不是 launch authority。因為目前 reconcile 不按 `plan_id` filter，絕不能用
`delivery.rail-owner.<plan>` 讓不同 plan 各選一種模式。Decision 只是 caller coordination
evidence，不是 runtime lock、身份授權或 exactly-once 保證；切模式要操作者授權並先處理
scheduler/process/lease/side effects。

Manual controller 用 `delivery.active.<plan>` 的 exact decision 加 exact `plan_id` recovery，
每個 `task new --key` 都要 capture ID 後 `task show --json` 比對；same key 只 dedupe create，
不更新 brief／owner／scope／deps。Failed 且 assignment 不變時 `task start` 同一 ID 是 retry；
owner／brief／dependency 改變則開下一 revision，重接 pending successor 後才 supersede map，
舊 task 留歷史、不 fake done。Controller launch 前 start 一次；worker 只讀 Running 後正常
done/fail。這個 ordinary `task done --receipt ... --evidence ...` 與 Done 後 metadata correction
只適用 legacy/uncontrolled task。Controlled completion 必須由 authorized product path 驗證
`WorkReceiptV1`，精確綁定 brief/session/attempt/lease/outcome 並帶所需 S6 authority seal；不接受
ordinary `--evidence` 或 legacy post-Done correction，且 capability 尚不可用時 fail closed。
Dispatch 的 `outcome=done` 不等於 task done、review 或 merge。

| Backend | 本 repo 的 caller contract |
|---|---|
| Pi | task-linked `--prompt-file` + `--cwd`；續跑重用 `--session-id`，若選過則保留 `--session-dir`；不用 `--task-id`／`--resume` |
| Claude | task-linked prompt；既有 conversation 用同一 `--session-id --resume`；不用 `--task-id` |
| Codex | task-linked prompt；同一 `--session-id` 並核對 observed thread；不用 `--task-id`／`--resume` 或不支援的 model/tool flags |
| ACP | 另建 task 且 `--agent acp:<target>`、reachable relative brief、existing concrete roots；start 後只用 matching `--task-id`，不塞 prompt/session/legacy flags/`--detach` |
| Host subagent | host task text 明載同一 brief；沒觀測到 native continuity 就明示 replacement，不假稱 Edda dispatch session |

ACP preflight 驗 task/kind/Running/concrete roots，但不證明 brief 可讀；bounded injection 可能
`unavailable`／`truncated`，worker 仍須用實際能力讀 full card。Caller-authored event 也不能
繞過目前 controlled execution 所需的 product-verifiable authority seal。需要網路的 Codex
角色走 `edda dispatch --agent codex` 以保留 user global Codex config；不要改用會強制
read-only sandbox、使 outbound 失敗的 Claude Code codex plugin。

Author 只做一次 combined self-check activity，同一 receipt 記 behavior lens 與最可能失敗的
counterexample lens；它不取代 independent current-head review。本 repo author 跑 focused L0，
L1 是 exact-head CI，加 verifier 對 Windows CI 未覆蓋 surface 的 focused C5；不要因 SHA freeze
本機重跑 workspace 全套，也不要給 docs-only 工作虛構 build lane。合併只走
`edda review merge --pr <N>` 的既有 R6 path；`--no-merge`、task done、worker／fixer／reviewer
都不會取得 authority，絕不改成 raw `gh pr merge`。

## START HERE：控制者開場

1. **先確認這個 checkout 在 `main` 上**。控制者 session 若開在過時或 feature-branch 的
   checkout，runbook、fleet skills、`scripts/fleet/` 全都不在樹裡，後面每一步都會落空
   （2026-09-02 就有一個 session 這樣燒掉）。驗證（先 `git fetch`——本地追蹤的
   `origin/main` ref 可能過時，不 fetch 就比會對舊 ref 誤報 0）：
   ```bash
   git fetch -q origin                     # 先刷新 origin/main 追蹤 ref
   git status                              # 乾淨、on branch main
   git rev-list --count HEAD..origin/main  # 0 = 沒落後
   ```
2. **接單**：手動認領用 `sh scripts/fleet-claim-issue.sh <N> <machine>/<role>`（例如
   `4090/worker-1`、`docs/reviewer`）；`edda dispatch --issue <N> --machine <machine>/<role>`
   也會在派發前做同一套認領。認領、釋放與已交付歷史的正典是帳本
   `fleet.cross-machine-claim`（`edda ask fleet.cross-machine-claim`）；本 runbook 不重述該協定。
   實作入口、`--check` 的 exit code 與身分格式見 `docs/fleet/rules.md` R21（#782）。
3. **派 lane**（開 worktree 後）。**先選路徑——有三條，失敗模式不同，不要照抄別人的**：

   | 路徑 | 用在 | 活過 controller session? | 停法 |
   |---|---|---|---|
   | **A. Agent 背景 subagent** | 同一輪要來回幾次的修復／審查；控制者全程在線 | **否**——host 程序死，lane 一起死 | `TaskStop`（不可逆，見下） |
   | **B. `lane-launch.ps1` + 排程任務** | 長工、無人值守、要活過 context 耗盡 | 是 | `lane-stop.ps1`（只有這個算停，R3） |
   | **C. `edda review --pr <N>`** | 只要一份獨立判決 | 隨運輸 | 見第 5 步 |

   **路徑 A**（裁定 `review.dispatch-transport=controller-subagent-direct`，2026-09-08；
   `REVIEW.md` §0 也走這條）：brief 寫成 `.md` 放 scratchpad，用 Agent 工具起背景 subagent，
   model 在呼叫裡指定（修復 `sonnet`、審查 `opus`——`fleet.review-engine-model`）。
   brief 版面見 [`brief-template.md`](brief-template.md)。這條路有三個**只有它才有**的陷阱：

   - **這個 build 沒有 `SendMessage`**——停掉的 lane 接不回來，transcript 也不會 flush。
     所以 `TaskStop` 是不可逆動作，不是暫停。
   - **`.output` 檔是 0 bytes 不代表卡住。** 它在結束時才寫。2026-09-09 控制者用
     「output 0 bytes ＋ worktree 乾淨」判定一條 lane 卡住而殺掉，它被殺前最後一句是
     「機制已經弄清楚，準備動手改」。判死一律看 worktree 的 `git status` 加 PowerShell
     程序表（memory `liveness-signals-lie-by-tool`），而且判死門檻要跟動作的可逆性掛鉤：
     查一下成本近零，殺掉成本是整條 lane。
   - **lane 停在 pre-commit hook 的冷建上是常態，不是紀律問題**（#1099）。brief 要給
     `SKIP_CLIPPY=1`，見 brief 範本。

   **路徑 B**：
   ```bash
   pwsh -NoProfile -File scripts/fleet/lane-launch.ps1 -Name <lane> -Brief <brief.md> -Cwd <worktree> -Owns "<repo-path>[,<repo-path>...]"
   ```
   脚本不合成 build lane：`-BuildLane` 只收 `worker-1|worker-2|verifier|verifier-2`
   （決策 `verification.cost-discipline`），給了就在 wrapper 設
   `CARGO_TARGET_DIR = <lane root>\<BuildLane>`（lane root =
   `$env:LOCALAPPDATA\fleet-workstation\lanes`，可用 `FLEET_LANE_ROOT` 改）；
   寫入 lane 也要傳它實際會改的最小 `-Owns` repo path。**`-Owns` 只吃一個引數**：
   多個 scope 用逗號或分號串成同一個字串，例如
   `-Owns "crates/edda-cli/src/cmd_dispatch.rs,docs/guides/operator-runbook.md"`。
   舊的 `-Owns a b c` 寫法已不再接受：`pwsh -File` 只綁第一個值，其餘會被
   `-SessionId`／`-LogDir` 這些還空著的具名參數悄悄吃掉——實測基準版，
   被 `-LogDir` 吃掉那次把 lane 的 wrapper/log/done 寫進了 repo 裡（GH-937）。
   `-BuildLane` 不在此列：它的 allowlist 會在建任何 log 目錄前大聲失敗。
   現在整個寫法都會在綁定階段就大聲失敗。
   scope 必須是 canonical repository-relative path：不可用 absolute、drive/UNC、`..` 或 `./`
   alias；review lane 是唯讀可省略。
   Rust lane 要明確傳，如 `-BuildLane worker-1`；docs lane 只寫文件不編譯，
   不傳 build lane，wrapper 就不設 `CARGO_TARGET_DIR`（見 §六）。
4. **盯進度**（不用再翻檔案時間戳）：
   ```bash
   pwsh -NoProfile -File scripts/fleet/lane-status.ps1
   ```
5. **停 lane**（只有這個動作是真的停）：
   ```bash
   pwsh -NoProfile -File scripts/fleet/lane-stop.ps1 -Name <lane>
   ```
   `Stop-ScheduledTask` 與 `Unregister-ScheduledTask` **都不殺子程序樹**（GH-672）：
   它們只終止 wrapper，`edda dispatch` 子程序會繼續跑到 commit／push／開 PR，
   而任務顯示 `State = Ready`。停 lane 一律走 `lane-stop.ps1`：它停任務、殺整棵
   process tree、依 `CommandLine` 比對 wrapper／brief 路徑驗證無殘留，並補寫
   wrapper 已寫不出的結束記錄（done-file + `=== EXIT ===` 行）。
   硬殺撞上 git 寫 `.git/config` 會把那個檔變成整片 NUL，主 checkout 與全部
   worktree 同時失去 git（GH-715）。**殺法沒有變**（不帶 `/F` 的 `taskkill` 送的
   是 WM_CLOSE，而 lane 的程序是沒有視窗的隱藏 console 程序，實測回 exit 128
   「只能強制終止」且目標存活，所以沒有可用的優雅關閉），改成殺完之後修：
   `lane-stop.ps1` 驗證那份共用 config 仍可解析，壞了就用 `lane-launch.ps1`
   開跑前存的已驗證備份還原，並在 stdout 印 `gitconfig=…`；還不回來就 exit 1
   ——但結束記錄（done-file + `=== EXIT ===`）一定先寫，不會因為 config 的事
   丟掉（GH-672）。
   手動檢查或修復用同一支脚本：
   ```bash
   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <worktree> -Verify
   pwsh -NoProfile -File scripts/fleet/git-config-guard.ps1 -RepoPath <worktree> -Restore
   # 若最新備份本身破損（但仍可解析）已被套用，直接手動覆蓋為前一代備份：
   Copy-Item <worktree>/.git/config.guard.bak.prev <worktree>/.git/config -Force
   ```

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
| 合併授權 | 對控制者授予 repository R6 standing authority（「LGTM 就合」） | 前置：final current-head LGTM、P0=0/P1=0、同 SHA 無其他非 LGTM 判決、required check「`CI Gate`」綠（`ci.merge-gate`）、SHA 窗檢查（見 §六）；條件成立後控制者立即合，不再二次請示 |
| 看板 | `edda watch`（即時 peer／事件 TUI）、`gh pr list`、`edda task list` | dispatch 出去的 lane 目前不出現在 peers（#569） |

---

## 三、控制者的一天

1. **開場**：在 `C:\ai_agent\edda` 開 Claude Code。pack 自動列出決策、peers、任務。說：
   「`/fleet-orchestrate` 今天跑 ready 的單」。控制者先做 fleet-orchestrate 的 controller sequence
   第 1–2 步：定目標、排除、證據門檻、開單與合併授權、停止條件；看 revision、dirty state、peers、claims、issue/PR 狀態。
   **Standing 授權（不必逐批請示）**：`fleet:ready` 標籤就是操作者的簽名——控制者接著自己跑
   fleet-orchestrate 的 ready-batch selection 程序選出這一批、產出選/排表，不回頭問編號；
   操作者的介入點是 promote 與裁決，不是每批打字給編號（程序正典在 fleet-orchestrate，這裡不重述）。
2. **判併行**：`/parallel-wave`——輸入就是上一步選單程序的選/排表；每張選中的 ready issue 推 predicted write surface，兩兩交集：
   disjoint → 一起派；同檔不同符號 → 兩邊 brief 寫 FORBIDDEN 符號清單；同符號 → 串成一條；scope 太糊 → 退回佇列。
   `edda claim check`（#576，2026-09-02 已合進 main）把這步變成機器判——**但要用從 main 重建的二進位**：
   PATH 上的 `edda.exe` 可能比 #576 舊，`edda claim --help` 沒列出 `check` 就是舊的（它會把 `check` 當成 claim 的 label）。
   重建：`cargo install --path crates/edda-cli --force`，再 `edda claim --help` 確認。
3. **每張一個 plan、每條 lane 一個固定 worktree**：
   ```bash
    # 首次準備或切到下一張 issue：固定路徑，不建 per-issue worktree
    pwsh -NoProfile -File scripts/fleet/lane-prepare.ps1 -BuildLane <worker-1|worker-2|verifier|verifier-2> -Branch <branch> -Repo C:/ai_agent/edda
   edda claim "ghNNN" --paths "crates/<crate>/src/*"
   edda conduct run <plan.yaml> --agent pi --cwd C:/ai_agent/edda-wt-<lane>      # 多 phase
   edda dispatch --agent pi --prompt-file brief.md --cwd C:/ai_agent/edda-wt-<lane> --budget-usd 5   # 單輪
   ```
    plan YAML 放 scratchpad 或 `.tmp/plans/`，不進 repo。lane worktree 固定為
    `C:\ai_agent\edda-wt-<worker-1|worker-2|verifier|verifier-2>`；prepare 只在閒置、乾淨
    worktree 且舊分支的 local tip 等於 `origin/<branch>` 時切換，絕不 force checkout、刪 branch 或刪 source。
    Rust lane 設 `CARGO_TARGET_DIR` 為
    `$env:LOCALAPPDATA\fleet-workstation\lanes\worker-1|worker-2|verifier|verifier-2`。
   lane 的**啟動方式**用 `scripts/fleet/lane-launch.ps1`（見 START HERE；Task Scheduler，不是 nohup，規則見 §六）。
4. **審查：強引擎直審是預設；沒有輪詢器**
   （`review.default-path=direct-review-default-shell-reserved-for-flash`、
   `review.merge-gate=ci-gate-only-independent-review-status-removed`、
   `review.watcher=polling-stopped-independent-rounds-dispatched-on-demand`，皆 2026-09-07）。

   預設路徑：任一在線的強引擎 session 直接讀 diff、照 `REVIEW.md` 從頭跑到尾、把 §7 判決貼上
   PR；合併仍須第 7 步的完整 R6 條件。**不起 lane、不建 review worktree、不做全檔快照。**
   這條路今天實測每張 PR 約 5–20 分鐘；同一批 PR 的審查 lane 在 40 分鐘後仍卡在快照階段。
   審查內容一點都沒放寬——`REVIEW.md` 全規則照跑、判決釘 full SHA、每次 push 使前一輪失效、
   合前做窗檢查——省掉的只有排隊。

   `Independent Review` 必要 status 已於同日從 ruleset 移除；它只保留為 `edda review deliver`
   冪等寫出的 advisory delivery projection，不是 `edda review merge` 的輸入。權威合併輸入是可信作者貼出的
   SHA-pinned §7 判決留言；ruleset 唯一 required check 是 `CI Gate`。
   **輪詢器連同整套審查殼已退役**（GH-1061，`review.shell-branch`）：它對每一張 open PR
   無差別自動派 lane，連只改一個 markdown 檔的 docs PR 也照收全額快照稅，這正是樸實流程要
   拿掉的東西。腳本已從 repo 刪除，它註冊的隱藏排程任務已從本機解除註冊；
   沒有回復路徑，也不需要——獨立輪次改成按需派，見第 5 步。

   要一份**獨立**判決時（實作者與控制者以外的第三方），用第 5 步的 lane 機制**按需刻意派一條**，
   不靠輪詢器。值得付這筆錢的兩種情況：(a) flash 引擎執行的單——brief 渲染、`brief-validate`、
   oracle bundle、資格表、SHADOW 校準整套是它安全上工的前提；(b) 實作者就是控制者的 PR。
   殼的成本只該由需要殼的引擎付，而且只在真的需要那一次付。

5. **按需派一條獨立輪次**（沒有殼、沒有輪詢器、沒有快照；三步，都是產品動詞）：

   1. **派**：`edda review --pr <N> --agent claude --model claude-opus-5`。它自己組 brief
      （`REVIEW.md` 讀 base SHA 那份）、以唯讀能力起審查者、把 `review_verdict` 事件寫進帳本。
      要自己控制運輸時就 `edda dispatch --agent claude --exclude-tools Edit,Write,NotebookEdit`
      餵同一份 brief——工具集是唯讀的那一半，brief 正文是另一半。
   2. **貼**：審查者自己用 `gh pr comment` 貼 §7 判決，釘 full SHA。沒有中間人代貼。
   3. **落**：`edda review deliver --pr <N>` 依 union 規則（`edda review gate`）冪等結算
      `review:*` label 與 advisory `Independent Review` commit status；判決格式壞掉時它貼一次告示而不是猜。
      這是 delivery projection，不是 ruleset required check，也不供 `edda review merge` 讀取。

   **唯讀怎麼證**（`review.readonly-proof=capability-flags-not-per-file-hash`）：看 capability
   旗標，加上前後各一次 `git status --porcelain`。**不做逐檔雜湊**——退役的殼對 1077 個檔案各
   spawn 一次 `git hash-object`，在這台機器（spawn ~2.7 s）等於每輪 ~100 分鐘，而唯讀性早就由
   `--exclude-tools` 結構性保證了。

   **合併不在這一步**——仍在第 7 步、依規則閘執行（`docs/fleet/rules.md` R6）。
6. **收斂 —— lane 開了 PR 之後，控制者的迴圈**。`/fleet-pr-loop` 的 bash driver 吐
   `ACTION: REVIEW | FIX | DONE | BLOCKED`,照做到 LGTM；driver 不合併。走第 4 步直審路徑時
   不需要 driver：判決與修正在同一個 session 內來回。

   **不論走哪條，派審之前控制者自己先驗這五項**——lane 的回報不算證據：

   ```bash
   gh pr view <N> --json headRefOid,state,baseRefName      # head 凍結
   gh pr diff <N> --name-only                              # 實際動到的檔
   git merge-base origin/main <head-sha>                   # 分支真正的 parent
   git diff --stat <base-sha>..origin/main -- <那些檔>      # 空窗檢查
   gh pr checks <N> --json name,state                      # exact-head CI
   ```

   **這個窗——派審之前的那個——方向是 `<base>..origin/main`。** 它問的是「從分支起點到現在，
   main 有沒有動過這張 PR 要改的檔」。寫成 `<head>..origin/main` 會把 PR 自己還沒合併的改動
   也算進去，永遠不會是空的；2026-09-09 弄反過一次。
   **這是本 runbook 加的一道，不是 R6 那道。** R6 與 `.claude/CLAUDE.md` item 9 列的窗是
   **合併前置條件**：`<審過的 SHA>..origin/main`，問「判決釘住的那棵樹跟 main 之間是不是空的」，
   由合併的 session 自己執行並記進 PR——`edda review merge` 刻意不做這一步（GH-1105 把閘
   收進這個動詞之後仍然如此：R6 的窗紀錄是 checkout 側的動作）。兩者基準不同——一個是分支起點，一個是判決
   釘住的 commit——時間點也不同，所以方向與時機都不能互抄。
   這一道要在派審**之前**做：審一棵已經被 main 動過的樹是浪費一整輪。

   **判決回來之後，讀 PR 上貼出來的那則留言，不要讀 agent 的回報。**
   兩者會不一致，而且以留言為準（R23：第一行不是 `## Code Review: Round <N> — PR #<n> @ <完整 SHA>`
   的留言不是判決）。這個 repo 是 **PUBLIC**,所以連 `authorAssociation` 一起看——
   只有 `OWNER`／`MEMBER`／`COLLABORATOR` 算數（GH-993 修的就是這個洞）。

   ```bash
   gh pr view <N> --json comments --jq '.comments[]
     | select(.body | test("(?m)^## Code Review: Round [0-9]+"))
     | "author=\(.author.login) assoc=\(.authorAssociation)"'
   ```

   **同一個 SHA 上可能有不只一則判決。** 2026-09-09 的 PR #1108 就有兩則 Round 3——一則
   sol、一則 Opus,不同 session 派的。依 R18 的 union 規則結算：只要還有一則非 LGTM 站著，
   後來的 LGTM 蓋不過。兩則都是 Changes Requested 時，阻擋集合是**兩者的聯集**。

   **後續輪次**：修復 brief 要明寫**什麼已經定案、不可重開**(審查契約第 2 條：後輪的 blocker
   必須是 fix 造成的或先前不可觀測的),否則 replacement verifier 會從頭再審一遍。
   走路徑 A 時原審查 session 接不回來(沒有 `SendMessage`),所以下一輪一定是 replacement——
   brief 要叫它**先讀 receipts**：前幾輪的判決留言、`Review Response`、exact-head CI。

   **什麼時候停**：審查契約第 6 條——兩輪沒有實質進展就停，把發現分類、路由，不要硬推。
   判準是「這一輪有沒有找到新的真缺陷」，不是輪數。真的要停時**把 PR 開著**,貼一則控制者
   handoff：現在的狀態、什麼已定案不要重審、下一手具體做什麼，並**把 `Closes #NNN` 從 PR body
   拿掉**,免得有人事後合併時關掉一張判決已經否掉的單。

7. **合併**（只由已具 repository R6 standing authority 的控制者執行；條件綠就立即合，不再二次詢問；worker、fixer、reviewer 永不合併；`docs/fleet/rules.md` R6）：先執行 `edda review merge --pr <N>`；它直接讀可信作者貼出的 SHA-pinned §7 留言，核對最新可信審查是目前完整 SHA 的 LGTM、P0=0/P1=0、無待升級項目且必要 CI 檢查通過，再直接結算留言聯集（GH-1057），不呼叫 `edda review deliver`、不讀 `Independent Review` status。該 SHA 上只要還有一則站著的 Changes Requested，後來的 LGTM 也蓋不過（GH-742）。**開著的 PR 整體漂移它只印不擋**：跨 PR 的漂移不是 R6 條件，被判的永遠是被合的那張自己——漂移行仍逐行印出（#1124、`review.merge-drift-guard=advisory-not-an-r6-condition`；fleet 健康訊號看 `edda review drift` 與每日摘要，不看這一步的 rc）。**被合的那張自己仍然擋，而且是從它自己讀的**：`mergeable=CONFLICTING`（R24 不准在這種狀態回報就緒，而 `--check` 就是那個回報）、孤兒 Review Response（回應一個從未貼出的輪次，GH-993）、walk 自己的留言順序把主體最新的**權威**判決讀成 stale（它按建立序讀留言、閘自己按 GitHub 的編輯序選最新的一輪；兩者不一致時以擋為準，不當綠，因為讀同一份留言得到兩個答案不是綠。SHADOW 輪次不是判決、不進 union，所以擋不住）、判決沒釘在 head（R6 的條件）——walk 讀不到時也藏不住這四者。檢查通過後，已有上述 standing R6 authority 的控制者直接使用 `edda review merge --pr <N> --merge`，不另請示；它以 `--match-head-commit` 鎖定審查 SHA，避免最後一刻 push 越過判決。它也把 squash subject 永遠釘在 PR 標題上（GH-1100）：合併一律帶 `--subject`——單 commit 的 PR 若讓 GitHub 自選 subject，會原封抄用該 commit 的標題，fa0d011 那次就是這樣把 `wip(review): ...` 寫進 main——且標題先按 conventional commit 格式驗證（REVIEW.md §5 U4），`wip(...)`、空 scope、缺 type 一律當場拒絕、不執行合併（`--check` 也驗，早一步給訊號）。GitHub 只會替「自己挑的」subject 補上 ` (#N)` 這個 PR 回指，帶了 `--subject` 就原文照用、不補；所以這個動詞自己接上 `<PR 標題> (#<PR 編號>)`，squash commit 才不會從此在 main 上失去 PR 指標（U4 驗的仍是純標題，不含後綴；標題若已經以「本 PR 自己的編號」結尾就不重複補，結尾是「別的 PR 編號」則照補，免得 `git log` 的回指指到無關的 PR）。合併 body 可用 `--body-file <path>` 指定（只在 `--merge` 有效，空字串或配 `--check` 都會拒絕）；沒給就自動組一張最小收據（審查 SHA、LGTM 輪號、CI run 連結）當 body。`scripts/merge-reviewed-pr.sh` 還在，是一行適配器（GH-1105）：PR 編號之後的參數原封轉發（`--check`／`--merge`／`--body-file <path>` 都穿得過去），`--help` 也直接接到動詞自己的說明，所以舊呼叫不變。合併後對剩下的 PR 做 Layer-3 交集：不相交直接合，相交要 rebase → 判決失效 → 再一輪。
8. **開單**：審查 exhaust、runtime 的傷、重複兩次的手動步驟，當場 `/issue-intake`／`/issue-create`（含四問接線審計）。不要留在對話裡。
9. **回收**（wave 收尾，**控制者**跑，在 `C:\ai_agent\edda` 主 checkout 跑；GH-1009）：
   先看 dry-run —— `sh scripts/fleet/reclaim-merged.sh`。每個 worktree／local branch／remote
   branch 一行，附對應 PR、PR 狀態、dirty 與否，以及被留下的理由；確認 `RECLAIM` 那幾行就是
   自己要清的，再 `sh scripts/fleet/reclaim-merged.sh --apply`，每移除一項留一行收據
   （路徑、branch、PR#、squash SHA）。
   它只動 `fleet.merged-artifact-cleanup` 授權內的東西：PR 已 MERGED、worktree 乾淨、ref 還停在
   被合併的那顆 commit。closed-unmerged、open、dirty、無 PR、同名多 PR、locked、detached、
   主 checkout 與 `.claude/worktrees/` 底下的 agent worktree 一律只列不碰（R7）；任何一項檢查
   出錯就降級成只列。還在跑的 lane 用 `--protect <worktree 目錄名或分支名>` 保住（可重複）。
   **lane 不跑這支**——§四第 3 條仍是「不刪分支、不刪 worktree」；回收是控制者的動作。
   這支腳本是過渡載體，產品家在 `edda fleet reclaim`（腳本檔頭載明）。
10. **收工**：`edda note "completed X; decided Y; next: Z" --tag session`；回報你：合了什麼、開了什麼、等你什麼。

---

## 四、Lane 的三件事

開工先裝 hooks（每個 worktree 一次）：`sh scripts/githooks/install.sh`——之後 L0 的
fmt／clippy／lint／size 閘由 pre-commit（bash 腳本；commit-msg 為 POSIX sh）／commit-msg 機器擋（1 MB 上限、staged `*.rs`/`Cargo.*` 跑
`cargo fmt --all --check`、touched `crates/*` 跑 clippy、staged `*.md` 跑 markdown lint、
conventional commit 格式；`SKIP_CLIPPY=1` 跳過 clippy 並自動在訊息尾巴補 `[skip-clippy]`）。
`cargo test -p <crate>` 不在 hook 裡——仍是手動 L0 步驟，CI 也會跑。`--no-verify` 全跳；
CI 只在 PR 與 push 到 main 時跑，feature branch 靠 PR 的 CI Gate。

1. 在指定 worktree 與分支上做 brief 說的那一件事——不 checkout main、不 pull、不開別的分支。
2. L0 閘（`cargo fmt --all --check`；`cargo clippy -p <crate> --all-targets -- -D warnings`；`cargo test -p <crate>`）。
   凍結 SHA 的 L1 是 exact-head CI；Windows CI 未覆蓋的 touched crate 由 verifier lane 以
   `CARGO_INCREMENTAL=0` 跑一次 `cargo test -p <crate>`（C5 selector），並記 gate receipt
   （SHA、CI run、toolchain、lane、結果）。不要以本機 workspace 全套代替 L1。
3. `git push -u origin <branch>`、開 PR、**停**。不合併、不刪分支、不刪 worktree。

Brief 必含：assigned build lane、verification budget（L0 while iterating；L1 once per frozen SHA）、cleanup authority（build cache 可清；worktree／branch／source 不刪）。

---

## 五、每個機制今天在哪、缺什麼

| 機制 | 今天用什麼 | 缺（單號） |
|---|---|---|
| 觀測 | `edda watch`、`edda peers`、`edda conduct status`、`gh pr checks`、`edda status` | dispatch lane 不在 peers（#569）；統一狀態面（#567）；孤兒回收（#573）；freshness（#604） |
| 進度追蹤 | issue 標籤（pending → ready → PR → merged）；`edda task new <title> --after <id> --assignee <label>`、`edda task start <id>`、`edda task done <id> --receipt "<可驗的話>" --evidence <path>`；PR 上的審查輪 | 成本與模型不進帳本（#582、#574） |
| 派發 | `edda dispatch --agent <claude|pi|codex> --prompt-file <f> [--session-id] [--cwd] [--budget-usd] [--timeout-sec] [--permission-mode] [--json]`；`edda conduct run <plan> --agent <x> [--cwd] [--dry-run] [--tmux] [--json]`；審查按需派一輪（`edda review --pr`，判決由審查者自己貼，見第 5 步） | 選模型/思考深度/工具(#574);角色 profile(#593);批次發射(`edda wave`,等 #576 與 #599) |
| 討論提問 | 你 ↔ 控制者對話；控制者 ↔ 其他 Claude session 用跨 session 訊息；對 lane 用 `edda request "<label>" "<msg>"`（門鈴；lane 沒心跳時要 `--force` 排隊）；耐久的寫 issue／PR 留言 | 事件驅動門鈴（#545）；lane 心跳（#569） |
| 決策 | `edda ask "<domain>"` → `edda decide "k=v" --reason "…"`（agent，unratified）→ `edda ratify <key>`（你） | 簽章身分（#609） |
| 開單 | `/issue-intake`、`/issue-create`（四問接線審計必填） | 批次進料與確認表（#599）；驗收端 wiring verdict（#594） |
| 通知 | 背景任務完成會叫醒控制者；`edda notify` 存在 | 事件驅動（#545） |
| 成本 | `--budget-usd`；plan 級 measured-ness（#533 已合） | 讀端報表（#582）；digest 成本 0.0 哨兵（#585）；conductor 散文成本（#584） |

**存在但本頁未驗證是否符合現行流程的動詞**：`edda intake`（外部任務進帳）、`edda prs`（掃 GitHub PR 事件）、`edda bundle`（審查 bundle）、`edda scan`（能力掃描）、`edda brief`（任務 brief 檢視）。用之前先 `--help` 並確認有讀者。

---

## 六、今天的硬規則（來源＝帳本決策，`edda ask` 可查全文）

| 規則 | 決策 key |
|---|---|
| 執行用便宜模型（pi 預設 glm-5.3-flash）；**審查一律 Claude Opus**（`claude-opus-5`，顯式 `--model` 釘死，正常臂走 `edda dispatch --agent claude` 訂閱運輸——本機 pi/openrouter 到不了任何 Anthropic 模型；brief 超出 spawn 上限的 fallback 走唯讀 allowlist 的 `claude -p` stdin，表頭印實際臂） | `fleet.review-engine-model`、`fleet.review-backend`（supersede `fleet.agent-model-split` 的審查半邊） |
| 審查 provider 過載：**改運輸不降模型**——(1) 同 `--model` 先用最低成本探測，通了才重試一次（同一 claude 訂閱運輸）；(2) 仍沒有判決就對該 head 標 `review:unreviewed` 並停——未審查是誠實狀態，便宜模型的判決不是。watcher 無 Codex 路線（superseding 決策 `…codex-route-withdrawn-for-automated-watcher`：Codex 對 watcher 做不到唯讀；人類控制者仍可手動用 Codex）。**2026-09-03 操作者裁決（`opus-default-sol-via-pi-fallback-no-codex`）：不是矛盾，是過時——Opus 是預設引擎，`fleet.review-engine-pool` 的錨仍是 sol（走 pi）；codex 自 `fleet.reviewer-agent` 起就不是審查運輸。watcher 自己不換模型，降到錨引擎是操作者動作** | `fleet.review-provider-overload` |
| **lane 啟動走 Task Scheduler，不走 nohup／Start-Process**：Claude Code 的工具 shell 在 Windows Job Object 裡，nohup 的子程序仍隨 session 死。`Register-ScheduledTask` + `Start-ScheduledTask`（父程序是 svchost）；該環境 `HOME` 為空，lane wrapper 必須顯式設；`CARGO_TARGET_DIR` 只在 `-BuildLane` 指名四個允許 build lane 之一時設（不編譯的 session 沒有 build lane——`.claude/CLAUDE.md`、`verification.cost-discipline`；要編譯的必須給四擇一，launcher 拒絕其他名字）。`lane-launch.ps1` 不合成 build lane：`-BuildLane` 只收 `worker-1|worker-2|verifier|verifier-2`，設 `CARGO_TARGET_DIR`＝lane root（`$env:LOCALAPPDATA\fleet-workstation\lanes`，可用 `FLEET_LANE_ROOT` 改）\`<BuildLane>`；docs lane 不傳，wrapper 不設。`Get-ScheduledTaskInfo` 可輪詢，`Unregister-ScheduledTask` 清理。重派前先讀 worktree／branch／PR 狀態，不信任 live handle。**手續已脚本化**：用 `scripts/fleet/lane-launch.ps1` 註冊起 lane、`scripts/fleet/lane-status.ps1` 盯狀態（用法見 START HERE），不要再手寫 wrapper | `fleet.lane-launch`、`fleet.lane-dispatch` |
| **停 lane 一律走 `scripts/fleet/lane-stop.ps1 -Name <lane>`**：`Stop-ScheduledTask` 與 `Unregister-ScheduledTask` 都只終止任務的 wrapper，**不殺它 spawn 的 process tree**（GH-672：被「停」的 lane 照樣 commit／push／開 PR，任務卻顯示 `State = Ready`）。`lane-stop.ps1` 停任務、殺整棵樹（wrapper 已死時依 `CommandLine` 比對 wrapper／brief 路徑抓孤兒）、驗證無殘留、回報實際終止了什麼，並補寫結束記錄（done-file + lane log 的 `=== EXIT ===` 行）——wrapper 本身也在 `finally` 寫同樣的結束記錄，所以正常結束、出錯、被停三種 endings 都有 EXIT。**殺完要驗共用 `.git/config`**：硬殺撞上 git 寫 config 會把它變成整片 NUL，主 checkout 加全部 worktree 同時失去 git；2026-09-02／03 各發生一次，而當時的 `.bak` 是**損毀後**才複製的，所以也是整片 NUL——備份不驗證等於沒有備份（GH-715）。殺完 `lane-stop.ps1` 驗證 config 仍可解析，壞了就從 `lane-launch.ps1` 開跑前存的**已驗證**備份還原（`scripts/fleet/git-config-guard.ps1`），還不回來就 exit 1；結束記錄一定先寫。沒有優雅關閉窗口：不帶 `/F` 的 `taskkill` 對沒有視窗的隱藏 console 程序無效（實測 exit 128、目標存活），加一段等待只會讓每次停 lane 多付秒數而擋不住任何損毀 | `fleet.lane-stop-4090` |
| Issue/program、work bundle/task、PR/release slice 分開；同一不穩 code chain 一個 owner，只有實際 artifact dependency 寫 `--after`；compile lane 依 session 實際需要配置，不按 issue 數機械切分 | `cleanup.parallel-exec`、`cleanup.review-gate` |
| 跨機器認領、釋放與已交付歷史：讀帳本 `fleet.cross-machine-claim`；起手守門的實作入口與拒絕條件見 `docs/fleet/rules.md` R21，本表不重述 | `fleet.cross-machine-claim`、#784（R21） |
| build lane 只用 `worker-1|worker-2|verifier|verifier-2`；永不建 ad-hoc `CARGO_TARGET_DIR`；L1 與 verifier 設 `CARGO_INCREMENTAL=0` | `verification.cost-discipline` |
| `/issue-pipeline` 是 legacy 薄入口：保留 issue／claim／`--skip-plan`／`--no-merge` 意義後轉 `delivery-flow/1`；只有真正並行 writers 才開 in-session formation，審查者不修自己判的 PR；長時間無人值守使用既有 authorized scheduler path | `fleet.parallel-modes=in-session-pipeline-when-operator-present-lanes-when-unattended` |
| 審查釘 full SHA；**每次 push 使前一個判決失效**；一個 PR 一個審查者身分 | `fleet.review-protocol` |
| 合併＝final current-head LGTM、P0=0/P1=0、required check「`CI Gate`」綠（`ci.merge-gate`）、SHA 窗檢查為空；docs-only PR 的 clippy／test job 顯示 skipped 而 `CI Gate` 仍綠＝`ci.path-filter` 正常跳過，不是漏跑 | `pr.merge-policy`、`ci.merge-gate` |
| 未合併的 worktree／branch／source 永不刪；已合併的（PR MERGED）由控制者依第 9 步回收；build cache 可清、按年齡回收 | `fleet.merged-artifact-cleanup` |
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

Fleet 使用 edda run -- <gate> 在乾淨的受審 SHA 記錄收據；edda review 讀取收據與 exact-head CI，reviewer 不重跑已涵蓋的 workspace gates。
