Warning: No project session found with id 'microtest-variant-20260902-151034-2'; creating a new session with that id.
## Code Review — PR #?（Round 1）
*獨立審查 · GATE-01（fork，非作者）· 驗證階梯：READ receipt ＋ exact-head CI*

> 演練限制聲明：本次為純文字審查，無工具、無 issue body、無 PR 號碼與實際 repo 狀態。以下 diff 級審查完整，但 RAN/READ 只能記錄「本次演練無法執行」的事實，不虛構結果。

### RAN
無。本次審查為純文字演練，未執行任何 Cargo gate、`wiring-scan.sh` 或 focused 檢查。正常執行時應跑：`scripts/wiring-scan.sh <base> <head>`（本 diff 有新 `pub` 面，必附），以及 issue `verify` 指令與針對下述 P0 的 focused 檢查（見下方紅燈分類）。

### READ
無法驗證。L1 gate receipt（frozen SHA 全套 fmt/clippy/test）與 exact-head CI 均未隨 diff 提供，本輪不採信任何未附收據的「已通過」主張。本 PR 實際改到的 crate 為 `edda-conductor`；其是否落在 CI Windows 測試子集（7 個 crate）內，需比對 `.claude/CLAUDE.md`「Verification ladder」清單——若在子集外，即為涵蓋缺口，正常流程應在 Windows 跑 `cargo test -p edda-conductor` 作為 focused 檢查，而非全套重跑。

### Cost
本輪為純文字演練：0 工具呼叫、無 token 計量、無本地編譯耗時。審查本身為單次 diff 通讀。

### P0
1. **`crates/edda-conductor/src/receipt.rs:44-48` — round receipt 寫入失敗被吞，回傳謊言般的 `Ok(path)`。** 原碼 `fs::write(&path, body)?` 會把失敗往上拋；改後只 `tracing::debug!` 然後照樣回傳 `Ok(path)`——回傳一個**根本沒寫成的檔案路徑**。具體失敗情境：磁碟滿／權限錯誤時，該輪 receipt 永久遺失，但呼叫端（round 協調／帳務路徑）收到成功與合法路徑，後續讀回該 receipt 時才會發現檔案不存在，甚至更晚才發現帳目缺輪。receipt 是 coordination/ledger 產物，靜默丟資料屬正確性缺陷。且 log 層級用 `debug!` 屬 success-only 可見性——死掉的路徑完全無訊號。這同時踩中 wiring 條款「ledger 路徑上吞錯」。
2. **`crates/edda-conductor/src/agent/spawn_config.rs:17,30-33` — `model` 新功能整體未接線，屬 spec 未達。** `spawn_command()` 完全沒讀 `self.model`——`with_model()` 設了欄位後，spawn 出去的 argv 與未設 model 時**逐 byte 相同**。若本 PR 的目的（由新面推斷）是讓 spawn 帶上 model，則功能是死的：呼叫端以為換了模型，實際 spawn 仍用預設。正確性層級的「存在但無效」，不只是風格問題。

### P1
1. **wiring verdict：`pub model: Option<String>`（spawn_config.rs:17）與 `pub fn with_model`（spawn_config.rs:30）皆為「no consumer」且 diff 內無具名後續 issue → dead on arrival。** 唯一讀者只有測試。規則寫死：無 consumer＋無後續 issue 編號＝P1。
2. **`crates/edda-conductor/src/agent/spawn_config.rs:42-45` — 測試只斷言欄位，未斷言 `--model` 出現在 spawn 命令列。** 若 doneWhen 要求 model 到達 spawn 層（旗標→builder→spawn 的最後一跳），現有測試證明不了任何到達——正是「旗標未斷言出現在 spawn 命令列」條款。即使 doneWhen 沒寫到這層，也應列 FOLLOW-UP；以本 diff 推斷的 intent，直接算 P1。
3. **receipt.rs 的吞錯改動疑似超出 issue 範圍。** 一個「加 model 支援」的 diff 同時把 receipt 錯誤處理從 propagate 改成 swallow，是無要求的行為變更，且方向有害（見 P0-1）。若 issue 未要求此變更，應整段還原而非改 log 層級。

### Wiring
| 新面 | Writer & shape | Reader | Failure signal | Layer reach |
|---|---|---|---|---|
| `pub model: Option<String>`（spawn_config.rs:17） | `Default`（None）＋ `with_model`（spawn_config.rs:30-34）寫入 | **no consumer**（spawn_command 未讀；唯一讀者為 spawn_config.rs:43 測試） | — | 旗標→builder ✅（builder 設欄位）；→spawn ❌ **斷裂**：spawn_config.rs:36-45 的 argv 組裝無 `--model`；無 read-back/命令列斷言 |
| `pub fn with_model`（spawn_config.rs:30） | builder，`impl Into<String>` 收斂型別 | spawn_config.rs:43 測試；**無生產端 consumer** | 無錯誤路徑（單純 setter，可接受） | 同上，終點是死欄位 |

receipt.rs 變更非新面（既有寫端 `write_round_receipt` 的行為修改），但其吞錯已在 P0-1 / wiring 條款「ledger 路徑吞錯 → P1+」計分。

### Minor
- `receipt_dir` 內 `dir` → `receipts_dir` 改名：無行為影響，可留。
- spawn_config.rs 頂部 doc comment 措辭改寫（"one"→"a single"）：無意義但無害。
- `format!("{}", self.budget_usd)` 為既有碼，不在本輪範圍；若日後動到可考慮 `{}`→顯式格式，避免科學記號邊角。

### Verdict：**Changes Requested** — receipt 寫入失敗被靜默吞掉（ledger 路徑丟資料且回傳虛假成功），且 `model` 新面無 consumer、未接到 spawn argv，功能整體是死的。

依裁定規則：有 P0/P1 → comment 已貼（本演練以本回覆代之）、PR 留開。**停，回報操作者。** 修是 `fleet-worker`／後續 pass 的事，不是本閘的；merge 由操作者執行。
