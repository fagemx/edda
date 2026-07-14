# Task Rail v1: ledger-driven multi-agent handoff（conductor 升級案）

> Status: **design landed**（2026-07-14,owner 裁定方向後落檔;實作未動工）
> Repo: `C:\ai_agent\edda`
> Language: **Rust**（ACP client 以 Rust 重寫語義,不是搬 JS 碼)
> 參考實作: `C:\ai_agent\bryti\adapters\session-acp.js`（ACP 傳輸層,已 live-verified）
> 對照案例: `C:\ai_project\agmsg`（借觀念、不搬 watcher)
> 起源: 2026-07-14 對談——「A 做完自動叫 B,不用輪詢,怎麼更穩」

## 0. Binding constraints（owner 裁定,不重議）

1. **Runner 傳輸一律 ACP(Agent Client Protocol),禁用 `claude -p` headless。**
   理由:ACP 給真 session(可 `session/load` 續接、JSON-RPC 全程可駕駛、
   permission 是協議內建通道);`-p` 是一次性黑箱,crash 只能重跑不能續跑。
2. 真相只有一份:任務狀態轉移寫進 hash-chained ledger,不另起第二個 store。
3. 兜底層 reconciler 是純確定性子命令,**零 LLM、零常駐 process**。
4. **rail 的 `task.done` ≠ 驗收。** 裁決權在 wusanto gate;rail 只保證
   「活有人做、收據有留」,永遠不自我認證驗收(§8)。

## 1. 問題

「A 做完 → B 接手」目前有兩種既有做法,都不到生產級:

- **agmsg 式輪詢 watcher**:讀 SQLite 很穩,但長駐哨兵(`watch.sh`)會死,
  而且死了沒人知道——靜默失聯是生產線不可接受的失效模式。
- **conductor 現況**:plan/topo(DAG)+ state/machine + runner 已存在,
  但推定是「指揮家活著才有效」的 foreground 模型(§8 待驗證)。
  指揮家 process 死掉 = 整條 pipeline 停擺。

核心病灶相同:**可靠性繫在某顆必須一直活著的 process 上**。

## 2. 解法:把存活問題外包

> 觸發用推的、可以掉;真相寫在帳本裡、由對帳兜底。
> 掉一次觸發沒關係——狀態還在,最多晚幾分鐘被撿起來。
> Level-triggered(看狀態)而非 edge-triggered(等事件)。

```
L1 真相層   ledger:task 事件(hash-chained,append-only)
            `edda task done` 一筆交易同時寫「done+收據」與「繼任者 ready」
            （transactional outbox——A 死在半路也不會做完了沒交棒）

L2 快路徑   spawn-on-finish:done 的同一動作經 ACP 把繼任 agent 叫起來
            正常情況 B 秒級起跑;這條掉了也無妨,L3 會撿

L3 兜底     `edda reconcile`:OS 排程器(Windows Task Scheduler / cron)
            每 N 分鐘跑一次,掃帳本補漏
            唯一的「常駐者」是 OS 排程器——整台機器上最不會死的東西
```

失效模式對照:輪詢 = 哨兵死了沒人知道;本案 = 快路徑掉了,對帳最多晚
N 分鐘補上。**靜默失聯 → 有上限的延遲**,這就是升級的全部意義。

## 3. 資料模型(ledger 事件)

沿用 edda 事件慣例,新增 `task.*` 家族(全部入 hash chain):

| 事件 | 欄位 | 備註 |
|---|---|---|
| `task.created` | task_id, title, brief_ref, assignee(agent label), agent_kind(claude-acp/codex-acp/…), after:[task_ids], plan_id?, work_unit_ref?, idempotency_key | DAG 依賴用 `after`;readiness 由投影推導;`work_unit_ref` 對映 wusanto work unit(§8) |
| `task.started` | task_id, lease_ttl_s, attempt | 取租約 |
| `task.session` | task_id, acp_session_id | session/new 回來就記——**續跑的關鍵** |
| `task.done` | task_id, receipt, evidence_paths[] | 同 transaction 推導繼任者 ready |
| `task.failed` | task_id, reason | 含 `ended-without-receipt`(agent 講完沒交收據) |
| `task.requeued` | task_id, attempt | reconciler 寫,attempt 有上限 |

**租約續期不入 hash chain**(噪音):放 mutable side table `task_leases`,
與 edda 既有「ledger vs derived views」分離一致。

**收據 ≠ 送達**:`task.done` 必附 receipt。沒有收據的完成不存在——
這是 fleet 收據文化在資料模型上的落點。

## 4. ACP runner(`edda-conductor/src/runner/acp.rs`,新增)

語義全部對照 bryti `session-acp.js`(該檔已 live-verified 的部分直接繼承):

### 4.1 Agent registry(隨 config 可擴)

| key | 啟動 | live-verified |
|---|---|---|
| `claude-acp` | `npx @agentclientprotocol/claude-agent-acp` | ✅ 2026-07-06(v0.56.0,三關全通)|
| `codex-acp` | `npx @agentclientprotocol/codex-acp` | ✅ 2026-07-06(v1.1.0)|
| `opencode-acp` | `opencode acp` | ✅ 2026-07-07(v1.0.223)|
| `hermes-acp` | `hermes acp` | ❌ 未實跑,drill 通過才翻 true |

裸名 `claude-agent-acp` 在 npm 是 404;Zed 的 `@zed-industries/claude-code-acp`
已棄用——**入口名以上表為準**,別再踩一次。

### 4.2 驅動流程(三關)

```
spawn child(stdio JSON-RPC, newline-delimited)
  → initialize {protocolVersion: 1}
  → session/new {cwd, mcpServers} → 拿 sessionId → 立刻寫 task.session 入帳
  → session/prompt {sessionId, prompt:[{type:'text', text: <brief 注入包>}]}
  → 等 stopReason: end_turn
  → 驗收:讀 ledger 確認 agent 自己跑了 `edda task done`
    沒有 → 寫 task.failed reason=ended-without-receipt(reconciler 有限次重排)
```

### 4.3 已知地雷(bryti 踩過,直接繼承解法)

- **F7 巢狀守衛**:子進程 env 必剝 `CLAUDECODE`、`CLAUDE_CODE_ENTRYPOINT`、
  `CLAUDE_CODE_SSE_PORT`,否則在 Claude Code session 內 spawn 時
  `session/new` 直接死。剝除是正解不是繞過(子進程是獨立 stdio agent)。
- **win32**:spawn 需經 shell(npx 的 .cmd/.ps1 shims);本機另有
  `NoDefaultCurrentDirectoryInExePath=1` 硬化,bare `.cmd` 解析要留意。
- **server→client 請求不可不理**:`session/request_permission` 沒人回會
  永久卡死。v0 底線 = 自動回拒但必回覆(bryti autoDeny);
  v1 對接 fleet 預授權信封——per task class 的白名單 policy,handler 可注入。

### 4.4 續跑(ACP 獨有紅利)

租約過期時,reconciler 查 `task.session` 有無 acp_session_id:
- 有 → 試 `session/load` 續接原 session,prompt「continue task X」
- 載不回來 → `task.requeued` 重起新 session(attempt 上限內)

`-p` 模式下 crash 只能全部重跑;ACP 讓 crash recovery 從「重做」降級成
「續做」。此為選 ACP 的最大實質理由。

## 5. CLI surface

```bash
# agent 動詞(寫進 edda init 的 CLAUDE.md 教學區,與 decide/note 同級)
edda task new "跑整合測試" --assignee tester --agent codex-acp --after 12
edda task start 13                # 取租約
edda task done 13 --receipt "110/601 綠,產物在 dist/"   # 一個動作=交棒
edda task fail 13 --reason "..."

# 用戶動詞
edda task list / edda task show 13
edda plan run release.md          # conductor DAG 糖衣(topo 已有)
edda plan status                  # 一眼看整條 pipeline 卡在哪
edda reconcile                    # 兜底掃描(排程器目標,亦可手跑)
edda reconcile --install-scheduler  # 註冊 schtasks / cron
```

Hooks(edda 已有的裝置直接沿用):
- **SessionStart**:互動 session 起跑時注入「指派給你的 pending tasks」
- **Stop**:回合結束查「有沒有新指派」——輕量 nudge,不是傳輸主力
- **brief 注入包**(state/brief + edda-pack 組裝):任務簡報 + 上游收據 +
  相關 binding decisions。**帳本就是通訊**——B 不需要收訊息,
  B 起跑時該知道的都在開場 prompt 裡。

## 6. 與現有元件的關係

| 元件 | 處置 |
|---|---|
| `conductor/plan`(parser/schema/topo) | **保留**,DAG 定義照用 |
| `conductor/state`(brief/derive/machine/persist) | machine 改為「事件投影」:狀態從 ledger 推導,不自持真相 |
| `conductor/runner` | 新增 `acp.rs` 為主傳輸;`tmux.rs` 降級為 Linux 顯示殼(可看,不承載可靠性) |
| `edda-notify` | 可選通知層(「B 起跑了」推給人看),不承載交棒 |
| agmsg | 借:SQLite WAL 地板、watermark 只看新事件、durable 可重播。不搬:長駐 watcher、訊息式交棒(訊息可掉,狀態不可掉) |

## 7. 分期(每期獨立可用)

| 期 | 交付 | 驗收 |
|---|---|---|
| **P1** | `task.*` 事件 + CLI 動詞 + Stop-hook nudge | 兩個互動 session 手動接力一條 2 步鏈;ledger 重播看得到完整交棒鏈 |
| **P2** | `edda reconcile` + 租約 + `--install-scheduler` | 中途 kill 掉執行者,N 分鐘內任務被重排並完成;冪等鍵擋住重複副作用 |
| **P3** | ACP runner(spawn-on-finish + brief 注入 + 續跑) | A done → B 經 ACP 自起,無人介入;F7 剝除在巢狀 session 實測;win32 實測;`session/load` 續跑 drill 至少一個 agent 通過 |
| **P4** | plan DAG 糖衣 + `edda watch` task board | 3 步驟 × 3 種 agent 的 pipeline 在 watch TUI 全程可視 |

護欄(全期適用):auto-spawn 是 **opt-in**,每小時 spawn 上限 + 每任務
attempt 上限——沿用 edda「LLM 花費一律 budget-capped」哲學。

## 8. wusanto 整合(訂單層 ↔ 傳動層)

> wusanto 是監理所(定義什麼叫做完、驗收、發照);task rail 是輸送帶
> (把活從 A 送到 B,保證不掉)。訂單從 wusanto 進,執行在 rail 跑,
> 收據回 wusanto 驗。

### 8.1 全鏈流程

```
wish(用戶一句話)
  → wusanto planner 生 work units + 驗收條款(planning pack)
  → 每個 work unit 落成 rail task:
      edda task new "WU2 測試" --assignee tester --agent codex-acp \
        --after <WU1-task> --work-unit <wu_id>
      brief 夾:該 WU 的驗收條款 + 上游收據
  → rail 執行(§2–4:交棒、租約、續跑、重排)
  → task.done(receipt + evidence_paths)
  → bridge:receipt 提交 wusanto submit_evidence_candidate
  → gate review 裁決
      過   → 下一個 work unit 的 task 解鎖
      不過 → bridge 生 rework task 回 rail(brief 夾打槍理由,attempt 上限)
```

任務不是人手寫的:**wusanto planner 是訂單來源,rail 是排程與傳動**。
`control.json` 車間後門在此升級路徑上被 rail 取代。

### 8.2 整合原則:不傳訊息,同一本帳的投影

進度與結果不是「訊息」在兩系統間流——兩邊讀寫同一本 ledger:

| 視角 | 入口 | 本質 |
|---|---|---|
| 執行進度(哪步在跑、卡哪) | `edda plan status` / `edda watch` | `task.*` 事件投影 |
| 治理進度(mission 到哪、gate 過沒) | wusanto `get_mission_status` / `list_work_units` | 同一本帳,換個投影 |
| 執行結果 | `task.done` 的 receipt + evidence_paths | bridge 餵 evidence candidate |

通知(如 Telegram 推「WU2 done」)走 edda-notify——裝飾,不是真相。

### 8.3 Bridge(唯一要新寫的膠水:零 LLM,兩方向)

| 方向 | 觸發 | 動作 |
|---|---|---|
| rail → wusanto | `task.done` | `submit_evidence_candidate`(receipt、evidence_paths、work_unit_ref 對映) |
| wusanto → rail | gate 打槍 | `edda task new` rework(`--after` 原 task、brief 夾 rejection reason、attempt 計數沿用) |

bridge 只投料、不裁決:wusanto MCP 端點本來就是 candidate 制
(submit_* 全是 candidate),裁決永遠留在 gate。兩層「完成」故意分開——
rail done = 執行完畢有收據;wusanto accepted = 驗收條款通過。
agent 亂交差過不了 gate,gate 打槍自動變成帶理由的重工任務回輸送帶,
閉環不需人肉搬運。

### 8.4 權限:對接預授權信封

runner 的 `session/request_permission` policy(§4.3)per task class 掛
一個預授權信封:信封內 → 自動准;信封外 → 回拒並停下等 operator。
與 fleet.preauth-envelope1 已裁定的精神一致——授權是事前簽好的範圍,
不是執行中臨時討價還價。

## 9. 誠實邊界(動工前必查)

1. **conductor 現況考古**:`state/persist` 落盤的東西,指揮家死後能否被
   獨立 process 撿起續跑?若不能,P2 前先補這一刀。
2. **`session/load` 語義**:bryti 只 live-verified 三關
   (initialize/new/prompt);load/resume 各 agent 要逐一 drill,
   通過一個翻一個 verified——不預先謊報。
3. **schtasks 註冊 UX**:硬化 shell 環境(NoDefaultCurrentDirectoryInExePath)
   下的路徑解析,以及使用者不在登入狀態時的觸發行為。
4. **Rust ACP client**:自寫 JSON-RPC(newline-delimited)成本低、
   不引 node 依賴進 edda 本體;agent 端(npx)反正需要 node,
   但 edda 自己保持純 Rust 器官。
5. **agent 不聽話面**:brief 教了 `edda task done`,agent 仍可能不跑——
   `ended-without-receipt` + 有限重排是底線,重排 prompt 要加壓
   (「你上次沒交收據」)。此面向無法確定性消滅,只能收斂。
6. **bridge 對映與端點**:`work_unit_ref` 欄位 P1 就要進 schema
   (事後補對映最痛);wusanto MCP 是 session 級連線,bridge 作為
   獨立 process 要用哪條路呼叫端點(MCP client / CLI)先驗證再動工。
