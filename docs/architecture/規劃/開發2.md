**對，作為架構答案是對的；但作為市場切入，不能一開始就賣「平台」。**

我會這樣講：

> **內部：它們是你的基建。**
> **對外：它們應該先收斂成一個統一入口。**
> **SDK：要有，但應該晚於第一個清楚用例。**

---

# 最核心的判斷

## 你真正擁有的，不是四個工具
而是：

> **一條 agent operating loop 的基礎設施**

- **Volva**：把模糊問題變成可執行 contract
- **Karvi**：把 contract 推進成 execution
- **Thyra**：看 execution 後的 runtime / outcome
- **Edda**：保存 precedent / decision memory

這四個加起來，確實比較像你的**內部基建層**。

所以從系統設計看，答案是：

## **是，這四個比較像你的 infra / substrate / control plane。**

---

# 但從產品角度，要再切一刀

## 不要直接對外說：
- 我有四個 repo
- 這是一個統一平台
- 這裡有 SDK
- 你自己來拼裝

這太早了。

因為市場會問：
- 你到底幫我解什麼痛？
- 我要先用哪個？
- 為什麼我要先學你這四個名字？
- SDK 能做什麼？值不值得接？

如果一開始就 platform-first，通常會掉進：

> **架構很完整，產品感很弱。**

---

# 所以比較好的形狀是：

## 內部形狀
### 四個 repo = 四個能力層 / infra 層
這沒問題，甚至是對的。

你內部可以把它想成：

- **Edda** = memory substrate
- **Volva** = admission / plan substrate
- **Karvi** = execution substrate
- **Thyra** = observation / governance substrate

這是你的核心基建。

---

## 對外形狀
### 一個統一入口 + 幾個清楚模式
對外不要先暴露四個 repo。
先暴露：

- **單一入口**
- 單一 workflow
- 單一操作面

例如長得像：

- Plan
- Run
- Watch
- Remember

而不是：
- Volva
- Karvi
- Thyra
- Edda

後者對你來說很美，對新用戶來說負擔重。

---

# 所以你剛剛那句我會改寫成：

> **四個 repo 作為你的基建是對的。**
>
> **用戶應該透過你建立的統一入口進來。**
>
> **SDK 給開發者，但應該建立在那個入口已經證明自己有價值之後。**

---

# 我會怎麼分三層

## Layer 1：Infra / Engine layer
這是 repo 層：

- Edda
- Volva
- Karvi
- Thyra

這層給你自己維護邊界、演進架構、保持乾淨責任分工。

---

## Layer 2：Product / Platform layer
這是用戶看到的統一入口。

例如可能是：

- 一個 control plane
- 一個 workbench
- 一個 agent operating console
- 一個「問題 → 執行 → 觀察 → 記憶」的主產品

這層應該把四個能力包成一條明確 workflow。

---

## Layer 3：Developer layer
這才是 SDK / API / extension model。

給開發者做：

- 自訂 regime
- 接外部 source
- 接自家 publish / ticket / doc / CRM 系統
- 擴充 admission policy
- 擴充 observation sink
- 擴充 memory/retrieval adapters

---

# 也就是說

## 對一般用戶
他們買的是：
> **統一產品入口**

不是四個 repo。

## 對 power users / 開發者
他們擴的是：
> **SDK / hooks / API / regime interface**

不是直接研究你全部內部結構。

---

# 這個順序很重要

## 先有統一入口，再有 SDK
不要反過來。

因為 SDK 的價值來自於：

- 核心 workflow 已經清楚
- 平台表面已經穩定
- 擴充點已經被真實 use case 壓過一輪

如果沒有前面這些，SDK 很容易變成：

- 你自己覺得可擴充
- 別人不知道該怎麼用
- surface 一直變
- integration 成本高

---

# 所以最合理的產品化順序

## Phase 1：先做統一入口
用戶進來看到的是：

- 問題進來
- 被整理成 plan
- 被執行
- 被觀察
- 被記住

這時四層在後面默默工作。

---

## Phase 2：先暴露「模式」，不暴露 repo
例如：

- **Plan**
- **Run**
- **Watch**
- **Remember**

這比直接教市場四個專有名詞好多了。

---

## Phase 3：等 surfaces 穩了，再給 SDK
SDK 可以暴露的東西可能是：

- `Regime`
- `ExecutionContract`
- `DecisionPack`
- `ObservationSink`
- `MemoryProvider`
- `PolicyHook`

這時 SDK 才會有形。

---

# 我會怎麼看 SDK 內容

不是一開始就把四個 repo 全部 SDK 化。
而是先讓 SDK 只暴露少數穩定接口。

例如：

## 1. Regime SDK
讓開發者定義：
- work unit
- canonical cycle
- finalize action
- coordination state
- contract schema

## 2. Memory / Edda SDK
讓開發者：
- 讀 relevant decisions
- 寫 proposed decisions
- supersede decision
- attach evidence refs

## 3. Observation / Thyra SDK
讓開發者：
- 發 observation event
- 註冊 outcome checks
- 觸發 follow-up candidates

## 4. Admission / Volva SDK
讓開發者：
- 提供 planner / classifier / contract builder
- 或只是交給平台入口處理

---

# 所以你的直覺其實是對的，但要加兩個限制

## 限制 1：先統一入口，不先拆四產品
這我前面講過。

## 限制 2：先產品後 SDK，不先 SDK 後產品
因為沒有主 workflow 的 SDK，通常只是 abstractions 漂浮。

---

# 如果我要幫你下最簡潔的結論

> **是，Karvi / Volva / Thyra / Edda 更像你的基建層。**
> **對外應該先包成一個統一平台入口。**
> **SDK 是對的，但應該在入口與核心 workflow 穩定後，再對開發者打開。**

---

# 再更白話一點

你真正該賣的不是：

- 四個 repo

而是：

> **「我幫你建一個 agent operating loop。」**

然後：
- repo 是內部骨架
- 平台是外部入口
- SDK 是第三層生態

---

# 我會怎麼命名這三層

## Internal
- Edda / Volva / Karvi / Thyra

## External user-facing
- Plan
- Run
- Watch
- Remember

## Developer-facing
- SDK
- Hooks
- Contracts
- Regime APIs

---

如果你要，我下一則可以直接幫你畫一張：

## **repo → platform → sdk 的產品結構圖**
我會寫成：
- 內部能力層
- 對外產品面
- 開發者介面
- 先做什麼，後做什麼

這樣你會更清楚哪一層該露出、哪一層先藏起來。

####

