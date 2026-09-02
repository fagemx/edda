# Fleet 管理者代理：黑板、工作者、管理者（Layer 3 的迴圈）

- 日期：2026-09-02
- 狀態：設計稿。操作者已核准方向（「對，寫設計稿並開第一步的單」）並裁示判斷不回到人（「不可能給人 要自動化」）
- 基準：`origin/main` `a1dd3d8aff369f7511360109f9f70104d9457be3`
- 承接：`2026-09-02-control-layer-and-l2-shapes-design.md`（控制層 = signal / watch / report / promote / intake）
- 改變：那份設計稿 §2.2 把「判斷半邊留給操作者」。本稿依操作者 2026-09-02 裁示改為**判斷由管理者做，操作者只改規則**
- 第一步的單：GH-MANAGER-V0

## 0. 一句話

操作者只看一個地方（黑板上的一行狀態），只做一種輸入（改規則）。代理之間的所有協調不經過人。

## 1. 為什麼今天卡住（量測，全在 2026-09-02）

| 症狀 | 事實 | 根因 |
|---|---|---|
| 跨機重工 | #632／#634 各兩張 PR；#650 交還後 lane 未死、推了 PR #670 | 認領只在本機（`edda claim`）；「停止」只殺 wrapper（#672） |
| 控制者盲區 | 三件 lane 完成 40 分鐘無人接；9/1 四小時空轉（#573） | 控制者是 Claude session，會死、會盲、只能被人叫醒 |
| 人肉路由 | 三方要改 `operator-runbook.md`；七張單互相引用；每個裁示回到操作者 | 判斷半邊在人身上；訊息靠人轉 |
| 靜默失敗 | `edda dispatch` 認證失敗回 exit 0（#669） | 死亡不可見 |
| 身分漂移 | session 顯示名續接就變（`edda-b5` → `edda-7f`）；`edda peers` 三個活 session 全叫 `main` | 沒有固定角色名 |

host 訊息（Claude `SendMessage`）的實測結論：只保證收下、只送得到活著的 session、跨機單向。它是門鈴，不是真相（`docs/guides/multi-agent.md:130-136`、#446 關單留言）。所以本稿不做跨平台的叫醒，只做跨機的真相層與一個會自己醒來的管理者。

## 2. 三個角色，一塊板

### 2.1 黑板

GitHub issues 與 PRs。**唯一**的協調面。任務的持有者、進度、阻塞、決定，全部以留言與 label 寫在任務上。兩台機器讀同一塊板。

edda 帳本是黑板的本機鏡射（決策）與心跳面（存活），不是第二塊板。跨機鏡射照操作者已記錄的 `ledger.cross-machine-projection=committed-mirror-stamped-at-wave-close-quote-never-paraphrase`（PR #668 body）與 #671。

### 2.2 工作者

任何 agent（Claude、pi、codex 都可以）。只有三條規則：

1. 開工前讀自己的任務：issue body 加最新留言。
2. 有話寫在任務上（進度、問題、阻塞）。不傳話給人，不需要知道別的工作者是誰。
3. 做完或卡住寫在任務上（PR 連結，或 `blocked: <原因>`），然後停。

### 2.3 管理者

每台機器一個。每 N 分鐘（v0：5 分鐘）醒來一次，由排程任務啟動，不依賴任何 Claude session 活著。醒來做 §3 的迴圈，每個決定寫在任務上、附理由與引用的規則編號，然後結束。**不問任何人。**

### 2.4 操作者

不在迴圈裡。看黑板上的一行狀態；改 `docs/fleet/rules.md`；偶爾 `edda ratify`。唯一非操作者不可的事是認證（登入、token）。管理者遇到就停該運輸並記一筆 `needs-operator`，不重試（R11）。

## 3. 管理者醒來時的迴圈

### 3.1 讀（不改任何東西）

- 黑板：`gh issue list`（`fleet:ready`、taking 留言、`lane:*`、blocked）、`gh pr list`（`review:*`、CI、head SHA）。
- 本機：排程任務狀態、process tree、lane log 的 `=== EXIT ===` 行。三者一致才算「停」（#672）。心跳用 `edda peers --json`。
- 規則：`docs/fleet/rules.md`（git 追蹤，兩台一致）加帳本已 ratify 的決策（`edda ask`）。優先序：已 ratify 決策 > 操作者規則 > 管理者自訂規則。

### 3.2 比對後做

每一項都可逆、都寫回任務：

| 動作 | 觸發 | 規則 |
|---|---|---|
| 派工 | 有 `fleet:ready` 且無 taking 的任務，本機有空 lane | R1：先寫 `taking: <machine>/<role>` 再派；別台已 taking 就不派 |
| 起審、合併 | PR 開了或 push 了；LGTM 釘的 SHA 等於 current head | 審查沿用 PR 審查巡邏（#632）；合併照 R6 |
| 回收 | lane 無心跳、process tree 消失、log 有 EXIT 但任務沒 PR | 把本機 worktree 的 commit 狀態寫回任務，標 `blocked: lane died at <sha>`；重派時從該 commit 續做 |
| 解撞車 | 同一 issue 兩份產物；同一檔多方要改 | R2（留較完整）、R5（改多者先） |
| 判 lane 類型 | 任務被派錯機器 | R4（diff 需 cargo 就走 4090） |
| 無規則 | 上述都不涵蓋 | R8：自訂一條寫進 rules.md「管理者自訂」段，附案例，照做 |
| 被擋 | 不可逆、認證 | R7、R11：不做、不問、記一筆，繼續其他 |

### 3.3 寫狀態

黑板一行，寫在固定的 board issue 最新留言（v0 用 #613），格式：

```text
in-progress N · blocked N · needs-operator N · cost today $X · wake <time> · by <machine>/manager
```

超過兩個間隔沒更新，另一台的管理者把它標成 `needs-operator: manager on <machine> silent`。

## 4. 身分

角色名固定：`<machine>/<role>`，例如 `4090/manager`、`4090/lane-gh617`、`docs/manager`。用在 taking 留言、PR 留言、`EDDA_SESSION_LABEL`、心跳 label。session id、分支名、`edda-7f` 這類顯示名一律不當身分（R9）。

## 5. 跨機、跨平台

- 每台一個管理者，只管本機 lane。兩台的管理者只透過黑板互動（R1 先寫先贏）；同一分鐘平手時，機器名字典序小的讓。
- 跨平台：管理者用 `edda dispatch --agent <kind>` 起任何 agent；工作者只需要會讀寫黑板與跑 `edda`。五個 bridge 已經在同機共用同一套心跳與協調協定。
- 不做跨平台的叫醒。host 訊息只在同機當加速器（`fixate first, ring second`）。

## 6. 模型與成本

- 醒來讀板：便宜模型（`fleet.agent-model-split` 的執行檔）。
- 要下決定（撞車、無規則）：換強模型（sol；Opus 只在 `claude` 運輸可用時）。
- 管理者自身每日預算寫在 rules.md（R14）；超過就只做「讀加寫狀態」。

## 7. 前置缺陷（先修才可信）

- #669 `edda dispatch` 認證失敗回 exit 0（管理者靠 exit code 判成敗）。
- #672 停止不殺 process tree（回收判定要三證合一）。
- #617 claim check 死 session 假陽性；#656 跨機 issue 認領守門（R1 的機械版）。兩張已 ready。
- #668／#606 launcher 進 repo（管理者要用同一支起 lane）。

## 8. 分步

1. **v0，本週，只在 4090。** 巡邏殼複製自 `scripts/pr-review-watch.sh`；腦是 `fleet-orchestrate` 加本稿 §3；只做派工、回收、解撞車三件；`docs/fleet/rules.md` 以 2026-09-02 的裁示種好。單：GH-MANAGER-V0。
2. 第二台也跑管理者；角色名全面切換。
3. 帳本跨機（#671）；規則從 rules.md 搬回帳本。
4. 放權：升 ready、記決策、關單。條件是日誌證明連續三天沒有錯誤決定。

成功指標：操作者裁示次數每天為零；跨機重工為零；lane 完成到被接手不超過一個巡邏間隔；PR 從開到合併不再有等人的時間。

## 9. 非目標

跨平台叫醒、GitHub Actions 常駐 runner、重寫 conductor、第二塊板。

## 10. 決策

- `fleet.control-loop=manager-agent-on-schedule-judgment-by-rules`，supersedes 控制層設計稿 §2.2「判斷半邊留給操作者」。
- `fleet.identity=machine-slash-role`。
