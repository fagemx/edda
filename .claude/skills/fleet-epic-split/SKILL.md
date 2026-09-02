---
name: fleet-epic-split
description: Use when refilling the Fleet feature queue — accepts four input shapes (epic issue, planning doc, ledger decision key, pasted planning summary), then recon → decompose (independence three-question) → dedupe → confirmation table → batch fleet:pending create → cross-references → provenance report. Never writes implementation code.
---

# Fleet Epic Split（出題器 A）

把一顆已簽核的目標拆成互相獨立、各自可 merge 的原子 fleet:pending 單。
你**只起草提案**，撕成 ready 是操作者的事。本 skill 於 #599 吸收了已退役的
plan-decompose：輸入形狀放寬、去重程序、建單前確認表、交叉引用、provenance
回連。慣例見 repo 自身的 `CLAUDE.md`／`AGENTS.md`（本 repo：`.claude/CLAUDE.md`）。

## 輸入（四選一；開工時先固定 provenance）

| 形式 | 內容 | provenance 記錄成 |
|---|---|---|
| `--epic <N>` | 一個 epic issue 的 body（`gh issue view <N>`） | issue 號＋引述的節名 |
| `--doc <path>` | 規劃文件（roadmap／gap analysis／設計文件的一節） | path＋行號範圍 |
| `--decision <key>` | `edda ask <key>` 的決策全文 | 決策 key |
| 貼上摘要 | 操作者在對話中貼的規劃文字 | 「operator 對話 YYYY-MM-DD」＋逐字引述 |

provenance 是 ⑦ 回報與每張單 Relation/footer 的回連目標——輸入沒記，後面就斷了。

## 開工前

- repo 根有 `FLEET_PAUSE` → idle 退出。
- 目標已有簽核（`fleet:goal` issue、operator 簽過的 epic、或操作者當面指示）。

## 七步（順序不可跳；每步有可核對的產出）

### ① recon（只准調查）→ 產出：帶 file:line 證據的現況報告
讀 repo 現況與輸入，產出報告：輸入要的東西，哪些**已經做完**（既有程式、
測試、輸入裡的 `[x]` 標記與實際碼是否相符——標了不算數，要碼在），哪些還沒。
**禁止**在這一段提解法或寫計畫。已完成的部分不生單。

### ② decompose（拆原子單）→ 產出：候選清單，每張標獨立性判定
把「輸入 − 已完成」的差距拆成原子單。每張**必過獨立性三問**：
1. 單獨 merge 會不會弄壞 main？
2. 跟別張單改同一批檔案嗎？
3. 不看其他單，能不能直接開工？
兩張搶同一檔 → 合併成一張，或標 `獨立性: blocked by #N` 明示依賴。
**禁止**：出「重構整個 X」這種巨單；出一個 session 做不完的單（做不完就再拆）。
**每輪上限 30 張**——超過就分輪，這一輪只取 30 張走完全程。

### ③ dedupe（去重）→ 產出：每張候選一行的 dedupe 紀錄
四道程序都要跑，並紀錄查了什麼、結果如何：
1. **文件內 `#N` 引用** — 輸入裡已指名的 issue 號 → 該項 `skip（已存在 #N）`，不生單。
2. **`edda ask "<domain>"`** — 命中已裁決範圍 → 不生單（紀錄命中的決策 key）。
3. **`edda search query "<keyword>"`** — 帳本／文件的既有軌跡。
4. **open-issue 模糊比對** — `gh issue list --state open --limit 200 --json number,title`
   對標題模糊比對；疑似重複在確認表標 `possible duplicate? #N` 交操作者裁。
每張候選一行：`<候選> | queries: … | verdict: new / duplicate of #N / merge with 候選k`。
沒有目標 repo／帳本可查的環境（例如純文字的隔離 dry run）：程序 2–4 記
`unavailable（無 repo/ledger 環境）`，確認表的 dedupe 欄標 `unverified`，
補查交給操作者在裁決時做。

### ④ 確認表（建單前必經）→ 產出：一張確認表
**操作者沒看過這張表，不准建任何 issue。** 一張擬建單一列：

| # | 標題 | Predicted surface（paths） | 獨立性 | dedupe verdict | 來源（輸入哪一段） |
|---|---|---|---|---|---|

表後附 skip 清單（重複項 → 指向的既有 issue）。等操作者回 `yes / adjust / cancel`；
adjust 就改表重出。**dry run**（操作者要求只看不建）：停在這一步——印確認表
＋每張擬建單的完整 body，不執行任何 `gh issue create`。

### ⑤ create（全部 `fleet:pending`）→ 產出：issue 號清單
每張 body 逐節照下方「Body 契約」填寫後：
`gh issue create --title "<動詞開頭>" --body-file <tmp> --label fleet:pending`
標題動詞開頭；body 內不帶 closing keyword。不使用 ready-bar 以外的任何
body 格式（舊並行格式已隨 plan-decompose 退役）。

### ⑥ 交叉引用 → 產出：comment 的連結
- batch 內依賴：`gh issue comment <N> --body "Blocked by #M. Related: #K"`——
  blocked-by 與 related 都要落成 comment，不能只寫在 body 裡。
- skip 但相關的既有 issue：到該 issue 留一則 comment（指回 provenance 與新單號）。

### ⑦ report（provenance 回連）→ 產出：回報清單
issue 號＋標題＋獨立性＋surface 一覽。每張單的 `Relation to existing issues`
節與結尾 footer 都回連 provenance（epic issue 號／`--doc` path:line／決策 key）；
回報本文本身也附 provenance 連結。

## Body 契約（唯一定義處）

**唯一定義在 [`issue-intake/templates.md`](../issue-intake/templates.md)**——
本檔不複製全文；寫單前先讀該檔，逐節照填。讀不到該檔的環境（隔離／純文字）：
仍須按下列固定的節順序逐節產出——節名與順序如下，內容自填，缺一節不准發。
節順序固定：

`What happened → Why it matters → Suspected surface → Predicted surface →
doneWhen → Wiring audit（四問槽）→ Relation to existing issues`

- `Predicted surface` = 這張單會**寫**的 paths＋symbols——parallel-wave Layer 1
  拿它做並行判定；列不出來 = 範圍還太糊，回 ② 重拆。
- `Wiring audit` 槽：引用或新增 code 的單，每個 component 一行四問
  （writer／reader／failure signal／layer reach），file:line 本 session 驗過。
- 缺任何一節不准發。

## 界線
你不撕 ready、不寫實作碼、不 merge、不在確認表之前建單。你的產出是「待簽的
提案」，下游有測試閘門兜底，但錯的**方向**沒有閘門攔得住——所以寧可少拆、
標清依賴，也不硬湊數量。
