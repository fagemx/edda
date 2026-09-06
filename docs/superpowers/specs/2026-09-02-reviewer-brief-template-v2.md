# 審查 brief 模板 v2（substitutable reviewer 判準包）

- 日期：2026-09-06（#884 交付）
- 狀態：提案（操作者裁定前是紀錄，不是授權）
- 取代：[v1](2026-09-02-reviewer-brief-template-v1.md)——v1 保留為歷史，不再是派工來源
- 上游裁定：`fleet.review-engine=replaceable-by-qualification-not-brand`、
  `fleet.review-brief-framing=validation-checklist-not-attack-plan`、
  `fleet.claude-subscription-transport=claude-code-only`、
  `fleet.flash-tier-brief-contract`、`review.brief-source`
- 讀者：審查控制者（把本模板填成一次審查的 brief），與任何被派審查的引擎。

---

## 0. v2 改了什麼（只有兩處，其餘逐字沿用 v1）

| # | 位置 | v1 | v2 |
|---|---|---|---|
| 1 | §2 `[判斷]` 標籤規則 | 清單型引擎遇 `[判斷]` 項不得列 finding，一律標需升級 | 每個引擎都裁定並附推導；清單型引擎的裁定標 `provisional`，經合格引擎驗證後才計入合併閘，永不被壓下 |
| 2 | §6.1 項 1 | shell 解析樹＋觸發條件標 `[判斷]` | 解析樹＋真值表＋觸發條件是零裁量項；嚴重度查 §3 的表 |

改的理由是一次量測，不是偏好。#618 校準（設計文件 §3）c1 那一格逐字記著：
glm-5.3-flash 的解析樹與觸發條件**全對**（含「fast_build 成功路徑也會刪」），
卻因 v1 的規則不能列 finding，P0 閘因此記 1/2。設計文件 §3 學習 1 判定那是
**誤標**——解析樹與觸發條件是機械可驗的——並在 §7 項 4 排程 v2 修正。本文件是那項修正。

同一份校準也顯示嚴重度低估連錨都有（sol 對 c4 給 P1；Opus 對 c5 給 P2），
所以 v2 把嚴重度定成**查表**，不是任何引擎的裁量（§3）。

---

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

## 2. `[判斷]` 標籤規則 v2

- 標籤範圍：只有**沒有機械判定步驟**的項目標 `[判斷]`。能寫成機械步驟的一律
  寫成機械步驟——這條 v1 就有，v2 據此把 §6.1 項 1 移出 `[判斷]`。
- **每個引擎都對 `[判斷]` 項給出裁定，並在判決裡附上推導**（解析樹、真值表、
  逐條對照、指令輸出）。沒有一種引擎被禁止提出 finding。
- 清單型引擎提出的 `[判斷]` finding 在判決中標 `provisional`，同時列進
  `escalations:` 欄。`provisional` 的意思是**先記錄、後驗證**：由該類別合格的
  引擎驗證過才計入合併閘，驗證不過就降級或撤回，附理由。
- **壓下一個 `[判斷]` 發現是 P1**（不列出、或悄悄當成 RAN 都算）。v1 的規則會
  讓正確的推導無法變成 finding，那是 #618 c1 的實際損失。
- 「你是不是清單型引擎」仍然不由引擎自己決定：brief 沒有在當前校準表裡把你
  列為該類別合格，你就是清單型引擎——差別只在標籤，不在能不能講。

## 3. 證據門檻與嚴重度表

- 每個 finding 必須附：檔案:行（或指令輸出）＋讓人能重現的證據。
  只有主張沒有證據 = 不收。
- 安全相關的檢查以**程式必須成立的性質**與**必須確認的輸入形狀**表述，
  不寫成攻擊計畫（裁定 `fleet.review-brief-framing`——攻擊框架會被 provider
  拒絕，靜默燒掉一輪審查）。
- **嚴重度是查表，不是裁量。** 對著下表落一格；落不進任何一格才問控制者：

| 條件 | severity |
|---|---|
| 破壞或損失已追蹤資料／已推送工作（`rm -rf`、`git rm`、`reset --hard`、`git clean`、`--delete-branch`、force push）在非預期路徑上會執行 | P0 |
| 違反權限邊界（跳過審查、越權合併、指示讀者做超出角色的事） | P0 |
| 錯誤宣稱、不存在的介面／旗標／路徑、明確缺陷 | P1 |
| 資源未釋放、`set -e` 語意誤用、測試宣稱與實際不符 | P1 |
| 品質、可讀性、風格建議 | P2 |

同一條 finding 落進兩格時取較嚴重的一格。與 `expected.md` 的 severity 不符
要在校準表記為 `severity_match: no`（設計文件 §1.3）。

## 4. 唯讀約束

審查是唯讀角色。運輸層能強制的就運輸層強制
（pi：`--exclude-tools edit,write`；claude：`--allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)"`），
brief 文字只作為第二層：不編輯產品檔、不 push、不合併、不改帳本。

運輸限制照 `fleet.claude-subscription-transport`：Anthropic 模型只經 Claude Code，
不經 pi／openrouter，即使 pi 的 model 列表顯示 ready。

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
2. 標 [判斷] 的項目：裁定它，並附推導。你若不是本類別合格引擎，把該 finding
   標 provisional 並同時列進 escalations 欄——照樣寫出來，不省略。
3. 嚴重度查 §3 的表落格，不自行斟酌；落不進任何一格才回報控制者。
4. 唯讀：不編輯、不 push、不合併、不改帳本。
5. 證據門檻：每個 finding 附 檔案:行 或指令輸出；安全檢查以性質表述。
6. 非平凡 diff 零發現時，明確寫「零發現」與你檢查過的項目清單，不得留空。

判決格式（照抄此節，替換角括號）：

## Code Review: Round N
- elapsed: <pi session first-to-last message elapsed_ms; unmeasured if unavailable>
- model_requested: <dispatch 指定的模型>
- model_observed: <由系統取得：pi 讀 session 檔的 "model" 欄；claude 讀
  `claude -p --output-format json` 的頂層 **modelUsage** 鍵（其 key 就是模型 id，
  例如 claude-opus-5）——**沒有頂層 model 鍵**。禁止照抄 model_requested，
  禁止引用引擎對自己的標示（#616 實證：glm 曾照模板寫 gpt-5.6-sol）；
  環境變數身分（如 PI_MODEL）不算數。若無法從系統取得，寫 "unverified"，不得編造>
- brief: reviewer-brief-template-v2
- class: <code-risk | docs-skills | …>
- escalations: <provisional finding 列表；無則 none>

### Findings
- [P0/P1/P2] <一句話> — 證據：<檔案:行 / 指令輸出>
- [P0/P1/P2][provisional] <一句話> — 推導：<解析樹／真值表／對照> — 證據：<…>

### Checklist RAN
- <項目> → RAN（<exit code / 量測結果>） | 需升級 | N.A.（<理由>）

### Follow-up issues（範圍外，不擋本輪）
- <一句話 + 證據>
```

## 6. 類別檢查清單 v2

類別定義與判定方式見設計文件 §1.1。清單是**起步集**：
sol 抓到而他人漏掉的 finding 會固化為新清單項或新金絲雀，線只升不降。

### 6.1 code-risk

1. **shell／腳本優先序（零裁量）**：對每一條混用 `||` 與 `&&` 的新增行，寫出
   解析樹、寫出運算元結果的真值表、逐列說明哪些指令會執行。POSIX sh 的
   `||`／`&&` 同優先序、左結合，所以 `A || B && C` 是 `(A || B) && C`——`C`
   在 `A` 成功的路徑上也會執行。任何刪除／覆寫類指令（`rm -rf`、
   `git reset --hard`、`git clean`、`git rm`）標出確切觸發條件。嚴重度查 §3 的表：
   真值表有任一列在非預期路徑上執行破壞性指令＝P0，否則 P1。
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

`model_observed` **只從系統取得**，沒有第二個來源：pi 讀 session 檔的 `"model"`
欄位、claude 讀 `--output-format json` 的 **`modelUsage`** 鍵（RAN 2026-09-02：
`claude -p --model opus --output-format json "reply with OK" | jq 'has("model"), has("modelUsage")'`
→ `false` / `true`；`.modelUsage | keys[]` → `"claude-opus-5"`。頂層鍵名有
`total_cost_usd`、`usage`、`modelUsage`、`session_id` … 但**沒有** `model`）。
引擎的自我標示、模板照抄值、環境變數身分（`PI_MODEL`）都不是系統觀察值；
取不到就寫 `unverified`。長期來源是 #574 S5（dispatch 收據自動附帶）。
合併政策讀**欄位**不讀標頭（裁定 `fleet.review-engine`）。

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
the first timestamp remain unmeasured. This session interval and dispatch's
process lifetime are different observations; always identify the source in the
header.

## 8. 版本

- v1（2026-09-02，#618）：初版。零裁量規則、`[判斷]` 規則、判決格式、兩類清單。
- v1.1（2026-09-02，#638 R1）：`model_observed` 的 claude 來源更正為 `modelUsage`。
- **v2（2026-09-06，#884）**：§2 由「清單型引擎不得列 finding」改為
  「裁定並附推導，標 `provisional` 待驗證」；§6.1 項 1 由 `[判斷]` 改為零裁量
  並加真值表；§3 新增嚴重度查表；§7 收斂為「`model_observed` 只從系統取得」。
  依據：設計文件 §3 學習 1／2 與 §7 項 4。
- 變更紀錄會附在每次判決的 `brief:` 欄（如 `reviewer-brief-template-v2`），
  讓抓取率可以對著 brief 版本解讀。
