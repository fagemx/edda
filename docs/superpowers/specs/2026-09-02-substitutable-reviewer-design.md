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
