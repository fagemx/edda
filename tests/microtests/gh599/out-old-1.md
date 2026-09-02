Warning: No project session found with id 'microtest-gh599-old-20260902-164626'; creating a new session with that id.
Dry-run 環境檢查：working directory 是空的 temp dir，沒有 repo、沒有 `FLEET_PAUSE`、沒有 `.claude/CLAUDE.md`。以下印出此 skill 在此輸入下每一步會產出的內容（**不建單、不執行 `gh`**）。

---

## 開工前

```
[pre-flight] FLEET_PAUSE 檢查……不存在 → 繼續。
[pre-flight] 慣例檔 .claude/CLAUDE.md 查無（非 repo 環境）→ 慣例以 skill 內建為準。
[pre-flight] 目標確認：操作者指定之 epic「Stage 2 — event-driven delivery」。
             兄弟單 #545（gate-entry routing）已 filed，屬既有範圍，不重拆。
```

---

## ① recon（只准調查）

> ⚠️ Dry run：recon 無法執行（無 repo 可查）。依指示，輸入中的每個項目一律視為**尚未完成**。以下本應附 file:line 證據之處，以 `〈待 recon 補 file:line〉` 標記——真實跑這步時缺證據不得進入 ②。

```
目標：phase 終態（terminal-state）通知走 edda-notify 既有通路（filed channels），
      #545（gate-entry routing）為兄弟單。
驗收：controller 能對 phase/gate 事件做出反應，且不需輪詢 stdout。

現況報告：
- phase 終態 → edda-notify 通路：〈未做〉（dry run：視為未完成，無碼可查）
- controller 免輪詢的事件接收路徑：〈未做〉（同上）
- gate-entry 事件路由：已由 #545 涵蓋 → 不生單，僅作依賴引用。
已完成部分：無（dry run 模式）→ ② 的差距 = 整個目標 − #545 已涵蓋的 gate-entry。
```

---

## ② decompose（拆原子單）

差距拆成 2 張。每張過獨立性三問；因 recon 不可用、無法確認檔案不重疊，依「寧可少拆、標清依賴」原則，第二張明示 blocked-by，不硬湊第三張。

```
差距 → 原子單：
  A. phase 終態事件接上 edda-notify 通路（發送端）
  B. controller 免輪詢接收 phase/gate 事件（接收端）

獨立性三問檢核：
[A] 1. 單獨 merge 會弄壞 main？ 不會——只加發送掛點，不改既有行為。
    2. 跟別張搶同一批檔案？ 可能與 B 共用事件格式/通路定義 → 不合併，B 標依賴。
    3. 不看其他單能直接開工？ 能（#545 已定義通路慣例可參照）。
[B] 1. 單獨 merge 會弄壞 main？ 不會——新增接收端 helper，無人呼叫即無影響。
    2. 跟別張搶同一批檔案？ 事件格式與 A 重疊 → 獨立性: blocked by A。
    3. 不看其他單能直接開工？ 不能——需 A 產出的事件存在（gate 側則依賴 #545）。

禁則檢查：無「重構整個 X」巨單；兩張皆為單 session 可完成之尺寸。✓
```

---

## ③ propose（照模板起草）

以下為兩張 issue body 草稿與**將會執行**（dry run 不執行）的建單指令。`〈待 recon 補 file:line〉` 欄位在真實運行中未補齊前**不准發**（缺欄不准發）。

**Issue A**
> **Title:** `接上 phase 終態通知到 edda-notify 通路`
> **Labels:** `fleet:pending`, `lane:feature`

```markdown
## 背景
Stage 2 — event-driven delivery：phase 抵達終態（完成/失敗）時，目前
〈待 recon 補 file:line：現況無事件發出〉。目標是讓終態事件走 edda-notify
既有 filed channels，與 #545（gate-entry routing）同為事件遞送的一部分。

## 改哪裡
〈待 recon 補 file:line：phase lifecycle 終態判定處 + edda-notify 通路介面〉。
只加發送掛點，不動 #545 的 gate-entry 路由。

## doneWhen
phase 進入任一終態時，edda-notify 對應 channel 收到一筆含 phase 識別與
終態結果的通知；gate-entry 行為（#545）不受影響。

## verify
〈待 recon 補：跑一個 phase 至終態的具體指令/測試〉——觀察 channel 檔案/
通路出現該通知；既有 #545 測試仍綠。

## 獨立性
可獨立 merge。B 需依賴本單；本單不依賴 B。

## 尺寸
S–M（單 session 可完成）。
```

```
gh issue create --title "接上 phase 終態通知到 edda-notify 通路" \
  --body-file <tmp> --label fleet:pending,lane:feature
```

**Issue B**
> **Title:** `提供 controller 免輪詢反應 phase/gate 事件的接收路徑`
> **Labels:** `fleet:pending`, `lane:feature`

```markdown
## 背景
Stage 2 驗收：controller 要能對 phase/gate 事件反應，且**不輪詢 stdout**。
A（本輪 issue）供 phase 終態事件，#545 供 gate-entry 事件；缺一個接收端，
controller 仍得自己盯 stdout。

## 改哪裡
〈待 recon 補 file:line：接收端 helper/CLI 的落點，讀 edda-notify 通路〉。

## doneWhen
一個 controller 以本接收路徑等到 phase 終態事件（及 #545 的 gate 事件）並
做出反應，全程未讀取 stdout。

## verify
〈待 recon 補：以 controller 腳本訂閱事件並斷言反應的具體指令/測試〉。

## 獨立性
**blocked by A（本輪 phase 終態發送單）**——事件格式與其重疊；gate 事件面
另依賴已 filed 的 #545。不得在 A merge 前撕 ready。

## 尺寸
S（單 session 可完成）。
```

```
gh issue create --title "提供 controller 免輪詢反應 phase/gate 事件的接收路徑" \
  --body-file <tmp> --label fleet:pending,lane:feature
```

---

## 回報清單（dry run：未建立，僅提案）

```
本輪提案 2 張（上限 30 ✓），待操作者裁決：
  #A  接上 phase 終態通知到 edda-notify 通路            獨立性: 無阻塞
  #B  提供 controller 免輪詢反應 phase/gate 事件的接收路徑  獨立性: blocked by #A（gate 面依賴 #545）

備註：
- 兩張的「改哪裡/verify」含〈待 recon 補 file:line〉佔位——真實運行需先補齊證據才可發單。
- Dry run：未執行任何 gh issue create，未撕 ready，未寫實作碼。
- 刻意只拆 2 張：接收端與發送端共用事件格式，拆更細只會增加協調成本（寧少拆、標清依賴）。
```
