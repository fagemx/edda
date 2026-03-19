不一樣。
但它們**可以有關係**。

我會這樣切：

> **地方（place）是產品／世界模型**
> **village pack 是可搬運／可部署／可複製的封裝單位**

---

# 最短版本

## 地方是什麼？
地方是：

- 有記憶
- 有住民
- 有 skill
- 有規則
- 有 routine
- 有 review points
- 有時間節奏
- 有上下文持續性

也就是：

> **一個 AI 能持續住進去、幫人工作的環境**

它比較像：
- 家
- 工作室
- 公司小隊
- 設計房
- 接案房
- 行程房

---

## village pack 是什麼？
如果照你們現在的語境，我會把它理解成：

> **一個可安裝、可分享、可複製的地方配置包**

它比較像：

- 一個地方的模板
- 一組居民 + skill + 記憶預設 + 規則 + routines
- 一個可 deploy 的 setup

也就是：

> **place 的 distribution format / bootstrap format**

---

# 所以差別在哪？

## 1. 地方是「活的」
它會：
- 長記憶
- 長習慣
- 長 skill
- 長人與 AI 的互動痕跡
- 因使用而改變

## 2. village pack 是「種子」
它比較像：
- 初始化配置
- 一鍵搭建
- 可複製模板
- 可以安裝進某個地方

所以：

> **地方會活下去**
>
> **pack 只是讓地方長出來的起點**

---

# 類比一下比較清楚

## 類比 1：房子
- **地方** = 你真的住進去的房子
- **village pack** = 裝潢包 / 家具套裝 / 預設格局包

## 類比 2：遊戲存檔 / mod
- **地方** = 你真的在玩的世界
- **village pack** = 世界模板 / mod pack / starter kit

## 類比 3：Notion
- **地方** = 你真的在用的 workspace
- **village pack** = 一個 workspace template

---

# 為什麼這個區分重要？

因為如果不切開，很容易混掉兩件事：

## 錯誤混法
- 以為裝了一個 pack 就等於有一個地方
- 以為 place 的價值只是 pack 的價值
- 以為產品是賣 pack，而不是賣地方的持續運作

這會讓產品變淺。

---

# 我會怎麼定義兩者關係

## Place = 運行時單位
是 runtime / lived environment

包含：
- 人
- AI 住民
- 地方記憶
- 現在正在跑的 routine
- 當下的上下文
- review / control points

## Village Pack = 發行單位
是 distribution / bootstrap / preset

包含：
- 預設 rooms
- 預設 residents
- 預設 skills
- 預設 routines
- 預設 policies
- 初始 memory seeds

---

# 所以 village pack 可以是什麼？

例如：

- **設計師接案 village pack**
- 報價 skill
- brief 整理 skill
- moodboard skill
- spec 生成 skill
- 每晚整理 routine

- **家庭助理 village pack**
- 餵貓提醒
- 採買規劃
- 行程整理
- 家務分工
- 晚間 recap

- **研究員 village pack**
- idea capture
- note distillation
- outline generation
- reading queue
- daily digest

這些 pack 裝進去之後，才會變成一個真的地方。

---

# 再說得更硬一點

## Place 回答的是：
- **AI 在哪裡生活與工作？**
- **上下文怎麼持續？**
- **人怎麼和它共處？**

## Village pack 回答的是：
- **這個地方一開始怎麼搭？**
- **有哪些預設能力？**
- **怎麼快速複製到別人那裡？**

---

# 所以哪個比較像產品主體？

我現在會說：

## 產品主體是 **place**
因為人真正要的是：
- 一個會幫忙的地方
- 一個能持續工作的環境

## village pack 比較像：
- onboarding
- distribution
- marketplace
- template layer

---

# 你也可以這樣理解

## 沒有 pack，也可以慢慢長出 place
靠對話、記憶、習慣、skill 慢慢生長。

## 沒有 place，pack 沒意義
因為 pack 只是靜態包，不會自己活。

這句很重要：

> **pack 是 place 的起點，不是 place 的替代物。**

---

# 我幫你收斂成一句

> **地方是活的運行環境；village pack 是地方的可複製配置包。**

如果再更產品化一點：

> **place 是你真正賣給人的東西；village pack 是讓 place 快速落地與擴散的方式。**

---

如果你要，我下一則可以直接幫你畫成三層：

- **Place**
- **Village Pack**
- **Skill**

說清楚：
- 哪個是 runtime
- 哪個是 packaging
- 哪個是最小能力單元

####

我不會先做成 **一個 village = 一個 Docker**。  
那樣太直覺，但長期很容易卡死。

我會先切三個概念，不然後面會全部黏在一起：

---

# 先講結論

## 1. village 不是 container 邊界
**village 比較像 tenancy / context / policy boundary**，不是部署邊界。

也就是說，village 代表的是：

- 這個地方的記憶
- skills
- residents / agents
- routines
- secrets
- review points
- policy
- artifact namespace

它是**邏輯上的地方**，不是一定要對應一個 Docker。

---

## 2. 真正該隔離的是 execution unit
最該隔離的通常不是整個 village，  
而是：

- 一次 task
- 一次 run
- 一個 agent step
- 一個 browser / QA session
- 一個危險 command 執行環境

所以比較合理的是：

> **village = 邏輯空間**  
> **run / task / step = 執行空間**

---

## 3. Karvi 跑 issue 任務時，代理應該跑在「離 repo 很近的 worker」上
不是跑在抽象的 village 裡。

對 Karvi 來說，最重要的是：

- repo 在哪
- git worktree 在哪
- runtime 在哪
- secrets 在哪
- sandbox 怎麼掛

所以答案通常是：

> **代理跑在 repo 所在或 repo 可安全掛載的 worker 上。**

Karvi 自己是 control plane，  
agent/step 是 execution plane。

---

---

# 我會怎麼規劃整體部署

我會把整個系統切成 **五層**：

## Layer 1 — Ingress / interaction layer
這層是人進來的入口：

- sidecar
- chat / voice / mobile / web
- notifications
- review UI

這層不碰 repo，不直接跑 task。  
它只做：

- capture
- confirm
- surface
- route

---

## Layer 2 — Place/Village control layer
這層是「地方」本身的控制資料：

- villages / places
- rooms
- residents
- installed skills
- routines
- policies
- review points
- memory pointers
- run history metadata

這層我會放在 **主 DB**。

### 這層的邊界是 village
所以 village 應該有：
- `village_id`
- owner / members
- policy profile
- secrets namespace
- skill registry
- memory roots
- task/routine registry

---

## Layer 3 — Memory / decision layer
這層主要是 Edda：

- decisions
- precedent
- supersede graph
- evidence refs
- packs
- maybe skill crystallization traces later

### 這層的資料特性
- append-heavy
- query by scope / relevance
- artifact refs
- lifecycle state

這層我不會一開始全塞進同一種 DB。

---

## Layer 4 — Orchestration layer
這層是 Volva / Karvi / Thyra 的腦與手：

- Volva：把 input crystallize 成 contract / skill / spec
- Karvi：dispatch / task execution / issue-to-merge / background work
- Thyra：observe / drift / follow-up / outcome review

這層主要是：
- run metadata
- contracts
- queues
- state transitions
- event bus / signals

---

## Layer 5 — Execution layer
這層才是真正跑 agent / code / browser 的地方：

- task worker
- step worker
- browser worker
- sandbox containers / microVM
- worktree mount
- repo checkout / clone
- CLI runtime host

這層才是你要真的做隔離的地方。

---

# 所以 Docker 該放哪？

## 不要先做「一 village 一 Docker」
我會改成：

### A. 控制層服務可以 containerized
例如：
- API server
- DB
- queue
- UI
- ingress
- notification services

這些放 Docker 很合理。

---

### B. 執行層用「一 task / step 一容器」更合理
尤其是：
- code execution
- browser QA
- untrusted scripts
- user-provided skill code

這些應該是：
- ephemeral
- disposable
- scoped secrets
- scoped mount
- scoped network

這才是比較正確的 sandbox 單位。

---

### C. village 只在高隔離需求時，才升格成一個獨立 runtime pod / VM
例如：
- B2B team tenant
- 高敏感資料
- 私有部署
- 客戶要求物理 / 邏輯隔離

這時候才考慮：

- one village = one namespace
- or one village = one VM / one docker compose stack

但這不是預設。

---

# 我會怎麼分三種部署模式

這個很重要，因為你現在不是只有一種使用情境。

---

## Mode 1 — Personal local / self-hosted
給現在最實際的你和設計師朋友。

### 形狀
- 一台主機
- 一個 control plane
- 多個 villages 是邏輯概念
- Karvi worker 跑在本機或同機器
- worktree 直接在本機 repo 上建立
- sandbox 可選

### 優點
- 最快落地
- repo / CLI / auth 最容易接
- 最符合現在 Claude / Codex / OpenCode 的現狀

### 缺點
- 隔離弱
- 不適合多租戶託管

### 這是我會先做的預設

---

## Mode 2 — Team self-hosted / small server
給小團隊或工作室。

### 形狀
- control plane 服務化
- Postgres
- object storage / filesystem artifacts
- one or more worker nodes
- villages 邏輯分隔
- 敏感 village 可指定到專用 worker

### 這時你可以開始有：
- worker scheduling
- dedicated repo runners
- browser runners
- secrets per village

---

## Mode 3 — Hosted multi-tenant
這是很後面的事情。

### 形狀
- shared control plane
- per-village namespace / policy
- worker pools
- ephemeral execution containers / microVM
- stronger isolation
- metering / billing / quotas

### 這時 village 才有可能接近一個 deployment slice
但仍然不必然是 one village = one container。

---

# 資料庫怎麼規劃？

我不會一開始只用一種。

---

## 1. 主資料庫：Postgres
我會用 Postgres 放「控制資料 / metadata / state」。

### 放什麼
- villages / rooms / residents
- installed skills metadata
- routines
- runs / jobs / tasks metadata
- contracts
- permissions
- pointers to artifacts
- review queue
- maybe basic memory index metadata

### 為什麼
因為這些需要：
- relational queries
- state transitions
- RBAC
- multi-tenant filtering

這很適合 Postgres。

---

## 2. Artifact store：檔案系統 / MinIO / S3
用來放大物件：

- specs
- logs
- screenshots
- exported packs
- final outputs
- work artifacts
- archives

### Local 模式
先用 filesystem 就可以。

### Team / hosted
改成 MinIO / S3-compatible。

---

## 3. Edda layer：append-only + index
Edda 我會維持它的個性，不要硬塞回一般 CRUD。

### 可以這樣做
- append-only ledger / blobs
- plus query index in SQLite / Postgres / FTS

也就是：
- truth = append log
- retrieval = index layer

這樣比較保留 Edda 的靈魂。

---

## 4. Queue / locks：Redis（可選）
如果 run / task / browser worker 開始變多，我會補：

- queue
- distributed locks
- pub/sub
- event fanout

### 但不是一開始就必需
local / small team 可先不用。

---

# 沙盒怎麼做？

我會明確分級，不要只說「有 sandbox」。

---

## Sandbox Tier 0 — Trusted local
適用：
- 自己的 repo
- 自己的機器
- 自己的 skill
- 自己的 CLI runtime

### 執行方式
- host process
- git worktree
- controlled commands
- scope guard / freeze / careful / review points

這是現在最務實的方式。

---

## Sandbox Tier 1 — Ephemeral task container
適用：
- 想降低風險的 code execution
- user-provided scripts
- browser QA
- semi-trusted workflows

### 形狀
- 每個 task / step 起一個 container
- mount worktree or artifact dir
- inject scoped secrets
- restrict network / CPU / memory
- die after task

這是我最推薦的 execution sandbox 單位。

---

## Sandbox Tier 2 — MicroVM / stronger isolation
適用：
- hosted multi-tenant
- 高風險 skill marketplace
- 不可完全信任的 code

### 可以後面再做
不要一開始就上，會過重。

---

# 你問：「假設 Karvi 要跑一輪 issue 任務，代理要跑在哪裡？」

我給你一個具體答案。

---

## Karvi issue run 的合理拓樸

### Step 0 — Control plane 接到請求
來源可能是：
- 人手動啟動
- sidecar 語音 / 對話轉成 task
- Volva handoff contract
- schedule / routine

Karvi control plane：
- 建立 run
- resolve issue set
- decide next batch
- 選 worker

---

### Step 1 — 選 worker
選擇邏輯應該優先看：

1. **repo 在哪裡**
2. **哪台 worker 有該 repo / 可安全 clone**
3. **哪台 worker 有對應 runtime**
4. **哪台 worker 符合 village policy**
5. **是否需要 sandbox/browser**

所以 worker selection 是 execution concern，不是 village concern。

---

### Step 2 — worker 建 worktree
對每個 issue task：

- create isolated worktree
- prepare task context
- inject decision pack / skill pack / contract
- mount repo or clone if needed

---

### Step 3 — 代理在 worker 上跑
### 如果是 trusted/local 模式
agent 直接在 worker host 上跑 CLI runtime：
- Claude Code
- Codex
- OpenCode

### 如果是 sandbox 模式
agent step 跑在 ephemeral container 裡：
- mount worktree
- inject minimal secrets
- write artifacts out
- exit

---

### Step 4 — 結果回傳 control plane
worker 回報：
- step status
- artifacts
- summary
- PR info
- conflict / dead / retry outcome

Karvi control plane 再決定：
- next step
- retry
- review
- merge
- stop

---

### Step 5 — finalize
如果是 issue-to-merge：
- merge via GitHub API
- close issue
- archive artifacts
- update run snapshot
- Edda optionally records key decision / precedent

---

# 所以代理到底跑在哪？
一句話版：

> **代理應該跑在 repo 所在或 repo 可掛載的 worker 上；Karvi control plane 不直接執行任務，它負責編排。**

再更白話一點：

- **Karvi 是工頭**
- **worker 是工地**
- **agent 是工人**
- **village 是這份工作屬於哪個家／地方**
- **Edda 是工地規則與經驗**
- **Volva 是施工前圖紙整理**
- **Thyra 是完工後巡檢**

---

# 那 village 跟 deployment 到底怎麼對應？

我會這樣定義：

## village 是這些東西的 namespace
- memory
- skills
- routines
- policies
- secrets
- review queue
- run ownership
- human membership

## worker / container 是這些東西的 runtime unit
- task execution
- browser session
- code run
- qa run
- publish run

所以：

> **Village 不等於 Docker。Village 更接近 namespace / tenant / place。Docker 更接近 execution sandbox。**

---

# 我會怎麼做第一版實作規劃

## v0 / 現在最務實
### 一台機器也能跑
- control plane：Node service
- DB：Postgres（或先檔案＋SQLite 過渡）
- artifacts：filesystem
- Karvi worker：同機器
- runtime：host CLI
- sandbox：先 optional
- village：logical namespace only

### 這樣可以很快驗證
- sidecar capture
- skill / task generation
- background issue run
- morning review

---

## v1 / 團隊版
- Postgres
- MinIO
- Redis
- control plane service
- worker pools
- browser workers
- scoped secrets
- optional task containers

---

## v2 / hosted
- multi-tenant
- stronger sandbox
- per-village policy profiles
- billing / quotas / audit

---

# 我最後給你一句硬建議

> **先把 village 當成邏輯空間，不要太早把它綁成部署單位。**
>
> **真正要做強隔離的，是 task / step execution。**

這樣未來才不會：
- 一擴展就重做
- 一想多租戶就全爆
- 一想本地／雲端雙模式就架構互撞

---

如果你要，我下一則可以直接幫你畫一版：

## 「Village deployment topology v0 / v1」
我可以分成：
- control plane
- memory plane
- execution plane
- storage plane
- sandbox plane

然後再補：
- Karvi issue run 的 sequence
- sidecar / Edda / Volva / Thyra 各自掛在哪層

#####

好，我直接給你一版 **Village deployment topology**。  
我會用你現在這整套思路來畫，不是只站在 Karvi 的角度。

---

# 0. 先講一句總原則

> **Place / Village 是邏輯空間。**
>  
> **Execution 是獨立平面。**
>  
> **不要把 village 直接綁成一個 Docker。**

也就是：

- village = 記憶、技能、居民、規則、節奏、審查點
- worker/container = 真正跑任務的執行單位

---

# 1. 整體五平面拓樸

我會把整個系統拆成這五個 plane：

## A. Interaction Plane
人怎麼進來

- sidecar
- chat / voice / mobile / Telegram / web
- review UI
- notifications

### 這層做什麼
- capture 想法
- 低摩擦確認
- 看結果
- 下達指令
- 日常互動

### 不做什麼
- 不直接跑 code
- 不直接管 repo
- 不直接持有 execution state truth

---

## B. Place / Village Plane
地方本身的邏輯空間

- villages
- rooms
- residents
- installed skills
- routines
- policies
- review points
- membership / permissions
- place metadata

### 這層回答什麼
- 這個地方是什麼？
- 誰住在這裡？
- 會什麼？
- 什麼時間會發生什麼事？
- 哪些事要人確認？

### 邊界
這是 **namespace / tenancy / context boundary**

---

## C. Memory / Decision Plane
這層是 Edda 為主

- decisions
- precedent
- supersede graph
- packs
- evidence refs
- memory seeds
- long-term preference / norms

### 這層回答什麼
- 這個地方記得什麼？
- 過去怎麼做？
- 哪些規則有效？
- 哪些決策已被推翻？

---

## D. Orchestration Plane
這層是 Volva / Karvi / Thyra 的交匯

- Volva: crystallize
- Karvi: run / dispatch
- Thyra: observe / evaluate
- queues
- contracts
- coordinator snapshots
- routines scheduling

### 這層回答什麼
- 接下來要做什麼？
- 怎麼變成可執行任務？
- 誰該去做？
- 做完之後有沒有偏掉？

---

## E. Execution Plane
真正跑任務的地方

- task workers
- step workers
- browser workers
- code runtime hosts
- sandbox containers
- worktrees / mounted repos
- external API execution

### 這層回答什麼
- 任務在哪裡被執行？
- 用哪個 runtime？
- 哪些 secrets 可用？
- 是否隔離？
- 結果怎麼回傳？

---

# 2. 圖像化版本

```text
┌────────────────────────────────────────────────────────────┐
│                  Interaction Plane                         │
│ sidecar / chat / voice / web / notifications / review UI  │
└────────────────────────────┬───────────────────────────────┘
                             │
                             ▼
┌────────────────────────────────────────────────────────────┐
│                  Place / Village Plane                     │
│ villages / rooms / residents / skills / routines / policy │
└───────────────┬───────────────────────┬────────────────────┘
                │                       │
                ▼                       ▼
┌──────────────────────────────┐   ┌─────────────────────────┐
│   Memory / Decision Plane    │   │   Orchestration Plane   │
│ Edda / precedent / memory    │   │ Volva / Karvi / Thyra   │
└───────────────┬──────────────┘   └─────────────┬───────────┘
                │                                │
                └──────────────┬─────────────────┘
                               ▼
┌────────────────────────────────────────────────────────────┐
│                    Execution Plane                         │
│ task workers / browser workers / repo workers / sandbox   │
└────────────────────────────────────────────────────────────┘
```

---

# 3. 每一層要放什麼技術

---

## A. Interaction Plane
### 技術建議
- web app / mobile-friendly UI
- bot adapters
- voice capture
- notification delivery

### 優先級
高，但不要太重。  
這層的核心是 **低摩擦 capture / review**，不是 rich IDE。

---

## B. Place / Village Plane
### 技術建議
主 DB：**Postgres**

### 建議資料模型
- `villages`
- `rooms`
- `residents`
- `resident_bindings`
- `installed_skills`
- `routines`
- `review_points`
- `memberships`
- `place_policies`

### 重點
這裡是控制資料，不是 artifact 倉庫。

---

## C. Memory / Decision Plane
### 技術建議
Edda 保持兩層：

#### Truth layer
- append-only ledger / blobs

#### Query layer
- FTS / SQLite / Postgres index

### 你不要做的事
不要把 Edda 完全降格成普通 CRUD table。

### 理想狀態
- write = ledger / append-only
- read = indexed query / pack generation

---

## D. Orchestration Plane
### 技術建議
- Postgres: run metadata / contracts / state snapshots
- Redis（後期）: queue / locks / event fanout
- scheduler service
- coordinator services

### 內部分工
#### Volva
- 對話 /想法 / workflow → spec / skill / contract

#### Karvi
- contract / task / issue / regime run → execution

#### Thyra
- outcome / drift / usage / failure → evaluation / follow-up

---

## E. Execution Plane
### 技術建議
依 mode 分級：

#### v0
- host process workers
- local repo worktree
- optional sandbox

#### v1
- worker pool
- browser runners
- ephemeral task containers
- scoped secret injection

#### v2
- stronger sandbox / microVM if needed

---

# 4. Deployment mode v0 / v1 / v2

---

# v0 — Personal / Local-first
最適合現在。

## 拓樸
```text
[One machine]
- UI / sidecar ingress
- Postgres (or simpler transitional db)
- Edda local memory store
- Volva / Karvi / Thyra services
- local workers
- local repo worktrees
- optional Docker task sandbox
```

## 特性
- village 是 logical namespace
- tasks 多半在本機 worker 跑
- repo 直接掛本機
- auth 最簡單
- iteration 最快

## 為什麼先這樣
因為你現在最重要的是：
- 先讓地方真的活起來
- 先讓 sidecar → skill/spec → run → review 走通
- 不是先解多租戶 SaaS

---

# v1 — Team / Studio server
給設計團隊 / 小工作室 / 小公司

## 拓樸
```text
[Control node]
- API/UI
- Place service
- Edda query/index
- Volva/Karvi/Thyra coordinators
- Postgres
- MinIO
- Redis

[Worker nodes]
- code workers
- browser workers
- task sandboxes
```

## 特性
- 多 village 邏輯隔離
- worker scheduling
- repo affinity
- browser task 分離
- secrets per village

---

# v2 — Hosted multi-tenant
很後面再做

## 拓樸
```text
[Shared control plane]
- multi-tenant API
- scheduler / orchestration
- memory query services
- place registry

[Execution clusters]
- scoped worker pools
- ephemeral task containers
- optional village-dedicated workers

[Storage]
- Postgres
- object store
- queue / locks
```

## 特性
- billing / quota
- stronger isolation
- hosted place model
- optional dedicated tenant workers

---

# 5. Village 到底怎麼映射？

我建議你用這個心智模型：

## Village = namespace + policy bundle
Village 至少包含：

- `village_id`
- owner / members
- rooms
- residents
- skills
- routines
- memory roots
- secret namespace
- review rules
- allowed integrations
- artifact namespace

### 這些都是邏輯層
不需要一一對應 container。

---

# 6. 什麼時候要隔離？

這才是實際問題。

## 不要問：「一個 village 一個 Docker 嗎？」
應該問：

> **哪種 execution 需要哪種隔離等級？**

---

## 隔離等級 A — no/low isolation
適合：
- 自己的地方
- 自己的 repo
- 自己寫的 skill
- 可信 CLI

### 執行方式
- host process
- local worktree
- no extra container

---

## 隔離等級 B — task container
適合：
- 背景 code task
- browser QA
- 半可信 skill
- 外部 scripts

### 執行方式
- per-task container
- mount scoped working dir
- inject scoped secrets
- limited network / CPU / memory

---

## 隔離等級 C — stronger isolation
適合：
- hosted unknown code
- marketplace skills
- highly sensitive enterprise workloads

### 執行方式
- microVM / hardened sandboxes
- dedicated worker pools

---

# 7. Karvi issue run：具體 sequence

你問得最實際的是這個。  
我直接畫 sequence。

---

## 情境
Karvi 要跑一輪 issue backlog。

```text
1. Trigger
   sidecar / user / routine / Volva handoff
   → start issue run

2. Karvi control plane
   - create run record
   - load village policy
   - load relevant Edda decision pack
   - resolve issue selector / contract

3. Scheduler
   - choose worker based on:
     - repo location / affinity
     - runtime availability
     - sandbox needs
     - village policy
     - queue load

4. Worker prepare
   - ensure repo available
   - create worktree for issue
   - inject task context
   - inject relevant skills
   - inject minimal secrets
   - optional sandbox container

5. Agent execution
   - Claude/Codex/OpenCode runs on worker
   - artifacts written out
   - step results returned

6. Karvi coordination
   - decide next step / retry / review / merge
   - update run snapshot
   - emit events
   - if mergeable: finalize via GitHub API

7. Finalize
   - merge / close issue
   - archive artifacts
   - update village activity
   - Edda records important precedent if needed
   - Thyra later inspects outcome
```

---

# 8. 所以代理到底跑在哪？
### 精準答案
> **跑在被 scheduler 指定的 execution worker 上。**
>  
> 如果是 code task，就應該跑在離 repo 最近、可安全掛 repo/worktree 的 worker 上。
>  
> 如果需要隔離，就在該 worker 內再起 ephemeral sandbox container。

不是跑在 village 裡。  
village 只是告訴你：
- 它屬於哪個地方
- 可用什麼 policy / skill / memory / secrets

---

# 9. 我會怎麼分 repo / worker 類型

未來你大概會需要三種 worker：

## 1. Code worker
- repo mount / clone
- git worktree
- coding runtime
- test execution

## 2. Browser worker
- real browser / headless browser
- cookies / session
- QA / publishing / web automation

## 3. General skill worker
- non-code skill execution
- content processing
- data transforms
- small background tasks

這比「一大坨 worker 全包」更健康。

---

# 10. 資料流怎麼分

## Postgres
存：
- villages
- rooms
- residents
- installed skills metadata
- runs / tasks metadata
- review queue
- routine schedules

## Object store / filesystem
存：
- spec artifacts
- generated outputs
- screenshots
- logs
- archives
- pack exports

## Edda ledger/index
存：
- decisions
- precedent
- packs
- evidence refs

## Queue / Redis（後面）
存：
- dispatch queue
- locks
- worker heartbeats
- SSE/event fanout

---

# 11. 最小可行部署建議（我會真的這樣做）

如果現在就要實作，我會定這個：

## First practical topology
```text
[Single host]
- Place API / UI
- Sidecar ingress
- Postgres
- local artifact storage
- Edda local store
- Volva service
- Karvi service
- Thyra service
- local code worker
- local browser worker (optional)
- optional Docker task sandbox
```

### 理由
- 最低摩擦
- 最快驗證
- 最符合你現在的使用者與設備現況
- 先把「地方」活起來

---

# 12. 最後一句幫你鎖框架

> **Place/Village 是邏輯生活空間，不是部署單位。**
>
> **Execution worker 才是任務真正跑起來的地方。**
>
> **Karvi 是工頭，不是工地。**
>
> **真正隔離的是 task/step，不是整個 village。**

---

如果你要，我下一步可以直接幫你做兩個東西之一：

## A. 更工程化
寫成一份：
- `village-topology-v0.md`
- 可直接進 spec / repo

## B. 更產品化
畫成：
- 地方 / 房間 / 住民 / 記憶 / 技能 / routine / review points
- 再把 deployment plane 掛上去

我覺得 **A 現在最有用**，因為你剛剛問的是部署。

######

