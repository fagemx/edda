Warning: No project session found with id 'microtest-gh599-new-20260902-164626'; creating a new session with that id.
環境查核完成：作業目錄為隔離空目錄（無 `.git`、無 repo 檔案）、無 `FLEET_PAUSE`、`templates.md` 讀不到（啟用隔離環境的固定節順序 fallback）。以下逐字印出本 skill 在此輸入下的每步輸出。

---

## 開工前

- `FLEET_PAUSE`：不存在 → 不 idle 退出。
- 簽核來源：操作者當面指示（對話貼上規劃摘要＋明示 dry run）→ 視為已簽核，可開工。

## Provenance（固定）

> **形式：貼上摘要（四選一之第四）**
> provenance 記錄成：「operator 對話 2026-09-02」＋逐字引述：
>
> "**Stage 2 — event-driven delivery:**
> phase terminal-state notifications through edda-notify channels (filed), with
> #545 (gate-entry routing) as sibling. Acceptance: a controller reacts to
> phase/gate events without polling stdout.
>
> Dry run: do not create issues. Recon is unavailable here — treat every item in the input as not yet done."

---

## ① recon（只准調查）→ 帶 file:line 證據的現況報告

**環境限制**：作業目錄為隔離空目錄（`/tmp/tmp.2KdWZAOiF6`，無 `.git`、無原始碼），操作者明示 recon 不可用並指示「輸入中每一項都視為尚未完成」。據此產出現況報告：

| 輸入要求的東西 | 現況 | 證據 |
|---|---|---|
| phase 終態通知經 edda-notify 既有已 filing 的 channels 送出 | **視為未完成**（操作者指示） | 無 repo 可驗碼 — `unavailable（無 repo/ledger 環境）` |
| 與 #545（gate-entry routing）為 sibling 關係 | #545 視為**已存在**的 sibling issue | 輸入明文指名 `#545` |
| 驗收：controller 對 phase/gate 事件做出反應、不輪詢 stdout | **視為未完成**（操作者指示） | 無 repo 可驗碼 |

本節不提任何解法、不寫計畫（界線遵守）。已完成清單：空 — 因此 ② 的差距 = 輸入全文。

---

## ② decompose → 候選清單（含獨立性判定）

「輸入 − 已完成」= 輸入全文。拆成 **2 張**原子單（寧可少拆，不硬湊數量）：

**候選 A：Emit phase terminal-state events through the filed edda-notify channels**
- 範圍：在 phase 抵達終態的發射點，把終態事件送上 edda-notify 既有的 filed channels；事件契約（schema）由本單定義與擁有。
- 獨立性三問：
  1. 單獨 merge 會不會弄壞 main？→ 不會：只新增發射路徑，無消費者時事件無人接聽，main 行為不變。
  2. 跟別張單改同一批檔案嗎？→ 不會：A 動 phase 終態發射點＋channel 送出版；B 動 controller 訂閱端。事件契約檔由 A 獨擁，B 只讀 → 不搶檔。
  3. 不看其他單，能不能直接開工？→ 能：只依輸入描述與既有 filed channels。
- 判定：**獨立：可獨立 merge**。

**候選 B：Make a controller react to phase/gate events without polling stdout**
- 範圍：controller 端改為訂閱事件（phase 事件來自 A 的契約；gate-entry 事件由 #545 路由），移除/繞過 stdout 輪詢路徑。
- 獨立性三問：
  1. 單獨 merge 會不會弄壞 main？→ 不會：訂閱一個尚未有發射者的契約不破壞現行為（fallback 保留至 doneWhen 驗收）。
  2. 跟別張單改同一批檔案嗎？→ 不會（見 A 問 2）；gate 端改動歸 #545，本單只做消費端。
  3. 不看其他單，能不能直接開工？→ 能對 A 的**事件契約**開工，但 doneWhen 驗收需 A 已 merge 才可端到端驗證。
- 判定：**獨立性：blocked by #A（執行期依賴，非檔案衝突）** — 建單後以 comment 明示。

禁則自查：無「重構整個 X」巨單；兩張皆可單 session 完成；未超過 30 張上限，單輪走完。

---

## ③ dedupe → 每張候選一行的 dedupe 紀錄

四道程序（逐道記錄查了什麼、結果如何）：

1. **文件內 `#N` 引用**：輸入指名 `#545`（gate-entry routing）→ 判定為**既有 sibling/依賴**，非本批候選的重複項；gate-entry routing 本身**不生單**（已存在 #545）。候選 A、B 的主體（phase 終態通知、controller 消費端）在輸入中無對應已存在的 `#N`。
2. **`edda ask "<domain>"`**：`unavailable（無 repo/ledger 環境）` — 隔離目錄無帳本可查。
3. **`edda search query "<keyword>"`**：`unavailable（無 repo/ledger 環境）`。
4. **open-issue 模糊比對**（`gh issue list …`）：`unavailable（無 repo/ledger 環境）` — 本目錄非 git repo，`gh` 無目標 repo。

逐候選紀錄：

```
候選A | queries: in-doc refs(#545→sibling, 非重複), edda ask unavailable, edda search unavailable, gh issue list unavailable | verdict: new (unverified)
候選B | queries: in-doc refs(#545→sibling/依賴, 非重複), edda ask unavailable, edda search unavailable, gh issue list unavailable | verdict: new (unverified)
```

---

## ④ 確認表（dry run → 停在本步，不建任何 issue）

| # | 標題 | Predicted surface（paths） | 獨立性 | dedupe verdict | 來源（輸入哪一段） |
|---|---|---|---|---|---|
| 擬建-1 | **Emit phase terminal-state events through filed edda-notify channels** | phase 終態發射點（file:line 待 recon 補）；edda-notify channel 送出版；事件契約檔（本單擁有） | 可獨立 merge | new（**unverified**） | 「phase terminal-state notifications through edda-notify channels (filed)」 |
| 擬建-2 | **Make a controller react to phase/gate events without polling stdout** | controller 事件訂閱端；controller 輪詢路徑的移除/降級點（file:line 待 recon 補） | **blocked by 擬建-1**（執行期依賴）；gate 端歸 #545 | new（**unverified**） | 「a controller reacts to phase/gate events without polling stdout」＋「#545 (gate-entry routing) as sibling」 |

**Skip 清單**：

| 項目 | 處置 | 指向 |
|---|---|---|
| gate-entry routing | skip（已存在，本批不生單） | #545 |

> **Dry run 指示生效：停在這一步。** 以下為每張擬建單的**完整 body**（`templates.md` 讀不到 → 按隔離 fallback 的固定節順序逐節產生；`Predicted surface` 的 file:line 與 Wiring audit 的驗證均標記待 recon 補驗）。

---

### 擬建-1 body

**Title**: Emit phase terminal-state events through filed edda-notify channels

**What happened**
Phase 抵達終態時，目前沒有事件經 edda-notify 既有的 filed channels 對外送出；下游無從被動得知 phase 已結束。

**Why it matters**
Stage 2（event-driven delivery）的基礎：沒有發射端，controller 只能輪詢 stdout。本單提供 Stage 2 的事件來源。

**Suspected surface**
Phase 生命週期／終態判定所在的元件；edda-notify 的 channel 送出層（filed channels 既有）。確切 file:line 待 recon。

**Predicted surface**
- phase 終態發射點：於該處呼叫 channel 送出（paths 待 recon 後補 file:line）
- edda-notify channel 送出版（新增呼叫，不改 channel 本身）
- 事件契約（schema）檔：**本單新增並擁有**，供擬建-2 與 #545 生態讀取

**doneWhen**
一個 phase 進入終態後，其對應事件出現在 filed edda-notify channel 上，內容足以辨識 phase 身分與終態種類；既有非事件路徑行為不變。

**Wiring audit（四問槽）**
- `phase terminal-state emitter` — writer：本單（新增送出呼叫，file:line 待 recon 驗證）；reader：edda-notify filed channel（既有）；failure signal：channel 送出失敗須可觀察（不靜默吞掉）；layer reach：phase 執行層 → 通知層。
- （file:line 均未於本 session 驗證 — 隔離 dry run，無 repo；建單前由操作者/recon 補驗。）

**Relation to existing issues**
Sibling: #545（gate-entry routing）——本單只負責 phase 終態事件，gate 進入事件的路由歸 #545。本單為擬建-2 的事件來源。

---
*(footer)* Provenance: operator 對話 2026-09-02 — 「Stage 2 — event-driven delivery: phase terminal-state notifications through edda-notify channels (filed), with #545 (gate-entry routing) as sibling.」逐字引述見 skill 執行紀錄。

---

### 擬建-2 body

**Title**: Make a controller react to phase/gate events without polling stdout

**What happened**
Controller 目前靠輪詢 stdout 得知 phase/gate 進度；事件送到 channel 後仍無消費者反應。

**Why it matters**
Stage 2 的驗收主體：讓 controller 成為事件驅動，輪詢只是歷史路徑。

**Suspected surface**
Controller 的事件處理／訂閱端；現行 stdout 輪詢迴圈。確切 file:line 待 recon。

**Predicted surface**
- controller 訂閱端：消費擬建-1 的 phase 事件契約與 #545 路由的 gate-entry 事件（paths 待 recon 後補 file:line）
- controller stdout 輪詢路徑：降級為 fallback 或移除（paths 待 recon 後補 file:line）

**doneWhen**
Controller 在不讀取 stdout 的前提下，對 phase 終態事件與 gate 事件做出正確反應（端到端驗證需擬建-1 已 merge）。

**Wiring audit（四問槽）**
- `controller event subscriber` — writer：擬建-1（phase）與 #545（gate-entry）；reader：本單（file:line 待 recon 驗證）；failure signal：事件遺失/逾時須可偵測（不得靜默卡死）；layer reach：通知層 → controller 決策層。
- （file:line 均未於本 session 驗證 — 隔離 dry run，無 repo。）

**Relation to existing issues**
Blocked by 擬建-1（事件來源；執行期依賴，非檔案衝突）。Sibling: #545（gate-entry routing 的發射端）。

---
*(footer)* Provenance: operator 對話 2026-09-02 — 「Acceptance: a controller reacts to phase/gate events without polling stdout.」逐字引述見 skill 執行紀錄。

---

## ⑤ create — **未執行（dry run）**

若非 dry run，本步將逐張以 `gh issue create --title "<動詞開頭>" --body-file <tmp> --label fleet:pending` 建單，並產出 issue 號清單。dry run：**零個 `gh issue create` 呼叫**。

## ⑥ 交叉引用 — **未執行（dry run）**

若非 dry run，將執行：
- `gh issue comment <擬建-2號> --body "Blocked by #<擬建-1號>. Related: #545"`
- `gh issue comment <擬建-1號> --body "Related: #<擬建-2號>. Sibling: #545 (gate-entry routing)"`
- 到 #545 留 comment：指回 provenance（operator 對話 2026-09-02）與兩張新單號（Stage 2 事件驅動交付批次）。

## ⑦ report — **未執行（dry run）**

若非 dry run，回報清單將為：

| Issue | 標題 | 獨立性 | Surface |
|---|---|---|---|
| （dry run：無號） | Emit phase terminal-state events through filed edda-notify channels | 可獨立 merge | phase 終態發射點、edda-notify 送出版、事件契約檔 |
| （dry run：無號） | Make a controller react to phase/gate events without polling stdout | blocked by 上一張 | controller 訂閱端、輪詢路徑移除/降級 |

回報本文將附 provenance 連結（operator 對話 2026-09-02 ＋逐字引述）。

---

**Dry run 終止。** 操作者裁決點：① 確認表兩張單的範圍/依賴是否 `yes / adjust / cancel`；② 兩張 dedupe verdict 均為 **unverified** — 裁決前請在有 repo/ledger 的環境補跑 ③ 的程序 2–4 與 #545 對照；③ `Predicted surface` 的 file:line 需 recon 後補齊，補不出來的單回 ② 重拆。
