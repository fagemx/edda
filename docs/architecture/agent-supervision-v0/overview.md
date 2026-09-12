# 多代理管理：管理者需要知道什麼？

> Status: `working draft`
>
> Purpose: 定義雙向通訊、狀態追蹤與有限上下文管理的產品邊界。
>
> Shared types: [shared-types.md](./shared-types.md)
>
> 本輪只做規劃；不介入既有代理任務、不新增巡查、不擴充實作。

## 1. 一句話定義

讓管理者用少量、可查證的任務資料管理多個代理，在需要決策時才取得相關上下文。

核心問題是「誰需要我介入、為什麼、我有權決定什麼、決定後是否真的有進展」。
管理單位是任務；Pi、Codex 等 session 是任務當下的執行端。

## 2. 它不負責什麼／常見失敗

- 不把所有代理的完整對話灌進管理者。全文保留在來源端，供特定問題追查。
- 不把程序存活、收到指令、呼叫工具、產出成果、驗收通過合成一個「進行中」。
- 不讓模型靠聊天措辭推定新授權，也不因代理寫「需要批准」便一律打擾操作者。
- 不再建立一套平行的 task rail、review gate 或跨機器公開服務。

## 3. 角色與責任

| 元件 | 寫入／維護 | 不代替誰判斷 |
| --- | --- | --- |
| 工作代理 | 任務檢查點、停止原因、決策請求、完成主張 | 不替獨立審查者宣告驗收，不替操作者增加授權 |
| Runtime adapter | 真實 session/instance 綁定、工具與生命週期事件、收件證據 | 不從 idle 或最後一句話猜任務完成 |
| Edda 投影與收件匣 | 任務卡、事件游標、依賴、授權引用、待處理問題 | 不重作已有任務／review 狀態機 |
| 管理者 | 按授權處理問題、提出具體下一步、選取必要證據 | 不接手每個 worker 的全部實作脈絡 |
| 操作者 | 目標與授權邊界、真正新增或衝突的決策 | 不承擔可委派的例行確認 |

資訊分開保存：

1. **觀測事實**：runtime 是否在跑、哪個工具結束、是否失聯。
2. **代理主張**：它說正在修什麼、為何停下、認為完成哪些項目。
3. **驗收證據**：固定版本的產物、測試、指定驗收者的判決。
4. **授權依據**：操作者指令、有效委派範圍、明確保留的事項及版本。

四者矛盾時顯示矛盾並查證，不以最後一則自然語言覆寫其他來源。

## 4. 系統位置

```text
                    operator goals / delegation
                                |
                                v
 Pi / Codex worker <---- commands / decisions ---- manager
       |                                          ^
       | reports + observed runtime events        | selected context
       v                                          |
 runtime adapter ---> Edda task cards + attention inbox
                           |             |
                           v             v
                    evidence links   durable read cursor

 Full conversations stay with workers; fetch relevant spans only on demand.
 Existing task/review/authority services remain the state and authority owners.
```

雙向不是「雙方都能發聊天文字」便結束。上行要能回傳狀態、問題、產物；
下行要能傳達具體動作、對哪個問題的決定與採用的授權版本；接收方再回傳
是否接受、是否開始處理及後續成果。完整循環見 [canonical-form.md](./canonical-form.md)。

## 5. 上下文分層與預算

共用的是帶來源與版本的資料，不是模型內部推理或任意摘要拼接。

| 層 | 內容 | 載入規則 |
| --- | --- | --- |
| L0 索引 | 任務名稱、目前階段、阻礙類別、是否有新事件 | 確定性查詢／排序；不必逐張交給 LLM |
| L1 任務卡 | 目標、授權引用、驗收條件、目前里程碑、下一步及證據索引 | 只取需要關注或使用者詢問的任務 |
| L2 決策包 | 一個問題、候選方案、推薦及短理由、影響、相關授權／證據 | 管理者作決定前載入 |
| L3 來源片段 | 與該問題相關的對話段、程式、測試或判決 | L2 不足時按引用取回；跨專案讀取仍受範圍限制 |

v0 調校起點：每次最多處理 3 張熱任務卡、1 個決策包，目標管理輸入不超過
8k tokens；另以序列化輸入 32 KiB 做硬性位元組預算。token 是模型相關估值，
不能宣稱與 byte 上限等價。這些是容量參數，不是精確品質分數。

授權、範圍、問題、版本與反對證據不可因預算而靜默截掉。放不下時改為
`needs_context`、縮小問題或分頁；禁止缺少授權資料卻繼續批准。

```typescript
// Canonical definitions: ./shared-types.md, sections 3 and 5.
// Manager input uses TaskCard + DecisionRequest + referenced evidence spans.
```

## 6. 現有基礎與待補差距

基線：本地 integration `69ca3aba99308d59838fd713b17b39a1117ca876`；下表是
本地能力盤點，不代表已合併或已通過遠端 CI。

| 已有 | 下一步缺口 |
| --- | --- |
| [Pi 通道](../../../integrations/pi/README.md)：session、訊息去重、收件紀錄 | runtime instance 與 task/attempt 的正式綁定 |
| 雙向回覆、目前 branch／legacy transcript 讀取 | 結構化停止原因、檢查點、決策包；全文應降為診斷後援 |
| `watch`、scope、cursor、reply intent、checkpoint | 按語義變化觸發的收件匣；上下文預算；獨立的進度與驗收投影 |
| [task.session](../../../spec/events/task.session.schema.json)、[task.done](../../../spec/events/task.done.schema.json) | 對新觀測與報告的映射，保持既有 receipt/attempt 規則 |
| [Agent client contract](../../reference/client-contract.md) | 經既有 MCP／服務層提供新能力；能力探測，不杜撰現有命令 |

尚在其他工作分支中的 authority/control 功能不視為已可依賴。
落地前必須釘選接受的基線，檢查真正可用的服務介面；不能讓 JS helper
另寫一個業務狀態機來填空。

目前若使用每次都喚醒模型的 host heartbeat，只能作為過渡方案；它不符合
「沒有新問題就不呼叫管理模型」的目標。P4 需要可在模型之外執行的事件／輪詢
分流入口；先盤點 host 與既有 runtime 能力，再選擇落點，不能把這個缺口說成已完成。

## 7. 兩個典型管理情境

以下是規劃用投影例子，不是對實際代理發出的新指令。

```typescript
// Pick<TaskCard, "task" | "attention" | "nextStep">
const needsDecision = {
  task: { projectId: "demo-character", taskId: "46" },
  attention: "decision_required",
  nextStep: "核對已委派的測試資料庫變更範圍，回答 request-46-1"
};
```

管理者只取得這一項測試資料庫決策的範圍與證據；若有效委派已包含它，
直接決定並回傳，不要求操作者再說同一句「批准」。

```typescript
// Pick<TaskCard, "task" | "attention" | "nextStep">
const silentStop = {
  task: { projectId: "demo-edda", taskId: "200" },
  attention: "missing_report",
  nextStep: "先讀取 adapter 狀態與最近 checkpoint，必要時請 worker 補停止原因"
};
```

第二例不先讀整份歷史，也不直接重開 worker。缺少回報表示未知，不表示失敗。

## 8. 分期與本輪邊界

| 階段 | 交付焦點 | 可見驗收 |
| --- | --- | --- |
| P1 回報契約 | task/attempt 綁定、checkpoint、停止原因、完成主張 | 停止會出現在收件匣；未回報者明確顯示未知 |
| P2 決策上下文 | 任務卡、決策包、來源引用與預算 | 管理者解決一個問題，不需載入其他 worker 的全文 |
| P3 受委派閉環 | 回覆與授權版本綁定、陳舊決定拒收、結果追蹤 | 例行問題可自主回答；被保留事項只升級一次 |
| P4 多代理運作 | 增量事件、依賴喚醒、重啟恢復、公平排程 | 50 個正常任務不造成 50 次模型巡問 |

第一個 MVP 是 **兩個受管理任務、一個 Pi adapter、一位管理者**，覆蓋
「等待決策」與「沒有停止原因」兩條路徑。Codex 執行端之後沿相同契約接入。
本輪不指定實作工期、issue、模型品牌或新的常駐服務，也不啟動代管。

升格狀態：仍是概念設計草案。核心循環可討論，命名與 authority 映射尚待確認；
情境是紙上驗收設計，不宣称結構化回報閉環已跑通。因此尚不轉成實作任務包。
