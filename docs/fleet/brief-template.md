# Lane brief 範本（2026-09-10）

控制者寫給 lane 的 brief。四個段落，順序固定。風格規則是帳本
`brief.style`／memory「強模型指引=事實塊+附理由原則」：**事實給足、原則附理由、只機械化競態與權限**,
不寫程序——強模型照程序走會比照原則走差。

這份範本記的是版面與慣例。**每一條慣例底下都有一次真實的失敗**;沒有踩過的規則不寫進來。

---

## 段落一 — 事實塊

開頭一律標明**控制者已驗證於哪個 SHA**:

```
## Fact block（controller verified at origin/main = <40 位完整 SHA>）
```

這行不是禮貌用語，它決定 lane 要不要重查。沒有這行，lane 會把控制者的敘述當傳聞再查一遍，
一輪的預算就這樣沒了。標了，它就直接用。

事實塊裡要有：

- 缺陷的**確切座標**——`crates/…/foo.rs:123`,不是「在 export 那邊」。
- **C5 selector 空不空**:touched crate 全在 CI 的 Windows 7-crate 子集裡就明寫「C5 selector is empty
  ——no focused Windows-gap run is owed」。不寫，lane 會自己跑一次不欠的閘。
- 相關的既有裁定，用 key 引(`fleet.ci-flake-carrier=…`),不要複述內容。
- 這張單和哪張 PR／哪個既有機制是同一個 scheme——例如「這是 PR #1017 建立的『編碼是全稱的』
  性質剩下的最後一個洞，動手前先讀 #1017 的 diff」。lane 會把修法接進既有 scheme 而不是擺在旁邊。

**控制者查到而 issue 沒寫的東西，要明說是控制者加的。** GH-1033 的 issue 只點名一處
`contains("OK")`,實際有兩處一模一樣的；brief 明寫「`:296` 是控制者加進範圍的，不是 lane 越界，
請在 PR body 說明這是刻意的範圍判斷」，審查才不會當 scope creep 記在實作頭上。

---

## 段落二 — 這輪的重點

修復 brief 寫**要修什麼、為什麼那樣修不行**。審查 brief 寫**問題，不寫答案**。

這是整份範本最重要的一條。控制者心裡有懷疑時，把懷疑寫成問題交出去，不要寫成結論：

> 追每個 `poll_until` 呼叫點在 `Ok(false)` 之後發生什麼。如果答案還是「測試失敗」，就直說，
> 然後判斷這個 PR 畫的區別撐不撐得住。

PR #1112 就是這樣寫的，回來的答案比控制者自己的準——控制者只想到「上限還在斷言位置」，
審查補上了「但這在 GH-1078 的 doneWhen 之內，不在 GH-1031 的」。寫成結論就拿不到後半句。

PR #1108 的 P0 也是問出來的：「寫端讀端真的互逆嗎？**檢查這個宣稱，不要接受它。**」

後續輪次的重點段還要寫**什麼已經定案、不可重開**——審查契約第 2 條：後輪的 blocker 必須是
fix 造成的或先前不可觀測的。不寫，replacement verifier 會從頭再審一遍。

---

## 段落三 — 驗證預算

只給 L0,明列指令，並明寫**不要跑什麼**:

```
cargo fmt --all --check
cargo clippy -p <crate> --all-targets -- -D warnings
cargo test -p <crate>
sh scripts/lint-file-length.sh --tree
```

- **`cargo test --workspace` 明文禁止。** L1 是 exact-head CI 加 verifier 的重點跑，不是 lane 的事(R20)。
- **`scripts/lint-file-length.sh --tree` 要註明它很慢**:389 個追蹤 `.rs`,每檔 3 個 spawn
  (`git cat-file` 管到 `awk` 數行，再一個 `awk` 查天花板)= 每輪 **1,167 次 spawn**。實測四分鐘牆鐘
  只有 3.73 秒 CPU——它是 spawn-bound,不是 compute-bound。**前景、最後跑、只跑一次。**
- 不要求任何**重複跑**的證據。要求統計證據前先讀 `test.flake-evidence`
  ——重現不出來的 flake,證據標準是結構論證加一個釘住順序性質的測試。
  2026-09-09 有兩條 lane 燒在「拿出修好前後的失敗率」這個無法滿足的要求上：修好前是 11/11 全綠。

---

## 段落四 — Mechanized（只有競態與權限）

固定條目，照抄再依單調整：

1. **Build lane** — 指定 `worker-1|worker-2|verifier|verifier-2` 其中一條，寫成
   `export CARGO_TARGET_DIR="$LOCALAPPDATA/fleet-workstation/lanes/<lane>"`,並寫
   「**never an ad-hoc target directory, under any name, for any reason**」。
   不寫「under any name」這半句會被繞過：2026-09-09 一條 lane 建了
   `scratchpad/wave3/loadgen-target` 做 `rm -rf` 加十次完整 workspace 重建。
   不編譯的 docs lane 不指定，回報 `n/a`。
2. **用 `SKIP_CLIPPY=1` 提交**——先在自己的 lane 跑熱的 L0 clippy,再
   `SKIP_CLIPPY=1 git commit ...`。pre-commit hook 會重跑同一道 clippy,而它的 cargo
   **繼承不到 lane 的 `CARGO_TARGET_DIR`**(#1099),所以每次 commit 都在 worktree 裡冷建一個獨立
   `target/`——實測 0.76–0.85 GB、數十分鐘。這不是跳過驗證：同一道閘你剛跑過，CI 還會在三個 OS 再跑一次；
   被跳掉的只有「同一件事冷建第二遍」。其餘 hook 閘照跑。commit 後刪掉殘留的 `target/`。
3. **Peer-claimed 路徑**——逐條列出並行 lane 正在改的檔，寫「do not touch」。
4. **不刪任何 worktree 或分支。** 殘留的 `target/` 是唯一可刪的東西。
5. **一律前景。** 唯一合法的例外是 hook 的冷建。禁止的是**自製的工作**:負載產生器、
   重複量測迴圈、任何為了生出一個數字而燒機器的東西。
   2026-09-09 一條停住的 lane 留下三個孤兒負載程序(十次 workspace 重建、500×16 支 PowerShell、
   一個測試迴圈),把隔壁健康的 lane 餓死。
6. **每完成一段就 commit 並 push,不要留到最後。** 這台機器上的 lane 反覆在活做完、
   沒提交的狀態下停住。
7. **所有 GitHub 文字先寫成檔案，用 `--body-file` 傳。** 絕不把多行文字塞進 shell 雙引號——
   反引號會被當成指令替換執行。2026-09-09 一則 issue 留言的測試名稱就是這樣被吃掉的。
8. **Closing 關鍵字只寫 PR body,不要寫進 commit 訊息。** commit 訊息建立的 auto-link
   在推上去當下就成立，**從 PR body 拿掉關鍵字也撤不掉**;要撤只能改寫 commit,而 R7 禁止
   已推送的改寫。#1031 就是被一個後來被審查推翻的 commit 自動關掉的。
9. **R7**:已推送的 commit 不 amend、不 rebase、不 force push——要改就加新 commit。
10. Commit 結尾 `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`;
    PR body 結尾 `🤖 Generated with [Claude Code](https://claude.com/claude-code)`。
11. **開 PR 後停下，不要合併。** 回報 branch／完整 head SHA／CI run id／跑過的閘／沒做完的事。

---

## 審查 brief 的額外條目

- **READ-ONLY**:`git show <sha>:<path>` 與 `git grep <pat> <sha>`,不 checkout、不開 worktree、
  不 `gh pr checkout`、不跑 cargo。唯讀怎麼證見 `review.readonly-proof`。
- **共用 checkout 的 `main` 可能落後**——那樣 worktree 裡的 `REVIEW.md` 就是舊副本。
  控制者派審前應先把主 checkout 快轉；沒快轉就在 brief 裡叫審查者從
  `git show <base>:REVIEW.md` 讀。2026-09-09 主 checkout 落後 6 個 commit,`REVIEW.md` 差 31 行，
  舊版的 §1 在共用 checkout 裡是禁止 `gh pr checkout` 的。
- **CI 紅但是已知 flake 時要點名**:寫「若 `Test (windows-latest)` 紅在 `<測試名>`,那是 #NNNN,
  判環境問題，不要花一次 rerun」。
- **判決自己貼**(R23 形狀),控制者不代貼。回報判決行與 P0/P1 數。
- 不設 label、不寫 status、不合併。
