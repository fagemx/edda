---
name: plan-decompose
description: "RETIRED — absorbed by fleet-epic-split. Use fleet-epic-split to turn a planning doc, epic issue, or ledger decision into issues."
---

# RETIRED — use fleet-epic-split

plan-decompose 已於 #599 併入 `fleet-epic-split`；本檔保留只為舊連結可解析。

原本讓 plan-decompose 有價值的四件事都活在 fleet-epic-split 的流程裡：輸入
形狀放寬（epic issue／規劃文件／決策 key／貼上摘要，見其「輸入」節）、去重
程序（文件內 `#N` 引用 + `edda ask` + `edda search` + open-issue 模糊比對，
見其步驟 ③）、建單前確認表（步驟 ④）、交叉引用與 provenance 回連（步驟
⑥⑦）。其輸出 body 一律照 `issue-intake/templates.md` 的單一 ready-bar 契約
（含 Wiring audit 槽與 `Predicted surface` 欄），不再使用舊的並行 body 格式。

要把規劃拆成 issue，用
[`fleet-epic-split/SKILL.md`](../fleet-epic-split/SKILL.md)。
