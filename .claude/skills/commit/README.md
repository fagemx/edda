# Commit Skill 文件分類索引

本目錄包含完整的 commit 規範和工具說明，按內容分類如下：

## 📚 文件分類

### 1️⃣ **快速開始** - 主要工作流程
**文件：** `SKILL.md`

**內容：**
- Commit skill 的完整使用說明
- Pre-commit 檢查流程（format, lint, test）
- Commit message 格式規範
- 快速參考表格

**適合：** 第一次使用或快速查閱

---

### 2️⃣ **類型定義** - 所有 Commit 類型詳細說明
**文件：** `types.md`

**內容：**
- **會觸發版本發布的類型：**
  - `feat:` - 新功能（Minor 版本）
  - `fix:` - 錯誤修復（Patch 版本）
  - `deps:` - 依賴更新（Patch 版本）
  - `<any>!` - 破壞性變更（Major 版本）

- **不會觸發版本發布的類型：**
  - `docs:` - 文件更新
  - `style:` - 程式碼風格
  - `refactor:` - 重構
  - `test:` - 測試
  - `chore:` - 建置/工具
  - `ci:` - CI 設定
  - `perf:` - 效能優化
  - `build:` - 建置系統
  - `revert:` - 還原變更

- Scope（範圍）說明
- 專案常用 scope 列表

**適合：** 需要了解特定類型的使用時機和規則

---

### 3️⃣ **範例參考** - 好壞範例對比
**文件：** `examples.md`

**內容：**
- ✅ 好的 commit message 範例
- ❌ 錯誤的 commit message 範例
- 各種場景的實戰範例：
  - 新功能開發
  - 錯誤修復
  - 重構
  - 文件更新
  - 測試
  - 破壞性變更
- 多行 commit message 範例
- Scope 使用範例
- 反模式（Anti-patterns）說明
- Commit message 檢查清單

**適合：** 需要參考實際範例或學習最佳實踐

---

### 4️⃣ **版本管理** - 版本發布規則
**文件：** `release-triggers.md`

**內容：**
- Semantic Versioning 說明
- 版本發布矩陣（哪些類型觸發哪些版本）
- release-please 運作機制
- 決策樹：我的 commit 會觸發發布嗎？
- 策略性選擇 commit 類型
- 特殊情況：重構如何觸發發布
- PR 中多個 commit 的處理
- 破壞性變更的標記方式
- 實戰場景範例
- FAQ

**適合：** 需要控制版本發布或了解版本策略

---

## 🎯 使用建議

### 第一次使用
1. 閱讀 `SKILL.md` 了解基本流程
2. 參考 `examples.md` 看範例
3. 需要時查閱 `types.md` 了解類型規則

### 日常使用
- **快速查詢類型：** → `types.md`
- **需要範例參考：** → `examples.md`
- **控制版本發布：** → `release-triggers.md`
- **忘記流程：** → `SKILL.md`

### 進階使用
- **策略性選擇類型：** → `release-triggers.md` § Strategic Commit Type Selection
- **處理特殊情況：** → `release-triggers.md` § Special Case: Refactoring
- **破壞性變更：** → `types.md` § Breaking Changes + `release-triggers.md` § Breaking Changes

---

## 📋 快速決策流程

```
我需要 commit，該用什麼類型？

1. 這是破壞性變更嗎？
   ├─ 是 → 使用 `<type>!` 或 `BREAKING CHANGE:` footer
   └─ 否 → 繼續

2. 這是新功能嗎？
   ├─ 是 → 使用 `feat:` （會觸發 Minor 版本）
   └─ 否 → 繼續

3. 這是錯誤修復嗎？
   ├─ 是 → 使用 `fix:` （會觸發 Patch 版本）
   └─ 否 → 繼續

4. 這是依賴更新嗎？
   ├─ 是 → 使用 `deps:` （會觸發 Patch 版本）
   └─ 否 → 繼續

5. 這是重構且需要發布嗎？
   ├─ 是 → 使用 `fix: refactor ...` （會觸發 Patch 版本）
   └─ 否 → 使用 `refactor:` （不會觸發版本）

6. 其他情況：
   - `docs:` - 文件更新
   - `test:` - 測試
   - `chore:` - 建置/工具
   - `ci:` - CI 設定
   - `style:` - 程式碼風格
   - `perf:` - 效能優化
   - `build:` - 建置系統
   - `revert:` - 還原變更
```

---

## 🔗 相關資源

- **專案標準：** 見 `CLAUDE.md`
- **Pull Request 流程：** 見 `.claude/skills/pull-request/`
- **程式碼品質：** 見 `.claude/skills/code-quality/`

---

## 📝 文件維護

當需要更新 commit 規範時：
1. 更新類型定義 → `types.md`
2. 更新範例 → `examples.md`
3. 更新版本規則 → `release-triggers.md`
4. 更新主文件 → `SKILL.md`
5. 更新本索引 → `README.md`
