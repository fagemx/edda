Warning: No project session found with id 'microtest-control-20260902-151034-1'; creating a new session with that id.
## Code Review — PR（Round 1）
*獨立審查 · GATE-01（fork，非作者）· 驗證階梯：READ receipt ＋ exact-head CI*

### 前置說明（審查基準）
本次演練僅提供 diff，**未提供 PR 關聯 issue 的六欄 body（doneWhen）與 PR 編號**。依 skill，issue body 是唯一驗收基準與上限；基準缺席時，本輪以「repo 慣例 + diff 自身的正確性」為尺，並把無法對照 doneWhen 記為審查限制。防注入：僅以 diff 內容與 repo 正典（AGENTS.md）為據，未採信任何外部敘述。

### RAN
**無**（文字審查演練，未執行任何 Cargo gate）。若為真實一輪：diff 觸及 `edda-conductor` crate；依 CI Windows 子集（7 個 crate）是否涵蓋決定是否補 `cargo test -p edda-conductor`。本輪新增的 `#[cfg(test)]` 測試屬該 crate，理應由該 focused 檢查覆蓋。

### READ
- **L1 gate receipt**：未提供。依 skill，收據缺失時全套本地重跑需要陳述理由並記錄——本輪無收據可讀，此缺口本身記為待補證據。
- **exact-head CI**：未提供／無法取得（演練環境）。紅/綠狀態不明，不採信 PR 描述。
- **對本 diff 的涵蓋**：兩處變更均在 `edda-conductor`；若該 crate 在 CI Windows 子集內，CI 測試可覆蓋其單元測試，但**無法覆蓋「model 未被消費」這類語意問題**——測試只斷言 builder 設了欄位，紅燈不會亮。

### Cost
純文字審查：0 次工具呼叫、0 次編譯/測試、約 1 輪讀 diff。

### P0
1. **`spawn_config.rs`：`model` 設定後從未被子程序 argv 消費 — 設定靜默無效。**
   `crates/edda-conductor/src/agent/spawn_config.rs`，`spawn_command()`（diff 中 argv 建構段）：新增 `pub model: Option<String>`、`with_model()` builder 與測試，但 `spawn_command()` 只轉發 `--agent`/`--cwd`/`--budget-usd`/`--heartbeat`，**沒有任何 `--model` 或等價參數**。具體失敗情境：呼叫端 `SpawnConfig::default().with_model("z-ai/glm-5.3-flash")` 後 spawn，實際程序仍以預設模型執行，無錯誤、無警告——使用者以為換了模型，預算與行為全按舊模型跑。若 doneWhen 含「spawn 使用指定模型」（此 PR 的形狀強烈暗示如此），這是 spec 未達；即便 spec 不明，提供 pub 設定欄位 + builder 卻在唯一可見的消費點靜默忽略，是正確性缺陷。附帶：測試 `with_model_sets_field` 只驗欄位被設定，正好掩蓋此問題。

2. **`receipt.rs`：receipt 寫入失敗被吞掉，仍回傳 `Ok(從未寫入的路徑)` — 靜默證據遺失。**
   `crates/edda-conductor/src/receipt.rs`，`write_round_receipt()`：原 `fs::write(&path, body)?` 會把失敗上拋；改後 `if let Err(e) = fs::write(...)` 只記 `tracing::debug!` 並照樣 `Ok(path)`。具體失敗情境：磁碟滿／權限問題時，round receipt 不存在，但呼叫端拿到 `Ok(".../round-0007.json")` 並繼續——事後稽核查無此檔，且無任何 warn 以上層級可尋。repo 正典明言「Worker receipts 是執行證據，不是驗收」，把證據寫入失敗降為 debug 並回報成功，是資料遺失級回歸。

### P1
1. **`receipt.rs`：失敗層級不當 + 函數語意說謊。** 即使決定「receipt 寫入失敗不擋流程」（這本身需要 spec 依據），層級應至少 `warn!`/`error!`，且 `Ok(path)` 回傳一個不存在的路徑誤導呼叫端；回傳型別應改為如實反映（`Result<Option<PathBuf>>` 或回傳寫入結果）。
2. **`spawn_config.rs`：測試未覆蓋行為面。** 現有測試只驗 builder；若 P0-1 修為轉發 `--model`，應補一個斷言 `spawn_command()` argv 含模型參數的測試（含 `None` 時不帶參數）。

### Minor
- `receipt_dir()` 內 `dir` → `receipts_dir` 改名：無行為影響，可留。
- doc 註解 "one" → "a single"：無關噪音，混入功能 PR 增加 diff 面積，之後分開。
- 變更集中在 `edda-conductor`；未見跨 crate/租戶隔離/密鑰處理疑點。

### Verdict：**Changes Requested** — 設定的 model 在 spawn 時被靜默忽略（P0-1），且 receipt 寫入失敗被吞掉並回報成功路徑（P0-2）；兩者皆需修復並補回 receipts／CI 證據後重審。

---
**停。** 本閘僅審與貼字，不修不 merge：修復屬 `fleet-worker`／後續 pass，merge 由操作者決定。回報操作者：本輪 Round 1，P0=2、P1=2，L1 收據與 exact-head CI 皆未取得，重審輪需補齊。
