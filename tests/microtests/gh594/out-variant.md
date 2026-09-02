Warning: No project session found with id 'microtest-variant-20260902-142248'; creating a new session with that id.
## Code Review — PR #（文字演練輪 · Round 1）
*獨立審查 · GATE-01（fork，非作者）· 驗證階梯：READ receipt ＋ exact-head CI*

> 演練前提聲明：本輪為純文字審查，未取得 issue body（六欄 doneWhen）、L1 receipt、`gh pr checks` 輸出，也無法 checkout。因此 spec 合規僅能就 diff 本身與 wiring 判定規則裁定；凡因缺少 issue 基準而無法定案者，依規則「不確定就當 P1 提出」。

### RAN
- 無實際指令執行（演練禁用工具）。`sh scripts/wiring-scan.sh <base> <head>` 未跑；本裁定以人工 grep 等價檢視 diff 新增行取代，新增行命中吞錯樣式：`let _ = `（1 處）、`best-effort` 註解（1 處）。

### READ
- 未提供 L1 gate receipt（frozen SHA 的 fmt/clippy/test ＋完整 SHA 紀錄）→ **不可採信**，依規則需陳述理由：收據缺失即不可信，正常流程下此處即構成全套本地重跑的事由。
- 未提供 exact-head CI 結論 → 無法確認本 SHA 為綠；無法確認 Windows 測試子集（7 crates）是否涵蓋 `edda-conductor`。
- 對本 diff 的涵蓋缺口：`with_cost_weighting` 是否能編譯（struct 是否已有該欄位）完全依賴 receipt/CI 證明，本輪兩者皆缺 → 列為 P1 待證。

### Cost
- 文字審查一輪：0 次工具呼叫、無 Cargo 建置、無 CI 呼叫。

### P0
- 無（無法在缺 receipt/CI/issue 的前提下證明正確性破損到 P0 級；相關疑點降 P1 列出）。

### P1
1. **寫入吞錯（best-effort 化）**：`report_digest.rs` `build()` 內 `let _ = fs::write(&self.digest_path, &rendered); // best-effort`。原碼以 `?` + `context("write digest")` 回報失敗，改成靜默吞錯後，digest 寫檔失敗（磁碟滿、權限、路徑不存在）仍回傳 `Ok(Digest { path })`，且 `path` 指向一個不存在/過期的檔案。digest 是報表/成本路徑，命中判定規則「在 ledger / coordination / cost 路徑上吞錯（`let _ =`、best-effort、只記成功）→ **P1**」，同時命中「新增寫端而輸出無 freshness/coverage 訊號，且該路徑有報表或決策依賴 → **P1**（death visibility）」。
2. **`with_cost_weighting` 疑似 dead on arrival**：新 `pub fn` 設定 `self.cost_weighting`，但 diff 內無任何讀取端——`render_markdown` / `collect` 均未使用 `cost_weighting`（diff 未顯示，且 diff 也未包含 struct 欄位宣告的新增）。「no consumer」且未見具名後續 issue → **P1**。若 issue 有列後續 issue 編號，可改列 FOLLOW-UP ISSUE 放行——請作者在回覆中引用該編號。
3. **編譯完整性未證明**：diff 只改 impl 區塊，未見 `DigestBuilder` struct 新增 `cost_weighting: bool` 欄位。若欄位本就存在，此條撤銷；若不存在，本 PR 無法編譯，L1 receipt 應已抓到——但 receipt 缺失（見 READ），故列 P1 要求以 receipt 或 focused `cargo check -p edda-conductor` 證明。
4. **測試缺位**：`with_cost_weighting` 與新的寫入行為均無對應測試新增於 diff。若 issue doneWhen 要求行為到達某層而無測試證明 → 本應 P1；因 issue body 缺失，保守列 P1 待作者對照 doneWhen 抗辯。

### Wiring
| 新面 | Writer & shape | Reader | Failure signal | Layer reach |
|---|---|---|---|---|
| `pub fn with_cost_weighting(bool)` → `DigestBuilder` | `report_digest.rs` `build()` 前的 builder setter；shape：`bool`，存入 `cost_weighting` 欄位（欄位宣告不在 diff 內） | **no consumer**（diff 內 `collect`/`render_markdown` 均未讀取） | 不適用（setter 本身無錯誤路徑） | 旗標→builder→**斷鏈**：欄位無下游，未到達 render/collect 層 |
| digest 寫出行為改變（`fs::write` 結果） | `report_digest.rs` `build()`；寫 `self.digest_path`，shape：markdown 全文覆寫 | 既有：`Digest.path` 回傳給呼叫端（既有 read-back 面不在 diff 內） | **吞錯／success-only**：`let _ =`＋best-effort，寫失敗仍回 `Ok` | 欄位→store（fs）→read-back **失去失敗可見性** |

補充：本 diff 無 CLI 旗標／config 鍵／事件 payload／side-file 等其他新面。

### Minor
- `// best-effort` 註解本身不構成設計理由；若確要 best-effort，至少應以 `tracing::warn!` 記錄寫入失敗並在 `Digest` 帶 freshness 訊號（如 `written: bool` 或時間戳）。
- builder 命名 `with_cost_weighting` 暗示加權行為，但目前無行為差異；命名與實作不符會誤導呼叫端。

### Verdict：Changes Requested — 寫入路徑由傳播錯誤改為 best-effort 吞錯（cost/report 路徑，P1-1/P1-2 條款命中），且新 API `with_cost_weighting` 無 consumer、無測試、欄位宣告未隨 diff 提供證明。

PR 留開、不 merge、不修——修為 `fleet-worker`／後續 pass 之事。停，回報操作者。
