# 管理者何時介入，以及一次介入如何結束？

> Status: `working draft`
>
> Purpose: 定義從狀態變化到決定、執行及驗收的單一管理循環。
>
> Shared types: [shared-types.md](./shared-types.md)

## 1. 一句話定義

一次管理循環處理一個有版本的問題，並追蹤決定的執行結果，而非發出一句話就結束。

## 2. 它不代表什麼／常見失敗

- `received` 不是 `action_started`，`action_started` 也不是 `accepted`。
- `runtime=idle` 只表示 runtime 已停止運作；不自動代表等授權、失敗或完成。
- 工具一直被呼叫不代表產物前進；重複讀取相同狀態不刷新「實質進展」。
- 回報遺失不能變成新的強制工作閘門；它影響管理可見度，不阻止正常 worker 做事。

## 3. 觸發與路由

Adapter 使用可觀測事件產生狀態；worker 在里程碑、需要決策、等待依賴、
準備交付或已知失敗時產生報告。高頻 token/工具片段合併成狀態更新，
不能每個片段都喚醒管理模型。

| 條件 | 確定性處理 | 何時需要管理者判斷 |
| --- | --- | --- |
| 健康且無新阻礙 | 更新索引／心跳 | 不喚醒 LLM |
| 新的決策請求 | 去重、載入引用的授權與任務卡 | 讀單一決策包 |
| runtime 結束，但沒有對應報告 | 短暫等報告到達；仍缺少便 `missing_report` | 先診斷，再決定是否可要求補報 |
| 已報等待依賴 | 訂閱被依賴任務的版本變化 | 依賴交付後重新核對，避免定時催問 |
| 沒有實質進展超過任務檢查時限 | `progress_overdue`，保留最後證據 | 查指定工具／產物；不直接判定 hang |
| 心跳逾期 | `offline` 觀測，停止把舊狀態當 live | 不自動重啟；先讀最後 checkpoint |
| worker 宣告完成 | `completion_pending`，讀驗收／審查證據 | 使用既有驗收規則接受或退回 |
| 來源、授權或產物版本不一致 | `conflict`，凍結該項決定 | 取得衝突相關片段，不展開全部上下文 |

時限採每任務 `watchPolicy`，長測試與等待人類不能使用同一 hang 門檻。
v0 可測預設：報告等待 10 秒，心跳每 15 秒、失聯判定 60 秒；實質進展期限
由任務宣告，如長測試 20 分鐘。它們只是起始配置。
若 host 只支援 5 分鐘輪詢，須展示觀測延遲；不能宣稱能在 60 秒內通知。

## 4. Canonical flow

```text
 observe event / receive worker report
                  |
        bind task + attempt + instance
                  |
      deduplicate / detect gap / project card
                  |
      no attention? ----yes----> update index only
                  |
                  no
                  v
       select card + one decision capsule
                  |
       sufficient evidence + authority?
            /                     \
          no                       yes
          |                         |
 fetch targeted span          manager decides
 or escalate once                   |
          |                 recheck task/grant versions
          |                         |
          +<---- stale/conflict -----+
                                    |
                      persist command intent + commandId
                                    |
                      target receives / rejects / unknown
                                    |
                     worker reports action_started + outcome
                                    |
                     verify artifact/acceptance evidence
                                    |
                      update card + read cursor + next step
```

管理者中斷後，從任務卡、未處理請求、有效授權、未結算 command 與 read cursor
恢复。重讀相同事件不重送；沒有證據證明前一次未執行時，未知結果保持未知。

## 5. 狀態與決定規則

```typescript
// Canonical definitions: ./shared-types.md sections 2–6.
// RuntimeObservation + WorkerReport => TaskCard
// TaskCard + DecisionRequest + effective Delegation => ManagerDecision
// ManagerDecision => CommandReceipt => OutcomeReport => acceptance evidence
```

三個獨立狀態軸：

```text
 runtime:      running / executing_tool / idle / offline
 worker claim: working / waiting_decision / waiting_dependency /
               completed / failed / paused / unknown
 acceptance:   not_submitted / pending / accepted / rejected
```

「worker completed，但 acceptance rejected」是有效的可見狀態；不得折疊成綠燈。
任務卡是投影。是否寫入既有 `task.done` 由原本 task/review 契約決定，
新報告不能繞過它，也不能憑文字讓 successor 解鎖。

授權判斷順序：

1. 讀引用的操作者來源與現行委派版本；worker 提供的引用不是自證授權。
2. 動作、資源、環境、預算與排除事項全部符合，且無明確保留，管理者可直接決定。
3. 沒有新的 scope/grant 變化時，同一事項不重問批准。
4. 遇到明確保留、互相衝突或真正越界，才送給操作者；把具體改動一起送出。
5. 相容的新授權可解除先前阻礙；不能把較晚的模糊措辭當成撤销明確排除事項。

缺少停止報告時，先做不花模型費的 adapter／ledger 讀取。仍不足且允許額外
對話時，最多針對同一 stop episode 發一次 `request_report`；若一回合試跑等
限制禁止新呼叫，直接顯示需要介入。這個 probe 本身不能暗中啟動下一份工作。

## 6. 通訊與上下文交接

下列為規劃的語義操作，不是宣稱已存在的 CLI／HTTP endpoint：

| 操作 | 發起者 → 接收者 | 回傳 |
| --- | --- | --- |
| `publish_report` | worker/adapter → Edda | event cursor；版本不符則拒收為當前狀態 |
| `list_attention` | manager → Edda | 精簡索引與增量游標 |
| `get_task_card` | manager → Edda | 有版本的 TaskCard |
| `get_decision_context` | manager → context selector | 單一決策包、引用、缺少項目、預算資訊 |
| `resolve_decision` | authorized manager → existing service/adapter | CommandReceipt，與 requestId/version 綁定 |
| `read_outcome` | manager → Edda | 具體動作結果及驗收引用 |

資料通訊可以重試同一 id；**執行效果**不能因此重播。重啟 generation 改變、
grant 撤销或問題已過期時，舊指令不得進入新 session。需要同任務續跑時，
重新建立 binding，但保留同一 task identity 與歷史。

收件匣按問題嚴重性與等待時間排序，保留公平配額，避免高頻代理擠掉安靜的阻礙。
依賴指向 task，不指向可能消失的 PID。多個管理者並存時，只有持有該任務管理
epoch 的管理者可寫決定；只讀者可同時存在。所有權與 CAS 必須進共用服務層。

## 7. 具體閉環與驗收

### A. 已在委派範圍內的確認

```typescript
// Pick<DecisionRequest, "requestId" | "question" | "requestedAction" | "recommendation">
const ask = {
  requestId: "request-46-1",
  question: "可否執行已核准設計的測試資料庫 migration？",
  requestedAction: { kind: "test_db_migration", resource: "character_runtime_test" },
  recommendation: "approve"
};
```

Edda 解析 `grant-test-db-3` 的有效來源：明列測試 migration 可委派，dev/prod
排除。管理者讀 task card + 該問題的變更摘要，回覆具體限定批准；worker 再次
檢查目標資料庫，報 `action_started`，產出測試證據與結果。管理者核對結果，
該 request 才完成。整條路徑不再詢問操作者同一授權。

### B. 停下來但沒有交代原因

```typescript
// Pick<TaskCard, "runtimeState" | "reportedState" | "attention" | "nextStep">
const unknownStop = {
  runtimeState: "idle",
  reportedState: "working",
  attention: "missing_report",
  nextStep: "查 stop-200-4 的 adapter 與最近 checkpoint；尚不能發 continue"
};
```

Adapter 回報 turn settled；報告等待期後仍無 StopReport。確定性層將問題放入
收件匣。若可用額度允許，管理者只問停止原因。worker 回傳缺少 fixture，管理者
從任務卡取得允許建立的測試資料範圍，回覆具體處置；之後以新增 fixture 及測試
結果確認進展。若無額度，保留 `missing_report`，不花費一次隱藏的補問。

紙上測試清單：

| 情境 | 必須看見的結果 |
| --- | --- |
| 50 個健康代理只有心跳 | 0 次管理模型呼叫；不載入 50 份全文 |
| 其中 1 個有新問題 | 僅取該任務卡與決策包；其他全文讀取為 0 |
| 同一停止事件重送／巡查重跑 | 至多一個補報 intent，不重複催問 |
| 更新版本的操作者指令先到 | 舊 request/decision 顯示 superseded；不按舊授權執行 |
| command 送出後管理者崩潰 | 以相同 commandId 查收件／結果，不產生第二次效果 |
| worker 宣告完成，審查未通過 | 顯示完成主張與未驗收，不寫產品成功 |
| 新對話在完成檢查前到達 | 陳舊 completion checkpoint 被拒，繼續管理 |
| manager 恢復到另一模型／session | 從 durable card/request/grant/cursor 接續，不要求全文回放 |

## 8. 待確認的落地點

- 報告入口選 Pi tool、extension command 或 MCP：需保證任務綁定並避免雙寫。
- 現有 control/authority 分支尚未接受的部分，須先確認基線再定映射；不得假定可用。
- host 的 event wake 能力與離線通知語義需實測；v0 可以輪詢，但公開延遲。
- 回報過於冗長、引用失效或三者矛盾時，保留「缺上下文」而非強行產出決定。

這些是下一輪設計／原型驗證點，不構成本輪啟動其他代理或部署的授權。
