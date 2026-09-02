# 策略：五參數，與 edda 的表達方式

- 日期：2026-09-02
- 狀態：操作者同意落檔（Tim：「可以 請改」）；框架為控制者提案，帳本 `product.strategy=data-not-code-five-parameters`，**2026-09-02 由操作者 ratify**（ratify 事件用 `edda log --family governance --keyword product.strategy --json` 查；`edda ask` 只顯示決策與時間線，不顯示 ratify）。
- 姊妹篇：[控制層與非 coding 形狀的執行層](2026-09-02-control-layer-and-l2-shapes-design.md)。
  那篇定義**形狀**（單一 lane 的契約）；本篇定義**策略**（形狀之上的組合），並回頭修改 #602、#603 的設計問題。
- 起點：操作者提到一位開發者自行設計「群狼」策略——用多個代理的多種組合去量測模型的極限——
  並問「怎麼設計策略、怎麼搭起來」。本篇的答案：**策略是資料，不是程式**；edda 是策略被宣告、
  執行、量測、比較的基底。

---

## 1. 形狀 vs 策略

| | 形狀（shape） | 策略（strategy） |
|---|---|---|
| 定義 | 單一 lane 的契約：有界單位／隔離／面／驗收載體／收據 | 多條 lane 的組合方式 |
| 例 | coding、研究、loop、內容 | 群狼、best-of-N、辯論、階梯、map-reduce、演化、偵察後投入 |
| 誰擁有 | L2 執行層 | L3 控制層（宣告、量測、選擇） |
| 不變量 | 策略**不改變**形狀的契約；它只決定開幾條、怎麼差、看得到什麼、誰勝、何時停 | |

我們每天跑的「實作 → 對抗審查 → 修復輪 → 合併」本身就是一個策略（兩人賽局加裁判），
只是從沒被當成策略設計過。把它寫成五參數，是本篇的第一個用途。

---

## 2. 五參數

| 參數 | 問題 | 常見取值 | edda 的表達 |
|---|---|---|---|
| **群體** population | 幾條 lane、同時還是接力 | 1、N 同時、接力鏈 | 一波裡的 plan 數（parallel-wave：一 issue 一單 phase plan）；接力＝`depends_on` 帶真實理由 |
| **多樣性軸** diversity | lane 之間差在哪 | 模型、思考深度、提示／切入角、工具面、temperature | **profile**（#593：agent + model + thinking + tools + budget）；切入角寫在 brief |
| **資訊流** information flow | 彼此看得到什麼、何時看 | 隔離、共享氣味（即時廣播）、只在結束時彙整 | **finding 物件**（#602）的可見範圍與時機；claim 的面（#581 process object） |
| **選擇** selection | 誰判成功、怎麼比 | 第一個通過驗證者、驗證者評分、投票、裁判 | **verdict**（`edda verdict`，綁 subject + SHA）；審查者 profile 與獨立性 |
| **停止** stop | 何時收手 | 預算、覆蓋完成、第一個 kill、操作者 | `--budget-usd`（#533 measured-ness）、campaign charter 的 stop 條件、操作者 |

五個參數都填了，策略就完整；少一個就是沒設計（最常漏的是**多樣性軸**與**停止**：
N 份同一提示不是群狼，是 N 倍成本；沒有停止條件的群狼是無底洞）。

---

## 3. 策略型錄

| 策略 | 群體 | 多樣性 | 資訊流 | 選擇 | 停止 | 什麼時候划算 |
|---|---|---|---|---|---|---|
| **群狼** | N 同時 | 切入角＋模型 | 共享氣味（finding 即時可見） | 第一個被驗證的 kill，或驗證者評分 | 預算或第一個 kill | 空間寬（搜尋、找缺陷、量極限）、有便宜驗證者、多樣性是真的 |
| **best-of-N** | N 同時 | 模型或 temperature | 隔離 | 驗證者選 | N 跑完 | 有硬驗證（測試、benchmark）；不需要協作 |
| **辯論／紅藍隊** | 2 對抗 | 角色（攻／守） | 互看對方論證 | 裁判 | 輪數 | 判斷型問題、審查、安全 |
| **階梯** ladder | 接力 | 模型由便宜到貴 | 上一階的失敗原因 | 通過驗證即停 | 到頂 | 多數任務簡單、少數難——**`fleet.agent-model-split` 就是它** |
| **map-reduce** | N 同時 | 切分的子空間 | 隔離，結束彙整 | 彙整者 | 覆蓋完成 | 可切分的掃描（campaign：object × lenses，一格一審查者） |
| **演化** | 世代接力 | 對最佳者的變異 | 上一代的評分 | 評分函數 | 世代數或收斂 | 有可執行的目標函數（AlphaEvolve 型）；沒有硬評估就是 reward hacking |
| **偵察後投入** | 1 → 1 | 研究 lane → 實作 lane | 研究的 finding | 實作的驗收 | 兩段各自 | 不確定該不該做、先花小錢問問題 |
| **實作–審查迴圈**（現況） | 2 接力 | 執行者 vs 審查者（模型、工具面） | 審查者只看 PR 與 diff | 審查判決 P0=0/P1=0 | 輪數上限、diminishing returns | 交付物有硬驗證載體（PR + CI） |

老實說的限制：2025–26 的證據裡，多代理群體的增益**多半來自驗證與選擇，不是協作**；
一個 context 好的強 agent 常常打贏一群弱 agent。群狼只在表格最右欄的條件成立時划算，
否則就是 N 倍成本。策略型錄的目的不是鼓勵開更多 lane，是讓「開幾條、怎麼差」變成可以量的決定。

---

## 4. edda 的表達方式：策略是資料

一個策略在 edda 裡 = **wave 模板**（群體、接力）＋ **profile**（多樣性）＋ **finding 可見規則**（資訊流）
＋ **verdict 規則**（選擇）＋ **預算與 stop**（停止）。零件盤點：

| 零件 | 現況 |
|---|---|
| 並行 lane、隔離、面 | 有：`edda conduct` 已同時跑多個 plan；worktree；`claim --paths` / `claim check`（#576） |
| 預算與誠實成本 | 有：`--budget-usd`、plan 級 measured-ness（#533）；讀端待 #582 |
| 判決綁 SHA | 有：`edda verdict`、`GateKind::Verdict` |
| 多樣性軸 | **缺**：旗標接線 #574、profile #593 |
| 資訊流 | **缺**：finding 物件 #602——且需要「可見範圍與時機」語意（本篇 §6） |
| 策略身分 | **缺**：沒有 `strategy_run_id` 把一波裡的 plan、finding、receipt、verdict 串成一次策略執行 |
| 比較 | **缺**：`report` 按策略 × 問題類別分組（#582 的延伸） |

### 4.1 策略放在哪一層：wave，不是 plan

parallel-wave 與決策 `cleanup.parallel-exec=split-single-phase-plans-worktree-lanes` 已裁：
一 issue 一單 phase plan，**並行發生在 plan 之間**，不在 plan 裡。所以策略不該是 `Plan` 的欄位——
它是 **wave 模板**：一份宣告五參數的資料，展開成 N 個單 phase plan，每個 plan、finding、receipt、
verdict 都蓋上同一個 `strategy_run_id`。conductor 不需要懂策略，只需要把 id 帶著走；
控制層的 `report` 用 id 分組。

### 4.2 兩個範例（示意，不是 schema）

群狼——量測某類問題上模型的極限：

```yaml
strategy: wolf-pack
run_id: wp-2026-09-02-claim-check-edges     # 蓋在每個 plan / finding / verdict 上
population: 4                               # 四條研究 lane 同時
diversity:
  profiles: [scout-opus, scout-gemini, scout-glm, scout-sol]   # #593 的 actor profile
  angles: [unicode-folding, glob-class-escapes, symlink-roots, case-insensitive-fs]
information_flow: shared                    # finding 一進 candidate 就對同 run 可見（#602 §6）
selection:
  verifier: reviewer-sol                    # 唯讀審查者 profile
  rule: first-verified-kill                 # 或 score
stop: { budget_usd: 8, or: first-kill }
```

階梯——實作：

```yaml
strategy: ladder
run_id: ld-gh574
population: 1                               # 接力，不並行
diversity: { profiles: [impl-glm, impl-opus] }   # 便宜先、審查失敗才升級
information_flow: previous-round-verdict    # 下一階看到上一階的判決
selection: { verifier: reviewer-sol, rule: p0-0-p1-0 }
stop: { rounds: 3 }
```

兩個範例只用到既有或已開單的零件；新東西只有 `run_id` 與 finding 的可見規則。

---

## 5. 策略成為實驗

帳本是實驗記錄本。每次策略執行留下：run_id、各 lane 的 profile、成本（measured／unmeasured 分開）、
finding 的產出與存活（candidate → verified → decision | dropped）、判決與輪數。控制層的
`report strategy` 回答：**哪個策略在哪類問題上，每美元產出最多 verified finding**。

這把策略設計從手藝變成經驗迴圈：那位開發者的群狼是手工實驗；在 edda 裡同一件事是可重複、
可比較、可稽核的。再往前一步，策略選擇本身可以被學：問題類別 → 歷史上最划算的策略——
那是 L3 的 `intake` 與 `promote` 讀報表做的事，不是新機器。

護欄（與控制層文件 §2.2 同一組）：measured-ness（#533/#584/#585）、requested vs observed（#574）、
finding 的作者不審自己的升級、`recorded ≠ ratified`——迴圈可以自轉，授權不自轉。
沒有這些，「策略比較」會變成 Goodhart 的溫床（最會討好驗證者的策略贏）。

---

## 6. 對既有設計單的修改

### #602 finding 物件：新增第 7 問——共享氣味的語意

群狼與 map-reduce 需要 finding 在**同一次策略執行內**即時可見（氣味），但全域可見會污染別的 run。
要決定：

- **可見範圍**：run-local（同 `strategy_run_id`）／project／global；預設建議 run-local，升級成 verified 後才 project 可見。
- **可見時機**：candidate 一寫入就廣播，還是 verified 才廣播。群狼要前者（氣味的價值在早），
  intake 要後者（未驗證的東西不該進佇列）。兩者可並存：候選對同 run 可見，驗證後對全域可見。
- **跨狼去重**：兩條 lane 報同一個 finding 時，合併規則與功勞歸屬（先到者？證據較強者？）。
- **投遞**：靠 memory pack／hook 注入（門鈴），不靠 lane 自己輪詢帳本——與 `orchestration.cross-platform`
  「訊息只是門鈴，真相在帳本」一致。

### #603 conductor 載體／check：新增第 7 問——策略身分怎麼帶

- `Plan` **不加** `strategy:` 區塊（parallel-wave 已裁：並行在 plan 之間）。
- 加的是 `strategy_run_id: Option<String>`：plan、phase 事件、receipt、verdict、finding 都蓋章，
  conductor 只透傳不解讀。
- wave 模板（五參數 → N 個單 phase plan 的展開）住在 parallel-wave 的 templates 或 `.edda/`，
  不住在 conductor——是否要一個 `edda wave` 動詞另議。
- `report` 按 `strategy_run_id` 與問題類別分組（#582 的延伸欄位）。

---

## 7. 沒開的單（列給操作者）

- `strategy_run_id` 蓋章＋`report strategy`（按策略 × 問題類別的 verified finding／美元）——等 #602、#603 決策後再開。
- wave 模板格式與 `edda wave` 動詞——等 #599（批次進料）與 #576（claim check）落地後再開。
