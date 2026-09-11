# Gap closure：哪些是事實，哪些選擇已補齊？

> Status: proposed; evidence pinned to the overview basis, not live process truth.
> Purpose: prevent the executable cards from depending on unstated design choices.

## 1. Grounded findings

| Gap | Evidence | Closed design choice | Owner card |
|---|---|---|---|
| 全批階段 barrier | main `.claude/skills/issue-pipeline/SKILL.md` Phase 1–3 / execution rule 4 要等全員 | 以真實 prerequisite 推進；失敗只影響該 bundle 與必需 successors | A1 |
| 一 issue 一 fresh agent 導致上下文切碎 | 同 skill 的 per-issue planning、implementation、fresh fixer | 相同 call chain 用同一 owner；issue/task/PR 分開定義 | A1 |
| Review continuity callers 漂移 | pipeline 返工命令沒有 `--resume`；`cmd_review/prepare.rs` 支援 prior session | 能 resume 就同 reviewer；不能時明示 replacement，不重用空 UUID 假裝延續 | A2 |
| 隔離語義混淆 | REVIEW §1 允許 private ref；`cmd_review/mod.rs::run_with` 仍 create WorktreeGuard | direct reader 不需 worktree；產品自己的 guard 不拆；不額外包第二層 | A2 |
| Product facts carrier 缺失 | `prepare::assemble` 沒有作者 context 輸入；reviewer 無 shell/gh，不能自行取得任意 PR handoff | 補 optional --context-file，bounded data-only，直接進 brief 與既有 notes；不充當 trusted spec | A2 |
| 舊本機 freeze gate 指示仍在 | 兩份 tracked coord skills 指示 implementer 每 frozen SHA 跑 full gate set | 明確改成 implementer focused L0、L1 exact-head CI、verifier focused C5；fixture 防回歸 | A1 |
| 文書與缺陷等級混用 | REVIEW U3 自稱 convention miss 仍 P1；PR1147 已有 Closes 仍命中 | 只把 U3 exact-line convention 改 advisory；其他規則原樣 | A3 |
| REVIEW prompt 約 58 KB | `brief::assemble` 完整嵌入 base REVIEW.md，diff budget 不限 spec 長度 | 先減 caller 儀式；規則投影不在本輪，避免新 compiler/安全漏讀 | deferred |
| S10-before-carve 延後全部功能 | GH1141 plan §16 / S10 depends S1–S9 | owner 將 whole-program proof 與 usable-slice landing 分開 | B1 |
| 已有成果尚未發布 | continuity train 的 program.md 記錄 S1/S2 local LGTM、無 exact-head CI | B2 採既有 commits 加 current-base consumer 驗證，不重做功能 | B2 |
| Flash 沒有可比較的交付證據 | 大多 review elapsed/cost 未量測；S8 proof 在龐大 S6 後才被關注 | 一次有界歷史 bug trial，所有 failure/retry 都列入；不阻擋功能上市 | C1 |
| 舊 merge caller 殘留 | pipeline Phase 4 教直接 `gh pr merge --squash` | controller 只使用 canonical `edda review merge`，保留 R6 checks | A1 |

## 2. Evidence snapshots

本輪 repo basis：`cbafe8fad409cfad6523d4a12d4226bb3c316d30`。
Main CI `34598223091` 和 contract `34598223081` 成功；這不是新 planning files 的 CI。

GH1141 local integration reference：`f8e91dfa27deeee238e7cd4d6ff2a907f7478187`。
其 [program receipt](https://github.com/fagemx/edda/issues/1141) 與本機
`git show f8e91dfa27deeee238e7cd4d6ff2a907f7478187:docs/superpowers/continuity/program.md`
需一起看：GitHub issue checklist 有落後現場的風險，不能只讀它推斷未實作。

完整讀取的 operator 指定 plan 位於：
`C:/ai_agent/edda-worktrees/context-save-edda-plan/docs/superpowers/plans/2026-09-11-context-save-edda-integration.md`。
讀取 snapshot SHA256：`ea9a437b3f5354ae9747d2472fe0da27a1a5765598a2e9d608f3a80f5da69378`。
執行前檢查 plan 是否更新，不覆寫新的 owner 決定。

## 3. Review sample: distinguish wait from review execution

| PR | Unique round/head pairs | Open-to-merge minutes |
|---|---|---:|
| 1137 | 1 | 24.3 |
| 1140 | 1 | 72.5 |
| 1142 | 1 | 38.8 |
| 1143 | 2 | 59.1 |
| 1146 | 1 | 32.3 |
| 1147 | 2 | 60.3 |

PR1147 的 Round 1 有兩則重複留言，不能算第三輪。
Open-to-merge 混合執行、等待、CI、返工，不能宣稱全是 review 耗時。
PR1147 還有 production expect blocker，所以不能把整輪成本歸給 U3。

## 4. Real boundaries that remain

Task #139 的 S5 findings 含 acceptance authority、typed completion correlation、
ACP attempt race、reconcile sandbox/scope 和 SDK spec pin。#140 正在處理前四項；
#141 的 SDK 接合等待 #76 owner handoff。這些不是可刪文書。

B2 不能假設 continuity 的不同 event 不需要 SDK 整合：必須獨立核對該 slice
自己的 registry/spec/SDK consumer，使用既有 owner 完成 producer-consumer 對接。
不等待所有 S5 完成，也不繞過自己暴露的 contract defect。

## 5. Remaining inputs, not missing architecture

| Input | Bind when / by whom | If unavailable |
|---|---|---|
| Current main SHA / applicable CI | 每張卡開始，controller | 先做 read-only ground check；不使用舊綠燈宣稱新 head |
| Active owner / path permission | B1 現有 continuity controller，SDK 由 #76/#141 owner | 只停接合工作；A/C 繼續 |
| Usable release cut adoption | B1 在 #1141 durable record 與 owner plan | 保留候選，不擅自 carve/publish；不等其他程序批准 A |
| Flash provider/model/runtime/version | C1 操作者現有設定，由 controller 明示 | report unavailable；不猜 model ID 或讀出 secret |
| Timeout / funded call allowance | C1 啟動前 controller 綁定 | 不開 paid run；完成 offline preparation |
| Native reviewer session still exists | A2／每次續審 | 明確 replacement，讀 prior facts，不能偽造 resume |

## 6. Decision status

這些是使用者要求的 spec/plan 選擇，不是對現行 REVIEW/R6 的即刻 override。
A1 流程修改、A2 可選資料輸入與 A3 policy delta 可分開交付。A3 無 A2 artifact 依賴。
B1 是現有 program 的具體
release amendment，不要求新 program 或逐張 issue 的再次批准。

相關定義：[SPEC.md](SPEC.md)、[CONTRACT.md](CONTRACT.md)。
