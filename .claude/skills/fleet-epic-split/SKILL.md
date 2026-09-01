---
name: fleet-epic-split
description: Use when refilling the Fleet feature queue — decompose one signed-off goal or one section of an existing PLAN into atomic, independently-mergeable fleet:pending issues. Recon before decompose; never write implementation code.
---

# Fleet Epic Split（出題器 A）

把一顆已圈的目標（或一份既有 PLAN.md 的一節）拆成互相獨立、各自可 merge 的原子任務單。
你**只起草提案**（fleet:pending），撕成 ready 是操作者的事。慣例見 `fleet-playbook/internal/fleet-ops.md`。

## 開工前

- repo 根有 `FLEET_PAUSE` → idle 退出。
- 確認目標：一顆 `fleet:goal` issue，或操作者指定的 PLAN.md 一節/一個 epic 描述。

## 三段式（順序不可跳）

### ① recon（只准調查）
讀 repo 現況與目標，產出帶 **file:line 證據**的現況報告：目標要的東西，哪些**已經做完**（查
既有程式、測試、PLAN 的 `[x]` 標記與實際碼是否相符——標了不算數，要碼在），哪些還沒。
**禁止**在這一段提解法或寫計畫。已完成的部分不生單。

### ② decompose（拆原子單）
把「目標 − 已完成」的差距拆成原子單。每張**必過獨立性三問**：
1. 單獨 merge 會不會弄壞 main？
2. 跟別張單改同一批檔案嗎？
3. 不看其他單，能不能直接開工？
兩張搶同一檔 → 合併成一張，或標 `獨立性: blocked by #N` 明示依賴。
**禁止**：出「重構整個 X」這種巨單；出一個 session 做不完的單（做不完就再拆）。

### ③ propose（照模板起草）
每張單照 fleet-ops 六欄寫成 issue body（背景/改哪裡/doneWhen/verify/獨立性/尺寸），缺欄不准發。
建單：`gh issue create --title "<動詞開頭>" --body-file <tmp> --label fleet:pending,lane:feature`。
**每輪上限 30 張**。全部產完，回報清單（issue 號＋標題＋獨立性）給操作者裁決。

## 界線
你不撕 ready、不寫實作碼、不 merge。你的產出是「待簽的提案」，下游有測試閘門兜底，
但錯的**方向**沒有閘門攔得住——所以寧可少拆、標清依賴，也不硬湊數量。
