# Delivery-first：讓工作向前，不讓流程彼此等待

> Status: proposed implementation plan; not an active policy override.
> Basis: `cbafe8fad409cfad6523d4a12d4226bb3c316d30` (origin/main inspected 2026-09-11).
> Scope: existing workflow simplification, first usable continuity delivery, one bounded Flash pilot.

## Goal

使用者交辦一個結果，agent 保留脈絡完成它；需要協助時交出具體例外，
而不是因同批其他任務、文書慣例、全新 session 或未完成平台而停住。

成功不是「更多 agents」或「更多 PR」，而是可接受成果、較少人工介入，
以及可解釋的等待與成本。數值改善尚未量測，不預先宣稱節省比例。

## Read order

1. [GAPS.md](GAPS.md)：已確認事實、補齊的選擇與仍需現場綁定的輸入。
2. [SPEC.md](SPEC.md)：工作單位、滾動推進、兩段自審與共享審查事實。
3. [CONTRACT.md](CONTRACT.md)：安全／相容界線及最小資料輪廓。
4. [TRACKS.md](TRACKS.md)：六張任務卡、真實依賴、owner 與檔案地圖。
5. [VALIDATION.md](VALIDATION.md)：對照案例、命令、證據與停止條件。

## What this is NOT

- 不是新 scheduler、control API、ledger event、認證系統或模型資格制度。
- 不改變現行 R6、SHA-pinned independent verdict、CI Gate 或不可逆安全界線。
- 不把兩段自審、facts pack、pilot 或本文件變成新的 required check。
- 不接管 GH1141 的 active worktrees，也不重做已完成的 S1/S2/S4。
- 不要求小修正套用大型 program，不批量預開 child issues。

## Tracks and rolling DAG

```text
A1 cohesive / rolling workflow -> A2 optional context / resume -> C1 Flash pilot
A3 U3 convention (independent)
B1 owner-adopted usable release cut -> B2 continuity slice delivery

A3、B1 與 A1 可各自推進；C1 不等 A3/B2/node/control/S10。
同一 owner 的容量影響排程，不虛構 artifact dependency。
```

| Track | Tasks | First outcome | Dependencies | Status |
|---|---|---|---|---|
| A Flow | A1, A2, A3 | 滾動流程、review 可選 context/resume、U3 提示 | A2 consumes A1；A3 independent | planned |
| B Continuity | B1, B2 | 已有本機保存／恢復與原生 skills 可被交付 | B1 需 existing owner 採納 release amendment | owner handoff pending |
| C Flash | C1 | 一件真實歷史 bug 的有限範圍執行證據 | A2；runtime/model/budget 現場綁定 | planned |

六張卡是可調整工作包，不是六個必須新建的 rail tasks 或 GitHub issues。
目前只有文件任務 #142；implementation 尚未開始。

## Product boundary

Edda 保留既有執行與證據能力；本計畫先改「如何使用它們」。
`edda review` 仍擁有自己的 guarded worktree，直接 read-only review 可以讀 immutable ref。
不把兩條路誤寫成同一種生命週期，也不在產品外再包一個 review worktree。
A2 唯一新增產品介面是可選的 `--context-file`；共享 facts 不靠 reviewer 自行跑 gh。
完整 control/node 能力仍由 GH1141 繼續交付，不作為本輪全部功能的前置条件。

## Promotion memo: spec to executable work

- Stable: issue/task/PR 不必一對一；cohesive bundle；shared facts 不等於 shared verdict；
  usable slice 不等於 whole-program completion。
- Canonical cycle: [SPEC.md](SPEC.md)；單一定義：[CONTRACT.md](CONTRACT.md)。
- Concrete closure: A/B/C 三個 bundle 與兩輪 review 的案例見 SPEC、VALIDATION。
- Promoted now: A1 guidance/fixture、A2 optional review context/resume、A3 U3-only policy。
- Conditional execution: B1/B2 的跨 owner release 協調；C1 的有界 runtime trial。
- Not promoted: 任意 REVIEW 規則投影、通用控制平面、跨機接管、全面降級 P0/P1。
- 這是可執行的 spec scenario，不是已跑成功的 runtime demo。

## Progress

- [x] Read code, procedures, six PR histories and GH1141 plan/receipts.
- [x] Close design gaps; define examples and failure behavior.
- [x] Produce six executable cards with ownership, verification and delivery units.
- [ ] A1 / A2 / A3 implementation and current-head acceptance.
- [ ] B1 owner adoption / B2 usable slice delivery.
- [ ] C1 pilot and factual result report.

## Local delivery status

本 planning pack 不新增 runtime gate，也不自行生效。A3 改 REVIEW 時按 base spec 審查；
B1 由 continuity owner 採納既有 program 的 release 修訂後，B2 才動作。
其餘無關工作不等待這些局部條件。完成文件驗證不等於產品功能已完成。
