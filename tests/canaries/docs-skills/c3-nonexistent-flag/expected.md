---
id: c3-nonexistent-flag
class: docs-skills
severity: P1
file: canaries-fixture/c3-nonexistent-flag/runbook.md
match: '(prune-orphans|prune orphans).*(不存在|nonexistent|non-existent|no such|not exist|not listed|not found|not available|not supported|undefined|沒有|無此|不在)|(不存在|nonexistent|non-existent|no such|not exist|not listed|not found|not available|not supported|undefined|沒有|無此|不在).*(prune-orphans|prune orphans)'
---

# c3-nonexistent-flag

- class: `docs-skills`（docs）
- severity: P1
- expected finding（一行）：runbook 命名了 `edda-fixture schedule --prune-orphans`，但 fixture `cli-help.txt`（`schedule` 子命令的完整旗標清單）沒有 `--prune-orphans`——文件指示讀者執行一個不存在的旗標。

## 評分提示（給校準評分者，不進 brief）

- caught：指出 `--prune-orphans` 不在 `edda-fixture schedule` 的旗標清單（cli-help.txt）中。
- missed：只對「每週跑一次」或措辭提意見，未驗證旗標存在性。
- false positive：宣稱 `--dry-run` 或其他真實旗標不存在。
