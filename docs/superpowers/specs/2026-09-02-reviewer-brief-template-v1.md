# 審查 brief 模板 v1（substitutable reviewer 判準包）

- 日期：2026-09-02
- 狀態：提案（本單 #618 交付；操作者裁定後，搬進 fleet-review skill 是後續單——與 #598／#594 同檔）
- 上游裁定：`fleet.review-engine=replaceable-by-qualification-not-brand`、
  `fleet.review-brief-framing=validation-checklist-not-attack-plan`、
  `fleet.claude-subscription-transport=claude-code-only`（`edda ask` 2026-09-02）
- 讀者：審查控制者（把本模板填成一次審查的 brief），與任何被派審查的引擎。

---

## 0. 這份模板是什麼

判準包（brief）是把「審查的線」從引擎身上拆下來的載體：
**零裁量清單＋`[判斷]` 標籤＋證據門檻＋判決格式**。同一份 brief 在該類別
合格的任何引擎上跑，結果應該等價——引擎可替換，線不變。

使用方式：控制者複製 §5 的模板，填入 §6 的類別檢查清單（可並列多類）、
PR 資訊與範圍，派給引擎池中選定的引擎（引擎池與替換規則見設計文件 §2–§3）。

## 1. 零裁量規則（這是規則，不是建議）

> **對審查範圍內每一個反引號包住的 `edda <字>`（以及審查對象自身引用的
> CLI 名稱），一律實際執行 `edda <字> --help` 並在判決中回報 exit code。**
>
> 禁止寫「被描述為會產生效果」「文件宣稱可以」「應該會」之類的轉述。
> 沒有跑過 `--help`，就不准對該指令的行為下任何結論。
> exit code 非 0 即為 finding（實證：`edda wave --help` → exit 2，
> #616 Round 1 中 glm 把 `edda wave 展開器` 當成功能名照抄，漏了這個 P1）。

同一規則推廣到文件宣稱的其他可驗證事實：旗標存在性（對照 `--help`）、
路徑存在性（實際 ls/read）、帳本狀態（實際 `edda ask`／讀帳本檔）。
**宣稱 vs 量測：brief 只接受量測過的宣稱。**

## 2. `[判斷]` 標籤規則

- 檢查清單項目凡是**需要裁量**（沒有機械判定步驟、要靠模型自行判斷好壞）的，
  在 brief 中標 `[判斷]`。零裁量項目不得標 `[判斷]`——能寫成機械步驟的就寫成機械步驟。
- **清單型引擎遇 `[判斷]` 項只能標「需升級」（needs escalation），不准自行裁定。**
  升級項送合格強引擎（目前＝sol）抽審，只審那些項
  （裁定 `fleet.review-engine`：「[判斷] 項由清單型引擎標需升級，送合格強引擎抽審」）。
- 引擎不得把 `[判斷]` 項悄悄當成 RAN；判決的 escalations 欄必須列出全部需升級項。

## 3. 證據門檻

- 每個 finding 必須附：檔案:行（或指令輸出）＋讓人能重現的證據。
  只有主張沒有證據 = 不收。
- 安全相關的檢查以**程式必須成立的性質**與**必須確認的輸入形狀**表述，
  不寫成攻擊計畫（裁定 `fleet.review-brief-framing`——攻擊框架會被 provider
  拒絕，靜默燒掉一輪審查）。
- severity：P0＝會造成破壞／資料損失／違反權限邊界，必擋；
  P1＝錯誤宣稱、不存在的介面、明確缺陷，擋合併；
  P2＝品質建議，不擋。

## 4. 唯讀約束

審查是唯讀角色。運輸層能強制的就運輸層強制
（pi：`--exclude-tools edit,write`；claude：`--allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)"`；
#574 落地前這是**旗標層**約束，dispatch 接線是後續單），brief 文字只作為第二層：
不編輯產品檔、不 push、不合併、不改帳本。

## 5. 模板本體（控制者複製此節填空）

```text
你是唯讀審查員。審查目標：<PR 連結 / diff 範圍 / SHA>，類別：<class>。

範圍（IN SCOPE）：<changed behavior/paths; direct callers/consumers; issue/spec
驗收; 引入或暴露的安全與資料損失回歸; current-base 整合>。範圍外的發現記為
FOLLOW-UP ISSUE，不擴大本輪。

檢查清單（見 §6 對應類別；逐項回報 RAN / 需升級 / N.A.）：
<貼上 §6 清單>

規則：
1. 零裁量：範圍內每個反引號 `edda <字>`（及審查對象引用的 CLI），一律跑
   `<cmd> --help` 並回報 exit code；未量測不得下結論。
2. 標 [判斷] 的項目：若你是清單型引擎，只能標「需升級」，不得自行裁定。
3. 唯讀：不編輯、不 push、不合併、不改帳本。
4. 證據門檻：每個 finding 附 檔案:行 或指令輸出；安全檢查以性質表述。
5. 非平凡 diff 零發現時，明確寫「零發現」與你檢查過的項目清單，不得留空。

判決格式（照抄此節，替換角括號）：

## Code Review: Round N
- elapsed: <pi session first-to-last message elapsed_ms; unmeasured if unavailable>
- model_requested: <dispatch 指定的模型>
- model_observed: <由系統取得：pi 讀 session 檔的 "model" 欄；claude 讀
  `claude -p --output-format json` 的頂層 **modelUsage** 鍵（其 key 就是模型 id，
  例如 claude-opus-5）——**沒有頂層 model 鍵**。禁止照抄 model_requested，
  禁止引用引擎對自己的標示（#616 實證：glm 曾照模板寫 gpt-5.6-sol）；
  環境變數身分（如 PI_MODEL）不算數。若無法從系統取得，寫 "unverified"，不得編造>
- brief: reviewer-brief-template-v1
- class: <code-risk | docs-skills | …>
- escalations: <[判斷] 需升級項列表；無則 none>

### Findings
- [P0/P1/P2] <一句話> — 證據：<檔案:行 / 指令輸出>

### Checklist RAN
- <項目> → RAN（<exit code / 量測結果>） | 需升級 | N.A.（<理由>）

### Follow-up issues（範圍外，不擋本輪）
- <一句話 + 證據>
```

## 6. 類別檢查清單 v1

類別定義與判定方式見設計文件 §1.1。清單是**起步集**：
sol 抓到而他人漏掉的 finding 會固化為新清單項或新金絲雀，線只升不降。

### 6.1 code-risk

1. shell／腳本：`||` 與 `&&` 混用時逐行寫出解析樹；任何刪除/覆寫類指令
   （`rm -rf`、`git reset --hard`、`git clean`）標出確切觸發條件。`[判斷]`
2. 新增的 `pub` 符號（fn/struct/trait）：在 diff 內找呼叫端/讀端；
   binary crate 的 `pub` 不算對外 API。找不到 → finding。
3. 錯誤處理與資源釋放：臨時目錄/檔案/鎖的釋放路徑；`set -e` 語意。
4. 邊界與注入：以性質表述（如「兩個不同輸入不得編碼到同一檔名」），
   逐形狀確認，不得寫成攻擊計畫。`[判斷]`
5. 測試宣稱 vs 實際：宣稱 covered 的路徑，測試真的會執行嗎。

### 6.2 docs-skills

1. **零裁量旗標驗證**：文件/skill 裡每個反引號 CLI 呼叫，跑 `<cmd> --help`
   驗證子命令與旗標存在，回報 exit code。非 0 → finding。
2. 帳本狀態一致性：文件宣稱的決策狀態（ratified／unratified／superseded）
   對照帳本實際事件；對不上 → finding（過期宣稱也算，即使方向「安全」）。
3. 權限邊界一致性：skill／runbook 指令不得指示讀者做超出其角色權限的事
   （合併、push、刪分支、跳過審查）；與 `fleet.merge-authority` 等現行裁定對照。
4. 連結與路徑存在性：相對連結、引用的檔案路徑，逐一實際開啟驗證。
5. 措辭與結構。`[判斷]`

## 7. 判決欄位與 #574 S5 的對齊

`model_observed` 的長期來源是 #574 S5（dispatch 收據自動附帶）；
在那之前由控制者從系統取得：pi 讀 session 檔的 `"model"` 欄位、
claude 讀 `--output-format json` 的 **`modelUsage`** 鍵（RAN 2026-09-02：
`claude -p --model opus --output-format json "reply with OK" | jq 'has("model"), has("modelUsage")'`
→ `false` / `true`；`.modelUsage | keys[]` → `"claude-opus-5"`。頂層鍵名有
`total_cost_usd`、`usage`、`modelUsage`、`session_id` … 但**沒有** `model`）。
合併政策讀**欄位**不讀標頭（裁定 `fleet.review-engine`）——引擎自我標示與模板照抄都不是證據。

## 8. 版本

- v1（2026-09-02）：初版。零裁量規則、`[判斷]` 規則、判決格式、兩類清單。
- v1.1（2026-09-02，#638 R1 修正）：`model_observed` 的 claude 來源由不存在的
  頂層 `model` 鍵更正為 `modelUsage`（RAN 實測見 §7），並明說環境變數身分
  （`PI_MODEL`）不算系統觀察值。
- 變更紀錄會附在每次判決的 `brief:` 欄（如 `reviewer-brief-template-v1.2`），
  讓抓取率可以對著 brief 版本解讀。

### Review elapsed source (GH-644)

Read elapsed from the **same pi session JSONL file** used for `model_observed`:

```sh
node scripts/pi-session-elapsed.mjs "$PI_SESSION_FILE"
```

Set `PI_SESSION_FILE` to the session file already located for this review.
Copy `elapsed_ms` into the verdict header as `elapsed: <N> ms (pi session)`
only when `elapsed_measured` is true; otherwise write `elapsed: unmeasured`.
The helper measures the first through last **message** timestamps, excluding
session headers. Missing, malformed, single-message, or a last timestamp before
the first timestamp
remain unmeasured. This session interval and dispatch's process lifetime are
different observations; always identify the source in the header.
