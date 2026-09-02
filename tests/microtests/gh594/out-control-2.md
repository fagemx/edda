Warning: No project session found with id 'microtest-control-20260902-151034-2'; creating a new session with that id.
## Code Review — PR #?（Round 1）
*獨立審查 · GATE-01（fork，非作者）· 驗證階梯：READ receipt ＋ exact-head CI*

> **演練聲明**：本次為純文字審查演練，未執行任何指令（無 `gh pr checkout`、無 `gh pr checks`、無 cargo）。以下 RAN／READ 如實記錄「實際未執行」，不虛構通過數；並依 skill 規則說明真實一輪應補的閘。issue body（六欄 doneWhen）亦未提供，spec 合規只能對 diff 內部一致性與 repo 慣例（AGENTS.md 摘要）審查。

### RAN
**無**（演練模式，未執行任何檢查）。

真實一輪應跑的 focused 檢查（記錄待辦）：
- 本 PR 只改到 `crates/edda-conductor` 一個 crate。AGENTS.md 指出 CI Windows 子集只跑 7 個 crate，但本輪無法讀 `.claude/CLAUDE.md` 確認 `edda-conductor` 是否在子集內 → **涵蓋缺口存在不確定性**，應跑 `cargo test -p edda-conductor` 一次以補缺口（僅此 crate，非全套）。
- 針對 P0-1（receipt 寫入失敗被吞掉）：focused 測試「receipt 目錄不可寫時 `write_round_receipt` 的行為」——目前碼面顯示它會回傳 `Ok(不存在的路徑)`。

### READ
- **L1 gate receipt**：無法取得（演練未提供）。diff 新增了 `#[cfg(test)]` 測試與新公開 API，fmt/clippy/test 是否在 frozen SHA 跑過全套**無收據可採信**。
- **exact-head CI**：無法取得。
- 依 skill 規則：收據缺失 → 全套本地重跑需陳述理由。本輪理由即為「**無 L1 收據、CI 狀態未知**」；真實一輪若仍無收據，應在該輪記錄一次全套重跑。
- 紅燈分類：無從分類（未見 CI）。

### Cost
- 工具呼叫：0（純文字演練）。耗時：約數分鐘閱讀與分析。token：本輪審查全文。

### P0
1. **`crates/edda-conductor/src/receipt.rs:44-47` — receipt 寫入失敗被靜默吞掉，回報成功卻回傳不存在的路徑**。`fs::write` 失敗時只記 `tracing::debug!`（debug 級別＝預設不可見）就回 `Ok(path)`。情境：磁碟滿／權限問題時，收據檔不存在，但呼叫端拿到 `Ok` 與路徑，事後審計會以為該輪有收據。AGENTS.md 明言「Worker receipts are execution evidence, not acceptance」——收據是本 repo 的承重證據鏈，這是**靜默資料遺失**，且讓「無收據」的失敗訊號消失。若原意是「收據目錄不可寫時不擋 spawn」，至少應升級 log 級別、或在回傳值/型別上表達「未寫入」（例如回傳 `Option<PathBuf>`），不能 `Ok` 一個不存在的路徑。

2. **`crates/edda-conductor/src/agent/spawn_config.rs` — `model` 欄位加了但完全沒接上 `spawn_command()`**。`SpawnConfig` 新增 `model: Option<String>`、提供 `with_model()` builder，但 `spawn_command()` 組 argv 時完全忽略 `self.model`——沒有對應的 `--model` 旗標。情境：呼叫端 `with_model("z-ai/glm-5.3-flash")` 後 spawn，子行程根本收不到 model，功能靜默無效。測試只驗 setter 有設欄位，驗的是「欄位存在」而非「行為發生」。除非 issue 的 doneWhen 明確只要求「加欄位不加行為」（本輪無 issue body 可證），否則這是 spec 未達／功能未完成；即便 doneWhen 只要求欄位，也應列 P1 質疑其意義。傾向 P0。

### P1
1. **測試缺口**：新增的測試只覆蓋 `with_model_sets_field`，未覆蓋 `spawn_command()` 的 argv 組裝（含 heartbeat 條件路徑）。若 P0-2 修復（把 model 接進 argv），必須補一個斷言 argv 含 `--model <value>`（且 `None` 時不含）的測試。
2. **`receipt.rs` 變數改名 `dir` → `receipts_dir`** 與行為修復混在同一個 hunk 的語義無關變更，增加 review 噪音；建議分離 commit（不擋 merge，但請下輪分開）。

### Minor
- `spawn_command` 的 doc comment 說明 `--heartbeat` 條件，修復 P0-2 時請一併更新 doc 說明 `--model` 的條件（`Some` 才傳）。
- `format!("{}", self.budget_usd)` 可考慮 `self.budget_usd.to_string()`（風格，不擋）。

### Verdict：**Changes Requested** — receipt 寫入失敗被靜默吞掉並回傳假成功（P0-1），且新增的 `model` 欄位未接進 spawn argv、功能靜默無效（P0-2）；另本輪無 L1 收據與 exact-head CI 可 READ，真實一輪須補齊驗證紀錄。

---
**閘後動作（依 skill，演練中僅聲明、不執行）**：貼此裁定回 PR → PR 留開 → 停，回報操作者。修復屬 `fleet-worker`／後續 pass；我不修碼、不 merge（四禁）。
