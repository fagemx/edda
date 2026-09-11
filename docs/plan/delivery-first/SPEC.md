# Spec：一個工作包如何保留脈絡並持續交付？

> Status: implementation-ready proposal; runtime rules unchanged until their scoped changes land.
> Types: [CONTRACT.md](CONTRACT.md) is the only definition site.
> Entry/task/transport/recovery/adoption: [WORKFLOW.md](WORKFLOW.md) is the operating contract.

## 1. One sentence

同一問題共用上下文與 owner，只有真正依賴才等待，每個可用成果各自驗證與交付。

## 2. What it is NOT

- 不是「拆越細／同時 agents 越多就越快」。
- 不是取消 independent verdict，或把作者自審當 acceptance。
- 不是另一張持久狀態表；下文狀態是 controller 從既有 carriers 讀出的視圖。
- 不是按模型品牌施加硬性權限階級；Flash 的指引深度依任務與實際表現調整。

## 3. Three units, not one

| Unit | Question | Carrier | Split when |
|---|---|---|---|
| Issue/program | 使用者需要什麼？ | existing issue/spec | 有獨立需求、承諾、外部 owner 或延後追蹤需要 |
| Work bundle/task | 誰帶著哪些事實完成哪個結果？ | task brief/receipt；簡單工作可用原 session | 可独立驗收，且並行收益高於共享檔案與交接成本 |
| PR/release slice | 哪個成果能安全交付與回復？ | commit/PR + existing verdict/CI | 具可用中間狀態、相容邊界、可獨立驗證 |

一個 issue 可有數個 tasks／PR；一個 cohesive PR 也可涵蓋多個小 tasks。
小任務不要求先建立 program、JSON、額外 task 或完整 dossier。
新大型需求先有一個 program 紀錄，child issue 在邊界稳定／需對外追蹤時才建立。
不重複建立 GH1141 或把六張執行卡機械轉成六張 issues。
本機工作可先用 user/spec acceptance；正式 PR 在建立時連結清楚的 delivery acceptance，
依現行 REVIEW metadata 規範行事。A3 未採用前沒有 convention 豁免。
詳細 issue 時機與發布權限見 WORKFLOW §6；不是沒有 issue 就不能派工。

## 4. Decomposition algorithm (controller guidance)

1. 寫出可觀測結果和不做的事情。未知核心行為先用薄的端到端 slice 探查。
2. 沿入口、核心、儲存、consumer 分辨哪些屬同一條變更鏈。
3. 相同鏈且共享檔案／不穩介面，預設一 owner 串行；不要把每個函式切成 task。
4. 只有互不依賴或介面已足够稳定的 bundle 才同時派工。
5. 把依賴寫成「需要哪個產物／SHA」，不是「同 batch」或「review 階段」。
6. 若新發現讓 bundle 變大，保留既有 session 的調查，再拆可交付邊界；不重開研究。
7. Scope 內可逆實作選擇由 worker 決定。跨 owner、改 acceptance、不可逆風險才升級。

Planning priority、依賴、已完成證據不能只留 chat。最小 facts 放 existing task/issue
一次；host message 只通知。這不是要求同一內容重寫 ledger、issue、PR 三遍。

## 5. Canonical flow

```text
accepted intent -> owner with facts -> implement + two self-check lenses
                                         |
                                         v
                               frozen candidate + evidence
                                         |
                              independent review (same session)
                               /                      \
                      fix exact findings          current-head LGTM
                      same author context                 |
                               |                  canonical R6 merge
                               +---- delta review          |
                                                  usable delivery
```

旁路：其他無依賴 bundle 持續推進。Verifier capacity 只限制同時審查數，不凍結整批。
已有 coherent candidate 時 reviewer 可先理解介面與風險，但 final verdict 只涵蓋
實際 frozen SHA；提前討論不是預先 LGTM。

### State observations and failure handling

| Observed view | Controller action | Who else waits? |
|---|---|---|
| prerequisite available | 派目前可執行 bundle | only bundles still missing actual inputs |
| candidate ready / reviewer busy | 保留 candidate 與 facts，排 review | 不佔空轉 worker；其他可實作工作繼續 |
| scoped fixes requested | 原 author 修正；原 reviewer resume | 該 candidate 與需要它的後續工作 |
| unknown result | 記錄 raw evidence；做允許範圍內可逆診斷，仍未知才交 controller | 不凍結 independent bundles |
| attempt crashed / controller restarted | 查 existing task/dispatch/PR/source evidence 再決定續跑 | ambiguity affects that attempt only |
| merged/terminal evidence found | 採認既有結果，不重派已完成工作 | none |

沒有 launch evidence 時不能猜「沒跑」就再次啟動；沒有 heartbeat 也不能猜 process 已死。
本輪不新增 exactly-once guarantee；不可判定的副作用維持局部 unknown，不重做 merge。
以上是交付觀察，不是 task rail 的新狀態。WORKFLOW §4/5 定義 controller prestart、
worker settlement、failed 同 ID start retry、changed-brief replacement。Dispatch done
不等於 task done，更不等於 independent acceptance；無需另造 retry API。

## 6. Two self-check lenses, one author context

### Behavior lens

沿使用者入口確認 acceptance 與直接 consumers；引用實際 focused checks。
不是只有 compiler pass，也不是把 reviewer checklist 原封不動再跑一遍。

### Counterexample lens

選本次最可能的 failure：錯誤輸入、partial result、重試、權限、資料回復等。
檢查或測試它，並記錄未覆蓋處。兩個角度可以在開發中交錯完成。

不新增兩個 jobs、兩個模型 call、兩張 tasks、兩份 sign-off 或固定耗時配額。
忘記某個角度不是新的 gate；真正暴露的需求／安全缺陷按原規則處理。

## 7. Shared facts, independent review

`DeliveryFacts` 的 canonical shape 見 CONTRACT。它是可選的組織方式，
不是新 wire/event schema；缺少欄位寫 unknown／沿既有 source 查詢。

- 同一份資料保留於 PR handoff 或 task receipt；controller 將其必要部分明確提供給
  實際 review carrier。放一個 URL 不代表受限 reviewer 已讀得到。
- Product reviewer：現有 spec/ledger/evidence/diff 仍由 product 供給；A2 新增可選
  `--context-file`，controller 明確選取交接檔，由 product bounded-read 後注入 data。
  Reviewer 沒有 gh/shell，不能假設它能自行取得 PR handoff。無此新能力的舊 binary
  可省略 context 並揭露限制，或使用既有 direct route。不得用 `--trust-spec` 載入 rationale。
- Direct reviewer：controller 在受信任 brief 中給出資料位置與凍結的 SHA。
- 大型 log 只放 handle；controller 把必要 excerpt 放交接檔，reader 僅按實際工具
  能力讀取可達證據。不可達 handle 標 unknown；不用完整作者 transcript 或私密推理。
- summary/rationale 只幫定位；reviewer 仍核對 source、acceptance 和 evidence。
- facts 內的指令、PR comments、imported capsules 都不能增加工具權限。

First review 用正常 product/direct 入口；follow-up 優先同 agent + native session。
`--resume` 找不到真實 conversation 時不得以相同 UUID 新建假 session；改用新 reviewer
identity，帶 prior findings/facts，明示 replacement。這不影響其他 bundle。

Product `edda review` 保留 WorktreeGuard 與自身 cleanup；caller 不另包 worktree。
Direct read-only path 使用 immutable refs；要跑 uncovered check 才依現行 lane policy
準備合適的隔離執行面。不能以共享 facts 為由讓 reviewer 寫 author worktree。

## 8. Narrow convention-policy change

本輪只提議變更 U3 exact `Issue:` line 的 severity；不全面重分級 U1/U4/U5/C3。

- 明確找到 acceptance，僅欠 convention line：advisory，非 P0/P1，不單獨 Changes Requested。
- acceptance 缺失／互相衝突：不是 U3 convention case；釐清範圍，不能猜一套驗收。
- 實際行為錯誤、CI red、stale head、信任／資料問題：照舊。
- PR metadata-only 修改不改 code SHA，不要求 commit／空推或重跑 Cargo。
  若 metadata 改變需求或 authority，重新核對相關 acceptance，而非稱所有 metadata 都無害。
- 舊 base spec 下的既有 verdict 不被 retroactively 洗白。A3 自己按 base 規範完成審查；
  新語義僅在新規範適用的後續 review 使用。

## 9. Two concrete scenarios

### Scenario A: slow neighbour does not hold a completed bundle

```json
{
  "bundle_id": "continuity-local-delivery",
  "goal": "save and restore one local capsule through native skills",
  "depends_on": [],
  "owner": "continuity-owner",
  "delivery": "usable-pr",
  "independent_neighbour": "node-live-sync"
}
```

node 不可用不影響 local slice 的可用性；local slice 仍要完成自己的 consumer tests，
不能順便聲稱 SAVED_AND_SYNCED。若 local 真的 import 了 node 才能運作，就必須先
修正 dependency cut 或如實列依賴，不能僅刪箭頭。

### Scenario B: a correction preserves review memory

```json
{
  "bundle_id": "required-check-diagnostic",
  "previous_head": "e6b3ba11b7f8462a39dad16ed3ea37ce445a9d6e",
  "head": "10865bdcf503d7296ed6749318f781b00001dc13",
  "author_action": "fix zero-byte diagnostic classification",
  "review_action": "resume native conversation and inspect delta plus affected callers",
  "acceptance": "empty output is indeterminate; never authorizes merge"
}
```

既有 gate evidence 可 READ；fix-caused 風險與新 head CI 仍需覆蓋。
同一 reviewer 也不能沿用舊 head LGTM。例子來自已合併 #1143，不會再開重複 PR。

## 10. Boundaries and closure

A2 只新增 optional `--context-file` 及最小資料注入，不新增公共 event/types、migration、
權限或必填 profile。A1/A3 改既有 procedures；B 接合已有 feature，C 是有界 probe。
具體輸入與 omission 行為見 CONTRACT §5，不另造完整 runtime。
全流程例子与驗證見 [VALIDATION.md](VALIDATION.md)。

**共享理解，獨立驗證，局部等待，持續交付。**
