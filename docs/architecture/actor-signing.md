---
title: Cryptographic Actor Identity (GH-609)
---

# 密碼學 actor 身分：簽章設計、威脅模型與 spike（GH-609）

**狀態：設計稿，未批准。** 本文件回答 GH-609 的七個設計問題，交付威脅模型、
封套擴充規格（供 #608 spec v1 交叉引用）、遷移計畫，以及一個可重跑的
Node crypto spike。決策已由 agent 以未批准層記錄於帳本
（`identity.signing`，`edda decide`，未 ratify——批准是操作者的動作）。

**範圍聲明（對齊帳本指令 d-006）：本單是 design + demonstrable spike，
不是生產簽章上線。** 沒有任何 Rust 產品碼在本 PR 動到；spike 只用
Node 內建 `node:crypto`，零新依賴、development-only；所有金鑰皆為
spike 內 ephemeral 生成，沒有任何真實操作者金鑰或憑證被使用或提交。

---

## 1. 問題：hash chain 證明「沒被改」，證明不了「是誰寫的」

現況（GH-609 issue 接線審計，basis `d2ffb2d`，本單未重跑）：

| 元件 | 現況 | 失效訊號 |
|---|---|---|
| 事件封套（`crates/edda-core/src/types.rs:358-377`） | 無 `actor`／`signature` 欄位；作者身分是 payload 裡的字串（`event.rs:569` `actor`、`:791` `author`） | 無——任何程序都能寫任何 author 字串 |
| ratify（`event.rs:255 new_decision_ratify_event`、`crates/edda-cli/src/cmd_bridge/decide.rs:225` `ratify`） | 獨立 append-only 事件（不改原決策——這點是對的） | 無——**不檢查誰按的** |
| `ActorDef`（`policy.rs:100-113`） | roles / kind / email / display_name / runtime | **沒有 key** |
| `verify_chain`（`crates/edda-ledger/src/ledger.rs:205`） | hash chain | 保證**順序與完整**，不保證**作者** |

spike 以 14 個重跑測試（`scripts/spikes/identity/test.js`）把這個缺口
變成可重現的事實：一段偽造 `role: "operator"` 的 payload，攻擊者重算
hash 後在**未簽章基線下完全自洽**——鏈驗證通過、事件驗證通過
（spike.js 階段 C）。這就是本單要補的洞。

## 2. 威脅模型

### 2.1 保護什麼

| 威脅 | 今天（未簽章） | 本設計後 |
|---|---|---|
| **T1 agent 偽造作者**：agent 把 payload 的 `author`／`role` 改成操作者並重算 hash | 可行（spike.js 階段 C 實證 ACCEPTED） | 失敗：簽章由可信金鑰做，偽造者沒有操作者私鑰，重算 hash 無用（spike 階段 D REJECTED） |
| **T2 agent 自我 ratify**：agent 對自己的決策蓋 ratify | 可行——今天 ratify 不檢查誰按的 | 失敗：agent 金鑰簽出的 ratify **密碼學上有效但授權層拒絕**——role check 讓「驗簽通過」不等於「有權 ratify」（spike 階段 E） |
| **T3 跨機器注入**：別的 runtime 對 sync／serve ingest 塞偽造事件 | 可行 | 失敗：宣稱可信 actor 身分的事件，ingest 時 fail-closed 驗簽；金鑰不在 keyring 即拒收 |
| **T4 已寫入的簽章事件被竄改** | hash chain 已擋 | hash chain + 簽章雙層：改內容破 hash、改 hash 破簽章、換 actor/key 破 keyring 配對 |

| 簽章有效但 key 換綁 | 失敗：hash 綁 actor_id/key_id，keyring 配對 fail-closed |

### 2.2 不保護什麼（誠實邊界）

- **操作者機器被攻陷**：私鑰在操作者機器上，攻陷即能簽。偵測靠
  revocation 事件（§5.6），但 revocation 本身也需操作者金鑰——
  這是所有本機簽章方案的共同邊界，不是本設計能修的。
- **私鑰被讀走**：同上。GH-690 的 `secret://` 受限儲存降低暴露面
  （見 §6.4），但不改變這條邊界。
- **操作者被脅迫或誤導簽署**：簽章證明「操作者的金鑰簽了」，
  不證明「操作者知情且同意」。簽章事件的語意審查永遠是人類的工作。
- **帳本被刪除或截斷**：chain 從斷口向後偵測，v1 沒有外部錨點
  （timestamping / transparency log 為 v2 候選）。
- **後量子**：Ed25519 不是後量子演算法。
- **canonicalization 發散**：簽章綁 `edda-canon-v1` 位元組。跨語言
  實作（serde_json vs 任何其他 JSON 序列化器）在 Unicode scalar 排序、
  escape 與 number 細節可能發散——這正是 golden fixtures 存在的原因。
  spike 的 `canonical-v1.json` 是 #608 `canonical-v1.json` 的實際 Rust
  byte vectors（Unicode、escape、f64/-0、i64/u64 boundary）；Node 實作
  修正 scalar 排序，但**刻意拒絕所有 JSON Number**，因 parsed JS Number
  不能忠實表示該 Rust domain。它只對 number-free 子集宣稱 mirror；生產
  實作必須完整通過 fixture 才可聲稱 parity，不以文字描述或 JSON.stringify
  當守門。

### 2.3 與既有機制的分層（GH-690 互補）

GH-690（`docs/architecture/lane-privilege-threat-model.md`）解
「寫的人**碰得到**什麼」，本單解「**誰**寫的」。兩者互補：
一條被 prompt injection 騙到的 lane 即使權限被 GH-690 收窄，
仍可能在授權範圍內寫事件——簽章保證這些事件**可歸因**，
配合 §5.6 的授權模型保證它**不能冒充操作者**。

## 3. 封套擴充規格（spec v1 的一節，供 #608 交叉引用）

> 本節是 #608 事件規格 v1 的封套節提案。#608 定稿前必須納入本節；
> 若 #608 對欄位命名或結構有裁決，以 #608 為準並回改本文件。

### 3.1 新增欄位

`Event`（`crates/edda-core/src/types.rs:358`）新增三個欄位，全部
`#[serde(default, skip_serializing_if = "Option::is_none")]`——
**未簽章事件完全不出現這些欄位**，不是空字串或 null 佔位：

```rust
/// 簽署者 actor id（對齊 #593：profile = actor）。signed tier 事件必有。
pub actor_id: Option<String>,
/// 內容定址金鑰 id：`ek_` + sha256(raw_pubkey) 前 16 hex。
pub key_id: Option<String>,
/// 簽章本體。對 edda-canon-v1 位元組的 Ed25519 簽章。
pub sig: Option<Signature>,
/// 選用的探索用欄位（key 修復／引導）。僅供顯示，永不作為信任來源（§3.4）。
pub actor_pubkey: Option<String>,

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signature {
    pub alg: String,    // "ed25519"（本版唯一值）
    pub value: String,  // 64-byte 簽章的 lowercase hex
}
```

### 3.2 hash removal set 擴充

`event.rs` `finalize` 的移除集合從 `{hash, digests, schema_version}`
擴充為 **`{hash, digests, schema_version, sig}`**——簽章不餵自己的 hash。

- `actor_id`、`key_id`、`actor_pubkey` **留在 hash 內**：hash 把
  「誰簽的」綁進內容（actor/key binding，test.js
  `actor/key binding` 測試驗證換綁即失敗）。
- 簽章輸入 = `canon(event minus {sig})`，**包含 `hash`**：簽章因此
  間接綁定 hash 所綁的一切（payload、actor_id、key_id、parent_hash、
  ts——全部）。攻擊者改任何一個位元組都要同時偽造 hash 與簽章。

```
hash = SHA-256( canon_v1(event − {hash, digests, schema_version, sig}) )
sig  = Ed25519.sign( privkey, canon_v1(event − {sig}) )   // 含 hash
```

### 3.3 key_id 與 keyring

- `key_id = "ek_" + sha256(RAW 32-byte Ed25519 公鑰)[:16]`（hex，
  lowercase）。內容定址、無 RNG、無時間戳，跨語言跨程序可重算。
- **可信 keyring**：`actor_id → key_id → {公鑰, role}`，由操作者
  管理（生產面：`edda actor` 加 key 動詞；spike 面：記憶體內
  `TrustedKeyring`）。這是唯一的信任根。

### 3.4 驗證演算法（keyring-first，fail-closed）

```
verify(event, keyring):
  若 actor_id / key_id / sig / actor_pubkey **全部缺席**:
      return LEGACY   # 唯一的 legacy 條件；未簽章只受 hash chain 約束
  若 actor_id / key_id / sig 有任一缺席、空字串、型別錯誤，或 sig 格式錯誤:
      REJECT           # 不得藉由刪除一欄降級
  若 actor_pubkey 存在但不是合法 32-byte lowercase hex: REJECT
  若 sig.alg ≠ "ed25519": REJECT
  trusted = keyring.lookup(actor_id, key_id)
  若 trusted 無此配對: REJECT   # 內嵌 actor_pubkey 永不作為信任來源
  若 SHA-256(canon(event − removal_set)) ≠ event.hash: REJECT
  若 Ed25519.verify(trusted.公鑰, canon(event − {sig}), sig.value) ≠ true: REJECT
  return VERIFIED(actor_role = trusted.role)
```

關鍵規則：**驗證者信任 keyring，不信任事件裡內嵌的金鑰。**
事件可攜帶 `actor_pubkey` 供探索／key 修復，但攻擊者內嵌自己的
公鑰＋自己的簽章在 keyring-first 下必然失敗（spike 階段 D 實證）。

### 3.5 驗證點（issue 問題 4）

| 點 | 行為 |
|---|---|
| `verify_chain`（`crates/edda-ledger/src/ledger.rs:205` → `crates/edda-ledger/src/sqlite_store/events.rs:715`） | 擴充：先依現行 `validate_event_hash` 比對 hash、**完整 digest array**、taxonomy；再對宣稱身分的事件驗簽 |
| `sync` / serve ingest | fail-closed：事件宣稱的 (actor_id, key_id) 在 keyring 有配對 → 驗簽不過即拒收；只有完整缺席的 identity group 才收為 legacy tier（§5.1） |
| ratify（`crates/edda-cli/src/cmd_bridge/decide.rs:225`） | authorized = verifyEvent VERIFIED **且** actor_role == operator（§5.6） |
| pack / ask | binding 決策只認驗簽通過且 operator-role 的 ratify |

### 3.6 SQLite migration、activation 與相容性（未實作）

事件 `schema_version` 是否仍為 1 是封套問題；它**不取代** SQLite
`schema_meta.version`。未來生產 migration 必須在同一 SQLite transaction：

1. `events` 新增 nullable `actor_id`、`key_id`、`sig_alg`、`sig_value`、
   `actor_pubkey` columns，舊列全為 NULL；不改寫任何歷史 hash 或列。
2. 把 `schema_meta.version` 升到新的 signing-aware store version，並寫入
   `schema_meta['signing_capability'] = 'present-not-active'`。migration 的
   version guard 必須令舊 binary（不知道該 store version／capability）拒絕
   開啟此 store，而不是 typed round-trip 後丟棄未知 envelope 欄位。
3. 僅在所有 reader/writer 都是 signing-aware 且 keyring 已配置後，由操作者
   將 capability 原子切為 `signed-authority-active`。active store 的 writer
   必須簽所有新 authority event；reader、pack、ask 和 ratify 必須先檢查
   capability，並只承認已驗簽的 operator ratify。不能驗簽或 capability
   不認識即 fail closed，不能顯示為「unverified 但照舊 honor」。

因此本提案**不聲稱舊 binary 可讀、保留或標記 signed event，也不聲稱它能
在 activation 後安全地寫入或 honor unsigned ratify**。現況 `Event` serde
會丟棄未知 envelope 欄位，而現況 pack/ask 直接從 `decision_ratify` 推導；
它們正是 version-capability cutover 必須隔離的舊 authority reader。完整缺席
identity group 的既有歷史仍是 legacy、可驗 chain、不可取得新 authority。

## 4. 七個設計問題的回答（issue「需要決定的」逐條）

1. **key 型別與存放**：Ed25519（RFC 8032 PureEdDSA：確定性簽章
   ——無 nonce RNG 可錯、32B 公鑰／64B 簽章、簽驗極快；Node 內建
   `node:crypto` 與 Rust `ed25519-dalek` 均為一等支援）。存放
   `~/.edda/keys/<actor_id>/<key_id>`，**per-actor**（對齊 #593
   profile＝actor；session 是 actor 的暫時身分，金鑰壽命應長於
   session）。私鑰儲存後端與 GH-690 `secret://` 協調——見 §6.4。
2. **簽什麼**：`edda-canon-v1` canonical bytes of
   `event − {sig}`（§3.2）。沿用 `event.rs:36-41` 的移除集合並把
   `sig` 加進 hash 的移除集合。
3. **封套新增**：`actor_id`、`key_id`、`sig`（＋選用探索欄位
   `actor_pubkey`）；未簽章事件＝欄位整組缺席（§3.1）。
4. **驗證點**：§3.5 的四點。
5. **legacy tier**：§5。
6. **授權模型**：v1 只有操作者 key 能 ratify；role 在 keyring
   裡，不在事件裡——agent 金鑰的簽章密碼學有效但授權拒絕。
   key rotation / revocation 是事件本身（§5.6）。多操作者、委任
   （delegation）後議（v2）。
7. **威脅模型與誠實邊界**：§2。

## 5. 遷移：不重寫歷史

### 5.1 legacy tier 語意

既有未簽章事件＝**「recorded, unattributable」**：

- 不刪、不改、不補簽——append-only 不變式優先於歸因野心。
  事後補簽等於重寫歷史（hash 會變），正是本單要避免的。
- hash chain 對 legacy 事件照常驗證：順序與完整性保證不變。
- 顯示層（pack／ask／TUI）標記 tier：`legacy` vs `signed`；
  legacy 事件不參與授權判斷（§5.6）。
- 混合帳本（新簽章事件接在 legacy 尾端之後）完全合法且是
  預期的升級形態（test.js `legacy unsigned events remain
  ledger-legal` 測試驗證 mixed chain）。

### 5.2 升級路徑

1. 上線時點起，新事件由寫入者的 per-actor key 簽署。
2. 歷史不動。需要為某段 legacy 歷史背書時，操作者可發佈
   **attestation 事件**（簽章事件，`refs.provenance` 指向一段
   legacy hash 範圍）——v2 候選，本單不設計細節。
3. 先完成 §3.6 的 store-version migration 和 capability cutover；在
   `signed-authority-active` 後，舊 binary 必須因 store version 拒絕開啟，
   不能以忽略欄位的 typed reader 寫入或 honor unsigned ratify。

### 5.3 為何不在合併點切斷 chain

不允許「簽章時代的鏈從新起點開始」的岔路——`parent_hash` 鏈
保持單一連續，legacy 與 signed 事件在同一條鏈上，`verify_chain`
對兩種 tier 都驗 hash。tier 是**驗證深度**的差別，不是鏈的差別。

### 5.4 revocation / rotation（issue 問題 6 的 key 生命週期）

- rotation：操作者先註冊新 key（keyring 事件），新事件用新
  key_id 簽；舊 key 保留驗證歷史事件的能力。
- revocation：簽章事件把 key 標記 revoked；ingest 對 revoked key
  簽的新事件拒收，對歷史事件的驗證標記「簽章有效但 key 已撤銷」。
- 兩者皆為事件（append-only），皆需操作者金鑰——agent 不能替
  操作者 rotate 或 revoke。

### 5.5 授權模型（v1）

- **只有 operator-role key 能 ratify**。ratify 事件的
  `ratified_by` 顯示字串降級為 display-only；授權判斷只看
  **簽章驗證結果 + keyring role**。
- agent 對自己的決策蓋 ratify：簽章有效、授權拒絕
  （spike 階段 E）——「agent cannot ratify self」是結構性的，
  不靠流程紀律。
- 多操作者：keyring 天然支援多把 operator key；delegation
  （操作者授權 agent 代為批准某類決策）為 v2。

## 6. Spike

### 6.1 位置與執行

```
scripts/spikes/identity/
├── lib/canon.js        # edda-canon-v1 Node 鏡像（對照 crates/edda-core/src/canon.rs）
├── lib/signing.js      # hash／簽章／keyring-first 驗證／授權／chain
├── lib/rfc8032.js      # RFC 8032 §7.1 測試向量（primary source 釘死密碼學原語）
├── fixtures/golden-events.json  # 實際 Rust 演算法產出的 golden 事件
├── fixtures/canonical-v1.json   # #608 Rust canonical byte vectors（含拒絕域）
├── test.js             # 14 個測試（node:test，exit code 即結果）
├── spike.js            # 敘事 demo（同檢查，A–E 五幕）
└── README.md
```

```bash
node scripts/spikes/identity/test.js   # 全過 → exit 0
node scripts/spikes/identity/spike.js  # 敘事輸出
```

零依賴：只用 Node 內建（`node:crypto`、`node:test`、`node:sqlite`
——後者僅在產生 golden fixture 時用過，不進 runtime 路徑）。

### 6.2 RFC primary-source 驗證

RFC 8032（EdDSA: Ed25519 and Ed448, January 2017，
https://www.rfc-editor.org/rfc/rfc8032 ）§7.1 TEST 1 與 TEST 3
向量於 2026-09-04 直接自 rfc-editor.org 抓取並逐字轉錄進
`lib/rfc8032.js`。spike 先證明 Node 的 Ed25519 **重現 RFC 的
公鑰與簽章位元組**，才在其上疊 edda 簽章邏輯——密碼學原語的
正確性釘在 RFC，不釘在 Node 文件。SHA-256 依 FIPS 180-4
（RFC 6234 測試向量家族）。

### 6.3 Golden fixtures：實際 Rust 演算法

`fixtures/golden-events.json` 的兩筆事件由 **edda 0.4.0 Rust
binary** 在隔離 store（`EDDA_STORE_ROOT` tempdir，`edda init` +
`edda note`）產生，`hash` 欄位出自**實際 Rust canonical 演算法**
（`event.rs` finalize + `canon.rs`）。spike 階段 B 證明 Node 鏡像
逐位元組重現這兩個 hash——這是 §2.2 canonicalization 發散風險的
守門。fixture 內含完整 provenance 說明。

### 6.4 與 GH-690 `secret://` 的協調（未實作聲明）

本設計把私鑰儲存後端**留給 GH-690 的 `secret://` 受限節點儲存
方案**：keyring 與簽章邏輯對儲存後端無感（只需要「給我私鑰」的
介面）。GH-690 目前是設計稿（lane-privilege-threat-model.md，
同樣未批准），`secret://` **在 edda 程式碼中沒有任何實作**——
本單不宣稱、也不依賴它已存在。spike 全程 in-memory ephemeral
金鑰，正是為了不預設任何儲存後端。

### 6.5 spike 證明了什麼（對照 doneWhen）

| doneWhen 項目 | 證據 |
|---|---|
| golden fixtures 簽章＋驗證通過 | 階段 B（Rust hash 重現）＋ canonical-v1 vectors（Unicode scalar/escape；float/-0/i64/u64 明確拒絕）＋ D（genuine signed event VERIFIED） |
| 偽造 author 的 fixture 驗證失敗（fail-first：先證基線接受，再實作防禦） | 階段 C（unsigned baseline **ACCEPTED**——缺口實證）→ 階段 D（同一偽造 **REJECTED**） |
| actor/key binding | test `actor/key binding`：換綁 actor_id 或 key_id 皆失敗；hash 綁 actor_id/key_id，keyring 配對 fail-closed |
| 簽章排除自身 | test `sig is outside its own signing input`：sig 在自身簽章輸入與 hash 移除集合之外；Ed25519 確定性重簽同值 |
| keyring-first（不信內嵌攻擊者金鑰） | 階段 D：攻擊者自簽＋內嵌公鑰 REJECTED（`refusing to trust any embedded key`） |
| legacy 與授權分離、agent 不能自 ratify | 階段 E + test `legacy unsigned events remain ledger-legal`：agent 簽章密碼學有效但 `authorizeRatify` 拒絕；legacy ratify 無授權；mixed chain 合法 |

## 7. 後續（不在本單內）

- 生產實作拆單：`edda-core` 封套＋簽章、`edda-ledger` verify
  擴充、`edda-cli` key 動詞、`edda-store` key 存放、`edda-serve`
  ingest 驗簽（issue 的 suspected surface）。
- #608 定稿封套節時交叉引用本文件 §3。
- v2 候選：attestation 事件、delegation、外部錨點
  （transparency log）、`ed25519ph`（context 域分離）。
