Warning: No project session found with id 'microtest-variant-20260902-151034-1'; creating a new session with that id.
## Code Review — PR #（演練：未附號碼）（Round 1）
*獨立審查 · GATE-01（fork，非作者）· 驗證階梯：READ receipt ＋ exact-head CI*

### RAN
無。本輪為純文字審查演練，未執行任何工具（無 `gh pr checkout`、無 `cargo test`、無 `scripts/wiring-scan.sh`）。wiring-scan 未跑，新面盤點為人工比對 diff 所得。若為真實審查，本輪必須補：edda-conductor 是否在 CI Windows 7-crate 子集內的確認；不在則 `cargo test -p edda-conductor` focused 檢查。

### READ
- **L1 gate receipt**：未提供 → 無法採信，視同缺失。
- **exact-head CI**：未提供（演練無 gh 存取）→ 紅/綠未知，視同未驗。
- **issue body（doneWhen 六欄）**：未附於本次審查素材。skill 規定只信操作者簽過的 issue body——缺它則 spec 合規無法逐條對照，這本身即構成不可判 LGTM 的程序缺口。以下裁定僅基於 diff 內部一致性與 repo 慣例（AGENTS.md/CLAUDE.md 摘要：錯誤處理、review 紀律）。

### Cost
純文字演練：0 次工具呼叫、0 次 cargo/gh 執行。真實審查需另計。

### P0
1. **`with_model` 是靜默 no-op——旗標從未到達 spawn 層**（`crates/edda-conductor/src/agent/spawn_config.rs`，`spawn_command()`）。新欄位 `model` 與 builder `with_model()` 只把字串存進 struct，`spawn_command()` 建構 `Command` 時完全沒有讀取 `model`、未輸出 `--model` argv。失敗情境：呼叫者 `SpawnConfig::default().with_model("z-ai/glm-5.3-flash")` 後 spawn 的 agent 仍以預設模型執行，無任何錯誤、警告或輸出——功能整條斷在 spawn 層。唯一新增測試只斷言「欄位被設定」，恰好掩蓋「旗標未出現在命令列」，屬 doneWhen 型的 reach 無證明（wiring 表判定詳下）。

2. **receipt 寫入失敗被吞，且仍回傳不存在的路徑**（`crates/edda-conductor/src/receipt.rs`，`write_round_receipt`）。`fs::write` 的 `Err` 降為 `tracing::debug!` 後函式照樣 `Ok(path)`——回傳一個指向**未寫成檔案**的 `PathBuf`。失敗情境：磁碟滿／權限問題時，round receipt 消失，呼叫者與下游依賴 receipt 的報表／決策路徑把「回傳的路徑」當成功憑證，資料遺失完全不可見（success-only、無 freshness 訊號）。這是 ledger/coordination 路徑上的吞錯（wiring 規則下限即 P1），且相對既有行為（錯誤會 propagate）是**正確性回歸**，故升 P0。`debug!` 層級對資料遺失事件亦過低。

### P1
1. `SpawnConfig.model` / `with_model()` 在產品碼內 **no consumer**（唯一讀者是測試），diff 內無具名後續 issue → 依 wiring 判定規則為 dead on arrival，併入 P0-1 修復（在 `spawn_command()` 輸出 `--model`）。

### Wiring
| 新面 | Writer & shape | Reader（本 PR 內或既有） | Failure signal | Layer reach |
|---|---|---|---|---|
| `SpawnConfig.model: Option<String>`（新 pub 欄位） | `Default`（None）＋ `with_model()` 寫入；spawn_config.rs | 本 PR 內僅測試 `with_model_sets_field` 讀回；`spawn_command()` 未讀 → **產品碼 no consumer** | 無——存入無效模型字串也不會有任何錯誤或輸出 | 旗標→builder ✓ → **spawn ✗**：`spawn_command()` 未輸出 `--model`，鏈在 spawn 層斷裂（P0-1） |
| `SpawnConfig::with_model()`（新 pub fn） | builder，吃 `impl Into<String>` | 同上，僅測試 | 無——呼叫後無法得知旗標是否會生效 | 未到達 spawn 層（併入 P0-1） |

receipt.rs 無新面（既有 pub fn 內部行為變更；吞錯已列 P0-2）。

### Minor
- `dir` → `receipts_dir` 改名與 doc comment 措辭調整：無行為影響，可留。
- **FOLLOW-UP ISSUE**：新增測試應斷言 `model` 旗標實際出現在 `spawn_command()` 產生的 `Command` argv（read-back 斷言），避免同類斷鏈再犯；doneWhen 是否要求未知（issue 未附），故列 follow-up 而非本輪擋項。
- **程序事項**：本輪缺 issue body 與 exact-head CI，真實運行時需補齊後重審。

### Verdict：Changes Requested — 模型旗標靜默 no-op（spawn 層斷鏈），且 receipt 寫入失敗被吞、回傳指向不存在檔案的路徑；兩者皆為正確性缺失，未達 LGTM。

（依 skill：comment 貼回 PR、PR 留開、不加 label、不修不 merge——修是 `fleet-worker` 的事。停。）
