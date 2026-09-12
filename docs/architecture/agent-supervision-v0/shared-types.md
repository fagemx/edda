# 管理資料如何保持身分、版本與證據一致？

> Status: `working draft`
>
> Purpose: 為任務卡、狀態回報與決策交換提供單一概念契約。
>
> 這些 TypeScript 是設計型別，不是已發布 SDK、JSON Schema 或新 ledger event。
> 其他文件引用本檔，不重複定義；現有 schema 的相容性仍由原契約管理。

## 1. 一句話與定位

每則管理訊息綁定任務、執行世代、問題版本與證據來源，讓管理者能局部理解並安全接續。

```text
 TaskRef ---> Binding ---> RuntimeObservation / WorkerReport
    |                            |
    +------> TaskCard <-----------+
                 |
 Delegation ---> DecisionRequest ---> ManagerDecision ---> CommandReceipt
                 |                                         |
                 +----------- evidence refs <------- OutcomeReport
```

## 2. 不應混淆的概念與基礎型別

task identity 不等於 session/PID；報告不等於觀測；引用不等於有效授權；
傳輸游標不等於任务進度。下面的字串 ID 是管理 view 的表示，不改寫原 ledger
數字詞元或 hash。taskId 對既有 Edda task ID 使用無損十進位字串；adapter 負責
與既有 task schema 轉換，不由 SDK 自建另一套身分。

```typescript
type TaskRef = { projectId: string; taskId: string };
type Binding = {
  task: TaskRef;
  attempt: number;
  agentKind: "pi" | "codex";
  sessionId: string;
  instanceId: string;
};
type SourceRef = {
  uri: string;                 // ledger event, fixed artifact, or bounded transcript span
  revision: string;            // full SHA, event ID, digest, or immutable entry ID
  provenance: "operator" | "runtime" | "worker" | "verifier";
};
type EventCursor = { instanceId: string; sequence: number };
type Action = { kind: string; resource: string }; // kind comes from an adapter capability registry
type RuntimeState = "running" | "executing_tool" | "idle" | "offline";
type ReportedState = "working" | "waiting_decision" | "waiting_dependency"
  | "completed" | "failed" | "paused" | "unknown";
type Attention = "none" | "decision_required" | "missing_report"
  | "progress_overdue" | "completion_pending" | "conflict" | "needs_context";
type Acceptance = "not_submitted" | "pending" | "accepted" | "rejected";

type RuntimeObservation = {
  binding: Binding;
  cursor: EventCursor;
  observedAt: string;
  runtimeState: RuntimeState;
  kind: "heartbeat" | "turn_started" | "tool_started" | "tool_finished"
    | "turn_settled" | "session_disconnected";
  toolName?: string;           // never requires raw tool arguments or private reasoning
};
```

sequence 只在同一 instance 內單調增加。偵測缺口時要求快照／補頁，不能把缺少
的事件當不存在。另一 instance 的延遲報告只保存為歷史，不能覆寫當前卡。
同一 instance 的觀測與報告由 adapter 統一編序並填入已登記 binding；worker
不能藉由自填 binding/provenance 欄位冒充其他任務、操作者或驗收者。

## 3. 任務卡與授權引用

```typescript
type Delegation = {
  grantId: string;
  version: number;
  sources: SourceRef[];        // resolved against authoritative operator/config sources
  permitted: Action[];
  excluded: Action[];
  reservedForOperator: Action[];
  budgetRef: SourceRef | null; // null means unknown, not unlimited
  validUntil: string | null;
};
type TaskCard = {
  task: TaskRef;
  binding: Binding | null;
  snapshotVersion: number;    // semantic changes; heartbeat alone does not increment
  goal: string;
  goalSource: SourceRef;
  acceptanceCriteria: string[];
  delegation: { grantId: string; version: number } | null;
  runtimeState: RuntimeState;
  reportedState: ReportedState;
  acceptance: Acceptance;
  stage: string;
  lastCheckpointRef: SourceRef | null;
  lastMeaningfulProgressAt: string | null;
  attention: Attention;
  openRequestIds: string[];
  dependencies: TaskRef[];
  nextStep: string;
  evidence: SourceRef[];
  watchPolicy: {
    reportGraceSeconds: number;
    heartbeatStaleSeconds: number;
    progressReviewSeconds: number;
  };
};
```

lastMeaningfulProgressAt 需對應新 checkpoint 或有版本的產物／測試證據；輪詢、
token 串流、心跳不能單獨刷新它。goal/criteria/grant 來源不可由 worker summary
取代。summary 是可重建快取；版本變化後重建相關卡，不全面回放每個 session。
binding 不為 null 時，其 task 必須等於卡的 task；舊 attempt 的證據保留歷史，
不能直接滿足新 attempt 的當前驗收。

## 4. 工作代理報告

```typescript
type Checkpoint = {
  checkpointId: string;
  basisSnapshotVersion: number;
  stage: string;
  completedMilestones: string[];
  nextStep: string;
  evidence: SourceRef[];
};
type StopReport = {
  stopId: string;              // one stop episode, stable across retransmission
  reason: "waiting_decision" | "waiting_dependency" | "completed"
    | "failed" | "paused" | "unknown";
  summary: string;
  nextStep: string;
  requestId: string | null;
  dependencies: TaskRef[];
  evidence: SourceRef[];
};
type OutcomeReport = {
  commandId: string | null;
  result: "action_started" | "completed" | "failed" | "not_applied";
  summary: string;
  evidence: SourceRef[];
};
type WorkerReport = {
  eventId: string;
  binding: Binding;
  cursor: EventCursor;
  recordedAt: string;          // adapter records ingestion time; not proof of work time
  body:
    | { kind: "checkpoint"; value: Checkpoint }
    | { kind: "stopped"; value: StopReport }
    | { kind: "decision_request"; value: DecisionRequest }
    | { kind: "outcome"; value: OutcomeReport };
};
```

`waiting_decision` 必須有 requestId；`waiting_dependency` 必須有 dependencies；
`completed` 必須引用產物／驗收資料。資料不足仍保存原主張，但標為缺資料，
不得自動變成已驗收或停止監測。報告出口出錯不使實作本身失敗。

## 5. 決策包與最小上下文

```typescript
type DecisionRequest = {
  requestId: string;
  basisSnapshotVersion: number;
  question: string;
  requestedAction: Action;
  recommendation: "approve" | "reject" | "defer";
  alternatives: { label: string; impact: string }[];
  rationale: string;           // short justification, never hidden chain-of-thought
  authorityRefs: SourceRef[];
  evidence: SourceRef[];
  missingContext: string[];
};
type DecisionContext = {
  card: TaskCard;
  request: DecisionRequest;
  effectiveDelegation: Delegation | null;
  selectedSpans: { source: SourceRef; text: string }[];
  conflicts: string[];
  missingContext: string[];
  budget: { maxSerializedBytes: number; targetInputTokens: number; tokenCountKnown: boolean };
};
```

引用取回遵守資料範圍；不允許問題包引用任意檔案便獲得讀取授權。
來源內容視為待判斷資料；只有由 authority resolver 識別的操作者來源能提供授權。
必填資料放不進 context 預算時返回 needs_context，不能截斷後自動批准。

## 6. 管理者回覆與收件結果

```typescript
type ManagerDecision = {
  commandId: string;           // persisted before delivery; same logical decision reuses ID
  requestId: string;
  target: Binding;
  managerEpoch: string;        // current manager ownership generation
  expectedSnapshotVersion: number;
  grant: { grantId: string; version: number } | null;
  action: "approve" | "reject" | "defer" | "request_context" | "request_report";
  instruction: string;         // concrete next step, not an unqualified continue
  rationale: string;
  evidence: SourceRef[];
};
type CommandReceipt = {
  commandId: string;
  target: Binding;
  status: "recorded" | "received" | "unconfirmed" | "rejected" | "unknown";
  reason: "ok" | "stale_task" | "stale_instance" | "stale_grant"
    | "wrong_manager" | "not_authorized" | "backend_unobservable";
};
```

缺少 StopReport 的補報命令使用收件匣建立的 stop-episode requestId，並不需要
杜撰 worker 已提出問題。approve 必須有可解析的有效 grant；其他動作也需
相應讀取／互動能力，grant=null 不代表自由執行。接收方重新驗證 task、instance、
managerEpoch 與授權版本；舊決定不能僅靠訊息前綴進入新版任務。

transport receipt 只證明傳送層結果；真正的工作開始由 OutcomeReport 表示，
完成與驗收由 TaskCard 的不同欄位呈現。scope、版本或目標改變就是新決定，
不能修改舊 commandId 的 payload。

## 7. 具體例子

```typescript
const waiting: StopReport = {
  stopId: "stop-46-1", reason: "waiting_decision",
  summary: "設計已接受，等待測試環境 migration 決定",
  nextStep: "執行 request-46-1 的結果", requestId: "request-46-1",
  dependencies: [],
  evidence: [{ uri: "ledger://demo-character/schema-review", revision: "review-46-r2", provenance: "verifier" }]
};
```

```typescript
const started: OutcomeReport = {
  commandId: "command-46-approve-1", result: "action_started",
  summary: "已建立隔離工作樹，開始核對測試資料庫目標",
  evidence: [{ uri: "artifact://demo-character/worktree-receipt", revision: "sha256:4b26", provenance: "runtime" }]
};
```

第二例的 revision 是示意引用；實作驗收必須用實際完整 digest／SHA，不能將
短範例當作可驗證證據。URI scheme 在本稿是概念標記，尚不是可呼叫協定。

## 8. 相容與尚未定案事項

- 新契約是 task/session/receipt 上方的管理 view；不取代 Edda 既有事件 canonicalization。
- 新資料若進 ledger，必須加入原本 schema registry 與共用 service，再產生 SDK；
  不能讓各 adapter 手寫不同版本的狀態規則。
- source binding、authority resolver、manager ownership/CAS 的實際介面待基線盤點；
  這些契約仍是 working draft，沒有宣稱 runtime 已具備所有驗證。
- 遷移中的舊代理只能提供觀測及有限對話，故其 reportedState 可為 unknown；
  不要求所有代理先升級，才允許其他代理繼續工作。
