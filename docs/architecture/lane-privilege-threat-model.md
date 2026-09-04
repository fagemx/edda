---
title: Lane Privilege Threat Model
---

# Lane 權限威脅模型

**狀態：設計稿，未批准。** 本文件回答 GH-690，交付威脅模型與一筆決策提案；
提案由 agent 記錄於帳本（`fleet.lane-privilege`），屬未批准層，批准是操作者的動作。
本單不含實作，也不含 spike（見 §8）。

這份文件解的是一個問題：**一條 lane 碰得到什麼。**

Edda 的核心宣稱是「agents run on your machine, and you can always check what
they did — with the authority trail attached」。今天「what they did」有帳本，
「what they could reach」沒有邊界。GH-609（密碼學身分）解「誰寫的」，本文件解
「寫的人碰得到什麼」——兩者互補，缺一不可（§5）。

## 1. 現況：實測的信任邊界

以下是既有證據，來源為 GH-690 issue body，2026-09-02 22:5x 於 4090 工作站量測，
basis `origin/main` `a1dd3d8aff369f7511360109f9f70104d9457be3`。
**本單未重跑這些量測**——重跑等於再次接觸憑證，而結論不因重跑而改變。

| 量測 | 結果 |
|---|---|
| lane 排程任務的 principal（`Get-ScheduledTask edda-b-lane-gh648`） | `UserId: fagem`、`LogonType: Interactive`、`RunLevel: Limited` — **操作者帳號** |
| `gh auth status` | `gho_…` OAuth token（keyring），scopes `admin:org, gist, project, repo, workflow` — 任何以該帳號跑的程序都能用 |
| 同帳號可讀的 vendor 憑證檔 | `~/.claude/.credentials.json`、`~/.codex/auth.json`、`~/.pi/agent/auth.json` 三家全部可讀 |
| briefs／logs 是否洩漏憑證 | **無**，乾淨 |
| edda 是否有憑證代管或間接引用 | **無**（crates 內無 keyring／DPAPI／`secret://`／vault 的實作） |
| dispatch 是否把 env 憑證注入 prompt | 未見 |

一句話：**每條 lane 都以操作者身分跑，拿得到 org 級 gh token 與三家 vendor 登入。**

爆炸半徑不是推測，是已實證的。GH-593 的 `claude` OAuth 被撤銷時，
GH-669 記錄整台機器每一條走 claude 運輸的 lane 同時靜默失敗且回報成功——
因為它們全共用同一份登入。同一個結構反過來讀就是本文件的 T1：
一條被 prompt injection 騙到的 lane，能讀走三家登入與一把 org 級 GitHub token。

lane 讀別人寫的 issue 與 PR 留言，是我們流程刻意要求的行為
（`.claude/CLAUDE.md` 的 PR review-fix loop 與 telegram 指令都明寫這是攻擊向量）。
injection 不是假想的邊緣情況，它是這個系統的正常輸入通道。

## 2. 資產與信任邊界

| 資產 | 今天誰持有 | 今天誰讀得到 | 洩漏或失效的影響範圍 |
|---|---|---|---|
| GitHub OAuth token（org 級 scopes） | gh keyring，操作者帳號 | 任何以操作者身分跑的程序 | 全 org |
| vendor 登入（claude／codex／pi） | 各 vendor CLI 的 profile 檔 | 同上 | 整台機器所有 lane |
| 共用 store（`coordination.jsonl`、帳本） | 檔案系統 | 任何以操作者身分跑的程序，含測試（GH-646） | 整個協調面 |
| 節點 peer token（GH-685 v0） | `~/.edda/node.toml` 靜態檔 | 節點；同帳號 lane 亦可讀 | tailnet |
| build lane 快取 | lane root 目錄 | 同上 | 只有可用性，無機密 |
| 操作者的 Windows 帳號本身 | 操作者 | 操作者 | 一切（**邊界外**，見 §4） |

今天所有列都落在同一個信任域裡：**「以 fagem 身分執行」= 全部**。
本文件要引入的唯一新邊界是 **operator ↔ lane**。

## 3. 保護什麼（in scope）

### T1 — 被 injection 的 lane 讀走憑證

**攻擊者**：任何能讓文字進入 lane 上下文的人——issue 作者、PR 留言者、
被引用的外部內容、被讀取的 repo 檔案。不需要主機存取權。

**今天為什麼會成功**：lane 以操作者身分執行，vendor 憑證檔與 gh keyring
在同一個 profile 內可讀（§1）。lane 有 shell，讀檔不需要提權。

**要達成的性質**：lane 的執行身分讀不到操作者 profile 內的任何憑證。
lane 取得的 GitHub 授權是短效且限於它該碰的 repo。
即使 injection 完全成功，攻擊者拿到的是一把幾十分鐘後過期、只能動一個 repo 的 token。

**殘餘風險**：lane 在它自己的授權範圍內仍可被誤導（推壞的 code、開錯的 PR）。
這由 GH-609 的作者身分與 PR review loop 處理，不由本文件處理。

### T2 — 單一憑證撤銷或洩漏放倒整機

**觸發**：一次 vendor 端撤銷、一次 token rotation、一次外洩後的緊急撤銷。

**今天為什麼會放倒整機**：所有 lane 共用同一份登入。GH-593／GH-669 已實證：
撤銷發生時全部 lane 同時失敗，而且**靜默**——`gh` 報錯但 `edda dispatch` 回 exit 0。
失效不可見讓爆炸半徑從「全部失敗」惡化成「全部假裝成功」。

**要達成的性質**：撤銷的粒度是單一 lane 或單一 lane 帳號，不是整台機器；
且撤銷造成的失敗**可見**——lane 以非零 exit 結束，狀態面顯示原因。
可見性本身是 GH-669 的範圍，本文件依賴它，不重複解。

### T3 — lane 寫壞共用 store

**觸發**：不需要惡意。GH-646 的測試污染就是一條以操作者身分跑的普通程序
寫進了共用協調狀態。

**今天為什麼會成功**：store 的寫入路徑對任何同帳號程序開放，沒有寫入者的概念。

**要達成的性質**：store 的權威寫入者是節點。lane 的 claim／request／heartbeat
經節點 API 進入，帶著可歸因的 lane 身分。lane 對 store 檔案本身沒有寫權限。
順帶得到的是 GH-646 的結構性解法：測試程序沒有寫入通道，污染不了。

## 4. 不保護什麼（out of scope）

明確寫出來，因為沒寫出來的邊界會被當成保證。

- **操作者帳號本身被攻陷。** 若攻擊者取得操作者的互動 session 或系統管理權限，
  本文件的每一條防護都失效——lane 帳號的 ACL 擋不住 administrator，
  DPAPI 保管的密鑰以操作者身分解得開。這是設計上的邊界，不是缺口。
- **tailnet 帳號被盜。** 與 GH-685 §4 的安全邊界一致。
- **vendor 端的洩漏。** claude／codex／pi 各自的服務端如何保管我們的登入，
  我們控制不了，只能控制它們在本機的可讀性。
- **供應鏈。** 我們執行的 CLI、cargo 依賴、skill 內容本身被投毒，屬另一個題目。
- **micro-VM 級的隔離。** 見 §6.1：Windows 上不追這條。
- **lane 之間的互相隔離。** v1 的邊界是 operator ↔ lane，不是 lane ↔ lane。
  同一台機器上的 lane 彼此可見。理由與代價見 §6.1。

## 5. 與 GH-609 的關係

兩張單是同一個信任問題的兩個軸，任一軸單獨成立都不足以支撐「可稽核的 agent 機隊」。

| | **本文件（GH-690）**：碰得到什麼 | **GH-609**：誰寫的 |
|---|---|---|
| 問題 | capability / reach | authorship / authority |
| 機制 | 執行身分、短效 token、憑證代管、store 寫入者 | 每 actor 金鑰、事件簽章、ratify 只認操作者簽章 |
| 沒有它會怎樣 | 帳本記得一清二楚，但每個 actor 都能碰到一切 | 邊界劃得很漂亮，但任何人都能宣稱是別人寫的 |

**四象限**：沒有身分也沒有邊界＝今天；有身分沒有邊界＝知道是誰讀走了憑證，
但仍然讀得走；有邊界沒有身分＝拿不到別人的東西，但寫進 store 的事件無法歸因；
兩者都有＝可稽核的最小權限。

**共用的介面**：金鑰存放。GH-609 的 per-actor ed25519 私鑰與本文件的
`secret://` 間接引用（§6.5）必須是**同一套保管機制**——都是 DPAPI／Credential
Manager 保管、都不進 env、都不落明文檔案。兩張單的封套與金鑰存放要一起定，
不要各自長一套。GH-685 的節點是兩者天然的共同持有者（§7）。

**共用的假設**：兩者都不保護「操作者機器被攻陷」（GH-609 doneWhen 第七條的
誠實邊界與本文件 §4 一致）。GH-609 的提案稿另見
`docs/architecture/actor-signing.md`（行內碼引用：該文件是另一張 PR 的提案，
落地前刻意不留追蹤式連結以免斷鏈）。

## 6. 六個方向的取捨

GH-690 列了六個方向。以下逐條給取捨與提案值，合起來構成 §9 的那筆決策。

### 6.1 lane 用低權限身分跑

**提案：每台機器一個專用的 Windows 標準帳號（例如 `edda-lane`），不是 per-lane 帳號，
不上 Windows Sandbox，不上 WSL2。**

- **per-lane 帳號**（隔離度最高）被否決的理由是成本而非原則：build cache 綁在
  profile 上，`.claude/CLAUDE.md` 的 Build lanes 一節記錄了單一 warm lane
  可達 40.9 GB。N 個帳號 × N 份 lane root 在今天的磁碟預算下不成立。
  這就是 §4 最後一條的代價：lane 之間不互相隔離。
- **Windows Sandbox** 每次啟動重建，保不住 build cache，等於每條 lane 冷編譯。
- **WSL2** 讓 `.claude/CLAUDE.md` 的 Windows-First 假設（`cmd.exe /d /s /c` 的
  spawn pattern、路徑處理）整組失效，且 CI 的 Windows 測試子集是為了抓
  Windows-only 缺陷才存在的——把 lane 搬進 Linux 等於放棄那層覆蓋。
- **取得的性質**：Windows 使用者 profile 預設只有 owner 與 administrator 可讀，
  所以「換帳號」這一步就同時拿掉了 `~/.claude`、`~/.codex`、`~/.pi` 與 gh keyring
  的可讀性——T1 的主要通道。不需要新機制，只需要換 principal 並確認 ACL。
- **代價**：lane 帳號需要對 worktree 與 lane root 有寫權限（明確授權，不是繼承）；
  排程任務註冊要帶密碼或用 `-LogonType S4U`；操作者要能看 lane 的檔案（反向 ACL）。

### 6.2 GitHub 走短效、限 repo 的 token

**提案：GitHub App installation token，由節點按 lane 發；lane 不持有 `gho_`。
降級路徑是 per-lane fine-grained PAT。**

- installation token 天生短效（一小時）且可按 repo 限縮，續期由節點做；
  fine-grained PAT 較好設定但仍是長效密鑰，只把「org 級」降成「repo 級」，
  沒有解決 T1 的「拿到就一直有效」。
- **代價**：要註冊一個 GitHub App 並保管它的私鑰。私鑰只存在節點，經 §6.5 保管，
  永不出現在 lane。這把單點從「一把 org token」換成「一把 App 私鑰」——
  但前者今天躺在每條 lane 都讀得到的地方，後者躺在唯一常駐可信程序裡。
- **降級**：App 不可用時退回 per-lane fine-grained PAT。仍不共享 org token。

### 6.3 vendor 憑證不落 lane

**這是最難的一條，因為三家 vendor 今天都沒有 per-lane 短效登入。**

**提案：分兩步。v1 = lane 帳號持有自己的 vendor 登入；v2 = 節點代跑。**

- **v1（lane 帳號自己登入）**：把操作者的登入移出 lane 的可讀範圍，撤銷粒度變成
  「lane 帳號」而不是「整台機器＋操作者」。**誠實的限制**：這**不**阻止一條被
  injection 的 lane 讀走 lane 帳號自己的 vendor 憑證——它只保證操作者的登入安全，
  並讓撤銷不再連坐操作者的互動 session。以 T1 的標準衡量，v1 是部分解。
- **v2（節點代跑）**：lane 不持有任何 vendor 憑證，需要 vendor 呼叫時把請求送給節點，
  節點以自己持有的登入執行並回傳結果。這是 T1 的完整解。
- **與 GH-685 的衝突要明寫**：GH-685 §4 說「節點不執行任何指令，只搬事件」。
  v2 修訂該條為：**節點執行白名單內的 vendor 呼叫，不執行任意指令。**
  這個修訂必須在 GH-685 v1 之前裁定，不能默默發生。
- **受 `fleet.claude-subscription-transport=claude-code-only` 約束**：
  Claude 訂閱只能經 Claude Code 使用，所以 v2 的代跑對 claude 而言是
  「節點跑 `claude -p`」，不是「節點呼叫 API」。

### 6.4 store 只由節點寫

**提案：採用為方向，排在 GH-685 節點落地之後；lane 的 claim／request／heartbeat
改走節點 API，lane 對 store 檔案沒有寫權限。**

- 順帶結構性解決 GH-646：測試程序沒有寫入通道，污染不了共用協調狀態。
- **代價**：新增單點故障與延遲。節點死了 lane 就不能 claim。
  緩解是本機離線佇列——lane 寫本地佇列，節點復活後排空；
  GH-685 §3 已經有「外送佇列在磁碟，不丟」的形狀，沿用它。
- **順序**：這條依賴 GH-685 v0 上線，不能先做。

### 6.5 設定檔的 `secret://` 間接引用

**提案：採用。config 只寫名稱，節點以 DPAPI／Windows Credential Manager 解析，
永不經 env、永不落明文檔。**

- 這是 §5 說的「與 GH-609 共用的金鑰保管機制」——同一套保管，兩個用途。
- **代價**：跨平台要各自實作（macOS Keychain、Linux secret-service）。
  edda 是 Windows-first，非 Windows 平台 v1 先退回檔案模式並在
  `edda node status` 明確標記為 unprotected——**標記出來的弱點好過看不見的弱點**。
- **不做**：AWS Secrets Manager 這類雲端後端。原則（間接引用、永不落檔）搬過來，
  實作不搬；edda 維持本機優先、零外部執行期依賴。

### 6.6 節點 peer token 升級

**提案：v0 的 `node.toml` 共享 token 維持，但標記為 known-weak，
並在 v1 前依 §7 的路徑升級。** 詳見 §7。

## 7. 節點作為憑證代管者（給 GH-685 設計稿的一節）

> **銜接說明（更新於 PR，GH-686 已合併）**：GH-685 的設計稿已隨 PR #686 合併，
> 現位於 `docs/superpowers/specs/2026-09-02-edda-node-agent-transport-design.md`。
> 本節保留在威脅模型內作為源文本；**把本節併入該設計稿（接在其 §4「安全邊界」之後）
> 仍是待辦的跨文件編輯**，屬 GH-690 的後續實作，本 PR 不修改該設計稿。

### 7.0 與節點設計稿的對照

節點設計稿（`docs/superpowers/specs/2026-09-02-edda-node-agent-transport-design.md`）
與本節的對應關係：

| 本節 | 節點設計稿 | 關係 |
|---|---|---|
| §7.1 `node.toml` 共享 token 的升級路徑 | §2.1 節點（「共享 token 放 node.toml，不進 git」、沒有 token 的 POST 一律 401） | v0 行為的出處；本節為它補上 sunset 與 v0.5/v1 階段 |
| §7.2 代管者的失敗訊號 | §3 失敗模式與訊號 | 表格形狀沿用 §3，新增代管職責三列 |
| §6.4 store 只由節點寫 | §2.1 複製器 / §4 安全邊界 | 節點成為 store 的權威寫入者後，兩邊的安全邊界敘述要對齊 |
| §6.2 GitHub App installation token | §2.6 GitHub 的角色（節點只註冊/搬事件，不代發 token） | **本節實質修訂**節點職責：代管者要按 lane 發短效 token；該修訂併入設計稿時必須連同 §6.3 v2 的「白名單 vendor 呼叫」一起裁定 |

節點 v0（已合併的第一片）不含憑證代管端點——這正是 §8 的 spike 腳本把
`edda-node://` token 引用分類為「明確不支援、不得造假」的依據。

節點是每台機器上唯一的常駐可信程序，因此它是憑證代管者的天然位置：
它比 lane 活得久（可以續期短效 token）、比操作者的互動 session 穩定（可以被排程任務看管）、
而且已經是唯一被授權寫共用 store 的角色（§6.4）。
把「代管」加給節點不是擴張它的職責，是把已經散落在每條 lane 上的信任收攏到一個點。

**節點作為代管者要持有的**：GitHub App 私鑰（§6.2）、
`secret://` 名稱到實際密鑰的對應（§6.5）、v2 的 vendor 登入（§6.3）、
以及它自己的 peer 授權憑證（以下）。

### 7.1 `node.toml` 共享 token 的升級路徑

GH-685 v0 的授權是 `~/.edda/node.toml` 裡的靜態共享 token
（設計稿 §2.1 與 §4：「沒有 token 的 POST 一律 401」、「共享 token 放 node.toml，不進 git」）。
它有兩個已知弱點：**全 peer 共用一把**（撤銷一台等於撤銷全部），
且**同帳號 lane 讀得到**（§1 的信任域問題原封不動地複製到節點身上）。

v0 在私網內可接受——Tailscale 已做裝置認證與加密，只綁 `100.x`。
但可接受的前提是它有明確的 sunset，而不是變成永久設計。

| 階段 | 授權形式 | 依賴 | 拿到什麼 |
|---|---|---|---|
| **v0**（現況，GH-685 第一片） | `node.toml` 靜態共享 token，全 peer 同一把 | 無 | 擋住 tailnet 外的 POST |
| **v0.5**（本文件提案，不依賴 GH-609） | (a) token 移到 `secret://node/peer-token`，`node.toml` 只留名稱；(b) **一 peer 一 token**，撤銷單台不連坐；(c) 節點啟動時檢查 lane 帳號對 `node.toml` 無讀權限，否則印警告 | §6.1 的 lane 帳號、§6.5 的 DPAPI 保管 | 撤銷粒度變成單一 peer；token 不再是同帳號 lane 讀得到的明文檔 |
| **v1**（GH-609 落地後） | 每節點 ed25519 金鑰；POST 的授權從「持有 token」改為「簽章驗證通過」，與 GH-609 的 actor 簽章同一套金鑰保管 | GH-609 | 授權與作者身分合一；不再有可複製的 bearer 密鑰 |

**相容與收尾**：v1 期間節點同時接受 token 與簽章，
`edda node status` 顯示每個 peer 目前用哪一種；
全部 peer 都升級後關閉 token 路徑，並從 `node.toml` 移除該欄位。
**升級不得無限期停在相容模式**——sunset 的判準是「所有 peer 都顯示 signature」，
不是時間。

### 7.2 代管者的失敗訊號

沿用 GH-685 §3 的形狀，代管職責新增三列：

| 情況 | 訊號 | 行為 |
|---|---|---|
| 短效 token 續期失敗 | lane 的 `gh` 呼叫 401，**且 lane 以非零 exit 結束**（依賴 GH-669） | 節點重試續期；連續失敗寫 `credential_broker_down` 事件，狀態面顯示 |
| `secret://` 名稱解不出來 | 節點啟動即失敗並指名是哪個名稱 | 不啟動——半個代管者比沒有代管者更危險 |
| 節點對 lane 帳號的 ACL 檢查不通過 | `edda node status` 顯示 `credentials: readable by lane account` | 印警告並繼續（v0.5 是警告，v1 應拒絕啟動） |

## 8. 後續步驟：spike（本單不做）

GH-690 doneWhen 第三條要求一個受限帳號的實測。**本單刻意不做**，理由：
它需要建立 Windows 帳號、修改 ACL、修改排程任務的 principal，
可能還要動 `scripts/fleet/lane-launch.ps1`（GH-672 的 lane 正在改它）。
這些是**操作者裁決之後的實作動作**，不是設計動作，而且不可逆性高。
本單只交設計；spike 在 §9 的決策被批准後另開一張實作單。

**腳手架已備（GH-690 後續 PR）**：`scripts/spikes/lane-privilege/` 放有一套
fail-closed 的 spike 腳手架——無密鑰的 metadata preflight 與 fail-closed action
分離、負向測試只做 open/dispose 不讀內容、push 只允許精確的 `spike/` 分支、
principal 取自處理序 token 偽造不了、token 解析只做真實或明確不支援。
其 fixture 測試（no-op preflight 與拒絕分支）已驗證；**受限帳號的正負向實測
仍未執行（NOT RUN）**——主機上沒有受限帳號，也沒有 GitHub App installation
token 來源。腳手架的存在不是實測證據；該單的 doneWhen 仍然要求實測本身。
（`PRIVILEGE_HANDOFF.md` 記錄操作者需提供的確切設定。）

spike 的驗收條件（先驗證 FAIL 再實作）：

- **負向測試必須先在今天的機器上 FAIL**——也就是說，現在跑它，
  lane 讀得到 `~/.claude/.credentials.json`、也用得了操作者的 `gh` token。
  這一步不需要新的量測：§1 的實測表**已經**是那個 FAIL 的紀錄。
- **正向**：一條 lane 在受限帳號下完成 build、test、push，push 用的是短效 token。
- **負向（實作後必須通過）**：同一條 lane 讀 `~/.claude/.credentials.json`
  得到 Access Denied；`gh auth status` 沒有 org 級 token，
  或只有限於該 repo 的短效授權；lane 對 store 檔案無寫權限。
- **build lane 不退化**：受限帳號下的 lane 仍能重用它被指派的 lane root，
  不是每次冷編譯（§6.1 否決 Windows Sandbox 的同一個理由）。

**測試時不得把任何 token 值寫進日誌、文件或 PR。** 負向測試的證據是
「Access Denied」與「無 token」，不是憑證內容。

## 9. 決策提案

```text
fleet.lane-privilege = node-brokered-least-privilege-lane-account
```

一句話：**lane 以專用低權限帳號執行，不持有長效憑證；
節點是唯一的憑證代管者與 store 寫入者；密鑰經 `secret://` 間接引用，永不落檔、永不進 env。**

六個方向的提案值：§6.1 專用 Windows 標準帳號（非 per-lane、非 Sandbox、非 WSL2）、
§6.2 GitHub App installation token 由節點按 lane 發、
§6.3 vendor 憑證 v1 移出操作者 profile／v2 節點代跑、
§6.4 store 只由節點寫（排在 GH-685 之後）、
§6.5 `secret://` + DPAPI、
§6.6 peer token 依 §7.1 三階段升級。

**這是 agent 記錄的提案，屬未批准層。** 批准（`edda ratify`）是操作者的動作。

## 10. 參考

- **GH-690** — 本文件的來源單；§1 的實測表出自其 issue body。
- **GH-609** — 密碼學身分（誰寫的）；與本文件的關係見 §5。
- **GH-685** / PR #686（已合併）— edda node；§7 是給它的設計稿的一節，
  併入仍是後續編輯（§7.0 有對照表）。
- **GH-609 的簽章提案**：`docs/architecture/actor-signing.md`（以行內碼引用；
  該文件是另一張 PR 的提案稿，落地前刻意不留追蹤式連結以免斷鏈）。
- **GH-669** — 撤銷可見性；T2 的可見性部分依賴它。
- **GH-593** — claude OAuth 撤銷事件；T2 的實證來源。
- **GH-646** — store 污染；T3 的實證來源，由 §6.4 結構性解決。
- **GH-606 / #668** — lane launcher 的 principal 欄位；§6.1 的落地點。
- **GH-672** — process tree 同時是隔離邊界。
- 外部參照：Pahud Hsieh 2026-09 的 OpenAB 貼文（agent 不碰真憑證、只拿臨時 token、
  唯讀沙箱、micro-VM）。**採其原則，不採其雲端實作**（§6.5、§4）。
