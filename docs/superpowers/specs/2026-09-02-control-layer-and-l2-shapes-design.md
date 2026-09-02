# 控制層（Layer 3）與非 coding 形狀的執行層（Layer 2）

- 日期：2026-09-02
- 狀態：操作者裁定（Tim）：Layer 3 命名為**控制層**；非 coding 的 Layer 2 形狀**研究先做**。
- 帳本：`product.layer3=control-layer`、`product.l2-next-shape=research`（`edda ask "product"`）
- 來源：本文件把 2026-09-02 一整天的討論落檔——Uber 成本工程與 SKILL.state 兩篇貼文、
  `edda dispatch` 接線缺口（#574）、審查靜默降級事故、併行瓶頸量測、plan-decompose 的診斷。
- 讀者：操作者與之後接手 #560 的控制者。這是**定位與邊界**文件，不是實作規格。

---

## 1. 兩層各自是什麼

README（PR #570）把 edda 定位為「一本帳、兩層應用」。把兩層寫成**物件＋動詞**，
第三層是什麼就會自己浮出來：

| 層 | 物件 | 動詞 | 回答的問題 | 死亡形式 |
|---|---|---|---|---|
| L1 帳本（記憶層） | event：decision / note / task / claim / receipt / digest | `decide` `ask` `note` `search` `ratify` | 決定過沒？做過沒？ | session 死了決策死 |
| L2 執行層 | lane：一個 agent 在隔離工作區裡做一個有驗收的單位 | `dispatch` `conduct` `claim` `verdict` | 誰在做什麼、會不會撞？ | agent 死了工作狀態死 |

L1 是領域無關的——fleet 裡 dazun 的定價單位裁定、yushan 的語域裁定，都是純業務決策進帳本。
L2 今天是 coding 形狀的：五個 bridge 全是 coding agent、`AgentKind` 只有 claude/pi/codex、
Plan 的 doc comment 直接寫「multi-phase AI coding plan」、check 是 `cmd_succeeds`、
交付載體是 git worktree → PR → CI → merge。

### 1.1 L2 的五個假設

L2 的 coding 形狀其實是五個假設的一組實作：

| 假設 | coding 的實作 |
|---|---|
| **有界單位**，帶驗收 | issue + doneWhen |
| **隔離**，平行 lane 不互撞 | git worktree |
| **面**，給碰撞偵測 | paths + symbols（`edda claim --paths`、`edda claim check`） |
| **驗收載體**，撐過 session 死亡 | PR + CI + review 輪次 + merge gate |
| **收據**回 L1 | `edda task done --receipt`、digest |

git/PR/CI 只是「隔離」和「載體」的實作，不是 L2 的本質。這是第 4 節通用化的依據。

---

## 2. Layer 3：控制層

### 2.1 定義

控制論的三件套：記憶、致動器、控制器。L1 是記憶，L2 是致動器，缺的是**控制器**——
量測 → 比較 → 決定下一步。這一層今天**存在，但是人＋一個 Claude session 手動在做**。

| 層 | 物件 | 動詞 | 回答的問題 |
|---|---|---|---|
| L3 控制層 | **signal**：heartbeat、freshness、cost、verdict、queue depth、surface intersection | `watch` `report` `promote` `intake` | 該做什麼、值不值、有沒有在動？ |

判斷「L3 是真的一層而不是功能堆」的測試：能不能說出它自己的物件和動詞。能，所以是。

### 2.2 邊界：機械半邊進產品，判斷半邊留給操作者

L3 產品化的是控制者的**機械半邊**：

- 交集判斷（parallel-wave Layer 1 → `edda claim check`）
- 狀態面（`conduct status` 統一 heartbeat，#567）
- 存活與回收（`edda fleet watch`，#573；dispatch lane 的 heartbeat，#569）
- 成本彙總與來源新鮮度（`edda report cost`，#582；measuredness #584/#585）
- 靜默死亡偵測（空 digest #578；freshness 行）
- 接線掃描與驗收槽（#594）、批次進料與確認表（#599）
- 角色 → 模型／工具的政策落地（lane profile，#593；旗標接線，#574）

**判斷半邊留給操作者**：方向、`fleet:pending → fleet:ready` 的 promote、`edda ratify`。
操作者不在三層裡，是三層之上的授權主體。

這與既有決策 `orchestration.cross-platform=ledger-data-plane-adapter-control-plane` 一致：
資料面是帳本物件（brief → 凍結 SHA → receipt → verdict），控制面是每平台一個薄 adapter 當門鈴，
loop 的控制權在 LLM 編排代理，conductor 退為派發工具箱。控制層就是把那個「LLM 編排代理」
手上可以機械化的部分，變成 edda 讀得到、算得出、違反會出聲的東西；不能機械化的判斷仍在
編排代理與操作者。

### 2.3 #560 就是 L3

epic #560 的標題寫著 "intelligence layer"——它就是控制層，只是還沒被叫這個名字。
建議把 #560 正名為 Layer 3，並把 signal / watch / report / promote / intake 寫進 README 的定位，
與 L1/L2 並列。已開的單全部落在這層：

| Issue | 控制層動詞 |
|---|---|
| #567 conduct status 統一狀態面 | watch |
| #569 dispatch lane 的 heartbeat | watch |
| #573 fleet watch 偵測孤兒 lane | watch |
| #578 空 digest 反覆寫入 | watch（freshness） |
| #582 report cost | report |
| #584 / #585 measuredness | report（誠實度） |
| #574 dispatch 接出 model/thinking/tools | promote 的執行面（政策可套用） |
| #593 lane profile | promote 的政策面（角色 → 模型／工具） |
| #594 驗收端 wiring verdict | intake 的驗收半邊 |
| #599 epic-split 吸收 plan-decompose | intake 的出生半邊 |

### 2.4 為什麼今天 L3 是人肉：2026-09-02 的量測

| 項目 | 數字 |
|---|---|
| Open issues | 45：`fleet:pending` 22、`fleet:ready` 4、其他 19 |
| 三天進料 vs 出貨 | 開 54 張 issue，合併 18 張 PR |
| 實際併行 | 9/1 同時開著 3–4 張 PR，合了 10 張——併行有發生 |
| 慢的 PR | 3–5 輪審查的 PR 6–9 小時；模型實際工時約 30 分鐘，其餘在等人接力 |
| 閒置 | 4 張 PR 等審 8–10 小時，其中 2 張七格全綠 |
| CI | 6–7 分鐘、GitHub 端平行；25 分鐘的兩次都是 #583 flake |
| 機器 | RAM 39.6 GB 剩 18.9、磁碟剩 349 GB、cargo 只有 2 個程序——算力沒用滿 |

結論：綁定限制不是 CI、不是磁碟；編譯只是 Rust lane 的軟上限（2–3 條）。卡住的是
**審查線**（每輪要控制者手動起審查、讀判決、貼、轉）與**升級閘**（逐張 promote），
兩個都是控制層還沒產品化的症狀。同一天的跨 session 訊息轉送也撞到 #569：一條有 claim
但沒有 heartbeat 的 lane，`edda request` 拒收——有工作狀態、沒有存活訊號，訊息無處投遞。
這就是 signal 缺席的樣子。

---

## 3. 非 coding 形狀的 L2

把 1.1 的五個假設換掉實作，就得到別的 L2：

| 形狀 | 有界單位 | 隔離 | 面 | 驗收載體 | 現況 |
|---|---|---|---|---|---|
| **研究** | 一個問題＋證據門檻（issue-intake 的 campaign charter：object / lenses / evidence bar / stop） | 每題一個 session ＋ scratch | 來源／主題重疊 → serial | **finding**：candidate → verified → decision；對抗式審查；RAN vs READ | 已經在跑——#587 的 post-merge 複審就是一條研究 lane：brief → detached pi → 判決 → 開單（#595–#598）。缺的只是 finding 這個物件當載體 |
| **Loop** | 一個 tick ＋ 一個檢查條件 | 無／共享 | 時間窗 | 檢查結果＋**freshness**（此形狀「靜默死掉」最兇：bg_digest 空寫十天） | fleet watch、bg digest、cost daily 都是；`task --after`、scheduled-tasks、verifiable-goal-loop 是零件 |
| **內容／營運** | 一篇稿或一份提案對一個 brief | draft | 篇／段 | 對照帳本**裁定**審（dazun 定價三鐵律、yushan 語域裁定已在 fleet 裡）→ publish | L1 對此形狀最值錢：`edda ask` 回的就是規則；L2 缺 draft 載體 |

三種的共同點：**載體不是 PR，但五個假設一個都不能少**。通用化不是「支援第四種 agent」，
是把 conductor 的兩個點拔成可插：

1. **載體**：PR 是一種；finding 與 draft 是另外兩種。
2. **check 種類**：`cmd_succeeds` 是一種；`gate: verdict`（已有）與 freshness 斷言是另外兩種。

不要重寫 parallel-wave / fleet-orchestrate：它們是消費端，契約統一後不用動。

---

## 4. 研究先做（操作者裁定）

### 4.1 為什麼是研究

1. **已經在用現有零件跑**：dispatch + brief + detached pi + 判決貼回。今天的複審、接線審計、
   campaign 都是研究 lane，只是用 issue comment 冒充載體。
2. **需要的新基建最少**：一個 finding 物件、一條證據門檻、issue-intake 已有的 promotion ladder。
3. **直接餵控制層的 intake**：研究 lane 的產出就是 issue（或決策）。#587 複審一趟產出四張單。

### 4.2 finding 物件草案

| 欄位 | 內容 | 對應既有物 |
|---|---|---|
| `question` | 一句話問題 | campaign charter 的 object |
| `basis` | 凍結的 full SHA / 文件版本 | `fleet.review-protocol=pin-commit-and-freeze` |
| `evidence_bar` | repro / failing test / trace / direct code proof | issue-intake templates |
| `attempts` | 試過什麼、看到什麼 | parked candidate 的 Attempted 欄 |
| `next_experiment` | 能升級或殺掉此 finding 的下一個有界檢查 | parked candidate |
| `state` | candidate → verified → decision（或 dropped） | promotion ladder |
| `verdict` | 審查者的 RAN vs READ 與判決 | Code Review round 的欄位 |
| `receipt` | 進 L1 的事件 id | `edda task done --receipt` |

狀態機就是 issue-intake 的 promotion ladder；驗收就是 fleet-review 的對抗式審查；
獨立性規則不變（finding 的作者不審它自己的升級）。

### 4.3 研究 lane 的五假設

| 假設 | 研究的實作 |
|---|---|
| 有界單位 | question + evidence_bar + stop 條件（coverage complete / budget / operator） |
| 隔離 | 每題一個 session id、scratch 目錄、唯讀工具面（`--exclude-tools edit,write`） |
| 面 | 來源／主題；兩條 lane 讀同一來源 → serial 或合併 |
| 驗收載體 | finding 物件（不是 PR）；審查判決綁 basis SHA |
| 收據 | finding 升級為 decision 或 issue 時的帳本事件 |

### 4.4 順序

研究 → Loop → 內容。L3 不另起 crate。

---

## 5. 外部佐證（簡述）

- **Uber 成本工程**（brewbytes.ai 轉述）：優化單位從「每百萬 token」改成「每個 PR / 每次 review」；
  sub-agent 預設便宜模型；context 當成本管理；MCP 工具延遲載入。對 edda 的映射：
  成本讀端（#582）、模型分工（`fleet.agent-model-split`、#574、#593）、pack 預算截斷（已有）。
  本地不建 context graph——ask/pack 就是本地尺度的等價物。
- **SKILL.state**（Google DeepMind，arXiv 2608.26263，貼文轉述）：每步只帶「技能說明＋
  結構化狀態＋最新觀察」，丟掉過程；價值上限由「寫入狀態的紀律」決定。對 edda 的映射：
  memory pack 就是這個形狀；#578 是 edda 自己的 state-writing failure；#575 的 session
  生命週期應偏向「從帳本 rehydrate ＋ 只讀增量」而非 `--fork`。稽核需要過程——用 hot/cold
  分層（pack 熱、transcript 冷、ledger 稽核）解決，不必二選一。

---

## 6. 後續單（操作者 2026-09-02 授權開立）

1. #601 — README 三層定位：把控制層（signal / watch / report / promote / intake）與 L1/L2 並列；
   #560 正名為 Layer 3。
2. #602 — finding 物件：schema、狀態機、`edda finding` 動詞（或掛在 task rail 上）、與 issue-intake
   promotion ladder 對齊（設計先行）。
3. #603 — conductor 載體／check 可插：PR / finding / draft 三種載體；freshness 與 finding-verdict
   check 並列於既有六種 check（`cmd_succeeds`、`edda_event`、`file_exists`、`file_contains`、
   `git_clean`、`wait_until`）之旁（設計先行）。
4. #604 — Loop 形狀的 freshness 一等訊號：writer 宣告預期節奏，staleness 不必讀 writer 即可見
   （設計先行；可與 #573 / #578 合併處理）。

---

## 7. 決策指標

- `product.layer3=control-layer` — Tim 2026-09-02 裁定命名；物件 signal，動詞 watch/report/promote/intake；機械半邊進產品，判斷半邊留操作者。
- `product.l2-next-shape=research` — Tim 2026-09-02 裁定順序：研究 → loop → 內容。
- 相關既有決策：`orchestration.cross-platform`、`fleet.agent-model-split`、`fleet.lane-profile`、
  `issue-intake.wiring-audit`、`verification.cost-discipline`、`fleet.review-protocol`。
