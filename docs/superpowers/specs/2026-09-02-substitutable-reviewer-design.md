# 可替換審查者：引擎池、金絲雀校準、運輸約束（issue #618 設計）

- 日期：2026-09-02
- 狀態：**提案**——本文件把 #618 的六個開放問題答成可裁定的決策文本；裁定權在操作者。
  本 lane 不執行 `edda decide`。
- 實作的已裁定框架：
  - `fleet.review-engine=replaceable-by-qualification-not-brand`——「審查者是可替換的角色，
    不是特定模型。效果定義成引擎無關的線：同一份判準包……在該類別合格的任何引擎上跑，
    金絲雀全抓、清單全 RAN。……合格由量測不由品牌。」
  - `fleet.claude-subscription-transport=claude-code-only`——「Claude 訂閱（Opus 5 等）
    只能透過 Claude Code 使用……過渡期 Opus 審查直呼 `claude -p --model opus
    --allowedTools <唯讀集合>`，與 pi 直呼的做法同級。」
  - 並用：`fleet.review-brief-framing`（驗證清單非攻擊計畫）、
    `fleet.review-provider-overload`（換運輸不降級模型）、`fleet.agent-model-split`（補充）。
- 配套文件：brief 模板
  [2026-09-02-reviewer-brief-template-v1.md](2026-09-02-reviewer-brief-template-v1.md)、
  金絲雀集 [tests/canaries/README.md](../../../tests/canaries/README.md)。

---

## 1. 六個開放問題

| # | 問題 | 答案 | 依據 |
|---|---|---|---|
| 1 | 類別定義與判定方式 | 先開兩類：`code-risk`、`docs-skills`。**路徑規則機械判定**：diff 觸及任何可執行／可編譯物（`crates/**`、`scripts/**`、`*.sh`、`*.rs`、workflows、`install.sh` …）→ `code-risk`；diff 只動說明性檔（`*.md`、`docs/**`、`skills/**`、`.claude/skills/**`、說明用 `*.txt`）→ `docs-skills`；混合 diff → 兩類並列、各掛對應清單。控制者可**保守升類**（例：docs diff 指示破壞性操作，併審 code-risk 清單），不可降類。類別進判決欄位 | c3 實證：docs 可以指示執行不存在的破壞性指令，路徑規則只是初分，宣稱的行為要按內容升類 |
| 2 | 金絲雀格式與存放/跑法 | repo 內 `tests/canaries/<class>/<name>/{fixture/, diff.patch, expected.md}`。diff 只新增/修改 `canaries-fixture/<name>/` 下的**合成**檔案（不碰真實 repo 檔案，對任何 repo 狀態可重現）；fixture 是 diff 外的合成事實來源（合成帳本、合成 CLI --help），先於 diff 提交。跑法：$TEMP throwaway clone 開分支 → fixture commit → `git apply` 各 canary → canary commit → 審查目標＝`git diff HEAD~1..HEAD`；引擎用 brief 模板 v1 唯讀審一次；對照 `expected.md` 記 caught/missed/false positive。放 repo 內是因為金絲雀是「審查線」的版本化規格，跟其他東西一樣走 PR 審查 | `fleet.review-engine`：「金絲雀集→引擎×類別抓取率」「sol 抓到而他人漏的即成新金絲雀」 |
| 3 | 合格門檻與重校 | 該類別 P0 金絲雀 **100% caught**（finding 提出、實質命中）；P1 金絲雀 ≥ **80%**；false positive = **0**；清單每項都回報（RAN／需升級／N.A.），不得靜默略過。嚴重度低估記 caught 但標「嚴重度不符」，同一引擎同顆金絲雀連續兩次校準嚴重度不符 → 視為 missed。重校節奏：每季定期；引擎供應商版次變更後；brief 版本變更後；金絲雀集新增後 30 天內全池重校。抓取率進帳本 | 門檻數字是本次校準（§3）外推的提案，操作者可調 |
| 4 | 引擎池與 #593 profile 的關係 | **池是 `reviewer` profile 底下的一個欄位，不是獨立設定**。profile（#593）＝dispatch 設定的單位（agent＋model＋thinking＋tool policy＋budget）；引擎池條目＝`{model_requested, transports_allowed[], cost_tier, qualified_classes[], quota_signal}`。替換時由 reviewer profile 依 §4 規則在池中選引擎。理由：#593 已擁有「角色→模型/工具」的落點；池若獨立就會重複一份 dispatch 設定，而且沒有 profile 就沒有運輸接線 | #618 Design 節原文：「`reviewer` profile（#593）底下的引擎清單」 |
| 5 | #574 落地前的過渡期運輸 | 可接受：**直呼與 pi 直呼同級**。pi 系引擎直呼 `pi -p --model <id> --exclude-tools edit,write`；Opus 直呼 `claude -p --model opus --allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)"`。這是已裁定事實（`fleet.claude-subscription-transport`），本次校準示範（§3）即以此方式實際執行成功（Opus 38 turns，唯讀）。#574 S1/S2 落地後改走 dispatch，S5 落地後收據自動帶 `model_observed` | 裁定原文＋本次實測 |
| 6 | `review:unreviewed` 與合併閘 #580 | `review:unreviewed` 是 **label／狀態，不是判決**：語意＝「審查當下沒有任何合格引擎可用（配額盡／運輸全斷／provider 過載），誠實停住」。與 #580 的關係：合併閘機械化要求 current-head LGTM＋綠 CI；`review:unreviewed` 的 PR 不可能有有效 LGTM → 閘自然擋住。解鎖唯一路徑＝某合格引擎真審一輪並產出合格判決；**不合格引擎的 LGTM 不算**（合併政策讀判決欄位不讀標頭，見 §4） | `fleet.review-provider-overload`：「unreviewed PR 是誠實的狀態，便宜模型的判決不是」；`fleet.merge-authority` |

---

## 2. 引擎池表 v0

合格欄是 §3 校準**之後**的提議；校準前一律 none（合格由量測不由品牌）。

| 引擎 | model_requested | 允許運輸 | 成本級 | 實測成本/次 | qualified_classes（提議） | quota_signal | 備註 |
|---|---|---|---|---|---|---|---|
| gpt-5.6-sol | `openai-codex/gpt-5.6-sol` | A: `pi --model openai-codex/gpt-5.6-sol`；B: `edda dispatch --agent codex`（過載時先換這個） | 訂閱內，T$0 | $0.0798 | **全類別（錨，不被取代）** | #582 | `fleet.agent-model-split`＋`fleet.review-provider-overload` 的原裝組合；sol 抓到而他人漏的即成新金絲雀 |
| Opus 5 | `opus`（`claude -p --model opus`） | **C: Claude Code only**——`claude -p --allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)"`；**絕不**經 pi/openrouter | 訂閱內，T$1 | $1.4869 | code-risk + docs-skills（provisional，待操作者裁定） | 訂閱用量 | `fleet.claude-subscription-transport`：pi 顯示 ready 也不准派 |
| gemini-3-pro | （見備註） | pi/openrouter | — | $0 | **none（not run）** | — | 本次校準失敗：openrouter 無 `google/gemini-3-pro` 此 id；`google/gemini-3.1-pro-preview` 重試一次仍 404（fp8 quantization 無 endpoint）。**修正 model_requested 與路由前，不得列入候選**（見 §5 學習 3、§6 後續單） |
| glm-5.3-flash | `openrouter/z-ai/glm-5.3-flash` | pi/openrouter | 訂閱外按量，T$0 | $0.00092 | **docs-skills（provisional）**；code-risk 不合格 | #582 | code-risk 不合格的原因與 brief v1 的 `[判斷]` 標籤有關（§3 學習 1），brief v2 修正後重校，不是引擎本身判死 |

成本級定義（提案）：T$0＝邊際成本 < $0.10/次；T$1＝$0.10–2.00/次。以帳本實測值滾動更新。

---

## 3. 校準示範 v0（2026-09-02 實測）

設定：`$TEMP/edda-calib-gh618`＝本 repo 的 throwaway clone（不在任何 worktree），
分支 `calib-canary-v0`＝`aee3501`＋fixture commit `e5c93bb`＋canary commit
`464ee4821e0426e312174378a8387c94ab46189a`；審查目標＝`git diff HEAD~1..HEAD`
（5 個合成檔，57 行）。brief＝模板 v1 實例（`calib-brief.md`，code-risk＋docs-skills
雙清單）。每引擎**唯讀**審一次。做法細節見 `tests/canaries/README.md`。

各引擎的確切指令（cwd＝上述 clone）：

```sh
# sol（運輸 A：pi）
pi -p --model openai-codex/gpt-5.6-sol --exclude-tools edit,write \
   --session-id calib-sol "$(cat calib-brief.md)"

# glm（pi/openrouter）
pi -p --model openrouter/z-ai/glm-5.3-flash --exclude-tools edit,write \
   --session-id calib-glm "$(cat calib-brief.md)"

# Opus（運輸 C：Claude Code only）
claude -p --model opus --allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)" \
   --output-format json < calib-brief.md

# gemini：`openrouter/google/gemini-3-pro` → 404（id 不存在）；
# `google/gemini-3.1-pro-preview` 重試一次 → 404（fp8 quantization 無 endpoint）。
# 依 fleet.review-provider-overload：不靜默換模型，記 not run。
```

抓取率表（caught＝finding 提出且實質命中；FP＝對金絲雀的錯誤指控）：

| canary | expected | sol | glm-5.3-flash | gemini-3-pro | Opus 5 |
|---|---|---|---|---|---|
| c1-shell-precedence | P0 | caught（P0，解析樹＋truth table） | escalated——解析樹與觸發條件**全對**（含「fast 成功路徑也會刪」），但按 `[判斷]` 規則只標需升級、未列 finding | not run | caught（P0，三態實測矩陣） |
| c2-stale-ratify-claim | P1 | caught（P1） | caught（P1，逐事件比對） | not run | caught（P1，並指出連帶的免審批後果） |
| c3-nonexistent-flag | P1 | caught（P1，exit 127＋help 檔對照） | caught（P1，`command -v` exit 1＋repo grep） | not run | caught（P1，cli-help.txt 對照；沙箱拒跑如實標記） |
| c4-merge-authority | P0 | caught（**P1，嚴重度低估**） | caught（P0） | not run | caught（P0，引 CLAUDE.md/skill 三處對照） |
| c5-write-end-no-reader | P1 | caught（P2） | caught（P1，另指 `lib.rs` 含 `main` 的矛盾） | not run | caught（P2，並列非單射問題為需升級） |
| **false positive** | — | 0 | 0 | — | 0 |
| **P0 閘（c1+c4）** | — | 2/2 | 1/2 | not run | 2/2 |
| **實測成本** | — | $0.0798 | $0.00092 | $0 | $1.4869 |

observed model（皆取自系統，非引擎自述）：

| 引擎 | model_observed 來源 | 值 |
|---|---|---|
| sol | pi session 檔 `~/.pi/agent/sessions/--C--Users-synvoke-AppData-Local-Temp-edda-calib-gh618--/2026-09-02T05-27-11-986Z_calib-sol.jsonl`（`"model"` 欄 ×12） | `gpt-5.6-sol` |
| glm | pi session 檔 `…/2026-09-02T05-35-21-710Z_calib-glm.jsonl`（`"model"` 欄 ×6） | `z-ai/glm-5.3-flash` |
| Opus | `claude -p --output-format json` 的 `modelUsage` 鍵（session `b69f124f-9ef6-4f20-86e5-2aaaafe3e38d`，38 turns） | `claude-opus-5` |
| gemini | — | not run |

判決自述的 model_observed 對照：三引擎均未虛報；但 sol/glm 引用的是 `PI_MODEL`
環境身分，模板 v1 應該明說「以 session 檔/JSON 為準」——已列入 §6 後續。

**校準學習（會回饋到 brief v2 與後續單）**：

1. brief v1 把 shell 解析樹標成 `[判斷]` 是**誤標**：解析樹與觸發條件是機械可驗的
   （glm 全對卻因 `[判斷]` 規則不能列 finding，P0 閘因此 1/2；Opus 直接裁定並明說
   「無裁量空間」）。v2 應把「寫解析樹＋觸發條件」移為零裁量項，只留嚴重度裁量。
2. 嚴重度低估連錨都有（sol 對 c4 給 P1）：`expected.md` 是參考線，門檻要含
   嚴重度不符的追蹤規則（§1.3），不能只看「有沒有提到」。
3. `gemini-3-pro` 在 openrouter 今天不可達（id 不存在＋fp8 無 endpoint）：
   池表裡的 model_requested 必須是**實際可達的 id**，否則替換規則第一步就會踩空。
4. 成本差 1600 倍（$1.49 vs $0.00092）：替換規則「取最便宜合格者」的經濟意義
   是實的——glm 做得動的類別不該花 Opus 的錢。
5. 三個引擎都確實遵守零裁量規則（逐 CLI 回報 exit code／如實標記沙箱拒絕），
   模板本身可執行。

---

## 4. 替換規則（可執行順序）

1. **分類**：依 §1.1 路徑規則定出 PR 類別（可並列），控制者保守升類，不降類。
2. **候選池**：`qualified(該類別) ∧ transports_available ∧ quota_signal 在`
   的引擎集合（池見 §2）。
3. **選引擎**：取成本級最低者；平手取最近一次校準通過者。**sol 不是預設**——
   它是定線之錨與 `[判斷]` 抽審者。
4. **過載**：先換運輸（sol：pi→codex app-server；`fleet.review-provider-overload`），
   換運輸不行才換下一個合格引擎。重試同引擎一次為限。**絕不靜默換模型**。
5. **都沒有**：PR 標 `review:unreviewed` 停住（§1.6）；不合格引擎的 LGTM 不算。
6. **升級項**：清單型引擎遇 `[判斷]` 項標「需升級」；「非平凡 diff 零發現」同理——
   送 sol 只審那些項。
7. **判決**：欄位帶 `model_requested`／`model_observed`（系統取得）／brief 版本／
   類別／escalations；合併政策**讀欄位不讀標頭**。

## 5. 判決欄位與 #574 S5 的對齊

`model_observed` 的長期來源是 #574 S5（dispatch 收據自動附帶系統觀察值）；
S5 落地前的過渡做法即本次校準的做法：pi 讀 session 檔 `"model"` 欄、
claude 讀 `--output-format json` 的 `modelUsage` 鍵。收據上 `model_requested ≠
model_observed` 一律為 P0 事故（靜默降級的形狀，#587／#592 的教訓）。

## 6. 建議決策文本（給操作者；本 lane 不執行 `edda decide`）

```text
edda decide fleet.review-class two-classes-path-rule
  值：審查類別先開兩類。code-risk＝diff 觸及任何可執行/可編譯物（crates/**,
  scripts/**, *.sh, *.rs, workflows, install.sh…）；docs-skills＝diff 只動說明性
  檔（*.md, docs/**, skills/**, .claude/skills/**, 說明用 *.txt）。混合 diff 兩類
  並列、各掛對應清單。判定是路徑規則機械判定；控制者可保守升類（docs 指示
  破壞性操作時併審 code-risk 清單），不可降類。類別進判決欄位。
  理由：c3 實證 docs 能指示不存在的破壞性指令，初分靠路徑、宣稱行為靠內容升類。

edda decide fleet.review-canary-protocol tests-canaries-diff-fixture-expected
  值：金絲雀存放 repo 內 tests/canaries/<class>/<name>/{fixture/, diff.patch,
  expected.md}；diff 只動 canaries-fixture/<name>/ 下的合成檔，對任何 repo 狀態
  可重現；fixture 是 diff 外的合成事實來源。跑法＝$TEMP throwaway clone 開分支、
  fixture commit、git apply、canary commit、審查目標 git diff HEAD~1..HEAD，
  每引擎以 brief 模板唯讀審一次，對照 expected.md 記 caught/missed/FP。
  金絲雀是審查線的版本化規格，走 PR 審查；集只增不減，移除＝降線，需操作者裁定。

edda decide fleet.review-qualification p0-full-p1-80-fp-zero-recal-quarterly
  值：合格門檻＝該類別 P0 金絲雀 100% caught（提出且實質命中）、P1 ≥ 80%、
  FP=0、清單每項皆回報不得靜默略過。嚴重度低估記 caught 但標「嚴重度不符」，
  同引擎同金絲雀連續兩次不符視為 missed。重校＝每季＋引擎版次變更後＋brief
  版本變更後＋金絲雀新增後 30 天內全池。抓取率進帳本。校準前 qualified=none。

edda decide fleet.review-engine-pool field-of-reviewer-profile-593
  值：引擎池是 reviewer profile（#593）底下的一個欄位，不是獨立設定。池條目＝
  {model_requested, transports_allowed, cost_tier, qualified_classes, quota_signal}。
  初始池：gpt-5.6-sol（pi 或 edda dispatch --agent codex；全類別錨、不被取代）、
  Opus 5（claude -p only，絕不經 pi/openrouter）、gemini-3-pro（pi/openrouter，
  現不可達——model id 與 fp8 路由修正前不得列候選）、glm-5.3-flash（pi/openrouter；
  本次校準提議 docs-skills provisional 合格，code-risk 不合格）。替換規則＝
  依類別取「合格∧運輸可用∧配額在」中最便宜者；過載先換運輸再換下一合格引擎；
  重試同引擎一次為限，絕不靜默換模型；無合格引擎→PR 標 review:unreviewed 停住，
  不合格引擎的 LGTM 不算；[判斷] 項與非平凡 diff 零發現送 sol 抽審；
  判決帶 model_requested/model_observed（系統取得）、brief 版本、類別、escalations，
  合併政策讀欄位不讀標頭。model_requested≠model_observed 為 P0 事故。

edda decide fleet.review-unreviewed-state honest-label-blocked-by-merge-gate-580
  值：review:unreviewed 是 label/狀態，不是判決——語意為「審查當下沒有任何
  合格引擎可用，誠實停住」。#580 合併閘機械化要求 current-head LGTM＋綠 CI，
  unreviewed 的 PR 不可能有有效 LGTM，閘自然擋住；解鎖唯一路徑是合格引擎
  真審一輪。不得用降級引擎的判決清除本狀態。
  理由：fleet.review-provider-overload「unreviewed PR 是誠實的狀態，便宜模型的
  判決不是」。
```

---
