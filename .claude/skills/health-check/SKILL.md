---
name: health-check
description: "Periodic codebase health assessment — 5 dimensions, scorecard, trend tracking"
context: fork
---

# Health Check Skill

定期健康檢查，用 5 個維度快速評估代碼庫狀態，追蹤技術債趨勢。

## 設計理念

技術債最大的問題不是「存在」，而是「太晚發現」。這個 skill 的目標是：
- **快速**：自動化部分 < 5 分鐘
- **可比較**：每次產出相同格式的 scorecard，方便追蹤趨勢
- **可行動**：產出 Top 3 最急迫的行動項目

## Usage

```
args: ""              # Full 5-dimension health check
args: "quick"         # Fast mode: only automated metrics, skip manual analysis
args: "compare"       # Compare with last scan, show trend
args: "dim:<N>"       # Run single dimension (1-5)
```

## Workflow

### Step 0: Setup

```bash
# Create output directory
SCAN_DATE=$(date +%Y%m%d)
SCAN_DIR="/tmp/health-check-${SCAN_DATE}"
mkdir -p "$SCAN_DIR"

# Check for previous scans for trend comparison
PREV_SCAN=$(ls -td /tmp/health-check-* 2>/dev/null | grep -v "$SCAN_DIR" | head -1)
```

### Step 1: Dimension 1 — 可讀性 (Readability)

自動化指標：

**1a. 函數長度分佈**
```bash
# Find functions >50 lines in Rust files
# Use grep to find fn definitions, then count lines to next fn or closing brace
# Simplified: count files with functions >50 lines
```
- 用 Grep 搜尋 `crates/` 下所有 `.rs` 檔案
- 用 `wc -l` 統計每個檔案行數
- 計算 >200 行、>500 行、>800 行的檔案數

**1b. 命名品質**
```bash
# Search for single-char variable names (excluding loop vars i,j,k and common patterns)
# Search for vague names: tmp, data, result, handle, process, info, manager
```
- Grep 搜尋 `let [a-z]\b`（排除 `let i`, `let j`, `let k`）
- Grep 搜尋 `fn (handle|process|manage|do_)\w+`

**1c. 抽象層數**
- 從 `main.rs` 追蹤到核心邏輯的跳轉次數（手動抽樣 2-3 個 command）

**評分標準：**
| 分數 | 條件 |
|------|------|
| 5 | >500行檔案 = 0, 模糊命名 < 5 |
| 4 | >500行檔案 ≤ 2, 模糊命名 < 10 |
| 3 | >500行檔案 ≤ 5, 模糊命名 < 20 |
| 2 | >500行檔案 ≤ 10, 模糊命名 < 30 |
| 1 | >500行檔案 > 10 或 模糊命名 > 30 |

---

### Step 2: Dimension 2 — 耦合度 (Coupling)

**2a. PR 碰模組數（Git 考古）**
```bash
# Last 20 merged PRs or commits on main: how many crates does each touch?
git log --oneline --name-only -20 main | \
  grep "^crates/" | \
  sed 's|crates/\([^/]*\)/.*|\1|' | \
  sort -u
# Per-commit analysis
git log --format="%H" -20 main | while read hash; do
  echo -n "$hash: "
  git diff-tree --no-commit-id --name-only -r $hash | \
    grep "^crates/" | sed 's|crates/\([^/]*\)/.*|\1|' | sort -u | wc -l
done
```

**2b. 循環依賴**
```bash
# Check Cargo.toml dependencies between workspace crates
# Build adjacency list and detect cycles
```
- 讀取每個 crate 的 `Cargo.toml`，提取 workspace 內部依賴
- 檢測是否有 A→B→A 循環

**2c. God Module 檢測**
```bash
# Which crate is imported by most other crates?
grep -r "edda-" crates/*/Cargo.toml | grep -v "name = " | \
  sed 's/.*edda-/edda-/' | sed 's/".*//' | sort | uniq -c | sort -rn
```

**2d. Hotspot 檔案（高 churn）**
```bash
# Files changed most frequently in last 30 commits
git log --oneline --name-only -30 main | \
  grep "\.rs$" | sort | uniq -c | sort -rn | head -15
```

**評分標準：**
| 分數 | 條件 |
|------|------|
| 5 | PR平均碰 ≤1.5 crate, 無循環, 無 god module |
| 4 | PR平均碰 ≤2 crate, 無循環 |
| 3 | PR平均碰 ≤3 crate, ≤1 循環 |
| 2 | PR平均碰 ≤4 crate, ≤2 循環 |
| 1 | PR平均碰 >4 crate 或 >2 循環 |

---

### Step 3: Dimension 3 — 測試信心 (Test Confidence)

**3a. 測試基本指標**
```bash
# Test count
cargo test -- --list 2>&1 | grep ": test$" | wc -l

# Test execution time
time cargo test 2>&1

# Flaky detection: run tests twice, compare results
cargo test 2>&1 | tail -5
```

**3b. 測試覆蓋率（proxy）**
```bash
# Count source files vs files with test modules
SRC_FILES=$(find crates -name "*.rs" -not -path "*/tests/*" -not -name "test_*" | wc -l)
TESTED_FILES=$(grep -rl "#\[cfg(test)\]" crates/ --include="*.rs" | wc -l)
echo "Coverage proxy: $TESTED_FILES / $SRC_FILES"
```

**3c. 測試品質抽查**
```bash
# Tests that only assert is_ok/is_err without checking values
grep -rn "assert!(.*\.is_ok())" crates/ --include="*.rs" | grep -v "unwrap"
grep -rn "assert!(.*\.is_err())" crates/ --include="*.rs"
```

**3d. 核心路徑測試覆蓋**
- 手動檢查：`main.rs` 裡的每個 command 是否有對應的 integration test
- 手動檢查：`lib.rs` 的 public API 是否都有測試

**評分標準：**
| 分數 | 條件 |
|------|------|
| 5 | 覆蓋 >80%, 0 flaky, 全測 <2min, 弱斷言 <5% |
| 4 | 覆蓋 >60%, 0 flaky, 全測 <5min |
| 3 | 覆蓋 >40%, ≤2 flaky, 全測 <10min |
| 2 | 覆蓋 >20%, ≤5 flaky |
| 1 | 覆蓋 <20% 或 >5 flaky 或 全測 >15min |

---

### Step 4: Dimension 4 — 可觀測性 (Observability)

**4a. Error Handling 品質**
```bash
# Swallowed errors: empty catch / ignored Result
grep -rn "let _ =" crates/ --include="*.rs" | grep -v test
grep -rn "\.ok();" crates/ --include="*.rs" | grep -v test

# Bare ? without context
# Count ? operators vs ? with .context() or .map_err()
BARE_Q=$(grep -rn "?\s*;" crates/ --include="*.rs" | grep -v test | grep -v "context\|map_err" | wc -l)
CONTEXT_Q=$(grep -rn "context(\|map_err(" crates/ --include="*.rs" | grep -v test | wc -l)
echo "Bare ?: $BARE_Q, With context: $CONTEXT_Q"
```

**4b. Unwrap/Expect 在 production code**
```bash
grep -rn "\.unwrap()" crates/ --include="*.rs" | grep -v test | grep -v "#\[cfg(test)\]" | wc -l
grep -rn "\.expect(" crates/ --include="*.rs" | grep -v test | grep -v "#\[cfg(test)\]" | wc -l
```

**4c. Structured Logging**
```bash
# Check for tracing usage vs println/eprintln
grep -rc "tracing::" crates/ --include="*.rs" | grep -v ":0$" | wc -l
grep -rc "println!\|eprintln!" crates/ --include="*.rs" | grep -v ":0$" | wc -l
```

**4d. Error 型別品質**
```bash
# Typed errors vs String errors
grep -rn "thiserror\|#\[error(" crates/ --include="*.rs" | wc -l
grep -rn "anyhow::Error\|Box<dyn.*Error>" crates/ --include="*.rs" | wc -l
```

**評分標準：**
| 分數 | 條件 |
|------|------|
| 5 | 0 swallowed, >80% context ?, 0 println in lib, typed errors |
| 4 | ≤3 swallowed, >60% context ?, ≤2 println |
| 3 | ≤5 swallowed, >40% context ? |
| 2 | ≤10 swallowed, >20% context ? |
| 1 | >10 swallowed 或 <20% context ? |

---

### Step 5: Dimension 5 — 交付趨勢 (Delivery Trend)

**5a. Commit 頻率趨勢**
```bash
# Commits per week for last 4 weeks
for i in 0 1 2 3; do
  START=$(date -d "$((i+1)) weeks ago" +%Y-%m-%d)
  END=$(date -d "$i weeks ago" +%Y-%m-%d)
  COUNT=$(git log --oneline --after="$START" --before="$END" main | wc -l)
  echo "Week -$i: $COUNT commits"
done
```

**5b. Bug-fix 佔比**
```bash
# fix: commits vs total in last 30 commits
TOTAL=$(git log --oneline -30 main | wc -l)
FIXES=$(git log --oneline -30 main | grep -i "^[a-f0-9]* fix" | wc -l)
echo "Fix ratio: $FIXES / $TOTAL"
```

**5c. Revert 頻率**
```bash
git log --oneline -50 main | grep -i "revert" | wc -l
```

**5d. PR Cycle Time（如果有 GitHub CLI）**
```bash
# Average time from PR creation to merge for last 10 merged PRs
gh pr list --repo fagemx/edda --state merged --limit 10 \
  --json createdAt,mergedAt \
  --jq '.[] | "\(.createdAt) \(.mergedAt)"'
```

**評分標準：**
| 分數 | 條件 |
|------|------|
| 5 | 穩定/加速交付, fix <15%, 0 revert |
| 4 | 穩定交付, fix <20%, ≤1 revert |
| 3 | 微降, fix <30%, ≤2 revert |
| 2 | 明顯降速, fix <40% |
| 1 | 持續降速 或 fix >40% 或 >3 revert |

---

### Step 6: 產出 Scorecard

將結果寫入 `$SCAN_DIR/scorecard.md`：

```markdown
# Health Check Scorecard

**Date:** {YYYY-MM-DD}
**Branch:** main
**Commit:** {HEAD short hash}
**Total .rs files:** {count}
**Total lines:** {count}

## Scores

| # | Dimension | Score | Trend | Key Signal |
|---|-----------|-------|-------|------------|
| 1 | 可讀性 (Readability) | {1-5}/5 | {↑↓→} | {one-line summary} |
| 2 | 耦合度 (Coupling) | {1-5}/5 | {↑↓→} | {one-line summary} |
| 3 | 測試信心 (Test Confidence) | {1-5}/5 | {↑↓→} | {one-line summary} |
| 4 | 可觀測性 (Observability) | {1-5}/5 | {↑↓→} | {one-line summary} |
| 5 | 交付趨勢 (Delivery Trend) | {1-5}/5 | {↑↓→} | {one-line summary} |
| | **Overall** | **{avg}/5** | | |

## Trend (vs previous scan)

{If previous scan exists:}
| Dimension | Previous | Current | Delta |
|-----------|----------|---------|-------|
| ... | ... | ... | +1 / -1 / = |

{If no previous scan:}
> First scan — no trend data available yet.

## Top 3 Actions

1. **[Dimension] {具體問題}** — {為什麼急} → {建議動作}
2. **[Dimension] {具體問題}** — {為什麼急} → {建議動作}
3. **[Dimension] {具體問題}** — {為什麼急} → {建議動作}

## Raw Metrics

### Dim 1: Readability
- Files >200 lines: {n}
- Files >500 lines: {n}
- Files >800 lines: {n}
- Vague naming hits: {n}

### Dim 2: Coupling
- Avg crates touched per commit: {n}
- Circular dependencies: {n}
- Top hotspot: {file} ({n} changes in 30 commits)
- God module: {crate} (depended by {n} crates)

### Dim 3: Test Confidence
- Test count: {n}
- Test time: {seconds}s
- Coverage proxy: {n}/{m} files ({pct}%)
- Weak assertions (is_ok/is_err only): {n}
- Flaky tests: {n}

### Dim 4: Observability
- Swallowed errors (let _ =, .ok()): {n}
- Bare ? (no context): {n}
- ? with context: {n}
- Context ratio: {pct}%
- Unwrap in prod: {n}
- println/eprintln in lib: {n}

### Dim 5: Delivery Trend
- Commits (last 4 weeks): {w3}, {w2}, {w1}, {w0}
- Fix ratio (last 30): {n}/{m} ({pct}%)
- Reverts (last 50): {n}

---
*Generated by health-check skill*
```

### Step 7: 存檔 & 對比

```bash
# Save scorecard
# Also save machine-readable version for trend comparison
cat > "$SCAN_DIR/metrics.json" << EOF
{
  "date": "{date}",
  "commit": "{hash}",
  "scores": {
    "readability": {n},
    "coupling": {n},
    "test_confidence": {n},
    "observability": {n},
    "delivery_trend": {n}
  },
  "metrics": {
    "files_over_500": {n},
    "circular_deps": {n},
    "test_count": {n},
    "test_time_s": {n},
    "swallowed_errors": {n},
    "bare_question_marks": {n},
    "unwrap_in_prod": {n},
    "fix_ratio_pct": {n}
  }
}
EOF
```

如果有前次掃描，自動讀取 `$PREV_SCAN/metrics.json` 做差異比較。

### Step 8: 向使用者報告

輸出 scorecard 摘要（不是完整 raw metrics），重點突出：
1. 總分和各維度分數
2. 趨勢箭頭（如有前次掃描）
3. Top 3 行動項目
4. 告知完整報告路徑

---

## Quick Mode

`args: "quick"` 只跑自動化指標（Step 1a/1b, 2a/2c/2d, 3a/3b/3c, 4a/4b/4c/4d, 5a/5b/5c），跳過手動抽樣。

## Compare Mode

`args: "compare"` 只做趨勢對比，不重新掃描。讀取最近兩次 `metrics.json` 做 diff。

## Single Dimension Mode

`args: "dim:3"` 只跑第 3 維度（測試信心），適合修完 bug 後快速驗證。

---

## 建議使用頻率

| 場景 | 頻率 | 模式 |
|------|------|------|
| 日常開發 | 每週一次 | `quick` |
| Sprint 結束 | 每 2 週 | full |
| 大重構前後 | 重構前 + 後 | full + compare |
| PR review 發現問題 | 當下 | `dim:<N>` |

---

## References

- tech-debt skill: 靜態 bad smell 掃描（更深入的 pattern 分析）
- code-quality skill: PR-level code review
- local-testing skill: 測試環境設定
