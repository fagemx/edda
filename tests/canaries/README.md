# tests/canaries — 審查金絲雀集 v0

金絲雀是一個**已知答案的小 diff**：給審查引擎一份 diff，我們事先知道正確的
finding 是什麼（`expected.md`）。引擎定期對金絲雀集跑審查 → 得到
**引擎 × 類別抓取率**，這是「引擎可替換而效果不變」的量測基礎
（裁定 `fleet.review-engine=replaceable-by-qualification-not-brand`；
設計文件：`docs/superpowers/specs/2026-09-02-substitutable-reviewer-design.md`）。

## 目錄格式

    tests/canaries/<class>/<name>/
      fixture/        pre-state 檔案（canary 需要的合成事實來源；可為空/省略）
      diff.patch      統一 diff，路徑相對 repo 根，`git apply -p1` 可套用
      expected.md     class、severity、一行預期 finding、評分提示

- `<class>` 目前兩類：`code-risk`、`docs-skills`（類別定義見設計文件 §1.1）。
- `diff.patch` 一律只**新增或修改** `canaries-fixture/<name>/` 下的合成檔案，
  不碰真實 repo 檔案——金絲雀必須對任何 repo 狀態可重現。
- `fixture/` 是 diff 的**事實來源**（例如一份合成帳本、一份合成 CLI --help），
  先於 diff 提交，讓引擎在 diff 外能交叉查證。

## 現有金絲雀（v0，2026-09-02）

| id | class | severity | 測什麼 |
|---|---|---|---|
| c1-shell-precedence | code-risk | P0 | shell `A \|\| B && git rm -rf .` 優先序炸彈 |
| c2-stale-ratify-claim | docs-skills | P1 | 文件宣稱 unratified，帳本已有 ratify 事件 |
| c3-nonexistent-flag | docs-skills | P1 | runbook 命名不存在的 CLI 旗標 |
| c4-merge-authority-contradiction | docs-skills | P0 | skill 指令違反合併權限（跳過審查、`--delete-branch`） |
| c5-write-end-no-reader | code-risk | P1 | 新 `pub fn` 在 diff 內無呼叫端（binary crate） |

## 如何跑一次校準（calibration run）

在一個 **$TEMP 的 throwaway clone** 上做，不在工作 worktree：

```sh
WT=<this worktree>
CLONE="$TMPDIR/edda-calib-$$"
git clone "$WT" "$CLONE" && cd "$CLONE"
git checkout -b calib-canary-v0 origin/main

# 1. fixture pre-state commit（事實來源先進）
mkdir -p canaries-fixture
cp -r "$WT"/tests/canaries/docs-skills/c2-stale-ratify-claim/fixture \
      canaries-fixture/c2-stale-ratify-claim
cp -r "$WT"/tests/canaries/docs-skills/c3-nonexistent-flag/fixture \
      canaries-fixture/c3-nonexistent-flag
git add canaries-fixture
git commit -m "calibration: canary fixture pre-state"

# 2. canary commit（審查目標）
git apply "$WT"/tests/canaries/code-risk/c1-shell-precedence/diff.patch
git apply "$WT"/tests/canaries/docs-skills/c2-stale-ratify-claim/diff.patch
git apply "$WT"/tests/canaries/docs-skills/c3-nonexistent-flag/diff.patch
git apply "$WT"/tests/canaries/docs-skills/c4-merge-authority-contradiction/diff.patch
git apply "$WT"/tests/canaries/code-risk/c5-write-end-no-reader/diff.patch
git add -A
git commit -m "calibration: canary set v0"

# 3. 審查目標 diff
git diff HEAD~1..HEAD > /tmp/canary-v0.diff
```

然後對每個引擎，用審查 brief 模板 v1
（`docs/superpowers/specs/2026-09-02-reviewer-brief-template-v1.md`）跑
**一次唯讀審查**，引擎的 cwd 是上述 clone：

- pi 系（sol／gemini／glm）：
  `pi -p --model <model> --exclude-tools edit,write --session-id calib-<engine> "<brief>"`
- Opus（**只能走 Claude Code**，裁定 `fleet.claude-subscription-transport=claude-code-only`）：
  `claude -p --model opus --allowedTools "Read,Grep,Glob,Bash(git *),Bash(sh *)" --output-format json "<brief>"`

## 評分

對每顆金絲雀、每個引擎記一格：**caught / missed / false positive**
（評分基準在各 canary 的 `expected.md`）。抓取率進帳本，構成引擎 × 類別表；
合格門檻與重校節奏見設計文件 §1.3。

## 線只升不降

sol 抓到而其他引擎漏掉的 finding，自動成為新金絲雀
（裁定 `fleet.review-engine`：「sol 抓到而他人漏的即成新金絲雀」）。
金絲雀集只增不減；移除一顆金絲雀等於降線，需操作者裁定。
