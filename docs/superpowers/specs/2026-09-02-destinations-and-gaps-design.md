# 三個終點與 gap：企業使用、開源、AI 代理基礎設施

- 日期：2026-09-02
- 狀態：操作者採納（Tim：「對 開出來」）。帳本 `product.direction=infrastructure-core-oss-channel-enterprise-monetization`。
- 姊妹篇：[控制層與 L2 形狀](2026-09-02-control-layer-and-l2-shapes-design.md)、[策略五參數](2026-09-02-strategy-layer-five-parameters-design.md)。
  前兩篇談 edda 內部的層與策略；本篇退一步問：**如果終點是企業使用、十萬星開源、或 AI 代理基礎設施，gap 是什麼。**
- 起點：操作者問「今天要變成企業使用、或十萬星開源專案、或 AI 代理基礎設施，你要統整規劃；gap 是什麼」。

---

## 1. 今天的 edda（2026-09-02 事實）

| 面向 | 事實 |
|---|---|
| 規模 | 34 星、2 fork、v0.3.0、一個維護者＋agent fleet；約 600 張 issue 幾乎全是自填 |
| 授權 | Apache-2.0 / MIT 雙授權 |
| 有 | hash chain 帳本（`verify_chain`）、`SCHEMA_VERSION`、五個 hook bridge、三個 launcher、MCP、HTTP（約 30 條 `/api/*`，有 auth middleware）、`edda sync`（從群組成員拉決策）、CHANGELOG / CONTRIBUTING / issue 模板、brew / installer / 預建 binary、docs（getting-started / guides / reference / architecture） |
| 沒有 | 事件簽章與可驗證的作者身分（README 自述 "identity is not yet cryptographically enforced"；ratify 是慣例）、帳本事件的書面規格（reference 只有 brief-schema / cli / query-performance）、SECURITY.md、任何非作者的使用者證據、七項出口測試（0.5 / 7） |

一句總判：**edda 是「作者自己用得很順的工具」。三個終點共同的門檻是「陌生人能用、別人敢依賴」。**

---

## 2. 共同核心 gap（任一終點都要過）

1. **可靠與可觀察**：七項出口測試（免接力／可觀察／誠實帳／結構安全／吞吐／穩定／自舉）過 0.5 項。別人依賴的前提。已開單（#560 Layer 3 一族）。
2. **身分與授權要是真的**：整個賣點是「可稽核」，但事件沒簽章、作者可偽、ratify 靠紀律。這是唯一動到資料模型的大改，越晚越貴。→ #609。
3. **規格**：帳本格式只存在於 Rust 型別；沒有寫下來的 spec 就不是基礎設施，是應用。→ #608。
4. **陌生人測試與人**：從「我知道怎麼用」到「第一次看到的人 10 分鐘內得到好處」；一個維護者的 bus factor；沒有外部使用者回饋。**不是 issue 能解的，是時間與人。**

---

## 3. 各終點專屬 gap

| 終點 | 它真正需要的 | 今天有 | 缺 |
|---|---|---|---|
| **企業使用** | 多人多機（共享或同步帳本）、SSO / RBAC 強制、retention / redaction（transcript 內有 secret 與 PII）、審計匯出、schema 相容承諾、支援與 SLA、成本看板 | serve 有 auth、`sync` 有雛形、actors / policy / tool_tiers 檔存在但不強制、`redact.rs` 存在、成本讀端待 #582 | 同步模型與衝突語意、政策**強制**點、redaction / retention 政策、匯出格式、版本承諾、L3 看板、多於一個人的支援 |
| **十萬星開源** | 一分鐘 wow、一行安裝、清楚的類別與對比、docs 站與範例、貢獻者路徑、外部 integrations 上架、發佈節奏 | brew / installer / 預建 binary、CONTRIBUTING、README 兩層定位 | **wow demo**（Claude Code 裡決定的事，Codex 開起來就記得——跨工具記憶是最強鉤子）、對記憶類產品的定位對比、docs 站、範例庫、plugin marketplace 上架、社群通路 |
| **AI 代理基礎設施** | 書面規格與相容測試、可嵌入（SDK / MCP 契約）、多代理原語成為協定（claim / task / verdict / heartbeat / finding 的 adapter 契約與 conformance）、跨機同步／聯邦、簽章身分、可靠性工程（靜默死亡＝零）、中立治理 | 五個 bridge 證明 adapter 模式可行、決策 `orchestration.cross-platform` 已把「資料面＝帳本物件、控制面＝薄 adapter」講清楚、MCP / HTTP 存在、schema 有版本號 | spec v1（#608）、conformance（#610）、SDK（#611）、adapter 契約文件（#610）、遠端同步、簽章（#609）、L3（#604 一族）、第二個實作或第二個非 CLI runtime 的 adapter（#610） |

誠實邊界：十萬星是定義品類等級的事件（GitHub 前 0.01%）；現實目標是靠一個銳利鉤子到五千到一萬，之後看品類。星星跟著 wow 走，不跟著 roadmap 走。

---

## 4. 統整：一條路，三段

> **編號正本已移到 [`docs/plan/roadmap.md`](../../plan/roadmap.md)**（一個編號、兩條軌：0 → 1a ∥ 1b → 2 → 3a/3b → 4）。本節的 0–3 是首版編號，對照表見該頁「舊編號對照」。

三個終點不是三選一。**主體選「基礎設施」，開源是通路，企業是變現層**——基礎設施的 gap（規格、簽章、同步）同時是企業的前提；開源的 gap（wow、安裝、定位）是流量不是產品。

| 段 | 內容 | 出口 |
|---|---|---|
| **0 自舉可靠（現在）** | Stage A 的單走完（#574 / #578 / #582 / #584 / #585 / #567 / #569 / #573 / #594 / #599）；七項出口測試 | 7 / 7 連續兩個 wave |
| **1 基礎設施化** | #608 事件規格 v1、#609 簽章身分、#610 adapter 契約＋conformance、#611 SDK；`sync` / `merge` 語意寫清楚；**第二個領域證明**（素材產線／探索場，以乾淨 repo 做——基礎設施要有兩個域才可信） | 規格與簽章定稿；五個 bridge 過 conformance；一個非 CLI adapter；兩個語言的薄 SDK |
| **2 開源通路** | 一分鐘 wow、對比表、docs 站、範例、上架 | 產品已穩，星星不會變成 issue 海嘯淹死一個人 |
| **3 企業** | SSO / RBAC 強制、redaction / retention、匯出、多機同步、支援 | 用第 1 段的簽章與規格當地基，不重做 |

**最大的 gap 不在技術清單裡：一個人。** 企業要支援、開源要社群、基礎設施要治理——三條都需要第二個人類。技術上最貴的一項是簽章身分（動資料模型），最便宜但最缺的是規格與 onboarding（純寫作，可以派 lane）。

---

## 5. 決策指標

- `product.direction=infrastructure-core-oss-channel-enterprise-monetization`（Tim 2026-09-02）
- 相關：`product.layer3=control-layer`、`product.l2-next-shape=research`、`product.strategy=data-not-code-five-parameters`、`orchestration.cross-platform=ledger-data-plane-adapter-control-plane`
- 開單：#608、#609、#610、#611（第 1 段）；#601–#604（Layer 3）；#560（第 0 段）
