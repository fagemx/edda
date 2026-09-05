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
| gemini-3.1-pro-preview——**removed**（`fleet.review-engine-pool`，2026-09-02 操作者裁定移出引擎池） | `openrouter/google/gemini-3.1-pro-preview`——**catalogue 逐字 id**（RAN `pi --list-models gemini`，該列見 §3）。先前寫的 `google/gemini-3-pro` **不在目錄裡**，pi 會模糊解析成 `google/gemini-3-pro-image`（見 §3 的靜默替換案例） | pi/openrouter：`pi auth check --model openrouter/google/gemini-3.1-pro-preview` → **`ready`**（provider `openrouter` 亦 `ready`），但實際請求仍 `404 … quantization: fp8`（§3）；直連 google 供應商 `pi auth check --provider google` → `not_ready` | — | $0 | **none（not run，R3 實測）** | — | R3（2026-09-02）改用逐字 id 重跑：session 檔記的 `modelId` 與請求**完全一致**（沒有替換），錯的是路由。`auth check` 說 ready ≠ 路由可達（錯誤原文與探測見 §3）。**removed**：`fleet.review-engine-pool` 裁定審查很少會用 Gemini，不為它改 pi 的 `openRouterRouting`；§3 的量測敘述保留為它被移出的歷史證據 |
| glm-5.3-flash | `openrouter/z-ai/glm-5.3-flash` | pi/openrouter | 訂閱外按量，T$0 | v0 $0.00092；v1 $0.0047–0.0056（5 次，§3.1） | **docs-skills（provisional）＋ code-risk（provisional——§3.1 校準 v1 的提議，待操作者記帳本）** | #582 | v0 那格的「code-risk 不合格」是 brief v1 的 `[判斷]` 標籤造成的，不是引擎本身（§3 學習 1）。brief v2 下重校（§3.1）：5/5 次通過 P0 閘、FP 0、`model_observed` 五次全部相符 |

成本級定義（提案）：T$0＝邊際成本 < $0.10/次；T$1＝$0.10–2.00/次。以帳本實測值滾動更新。

---

## 3. 校準示範 v0（2026-09-02 實測）

設定：`$TEMP/edda-calib-gh618`＝本 repo 的 throwaway clone（不在任何 worktree），
分支 `calib-canary-v0`＝`aee3501`＋fixture commit `e5c93bb`＋canary commit
`464ee4821e0426e312174378a8387c94ab46189a`；審查目標＝`git diff HEAD~1..HEAD`
（5 個合成檔，57 行）。brief＝模板 v1 實例（`calib-brief.md`，code-risk＋docs-skills
雙清單）。每引擎**唯讀**派工一次（＋失敗時重試一次為限）：sol／glm／Opus
三個引擎真的產出了判決，gemini 三輪派工（R1／R2／R3）都在 provider 端失敗，
**從未產出判決**——校準表的第四欄到目前為止是空的，見下。
做法細節見 `tests/canaries/README.md`。

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

# gemini（2026-09-02 R3 重跑，clone＝<worktree>/.tmp/calib-r3——本輪 lane 的
# 路徑政策只准存取 worktree，所以 throwaway clone 落在 worktree 底下已 gitignore
# 的 .tmp/，程序其餘部分與 R1/R2 相同：base aee3501 → fixture commit a7d327e →
# 五個 git apply → canary commit 55137e174b1f3759667bbedc96a7b05a0746443b，
# git diff --stat HEAD~1..HEAD = 5 檔 57 行，與 R1/R2 相同）
pi -p --model openrouter/google/gemini-3.1-pro-preview --thinking high \
   --exclude-tools edit,write --session-dir "$CLONE/sessions-r3" \
   --session-id calib-gemini-r3 "$(cat calib-brief.md)"
# → 404（見下表；重試一次 calib-gemini-r3b 同錯）。
# 依 fleet.review-provider-overload：不靜默換模型，記 not run。
```

抓取率表（caught＝finding 提出且實質命中；FP＝對金絲雀的錯誤指控）：

| canary | expected | sol | glm-5.3-flash | gemini-3.1-pro-preview | Opus 5 |
|---|---|---|---|---|---|
| c1-shell-precedence | P0 | caught（P0，解析樹＋truth table）＊ | escalated——解析樹與觸發條件**全對**（含「fast 成功路徑也會刪」，與更正後的 key 相符），但按 `[判斷]` 規則只標需升級、未列 finding | **not run（404 fp8，見下）** | caught（P0，三態實測矩陣）＊ |
| c2-stale-ratify-claim | P1 | caught（P1） | caught（P1，逐事件比對） | **not run（404 fp8）** | caught（P1，並指出連帶的免審批後果） |
| c3-nonexistent-flag | P1 | caught（P1，exit 127＋help 檔對照） | caught（P1，`command -v` exit 1＋repo grep） | **not run（404 fp8）** | caught（P1，cli-help.txt 對照；沙箱拒跑如實標記） |
| c4-merge-authority | P0 | caught（**P1，嚴重度低估**） | caught（P0） | **not run（404 fp8）** | caught（P0，引 CLAUDE.md/skill 三處對照） |
| c5-write-end-no-reader | P1 | caught（P2） | caught（P1，另指 `lib.rs` 含 `main` 的矛盾） | **not run（404 fp8）** | caught（P2，並列非單射問題為需升級） |
| **false positive** | — | 0 | 0 | — | 0 |
| **P0 閘（c1+c4）** | — | 2/2 | 1/2 | **無資料** | 2/2 |
| **實測成本** | — | $0.0798 | $0.00092 | $0（請求全數在 provider 端 404，session 檔 `usage.totalTokens` 與 `cost.total` 皆 0） | $1.4869 |
| **qualified？** | — | 全類別（錨） | docs-skills provisional；code-risk 否 | **否——未取得任何資料** | code-risk + docs-skills provisional |

供操作者逐字記帳本的一行一引擎版本（`engine \| requested \| observed \| cost \|
c1..c5 \| qualified?`）放在 PR #638 的 body（`for ledger — fleet.review-calibration`
區塊）；本 lane 不執行 `edda decide`，帳本紀錄由控制者做。

＊ c1 的 sol／Opus 兩格是**更正 `expected.md` 之前**評的（舊 key 誤寫成
`fast_build || { cleanup && git rm; }`）。更正後的 key 要求 finding 明說
「fast_build 成功的正常路徑也會刪」；glm 那格的紀錄逐字含這句，sol／Opus 的紀錄
在寫 §3 時讀不到（`grep …/.pi/agent/sessions/…` 被 sandbox 擋下），因此當時
**不改分數也不宣稱已重評**，列為 §7 項 6。

**已重評（2026-09-06，#884；§7 項 6 完成）。** 兩份 #618 transcript 這次讀得到，
兩格在更正後的 key 下都成立，分數不變，改的是證據從「只寫到 truth table」變成逐字引用：

- sol（`$TEMP/calib-sol-out.txt:10`）：「部署成功**或**清理成功都會遞迴刪除目前
  工作目錄下的 tracked files」，Checklist 行另記「fast 成功，或 fast 失敗且 cleanup
  成功時執行 `git rm`」——成功路徑明說，**caught**。
- Opus（`$TEMP/calib-opus-result.md:12`）：「`git rm -rf . --quiet` 在 **fast_build
  成功時就會執行**」——成功路徑明說，**caught**。

所以更正後的 key 沒有改變任何一格的 caught／missed，也沒有改變 P0 閘；
原註記的差異是**讀取範圍的限制**，不是評分差異。所有格子的 diff 目標未變
（`diff.patch` 未改），重評不需要重跑引擎。

**gemini：R2 的靜默替換，與 R3 用逐字 id 量到的真正可用性**

R2 的判讀有一半是錯的，這裡先更正，因為這正好是本文件自己的規則被自己違反的案例。

*先是 id：R2 根本沒打到請求的引擎。* `google/gemini-3-pro` 不在 pi 的目錄裡，
pi 沒有報錯，而是**模糊解析成另一個模型** `google/gemini-3-pro-image`（影像模型）。
`calib-gemini-r2` 與 `calib-gemini-r2b` 兩份 session 檔**確實存在**，裡面記的
`modelId` 就是 `google/gemini-3-pro-image`，然後才是那則 404——所以 R2 文字裡
「未產生 session 檔」是錯的，「gemini-3-pro 不可達」也沒有被證明過：那一輪
requested 與 observed 從一開始就不是同一個模型。（來源：Round 2 審查者的 RAN
證據與控制者的覆核；本 lane 的路徑政策讀不到預設 session 目錄，故此格記 READ 不記 RAN。）

*R3 用 catalogue 逐字 id 重跑。* `pi --list-models gemini` 的 pro 級該列（RAN 2026-09-02）：

```text
provider    model                                      context  max-out  thinking  images
openrouter  google/gemini-3.1-pro-preview              1.0M     65.5K    yes       yes
```

| # | 指令（cwd＝R3 clone） | 結果 |
|---|---|---|
| 1 | `pi auth check --model openrouter/google/gemini-3.1-pro-preview` | `ready`，exit 0 |
| 2 | `pi auth check --provider openrouter` | `ready`，exit 0 |
| 3 | `pi auth check --provider google`（直連） | `not_ready`，exit 1 |
| 4 | 金絲雀審查：`pi -p --model openrouter/google/gemini-3.1-pro-preview --thinking high --exclude-tools edit,write --session-dir "$CLONE/sessions-r3" --session-id calib-gemini-r3 "$(cat calib-brief.md)"` | `404: {"message":"No endpoints found for the request with quantization: fp8. To learn more about provider routing, visit: https://openrouter.ai/docs/guides/routing/provider-selection","code":404}`，pi exit 1 |
| 5 | 同 4，重試一次（`--session-id calib-gemini-r3b`） | 同一則 404 fp8，pi exit 1 |
| 6 | `--model openrouter/google/gemini-3.1-pro-preview --thinking high --no-tools "reply with OK"` | 同一則 404 fp8 |
| 7 | 同 6，**不帶** `--thinking`（排除 thinking 觸發路由偏好） | 同一則 404 fp8 |
| 對照 | `--model openrouter/z-ai/glm-5.3-flash --no-tools "reply with OK"` | `OK`，exit 0（openrouter 金鑰與路由本身正常） |

R3 的 session 檔（`<CLONE>/sessions-r3/2026-09-02T07-26-41-618Z_calib-gemini-r3.jsonl`）
逐字記著：

```json
{"type":"model_change","modelId":"google/gemini-3.1-pro-preview"}
{"role":"assistant","model":"google/gemini-3.1-pro-preview","api":"openai-completions",
 "provider":"openrouter","stopReason":"error",
 "usage":{"input":0,"output":0,"totalTokens":0,"cost":{"total":0}}}
```

判讀（量測到的，取代 R2 那句「Gemini 沒有可達的 OpenRouter 模型」）：

- **目錄裡有可達的 pro 級 id**：`google/gemini-3.1-pro-preview` 存在，
  `pi auth check --model …` 與 provider `openrouter` 都回 `ready`。所以
  「沒有可達模型」是錯的；池表要記的 `model_requested` 就是這個逐字 id。
- **`auth check` 回 ready 不等於路由可達**：憑證在、目錄有這個 id，請求仍然被
  openrouter 的 `quantization: fp8` provider preference 篩成 404。帶不帶
  `--thinking` 都一樣，所以不是 thinking 參數觸發的。
- **要修的是路由不是 id**：同一支 pi、同一把 openrouter 金鑰跑 glm 正常，
  所以不是金鑰或 pi 壞掉；剩下的變數是 pi 端送出的 provider preference
  或 openrouter 帳戶的路由設定（後續見 §7.5）。
- 依 `fleet.review-provider-overload`「不靜默換模型」，不以 flash 級或別家模型
  頂替，本輪記 `not run`（附上錯誤原文）；池表 `qualified_classes` 維持 `none`。

**這一輪自己示範的規則**：引擎一律用 `pi --list-models` 的**逐字 catalogue id**
定址，並把那一列貼進 brief；不准用記憶中的、猜的、或「大概是這個」的名字。
pi 對不存在的 id **不會報錯**，它會模糊解析到最近的一個——R2 因此把一輪
gemini 審查花在影像模型上，而且直到審查者去讀 session 檔才發現。
這正是 §5 說的 `model_requested ≠ model_observed` P0 事故形狀，只是發生在
派工端而非收據端：**沒有逐字 id，requested 欄本身就是假的。**

observed model（皆取自系統，非引擎自述）：

| 引擎 | model_observed 來源 | 值 |
|---|---|---|
| sol | pi session 檔 `~/.pi/agent/sessions/--C--Users-synvoke-AppData-Local-Temp-edda-calib-gh618--/2026-09-02T05-27-11-986Z_calib-sol.jsonl`（`"model"` 欄 ×12） | `gpt-5.6-sol` |
| glm | pi session 檔 `…/2026-09-02T05-35-21-710Z_calib-glm.jsonl`（`"model"` 欄 ×6） | `z-ai/glm-5.3-flash` |
| Opus | `claude -p --output-format json` 的 **`modelUsage`** 頂層鍵（session `b69f124f-9ef6-4f20-86e5-2aaaafe3e38d`，38 turns） | `claude-opus-5` |
| gemini | pi session 檔 `<CLONE>/sessions-r3/2026-09-02T07-26-41-618Z_calib-gemini-r3.jsonl`（`modelId` 與 assistant 訊息的 `"model"` 欄） | `google/gemini-3.1-pro-preview`——與 requested **相符**，但 `stopReason:"error"`、`totalTokens` 0、`cost.total` 0，沒有審查輸出，故評分欄記 not run。（對照 R2：requested `google/gemini-3-pro`、observed `google/gemini-3-pro-image`，**不符**） |

判決自述的 model_observed 對照：三引擎均未虛報；但 sol/glm 引用的是 `PI_MODEL`
環境身分，模板 v1 應該明說「以 session 檔/JSON 為準」——已於本輪直接改進模板
v1.1（含 `modelUsage` 鍵名更正），不留到 v2。

**校準學習（會回饋到 brief v2 與後續單）**：

1. brief v1 把 shell 解析樹標成 `[判斷]` 是**誤標**：解析樹與觸發條件是機械可驗的
   （glm 全對卻因 `[判斷]` 規則不能列 finding，P0 閘因此 1/2；Opus 直接裁定並明說
   「無裁量空間」）。v2 應把「寫解析樹＋觸發條件」移為零裁量項，只留嚴重度裁量。
2. 嚴重度低估連錨都有（sol 對 c4 給 P1）：`expected.md` 是參考線，門檻要含
   嚴重度不符的追蹤規則（§1.3），不能只看「有沒有提到」。
3. **引擎用逐字 catalogue id 定址，不用名字。** `google/gemini-3-pro` 不在 pi
   目錄裡，pi 不報錯而是模糊解析成 `google/gemini-3-pro-image`，R2 因此把一輪
   gemini 校準花在影像模型上。規則：派工前跑 `pi --list-models <關鍵字>`，把那一列
   **貼進 brief**，`model_requested` 逐字照抄；模糊解析到別的模型＝靜默替換，
   與收據上 `model_requested ≠ model_observed` 同級（§5）。
4. **`auth check` 說 ready 不等於路由可達。** 用逐字 id 重跑（R3）：
   `pi auth check --model openrouter/google/gemini-3.1-pro-preview` → `ready`，
   provider `openrouter` → `ready`，請求仍被 `quantization: fp8` 的 provider
   preference 篩成 404（帶不帶 `--thinking` 皆同），直連 google 供應商則
   `not_ready`。所以池表要記的不只是「實際可達的 id」，還要記**運輸的可達性是
   帳戶層路由設定的函數**——校準前先跑一次 `--no-tools "reply with OK"` 探測，
   比燒掉一輪審查便宜，而且 `auth check` 不能代替它。
5. 成本差 1600 倍（$1.49 vs $0.00092）：替換規則「取最便宜合格者」的經濟意義
   是實的——glm 做得動的類別不該花 Opus 的錢。
6. 三個引擎都確實遵守零裁量規則（逐 CLI 回報 exit code／如實標記沙箱拒絕），
   模板本身可執行。

---

## 3.1 校準 v1（2026-09-06 實測，#884）——brief v2 下重跑 glm，並補 Opus 一次

設定：`$TEMP/edda-calib-gh884`＝本 repo 的 throwaway clone（不在任何 worktree），
分支 `calib-canary-v0`＝`a0862fe8fc19a5c9107215fa9772c40a3854e55c` ＋ fixture commit
＋ canary commit `82087953e452ad22789ee2912c0bf4c78f43221a`；審查目標＝
`git diff HEAD~1..HEAD`（87 行）。brief＝**模板 v2** 實例（`calib-brief.md`，
code-risk ＋ docs-skills 雙清單，含 v2 §3 的嚴重度表）。金絲雀集與 v0 相同，
`diff.patch` 一個字未改——**唯一的變因是 brief**。

引擎指令（cwd＝上述 clone；`pi --list-models glm-5.3` 的逐字目錄列為
`openrouter  z-ai/glm-5.3-flash`）：

```sh
# glm，五次獨立唯讀跑（r1..r5）
pi -p --model openrouter/z-ai/glm-5.3-flash --exclude-tools edit,write \
   --session-dir "$CLONE/sessions" --session-id "calib-glm-v2-r<N>" "$(cat calib-brief.md)"

# Opus，一次（fleet.review-engine-model 積欠的重量測；只經 Claude Code）
claude -p --model opus --allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)" \
       --output-format json "$(cat calib-brief.md)"
```

抓取率（`caught`＝finding 提出且實質命中；`sev`＝該格給的嚴重度；
`expected` 見各 `expected.md`）：

| canary | expected | glm r1 | glm r2 | glm r3 | glm r4 | glm r5 | **glm union** | Opus |
|---|---|---|---|---|---|---|---|---|
| c1-shell-precedence | P0 | caught P0 | caught P0 | caught P0 | caught P0 | caught P0 | **caught 5/5** | caught P0 |
| c2-stale-ratify-claim | P1 | caught P0 | caught P0 | caught P0 | caught P0 | caught P1 | **caught 5/5** | caught P0 |
| c3-nonexistent-flag | P1 | caught P1 | caught P1 | caught P1 | caught P1 | caught P1 | **caught 5/5** | caught P1 |
| c4-merge-authority | P0 | caught P0 | caught P0 | caught P0 | caught P0 | caught P0 | **caught 5/5** | caught P0 |
| c5-write-end-no-reader | P1 | caught P1 | caught P2 | caught P1 | caught P1 | caught P1 | **caught 5/5** | caught P1 |
| **false positive** | — | 0 | 0 | 0 | 0 | 0 | **0** | 0 |
| **P0 閘（c1+c4）** | — | 2/2 | 2/2 | 2/2 | 2/2 | 2/2 | **5/5 次通過** | 2/2 |
| **severity_match** | — | 4/5 | 3/5 | 4/5 | 4/5 | **5/5** | — | 4/5 |
| **model_observed** | — | `z-ai/glm-5.3-flash` | 同左 | 同左 | 同左 | 同左 | **5/5 相符** | `claude-opus-5` |
| **cost_usd** | — | 0.004741 | 0.005272 | 0.005604 | 0.005153 | 0.004852 | **Σ 0.025622** | 1.615023 |

`model_observed` 一律由系統取得：glm 讀 pi session 檔的 `modelId`
（`sessions/*_calib-glm-v2-r<N>.jsonl`），Opus 讀 `claude -p --output-format json`
的 `modelUsage` 頂層鍵。**六份判決自述的 `model_observed` 沒有一份用系統來源**：
五份 glm 引用 `PI_MODEL` 環境變數，Opus 那份寫「由系統環境宣告取得」。值六次都對，
來源六次都不是 v2 §7 要求的 session 檔／JSON——**對的值來自錯的來源不算量測**，
這是 brief 遵循度的缺口，不是身分不符；已列為後續。

**v0 → v1 的唯一變化是 `[判斷]` 標籤。** v0 的 c1 那格，glm 的解析樹與觸發條件
全對卻只能標「需升級」，P0 閘記 1/2；v2 讓它裁定並附推導之後，同一顆金絲雀、
同一份 diff、同一個引擎，五次全部提出 P0 finding。學習 1 的診斷因此成立：
**那格量到的是規則，不是引擎。**

嚴重度方面，學習 2 的判斷也再次成立且範圍更大：c2（expected P1）被
**glm 四次與 Opus 一次**同樣升為 P0，理由一致——文件不只宣稱過期，還據此指示讀者
「T3 工具一律視為免審批」，落進 v2 §3 嚴重度表第二列（權限邊界）。五個引擎跑次裡
只有 glm r5 給 P1。**這是金絲雀 key 與嚴重度表不一致的訊號，不是引擎失準**；
`c2/expected.md` 的 severity 應否改判由後續單處理（`線只升不降`，改 key 要操作者裁定）。

本節不執行 `edda decide`；帳本紀錄由控制者做，逐字版本在本單 PR body 的
`for-ledger — fleet.review-calibration v1` 區塊。

---

## 4. 替換規則（可執行順序）

1. **分類**：依 §1.1 路徑規則定出 PR 類別（可並列），控制者保守升類，不降類。
2. **候選池**：`qualified(該類別) ∧ transports_available ∧ quota_signal 在`
   的引擎集合（池見 §2）。
3. **選引擎**：取成本級最低者；平手取最近一次校準通過者。**sol 不是預設**——
   它是定線之錨與 `[判斷]` 抽審者。
   - **逐字定址**：選定後跑 `pi --list-models <關鍵字>`（claude 側同理），
     把該列**逐字貼進 brief**，`model_requested` 照抄目錄 id。**絕不用猜的、
     記憶中的或近似的名字**：pi 對不存在的 id 不報錯，會模糊解析到最近的一個
     （實證：`google/gemini-3-pro` → `google/gemini-3-pro-image`，§3），
     那是**派工端的靜默替換**，與 §5 的收據事故同級。
   - **派工前探測運輸**：跑一次 `--no-tools "reply with OK"`。`pi auth check`
     回 `ready` **不保證**路由可達（實證：`ready` 之後仍
     `404 … quantization: fp8`，§3）。
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

語法以 `edda decide -h` 實測為準（RAN 2026-09-02）：`<DECISION>` 是**一個**
`key=value` 位置參數，理由走 `--reason`：

```text
Usage: edda.exe decide [OPTIONS] <DECISION>

Arguments:
  <DECISION>  Decision in key=value format (e.g. "db=PostgreSQL")

Options:
      --reason <REASON>    Reason for the decision
```

以下五條可直接複製執行（每條都是一個 `key=value` 引數，不是兩個位置參數）。其中
`fleet.review-engine-pool` 一條在 ratify 時由操作者修訂：**初版值
`field-of-reviewer-profile-593`（把 gemini-3.1-pro-preview 列入初始池、fp8 路由修好即可列候選）已作廢**，
下列才是 ratifier 後的版本（`fleet.review-engine-pool`，2026-09-02）；初版值僅存於本段歷史標記：

```sh
edda decide "fleet.review-class=two-classes-path-rule" \
  --reason "審查類別先開兩類：code-risk＝diff 觸及任何可執行/可編譯物（crates/**, scripts/**, *.sh, *.rs, workflows, install.sh…）；docs-skills＝diff 只動說明性檔（*.md, docs/**, skills/**, .claude/skills/**, 說明用 *.txt）。混合 diff 兩類並列、各掛對應清單。判定是路徑規則機械判定；控制者可保守升類（docs 指示破壞性操作時併審 code-risk 清單），不可降類。類別進判決欄位。依據：c3 實證 docs 能指示不存在的破壞性指令，初分靠路徑、宣稱行為靠內容升類。"

edda decide "fleet.review-canary-protocol=tests-canaries-diff-fixture-expected" \
  --reason "金絲雀存放 repo 內 tests/canaries/<class>/<name>/{fixture/, diff.patch, expected.md}；diff 只動 canaries-fixture/<name>/ 下的合成檔，對任何 repo 狀態可重現；fixture 是 diff 外的合成事實來源。跑法＝TEMP throwaway clone 開分支、fixture commit、git apply、canary commit、審查目標 git diff HEAD~1..HEAD，每引擎以 brief 模板唯讀審一次，對照 expected.md 記 caught/missed/FP。金絲雀是審查線的版本化規格，走 PR 審查；集只增不減，移除＝降線，需操作者裁定。"

edda decide "fleet.review-qualification=p0-full-p1-80-fp-zero-recal-quarterly" \
  --reason "合格門檻＝該類別 P0 金絲雀 100% caught（提出且實質命中）、P1 ≥ 80%、FP=0、清單每項皆回報不得靜默略過。嚴重度低估記 caught 但標「嚴重度不符」，同引擎同金絲雀連續兩次不符視為 missed。重校＝每季＋引擎版次變更後＋brief 版本變更後＋金絲雀新增後 30 天內全池。抓取率進帳本。校準前 qualified=none。"

edda decide "fleet.review-engine-pool=sol-anchor-opus-provisional-glm-docs-only-gemini-removed" \
  --reason "（2026-09-02 ratifier 修訂，取代已作廢的初版 field-of-reviewer-profile-593——初版把 Gemini 列在初始池，與本池相牴觸。）Gemini 從審查引擎池拿掉——審查很少會用 Gemini，不值得為它改 pi 的 openRouterRouting（order=[novita], quantizations=[fp8]，為 glm 設的）。池＝gpt-5.6-sol（錨，openai-codex provider；pi 或 edda dispatch --agent codex）、Opus（暫定合格，只走 claude -p；fleet.claude-subscription-transport，絕不經 pi/openrouter）、glm-5.3-flash（docs/skills 暫定；code-risk 不合格）。池是 reviewer profile（#593）底下的一個欄位，不是獨立設定；池條目＝{model_requested, transports_allowed, cost_tier, qualified_classes, quota_signal}。替換規則＝依類別取「合格∧運輸可用∧配額在」中最便宜者；model_requested 一律逐字照抄 pi --list-models 的目錄 id（pi 對不存在的 id 不報錯而是模糊解析，google/gemini-3-pro → google/gemini-3-pro-image 即派工端的靜默替換）；派工前以 --no-tools 探測運輸，auth check 回 ready 不保證路由可達；過載先換運輸再換下一合格引擎；重試同引擎一次為限，絕不靜默換模型；無合格引擎→PR 標 review:unreviewed 停住，不合格引擎的 LGTM 不算；[判斷] 項與非平凡 diff 零發現送 sol 抽審；判決帶 model_requested/model_observed（系統取得）、brief 版本、類別、escalations，合併政策讀欄位不讀標頭。model_requested≠model_observed 為 P0 事故。"

edda decide "fleet.review-unreviewed-state=honest-label-blocked-by-merge-gate-580" \
  --reason "review:unreviewed 是 label/狀態，不是判決——語意為「審查當下沒有任何合格引擎可用，誠實停住」。#580 合併閘機械化要求 current-head LGTM＋綠 CI，unreviewed 的 PR 不可能有有效 LGTM，閘自然擋住；解鎖唯一路徑是合格引擎真審一輪。不得用降級引擎的判決清除本狀態。依據：fleet.review-provider-overload「unreviewed PR 是誠實的狀態，便宜模型的判決不是」。"
```

---

## 7. 本文件未決定的事（與後續單）

操作者裁定 §6 的五個 key 之前，以下都只是紀錄，不是授權：

1. **brief 模板進 fleet-review skill**——`.claude/skills/**` 在本 lane 是
   FORBIDDEN；移入與文字收斂歸 #633（REVIEW.md），與 #598／#594 同檔。
2. **接線**——`--model`／`--exclude-tools`／`--allowedTools` 進 `edda dispatch`、
   `model_observed` 進收據＝#574 S1/S2/S5；池表掛進 profile 讀取路徑＝#593。
3. **watcher／儀表接線**（抓取率表、`review:unreviewed` 狀態面、quota_signal
   顯示）＝#632。
4. **brief v2**——**done — see calibration v1**（#884）。
   [2026-09-02-reviewer-brief-template-v2.md](2026-09-02-reviewer-brief-template-v2.md)
   把「shell 解析樹＋觸發條件」移出 `[判斷]`（校準學習 1）、把嚴重度定成查表
   （學習 2），`REVIEW.md` 的 R2 與 §6 同步改（`review-spec-v1.5`）。
   重校結果見 §3.1：同一顆 c1、同一份 diff，glm 五次全部提出 P0 finding。
5. **gemini 運輸修正**——**已作廢**（`fleet.review-engine-pool`，2026-09-02：Gemini 移出引擎池，#618 的 Gemini 後續作廢；以下保留為歷史紀錄）——id 已經確定：`openrouter/google/gemini-3.1-pro-preview`
   在目錄裡，`pi auth check --model` 與 provider `openrouter` 都回 `ready`。
   剩下的是路由：`404 … quantization: fp8` 表示送出的請求帶著 fp8
   provider preference，而 `google/*` 沒有 fp8 endpoint（同金鑰跑 glm 正常，
   帶不帶 `--thinking` 皆同）。要查明那個偏好是 pi 端送的參數還是 openrouter
   帳戶的路由設定，或改走直連 google 供應商（現為 `not_ready`）。
   修好前 gemini 維持 `not run`，四引擎校準的第四格仍是空的。
6. **重評 c1 的 sol／Opus 兩格**——**done — see calibration v1**（#884）。
   兩份 #618 transcript 這次讀得到，兩格在更正後的 key 下都成立、分數不變，
   逐字證據見 §3 的 ＊註。
7. **金絲雀重校的自動化**——把 §1.2 的跑法變成腳本／lane（#594 wiring-scan
   同條 lane 候選），目前是手動程序＋README。

## 8. 連結

- 金絲雀集 v0（格式、跑法、評分基準）：[tests/canaries/README.md](../../../tests/canaries/README.md)
- 審查 brief 模板 **v2**（現行派工來源）：[2026-09-02-reviewer-brief-template-v2.md](2026-09-02-reviewer-brief-template-v2.md)
- 審查 brief 模板 v1（歷史，v0 校準是在它之下量的）：[2026-09-02-reviewer-brief-template-v1.md](2026-09-02-reviewer-brief-template-v1.md)
- 上游：#618（本單）·#560（epic）·#574／#593／#594／#580／#582／#598／#632／#633
- 重校：#884（brief v2 ＋ §3.1 校準 v1）
