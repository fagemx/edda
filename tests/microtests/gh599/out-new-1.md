Warning: No project session found with id 'microtest-gh599-new-20260902-160921'; creating a new session with that id.
Recon 與 dedupe 完成。以下為 fleet-epic-split 七步流程的 dry-run 產出。

---

## Provenance（固定）

- **輸入**：epic issue **#560**（fagemx/edda，`fleet:goal`，operator Tim 2026-09-01 簽核「可以 開epic」）之 **Stage 2 — event-driven delivery** 節，逐字引述：
  > "phase terminal-state notifications through edda-notify channels (filed), with #545 (gate-entry routing) as sibling. Acceptance: a controller reacts to phase/gate events without polling stdout."
- 開工前檢查：repo 根無 `FLEET_PAUSE`；簽核存在（#560 `fleet:goal`）。

## ① recon 現況報告（file:line 證據）

**輸入要求的東西 vs 程式碼現況（workspace SHA 11c1ec2）：**

| Stage 2 要求 | 現況 | 證據 |
|---|---|---|
| 透過 edda-notify channels 發 phase terminal-state 通知 | **已 filed，未實作**。#564 open（`fleet:ready, lane:4090, lane:feature`）；輸入裡的 "(filed)" 標記與實際 issue 相符，且碼不在 | `crates/edda-conductor/Cargo.toml` 無 `edda-notify` 依賴；`crates/edda-cli/src/cmd_conduct.rs:244` 硬編碼 `StdoutNotifier` |
| #545 gate-entry routing（sibling） | **已 filed，未實作**。#545 open（`fleet:pending`） | `cmd_conduct.rs:244` 仍為 `StdoutNotifier`；gate-entry 通知在 `crates/edda-conductor/src/runner/sequential.rs:916-925` 只走 `Notifier` prose → stdout |
| Acceptance：controller 不 polling stdout 即可反應 | 依賴上述兩張 + 既有 channel 层；channel 層本身已在 | `crates/edda-notify/src/lib.rs:11`（`Channel`：ntfy/webhook/telegram + per-channel events 過濾）、`lib.rs:214`（`dispatch`，5s timeout）——**但 conductor 不可及** |

**其他已完成、不生單的部分：**
- edda-notify channel 層完整存在：config load（`lib.rs:53`）、event matching（`lib.rs:44`）、三種 transport（`lib.rs:311/366/392`）、20 個測試。
- conductor 側 `Notifier` trait 已是接縫：`crates/edda-conductor/src/runner/notify.rs:3`（`StdoutNotifier`、`CollectNotifier`），`sequential.rs` 共 8 處 `notifier.notify` 呼叫點（152/163/563/918/1159/1221/1230/1267）。
- 結構化機器可讀訊號已在：`runner/event_log.rs:23-90`（`PhasePassed/PhaseFailed/PhaseSkipped/GateEntered/VerdictReceived/PlanCompleted/PlanAborted` → `events.jsonl`）。
- 注意：`NotifyEvent::PhaseChange`（`edda-notify/src/lib.rs:63`）是 agent session 的 phase 轉換，**不是** conductor plan phase 的 terminal state——兩者不同域，#564 需新增事件，#564 body 已正確敘明。

## ② decompose（候選 + 獨立性判定）

「輸入 − 已完成」的差距只有兩片，且**兩片都已經是 open issue**，本輪候選實質為零。為完整性，記錄被檢驗的兩個候選及其獨立性三問：

| 候選 | 單獨 merge 壞 main？ | 同批檔案？ | 能直接開工？ |
|---|---|---|---|
| A. phase terminal-state → edda-notify（= #564 內容） | 否（additive，stdout 不變） | `runner/sequential.rs` + `runner/notify.rs` + `edda-notify/src/lib.rs` | 是（但與 #563 同檔，需 serialize/coordinate——#564 body 已載明） |
| B. gate-entry → edda-notify（= #545 內容） | 否 | `cmd_conduct.rs` + `runner/notify.rs` | 是，但 channel 形狀應與 #564 一致（兩張 body 互相約束） |

無超過 30 張、無巨單、無做不完的單——因為**沒有新單**。

## ③ dedupe 紀錄（四道程序全跑）

```
候選A phase terminal-state notify | queries: 文件内引用→epic body "(filed)"+#560 comment 明指 #564; edda ask "notify"→命中 epic560.stage1-slice（裁決明列 "#564 terminal-state notify (sibling of 545)"）; edda search "terminal-state notification"→evt_01m1ee21…（"terminal state = edda-notify events (GH-564)"）; gh issue list 模糊比對→#564 標題逐字相符 | verdict: duplicate of #564
候選B gate-entry routing        | queries: 文件内引用→輸入節明指 #545; edda ask "notify"→同一裁決; edda search "notify channel"→既有軌跡; gh issue list 模糊比對→#545 標題相符 | verdict: duplicate of #545
```

## ④ 確認表（dry run — 停在此步）

| # | 標題 | Predicted surface（paths） | 獨立性 | dedupe verdict | 來源 |
|---|---|---|---|---|---|
| — | **（無擬建單）** | — | — | — | — |

**擬建 issue body：0 份**——沒有任何候選存活 dedupe，故無 body 可印。

**Skip 清單（重複項 → 既有 issue）：**
- Stage 2 phase terminal-state 通知 → **#564**（open，`fleet:ready, lane:4090`，M，body 已符合 ready-bar 契約）
- Stage 2 gate-entry routing（sibling）→ **#545**（open，`fleet:pending`）
- 兩張合計已完整覆蓋 Stage 2 acceptance：「phase 事件」（#564）＋「gate 事件」（#545），controller 端消費走既有 channel 層（webhook/ntfy/telegram）。

## ⑦ report（provenance 回連）

**本輪建單數：0。** Stage 2 的差距在 2026-09-01 的第一輪分解已全部 filed（#564 ＋ #545），並有帳本裁決背書（決策 key `epic560.stage1-slice`）。#560 body 的 "(filed)" 標記經碼檢屬實：兩張 issue open、對應程式碼尚未存在，無「標了完成但碼不在」的假完成。

**dry-run 下未執行、正式輪該做的動作：**
- ⑥ 交叉引用：到 #564 與 #545 各留一則 comment，回連 provenance（#560 Stage 2 節 ＋ 決策 key `epic560.stage1-slice`），並在 #564 補註「與 #563 同檔 coordination」已在其 body、無需重複。
- 撕 ready：#564 已是 `fleet:ready`，#545 仍 `fleet:pending`——是否提升由操作者決定。

provenance：epic issue #560「Stage 2 — event-driven delivery」節；決策 key `epic560.stage1-slice`（`edda ask "notify"` 可查）。
