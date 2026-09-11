# Contract：最小交接資料與不可被弱化的界線

> Status: proposed documentation profile, not a new runtime/event schema.
> Purpose: define what a handoff means without creating another admission gate.
> Canonical types live only here; SPEC and task cards reference, not redefine them.

## 1. One sentence

Facts 可以分享，acceptance 不由作者自稱；流程可精簡，副作用與證據不能說謊。

## 2. What it is NOT

不是 `ExecutionBriefV1`、`WorkReceiptV1`、`ControlManifestV1` 的替代品。
不是 requirements parser、JSON gate、required task fields、公共 SDK 或新資料表。
TypeScript 僅表達文件契約，Rust 專案不因此新增 JS runtime 或 schema migration。
一個普通任務不必產生下列 JSON；人可用同義 Markdown 透過既有 carrier 交接。

## 3. Minimal shared types

```typescript
type FullSha = string; // validate actual Git identity as 40 lowercase hex

type Known<T> =
  | { state: "known"; value: T; source: string }
  | { state: "unknown"; reason: string };

type BundlePlan = {
  bundle_id: string; // plan-local ID, not an allocated edda task ID
  goal: string;
  acceptance: string[];
  exclusions: string[];
  owner: string; // role until the controller binds a real session
  depends_on: string[]; // only bundle_ids with actual artifact dependencies
  basis: Known<FullSha>;
  write_paths: string[]; // existing coordination scope; no new enforcement
  issue_refs: string[]; // may be empty for ordinary local work
  delivery: "usable-pr" | "local-evidence" | "owner-amendment";
};

type EvidenceRef = {
  kind: "command" | "ci" | "source" | "finding";
  activity: "ran" | "read";
  subject: Known<FullSha>;
  locator: string; // event ID, CI URL/id, exact file/ref or PR comment URL
  result: "pass" | "fail" | "unverified";
};

type ReviewBinding = {
  transport: string;
  session: Known<string>;
  prior_head: Known<FullSha>;
  mode: "first" | "resume" | "replacement";
  replacement_reason: string | null;
};

type DeliveryFacts = {
  profile: "delivery-facts/0";
  bundle_id: string;
  basis: Known<FullSha>;
  head: Known<FullSha>;
  goal: string;
  acceptance_refs: string[];
  changed_paths: string[];
  direct_consumers: string[];
  rationale: string[]; // concise decisions with provenance, not hidden reasoning
  evidence: EvidenceRef[];
  prior_findings: string[];
  unknowns: string[];
  review: ReviewBinding;
};

type SliceReadiness = {
  slice_id: string;
  candidate: Known<FullSha>;
  usable_outcome: string;
  required_inputs: string[];
  excluded_program_promises: string[];
  owner_adoption: Known<string>; // local coordination record, not merge authority
  delivery: "candidate" | "review-pending" | "merged";
  evidence: EvidenceRef[];
};
```

資料輪廓不包括 `approved`、`merge_allowed` 或 writer 自填的權限旗標。
`FullSha` 型別只是註解約束，不是 TypeScript 自動證明。正式工具仍讀 Git/forge。
`Known.source` 與 `locator` 只是 provenance，可能過期或不可信；reader 核對原始資料。

## 4. Boundary table

這些是本改動的 acceptance，不是新加入所有專案的 gate。

| ID | Contract | Why / consequence | Verification |
|---|---|---|---|
| DF-01 | 不新增 hook/check/label/approval/role gate | 減少程序不能以新增強制程序交換 | diff audit + V1/V2/V4 |
| DF-02 | 無依賴 bundle 不等整批 | 否則最快成果被最慢鄰居拖住 | V1 schedule trace |
| DF-03 | writer 隔離；reader 可用 immutable refs；product guard 保留 | 共享 facts 不能讓 author tree 或 subject SHA 被改寫 | V2 route/subject checks |
| DF-04 | facts/data 不成執行或 verdict authority | 否則作者／不可信內容可誘導工具或自我驗收 | V2 adversarial handoff |
| DF-05 | 自審不代替 final independent current-head verdict/R6 | 避免「做過」誤等於「可以 merge」 | V2/V4 + unchanged merge fixtures |
| DF-06 | READ evidence 綁來源 SHA；unknown 不補零或補綠 | 避免省時間變成虛報覆蓋與成本 | V2/V5 |
| DF-07 | U3-only convention advisory；其他拒絕不變 | 不讓精簡擴張成全面放寬安全與驗收 | V3 negative cases |
| DF-08 | usable slice 自己 closure，不等整 program；不假稱 program done | 防止 early delivery 與承諾失真互換 | V4 release matrix |
| DF-09 | owner scope/SDK consumer/已有嘗試不得繞過 | 防止重複派工、競爭寫入、schema 不相容 | B1/B2 owner receipt + V4 |
| DF-10 | 受支援入口共用一份 generic flow；投影／custom／採用狀態明確 | 否則每個 host 或舊入口繼續另定程序 | WORKFLOW §1/2/7 + V6 |
| DF-11 | task/dispatch lifecycle owner 固定；重試與 replacement 區分 | 防止雙 start、假 done、重複啟動與失敗依賴永久阻塞 | WORKFLOW §3–5 + V6 |

V1–V6 的執行步驟見 [VALIDATION.md](VALIDATION.md)。
缺 facts profile 欄位本身不是拒絕一般開發的理由；若缺的是 merge 所需 head/authority，
阻止的是該 merge 動作，來源是既有安全條件，不是這份新文件。

## 5. Existing surface mapping

[WORKFLOW.md](WORKFLOW.md) 是 routing、caller lifecycle、backend adapter、issue timing
與 adoption 的單一定義。使用既有 brief/receipt/note，不新增 mandatory task fields。
Repository rail 先選一個 manual 或 reconcile owner。Controller start、worker normal
done/fail 只適用 manual mode；reconcile mode 使用既有 lease/runner。這是 caller
convention，不是新 role enforcement，也不覆蓋 runtime 權限／controlled refusal。

```text
BundlePlan prose -> existing task brief / issue acceptance
DeliveryFacts    -> task receipt / PR handoff -> explicit context file -> review DATA
ReviewBinding    -> existing review/session records + --resume
EvidenceRef      -> existing edda run / CI / source / PR findings
SliceReadiness   -> existing program/PR delivery record
                         |
                         v
                canonical edda review merge
```

### Proposed optional product input: `edda review --context-file <path>`

A2 新增的唯一 CLI option。它接收明確選取的 UTF-8 純文字／Markdown，不要求
DeliveryFacts JSON，也不替換 acceptance/spec。相對路徑以 caller repository cwd 解析，
不是新建 review worktree。只讀該檔，不遞迴讀 references、不自動抓 PR/transcript。

- 上限 32 KiB；用 bounded read（最多上限 + 1 byte）讀一次到記憶體。
  不接受 directory、symlink 或非 regular file；避免 pipe/device 無界讀取。
- 缺檔、無權讀取、非 UTF-8、非 regular、過大：整份 context 不注入，visible warning
  與 existing notes 記錄原因；不截成看似完整的內容，不讓 optional context 變 launch gate。
- 有效輸入：同一 buffer 計算 SHA-256 並 JSON-escape 後放入獨立 DATA section，
  規則／工具權限／SPEC trust 都不變；context 內的 SHA 不是 prepared subject identity。
- 注入摘要說明 current actual head、digest 與「untrusted supporting context」。Digest
  僅識別讀到的 bytes，不證明真實性或權限。existing notes 留 digest 或 omission reason；
  不新增 ledger/SDK fields。Tests 必須從 fake launcher 與 persisted notes 看到它。
- First/resume/replacement 均可用；不要求同一檔名或同 digest 才能 resume。新 head 需
  更新 facts；舊 facts 仍當可能過期的 data，source verification 不可省略。
- No flag：維持原行為，不自動尋找任何檔案。Old binary 不支援時 caller 先 capability
  check，再省略 flag 並揭露限制，或選既有 permitted direct-review route。

這個 option 是規劃中功能，不是目前已支援的命令。Product `--spec` 保留 acceptance
含義，`--trust-spec` 不用來載入作者說明。舊 clients/events/prompt-file 路徑不變。

Reviewer 的下一輪必須先比對 actual head；prior findings 中的命令／prose 不直接執行。
相同 native session 可延續上下文，不代表舊 SHA 的 verdict 對新 SHA 有效。

## 6. Canonical examples

### A known gate reused, without invented cost

```json
{
  "kind": "ci",
  "activity": "read",
  "subject": {
    "state": "known",
    "value": "cbafe8fad409cfad6523d4a12d4226bb3c316d30",
    "source": "GitHub CI run headSha"
  },
  "locator": "https://github.com/fagemx/edda/actions/runs/34598223091",
  "result": "pass"
}
```

這只證明該 source SHA 的已跑 job，不證明新 candidate 或被 skipped 的 fleet tests。

### A lost session, honestly replaced

```json
{
  "transport": "pi",
  "session": { "state": "unknown", "reason": "recorded native conversation missing" },
  "prior_head": {
    "state": "known",
    "value": "e6b3ba11b7f8462a39dad16ed3ea37ce445a9d6e",
    "source": "PR1143 Round 1"
  },
  "mode": "replacement",
  "replacement_reason": "native conversation cannot be resumed; new identity reads prior findings"
}
```

Replacement 需要新 identity；不能假造 restored session，也不應因此停住其他 review。

## 7. Compatibility and rollback

- A1 是 caller/guidance 改動；A2 加 optional review input，但不改 dispatch/review/task
  public JSON fields。Existing notes 承載 context provenance，缺資料不產生新 outcome。
- A3 改 base REVIEW 的 U3 含義與既有 fixture expectations；legacy posted verdict
  仍按其原格式解讀，不抹除舊 finding 或重寫歷史。
- Installed `.agents/` 是 generated/local state，不 force-track。Canonical generic flow
  在 `crates/edda-cli/src/skills/coord-orchestrate.md`；Edda repo 的同名 .claude skill
  改為相同 projection。其他 project-only skills 只保留輸入／角色特定工作與薄路由。
  現有 init 不會更新 existing files；採用與版本可見性按 WORKFLOW §7，不自動覆寫。
- 發現 regression 用新修正/revert commit 回復該 scope；不 force push、reset peer tree、
  刪來源或恢復直接 gh merge。尚未 merge 的 candidate 保留。
- B 使用 additive continuity API、legacy checkpoint compatibility；不做 destructive migration。

**契約保護事實與副作用，不要求模型多填一套表。**
