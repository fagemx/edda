# edda node：代理自己的通訊層（每台一個常駐節點，走 Tailscale）

- 日期：2026-09-02
- 狀態：設計稿。操作者裁示「我們應該自己開發通訊，代理用的 Tailscale」，並核准本稿與第一片的單
- 基準：`origin/main` `a1dd3d8aff369f7511360109f9f70104d9457be3`
- 第一片的單：#685
- 承接：`2026-09-02-fleet-manager-agent-design.md`（管理者迴圈；本稿給它一條自己的通訊路）
- **取代**：#446 關單結論「edda 不做傳輸，借 host 的推播當門鈴」；今日決策 `coord.cross-machine=…-no-cross-platform-doorbell` 中「不做常駐程序、不新增傳輸」那一句；以及 `orchestration.cross-platform` 中「控制面是每平台一個薄 adapter 當門鈴」的門鈴部分。資料面等於帳本物件這一點**不變**

## 0. 一句話

每台機器一個 edda 節點，代理只跟本機節點講話，節點之間走 Tailscale。每一封訊息在兩端的帳本都有紀錄和回執。名字固定，不隨 session 換。

## 1. 為什麼要翻掉「不做傳輸」

舊原則的理由是少一個會死的程序。2026-09-02 一天把帳算清楚了：

| 舊原則省下的 | 舊原則付出的 |
|---|---|
| 一個常駐程序 | 四支輪詢腳本（PR 審查、lane 狀態、認領、心跳）各自判成敗，全部被 #669 的假 exit 0 騙過 |
| | 每家 host 的訊息語意都不同：Claude 的送出成功只代表收下、顯示名續接就變、跨機單向、有 permission class 限制；pi 與 codex 根本沒有訊息機制 |
| | 於是拿 GitHub 留言當替代品，人變成路由器；操作者已明說撐不住 |
| | 同一台機器上兩個控制者對「誰活著」看法不同（120 秒視窗，#646），沒有任何一方能問對方 |

host 訊息的實測結論（同日三輪）：只保證收下、只送得到活著的 session、跨機單向。它可以繼續當加速器，但不能再是唯一的路。

## 2. 形狀

### 2.1 節點

`edda node`：常駐程序，每台一個，用排程任務或服務啟動（沿 `fleet.lane-launch`）。它就是現有的 `edda serve` 加三件事：

1. **peer 表**：`~/.edda/node.toml` 列出其他機器的別名、Tailscale 主機名、port、共享 token。本機別名例如 `4090` 對應 `desktop-bhdbi37`，`docs` 對應 `desktop-4t5s030`。
2. **複製器**：追蹤本機 `coordination.jsonl` 與帳本決策的新事件，批次 POST 到每個 peer 的 `/api/sync`；接收 peer 送來的事件寫進本機 store，標記來源機器；衝突照既有 `sync_from_sources` 規則（#394 一 key 一活、#395 保留治理欄位）。
3. **投遞**：收到寄給本機 actor 的 request，寫進本機 coordination 事件（既有形狀），並寫一筆 `request_delivered`；有 `edda inbox wait` 在等的就喚醒它。

只綁 Tailscale 的 IPv4（`100.x`），不綁 `0.0.0.0`。沒有 token 的 POST 一律 401。Tailscale 已經做了裝置認證與加密，v1 不自己做金鑰；#609 的簽章落地後接上。

### 2.2 名字

`<machine>/<role>`，沿 `fleet.identity=machine-slash-role`。`machine` 是別名，節點負責把別名對到 Tailscale 主機名。`role` 是 label（`manager`、`lane-gh617`、`verifier`）。寄信時 `machine` 等於本機就本地投遞，否則轉給那台的節點。session id、分支名、host 顯示名都不是地址。

### 2.3 訊息的一生

```text
寄件端                          收件端
request（既有事件）  ──POST──▶  request（同一筆，標來源機器）
                                request_delivered（節點寫）
                                request_ack（收件代理寫，既有）
```

寄件端隨時可查：`pending`（尚未送達）、`delivered`（對方節點已收）、`acked`（對方代理已讀）、`dead`（超過 TTL 未 ack）。「成功但沒人讀」這種狀態不再存在。

### 2.4 投遞到代理

| 代理種類 | 收到的時機 | 需要改什麼 |
|---|---|---|
| 靠 hook 的互動 session（Claude Code、codex、cursor、hermes、openclaw 五個 bridge） | 下一個 hook 邊界，經既有 `render_coord` | 不改。訊息只是變成先在本機 store 裡 |
| 無頭 lane（`edda dispatch` 起的 pi、codex） | 立刻：brief 的迴圈在回合之間呼叫 `edda inbox wait --actor <name> --timeout N` | 新增 `edda inbox wait`（#520 的形狀） |
| 管理者（#674） | 每次醒來先讀信箱，再讀 GitHub | #674 的迴圈第一步改成讀信箱 |

**它做不到的**：叫醒一個根本沒在跑的 session。訊息會等著，對方一醒就收到並回執，寄件端查得到它在等。Claude 不允許外部在回合中插話，這是 host 的邊界，不是本稿的缺口。

### 2.5 決策也走這條路

決策事件用同一個複製器同步，兩台的 `edda ask` 一致。今日 `fleet.merge-authority` 一台有一台沒有，就是驗收案例。

與 `ledger.cross-machine-projection=committed-mirror…`（操作者已記）的關係：**兩條路，兩個用途**。節點是活的協調路，秒級；git 裡的 mirror 是冷備份與新機器的起點，wave 結束時落一次。#671 的範圍縮成後者。

### 2.6 GitHub 的角色

只留 PR 與 issue：成果、審查、人看的板。代理之間的協調訊息不再寫成 issue 留言，除非那則留言本身就是給人看的裁定紀錄。

## 3. 失敗模式與訊號

| 情況 | 訊號 | 行為 |
|---|---|---|
| 節點死了 | 節點自己是一個 actor（`<machine>/node`），有心跳；管理者狀態行顯示 `node: down since <time>` | 排程任務重啟；外送佇列在磁碟，不丟 |
| peer 連不到 | `edda node status` 列出佇列長度與最後成功時間 | 指數退避重試；訊息狀態停在 `pending` |
| 寄給不存在的 actor | 送達端節點回 `unknown actor` | 寄件端狀態 `dead`，寫 dead-letter 事件 |
| token 錯 | 401 | 不寫任何事件，記 log |
| 兩台同時改同一 key 的決策 | `sync_from_sources` 的 conflict | 後到的以 inactive 匯入，既有規則 |

## 4. 安全邊界

- 只在 tailnet 內；綁定非 `100.x` 位址要顯式 `--insecure-bind` 且印警告。
- 共享 token 放 `node.toml`，不進 git。
- 節點不執行任何指令，只搬事件；派工仍是本機管理者用 `edda dispatch` 做。
- 不保護的：操作者機器被攻陷、tailnet 帳號被盜。與 #609 的威脅模型一致。

## 5. 分步

1. **v0（第一片，#685）**：`/api/sync` 解除 `cfg(test)` 並加 token；`edda node` 動詞（start、status、peers）；peer 表；複製器；request 轉送與 `request_delivered`；`edda inbox wait`。驗收是兩台真機的往返。
2. **v1**：#674 管理者改讀信箱；lane brief 迴圈加 `edda inbox wait`；節點心跳進狀態行。
3. **v2**：#609 簽章；host 門鈴當可選加速器；第三台機器。

## 6. 非目標

自己寫網路層（Tailscale 已做）、取代 GitHub 的 PR 與 issue、在回合中打斷 Claude、跨 tailnet 的公網存取。

## 7. 決策

- `coord.transport=edda-node-per-machine-over-tailscale`：取代「不做傳輸、不做常駐」；理由見 §1。
- `coord.message-receipt=delivered-and-acked-both-ledgers`：每封訊息兩端都有事件，寄件端可查狀態。
- `fleet.identity=machine-slash-role`（已記）：節點負責別名對應。
