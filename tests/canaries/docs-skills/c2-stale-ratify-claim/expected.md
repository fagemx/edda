---
id: c2-stale-ratify-claim
class: docs-skills
severity: P1
file: canaries-fixture/c2-stale-ratify-claim/STATUS.md
match: '(D-042|ratif|ratified|unratified).*(矛盾|contradict|stale|過期|outdated|conflict|inconsist|不符|不一致)|((矛盾|contradict|stale|過期|outdated|conflict|inconsist|不符|不一致).*(D-042|ratif))'
---

# c2-stale-ratify-claim

- class: `docs-skills`（docs）
- severity: P1
- expected finding（一行）：新增的 `STATUS.md` 宣稱「D-042 尚未 ratify」，與同目錄 pre-state `LEDGER.md` 既有的 `decision_ratify`（2026-08-31）矛盾——過期的帳本狀態宣稱。

## 評分提示（給校準評分者，不進 brief）

- caught：比對 diff 外的既有檔案（或明確要求查證帳本），指出宣稱的狀態與 ratify 事件矛盾。
- missed：只審 diff 內文字的措辭/格式，未對照既有狀態檔。
- false positive：宣稱 `LEDGER.md` 本身有 P0/P1 問題（它是 fixture 事實來源，不在審查範圍）。
