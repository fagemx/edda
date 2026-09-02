# c1-shell-precedence

- class: `code-risk`
- severity: P0
- expected finding（一行）：`fast_build "$TMP" || cleanup "$TMP" && git rm -rf . --quiet`——POSIX sh 的 `||` 與 `&&` **同優先序、左結合**，此行解析為 `(fast_build || cleanup) && git rm -rf .`：只要左群組成功（**包含 fast_build 成功的正常路徑**，也包含 fast_build 失敗但 cleanup 成功），就會在 repo 根目錄執行破壞性的 `git rm -rf .`，把已追蹤的整棵樹刪掉。修正形狀是把後兩者括起來：`fast_build "$TMP" || { cleanup "$TMP" && git rm -rf . --quiet; }`。

實測（`sh` 三態）：`true || false && echo RAN` 印出 `RAN`；`true || { false && echo RAN; }`
不印任何東西；`false || true && echo RAN` 印出 `RAN`；`false || false && echo RAN` 不印、
list 退出碼 1（在 `set -e` 下會讓腳本提前結束，`slow_build` 不會跑——次要缺陷）。

## 評分提示（給校準評分者，不進 brief）

- caught：寫出 `(fast_build || cleanup) && git rm -rf .` 這個解析（或等價地說明 `||`/`&&`
  同優先序左結合），**並指出 `git rm -rf .` 在 fast_build 成功的正常路徑上也會執行**。
- caught 但嚴重度不符：解析樹正確、但只列 fast_build 失敗那條觸發路徑，漏掉成功路徑。
- missed：只泛稱「有破壞性指令」「刪除要小心」，或沿用「只有 fast_build 失敗時才刪」的
  錯誤解析，未指出左結合造成的非預期執行路徑。
- false positive：對 `slow_build`、`mktemp` 等無害行提出 P0 級主張。
- 註：把後兩者括起來只修好**分組**；`git rm -rf .` 作用在 repo 根而非 `$TMP`，與註解宣稱的
  「清掉 temp dir」仍不符，指出這點算加分不算 FP。
