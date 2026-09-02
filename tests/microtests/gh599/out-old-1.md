Warning: No project session found with id 'microtest-gh599-old-20260902-160921'; creating a new session with that id.
Recon 完成。以下是本輪 fleet-epic-split（DRY RUN）的產出。

---

# ① Recon 現況報告

**目標**（epic #560 Stage 2 — event-driven delivery）：把 phase terminal-state 事件經由 edda-notify channels 送出（#545 gate-entry routing 是 sibling）。驗收：controller 能對 phase/gate 事件做出反應，**不必輪詢 stdout**。

## 已經做完（碼在，有 file:line）

| 已完成 | 證據 |
|---|---|
| conductor 的 terminal-state **事件型別**齊全：`PhasePassed`/`PhaseFailed`/`PhaseSkipped`/`GateEntered`/`VerdictReceived`/`PlanCompleted`/`PlanAborted` | `crates/edda-conductor/src/runner/event_log.rs:21-90` |
| 事件已寫入 `.edda/conductor/{plan}/events.jsonl`（append-only、seq+ts），可選 tee 到 stdout JSON | `event_log.rs:141-171`（`EventLogger::record`）、`sequential.rs:59` |
| 每個 terminal-state 在 runner 裡都有 record 站點 | `sequential.rs:319,417,557,802,971,1021,1057,1106,130,1251`；gate 站點 `sequential.rs:912,237,266` |
| edda-notify 的 **channel 基礎建設**（ntfy/webhook/telegram、per-channel 事件過濾、萬用字元、5 秒 timeout、best-effort dispatch）已存在且被 6 個非 conductor 端點使用 | `crates/edda-notify/src/lib.rs:13-29,77-124,155-167`；呼叫端：`bridge-claude/src/dispatch/session.rs:434-453`、`tools.rs:478-482`、`cmd_bridge.rs:1180-1184`、`cmd_draft.rs:710-715`、`cmd_reconcile.rs:2269-2299`、`cmd_task.rs:186` |
| 設定載入：`.edda/config.json` 的 `notify_channels`，缺檔/壞檔安全降級為空 | `edda-notify/src/lib.rs:45-74` |
| 供人看的 stdout 通知（conductor 內部 trait）與測試用 collector | `crates/edda-conductor/src/runner/notify.rs:1-47`、`cmd_conduct.rs:9` |
| 供輪詢用的 `runner-status.json`（含 `awaiting_verdict` 欄位） | `event_log.rs:186-241` |

## 還沒做（差距）

1. **edda-notify 沒有任何 conductor 事件變體**。`NotifyEvent` 只有 `approval_pending`/`phase_change`/`session_end`/`anomaly`/`request_pending`/`task_assigned`（`edda-notify/src/lib.rs:80-124`）；`phase_passed`/`phase_failed`/`plan_completed` 等名稱不存在，channels 無從訂閱。
2. **conductor 不依賴 edda-notify**。`crates/edda-conductor/Cargo.toml:12-24` 無此依賴；terminal-state 事件只落到 JSONL 檔與 stdout，沒有任何 dispatch。測試自己是靠「輪詢 events.jsonl」驗 gate 事件（`sequential.rs:2490-2497`）——這正是驗收要消滅的輪詢姿勢。
3. **gate 事件（gate_entered / verdict_received）同樣沒有 notification 路徑**，只有 #545 要做的 routing（`docs/guides/operator-runbook.md:96,99,139`）。
4. **事件名稱零文件**。`notify_channels`、事件名清單在 `docs/`、`README.md`、`CHANGELOG.md` 全數查無（grep 0 筆）——操作者無從設定 filter。
5. 失敗可見性：`dispatch` 吞錯只留 `tracing::warn`（`edda-notify/src/lib.rs:160-166`）；對「通知送不出去」目前無任何 surface 給 controller。

## 邊界（與既有單的分工）

- **#545（gate-entry routing）**：routing＝把 controller 引導到 gate 去下 verdict（`sequential.rs:902-908` 的 approve/reject 指引屬它）。Stage 2 的 gate 事件**通知**（事件發生時推播）與 routing 不同，但同檔同區——單上明示分工。
- **#567（統一狀態面）、#569（lane 心跳）**：狀態輪詢與心跳，非事件推送；不相干。dedupe 來源：operator-runbook 缺口表全列（`operator-runbook.md:94-99`），無重複單。

---

# ② Decompose — 獨立性三問檢核

差距拆成 **4 張**（上限 30，遠低於）。三問逐張過：見確認表「獨立性」欄；#B/#C 同搶 `sequential.rs`，已明示依賴而非硬拆平行。

# ③ Propose — 確認表

| # | 標題（gh issue create 用） | 尺寸 | 獨立性 | 觸及檔案 |
|---|---|---|---|---|
| A | feat(edda-notify): 新增 conductor plan 事件變體與三格式渲染 | S | 無依賴，可獨立 merge（crate 內加法變更，不破 main） | `edda-notify/src/lib.rs` |
| B | feat(edda-conductor): phase terminal-state 經 edda-notify channels 推送 | M | `blocked by #A`（需新變體）；與 #C 同檔故排其前 | `conductor/Cargo.toml`、`runner/sequential.rs`、`runner/notify.rs` |
| C | feat(edda-conductor): gate 事件（gate_entered/verdict_received）經 edda-notify 推送 | S | `blocked by #A、#B`（同檔同模式）；與 #545 分工：只通知、不 routing | `runner/sequential.rs`、`runner/notify.rs` |
| D | docs(edda-notify): 記錄 channel 事件名稱清單與 notify_channels 設定範例 | XS | `blocked by #A`（名稱定案才寫）；docs-only | `docs/guides/` |

四問 Wiring audit 在各單內。以下為完整擬發 issue body。

---

## Would-be issue A

**Title**: `feat(edda-notify): 新增 conductor plan 事件變體與三格式渲染`
**Labels**: `fleet:pending`, `lane:feature`

```markdown
## What happened
edda-notify 無法承載 conductor 的 phase terminal-state 事件：`NotifyEvent`
只有 approval_pending / phase_change / session_end / anomaly / request_pending /
task_assigned 六種（crates/edda-notify/src/lib.rs:80-124），而 conductor 已把
phase_passed / phase_failed / phase_skipped / plan_completed / plan_aborted
寫進 events.jsonl（crates/edda-conductor/src/runner/event_log.rs:30,36,59,79,86），
兩邊沒有橋——channels 的 events 過濾器（lib.rs:33-37）對這些名稱無從比對。
現況觀察於 worktree edda-wt-gh599 @ 11c1ec2。

## Why it matters
epic #560 Stage 2 的驗收是「controller 對 phase/gate 事件反應、不輪詢 stdout」，
但事件名稱不在 edda-notify 裡，channel 訂閱根本無法定義；本單是整條
通知路徑的地基，沒有它後續接線單無處可接。

## Suspected surface
crates/edda-notify/src/lib.rs（`NotifyEvent`、`event_name`、`to_json`、
`format_ntfy`、`format_telegram`；webhook 走 event_name+to_json 通用路徑）。

## Predicted surface
- crates/edda-notify/src/lib.rs：`NotifyEvent` 新增 5 個變體
  （PhasePassed / PhaseFailed / PhaseSkipped / PlanCompleted / PlanAborted，
  欄位對齊 event_log.rs 對應 Event 變體的已序列化欄位）、
  `event_name()` 回 "phase_passed" 等 snake_case 名、
  `to_json`、`format_ntfy`、`format_telegram` 各加一臂、
  `format_webhook` 驗證 payload 形狀。
- 不動 `Channel`、`dispatch`、`NotifyConfig`。
- 不動 conductor（接線是後續單）。

## doneWhen
- 5 個新變體有名稱、JSON payload、ntfy/telegram/webhook 三格式渲染，
  各有單元測試（比照現有 format_ntfy_approval_pending 等，lib.rs:494 起）。
- 事件名稱與 events.jsonl 的 serde tag 一致
  （event_log.rs:21-90 的 rename_all = "snake_case"）。
- 消費證明（write→read）：一個測試從建構事件走到 `dispatch` 的
  channel 比對（`Channel::matches`，lib.rs:37-41），證明用新名稱設定的
  channel 會被選中——用 CollectNotifier 式斷言或比對 matches()，不發真 HTTP。
- death-visibility line：dispatch 失敗維持 best-effort `tracing::warn`
  （lib.rs:160-166），本單不改變此語義，於單上明記「通知送不到＝靜默」
  的現狀，改進歸後續接線單。

## Wiring audit — REQUIRED whenever the issue cites or adds code
| Component | 1. Writer & shape | 2. Reader | 3. Failure signal | 4. Layer reach |
|---|---|---|---|---|
| 既有 conductor `Event`（本單引用） | event_log.rs:141 `EventLogger::record` 寫 JSONL，結構化欄位 | 唯一讀者是測試輪詢（sequential.rs:2490-2497）；production 無 reader | 寫檔失敗被吞（`let _ = append_line`，event_log.rs:168） | 只到檔案層；未達 notify 層 |
| 新增 `NotifyEvent` 變體（本單寫端） | crate 內建構＋dispatch 比對，結構化 JSON | 本單只接 `Channel::matches`（lib.rs:37-41）；下游 reader 由接線單供給 | matches 不中＝靜默跳過（現行語義） | 到 notify 層的比對；不到傳輸（dispatch 呼叫端屬接線單） |

## Relation to existing issues
- epic #560 Stage 2：本單是其第一步（型別層）。
- #545（gate-entry routing）：不涉 gate 事件，無交集。
- dedupe：operator-runbook.md:94-99 缺口表無本項；`grep -rn notify_channels docs/ README.md CHANGELOG.md` 0 筆，無既有文件單。

## 獨立性
無依賴，可單獨 merge。只加 enum 變體與渲染，現有六事件行為不變，
merge 後 main 不會壞。

## 尺寸
S——單一 crate 單一檔案，估 <300 行含測試。
```

---

## Would-be issue B

**Title**: `feat(edda-conductor): phase terminal-state 經 edda-notify channels 推送`
**Labels**: `fleet:pending`, `lane:feature`

```markdown
## What happened
conductor 的 phase terminal-state（Passed/Failed/Skipped 與 plan 級
Completed/Aborted）只寫 events.jsonl 與 stdout：record 站點在
crates/edda-conductor/src/runner/sequential.rs:971,802,1021,1057,1106,319,417,
1242,557,1251,130；crates/edda-conductor/Cargo.toml:12-24 沒有 edda-notify
依賴。controller 想知道 phase 結束，只能輪詢 stdout 或檔案——測試自己就是
這樣驗的（sequential.rs:2490-2497 輪詢 events.jsonl）。現況觀察於
worktree edda-wt-gh599 @ 11c1ec2。

## Why it matters
epic #560 Stage 2 驗收：controller 對 phase 事件反應、不輪詢 stdout。
沒有本單，通知鏈有型別（前一單）卻沒有觸發點，驗收不成立。

## Suspected surface
crates/edda-conductor/src/runner/sequential.rs（terminal-state record 站點）、
crates/edda-conductor/src/runner/notify.rs、crates/edda-conductor/Cargo.toml。

## Predicted surface
- crates/edda-conductor/Cargo.toml：加 edda-notify 依賴。
- crates/edda-conductor/src/runner/notify.rs：新增 channel 型 notifier
  （load `NotifyConfig`；空設定＝no-op，現行行為不變；保留 StdoutNotifier）。
- crates/edda-conductor/src/runner/sequential.rs：在 phase/plan terminal-state
  record 站點旁呼叫 edda_notify::dispatch，欄位取自當場既有的 Event 參數
  （phase_id、attempt、duration_ms、error、error_type、phases_passed 等）。
- 不動 GateEntered / VerdictReceived 站點（sequential.rs:912,237,266）——那是 #C。
- dispatch 是同步 ureq（5s timeout，edda-notify/src/lib.rs:154）而 runner 是
  tokio async：不得阻塞 run loop，包裹處理（spawn_blocking 或等價）由實作者
  擇一，單上不釘死實作。

## doneWhen
- phase_passed / phase_failed / phase_skipped / plan_completed / plan_aborted
  五種事件在設定有 channel 時經 edda_notify::dispatch 送出，payload 含
  plan 名與 phase_id。
- `.edda/config.json` 無 notify_channels（或空）時，行為與今天完全一致
  （回歸測試守住）。
- 通知失敗不影響 plan 執行：dispatch 錯誤維持 best-effort，plan 續跑。
- 消費證明（write→read）：一個 end-to-end 測試跑一個兩-phase plan，
  攔截 dispatch（test seam），斷言收到 phase_passed 與 plan_completed
  且 payload 欄位正確。
- death-visibility line：通知送不出時至少有一條可見訊號——`--json` 模式下
  dispatch 失敗要出現在執行輸出（沿用或補 tracing→輸出的既有路徑），
  並在單上記錄覆蓋面（哪些失敗模式可見、哪些仍靜默）。

## Wiring audit — REQUIRED whenever the issue cites or adds code
| Component | 1. Writer & shape | 2. Reader | 3. Failure signal | 4. Layer reach |
|---|---|---|---|---|
| 既有 record 站點（本單引用） | sequential.rs 十處 `event_log.record(...)`，結構化 JSONL | production 無 reader；測試輪詢（sequential.rs:2490） | 寫檔失敗被吞（event_log.rs:168） | 檔案層，未達 notify 層 |
| 新增 dispatch 呼叫（本單寫端） | runner 在 terminal-state 呼叫 `edda_notify::dispatch(config, &NotifyEvent::…)`，結構化 | ntfy/webhook/telegram 端點＝controller 的實際讀端（lib.rs:170-192） | dispatch 內部吞錯只 warn（lib.rs:160-166）；本單補 `--json` 可見性 | config 層（NotifyConfig::load，lib.rs:45-74）↔ runner 站點 ↔ 傳輸層全通 |

## Relation to existing issues
- `blocked by` edda-notify 變體單（#A）：需要新 NotifyEvent 變體才能建構事件。
- 與 gate 事件單（#C）改同一檔 sequential.rs，故本單先行、#C 排後。
- #545（gate-entry routing）：本單不含 gate 事件，無交集。
- dedupe：operator-runbook.md:94-99 缺口表「事件驅動（#545）」僅涵蓋門鈴，
  無 phase 通知單。

## 獨立性
blocked by #A。單獨 merge 不會壞 main（空設定 no-op）；不看 #C 可直接開工
（不碰 gate 站點）。

## 尺寸
M——一個依賴、一個 adapter、約十個站點接線、end-to-end 測試。
```

---

## Would-be issue C

**Title**: `feat(edda-conductor): gate 事件（gate_entered/verdict_received）經 edda-notify 推送`
**Labels**: `fleet:pending`, `lane:feature`

```markdown
## What happened
gate 事件是 conductor 唯二「需要外部行動」的狀態：phase 進 AWAITING_VERDICT
（sequential.rs:912-930 記 GateEntered，stdout 印 approve/reject 指引）與
verdict 從帳本被觀察到（sequential.rs:237,266 記 VerdictReceived）。兩者
目前只有 stdout 與 events.jsonl，無任何 channel 推送；controller 要嘛輪詢
runner-status.json 的 awaiting_verdict 欄位（event_log.rs:186-241）、要嘛
盯 stdout。現況觀察於 worktree edda-wt-gh599 @ 11c1ec2。

## Why it matters
epic #560 Stage 2 驗收明文含「gate 事件不輪詢 stdout」。gate 是整條
流程的等待點——通知不到，controller 就不知道該去下 verdict。

## Suspected surface
crates/edda-conductor/src/runner/sequential.rs（gate 站點 912,237,266）、
crates/edda-conductor/src/runner/notify.rs。

## Predicted surface
- crates/edda-conductor/src/runner/sequential.rs：GateEntered / VerdictReceived
  record 站點旁加 dispatch（沿用 #B 立的 adapter 模式）。
- 若 edda-notify 變體單（#A）未含 gate 事件變體，需在 #A 的基礎上補
  GateEntered / VerdictReceived 變體（edda-notify/src/lib.rs）。
- 不動 routing：不生 verdict 指引以外的行為改變，approve/reject 指引
  維持 sequential.rs:902-908 現狀。

## doneWhen
- 設定有 channel 時：phase 進 AWAITING_VERDICT 推出 gate_entered 通知
  （含 subject 與 gate_sha）；verdict 被觀察到推出 verdict_received 通知
  （含 decision）。
- 空設定 no-op，回歸測試守住。
- 通知失敗不影響 gate 等待邏輯。
- 消費證明（write→read）：end-to-end 測試跑一個 gated plan，攔截 dispatch，
  斷言 gate_entered 與 verdict_received 各一筆且欄位正確。
- death-visibility line：同 #B 的 `--json` 可見性要求。

## Wiring audit — REQUIRED whenever the issue cites or adds code
| Component | 1. Writer & shape | 2. Reader | 3. Failure signal | 4. Layer reach |
|---|---|---|---|---|
| 既有 GateEntered/VerdictReceived 記錄（本單引用） | sequential.rs:912,237,266 寫 JSONL，結構化 | 無 production reader；測試輪詢 events.jsonl（sequential.rs:2490） | 寫檔失敗被吞 | 檔案層；controller 端今天靠 runner-status.json 輪詢（event_log.rs:201-205） |
| 新增 dispatch 呼叫（本單寫端） | runner 在 gate 站點呼叫 dispatch，結構化 | controller 的 channel 端點 | 同 #B（吞錯＋warn；`--json` 可見性補上） | config ↔ runner 站點 ↔ 傳輸層全通 |

## Relation to existing issues
- `blocked by` #A（變體）與 #B（adapter 模式、同檔排程）。
- **與 #545 的分工邊界**：#545 做 gate-entry **routing**（把 controller 引到
  gate 去下 verdict，見 operator-runbook.md:96「事件驅動門鈴（#545）」）；
  本單只做**事件通知**。若 #545 落地時已涵蓋 gate 事件推播，本單由操作者裁撤
  ——這是本單獨立成張的唯一理由，寧可撤單不可重複蓋。
- dedupe：operator-runbook.md:94-99 缺口表無 gate 通知單。

## 獨立性
blocked by #A、#B；與 #545 有裁撤風險（見上）。單獨 merge 不壞 main
（空設定 no-op）。

## 尺寸
S——兩個站點接線＋兩個變體＋end-to-end 測試。
```

---

## Would-be issue D

**Title**: `docs(edda-notify): 記錄 channel 事件名稱清單與 notify_channels 設定範例`
**Labels**: `fleet:pending`, `lane:feature`

```markdown
## What happened
channel 過濾靠 `.edda/config.json` 的 `notify_channels` 事件名稱精確比對
（crates/edda-notify/src/lib.rs:33-41），但整個 repo 沒有任何文件列出
合法事件名稱或設定範例：`grep -rn "notify_channels" docs/ README.md
CHANGELOG.md` 為 0 筆。操作者只能讀原始碼拼設定。現況觀察於
worktree edda-wt-gh599 @ 11c1ec2。

## Why it matters
epic #560 Stage 2 的讀者是 controller（另一個 agent session 或人），
它必須能用正確的事件名稱設定 channel 才收得到 phase/gate 通知；
名稱拼錯＝静默收不到（比對不中不報錯，lib.rs:37-41）。Stage 2 落地後
事件名清單變長，沒有清單等於沒有訂閱面。

## Suspected surface
docs/（通知相關指引；operator-runbook.md:99 通知列是現存唯一掛點）。

## Predicted surface
- docs/ 新增或擴充一節：全部 `NotifyEvent::event_name()` 名稱清單
  （既有六種＋新增五種＋gate 兩種，以合併時點為準）、
  `notify_channels` 三種 channel 的設定範例（含 `"*"` 萬用字元）。
- operator-runbook.md「通知」列（:99）補連結。
- 不動任何程式碼。

## doneWhen
- 文件含完整事件名稱清單，與合併時點的 `event_name()` 實作逐一對得上
  （文件側抽查比對 lib.rs 作為 verify）。
- 含至少一個可複製貼上的 `notify_channels` 範例（三種 channel 各一）。
- 消費證明（docs 版）：範例 JSON 以 parse 驗證可被
  `Channel` 的 serde 反序列化（可放一個測試或人工貼上 `edda notify test`
  前的 parse 檢查記錄）。

## Wiring audit — REQUIRED whenever the issue cites or adds code
| Component | 1. Writer & shape | 2. Reader | 3. Failure signal | 4. Layer reach |
|---|---|---|---|---|
| `event_name()`（本單引用） | lib.rs:119-124 硬編碼字串，結構化 | `Channel::matches`（lib.rs:37-41）；操作者（本單的讀者） | 名稱不匹配＝channel 靜默跳過，無警告 | 實作層直達比對層；缺的是文件面 |

## Relation to existing issues
- `blocked by` #A（新名稱定案才寫清單；若 #C 裁撤，清單不含 gate 事件名）。
- dedupe：全 repo 文件 grep 0 筆，無既有文件單可重複。

## 獨立性
blocked by #A。docs-only，單獨 merge 不壞 main，不碰程式。

## 尺寸
XS——一節文件＋範例。
```

---

## 回報（操作者裁決點）

- 擬發 **4 張**（上限 30），依賴鏈：**A** → (B → C, D)。
- 需要裁決的：**C 與 #545 的重疊風險**——若 #545 會涵蓋 gate 事件推播，C 該撤；我把兩者切在「通知 vs routing」，但方向對錯只有你能簽。
- 本輪為 DRY RUN，未建立、未修改任何 GitHub issue；以上為待簽提案。
