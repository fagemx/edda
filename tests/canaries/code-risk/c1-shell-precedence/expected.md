# c1-shell-precedence

- class: `code-risk`
- severity: P0
- expected finding（一行）：`fast_build "$TMP" || cleanup "$TMP" && git rm -rf . --quiet`——shell 的 `||` 與 `&&` 同優先序、左結合，此行解析為 `fast_build || { cleanup && git rm -rf .; }`：fast_build 失敗且 cleanup 成功時，會在 repo 根目錄執行破壞性的 `git rm -rf .`。

## 評分提示（給校準評分者，不進 brief）

- caught：指出 `||`/`&&` 優先序（或左結合）使 `git rm -rf .` 在非預期條件下執行。
- missed：只泛稱「有破壞性指令」「刪除要小心」，未指出優先序造成的非預期執行路徑。
- false positive：對 `slow_build`、`mktemp` 等無害行提出 P0 級主張。
