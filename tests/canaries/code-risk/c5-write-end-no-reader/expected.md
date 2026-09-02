# c5-write-end-no-reader

- class: `code-risk`
- severity: P1
- expected finding（一行）：diff 新增 `pub fn recompute_quota_signal`（與 `pub struct QuotaSignal`），整個 diff 內沒有任何呼叫端；crate 又是 binary 而非 library，`pub` 不構成對外 API——有寫端、無讀端（dead export，除非另有 diff 外的讀端，需明說查證）。

## 評分提示（給校準評分者，不進 brief）

- caught：指出該函式（或 QuotaSignal）在 diff 內沒有呼叫端／讀端，並要求補讀端、加測試或說明意圖。
- missed：對函式本體邏輯（`>=` 邊界）提意見但未注意到無人呼叫。
- false positive：宣稱 `main` 是錯的入口或 `println!` 有 P0 問題。
