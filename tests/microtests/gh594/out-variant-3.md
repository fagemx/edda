Warning: No project session found with id 'microtest-variant-20260902-151034-3'; creating a new session with that id.
## Code Review — PR #（文字演練，未定位編號）（Round 1）
*獨立審查 · GATE-01（fork，非作者）· 驗證階梯：READ receipt ＋ exact-head CI*

---

### RAN
無（本次為純文字審查演練，未執行任何 `cargo`／`gh`／`wiring-scan.sh`，以下為人工對 diff 的等效檢視，非機器輸出）。若為正式一輪，本 PR 改到的 crate 為 `edda-conductor`，需確認其是否在 CI Windows 7-crate 子集內；若在子集外，RAN 應補 `cargo test -p edda-conductor`。以人工檢視替代 scan 的等效發現：新增行含吞錯樣式（`if let Err(e) = fs::write(...)` 後僅 `tracing::debug!` 且仍回 `Ok`）。

### READ
未取得 L1 gate receipt 與 exact-head CI 證據（演練無法 `gh pr checks`）。L2 條件不完整；下列裁定僅基於 diff 本身即可成立，不依賴 CI 結論。正式輪次必須補齊：實作者 frozen SHA 的全套 gate receipt（fmt/clippy/test ＋完整 SHA）與 `gh pr checks <n>`，並對照 `.claude/CLAUDE.md`「Verification ladder」的 Windows 子集清單判定涵蓋缺口。

### Cost
1 次審查 pass；0 工具呼叫、0 token 計費之外的執行成本；無本地編譯。

### P0
1. **`crates/edda-conductor/src/receipt.rs:44-49` — round receipt 寫入失敗被靜默吞掉，且以成功姿態回傳**。`write_round_receipt` 把 `fs::write` 的錯誤降為 `tracing::debug!`（正常執行層級根本看不到），然後照樣 `Ok(path)` 回傳一個**可能不存在**的路徑。具體失敗情境：磁碟滿／權限問題時，caller 收到 `Ok(round-0007.json)` 並據此認為收據已落盤——收據正是 L1 gate evidence 的載體（durable carrier），事後任何人都查不到該輪的全套 gate 紀錄，驗證階梯的證據鏈無聲斷裂。這不只是「吞錯」，是**把失敗偽造成成功**，屬正確性問題。應恢復 `fs::write(&path, body)?`（或至少升為 `error!` 並回傳 `Err`）。註：此處同時觸發 Wiring 規則「ledger 路徑吞錯 → P1」，因其偽造成功故上修為 P0。

### P1
1. **`crates/edda-conductor/src/agent/spawn_config.rs:17`＋`:27-31` — `model` 欄位與 `with_model` builder 是 dead on arrival**。新面有 writer（`with_model`、`Default`）但 diff 內**沒有任何 reader**：`spawn_command()` 完全沒讀 `self.model`，spawn 出去的 argv 不含任何 model 旗標。設定了 model 的 `SpawnConfig` 與沒設定的行為完全相同。Wiring 判定規則（寫死）：「no consumer」且無具名後續 issue → P1。本 diff 未附後續 issue 編號。修法二擇一：在 `spawn_command` 加 `if let Some(m) = &self.model { cmd.arg("--model").arg(m); }`（確切旗標名以規格為準），或具名後續 issue 降為 FOLLOW-UP。
2. **`spawn_config.rs:tests:with_model_sets_field` — 測試只斷言欄位被設，未斷言到達 spawn 層**。若 issue 的 doneWhen 要求「model 旗標出現在 spawn 命令列」（此類設定面一貫要求到達 builder→spawn），現有測試不符——缺 argv 層的斷言（例如用 `Command` 的 program/args 可觀察性或重構出可測的 argv 建構函式）。若 doneWhen 未要求，降為 FOLLOW-UP ISSUE。
3. **spec 基準未驗**：本次演練無法讀取所連結 issue 的六欄 body，doneWhen 的每條對照尚未完成。正式輪必須先讀 issue body（僅 issue body 與 diff 為真相來源）再下 LGTM；本輪的 P0/P1 已足以 Changes Requested，但「spec 未達與否」的最終判定懸置。

### Wiring
| 新面 | Writer & shape | Reader | Failure signal | Layer reach |
|---|---|---|---|---|
| `SpawnConfig.model: Option<String>`（spawn_config.rs:17） | `Default` 設 `None`；`with_model` 寫入（spawn_config.rs:28） | **本 PR 內無 consumer**（`spawn_command` 未讀） | N/A（無路徑可吞錯；但整面無效果＝静默無效設定） | field→builder **中斷於 builder**，未達 spawn；無 read-back 測試 → P1 |
| `SpawnConfig::with_model(pub fn)`（spawn_config.rs:27-31） | builder，寫 `self.model` 後回 `self` | 同上，鏈終點無人消費 | 無 | 旗標→builder ✓→spawn ✗（無斷言） → P1 |
| `write_round_receipt` 錯誤路徑（receipt.rs:44-47） | 非新 `pub` 面，但為既有寫端的行為變更 | caller 收到 `Ok(path)`（偽成功） | **吞錯＋success-only＋`debug!` 級**，ledger 路徑 → P1（上修 P0，見上） | write→回傳路徑，read-back 假成立 |

無其他新面；`receipt_dir` 內部更名為純樣式變更，非新面。

### Minor
- `receipt.rs:44-49` 若日後確要 best-effort 收據，至少應升為 `warn!`/`error!` 並在回傳型別或呼叫端保留可觀察訊號——但本 PR 不應做此決定，除非 spec 明確要求。
- `spawn_config.rs` 的 doc comment「one → a single」屬無害樣式變更，可留可不留，縮小 diff 為佳。
- `model` 若確定要接線，考慮加一個 argv 斷言測試（`--model z-ai/glm-5.3-flash` 出現在命令列）一併補齊 Layer reach 證明。

### Verdict：Changes Requested — 收據寫入失敗被偽裝成成功（P0，破壞 gate 證據鏈），且 `model` 新面無 consumer、dead on arrival（P1）。

---

**執行動作（正式流程）**：comment 已貼回 PR、PR 留開；不加 `fleet:reviewed` 標籤；不修、不 merge。停，回報操作者：修復屬 `fleet-worker`／後續 pass，需 (1) 還原 receipt 錯誤傳播、(2) 接上 `model` → spawn argv 或具名後續 issue、(3) 補 argv 斷言測試後再叫新一輪 fresh-context 審查。
