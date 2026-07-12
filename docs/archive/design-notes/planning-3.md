我換更落地的講法，不講 plane 名詞先。

## 先講最短答案

### 四個 repo 不要這樣部署
- 一個 repo 一個 Docker
- 或一個 village 一個 Docker

這樣太早、太重、也很容易卡。

### 我建議先這樣理解
- **Edda / Volva / Karvi / Thyra** 是四個後端能力
- 部署時不一定是四個獨立產品面
- 比較像 **幾個 service + 一組 workers + 一個共用資料層**

---

# 一、如果今天就要做，最實際的部署方式

我會先做 **v0：單機 / 單主機模式**

## 同一台機器上跑這些

### 1. 一個主 API / control service
這個 service 對外提供：
- sidecar 入口
- web UI
- review 介面
- routine / schedule
- village / place 管理

這個 service 不一定叫哪個 repo，實際上比較像**統一入口層**。

---

### 2. 四個能力服務可以先「同進程或同 compose」
#### Edda
- 管 memory / decision / packs

#### Volva
- 管 skill/spec crystallization

#### Karvi
- 管 dispatch / run / issue workflow / background execution

#### Thyra
- 管 observation / outcome / follow-up

### 重點
一開始不一定要拆成四個獨立 deployment。
你可以先：

- 同一個 server process 裡分模組
- 或同一個 docker-compose 裡 2~4 個 service

**先邏輯分層，晚一點再物理拆分。**

---

### 3. 一個主資料庫
我會先用：

- **Postgres**：place / village / runs / routines / metadata
- **檔案系統或 MinIO**：artifacts / screenshots / outputs / spec
- **Edda 自己的 ledger/index**：decision truth + query

如果你要更務實，早期甚至可以：
- Postgres + local files
就夠了。

---

### 4. 一組 worker
真正跑任務的是 worker，不是 village。

worker 負責：
- 跑 Karvi issue task
- 跑 skill
- 跑 browser automation
- 跑 code generation / QA / publish

所以你真正要想的是：

> **不是四個 repo 怎麼部署**
>
> **而是 worker 怎麼接這四個能力。**

---

# 二、那四個 repo 各自到底掛在哪？

我直接講你比較能想像的版本。

## 1. Edda
### 像什麼
- place 的長期記憶櫃
- precedent / decision store

### 部署方式
- 可以是 library + local store 開始
- 之後才獨立成 memory service

### 早期建議
**先不用單獨起一個 Edda server**
除非你真的已經有：
- 多 worker
- 多 village
- 多種 retrieval client

不然先做成：
- app server 內呼叫 Edda layer
比較務實。

---

## 2. Volva
### 像什麼
- skill/spec 生成器
- 把對話和 workflow 變成 skill 草稿 / spec 草稿

### 部署方式
- 非同步 job service
- 或主 API 裡的一個 orchestration module

### 早期建議
先不要獨立 heavy service。
因為它目前更像：
- pipeline step
- background job
- LLM-based transform layer

---

## 3. Karvi
### 像什麼
- 背景工作引擎
- 真正把 issue / task / skill 執行起來

### 部署方式
這個比較值得獨立：
- **Karvi control service**
- **Karvi workers**

因為它最接近：
- queue
- dispatch
- task state
- repo execution
- issue-to-merge flow

### 你問「代理跑在哪裡」
答案就是：

> **代理通常跑在 Karvi worker 上。**

不是跑在 Edda、Volva、Thyra 裡。

---

## 4. Thyra
### 像什麼
- observer / evaluator
- outcome / drift watcher

### 部署方式
- 可以先做 background analysis job
- 不需要先做常駐大 service

### 早期建議
先當：
- cron / routine / background evaluator
- 針對 run 結果做觀察
就好。

---

# 三、所以我會怎麼拆成服務

## v0 最實際版本
### Service A：Place Server
包含：
- sidecar ingress
- UI
- village/place config
- routines
- review queue
- 呼叫 Edda / Volva / Karvi / Thyra

### Service B：Karvi Control
包含：
- run creation
- queue / task orchestration
- worker scheduling

### Service C：Workers
包含：
- code worker
- browser worker
- general skill worker

### Storage
- Postgres
- files / MinIO
- Edda store

---

# 四、用 Docker 怎麼做比較好？

## 我建議的 docker-compose 心智模型

### 容器 1：app / place-server
- UI
- API
- place logic
- sidecar gateway

### 容器 2：karvi-control
- orchestration
- dispatch
- task state

### 容器 3：worker-code
- repo / worktree / coding runtime

### 容器 4：worker-browser
- browser automation / QA / publish-related web tasks

### 容器 5：postgres
- metadata

### 容器 6：minio（可選）
- artifacts

### Edda / Volva / Thyra
早期可以：
- 先內嵌在 app / karvi-control
- 不一定各自獨立容器

這樣比較合理。

---

# 五、最重要的一句：Karvi 跑 issue 任務時，代理要跑在哪？

## 正確答案
**跑在有 repo / worktree / runtime 的 worker 上。**

### 具體流程
1. 使用者或 routine 觸發一個 issue run
2. place server / Karvi control 建立 run
3. Karvi 選一台 worker
4. worker：
- 準備 repo
- 建 worktree
- 注入 skill / decision pack
- 執行 agent
5. 結果回傳 control
6. control 決定下一步 / merge / review / retry

所以：

- **Karvi 是工頭**
- **worker 是工地**
- **agent 是工人**

這樣想最準。

---

# 六、那 village 跟 deployment 的關係到底是什麼？

## village 比較像：
- namespace
- policy bundle
- memory boundary
- skill boundary
- routine boundary
- review boundary

### 一個 village 有：
- skills
- residents
- rooms
- routines
- secrets namespace
- memories
- active work

### 但它不需要有：
- 一個專屬 container
- 一個專屬 DB
- 一個專屬 process

除非未來是：
- 企業客戶
- 高敏感隔離
- hosted multi-tenant
才需要升級隔離。

---

# 七、那有需要做成桌面板嗎？

## 我的答案：**不是一開始最需要，但很可能值得做。**

不過不是傳統「厚桌面 app」那種想法。

---

## 先講結論
### 第一階段
先做：
- **web control panel**
- mobile-friendly
- notification / quick capture
- review UI

### 第二階段
再做：
- **桌面 sidecar / tray app**
- global hotkey
- quick capture
- 任務狀態浮窗
- 快速 approve / reject / review

### 不建議第一天就做
- 很重的 Electron IDE 式桌面板
- 把所有能力塞進桌面殼裡

---

# 八、為什麼桌面板不是第一優先？

因為你現在的第一性需求是：

> **讓忙的人不用花時間也能讓事情被推進。**

這意味著第一優先應該是：

- 隨手 capture
- 背景執行
- 低摩擦 review
- 早上看結果
- 晚上丟想法

這些其實：
- web + notification + mobile + sidecar
就能先做到很多。

---

# 九、但為什麼未來桌面板又值得做？

因為「地方」這個概念，最後很需要一個：

> **持續在身邊、低存在感、隨時可打開的入口**

桌面板如果做得對，會很像：

- 系統列小屋
- 常駐 sidecar
- 快速記錄點子
- 看今天有哪些 AI 住民在忙
- 一鍵 review
- drop file 進去就開始跑 skill

這種很有 place 感。

---

# 十、我會怎麼規劃桌面板

## 不做 IDE
而做 **ambient control surface**

### 它應該有：
- quick capture
- routine timeline
- active workers / residents status
- pending reviews
- morning digest
- drag-and-drop 生成 task / skill / spec
- notifications

### 它不應該有：
- 太完整的管理後台
- 太多 DevOps 面板
- 太多底層部署設定
- 太像你要去「操作一個系統」

這個差很多。

---

# 十一、我會怎麼排優先順序

## 第一波
- web app
- sidecar/chat ingress
- review queue
- Karvi workers
- Postgres + files
- optional docker sandbox

## 第二波
- worker pool
- browser worker
- better village policy model
- basic Thyra observation
- Volva skill/spec crystallization flows

## 第三波
- desktop tray / sidecar app
- richer place UI
- stronger isolation
- hosted multi-tenant option

---

# 十二、如果我要直接給你一句部署建議

> **先做一個 Place Server + Karvi Control + Worker Pool 的架構。**
>
> **Edda / Volva / Thyra 先當能力模組掛進去，不要急著一 repo 一 service。**
>
> **桌面板先不要做成主產品殼，先做 web + sidecar；等 place 感成形後，再做常駐桌面入口。**

---

如果你要，我下一步可以直接幫你畫得更具體：

## 版本 A
**docker-compose v0 草圖**
- service names
- volumes
- worker mount
- DB / artifact store

## 版本 B
**place desktop concept**
- 桌面板應該長什麼樣
- 什麼功能放桌面
- 什麼功能留 web

我覺得你現在最需要的是 **A**。

#####

好，我直接給你一版 **docker-compose v0 草圖**。
目標不是完美，而是：

> **讓 place/village 先活起來**
>
> **讓 Karvi 能跑任務**
>
> **讓 Edda/Volva/Thyra 先以能力模組掛進去**
>
> **不要一開始就拆成四個獨立產品服務**

---

# 1. v0 的核心原則

## 不做
- 一個 repo 一個 container
- 一個 village 一個 container
- 一開始就微服務滿天飛

## 先做
- **1 個 place-server**
- **1 個 karvi-control**
- **2 種 worker**
- **1 個 postgres**
- **1 個 artifact store（先本機 volume 就行）**

---

# 2. v0 compose 服務拓樸

```text
place-server
├─ sidecar ingress / API / web UI / village config / review queue
├─ 內掛 Edda client / Volva client / Thyra client
└─ 呼叫 karvi-control

karvi-control
├─ run orchestration
├─ task queue / dispatch
├─ worker scheduling
└─ issue-to-merge control logic

worker-code
├─ repo access / worktree / coding runtime
├─ Claude / Codex / OpenCode
└─ optional sandbox spawn

worker-browser
├─ browser automation / QA / publish-related web tasks
└─ isolated browser session

postgres
├─ villages / rooms / residents
├─ routines / runs / tasks metadata
└─ review queue / configs

artifact volume
├─ specs
├─ logs
├─ screenshots
├─ outputs
└─ archives

edda-store volume
├─ append-only ledger
├─ blobs
└─ query index
```

---

# 3. 你現在四個 repo 在 v0 裡怎麼放

## Edda
**先不要獨立 container。**

放法：
- place-server 直接讀寫 Edda store
- karvi-control 在需要 decision pack 時讀 Edda
- worker 接收到的只是打包好的 relevant pack，不直接操作 Edda core

## Volva
**先不要獨立 container。**

放法：
- place-server 裡面做一個 background job / module
- 對話、workflow、點子 → spec 草稿 / skill 草稿
- 產物丟到 artifacts + postgres metadata

## Karvi
**最值得獨立成 service。**

放法：
- karvi-control 獨立 service
- 真正協調 worker、queue、run state
- 之後最好就讓它保持獨立

## Thyra
**先不要獨立 container。**

放法：
- place-server 或 karvi-control 裡的 routine job
- 吃 run 結果、artifact、review outcome
- 先做簡單 observation，不先做大服務

---

# 4. 目錄與 volume 規劃

我建議先這樣：

```text
./deploy/
docker-compose.yml
.env

./data/
postgres/
artifacts/
edda/
browser/
logs/

./repos/
karvi/
edda/
volva/
thyra/
user-projects/
```

---

## Volume 對應

### postgres
- `./data/postgres:/var/lib/postgresql/data`

### artifacts
- `./data/artifacts:/app/artifacts`

### edda
- `./data/edda:/app/edda-data`

### browser session
- `./data/browser:/app/browser-data`

### repo mount
- `./repos:/repos`

---

# 5. docker-compose v0 草圖

下面這版是**概念可跑型**，不是最終 production hardened。

```yaml
version: "3.9"

services:
postgres:
image: postgres:16
container_name: village-postgres
restart: unless-stopped
environment:
POSTGRES_USER: village
POSTGRES_PASSWORD: village
POSTGRES_DB: village
volumes:
- ../data/postgres:/var/lib/postgresql/data
ports:
- "5432:5432"
networks:
- village-net

place-server:
build:
context: ../apps/place-server
container_name: place-server
restart: unless-stopped
depends_on:
- postgres
- karvi-control
environment:
NODE_ENV: development
PORT: 3000
DATABASE_URL: postgres://village:village@postgres:5432/village
ARTIFACTS_DIR: /app/artifacts
EDDA_DATA_DIR: /app/edda-data
KARVI_CONTROL_URL: http://karvi-control:3461
ENABLE_VOLVA: "true"
ENABLE_THYRA: "true"
volumes:
- ../data/artifacts:/app/artifacts
- ../data/edda:/app/edda-data
- ../data/logs:/app/logs
ports:
- "3000:3000"
networks:
- village-net

karvi-control:
build:
context: ../repos/karvi
dockerfile: Dockerfile
container_name: karvi-control
restart: unless-stopped
depends_on:
- postgres
environment:
NODE_ENV: development
PORT: 3461
DATABASE_URL: postgres://village:village@postgres:5432/village
ARTIFACTS_DIR: /app/artifacts
REPOS_ROOT: /repos
WORKER_CODE_URL: http://worker-code:4101
WORKER_BROWSER_URL: http://worker-browser:4102
EDDA_DATA_DIR: /app/edda-data
volumes:
- ../data/artifacts:/app/artifacts
- ../data/edda:/app/edda-data
- ../data/logs:/app/logs
- ../repos:/repos
ports:
- "3461:3461"
networks:
- village-net

worker-code:
build:
context: ../apps/worker-code
container_name: worker-code
restart: unless-stopped
environment:
PORT: 4101
REPOS_ROOT: /repos
ARTIFACTS_DIR: /app/artifacts
SANDBOX_MODE: host
DEFAULT_RUNTIME: codex
volumes:
- ../repos:/repos
- ../data/artifacts:/app/artifacts
- ../data/logs:/app/logs
- /var/run/docker.sock:/var/run/docker.sock
ports:
- "4101:4101"
networks:
- village-net

worker-browser:
build:
context: ../apps/worker-browser
container_name: worker-browser
restart: unless-stopped
environment:
PORT: 4102
ARTIFACTS_DIR: /app/artifacts
BROWSER_DATA_DIR: /app/browser-data
volumes:
- ../data/artifacts:/app/artifacts
- ../data/browser:/app/browser-data
- ../data/logs:/app/logs
ports:
- "4102:4102"
networks:
- village-net

networks:
village-net:
driver: bridge
```

---

# 6. 每個 service 的責任邊界

## `place-server`
### 角色
- 對外主入口
- sidecar ingress
- place / village CRUD
- room / resident / skill 安裝
- routines
- review queue
- Volva / Thyra 的高層調用

### 不做
- 不直接跑 code task
- 不直接持有 repo worktree

---

## `karvi-control`
### 角色
- 接收「要執行什麼」
- 建 run
- 解析 issue / task / contract
- 選 worker
- 追蹤 task 狀態
- 決定 retry / review / merge / stop

### 不做
- 不直接執行 Claude/Codex/OpenCode
- 不直接開 browser

---

## `worker-code`
### 角色
- 真正跑 agent
- 建 worktree
- mount repo
- execute step
- optional sandbox spawn

### 重要
這台才是 **Karvi issue run 時 agent 真正跑的地方**。

---

## `worker-browser`
### 角色
- QA
- browse
- publish
- authenticated web flows
- 截圖 / 互動

---

# 7. Karvi issue run 的 sequence（對應 compose）

## 使用者觸發
可能來自：
- web UI
- sidecar 對話
- routine
- Volva handoff

---

## 流程

### Step 1 — place-server 收到請求
例如：
- `幫我跑 karvi #598`
- 或某個 village routine 決定要跑 issue backlog

place-server 會：
- 查 village policy
- 查 installed skills
- 查 Edda relevant decisions
- 生成 execution request
- 發給 `karvi-control`

---

### Step 2 — karvi-control 建 run
它會：
- 建一個 run record
- resolve issue / task selector
- 決定需要 code worker 還是 browser worker
- 選一台 worker

---

### Step 3 — worker-code 準備 repo
它會：
- 找 repo：`/repos/karvi`
- 建 worktree
- 注入：
- task context
- relevant skill packs
- Edda decision pack
- runtime config
- 視設定決定：
- host process
- 或再起 ephemeral sandbox

---

### Step 4 — agent 真正執行
例如：
- Claude Code
- Codex
- OpenCode

都在 `worker-code` 跑。

---

### Step 5 — 結果回 karvi-control
karvi-control 看：
- step succeeded?
- 要 review?
- 要 retry?
- 要 merge?
- 要 stop?

---

### Step 6 — place-server surface 給人
如果需要：
- review queue 出現一條待確認
- sidecar 通知你
- 早上看 dashboard

---

# 8. 沙盒怎麼規劃

v0 我會給你三種模式，透過 env 切。

## `SANDBOX_MODE=host`
最務實，先跑起來。
- worker 直接在 host/container 內跑 CLI
- 適合自用 / trusted repo

## `SANDBOX_MODE=docker-task`
每個 task / step 再起一個 ephemeral 容器。
- 比較安全
- 適合 untrusted skill / 高風險任務

## `SANDBOX_MODE=off`
純本機開發測試

---

## 我建議 v0 預設
### 自用開發
`host`

### 做 demo / 想測隔離
`docker-task`

### 不要先上
- microVM
- k8s namespace per village
- one village per pod

太早。

---

# 9. 資料庫 schema v0 先放什麼

## Postgres 裡先放
### villages / places
- `villages`
- `rooms`
- `residents`
- `installed_skills`
- `routines`
- `review_items`

### orchestration metadata
- `runs`
- `run_steps`
- `tasks`
- `worker_leases`
- `contracts`

### user-facing history
- `activity_feed`
- `notifications`

---

## Edda 不要全塞進 Postgres
Edda 先維持：
- append-only truth
- query index

所以：
- truth 在 `./data/edda`
- place-server / karvi-control 只透過 Edda layer 讀 relevant packs

---

# 10. 桌面板在這個 v0 裡放哪？

你剛剛上一題有問。
在這個拓樸裡：

## 不需要先做桌面厚 app
place-server 已經可以：
- 提供 web UI
- 提供 mobile review
- 提供 notification

## 如果要做桌面
下一步比較像：
- tray app / sidecar shell
- 連到 place-server API
- 做 quick capture / quick review

所以桌面不是主後端的一部分，
它只是 interaction plane 的另一個 front-end。

---

# 11. 為什麼這版比較對？

因為它保留了兩件最重要的彈性：

## 1. village 不會被物理邊界綁死
未來你要：
- 一台機器多 villages
- 一個 village 多 workers
- hosted multi-tenant
都還能長。

## 2. Karvi execution plane 可以獨立強化
未來你可以：
- 多 code workers
- 多 browser workers
- repo affinity
- stronger sandbox
- queue / Redis
都不需要重寫 place 模型

---

# 12. 我給你的建議落地順序

## Sprint 1
先把這些跑起來：

- postgres
- place-server
- karvi-control
- worker-code
- local artifact volume
- Edda local store

### 先不要做
- worker-browser（如果還沒用到）
- docker-task sandbox
- Thyra heavy analysis
- Volva full pipeline

---

## Sprint 2
補：
- worker-browser
- review queue
- Edda decision pack injection
- Volva spec/skill draft endpoint

---

## Sprint 3
補：
- optional task sandbox
- routines
- Thyra observation jobs
- better worker scheduling

---

# 13. 最後一句幫你鎖定

> **v0 不要部署四個 repo 當四個獨立產品。**
>
> **先部署成一個 place-server + 一個 karvi-control + 一組 workers。**
>
> **Edda / Volva / Thyra 先作為能力層掛進去。**
>
> **Karvi issue 任務時，代理跑在 worker-code，不跑在 village 裡。**

---

如果你要，我下一步可以直接幫你再補：

## 版本 B：更具體的
- `apps/place-server` 要有哪些 endpoint
- `apps/worker-code` 要有哪些 API
- `karvi-control` 怎麼跟 worker 溝通

也就是把這份 compose 草圖再往**接口層**推一版。

######

對，這句是對的。
如果你的產品真的要**幫人工作**，而不是只當聊天介面，那：

> **桌面側一定要有一個本地 agent / sidecar / companion。**

不是為了做桌面 app 而做桌面 app，
而是因為：

- 要讀本機檔案
- 要看 Downloads / Desktop / 專案資料夾
- 要接本機 app / 瀏覽器 / cookie / 剪貼簿
- 要監看使用者當前上下文
- 要在本地直接觸發 skill / workflow

這些事情，**純 web panel 做不到**，或做起來很痛。

---

# 所以我會修正剛剛那句

## 不是：
- 桌面板不是第一優先

## 而是：
- **如果產品要真的工作，必須有本地 companion**
- 但不一定先做成「厚重桌面工作台」

這兩句不衝突。

---

# 我會怎麼切

## 你真正需要的是兩個東西

### 1. Local companion（必要）
這是本地常駐程式，真的能碰電腦。

負責：
- 讀本機檔案
- 監看指定資料夾
- 存取 clipboard
- 調用本機 CLI / app
- 管理 browser session / cookies
- 執行本機 skill
- 跟 place-server / orchestration 層同步

這個才是「能工作」的核心。

---

### 2. Desktop shell / panel（可薄）
這是桌面 UI。

負責：
- quick capture
- pending review
- active routine status
- resident / worker 狀態
- 一鍵 approve / reject
- 查看今天 place 幫你做了什麼

這層可以一開始做薄，
但 **local companion 本身不能沒有**。

---

# 換句話說

## 真正必要的不是桌面「板」
而是桌面「手」

你需要一隻真正碰得到使用者電腦的手。
那隻手就是：

> **local sidecar / local companion / local agent daemon**

UI 只是它的臉。

---

# 所以整個部署圖要改一下

## 原本五層裡，Interaction Plane 要拆成兩半

### A. Local Interaction / Companion Layer
跑在使用者電腦上

- tray app / desktop app
- local agent daemon
- file access
- OS integration
- browser integration
- local skill runtime
- quick capture / quick review

### B. Remote Place Layer
跑在 server 上

- place-server
- village config
- routines
- orchestration
- shared memory / history / runs

---

# 我會怎麼定義這個本地 companion

## 它不是單純 Electron UI
它應該是：

> **一個帶能力的本地代理節點**

至少要有這些能力：

### 檔案
- 讀指定目錄
- watch changes
- open / save / move
- 對 skill 暴露 file handles / path access

### 系統
- clipboard
- notifications
- global hotkey
- drag-and-drop intake
- maybe screenshot capture

### 瀏覽器
- authenticated session
- cookies
- current tab context
- web automation handoff

### 本地執行
- 跑 skill
- 跑 scripts
- 跑 CLI runtimes
- 觸發 worker-like local jobs

### 同步
- 接收 server 下發的任務
- 回傳 artifact / status / logs
- 依 village policy 決定哪些資料可上傳

---

# 這樣一來，四個 repo 的角色又更清楚了

## sidecar
最像這個本地 companion 的前端／人格層
它負責：
- 跟人互動
- 吸收上下文
- 低摩擦收集需求
- 陪伴式入口

## Edda
本地 companion 會讀寫的記憶層
例如：
- 本地偏好
- 最近任務
- relevant pack
- decision cache

## Volva
把本地收集到的材料：
- 對話
- workflow
- 檔案
- 習慣

轉成：
- skill 草稿
- spec 草稿
- contract 草稿

## Karvi
如果任務需要真正執行：
- 本地 companion 可直接做 local execution
- 或把任務交給 remote Karvi control + workers

## Thyra
看：
- 哪些本地 skill 真有用
- 哪些 routine 常被忽略
- 哪些流程 friction 太大

---

# 所以代理要跑在哪裡，答案變成兩種

## 類型 1：本地敏感 / 本地上下文任務
例如：
- 讀桌面文件
- 看設計素材
- 吃剪貼簿
- 用本機瀏覽器登入狀態
- 整理下載資料夾
- 本地 app 操作

### 代理應該跑在哪？
> **跑在本地 companion 上。**

因為資料就在本機，搬出去反而麻煩又危險。

---

## 類型 2：重計算 / repo 執行 / server-side workflow
例如：
- issue-to-merge
- 大量 background generation
- browser QA farm
- 團隊 shared run

### 代理應該跑在哪？
> **跑在 remote worker / Karvi worker 上。**

---

# 所以你真正需要的是「雙層 execution」

## Layer 1：Local execution
- 跟使用者電腦黏很緊
- 快速、低延遲、敏感資料留本機

## Layer 2：Remote execution
- 背景批量任務
- 重工作
- shared work
- repo-centric workflows

這樣才符合真實需求。

---

# 我會怎麼重新畫 v0 拓樸

```text
[ User Computer ]
local-companion
- sidecar UI
- local daemon
- file access
- local skills
- browser/session access
- quick review
- optional local execution

│ sync / jobs / packs / artifacts

[ Server ]
place-server
karvi-control
postgres
edda store
artifacts
workers
```

---

# 你如果問我：那桌面板要做多重？

## 我的答案
### 必須有：
- 本地 companion daemon
- 最小桌面 UI / tray / quick panel

### 不必一開始就有：
- 巨大 IDE 式桌面工作台
- 全功能 control center
- 所有設定都放桌面殼裡

---

# 因為真正重要的是這個

> **地方要能住進使用者電腦。**

不是只有 server 上有個地方。
因為很多真正有價值的工作，都發生在：

- 本地檔案
- 本地 app
- 本地習慣
- 本地登入狀態
- 本地工作流

所以如果沒有 local companion，
你的「地方」其實只住在雲端，沒住進使用者生活裡。

---

# 我最後幫你收斂成一句部署原則

> **Server 端是 place 的大腦與工廠；桌面 companion 是 place 伸進使用者生活裡的手。**

這句我覺得很準。

---

如果你要，我下一步可以直接幫你補：

## 「local companion architecture v0」
我會拆成：
- local daemon
- desktop shell
- local skill runtime
- browser bridge
- sync protocol with place-server

這樣你就能把桌面板這件事，從「要不要做」變成「要怎麼做」。

#####

